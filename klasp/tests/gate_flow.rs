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
/// exit code (or `None` on terminating signal) and the captured stderr.
fn spawn_gate(
    stdin_payload: &str,
    project_dir: &Path,
    extra_env: &[(&str, &str)],
) -> (Option<i32>, String) {
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

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    // Surface stderr in test logs so a failing assertion has the runtime
    // notices visible without re-running with `--nocapture`.
    if !stderr.is_empty() {
        eprintln!("klasp gate stderr:\n{stderr}");
    }
    (output.status.code(), stderr)
}

fn write_klasp_toml(project_dir: &Path, body: &str) {
    std::fs::write(project_dir.join("klasp.toml"), body).expect("write klasp.toml");
}

/// Initialise a fresh git repo with `commits` empty-ish commits. Used by the
/// `KLASP_BASE_REF` test to give `compute_base_ref` something to resolve
/// (`HEAD~1` requires at least two commits).
fn init_repo_with_commits(dir: &Path, commits: usize) {
    run_git(dir, &["init", "--initial-branch=main"]);
    run_git(dir, &["config", "user.email", "klasp-test@example.com"]);
    run_git(dir, &["config", "user.name", "klasp-test"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);
    for i in 0..commits {
        std::fs::write(dir.join(format!("f{i}.txt")), format!("commit {i}"))
            .expect("write fixture file");
        run_git(dir, &["add", "."]);
        run_git(dir, &["commit", "-m", &format!("c{i}")]);
    }
}

fn run_git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .expect("spawn git");
    assert!(status.success(), "git {args:?} failed");
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

    let (code, _stderr) = spawn_gate(FIXTURE_GIT_COMMIT, project.path(), &[]);
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

    let (code, _stderr) = spawn_gate(FIXTURE_GIT_COMMIT, project.path(), &[]);
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

    let payload = r#"{
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": "ls -la" }
    }"#;

    let (code, _stderr) = spawn_gate(payload, project.path(), &[]);
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
    let (code, stderr) = spawn_gate(FIXTURE_GIT_COMMIT, project.path(), &[]);
    assert_eq!(
        code,
        Some(0),
        "missing klasp.toml must fail open with exit 0, never block",
    );
    // Anchor on the notice prefix so a future refactor that silently
    // returns `Ok(default_config)` instead of `Err(ConfigNotFound)` doesn't
    // bypass the fail-open notice without breaking this test.
    assert!(
        stderr.contains("klasp-gate:"),
        "expected fail-open notice on stderr, got: {stderr:?}",
    );
}

#[test]
fn klasp_base_ref_is_exposed_to_shell_checks() {
    // End-to-end: the gate runtime computes a merge-base and threads it
    // through `RepoState` → `ShellSource::run` → the child's `KLASP_BASE_REF`
    // env var. Without a remote, `compute_base_ref` falls back to `HEAD~1`,
    // which is what the check command below asserts on.
    //
    // The check command writes the env var's value to a sentinel file in the
    // repo root, then exits 1 if it's empty. Reading the file after the
    // gate runs gives us the captured value to assert on without bolting
    // any additional ingestion plumbing onto the gate.
    let project = TempDir::new().expect("tempdir");

    // Initialise a minimal git repo so `compute_base_ref` has something to
    // resolve. Two commits give `HEAD~1` a target.
    init_repo_with_commits(project.path(), 2);

    let sentinel = project.path().join("base_ref.txt");
    write_klasp_toml(
        project.path(),
        &format!(
            r#"
                version = 1

                [gate]
                agents = ["claude_code"]
                policy = "any_fail"

                [[checks]]
                name = "echo-base-ref"
                triggers = [{{ on = ["commit"] }}]
                timeout_secs = 5
                [checks.source]
                type = "shell"
                command = 'printf "$KLASP_BASE_REF" > {sentinel}; test -n "$KLASP_BASE_REF"'
            "#,
            sentinel = sentinel.display(),
        ),
    );

    let (code, stderr) = spawn_gate(FIXTURE_GIT_COMMIT, project.path(), &[]);
    assert_eq!(
        code,
        Some(0),
        "check passes only if KLASP_BASE_REF is non-empty in the child env\nstderr:\n{stderr}",
    );

    let captured = std::fs::read_to_string(&sentinel).expect("read sentinel file");
    assert!(
        !captured.is_empty(),
        "KLASP_BASE_REF must be exported with a non-empty value, got: {captured:?}",
    );
    // Without an upstream / `origin/main`, the runtime falls back to
    // `HEAD~1`. With two commits, that resolves cleanly.
    assert_eq!(
        captured, "HEAD~1",
        "no-remote fallback must be HEAD~1, got: {captured:?}",
    );
}

#[test]
fn source_runtime_error_fails_open() {
    // Configure a check whose `ShellSource::run` returns
    // `CheckSourceError::Timeout` mid-flight — the gate handler must emit a
    // per-check notice and continue (no verdict appended). With no verdicts
    // and `policy = "any_fail"`, the aggregate is `Verdict::Pass` → exit 0.
    let project = TempDir::new().expect("tempdir");
    write_klasp_toml(
        project.path(),
        r#"
            version = 1

            [gate]
            agents = ["claude_code"]
            policy = "any_fail"

            [[checks]]
            name = "always-times-out"
            triggers = [{ on = ["commit"] }]
            timeout_secs = 0
            [checks.source]
            type = "shell"
            command = "sleep 1"
        "#,
    );

    let (code, stderr) = spawn_gate(FIXTURE_GIT_COMMIT, project.path(), &[]);
    assert_eq!(
        code,
        Some(0),
        "source runtime error must fail open (exit 0), got code = {code:?}\nstderr:\n{stderr}",
    );
    assert!(
        stderr.contains("klasp-gate:"),
        "expected fail-open notice on stderr, got: {stderr:?}",
    );
    assert!(
        stderr.contains("always-times-out"),
        "notice should mention the check name, got: {stderr:?}",
    );
}
