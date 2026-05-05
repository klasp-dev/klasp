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

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use klasp_core::{
    discover_config_for_path, CheckConfig, ConfigV1, GateProtocol, GitEvent, RepoState, Trigger,
    Verdict, VerdictPolicy,
};

use crate::cli::{GateArgs, OutputFormat};
use crate::git;
use crate::output;
use crate::sources::SourceRegistry;

/// Stderr prefix for every fail-open notice. Single source of truth makes
/// log-grep'ing `klasp-gate:` reliable across the binary.
const NOTICE_PREFIX: &str = "klasp-gate:";

pub fn run(args: &GateArgs) -> ExitCode {
    let mut stderr = io::stderr().lock();
    match gate(&mut stderr, args) {
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

fn gate<W: Write>(stderr: &mut W, args: &GateArgs) -> Outcome {
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

    // 5. Resolve repo root. Fail-open — no repo root means no gate.
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

    let registry = SourceRegistry::default_v1();
    let base_ref = git::compute_base_ref(&repo_root);

    // 6. Monorepo dispatch: group staged files by nearest `klasp.toml`.
    // Fall back to single-config mode when there are no staged files
    // (push event, empty index, or outside git) — preserves pre-v0.2.5
    // behaviour exactly.
    let staged = git::staged_files(&repo_root);

    let group_verdicts: Vec<Verdict> = if staged.is_empty() {
        // Single-config fallback.
        let config = match ConfigV1::load(&repo_root) {
            Ok(c) => c,
            Err(e) => {
                let _ = writeln!(stderr, "{NOTICE_PREFIX} config error ({e}), skipping.");
                return Outcome::Pass;
            }
        };
        let repo_state = RepoState {
            root: repo_root.clone(),
            git_event: event,
            base_ref,
        };
        run_config_checks(stderr, &config, &repo_state, &registry, event)
    } else {
        // Monorepo path: group files → run each group → collect per-group verdicts.
        let groups = group_by_config(stderr, &staged, &repo_root);
        if groups.is_empty() {
            // Every staged file was outside all known configs — treat as pass.
            return Outcome::Pass;
        }
        groups
            .into_iter()
            .flat_map(|(config_path, _files)| {
                let config = match ConfigV1::from_file(&config_path) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = writeln!(
                            stderr,
                            "{NOTICE_PREFIX} config error for {path} ({e}), skipping group.",
                            path = config_path.display(),
                        );
                        return vec![];
                    }
                };
                let repo_state = RepoState {
                    root: repo_root.clone(),
                    git_event: event,
                    base_ref: base_ref.clone(),
                };
                run_config_checks(stderr, &config, &repo_state, &registry, event)
            })
            .collect()
    };

    // 7. Aggregate cross-group under AnyFail: one failing group blocks the gate.
    let policy = VerdictPolicy::AnyFail;
    let final_verdict = Verdict::merge(group_verdicts, policy);
    dispatch_output(stderr, args, &final_verdict, policy);

    if final_verdict.is_blocking() {
        Outcome::Block
    } else {
        Outcome::Pass
    }
}

/// Run all trigger-matching checks in `config` and return their verdicts.
fn run_config_checks<W: Write>(
    stderr: &mut W,
    config: &ConfigV1,
    repo_state: &RepoState,
    registry: &SourceRegistry,
    event: GitEvent,
) -> Vec<Verdict> {
    let mut verdicts = Vec::new();
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
        match source.run(check, repo_state) {
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
    verdicts
}

/// Group staged files by nearest `klasp.toml` under `repo_root`.
///
/// Files with no enclosing `klasp.toml` under `repo_root` emit a per-file
/// notice on `stderr` and are omitted — skipped, not an error.
///
/// The `Vec<(PathBuf, Vec<PathBuf>)>` shape is intentional: issue #34 (rayon
/// parallel exec) will parallelise across this slice without restructuring.
fn group_by_config<W: Write>(
    stderr: &mut W,
    staged_files: &[PathBuf],
    repo_root: &Path,
) -> Vec<(PathBuf, Vec<PathBuf>)> {
    let mut order: Vec<PathBuf> = Vec::new();
    let mut map: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();

    for file in staged_files {
        match discover_config_for_path(file, repo_root) {
            Some(config_path) => {
                if !map.contains_key(&config_path) {
                    order.push(config_path.clone());
                }
                map.entry(config_path).or_default().push(file.clone());
            }
            None => {
                let _ = writeln!(
                    stderr,
                    "{NOTICE_PREFIX} no klasp.toml for {path}, skipping.",
                    path = file.display(),
                );
            }
        }
    }

    order
        .into_iter()
        .map(|k| {
            let v = map.remove(&k).unwrap_or_default();
            (k, v)
        })
        .collect()
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

/// Dispatch verdict rendering to the right formatter and write the result to
/// the configured destination.
///
/// - `Terminal` → write the human-readable text to `stderr` (existing v0.1
///   behaviour preserved for all users who don't pass `--format`).
/// - `Junit` / `Sarif` → write the machine-readable output to `--output` path
///   when provided, otherwise to stdout.
fn dispatch_output<W: Write>(
    stderr: &mut W,
    args: &GateArgs,
    verdict: &Verdict,
    policy: VerdictPolicy,
) {
    match args.format {
        OutputFormat::Terminal => {
            let text = output::terminal::render(verdict, policy);
            let _ = write!(stderr, "{text}");
        }
        OutputFormat::Junit => {
            let xml = output::junit::render(verdict, policy);
            write_machine_output(&xml, args);
        }
        OutputFormat::Sarif => {
            let json = output::sarif::render(verdict, policy);
            write_machine_output(&json, args);
        }
    }
}

/// Write machine-readable formatter output to `--output <path>` or stdout.
fn write_machine_output(content: &str, args: &GateArgs) {
    match &args.output {
        Some(path) => {
            if let Err(e) = std::fs::write(path, content) {
                let _ = writeln!(
                    io::stderr(),
                    "{NOTICE_PREFIX} could not write output file ({e})."
                );
            }
        }
        None => {
            let _ = write!(io::stdout(), "{content}");
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
