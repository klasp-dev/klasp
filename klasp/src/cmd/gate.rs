//! `klasp gate` — the hot path. Called by the bash shim with Claude Code's
//! tool-call JSON on stdin.
//!
//! Implements the seven-step flow from [docs/design.md §6]. The flow is
//! deliberately linear — no async runtime, no concurrent checks; v0.2 will
//! add `rayon`-based parallelism when the test surface is broad enough to
//! catch race regressions. **Every tooling failure fails open** with a
//! single stderr notice and exit 0; only a `Verdict::Fail` aggregated from
//! actual check results returns exit 2 to deny the tool call.
//!
//! The seven fail-open exit points:
//!
//! 1. Schema env var unreadable / mismatched → notice, exit 0.
//! 2. Stdin unreadable → notice, exit 0.
//! 3. Stdin not parseable as a `GateInput` → notice, exit 0.
//! 4. `tool_input.command` absent or fails trigger classification → silent exit 0
//!    (these are normal pass-through cases, not failures).
//! 5. `klasp.toml` missing or unparseable → notice, exit 0.
//! 6. A check has no registered source, or its source's `run()` errored
//!    → per-check notice, the gate runs the rest.
//! 7. `Verdict::merge` → exit 2 only when blocking, else 0.

use std::io::{self, Read, Write};
use std::process::ExitCode;

use klasp_core::{
    CheckConfig, ConfigV1, GateProtocol, GitEvent, RepoState, Trigger, Verdict, VerdictPolicy,
};

use crate::cli::GateArgs;
use crate::git;
use crate::sources::SourceRegistry;

/// Stderr prefix for every fail-open notice. Single source of truth makes
/// log-grep'ing `klasp-gate:` reliable across the binary.
const NOTICE_PREFIX: &str = "klasp-gate:";

pub fn run(_args: &GateArgs) -> ExitCode {
    let mut stderr = io::stderr().lock();
    match gate(&mut stderr) {
        Outcome::Pass => ExitCode::SUCCESS,
        Outcome::Block => ExitCode::from(2),
    }
}

/// Internal outcome distinct from `ExitCode` so the flow is testable end to
/// end if a future test wants to drive the runtime in-process.
enum Outcome {
    Pass,
    Block,
}

fn gate<W: Write>(stderr: &mut W) -> Outcome {
    // 1. Schema handshake — env var, not stdin (see design §3.3).
    match GateProtocol::read_schema_from_env() {
        Ok(env_value) => {
            if let Err(e) = GateProtocol::check_schema_env(env_value) {
                let _ = writeln!(
                    stderr,
                    "{NOTICE_PREFIX} schema mismatch ({e}), skipping. \
                     Re-run `klasp install` to update the hook."
                );
                return Outcome::Pass;
            }
        }
        Err(e) => {
            let _ = writeln!(
                stderr,
                "{NOTICE_PREFIX} could not read KLASP_GATE_SCHEMA ({e}), \
                 skipping. Re-run `klasp install` to regenerate the hook."
            );
            return Outcome::Pass;
        }
    }

    // 2. Parse stdin (fail-open on read or parse error).
    let mut buf = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut buf) {
        let _ = writeln!(
            stderr,
            "{NOTICE_PREFIX} could not read stdin ({e}), skipping."
        );
        return Outcome::Pass;
    }

    let input = match GateProtocol::parse(&buf) {
        Ok(i) => i,
        Err(e) => {
            let _ = writeln!(
                stderr,
                "{NOTICE_PREFIX} could not parse input ({e}), skipping."
            );
            return Outcome::Pass;
        }
    };

    // 4. Trigger classification. If `tool_input.command` is absent, the
    // tool call isn't a Bash invocation we care about → pass through. Same
    // for commands the trigger regex doesn't classify as commit/push.
    let command = match input.tool_input.command.as_deref() {
        Some(c) => c,
        None => return Outcome::Pass,
    };
    let event = match Trigger::classify(command) {
        Some(e) => e,
        None => return Outcome::Pass,
    };

    // 5. Resolve repo root and load `klasp.toml`. Fail-open on either step
    // — a missing config is not an error, it's "this repo hasn't enrolled
    // in klasp yet."
    let repo_root = match git::find_repo_root_from_cwd() {
        Some(r) => r,
        None => {
            let _ = writeln!(
                stderr,
                "{NOTICE_PREFIX} could not resolve repo root, skipping."
            );
            return Outcome::Pass;
        }
    };

    let config = match ConfigV1::load(&repo_root) {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(stderr, "{NOTICE_PREFIX} config error ({e}), skipping.");
            return Outcome::Pass;
        }
    };

    // 6. Run checks. Per-check failures (no registered source, runtime
    // error from the source) emit a notice and continue rather than
    // aborting the whole gate — one broken check must not wedge the
    // others.
    let registry = SourceRegistry::default_v1();
    let repo_state = RepoState {
        root: repo_root,
        git_event: event,
    };

    let mut verdicts: Vec<Verdict> = Vec::new();
    for check in &config.checks {
        if !triggers_match(check, event) {
            continue;
        }
        let source = match registry.find_for(check) {
            Some(s) => s,
            None => {
                let _ = writeln!(
                    stderr,
                    "{NOTICE_PREFIX} no source registered for check `{}`, skipping.",
                    check.name,
                );
                continue;
            }
        };
        match source.run(check, &repo_state) {
            Ok(result) => verdicts.push(result.verdict),
            Err(e) => {
                let _ = writeln!(
                    stderr,
                    "{NOTICE_PREFIX} check `{}` runtime error ({e}), skipping.",
                    check.name,
                );
            }
        }
    }

    // 7. Aggregate per-check verdicts. `VerdictPolicy::AnyFail` is the only
    // policy in v0.1; other variants land in v0.2.5 (see roadmap).
    let final_verdict = Verdict::merge(verdicts, config.gate.policy);
    render_terminal_summary(stderr, &final_verdict, config.gate.policy);

    if final_verdict.is_blocking() {
        Outcome::Block
    } else {
        Outcome::Pass
    }
}

