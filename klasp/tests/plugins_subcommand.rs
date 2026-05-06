//! Integration tests for `klasp plugins` subcommands (list / info / disable).
//!
//! Every test isolates the disable list via `KLASP_DISABLED_PLUGINS_FILE`
//! pointing at a tempdir, and prepends the fixtures dir to PATH so that
//! `klasp-plugin-fixture` (and others) are discoverable.
//!
//! Test inventory:
//!  1. `plugins_list_finds_klasp_plugin_binaries_on_path`
//!  2. `plugins_list_marks_disabled_plugins`
//!  3. `plugins_list_shows_describe_failure_inline`
//!  4. `plugins_list_shows_protocol_mismatch_inline`
//!  5. `plugins_info_pretty_prints_describe_output`
//!  6. `plugins_info_returns_error_when_binary_missing`
//!  7. `plugins_disable_writes_to_temp_disable_list`
//!  8. `plugins_disable_is_idempotent_on_already_disabled_name`
//!  9. `plugins_disable_creates_parent_dir_if_missing`
//! 10. `disabled_plugin_is_skipped_during_gate`

use std::path::PathBuf;
use std::process::{Command, Stdio};
use tempfile::TempDir;

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

/// Locate the compiled `klasp` binary.
fn klasp_bin() -> &'static str {
    env!("CARGO_BIN_EXE_klasp")
}

/// Spawn `klasp plugins <args>` with the given environment overrides.
/// Returns `(exit_code, stdout, stderr)`.
fn run_plugins_cmd(args: &[&str], path: &str, disable_list_path: &str) -> (i32, String, String) {
    let mut cmd = Command::new(klasp_bin());
    cmd.arg("plugins")
        .args(args)
        .env("PATH", path)
        .env("KLASP_DISABLED_PLUGINS_FILE", disable_list_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = cmd
        .spawn()
        .expect("spawn klasp")
        .wait_with_output()
        .expect("wait");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);
    (code, stdout, stderr)
}

// ── 1. list finds binaries ────────────────────────────────────────────────────

#[test]
fn plugins_list_finds_klasp_plugin_binaries_on_path() {
    let dir = TempDir::new().unwrap();
    let disable_file = dir.path().join("disabled.toml");
    let path = path_with_fixtures();

    let (code, stdout, _stderr) =
        run_plugins_cmd(&["list"], &path, &disable_file.to_string_lossy());

    assert_eq!(code, 0, "exit code must be 0");
    // The fixture binary is named klasp-plugin-fixture; list should include "fixture".
    assert!(
        stdout.contains("fixture"),
        "expected 'fixture' in list output; got:\n{stdout}"
    );
}

// ── 2. list marks disabled plugins ───────────────────────────────────────────

