//! `Shell` — the only `CheckSource` impl in v0.1.
//!
//! Spawns `sh -c "<command>"`, captures stdout and stderr, enforces an
//! optional per-check timeout, and maps the child's exit code to a
//! [`klasp_core::Verdict`]. Exit `0` → `Pass`; any non-zero (including
//! Claude Code's `2` "deny" convention) → `Fail` with the captured stderr
//! rendered into a structured `Finding`.
//!
//! **Design note: no `verdict_path`-driven JSON extraction in v0.1.**
//! [docs/design.md §6] sketches an exit-code-driven gate flow and §3.5's
//! [`klasp_core::CheckSourceConfig::Shell`] only carries a `command` field
//! — there is no config slot to point at a JSON `verdict` key for the
//! generic shell source. Named recipes (v0.2 — `fallow`, `pytest`) know
//! their tool's output schema and will parse JSON natively. The
//! [`extract_verdict_path`] helper lives here as a private utility so the
//! dot-notation path semantics ride alongside the rest of the source's
//! tests; it is wired up the moment the config grows the field.
//!
//! **Windows.** The Rust binary spawns `sh -c …` on every platform. On
//! Windows that resolves to Git for Windows' `sh.exe`, which klasp's hook
//! shim already requires (see [docs/design.md §14] open question). When
//! `sh` is missing, [`std::process::Command::spawn`] returns
//! [`CheckSourceError::Spawn`]; the gate runtime fails open with a stderr
//! notice rather than blocking the agent on a tooling gap.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use klasp_core::{
    CheckConfig, CheckResult, CheckSource, CheckSourceConfig, CheckSourceError, Finding, RepoState,
    Severity, Verdict,
};

/// Default per-check timeout when `klasp.toml` omits `timeout_secs`.
/// Intentionally generous — fail-open semantics demand we never kill a slow
/// check before the user expects to. Users with stricter budgets set
/// `timeout_secs` per-check.
///
/// `pub(super)` so the named-recipe sources share the same default — every
/// recipe ultimately calls `run_with_timeout`, so they should agree on the
/// budget when the user hasn't set one.
pub(super) const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Granularity of the std-only `try_wait` poll loop. 50 ms keeps idle
/// wakeups cheap and bounds gate-runtime latency on a fast-exiting check.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Stable identifier this source advertises through `CheckSource::source_id`.
const SOURCE_ID: &str = "shell";

/// Built-in `CheckSource` for `type = "shell"` config entries. Stateless;
/// safe to clone or share. Constructed once via
/// [`crate::sources::SourceRegistry::default_v1`].
#[derive(Default)]
pub struct ShellSource {
    _private: (),
}

impl ShellSource {
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl CheckSource for ShellSource {
    fn source_id(&self) -> &str {
        SOURCE_ID
    }

    fn supports_config(&self, config: &CheckConfig) -> bool {
        matches!(config.source, CheckSourceConfig::Shell { .. })
    }

    fn run(
        &self,
        config: &CheckConfig,
        state: &RepoState,
    ) -> Result<CheckResult, CheckSourceError> {
        let command = match &config.source {
            CheckSourceConfig::Shell { command } => command.as_str(),
            // `supports_config` should already have routed non-Shell
            // configs to a different source, but if a future caller
            // bypasses the registry the safest fall-through is a
            // typed runtime error rather than a silent panic.
            other => {
                return Err(CheckSourceError::Other(
                    format!("ShellSource cannot run {other:?}").into(),
                ));
            }
        };

        let timeout = Duration::from_secs(config.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
        let outcome = run_with_timeout(command, &state.root, &state.base_ref, timeout)?;

        let verdict = exit_status_to_verdict(&config.name, outcome.status_code, &outcome.stderr);
        Ok(CheckResult {
            source_id: SOURCE_ID.to_string(),
            check_name: config.name.clone(),
            verdict,
            raw_stdout: Some(outcome.stdout),
            raw_stderr: Some(outcome.stderr),
        })
    }
}

/// Buffered stdio + exit code from a finished child.
///
/// `pub(super)` so sibling sources (the v0.2 named recipes) can reuse the
/// same `sh -c` plumbing without re-implementing the timeout / drain dance.
/// The fields are intentionally narrow — anything richer (signal, duration)
/// would invite the recipes to depend on shell-source internals.
pub(super) struct ShellOutcome {
    /// `None` when the child was killed (signal on Unix, terminated by
    /// timeout). The runtime does not need to distinguish a missing exit
    /// code from a non-zero one — both map to `Verdict::Fail`.
    pub(super) status_code: Option<i32>,
    pub(super) stdout: String,
    pub(super) stderr: String,
}

/// Spawn `sh -c {command}`, capture stdio, kill if it overruns `timeout`.
///
/// Implementation notes:
///
/// - `sh -c` is the conventional invocation: identical surface on macOS,
///   Linux, and Git for Windows bash. Avoids dragging in `cmd.exe`'s
///   quoting rules on Windows.
/// - `cwd` is set to the repo root so commands like `cargo test` resolve
///   relative paths the way users expect.
/// - `KLASP_BASE_REF` is exported into the child env per
///   [docs/design.md §3.5] so diff-aware tools (`pre-commit`, `fallow`)
///   can scope themselves to changed-since-base. The gate runtime computed
///   the value via `git merge-base` before assembling [`RepoState`].
/// - Stdio is captured via background reader threads. Buffering the streams
///   on the main thread risks the child blocking on a full pipe before we
///   call `wait`; `wait_with_output` would solve that but doesn't compose
///   with the std-only timeout pattern (it blocks indefinitely).
pub(super) fn run_with_timeout(
    command: &str,
    cwd: &std::path::Path,
    base_ref: &str,
    timeout: Duration,
) -> Result<ShellOutcome, CheckSourceError> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .env("KLASP_BASE_REF", base_ref)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| CheckSourceError::Spawn { source })?;

