//! Integration tests for the v0.3 plugin protocol (`PLUGIN_PROTOCOL_VERSION = 0`).
//!
//! Each test prepends the `tests/fixtures/plugin/` directory to `$PATH` so
//! the mock plugin scripts are discoverable by `which::which`. Tests exercise
//! the full gate pipeline: `SourceRegistry::find_for` → `PluginSource::run`.
//!
//! Test list:
//! 1. `plugin_passing_returns_verdict_pass`
//! 2. `plugin_failing_returns_findings`
//! 3. `plugin_not_on_path_warns_and_continues`
//! 4. `plugin_crashing_exits_nonzero_warns`
//! 5. `plugin_malformed_json_warns`
//! 6. `plugin_future_protocol_version_warns`
//! 7. `plugin_timeout_warns`
//! 8. `plugin_receives_klasp_env`

use std::path::PathBuf;

use klasp_core::{Verdict, PLUGIN_PROTOCOL_VERSION};

/// Absolute path to the `tests/fixtures/plugin/` directory.
fn fixtures_dir() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    manifest.join("tests").join("fixtures").join("plugin")
}

/// Return a PATH string with the fixtures dir prepended.
fn path_with_fixtures() -> String {
    let fixtures = fixtures_dir();
    let host_path = std::env::var("PATH").unwrap_or_default();
    format!("{}:{}", fixtures.display(), host_path)
}

/// Spawn `klasp gate` with a synthetic `git commit` payload and a temporary
/// `klasp.toml` configured with a plugin check. Returns a `Verdict` derived
/// from the binary's exit code and stderr output.
///
/// This exercises the full gate pipeline end-to-end, including:
/// - `SourceRegistry::find_for` falling through to `PluginSource`
/// - `check_describe` (forward-compat check)
/// - `run_gate` (stdin/stdout JSON protocol)
/// - All failure-mode → `Verdict::Warn` conversions
fn run_gate_with_plugin(
    plugin_name: &str,
    path: &str,
    extra_env: &[(&str, &str)],
) -> (i32, String) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use tempfile::TempDir;

    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path();

    let toml = format!(
        r#"version = 1
[gate]
agents = ["claude_code"]

[[checks]]
name = "plugin-check"
[checks.source]
type = "plugin"
name = "{plugin_name}"
"#
    );
    std::fs::write(repo.join("klasp.toml"), toml).expect("write klasp.toml");

    // Synthetic Claude Code `git commit` payload.
    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"git commit -m 'test'"}}"#;

    // Locate the `klasp` binary produced by cargo.
    let binary = std::env::current_exe()
        .expect("current_exe")
        .parent()
        .expect("parent")
        .parent() // strip `deps/` from integration test binary path
        .expect("parent2")
        .join("klasp");

    let mut cmd = Command::new(&binary);
    cmd.arg("gate")
        .env("KLASP_GATE_SCHEMA", "2")
        .env("PATH", path)
        .env("CLAUDE_PROJECT_DIR", repo)
        .env("KLASP_BASE_REF", "test-base-ref")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().expect("spawn klasp gate");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .expect("write stdin");

    let output = child.wait_with_output().expect("wait");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);
    (exit_code, stderr)
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// 1. Mock plugin returns `pass` → gate exits 0.
#[test]
fn plugin_passing_returns_verdict_pass() {
    let path = path_with_fixtures();
    let (exit_code, stderr) = run_gate_with_plugin("mock-passing", &path, &[]);
    assert_eq!(
        exit_code, 0,
        "expected exit 0 (pass), got {exit_code}; stderr: {stderr}"
    );
}

/// 2. Mock plugin returns `fail` with 2 findings → gate exits 2 (block).
#[test]
fn plugin_failing_returns_findings() {
    let path = path_with_fixtures();
    let (exit_code, stderr) = run_gate_with_plugin("mock-failing", &path, &[]);
    assert_eq!(
        exit_code, 2,
        "expected exit 2 (fail/block), got {exit_code}; stderr: {stderr}"
    );
}

