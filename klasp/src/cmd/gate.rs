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

use rayon::prelude::*;

use klasp_core::{
    discover_config_for_path, CheckConfig, CheckResult, ConfigV1, GateProtocol, GitEvent,
    RepoState, Trigger, UserTrigger, Verdict, VerdictPolicy,
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

    // 4. Trigger classification. Built-in commit/push regex runs first; if it
    // doesn't match, user-defined [[trigger]] blocks are consulted after the
    // config is loaded. Commands absent or unmatched by any trigger → pass through.
    let command = match input.tool_input.command.as_deref() {
        Some(c) => c,
        None => return Outcome::Pass,
    };

    // Agent identity for user-trigger agent-filter matching. Read from env
    // so the hook script can export KLASP_AGENT_ID=claude_code without
    // requiring a schema bump. Falls back to empty string → no agent filtering.
    let agent_id = std::env::var("KLASP_AGENT_ID").unwrap_or_default();

    let builtin_event = Trigger::classify(command);

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

    // Accumulated per-check results across all groups (used by JSON formatter).
    let mut all_check_results: Vec<CheckResult> = Vec::new();

    let group_verdicts: Vec<Verdict> = if staged.is_empty() {
        // Single-config fallback: no staged files (push event, empty index, or
        // outside git). Use the root config's own policy for this single group.
        let config = match ConfigV1::load(&repo_root) {
            Ok(c) => c,
            Err(e) => {
                let _ = writeln!(stderr, "{NOTICE_PREFIX} config error ({e}), skipping.");
                return Outcome::Pass;
            }
        };
        // Resolve effective git event: built-in OR user trigger.
        let compiled_triggers = config.compiled_triggers();
        let event = match resolve_event(builtin_event, command, compiled_triggers, &agent_id) {
            Some(e) => e,
            None => return Outcome::Pass,
        };
        let repo_state = RepoState {
            root: repo_root.clone(),
            git_event: event,
            base_ref,
            staged_files: vec![],
        };
        let check_results = run_config_checks(stderr, &config, &repo_state, &registry, event);
        let verdicts: Vec<Verdict> = check_results.iter().map(|r| r.verdict.clone()).collect();
        let group_verdict = Verdict::merge(verdicts, config.gate.policy);
        all_check_results.extend(check_results);
        vec![group_verdict]
    } else {
        // Monorepo path: group files → run each group under its own policy →
        // collect one verdict per group.
        let groups = group_by_config(stderr, &staged, &repo_root);
        if groups.is_empty() {
            // Every staged file was outside all known configs — treat as pass.
            return Outcome::Pass;
        }
        groups
            .into_iter()
            .filter_map(|(config_path, files)| {
                let config = match ConfigV1::from_file(&config_path) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = writeln!(
                            stderr,
                            "{NOTICE_PREFIX} config error for {path} ({e}), skipping group.",
                            path = config_path.display(),
                        );
                        return None;
                    }
                };
                let compiled_triggers = config.compiled_triggers();
                let event = resolve_event(builtin_event, command, compiled_triggers, &agent_id)?;
                let group_policy = config.gate.policy;
                let repo_state = RepoState {
                    root: repo_root.clone(),
                    git_event: event,
                    base_ref: base_ref.clone(),
                    // Scope this invocation to the files belonging to this group.
                    staged_files: files,
                };
                let check_results =
                    run_config_checks(stderr, &config, &repo_state, &registry, event);
                let verdicts: Vec<Verdict> =
                    check_results.iter().map(|r| r.verdict.clone()).collect();
                let group_verdict = Verdict::merge(verdicts, group_policy);
                all_check_results.extend(check_results);
                Some(group_verdict)
            })
            .collect()
    };

    // 7. Aggregate cross-group under AnyFail: one failing group blocks the gate,
    // regardless of the individual group policies already applied above.
    let cross_group_policy = VerdictPolicy::AnyFail;
    let final_verdict = Verdict::merge(group_verdicts, cross_group_policy);
    dispatch_output(
        stderr,
        args,
        &final_verdict,
        cross_group_policy,
        &all_check_results,
    );

    if final_verdict.is_blocking() {
        Outcome::Block
    } else {
        Outcome::Pass
    }
}

/// Resolve the effective [`GitEvent`] for a command.
///
/// Built-in commit/push regex is tried first. If it matches, return that event.
/// Otherwise, check user-defined `[[trigger]]` blocks. If any user trigger
/// matches, return `GitEvent::Commit` as a sentinel (user triggers fire the
/// gate without caring about commit-vs-push semantics — checks filter on
/// `triggers = [{ on = ["commit"] }]` themselves). Returns `None` if no
/// trigger matches (pass-through).
fn resolve_event(
    builtin: Option<GitEvent>,
    command: &str,
    user_triggers: &[UserTrigger],
    agent_id: &str,
) -> Option<GitEvent> {
    if let Some(event) = builtin {
        return Some(event);
    }
    // User triggers fire with Commit semantics by default so existing
    // `triggers = [{ on = ["commit"] }]` checks participate.
    let matched = user_triggers.iter().any(|t| t.matches(command, agent_id));
    if matched {
        Some(GitEvent::Commit)
    } else {
        None
    }
}

