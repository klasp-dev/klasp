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
use std::thread;
use std::time::{Duration, Instant};

use klasp_core::{
    plugin_error_warn, CheckConfig, CheckResult, CheckSource, CheckSourceConfig, CheckSourceError,
    Finding, PluginConfig, PluginGateInput, PluginGateOutput, PluginTrigger, PluginVerdict,
    RepoState, Verdict, PLUGIN_PROTOCOL_VERSION,
};

/// Default plugin subprocess timeout. Intentionally shorter than the 120 s
/// shell default — plugins that hang are more likely misuse than intentional
/// long-running operations.
const DEFAULT_PLUGIN_TIMEOUT_SECS: u64 = 60;

/// Poll granularity for the `try_wait` loop, matching shell.rs.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Source ID prefix for all plugin sources.
const SOURCE_ID_PREFIX: &str = "plugin";

/// `CheckSource` impl for subprocess plugins.
///
/// One instance per plugin name. Constructed lazily by `SourceRegistry` when
/// a config check's `type = "plugin"` + `name` pair has not been seen before.
pub struct PluginSource {
    /// Plugin name, e.g. `"my-linter"` → binary `klasp-plugin-my-linter`.
    plugin_name: String,
    /// Cached source_id so `source_id()` can return `&str` tied to `&self`.
    id: String,
}

impl PluginSource {
    /// Construct a `PluginSource` for `plugin_name`. Does not verify the binary
    /// exists — that happens at `run()` time so the error surfaces as
    /// `Verdict::Warn` rather than a hard error at registry build time.
    pub fn new(plugin_name: impl Into<String>) -> Self {
        let plugin_name = plugin_name.into();
        let id = format!("{SOURCE_ID_PREFIX}:{plugin_name}");
        Self { plugin_name, id }
    }

    /// Read the plugin timeout from the environment, falling back to the default.
    fn timeout() -> Duration {
        let secs = std::env::var("KLASP_PLUGIN_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_PLUGIN_TIMEOUT_SECS);
        Duration::from_secs(secs)
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
        let verdict = run_plugin(&self.plugin_name, config, state);
        Ok(CheckResult {
            source_id: self.id.clone(),
            check_name: config.name.clone(),
            verdict,
            raw_stdout: None,
            raw_stderr: None,
        })
    }
}

/// Top-level plugin invocation. All error paths return `Verdict::Warn`.
fn run_plugin(plugin_name: &str, config: &CheckConfig, state: &RepoState) -> Verdict {
    let binary = match which::which(format!("klasp-plugin-{plugin_name}")) {
        Ok(p) => p,
        Err(_) => {
            return plugin_error_warn(format!(
                "plugin `{plugin_name}`: binary `klasp-plugin-{plugin_name}` not found on $PATH"
            ));
        }
    };

    // Phase 1: --describe (forward-compat check).
    if let Some(warn) = check_describe(plugin_name, &binary) {
        return warn;
    }

    // Phase 2: --gate.
    run_gate(plugin_name, &binary, config, state)
}

/// Run `--describe` and validate the protocol version. Returns `Some(Verdict::Warn)`
/// on any error; `None` means the plugin is compatible.
fn check_describe(plugin_name: &str, binary: &std::path::Path) -> Option<Verdict> {
    let timeout = PluginSource::timeout();
    let output = match spawn_and_wait(binary, &["--describe"], None, timeout) {
        Ok(o) => o,
        Err(msg) => {
            return Some(plugin_error_warn(format!(
                "plugin `{plugin_name}` --describe failed: {msg}"
            )));
        }
    };

    let describe: klasp_core::PluginDescribe = match serde_json::from_str(&output.stdout) {
        Ok(d) => d,
        Err(e) => {
            return Some(plugin_error_warn(format!(
                "plugin `{plugin_name}` --describe produced malformed JSON: {e}"
            )));
        }
    };

    if describe.protocol_version != PLUGIN_PROTOCOL_VERSION {
        return Some(plugin_error_warn(format!(
            "plugin `{plugin_name}` reports protocol_version={} but klasp supports only {}; \
             skipping (forward-compat: update the plugin or wait for klasp v1.0)",
            describe.protocol_version, PLUGIN_PROTOCOL_VERSION,
        )));
    }

    None
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
            return plugin_error_warn(format!(
                "plugin `{plugin_name}` --gate: failed to serialize gate input: {e}"
            ));
        }
    };

    let timeout = PluginSource::timeout();
    let output = match spawn_and_wait(binary, &["--gate"], Some(&input_json), timeout) {
        Ok(o) => o,
        Err(msg) => {
            return plugin_error_warn(format!("plugin `{plugin_name}` --gate failed: {msg}"));
        }
    };

    let gate_output: PluginGateOutput = match serde_json::from_str(&output.stdout) {
        Ok(o) => o,
        Err(e) => {
            return plugin_error_warn(format!(
                "plugin `{plugin_name}` --gate produced malformed JSON: {e}"
            ));
        }
    };

    convert_plugin_output(gate_output)
}

/// Buffered output from a finished plugin subprocess.
struct ProcessOutput {
    stdout: String,
    #[allow(dead_code)]
    stderr: String,
}

/// Spawn a plugin binary with `args`, optionally write `stdin_payload`, wait up
/// to `timeout`. Returns `Err(String)` on non-zero exit, spawn error, or timeout.
fn spawn_and_wait(
    binary: &std::path::Path,
    args: &[&str],
    stdin_payload: Option<&str>,
    timeout: Duration,
) -> Result<ProcessOutput, String> {
    let mut child = Command::new(binary)
        .args(args)
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
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn: {e}"))?;

    // Write stdin before draining — avoids deadlock on small payloads since the
    // child reads stdin and only then starts writing stdout.
    if let Some(payload) = stdin_payload {
        if let Some(mut stdin) = child.stdin.take() {
            if let Err(e) = stdin.write_all(payload.as_bytes()) {
                // Broken pipe is acceptable if the child already exited.
                let _ = e;
            }
            // stdin is closed when it drops here, signalling EOF to the child.
        }
    }

    // Drain stdout / stderr in background threads to avoid pipe-full deadlock.
    let stdout_handle = child.stdout.take().map(|r| {
        thread::spawn(move || {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::BufReader::new(r), &mut buf).map(|_| buf)
        })
    });
    let stderr_handle = child.stderr.take().map(|r| {
        thread::spawn(move || {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::BufReader::new(r), &mut buf).map(|_| buf)
        })
    });

    let started = Instant::now();
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    join_optional_handle(stdout_handle);
                    join_optional_handle(stderr_handle);
                    return Err(format!("timed out after {}s", timeout.as_secs()));
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                join_optional_handle(stdout_handle);
                join_optional_handle(stderr_handle);
                return Err(format!("wait error: {e}"));
            }
        }
    };

    let stdout = collect_handle(stdout_handle);
    let stderr = collect_handle(stderr_handle);

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

    Ok(ProcessOutput { stdout, stderr })
}

fn join_optional_handle(h: Option<thread::JoinHandle<std::io::Result<String>>>) {
    if let Some(h) = h {
        let _ = h.join();
    }
}

fn collect_handle(h: Option<thread::JoinHandle<std::io::Result<String>>>) -> String {
    h.and_then(|h| h.join().ok())
        .and_then(|r| r.ok())
        .unwrap_or_default()
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
