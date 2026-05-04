//! End-to-end gate flow tests.
//!
//! Spawns the compiled `klasp` binary as a child process, pipes a Claude
//! Code-shaped JSON payload to its stdin, and asserts the exit code matches
//! the four cases [docs/design.md §6] commits to:
//!
//! 1. `Verdict::Fail` from a triggered check → exit 2.
//! 2. All triggered checks pass → exit 0.
//! 3. Command isn't `git commit`/`git push` → exit 0 (no checks fire).
//! 4. `klasp.toml` is absent → exit 0 (fail-open, the gate isn't enrolled).
//!
//! The harness sets `KLASP_GATE_SCHEMA` to match the binary's compiled-in
//! version so the schema handshake passes without re-running `klasp install`.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use klasp_core::GATE_SCHEMA_VERSION;
use tempfile::TempDir;

const FIXTURE_GIT_COMMIT: &str = include_str!("fixtures/claude_commit_hook.json");

/// Path to the `klasp` binary the integration suite spawns. Cargo provides
/// `CARGO_BIN_EXE_<name>` for every `[[bin]]` target in the package; this
/// resolves at compile time and survives `cargo test --target …`.
fn klasp_bin() -> &'static str {
    env!("CARGO_BIN_EXE_klasp")
}

/// Spawn `klasp gate` with the given stdin and env overrides; return the
/// exit code (or `None` on terminating signal).
fn spawn_gate(stdin_payload: &str, project_dir: &Path, extra_env: &[(&str, &str)]) -> Option<i32> {
    let mut cmd = Command::new(klasp_bin());
    cmd.arg("gate")
        .env("KLASP_GATE_SCHEMA", GATE_SCHEMA_VERSION.to_string())
        .env("CLAUDE_PROJECT_DIR", project_dir)
        .current_dir(project_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().expect("spawn klasp binary");
    child
        .stdin
        .as_mut()
        .expect("piped stdin")
        .write_all(stdin_payload.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait for klasp");

    // Surface stderr in test logs so a failing assertion has the runtime
    // notices visible without re-running with `--nocapture`.
    if !output.stderr.is_empty() {
        eprintln!(
            "klasp gate stderr:\n{}",
            String::from_utf8_lossy(&output.stderr),
        );
    }
    output.status.code()
}

fn write_klasp_toml(project_dir: &Path, body: &str) {
    std::fs::write(project_dir.join("klasp.toml"), body).expect("write klasp.toml");
}

#[test]
fn fail_check_blocks_with_exit_2() {
    let project = TempDir::new().expect("tempdir");
    write_klasp_toml(
        project.path(),
        r#"
            version = 1

            [gate]
            agents = ["claude_code"]
            policy = "any_fail"

            [[checks]]
            name = "always-fail"
            triggers = [{ on = ["commit"] }]
            timeout_secs = 5
            [checks.source]
            type = "shell"
            command = "exit 7"
        "#,
    );

    let code = spawn_gate(FIXTURE_GIT_COMMIT, project.path(), &[]);
    assert_eq!(
        code,
        Some(2),
        "failing check on a `git commit` payload must exit 2",
    );
}

#[test]
fn pass_check_returns_exit_0() {
    let project = TempDir::new().expect("tempdir");
    write_klasp_toml(
        project.path(),
        r#"
            version = 1

            [gate]
            agents = ["claude_code"]
            policy = "any_fail"

            [[checks]]
            name = "always-pass"
            triggers = [{ on = ["commit"] }]
            timeout_secs = 5
            [checks.source]
            type = "shell"
            command = "true"
        "#,
    );

    let code = spawn_gate(FIXTURE_GIT_COMMIT, project.path(), &[]);
    assert_eq!(code, Some(0), "passing check must exit 0");
}

#[test]
fn non_git_command_skips_checks_and_returns_0() {
    // Same `klasp.toml` as the failing-check case, but the tool-call
    // payload uses a command that the trigger regex doesn't classify. The
    // gate must short-circuit before running any checks and exit 0.
    let project = TempDir::new().expect("tempdir");
    write_klasp_toml(
        project.path(),
        r#"
            version = 1

            [gate]
            agents = ["claude_code"]

            [[checks]]
            name = "always-fail"
            triggers = [{ on = ["commit"] }]
            timeout_secs = 5
            [checks.source]
            type = "shell"
            command = "exit 7"
        "#,
    );

    let payload = r#"{
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": "ls -la" }
    }"#;

    let code = spawn_gate(payload, project.path(), &[]);
    assert_eq!(
        code,
        Some(0),
        "a non-git command must short-circuit without running checks",
    );
}

#[test]
fn missing_klasp_toml_fails_open() {
    let project = TempDir::new().expect("tempdir");
    // Deliberately do *not* create klasp.toml — the gate runtime must
    // emit a notice and fail open.
    let code = spawn_gate(FIXTURE_GIT_COMMIT, project.path(), &[]);
    assert_eq!(
        code,
        Some(0),
        "missing klasp.toml must fail open with exit 0, never block",
    );
}
