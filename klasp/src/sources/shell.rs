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

use std::io::{Read, Write as _};
use std::process::{Child, Command, Stdio};
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
///
/// Single home: every subprocess source (shell recipes and the plugin source)
/// polls through [`spawn_with_timeout`], so this const is the one place the
/// cadence is defined.
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
/// Thin wrapper over [`spawn_with_timeout`]: builds the `sh -c` command with
/// the shell-source env, then adapts the shared primitive's [`ProcessOutput`]
/// / [`ProcessError`] back into [`ShellOutcome`] / [`CheckSourceError`].
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
/// - No stdin payload (`None` → `Stdio::null()`) and no output cap (`None` →
///   unbounded `read_to_string`), preserving v0.1 shell-source behaviour.
///   Non-zero exit is *not* an error here — the status code rides back inside
///   [`ShellOutcome`] for [`exit_status_to_verdict`] to interpret.
pub(super) fn run_with_timeout(
    command: &str,
    cwd: &std::path::Path,
    base_ref: &str,
    timeout: Duration,
) -> Result<ShellOutcome, CheckSourceError> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(cwd)
        .env("KLASP_BASE_REF", base_ref);

    let output = spawn_with_timeout(&mut cmd, None, timeout, None)?;

    Ok(ShellOutcome {
        status_code: output.status_code,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

/// Buffered stdio + exit status from a finished (or killed) child.
///
/// The shared output of [`spawn_with_timeout`]. `status_code` is `None` when
/// the child was killed by a signal (no exit code). Callers decide what a
/// non-zero or absent code means — the primitive does not treat non-zero exit
/// as an error.
///
/// `pub(super)` so sibling sources (the plugin source) can adapt it to their
/// own output shape.
pub(super) struct ProcessOutput {
    pub(super) status_code: Option<i32>,
    pub(super) stdout: String,
    pub(super) stderr: String,
}

/// Failure modes of [`spawn_with_timeout`]. Caller-agnostic so each caller can
/// map it to its own error type (`CheckSourceError` for shell sources, `String`
/// for the plugin source).
pub(super) enum ProcessError {
    /// The child could not be spawned, or a kernel-level `try_wait` error
    /// occurred while polling it.
    Spawn(std::io::Error),
    /// The child overran `timeout` and was killed.
    Timeout { secs: u64 },
    /// A drain thread failed: read error, panic, output cap exceeded, or
    /// non-UTF-8 bytes. The string describes the failure.
    Output(String),
}

impl From<ProcessError> for CheckSourceError {
    fn from(e: ProcessError) -> Self {
        match e {
            ProcessError::Spawn(source) => CheckSourceError::Spawn { source },
            ProcessError::Timeout { secs } => CheckSourceError::Timeout { secs },
            ProcessError::Output(msg) => CheckSourceError::Output(msg),
        }
    }
}

/// Shared spawn/poll/drain/kill primitive for klasp's subprocess sources.
///
/// Captures the mechanics that the shell source and the plugin source had
/// independently grown:
///
/// - a single `try_wait` poll loop on one [`POLL_INTERVAL`],
/// - background stdout/stderr drain threads (so a chatty child can't wedge on
///   a full OS pipe buffer while we poll),
/// - kill-on-timeout (and on `try_wait` error) followed by a `wait` reap,
/// - draining the reader threads on every exit path so none are detached.
///
/// The real per-caller differences are parameters:
///
/// - `stdin`: `None` wires `Stdio::null()`; `Some(payload)` pipes it in on a
///   dedicated writer thread (so a slow-reading child can't block the poll
///   loop, and a payload larger than the pipe buffer can't deadlock).
/// - `cap`: `None` reads each stream unbounded; `Some(n)` bails the drain with
///   a `ProcessError::Output` if either stream exceeds `n` bytes (OOM guard).
///
/// Callers keep their own env setup (set on `cmd` before calling) and their own
/// exit-status interpretation — the primitive returns the status code rather
/// than deciding whether non-zero is a failure. `stdout`/`stderr` are
/// configured here so callers don't have to.
pub(super) fn spawn_with_timeout(
    cmd: &mut Command,
    stdin: Option<&str>,
    timeout: Duration,
    cap: Option<usize>,
) -> Result<ProcessOutput, ProcessError> {
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(ProcessError::Spawn)?;

    // Spawn drain threads BEFORE writing stdin so a stdin payload larger than
    // the pipe buffer (~64 KB on Linux, ~16 KB on macOS) cannot deadlock if the
    // child interleaves stdin reads with stdout writes. Held in mutable Options
    // so error / timeout paths can `.take()` and join them before propagating,
    // rather than detaching.
    let mut stdout_handle = child.stdout.take().map(|r| spawn_drain(r, cap));
    let mut stderr_handle = child.stderr.take().map(|r| spawn_drain(r, cap));

    // stdin write happens in its own thread so a slow-reading child can't block
    // the parent's poll loop.
    let mut stdin_handle = match (stdin, child.stdin.take()) {
        (Some(payload), Some(mut pipe)) => {
            let payload = payload.to_string();
            Some(thread::spawn(move || {
                // BrokenPipe is acceptable if the child exited early — the
                // child's exit status is the authoritative signal for that case.
                let _ = pipe.write_all(payload.as_bytes());
            }))
        }
        _ => None,
    };

    let started = Instant::now();
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= timeout {
                    reap_and_join(
                        &mut child,
                        &mut stdin_handle,
                        &mut stdout_handle,
                        &mut stderr_handle,
                    );
                    return Err(ProcessError::Timeout {
                        secs: timeout.as_secs(),
                    });
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(source) => {
                // `try_wait` errors are kernel-level (rare); reap the child and
                // join the readers so we don't orphan the process or detach the
                // drain threads.
                reap_and_join(
                    &mut child,
                    &mut stdin_handle,
                    &mut stdout_handle,
                    &mut stderr_handle,
                );
                return Err(ProcessError::Spawn(source));
            }
        }
    };

    if let Some(h) = stdin_handle.take() {
        let _ = h.join();
    }

    let stdout = stdout_handle
        .map(join_drain)
        .transpose()?
        .unwrap_or_default();
    let stderr = stderr_handle
        .map(join_drain)
        .transpose()?
        .unwrap_or_default();

    Ok(ProcessOutput {
        status_code: exit_status.code(),
        stdout,
        stderr,
    })
}

/// Kill + reap the child and join every outstanding I/O thread. Used on the
/// timeout and `try_wait`-error exit paths so no process is orphaned and no
/// drain/stdin thread is detached.
fn reap_and_join(
    child: &mut Child,
    stdin_handle: &mut Option<thread::JoinHandle<()>>,
    stdout_handle: &mut Option<thread::JoinHandle<Result<String, ProcessError>>>,
    stderr_handle: &mut Option<thread::JoinHandle<Result<String, ProcessError>>>,
) {
    let _ = child.kill();
    let _ = child.wait();
    if let Some(h) = stdin_handle.take() {
        let _ = h.join();
    }
    if let Some(h) = stdout_handle.take() {
        let _ = h.join();
    }
    if let Some(h) = stderr_handle.take() {
        let _ = h.join();
    }
}

/// Drain `reader` to a `String` on a background thread.
///
/// `cap = None` reads unbounded via `read_to_string` (shell-source behaviour).
/// `cap = Some(n)` reads in chunks and bails with a `ProcessError::Output` if
/// total bytes exceed `n` — the plugin OOM guard.
fn spawn_drain<R: Read + Send + 'static>(
    mut reader: R,
    cap: Option<usize>,
) -> thread::JoinHandle<Result<String, ProcessError>> {
    thread::spawn(move || match cap {
        None => {
            let mut buf = String::new();
            reader
                .read_to_string(&mut buf)
                .map(|_| buf)
                .map_err(|e| ProcessError::Output(format!("failed to read child stdio: {e}")))
        }
        Some(cap) => drain_capped(reader, cap),
    })
}

/// Read `reader` into a `String`, bailing if total bytes exceed `cap`. Backs
/// the `Some(cap)` arm of [`spawn_drain`].
fn drain_capped(mut reader: impl Read, cap: usize) -> Result<String, ProcessError> {
    let mut buf = Vec::with_capacity(8 * 1024);
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() + n > cap {
                    return Err(ProcessError::Output(format!(
                        "output exceeded {cap}-byte cap; killed"
                    )));
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Err(e) => return Err(ProcessError::Output(format!("read error: {e}"))),
        }
    }
    String::from_utf8(buf).map_err(|e| ProcessError::Output(format!("not valid UTF-8: {e}")))
}

fn join_drain(
    handle: thread::JoinHandle<Result<String, ProcessError>>,
) -> Result<String, ProcessError> {
    match handle.join() {
        Ok(result) => result,
        Err(_) => Err(ProcessError::Output(
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
            staged_files: vec![],
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
            staged_files: vec![],
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
