//! `PluginSource` — subprocess-based `CheckSource` for the v0.3 plugin model.
//!
//! Design: [docs/plugin-protocol.md]. A `PluginSource` wraps a single
//! `klasp-plugin-<name>` binary. It communicates over stdin/stdout using
//! JSON defined by `PLUGIN_PROTOCOL_VERSION = 0`.
//!
//! **All plugin failures → `Verdict::Warn`**. Errors (binary missing, non-zero
//! exit, malformed JSON, protocol version mismatch, timeout) are wrapped into
//! a `Verdict::Warn` with `rule = "klasp::plugin"`. The gate continues with
//! remaining checks — plugin errors never panic or short-circuit the gate.
//!
//! **Lazy discovery.** `PluginSource` is instantiated on demand when
//! `SourceRegistry::find_for` encounters an unknown source type. It delegates
//! binary discovery to `which::which("klasp-plugin-<name>")`. No scan at
//! startup.
//!
//! **Timeout.** Default 60 s; override via `KLASP_PLUGIN_TIMEOUT_SECS` env var.

use std::io::Write as _;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use klasp_core::{
    plugin_error_warn, CheckConfig, CheckResult, CheckSource, CheckSourceConfig, CheckSourceError,
    Finding, PluginConfig, PluginGateInput, PluginGateOutput, PluginTrigger, PluginVerdict,
    RepoState, Verdict, GATE_SCHEMA_VERSION, KLASP_PLUGIN_BIN_PREFIX, PLUGIN_PROTOCOL_VERSION,
};

/// Default plugin subprocess timeout. Intentionally shorter than the 120 s
/// shell default — plugins that hang are more likely misuse than intentional
/// long-running operations.
const DEFAULT_PLUGIN_TIMEOUT_SECS: u64 = 60;

/// Poll granularity for the `try_wait` loop, matching shell.rs.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Cap each of stdout/stderr at 16 MiB. A buggy or hostile plugin that writes
/// gigabytes to a pipe would otherwise OOM the gate process. On overflow the
/// child is killed and the warn message names the cap.
const MAX_PLUGIN_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// Source ID prefix for all plugin sources.
const SOURCE_ID_PREFIX: &str = "plugin";

/// `CheckSource` impl for subprocess plugins.
///
/// One instance per plugin name. Constructed lazily by `SourceRegistry` when
/// a config check's `type = "plugin"` + `name` pair has not been seen before.
/// The resolved binary path and `--describe` handshake result are memoised on
/// first use so subsequent checks targeting the same plugin spawn `--gate` only.
pub struct PluginSource {
    /// Plugin name, e.g. `"my-linter"` → binary `klasp-plugin-my-linter`.
    plugin_name: String,
    /// Cached source_id so `source_id()` can return `&str` tied to `&self`.
    id: String,
    /// Memoised binary lookup. `Ok(path)` on success, `Err(message)` if `which`
    /// failed (cached so repeated lookups don't re-walk `$PATH`).
    binary: OnceLock<Result<std::path::PathBuf, String>>,
    /// Memoised `--describe` handshake. `Ok(())` if the plugin is compatible,
    /// `Err(message)` otherwise. Cached so a plugin invoked across many checks
    /// describes once per gate run.
    describe_ok: OnceLock<Result<(), String>>,
}

impl PluginSource {
    /// Construct a `PluginSource` for `plugin_name`. Does not verify the binary
    /// exists — that happens at `run()` time so the error surfaces as
    /// `Verdict::Warn` rather than a hard error at registry build time.
    pub fn new(plugin_name: impl Into<String>) -> Self {
        let plugin_name = plugin_name.into();
        let id = format!("{SOURCE_ID_PREFIX}:{plugin_name}");
        Self {
            plugin_name,
            id,
            binary: OnceLock::new(),
            describe_ok: OnceLock::new(),
        }
    }

    /// Read the plugin timeout from the environment, falling back to the default.
    fn timeout() -> Duration {
        let secs = std::env::var("KLASP_PLUGIN_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_PLUGIN_TIMEOUT_SECS);
        Duration::from_secs(secs)
    }

    fn resolve_binary(&self) -> &Result<std::path::PathBuf, String> {
        self.binary.get_or_init(|| {
            let bin_name = format!("{KLASP_PLUGIN_BIN_PREFIX}{}", self.plugin_name);
            which::which(&bin_name).map_err(|_| format!("binary `{bin_name}` not found on $PATH"))
        })
    }
}

impl CheckSource for PluginSource {
    fn source_id(&self) -> &str {
        &self.id
    }

