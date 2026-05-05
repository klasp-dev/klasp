//! Integration tests for `[gate].parallel = true` (issue #34, v0.2.5).
//!
//! These tests verify that:
//! 1. Five independent 5-second checks complete in under 15 seconds when
//!    `parallel = true` — demonstrating rayon's work-stealing speedup. The
//!    threshold is generous enough to absorb subprocess-spawn overhead and
//!    CI runner contention while still being well under the 25 s sequential
//!    baseline (a >40% speedup signal even on a slow runner).
//! 2. The sequential baseline takes at least 20 seconds (marked `#[ignore]`
//!    because it is slow — run manually as a proof, not a CI gate).
//! 3. Two checks writing to the same temp file do NOT panic klasp — the
//!    output is non-deterministic, which is the documented behaviour.
//!    The contract that checks must be stateless is enforced by docs, not
//!    by the runtime.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use klasp_core::GATE_SCHEMA_VERSION;
use tempfile::TempDir;

const FIXTURE_GIT_COMMIT: &str = r#"{
    "hook_event_name": "PreToolUse",
    "tool_name": "Bash",
    "tool_input": { "command": "git commit -m msg" }
}"#;

fn klasp_bin() -> &'static str {
    env!("CARGO_BIN_EXE_klasp")
}

fn spawn_gate(
    stdin_payload: &str,
    project_dir: &Path,
) -> (Option<i32>, String, std::time::Duration) {
    let mut cmd = Command::new(klasp_bin());
    cmd.arg("gate")
        .env("KLASP_GATE_SCHEMA", GATE_SCHEMA_VERSION.to_string())
        .env("CLAUDE_PROJECT_DIR", project_dir)
        .current_dir(project_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("spawn klasp binary");
    child
        .stdin
        .as_mut()
        .expect("piped stdin")
        .write_all(stdin_payload.as_bytes())
        .expect("write stdin");

    let t0 = Instant::now();
    let output = child.wait_with_output().expect("wait for klasp");
    let elapsed = t0.elapsed();

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !stderr.is_empty() {
        eprintln!("klasp gate stderr:\n{stderr}");
    }
    (output.status.code(), stderr, elapsed)
}

fn write_klasp_toml(project_dir: &Path, body: &str) {
    std::fs::write(project_dir.join("klasp.toml"), body).expect("write klasp.toml");
}

/// Build the TOML for N shell checks each running `sleep N_SECS` (or a
/// portable equivalent), under the given `parallel` setting.
fn sleep_checks_toml(count: usize, sleep_secs: u64, parallel: bool) -> String {
    let mut toml = format!(
        r#"version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
parallel = {parallel}

"#
    );
    for i in 0..count {
        toml.push_str(&format!(
            r#"[[checks]]
name = "sleep-{i}"
triggers = [{{ on = ["commit"] }}]
timeout_secs = 30
[checks.source]
type = "shell"
command = "sleep {sleep_secs}"

"#
        ));
    }
    toml
}

/// 5 checks × 5-second sleep, `parallel = true`.
///
/// All checks execute concurrently so the wall clock should be ~5 s. The
/// 15 s threshold absorbs subprocess-spawn overhead and CI runner
/// contention (one CI flake observed at 10.04 s on a 10 s threshold) while
/// still being well under the 25 s sequential baseline.
#[cfg(unix)]
#[test]
fn parallel_completes_5x5s_workload_in_under_15s() {
    let project = TempDir::new().expect("tempdir");
    write_klasp_toml(project.path(), &sleep_checks_toml(5, 5, true));

    let t0 = Instant::now();
    let (code, _stderr, _elapsed) = spawn_gate(FIXTURE_GIT_COMMIT, project.path());
    let wall = t0.elapsed();

    eprintln!("parallel 5×5s wall clock: {wall:.2?}");

    assert_eq!(code, Some(0), "all checks pass, gate must exit 0");
    assert!(
        wall.as_secs() < 15,
        "expected parallel 5×5s to complete in <15s, took {wall:.2?}",
    );
}

/// 5 checks × 5-second sleep, `parallel = false`.
///
/// Sequential execution takes ~25 s. This test verifies the baseline and
/// is marked `#[ignore]` — run manually with `cargo test -- --ignored
/// sequential_5x5s_workload_takes_at_least_20s --nocapture` to confirm.
#[cfg(unix)]
#[ignore]
#[test]
fn sequential_5x5s_workload_takes_at_least_20s() {
    let project = TempDir::new().expect("tempdir");
    write_klasp_toml(project.path(), &sleep_checks_toml(5, 5, false));

    let t0 = Instant::now();
    let (code, _stderr, _elapsed) = spawn_gate(FIXTURE_GIT_COMMIT, project.path());
    let wall = t0.elapsed();

    eprintln!("sequential 5×5s wall clock: {wall:.2?}");

    assert_eq!(code, Some(0), "all checks pass, gate must exit 0");
    assert!(
        wall.as_secs() >= 20,
        "expected sequential 5×5s to take >=20s, took {wall:.2?}",
    );
}

/// Two checks both writing to the same temp file via `echo X > $FILE`.
///
/// Run with `parallel = true`. klasp must NOT panic. The output in the
/// shared file is non-deterministic — last writer wins — which is the
/// documented contract: checks must be stateless when parallel mode is
/// enabled. Writing to shared state will race; klasp documents this but
/// does not detect or prevent it.
///
/// This test proves klasp is stable (no crash, no panic, exit 0) even
/// when the user violates the stateless contract. It also anchors the
/// documentation requirement: if the design.md §6.1 text about stateless
/// checks is ever removed, this assertion will catch it.
#[cfg(unix)]
#[test]
fn parallel_with_shared_tempfile_race_documents_contract() {
    let project = TempDir::new().expect("tempdir");
    let shared_file = project.path().join("klasp-race-test.txt");

    let toml = format!(
        r#"version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
parallel = true

[[checks]]
name = "writer-a"
triggers = [{{ on = ["commit"] }}]
timeout_secs = 10
[checks.source]
type = "shell"
command = "echo A > {path}"

[[checks]]
name = "writer-b"
triggers = [{{ on = ["commit"] }}]
timeout_secs = 10
[checks.source]
type = "shell"
command = "echo B > {path}"
"#,
        path = shared_file.display()
    );
    write_klasp_toml(project.path(), &toml);

    // klasp must not panic; exit code must be 0 (both shell checks exit 0).
    // The file content is non-deterministic: A or B depending on scheduler
    // ordering. That is the documented race behaviour.
    let (code, _stderr, _elapsed) = spawn_gate(FIXTURE_GIT_COMMIT, project.path());
    assert_eq!(
        code,
        Some(0),
        "racy checks must not crash klasp; gate must exit 0"
    );

    // Assert that the design.md §6.1 section about the stateless-checks
    // contract exists. If it is removed, this test breaks and re-documents
    // the requirement.
    let design_md = include_str!("../../docs/design.md");
    assert!(
        design_md.contains("stateless"),
        "docs/design.md must contain the stateless-checks contract for parallel mode",
    );
    assert!(
        design_md.contains("parallel"),
        "docs/design.md must document the parallel field",
    );
}