/// Run all trigger-matching checks in `config` and return their results.
///
/// When `config.gate.parallel == true`, checks execute concurrently via
/// rayon's work-stealing thread pool. Per-check stderr notices use
/// `io::stderr()` directly in parallel mode — the `&mut W` plumbing exists
/// for testing and is only used in sequential mode. This is an intentional
/// trade-off: in parallel mode stderr writes are serialised at the OS level
/// so lines are readable but may interleave across checks. For sequential
/// mode the injected writer is used so tests can capture stderr output.
fn run_config_checks<W: Write>(
    stderr: &mut W,
    config: &ConfigV1,
    repo_state: &RepoState,
    registry: &SourceRegistry,
    event: GitEvent,
) -> Vec<CheckResult> {
    let triggered: Vec<&CheckConfig> = config
        .checks
        .iter()
        .filter(|c| triggers_match(c, event))
        .collect();

    if config.gate.parallel {
        run_parallel(&triggered, repo_state, registry)
    } else {
        run_sequential(stderr, &triggered, repo_state, registry)
    }
}

/// Execute checks sequentially, writing notices to the injected `stderr`.
fn run_sequential<W: Write>(
    stderr: &mut W,
    triggered: &[&CheckConfig],
    repo_state: &RepoState,
    registry: &SourceRegistry,
) -> Vec<CheckResult> {
    let mut results = Vec::new();
    for check in triggered {
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
        // `SourceForCheck::run` delegates to the underlying CheckSource impl
        // (built-in or plugin subprocess) transparently.
        match source.run(check, repo_state) {
            Ok(result) => results.push(result),
            Err(e) => {
                let _ = writeln!(
                    stderr,
                    "{NOTICE_PREFIX} check `{}` runtime error ({e}), skipping.",
                    check.name,
                );
            }
        }
    }
    results
}

/// Execute checks in parallel via rayon's work-stealing pool.
///
/// Per-check error notices write directly to `io::stderr()` since the
/// `&mut W` test-injection point is not `Sync` — multiple rayon threads
/// cannot safely share a mutable reference. OS-level line buffering means
/// concurrent writes are readable, though lines from different checks may
/// interleave.
fn run_parallel(
    triggered: &[&CheckConfig],
    repo_state: &RepoState,
    registry: &SourceRegistry,
) -> Vec<CheckResult> {
    triggered
        .par_iter()
        .filter_map(|check| {
            let source = match registry.find_for(check) {
                Some(s) => s,
                None => {
                    let _ = writeln!(
                        io::stderr(),
                        "{NOTICE_PREFIX} no source registered for check `{}`, skipping.",
                        check.name,
                    );
                    return None;
                }
            };
            // `SourceForCheck::run` delegates to the underlying CheckSource impl
            // (built-in or plugin subprocess) transparently.
            match source.run(check, repo_state) {
                Ok(result) => Some(result),
                Err(e) => {
                    let _ = writeln!(
                        io::stderr(),
                        "{NOTICE_PREFIX} check `{}` runtime error ({e}), skipping.",
                        check.name,
                    );
                    None
                }
            }
        })
        .collect()
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
/// - `Junit` / `Sarif` / `Json` → write the machine-readable output to
///   `--output` path when provided, otherwise to stdout.
fn dispatch_output<W: Write>(
    stderr: &mut W,
    args: &GateArgs,
    verdict: &Verdict,
    policy: VerdictPolicy,
    check_results: &[CheckResult],
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
        OutputFormat::Json => {
            let json = output::json::render(verdict, policy, check_results);
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

    /// Verifies that `group_by_config` correctly scopes files to their nearest
    /// `klasp.toml`. This locks the invariant that `RepoState.staged_files` is
    /// populated with only the files belonging to that group, not the full
    /// staged set. Per-source consumption of the field is deferred to #34.
    #[test]
    fn group_by_config_scopes_files_to_nearest_config() {
        use std::io::sink;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().to_path_buf();

        // Write two package configs.
        let pkg_a = repo.join("packages").join("alpha");
        let pkg_b = repo.join("packages").join("beta");
        std::fs::create_dir_all(&pkg_a).unwrap();
        std::fs::create_dir_all(&pkg_b).unwrap();
        std::fs::write(
            pkg_a.join("klasp.toml"),
            "version = 1\n[gate]\nagents = []\n",
        )
        .unwrap();
        std::fs::write(
            pkg_b.join("klasp.toml"),
            "version = 1\n[gate]\nagents = []\n",
        )
        .unwrap();

        let file_a = pkg_a.join("index.ts");
        let file_b = pkg_b.join("index.ts");
        std::fs::write(&file_a, "").unwrap();
        std::fs::write(&file_b, "").unwrap();

        let staged = vec![file_a.clone(), file_b.clone()];
        let mut stderr = sink();
        let groups = group_by_config(&mut stderr, &staged, &repo);

        assert_eq!(groups.len(), 2, "expected two groups");

        // Canonicalize expected paths to handle /var → /private/var on macOS.
        let canon_a = pkg_a.canonicalize().unwrap_or(pkg_a.clone());
        let canon_b = pkg_b.canonicalize().unwrap_or(pkg_b.clone());

        // Find each group and assert its file list is exactly one file.
        for (config_path, files) in &groups {
            if config_path.starts_with(&canon_a) {
                assert_eq!(files.len(), 1, "alpha group must contain exactly one file");
                // The returned file path may also be canonicalized; compare
                // canonical forms.
                let got = files[0].canonicalize().unwrap_or_else(|_| files[0].clone());
                let exp = file_a.canonicalize().unwrap_or_else(|_| file_a.clone());
                assert_eq!(got, exp, "alpha group file mismatch");
            } else if config_path.starts_with(&canon_b) {
                assert_eq!(files.len(), 1, "beta group must contain exactly one file");
                let got = files[0].canonicalize().unwrap_or_else(|_| files[0].clone());
                let exp = file_b.canonicalize().unwrap_or_else(|_| file_b.clone());
                assert_eq!(got, exp, "beta group file mismatch");
            } else {
                panic!("unexpected group config path: {}", config_path.display());
            }
        }
    }
}