    fn supports_config(&self, config: &CheckConfig) -> bool {
        match &config.source {
            CheckSourceConfig::Plugin { name, .. } => name == &self.plugin_name,
            _ => false,
        }
    }

    fn run(
        &self,
        config: &CheckConfig,
        state: &RepoState,
    ) -> Result<CheckResult, CheckSourceError> {
        let verdict = self.run_plugin(config, state);
        Ok(CheckResult {
            source_id: self.id.clone(),
            check_name: config.name.clone(),
            verdict,
            raw_stdout: None,
            raw_stderr: None,
        })
    }
}

impl PluginSource {
    /// Top-level plugin invocation. All error paths return `Verdict::Warn`.
    fn run_plugin(&self, config: &CheckConfig, state: &RepoState) -> Verdict {
        let binary = match self.resolve_binary() {
            Ok(p) => p.clone(),
            Err(msg) => return plugin_error_warn(&self.plugin_name, msg.clone()),
        };

        if let Err(msg) = self.cached_describe(&binary) {
            return plugin_error_warn(&self.plugin_name, msg);
        }

        run_gate(&self.plugin_name, &binary, config, state)
    }

    /// Run `--describe` once per plugin instance and cache the result. Returns
    /// `Ok(())` if the plugin is compatible; `Err(reason)` otherwise.
    fn cached_describe(&self, binary: &std::path::Path) -> Result<(), String> {
        self.describe_ok
            .get_or_init(|| run_describe(binary))
            .clone()
    }
}

/// Run `--describe` and validate the protocol version. Pure function — no
/// caching — so it can be called from `OnceLock::get_or_init`.
fn run_describe(binary: &std::path::Path) -> Result<(), String> {
    let timeout = PluginSource::timeout();
    let output = spawn_and_wait(binary, &["--describe"], None, timeout, &[])
        .map_err(|msg| format!("--describe failed: {msg}"))?;

    let describe: klasp_core::PluginDescribe = serde_json::from_str(&output.stdout)
        .map_err(|e| format!("--describe produced malformed JSON: {e}"))?;

    if describe.protocol_version != PLUGIN_PROTOCOL_VERSION {
        return Err(format!(
            "reports protocol_version={} but klasp supports only {}; \
             skipping (forward-compat: update the plugin or wait for klasp v1.0)",
            describe.protocol_version, PLUGIN_PROTOCOL_VERSION,
        ));
    }

    Ok(())
}

/// Run `--gate` with the gate input on stdin and parse the output verdict.
fn run_gate(
    plugin_name: &str,
    binary: &std::path::Path,
    config: &CheckConfig,
    state: &RepoState,
) -> Verdict {
    let (args, settings) = match &config.source {
        CheckSourceConfig::Plugin { args, settings, .. } => (args.clone(), settings.clone()),
        _ => (vec![], None),
    };

    let plugin_config = PluginConfig {
        r#type: plugin_name.to_string(),
        args,
        settings,
    };

    let trigger = PluginTrigger::from_event(state.git_event, &state.staged_files);
    let input = PluginGateInput::new(trigger, plugin_config, &state.root, &state.base_ref);

    let input_json = match serde_json::to_string(&input) {
        Ok(j) => j,
        Err(e) => {
            return plugin_error_warn(
                plugin_name,
                format!("--gate: failed to serialize gate input: {e}"),
            );
        }
    };

    // Plugins receive the same env that recipe-based sources do, so they can
    // call back into klasp-aware tools (e.g. `git diff $KLASP_BASE_REF`) and
    // honour the same schema. The protocol spec at docs/plugin-protocol.md
    // §Isolation declares these as the stable v0 env vars.
    let schema_value = GATE_SCHEMA_VERSION.to_string();
    let project_dir = state.root.to_string_lossy();
    let extra_env: [(&str, &str); 3] = [
        ("KLASP_BASE_REF", state.base_ref.as_str()),
        ("KLASP_GATE_SCHEMA", schema_value.as_str()),
        ("KLASP_PROJECT_DIR", project_dir.as_ref()),
    ];

    let timeout = PluginSource::timeout();
    let output = match spawn_and_wait(binary, &["--gate"], Some(&input_json), timeout, &extra_env) {
        Ok(o) => o,
        Err(msg) => {
            return plugin_error_warn(plugin_name, format!("--gate failed: {msg}"));
        }
    };

    let gate_output: PluginGateOutput = match serde_json::from_str(&output.stdout) {
        Ok(o) => o,
        Err(e) => {
            return plugin_error_warn(plugin_name, format!("--gate produced malformed JSON: {e}"));
        }
    };

    if gate_output.protocol_version != PLUGIN_PROTOCOL_VERSION {
        return plugin_error_warn(
            plugin_name,
            format!(
                "--gate output reports protocol_version={} but klasp expects {}; \
                 verdict rejected (plugin describe/gate version mismatch)",
                gate_output.protocol_version, PLUGIN_PROTOCOL_VERSION,
            ),
        );
    }

    convert_plugin_output(gate_output)
}

/// Buffered output from a finished plugin subprocess.
struct ProcessOutput {
    stdout: String,
}

/// Spawn a plugin binary with `args`, optionally write `stdin_payload`, wait up
/// to `timeout`. `extra_env` is a list of additional env vars set on the child
/// (in addition to `KLASP_PLUGIN_PROTOCOL_VERSION`, which is always set).
///
/// Returns `Err(String)` on non-zero exit, spawn error, timeout, or
/// stdout/stderr exceeding `MAX_PLUGIN_OUTPUT_BYTES`. The child is always
/// killed and reaped on error.
fn spawn_and_wait(
    binary: &std::path::Path,
    args: &[&str],
    stdin_payload: Option<&str>,
    timeout: Duration,
    extra_env: &[(&str, &str)],
) -> Result<ProcessOutput, String> {
    let mut cmd = Command::new(binary);
    cmd.args(args)
        .env(
            "KLASP_PLUGIN_PROTOCOL_VERSION",
            PLUGIN_PROTOCOL_VERSION.to_string(),
        )
        .stdin(if stdin_payload.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().map_err(|e| format!("failed to spawn: {e}"))?;

    // Spawn drain threads BEFORE writing stdin so a stdin payload larger than
    // the pipe buffer (~64 KB on Linux, ~16 KB on macOS) cannot deadlock if
    // the child interleaves stdin reads with stdout writes.
    let stdout_handle = child
        .stdout
        .take()
        .map(|r| thread::spawn(move || drain_capped(r, MAX_PLUGIN_OUTPUT_BYTES, "stdout")));
    let stderr_handle = child
        .stderr
        .take()
        .map(|r| thread::spawn(move || drain_capped(r, MAX_PLUGIN_OUTPUT_BYTES, "stderr")));

    // stdin write happens in its own thread so a slow-reading child can't block
    // the parent's poll loop.
    let stdin_handle = if let (Some(payload), Some(mut stdin)) = (stdin_payload, child.stdin.take())
    {
        let payload = payload.to_string();
        Some(thread::spawn(move || {
            // BrokenPipe is acceptable if the child exited early — the child's
            // exit status is the authoritative signal for that case.
            let _ = stdin.write_all(payload.as_bytes());
        }))
    } else {
        None
    };

    let started = Instant::now();
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Some(h) = stdin_handle {
                        let _ = h.join();
                    }
                    let _ = join_drain(stdout_handle);
                    let _ = join_drain(stderr_handle);
                    return Err(format!("timed out after {}s", timeout.as_secs()));
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                if let Some(h) = stdin_handle {
                    let _ = h.join();
                }
                let _ = join_drain(stdout_handle);
                let _ = join_drain(stderr_handle);
                return Err(format!("wait error: {e}"));
            }
        }
    };

    if let Some(h) = stdin_handle {
        let _ = h.join();
    }

    let stdout = join_drain(stdout_handle)?;
    let stderr = join_drain(stderr_handle).unwrap_or_default();

    if !exit_status.success() {
        return Err(format!(
            "exited with status {}{}",
            exit_status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string()),
            if stderr.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", stderr.trim())
            }
        ));
    }

    Ok(ProcessOutput { stdout })
}