/// 3. Plugin binary not on PATH → gate exits 0 (warn, not block) and
///    emits a `klasp::plugin` notice on stderr.
#[test]
fn plugin_not_on_path_warns_and_continues() {
    // PATH does NOT contain the fixtures dir so `klasp-plugin-missing-plugin`
    // cannot be found.
    let empty_path = "/usr/bin:/bin";
    let (exit_code, stderr) = run_gate_with_plugin("missing-plugin", empty_path, &[]);
    assert_eq!(
        exit_code, 0,
        "expected exit 0 (warn/pass), got {exit_code}; stderr: {stderr}"
    );
    // The gate should mention the missing binary in its output.
    assert!(
        stderr.contains("missing-plugin") || stderr.contains("plugin"),
        "expected stderr to mention the missing plugin; stderr: {stderr}"
    );
}

/// 4. Mock plugin crashes (exits non-zero) → gate exits 0 (warn, not block).
#[test]
fn plugin_crashing_exits_nonzero_warns() {
    let path = path_with_fixtures();
    let (exit_code, _stderr) = run_gate_with_plugin("mock-crashing", &path, &[]);
    assert_eq!(
        exit_code, 0,
        "expected exit 0 (warn/pass) for crashing plugin, got {exit_code}"
    );
}

/// 5. Mock plugin prints garbage JSON on `--gate` → gate exits 0 (warn, not block).
#[test]
fn plugin_malformed_json_warns() {
    let path = path_with_fixtures();
    let (exit_code, _stderr) = run_gate_with_plugin("mock-malformed", &path, &[]);
    assert_eq!(
        exit_code, 0,
        "expected exit 0 (warn/pass) for malformed JSON plugin, got {exit_code}"
    );
}

/// 6. Mock plugin reports `protocol_version=99` → gate exits 0 (forward-compat warn).
#[test]
fn plugin_future_protocol_version_warns() {
    let path = path_with_fixtures();
    let (exit_code, stderr) = run_gate_with_plugin("mock-future-version", &path, &[]);
    assert_eq!(
        exit_code, 0,
        "expected exit 0 (warn) for future protocol version, got {exit_code}; stderr: {stderr}"
    );
    // Should mention the protocol_version mismatch.
    assert!(
        stderr.contains("protocol_version") || stderr.contains("plugin"),
        "expected stderr to mention protocol_version mismatch; stderr: {stderr}"
    );
}

/// 7. Mock plugin sleeps 10s; `KLASP_PLUGIN_TIMEOUT_SECS=1` → gate exits 0 after timeout.
#[test]
fn plugin_timeout_warns() {
    let path = path_with_fixtures();
    let (exit_code, _stderr) =
        run_gate_with_plugin("mock-slow", &path, &[("KLASP_PLUGIN_TIMEOUT_SECS", "1")]);
    assert_eq!(
        exit_code, 0,
        "expected exit 0 (warn/pass) after timeout, got {exit_code}"
    );
}

/// 8. Plugin receives `KLASP_BASE_REF` → returns pass (env check succeeds).
#[test]
fn plugin_receives_klasp_env() {
    let path = path_with_fixtures();
    // The mock-env-check plugin asserts KLASP_BASE_REF is non-empty and fails
    // if it isn't. `run_gate_with_plugin` always sets KLASP_BASE_REF="test-base-ref".
    let (exit_code, stderr) = run_gate_with_plugin("mock-env-check", &path, &[]);
    assert_eq!(
        exit_code, 0,
        "expected exit 0 (KLASP_BASE_REF was set), got {exit_code}; stderr: {stderr}"
    );
}

/// Verify `PLUGIN_PROTOCOL_VERSION = 0` constant is accessible from `klasp_core`.
#[test]
fn plugin_protocol_version_constant_is_zero() {
    assert_eq!(
        PLUGIN_PROTOCOL_VERSION, 0,
        "PLUGIN_PROTOCOL_VERSION must be 0 (experimental tier)"
    );
}

// Suppress unused-import warning from the Verdict import used only in doc.
#[allow(unused_imports)]
use Verdict as _Verdict;