    // Drain stdout / stderr in background threads so a chatty check can't
    // wedge on a full OS pipe buffer while we're polling `try_wait`. Held in
    // mutable Options so error / timeout paths can `.take()` and join them
    // before propagating, rather than detaching.
    let mut stdout_handle = child.stdout.take().map(spawn_drain);
    let mut stderr_handle = child.stderr.take().map(spawn_drain);

    let started = Instant::now();
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Some(h) = stdout_handle.take() {
                        let _ = h.join();
                    }
                    if let Some(h) = stderr_handle.take() {
                        let _ = h.join();
                    }
                    return Err(CheckSourceError::Timeout {
                        secs: timeout.as_secs(),
                    });
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(source) => {
                // `try_wait` errors are kernel-level (rare); reap the child
                // and join the readers so we don't orphan the process or
                // detach the drain threads.
                let _ = child.kill();
                let _ = child.wait();
                if let Some(h) = stdout_handle.take() {
                    let _ = h.join();
                }
                if let Some(h) = stderr_handle.take() {
                    let _ = h.join();
                }
                return Err(CheckSourceError::Spawn { source });
            }
        }
    };

    let stdout = stdout_handle
        .map(join_drain)
        .transpose()?
        .unwrap_or_default();
    let stderr = stderr_handle
        .map(join_drain)
        .transpose()?
        .unwrap_or_default();

    Ok(ShellOutcome {
        status_code: exit_status.and_then(|s| s.code()),
        stdout,
        stderr,
    })
}

fn spawn_drain<R: Read + Send + 'static>(
    mut reader: R,
) -> thread::JoinHandle<std::io::Result<String>> {
    thread::spawn(move || {
        let mut buf = String::new();
        reader.read_to_string(&mut buf).map(|_| buf)
    })
}

fn join_drain(
    handle: thread::JoinHandle<std::io::Result<String>>,
) -> Result<String, CheckSourceError> {
    match handle.join() {
        Ok(Ok(s)) => Ok(s),
        Ok(Err(e)) => Err(CheckSourceError::Output(format!(
            "failed to read child stdio: {e}"
        ))),
        Err(_) => Err(CheckSourceError::Output(
            "stdio reader thread panicked".to_string(),
        )),
    }
}

fn exit_status_to_verdict(check_name: &str, code: Option<i32>, stderr: &str) -> Verdict {
    match code {
        Some(0) => Verdict::Pass,
        Some(c) => {
            let trimmed = stderr.trim();
            let detail = if trimmed.is_empty() {
                format!("check `{check_name}` exited with status {c}")
            } else {
                format!("check `{check_name}` exited with status {c}: {trimmed}")
            };
            Verdict::Fail {
                findings: vec![Finding {
                    rule: format!("shell:{check_name}"),
                    message: detail.clone(),
                    file: None,
                    line: None,
                    severity: Severity::Error,
                }],
                message: detail,
            }
        }
        None => {
            let detail =
                format!("check `{check_name}` was terminated before producing an exit code");
            Verdict::Fail {
                findings: vec![Finding {
                    rule: format!("shell:{check_name}"),
                    message: detail.clone(),
                    file: None,
                    line: None,
                    severity: Severity::Error,
                }],
                message: detail,
            }
        }
    }
}