/// Does this check's `triggers = [...]` list mention the current git event?
///
/// Convention: an empty `triggers` list means "fire on every event" — the
/// pre-commit-style default. Users who want a check that *only* fires on
/// push can write `triggers = [{ on = ["push"] }]`. v0.2 will likely grow
/// non-git triggers (`pre-merge`, scheduled, …) and this helper is the
/// natural seam to extend.
fn triggers_match(check: &CheckConfig, event: GitEvent) -> bool {
    if check.triggers.is_empty() {
        return true;
    }
    let needle = match event {
        GitEvent::Commit => "commit",
        GitEvent::Push => "push",
    };
    check
        .triggers
        .iter()
        .any(|t| t.on.iter().any(|name| name == needle))
}

/// Render a one-shot summary on stderr. A real `render` module lands in W4
/// alongside `klasp doctor`; this is the minimal text rendering needed so a
/// failing gate explains itself without a `serde_json` dump in the user's
/// terminal.
fn render_terminal_summary<W: Write>(stderr: &mut W, verdict: &Verdict, policy: VerdictPolicy) {
    match verdict {
        Verdict::Pass => {
            // No noise on the happy path — quiet gates stay out of the way.
        }
        Verdict::Warn { findings, message } => {
            let _ = writeln!(
                stderr,
                "{NOTICE_PREFIX} warnings ({} findings):",
                findings.len()
            );
            if let Some(m) = message {
                let _ = writeln!(stderr, "  {m}");
            }
            for f in findings {
                let _ = writeln!(stderr, "  - [{}] {}", f.rule, f.message);
            }
        }
        Verdict::Fail { findings, message } => {
            let _ = writeln!(
                stderr,
                "{NOTICE_PREFIX} blocked ({} findings, policy={:?}):",
                findings.len(),
                policy,
            );
            let _ = writeln!(stderr, "{message}");
            for f in findings {
                let location = match (f.file.as_deref(), f.line) {
                    (Some(file), Some(line)) => format!(" ({file}:{line})"),
                    (Some(file), None) => format!(" ({file})"),
                    _ => String::new(),
                };
                let _ = writeln!(stderr, "  - [{}] {}{location}", f.rule, f.message,);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use klasp_core::{CheckConfig, CheckSourceConfig, TriggerConfig};

    use super::*;

    fn check_with_triggers(on: Vec<&str>) -> CheckConfig {
        CheckConfig {
            name: "demo".into(),
            triggers: if on.is_empty() {
                vec![]
            } else {
                vec![TriggerConfig {
                    on: on.into_iter().map(String::from).collect(),
                }]
            },
            source: CheckSourceConfig::Shell {
                command: "true".into(),
            },
            timeout_secs: None,
        }
    }

    #[test]
    fn empty_triggers_match_every_event() {
        let c = check_with_triggers(vec![]);
        assert!(triggers_match(&c, GitEvent::Commit));
        assert!(triggers_match(&c, GitEvent::Push));
    }

    #[test]
    fn commit_trigger_matches_only_commit() {
        let c = check_with_triggers(vec!["commit"]);
        assert!(triggers_match(&c, GitEvent::Commit));
        assert!(!triggers_match(&c, GitEvent::Push));
    }

    #[test]
    fn push_trigger_matches_only_push() {
        let c = check_with_triggers(vec!["push"]);
        assert!(!triggers_match(&c, GitEvent::Commit));
        assert!(triggers_match(&c, GitEvent::Push));
    }

    #[test]
    fn either_trigger_matches_both_events() {
        let c = check_with_triggers(vec!["commit", "push"]);
        assert!(triggers_match(&c, GitEvent::Commit));
        assert!(triggers_match(&c, GitEvent::Push));
    }

    #[test]
    fn unknown_trigger_name_matches_nothing() {
        let c = check_with_triggers(vec!["pre-merge"]);
        assert!(!triggers_match(&c, GitEvent::Commit));
        assert!(!triggers_match(&c, GitEvent::Push));
    }
}