/// Read from `reader` into a `String`, bailing with an error if total bytes
/// exceed `cap`. `stream_name` ("stdout" / "stderr") is interpolated into the
/// overflow error message.
fn drain_capped(
    mut reader: impl std::io::Read,
    cap: usize,
    stream_name: &'static str,
) -> Result<String, String> {
    let mut buf = Vec::with_capacity(8 * 1024);
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() + n > cap {
                    return Err(format!("{stream_name} exceeded {cap}-byte cap; killed"));
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Err(e) => return Err(format!("{stream_name} read error: {e}")),
        }
    }
    String::from_utf8(buf).map_err(|e| format!("{stream_name} not valid UTF-8: {e}"))
}

fn join_drain(h: Option<thread::JoinHandle<Result<String, String>>>) -> Result<String, String> {
    match h {
        None => Ok(String::new()),
        Some(h) => h
            .join()
            .map_err(|_| "drain thread panicked".to_string())
            .and_then(|r| r),
    }
}

/// Convert a `PluginGateOutput` into a `Verdict`.
fn convert_plugin_output(output: PluginGateOutput) -> Verdict {
    let findings: Vec<Finding> = output.findings.into_iter().map(Finding::from).collect();
    match output.verdict {
        PluginVerdict::Pass => Verdict::Pass,
        PluginVerdict::Warn => Verdict::Warn {
            findings,
            message: None,
        },
        PluginVerdict::Fail => {
            let message = findings
                .iter()
                .map(|f| f.message.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let message = if message.is_empty() {
                "plugin check failed".to_string()
            } else {
                message
            };
            Verdict::Fail { findings, message }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use klasp_core::{CheckConfig, CheckSourceConfig, GitEvent, RepoState};

    fn plugin_check(name: &str) -> CheckConfig {
        CheckConfig {
            name: format!("plugin-check-{name}"),
            triggers: vec![],
            source: CheckSourceConfig::Plugin {
                name: name.to_string(),
                args: vec![],
                settings: None,
            },
            timeout_secs: None,
        }
    }

    fn state() -> RepoState {
        RepoState {
            root: std::env::current_dir().unwrap(),
            git_event: GitEvent::Commit,
            base_ref: "HEAD~1".to_string(),
            staged_files: vec![],
        }
    }

    #[test]
    fn plugin_source_supports_matching_plugin_config() {
        let source = PluginSource::new("my-linter");
        let check = plugin_check("my-linter");
        assert!(source.supports_config(&check));
    }

    #[test]
    fn plugin_source_does_not_support_other_plugin_name() {
        let source = PluginSource::new("my-linter");
        let check = plugin_check("other-plugin");
        assert!(!source.supports_config(&check));
    }

    #[test]
    fn plugin_source_does_not_support_shell_config() {
        let source = PluginSource::new("my-linter");
        let shell_check = CheckConfig {
            name: "sh".into(),
            triggers: vec![],
            source: CheckSourceConfig::Shell {
                command: "true".into(),
            },
            timeout_secs: None,
        };
        assert!(!source.supports_config(&shell_check));
    }

    #[test]
    fn missing_binary_returns_warn_verdict() {
        let source = PluginSource::new("definitely-does-not-exist-klasp-test");
        let check = plugin_check("definitely-does-not-exist-klasp-test");
        let result = source.run(&check, &state()).expect("run must return Ok");
        assert!(
            matches!(result.verdict, Verdict::Warn { .. }),
            "expected Warn for missing binary, got {:?}",
            result.verdict,
        );
    }

    #[test]
    fn source_id_has_plugin_prefix() {
        let source = PluginSource::new("my-linter");
        assert_eq!(source.source_id(), "plugin:my-linter");
    }

    #[test]
    fn convert_plugin_output_pass() {
        let output = PluginGateOutput {
            protocol_version: 0,
            verdict: PluginVerdict::Pass,
            findings: vec![],
        };
        assert!(matches!(convert_plugin_output(output), Verdict::Pass));
    }

    #[test]
    fn convert_plugin_output_fail_builds_message() {
        use klasp_core::{PluginFinding, Severity};
        let output = PluginGateOutput {
            protocol_version: 0,
            verdict: PluginVerdict::Fail,
            findings: vec![PluginFinding {
                severity: Severity::Error,
                rule: "test/rule".into(),
                file: None,
                line: None,
                message: "something broke".into(),
            }],
        };
        match convert_plugin_output(output) {
            Verdict::Fail { message, findings } => {
                assert!(message.contains("something broke"));
                assert_eq!(findings.len(), 1);
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }
}