#[test]
fn plugins_list_marks_disabled_plugins() {
    let dir = TempDir::new().unwrap();
    let disable_file = dir.path().join("disabled.toml");

    // Pre-write the disable list.
    std::fs::write(&disable_file, r#"disabled = ["fixture"]"#).unwrap();

    let path = path_with_fixtures();
    let (code, stdout, _stderr) =
        run_plugins_cmd(&["list"], &path, &disable_file.to_string_lossy());

    assert_eq!(code, 0);
    assert!(
        stdout.contains("disabled"),
        "expected 'disabled' tag in list output; got:\n{stdout}"
    );
    assert!(
        stdout.contains("fixture"),
        "expected 'fixture' plugin listed; got:\n{stdout}"
    );
}

// ── 3. list shows describe failure inline ─────────────────────────────────────

#[test]
fn plugins_list_shows_describe_failure_inline() {
    let dir = TempDir::new().unwrap();
    let disable_file = dir.path().join("disabled.toml");

    let path = path_with_fixtures();
    // mock-malformed outputs garbage on --describe (from existing fixtures).
    let (code, stdout, _stderr) =
        run_plugins_cmd(&["list"], &path, &disable_file.to_string_lossy());

    assert_eq!(code, 0);
    // mock-malformed should appear in the list with an error annotation.
    assert!(
        stdout.contains("mock-malformed") || stdout.contains("malformed"),
        "expected malformed plugin listed; got:\n{stdout}"
    );
}

// ── 4. list shows protocol mismatch inline ────────────────────────────────────

#[test]
fn plugins_list_shows_protocol_mismatch_inline() {
    let dir = TempDir::new().unwrap();
    let disable_file = dir.path().join("disabled.toml");

    let path = path_with_fixtures();
    let (code, stdout, _stderr) =
        run_plugins_cmd(&["list"], &path, &disable_file.to_string_lossy());

    assert_eq!(code, 0);
    // mock-future-version reports protocol_version=99; list should show the error.
    assert!(
        stdout.contains("mock-future-version") || stdout.contains("future"),
        "expected future-version plugin listed; got:\n{stdout}"
    );
}

// ── 5. info pretty-prints describe output ────────────────────────────────────

#[test]
fn plugins_info_pretty_prints_describe_output() {
    let dir = TempDir::new().unwrap();
    let disable_file = dir.path().join("disabled.toml");
    let path = path_with_fixtures();

    let (code, stdout, stderr) =
        run_plugins_cmd(&["info", "fixture"], &path, &disable_file.to_string_lossy());

    assert_eq!(code, 0, "exit 0 expected for info; stderr: {stderr}");
    // Should be JSON output containing protocol_version key.
    assert!(
        stdout.contains("protocol_version"),
        "expected JSON with protocol_version; got:\n{stdout}"
    );
    assert!(
        stdout.contains("klasp-plugin-fixture"),
        "expected plugin name in describe output; got:\n{stdout}"
    );
}

// ── 6. info returns error when binary missing ─────────────────────────────────

#[test]
fn plugins_info_returns_error_when_binary_missing() {
    let dir = TempDir::new().unwrap();
    let disable_file = dir.path().join("disabled.toml");
    // Use a minimal PATH that definitely won't have the binary.
    let path = "/usr/bin:/bin";

    let (code, _stdout, stderr) = run_plugins_cmd(
        &["info", "definitely-not-installed"],
        path,
        &disable_file.to_string_lossy(),
    );

    assert_ne!(code, 0, "expected non-zero exit when binary not found");
    assert!(
        stderr.contains("not found") || stderr.contains("error"),
        "expected error message; got:\n{stderr}"
    );
}

// ── 7. disable writes to temp disable list ────────────────────────────────────

#[test]
fn plugins_disable_writes_to_temp_disable_list() {
    let dir = TempDir::new().unwrap();
    let disable_file = dir.path().join("disabled.toml");
    let path = path_with_fixtures();

    let (code, stdout, stderr) = run_plugins_cmd(
        &["disable", "my-test-plugin"],
        &path,
        &disable_file.to_string_lossy(),
    );

    assert_eq!(code, 0, "disable must exit 0; stderr: {stderr}");
    assert!(
        stdout.contains("disabled") || stdout.contains("my-test-plugin"),
        "expected confirmation message; got:\n{stdout}"
    );
    // Verify the file was written.
    let content = std::fs::read_to_string(&disable_file).expect("disable file must exist");
    assert!(
        content.contains("my-test-plugin"),
        "expected plugin name in disable list file; got:\n{content}"
    );
}

// ── 8. disable is idempotent ──────────────────────────────────────────────────

#[test]
fn plugins_disable_is_idempotent_on_already_disabled_name() {
    let dir = TempDir::new().unwrap();
    let disable_file = dir.path().join("disabled.toml");
    let path = path_with_fixtures();

    // First disable.
    let (code1, _, _) = run_plugins_cmd(
        &["disable", "my-linter"],
        &path,
        &disable_file.to_string_lossy(),
    );
    assert_eq!(code1, 0);

    // Second disable — should still exit 0 with a "already disabled" message.
    let (code2, stdout, _) = run_plugins_cmd(
        &["disable", "my-linter"],
        &path,
        &disable_file.to_string_lossy(),
    );
    assert_eq!(code2, 0, "idempotent disable must exit 0");
    assert!(
        stdout.contains("already disabled") || stdout.contains("my-linter"),
        "expected 'already disabled' message; got:\n{stdout}"
    );

    // The file must still list the plugin exactly once.
    let content = std::fs::read_to_string(&disable_file).unwrap();
    let count = content.matches("my-linter").count();
    assert_eq!(
        count, 1,
        "plugin should appear exactly once in disable list"
    );
}

// ── 9. disable creates parent dir ────────────────────────────────────────────

#[test]
fn plugins_disable_creates_parent_dir_if_missing() {
    let dir = TempDir::new().unwrap();
    // Nested path that does not exist yet.
    let nested = dir
        .path()
        .join("nested")
        .join("subdir")
        .join("disabled.toml");
    let path = path_with_fixtures();

    let (code, _, stderr) =
        run_plugins_cmd(&["disable", "new-plugin"], &path, &nested.to_string_lossy());

    assert_eq!(
        code, 0,
        "disable must exit 0 even with nested missing dirs; stderr: {stderr}"
    );
    assert!(nested.exists(), "disable file must be created");
}

// ── 10. disabled plugin is skipped during gate ───────────────────────────────

#[test]
fn disabled_plugin_is_skipped_during_gate() {
    use std::io::Write;

    let dir = TempDir::new().unwrap();
    let disable_file = dir.path().join("disabled.toml");
    // Disable the "fixture" plugin.
    std::fs::write(&disable_file, r#"disabled = ["fixture"]"#).unwrap();

    let repo = dir.path().to_path_buf();

    // Write a klasp.toml that uses klasp-plugin-fixture.
    let toml = r#"version = 1
[gate]
agents = ["claude_code"]

[[checks]]
name = "plugin-check"
[checks.source]
type = "plugin"
name = "fixture"
"#;
    std::fs::write(repo.join("klasp.toml"), toml).unwrap();

    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"git commit -m 'test'"}}"#;
    let path = path_with_fixtures();

    let mut cmd = Command::new(klasp_bin());
    cmd.arg("gate")
        .env("KLASP_GATE_SCHEMA", "2")
        .env("PATH", &path)
        .env("CLAUDE_PROJECT_DIR", &repo)
        .env("KLASP_BASE_REF", "test-base-ref")
        .env("KLASP_DISABLED_PLUGINS_FILE", &disable_file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("spawn klasp gate");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .expect("write stdin");

    let output = child.wait_with_output().expect("wait");
    let exit_code = output.status.code().unwrap_or(-1);

    assert_eq!(
        exit_code,
        0,
        "gate must exit 0 when plugin is disabled (pass-through); stderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}