/// Walk a dot-notation path (`.verdict`, `.results.summary.verdict`) into a
/// `serde_json::Value` and return the matched value's string form.
///
/// Limited on purpose: no array indexing (`.results[0]`), no escaping,
/// matching [docs/design.md §14]'s explicit v0.1 acceptance. v0.2 swaps to
/// a real JSON pointer if anyone hits the limitation.
///
/// Currently `pub(crate)` rather than `pub` because no public caller exists
/// — the v0.1 [`CheckSourceConfig::Shell`] has no `verdict_path` field, and
/// only `Shell`'s own tests exercise this. Promoted to `pub(crate)` (not
/// `pub`) so the moment the field lands the wiring is a one-liner without
/// re-exposing internals.
#[allow(dead_code)]
pub(crate) fn extract_verdict_path<'a>(
    value: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    let trimmed = path.trim_start_matches('.');
    if trimmed.is_empty() {
        return Some(value);
    }
    let mut cursor = value;
    for segment in trimmed.split('.') {
        cursor = cursor.get(segment)?;
    }
    Some(cursor)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use klasp_core::{CheckConfig, CheckSourceConfig, GitEvent, RepoState};

    use super::*;

    fn cwd() -> PathBuf {
        std::env::current_dir().expect("cwd available in tests")
    }

    fn state() -> RepoState {
        RepoState {
            root: cwd(),
            git_event: GitEvent::Commit,
            base_ref: "HEAD~1".to_string(),
        }
    }

    fn check(name: &str, command: &str, timeout: Option<u64>) -> CheckConfig {
        CheckConfig {
            name: name.into(),
            triggers: vec![],
            source: CheckSourceConfig::Shell {
                command: command.into(),
            },
            timeout_secs: timeout,
        }
    }

    #[test]
    fn passing_command_yields_pass() {
        let result = ShellSource::new()
            .run(&check("noop", "true", Some(5)), &state())
            .expect("shell source should run `true` cleanly");
        assert!(matches!(result.verdict, Verdict::Pass));
        assert_eq!(result.source_id, "shell");
        assert_eq!(result.check_name, "noop");
    }

    #[test]
    fn failing_command_yields_fail_with_finding() {
        let result = ShellSource::new()
            .run(
                &check(
                    "always-fail",
                    "echo something-on-stderr 1>&2; exit 7",
                    Some(5),
                ),
                &state(),
            )
            .expect("shell source should still produce a result for a failing command");
        match &result.verdict {
            Verdict::Fail { findings, message } => {
                assert!(message.contains("status 7"), "message = {message:?}");
                assert!(message.contains("something-on-stderr"));
                assert_eq!(findings.len(), 1);
                assert_eq!(findings[0].severity, Severity::Error);
                assert!(findings[0].rule.starts_with("shell:always-fail"));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
        assert!(result
            .raw_stderr
            .as_deref()
            .is_some_and(|s| s.contains("something-on-stderr")));
    }

    #[test]
    fn captures_stdout_for_passing_command() {
        let result = ShellSource::new()
            .run(&check("hello", "printf hello", Some(5)), &state())
            .expect("ok");
        assert_eq!(result.raw_stdout.as_deref(), Some("hello"));
    }

    #[test]
    fn child_sees_klasp_base_ref_env_var() {
        // The child's `printf "$KLASP_BASE_REF"` echoes the env var back via
        // stdout — that's the contract we ship to recipe authors. If this
        // test starts failing, the user-facing `${KLASP_BASE_REF}` recipes
        // (pre-commit, fallow) silently turn into empty-string substitutions
        // and the diff-aware tools lint the entire tree on every commit.
        let custom_state = RepoState {
            root: cwd(),
            git_event: GitEvent::Commit,
            base_ref: "deadbeefcafebabe".to_string(),
        };
        let result = ShellSource::new()
            .run(
                &check("base-ref-probe", "printf \"$KLASP_BASE_REF\"", Some(5)),
                &custom_state,
            )
            .expect("ok");
        assert_eq!(result.raw_stdout.as_deref(), Some("deadbeefcafebabe"));
        assert!(matches!(result.verdict, Verdict::Pass));
    }

    #[test]
    fn timeout_returns_typed_error() {
        // A 200 ms sleep against a 1-second timeout completes cleanly; the
        // inverse pair (1-second sleep, 200 ms budget) must surface
        // `CheckSourceError::Timeout` rather than wedging the test.
        let err = ShellSource::new()
            .run(&check("slow", "sleep 1", Some(0)), &state())
            // timeout_secs = 0 → 0 ms timeout, the first poll exceeds it.
            .expect_err("0 s timeout must trip the timeout path");
        match err {
            CheckSourceError::Timeout { secs } => assert_eq!(secs, 0),
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    #[test]
    fn supports_config_only_for_shell() {
        let source = ShellSource::new();
        let shell = check("a", "true", None);
        assert!(source.supports_config(&shell));
    }

    #[test]
    fn extract_verdict_path_root_with_dot() {
        let v = serde_json::json!({ "verdict": "pass" });
        let got = extract_verdict_path(&v, ".verdict")
            .and_then(|x| x.as_str())
            .unwrap();
        assert_eq!(got, "pass");
    }

    #[test]
    fn extract_verdict_path_nested() {
        let v = serde_json::json!({
            "results": { "summary": { "verdict": "fail" } }
        });
        let got = extract_verdict_path(&v, ".results.summary.verdict")
            .and_then(|x| x.as_str())
            .unwrap();
        assert_eq!(got, "fail");
    }

    #[test]
    fn extract_verdict_path_missing_returns_none() {
        let v = serde_json::json!({ "verdict": "pass" });
        assert!(extract_verdict_path(&v, ".missing").is_none());
    }

    #[test]
    fn extract_verdict_path_empty_returns_root() {
        let v = serde_json::json!({ "verdict": "pass" });
        // `.` alone (or empty) returns the whole document; useful for tests
        // and for a future config that supplies an explicit no-op path.
        assert_eq!(extract_verdict_path(&v, ".").cloned(), Some(v));
    }
}
