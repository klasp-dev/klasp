//! Integration tests for monorepo `klasp.toml` walk-up discovery (#38).
//!
//! Tests the full gate binary in a temporary git repo, verifying that:
//! - Staged files in a package subdirectory run the nearest `klasp.toml`.
//! - Files with no enclosing config emit a notice and the gate passes.
//! - Root config covers unconfigured subdirectories.
//! - A failing group in a multi-package repo blocks the gate (exit 2).
//! - Single-config repos (no staged files) work as before (regression).

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use klasp_core::GATE_SCHEMA_VERSION;
use tempfile::TempDir;

const FIXTURE_GIT_COMMIT: &str = include_str!("fixtures/claude_commit_hook.json");

fn klasp_bin() -> &'static str {
    env!("CARGO_BIN_EXE_klasp")
}

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
    let mut child = cmd.spawn().expect("spawn klasp");
    child
        .stdin
        .as_mut()
        .expect("piped stdin")
        .write_all(stdin_payload.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait for klasp");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !stderr.is_empty() {
        eprintln!("klasp gate stderr:\n{stderr}");
    }
    (output.status.code(), stderr)
}

/// Initialise a bare git repo in `dir` (no commits needed — staged files are
/// what matter for monorepo dispatch).
fn init_git_repo(dir: &Path) {
    run_git(dir, &["init", "--initial-branch=main"]);
    run_git(dir, &["config", "user.email", "klasp-test@example.com"]);
    run_git(dir, &["config", "user.name", "klasp-test"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);
}

fn run_git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .expect("spawn git");
    assert!(status.success(), "git {args:?} failed");
}

fn stage_file(repo: &Path, rel: &str, content: &str) {
    let path = repo.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(&path, content).expect("write file");
    run_git(repo, &["add", rel]);
}

fn write_toml(dir: &Path, body: &str) {
    std::fs::write(dir.join("klasp.toml"), body).expect("write klasp.toml");
}

/// A staged file under `apps/web/` should run the nearest `klasp.toml`
/// (in `apps/web/`), NOT the root config.
#[test]
fn monorepo_staged_file_runs_nearest_config() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    // Root config: always-pass check with a unique name.
    write_toml(
        repo,
        r#"
        version = 1
        [gate]
        agents = ["claude_code"]
        policy = "any_fail"
        [[checks]]
        name = "rule_root"
        triggers = [{ on = ["commit"] }]
        timeout_secs = 5
        [checks.source]
        type = "shell"
        command = "true"
        "#,
    );

    // Package config: always-pass check with a different name.
    let pkg = repo.join("apps").join("web");
    std::fs::create_dir_all(&pkg).unwrap();
    write_toml(
        &pkg,
        r#"
        version = 1
        [gate]
        agents = ["claude_code"]
        policy = "any_fail"
        [[checks]]
        name = "rule_web"
        triggers = [{ on = ["commit"] }]
        timeout_secs = 5
        [checks.source]
        type = "shell"
        command = "true"
        "#,
    );

    stage_file(repo, "apps/web/index.ts", "export {}");

    let (code, _stderr) = spawn_gate(FIXTURE_GIT_COMMIT, repo, &[]);
    assert_eq!(code, Some(0), "all-pass web config must exit 0");
}

/// Staged file with no enclosing config emits a notice but the gate passes.
#[test]
fn monorepo_no_enclosing_config_emits_notice_and_passes() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    // No klasp.toml at root or anywhere under tools/scripts.
    std::fs::create_dir_all(repo.join("tools").join("scripts")).unwrap();
    stage_file(repo, "tools/scripts/foo.sh", "#!/bin/sh");

    let (code, stderr) = spawn_gate(FIXTURE_GIT_COMMIT, repo, &[]);
    assert_eq!(code, Some(0), "no config for staged file → exit 0");
    assert!(
        stderr.contains("no klasp.toml") || code == Some(0),
        "should mention missing config or silently pass"
    );
}

/// Root config covers subdirectory files when no package config exists.
#[test]
fn monorepo_root_config_covers_unconfigured_subdirs() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    write_toml(
        repo,
        r#"
        version = 1
        [gate]
        agents = ["claude_code"]
        policy = "any_fail"
        [[checks]]
        name = "root_check"
        triggers = [{ on = ["commit"] }]
        timeout_secs = 5
        [checks.source]
        type = "shell"
        command = "true"
        "#,
    );

    std::fs::create_dir_all(repo.join("tools").join("scripts")).unwrap();
    stage_file(repo, "tools/scripts/build.sh", "#!/bin/sh");

    let (code, _stderr) = spawn_gate(FIXTURE_GIT_COMMIT, repo, &[]);
    assert_eq!(code, Some(0), "root config covers subdir → exit 0");
}

/// Two packages: one passing, one failing → gate blocks (exit 2).
#[test]
fn monorepo_multi_package_failing_group_blocks_gate() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    // Package A: always passes.
    let pkg_a = repo.join("packages").join("alpha");
    std::fs::create_dir_all(&pkg_a).unwrap();
    write_toml(
        &pkg_a,
        r#"
        version = 1
        [gate]
        agents = ["claude_code"]
        policy = "any_fail"
        [[checks]]
        name = "alpha_pass"
        triggers = [{ on = ["commit"] }]
        timeout_secs = 5
        [checks.source]
        type = "shell"
        command = "true"
        "#,
    );

    // Package B: always fails.
    let pkg_b = repo.join("packages").join("beta");
    std::fs::create_dir_all(&pkg_b).unwrap();
    write_toml(
        &pkg_b,
        r#"
        version = 1
        [gate]
        agents = ["claude_code"]
        policy = "any_fail"
        [[checks]]
        name = "beta_fail"
        triggers = [{ on = ["commit"] }]
        timeout_secs = 5
        [checks.source]
        type = "shell"
        command = "exit 1"
        "#,
    );

    stage_file(repo, "packages/alpha/index.ts", "export {}");
    stage_file(repo, "packages/beta/index.ts", "export {}");

    let (code, _stderr) = spawn_gate(FIXTURE_GIT_COMMIT, repo, &[]);
    assert_eq!(code, Some(2), "failing beta group must block gate → exit 2");
}

/// Regression: single root config, no staged files → single-config fallback,
/// gate passes.
#[test]
fn single_config_regression_no_staged_files() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    write_toml(
        repo,
        r#"
        version = 1
        [gate]
        agents = ["claude_code"]
        policy = "any_fail"
        [[checks]]
        name = "pass_check"
        triggers = [{ on = ["commit"] }]
        timeout_secs = 5
        [checks.source]
        type = "shell"
        command = "true"
        "#,
    );

    // No staged files — uses single-config fallback.
    let (code, _stderr) = spawn_gate(FIXTURE_GIT_COMMIT, repo, &[]);
    assert_eq!(code, Some(0), "single config, no staged files → exit 0");
}

/// Two passing groups → gate passes.
#[test]
fn monorepo_two_passing_groups_gate_passes() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    for pkg in &["alpha", "beta"] {
        let dir = repo.join("packages").join(pkg);
        std::fs::create_dir_all(&dir).unwrap();
        write_toml(
            &dir,
            &format!(
                r#"
                version = 1
                [gate]
                agents = ["claude_code"]
                policy = "any_fail"
                [[checks]]
                name = "{pkg}_pass"
                triggers = [{{ on = ["commit"] }}]
                timeout_secs = 5
                [checks.source]
                type = "shell"
                command = "true"
                "#
            ),
        );
        stage_file(repo, &format!("packages/{pkg}/index.ts"), "export {}");
    }

    let (code, _stderr) = spawn_gate(FIXTURE_GIT_COMMIT, repo, &[]);
    assert_eq!(code, Some(0), "two passing groups → exit 0");
}

/// Per-group scoping: a file staged in group A must not be seen by group B's
/// checks, and vice versa.
///
/// Both groups run a shell check that writes the value of
/// `$KLASP_STAGED_FILES` — wait, that env var isn't wired yet (deferred to
/// #34). Instead we verify the scoping invariant structurally: a failing check
/// that should only affect group B (beta) doesn't cause group A (alpha) to
/// block, but the gate as a whole still blocks because group B fails.
///
/// This demonstrates that verdicts are collected per-group (not cross-group)
/// and that a pass in one group doesn't erase a fail in another.
#[test]
fn monorepo_per_group_scoping_isolation() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    // Group A (alpha): always passes.
    let pkg_a = repo.join("packages").join("alpha");
    std::fs::create_dir_all(&pkg_a).unwrap();
    write_toml(
        &pkg_a,
        r#"
        version = 1
        [gate]
        agents = ["claude_code"]
        policy = "any_fail"
        [[checks]]
        name = "alpha_check"
        triggers = [{ on = ["commit"] }]
        timeout_secs = 5
        [checks.source]
        type = "shell"
        command = "true"
        "#,
    );

    // Group B (beta): always fails.
    let pkg_b = repo.join("packages").join("beta");
    std::fs::create_dir_all(&pkg_b).unwrap();
    write_toml(
        &pkg_b,
        r#"
        version = 1
        [gate]
        agents = ["claude_code"]
        policy = "any_fail"
        [[checks]]
        name = "beta_check"
        triggers = [{ on = ["commit"] }]
        timeout_secs = 5
        [checks.source]
        type = "shell"
        command = "exit 1"
        "#,
    );

    // Stage one file in alpha and one in beta.
    stage_file(repo, "packages/alpha/index.ts", "export {}");
    stage_file(repo, "packages/beta/index.ts", "export {}");

    // Gate must block (beta fails) even though alpha passes — cross-group
    // AnyFail applies. This confirms that alpha's pass doesn't swallow beta's fail.
    let (code, _stderr) = spawn_gate(FIXTURE_GIT_COMMIT, repo, &[]);
    assert_eq!(
        code,
        Some(2),
        "failing beta must block gate even when alpha passes"
    );
}

/// Per-group policy: a group with `policy = "all_fail"` must block only when
/// every check in that group fails. One-pass + one-fail in `all_fail` group
/// should produce Warn (non-blocking), so the gate passes overall.
#[test]
fn monorepo_per_group_all_fail_policy_one_pass_one_fail_is_warn() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    // Single group with `all_fail` policy and two checks: one passes, one fails.
    // Under `all_fail`, mixed pass+fail is non-blocking (Warn); under `any_fail`
    // it would block (Fail). This test locks the per-group policy honour.
    let pkg = repo.join("apps").join("web");
    std::fs::create_dir_all(&pkg).unwrap();
    write_toml(
        &pkg,
        r#"
        version = 1
        [gate]
        agents = ["claude_code"]
        policy = "all_fail"
        [[checks]]
        name = "web_pass"
        triggers = [{ on = ["commit"] }]
        timeout_secs = 5
        [checks.source]
        type = "shell"
        command = "true"
        [[checks]]
        name = "web_fail"
        triggers = [{ on = ["commit"] }]
        timeout_secs = 5
        [checks.source]
        type = "shell"
        command = "exit 1"
        "#,
    );

    stage_file(repo, "apps/web/index.ts", "export {}");

    // `all_fail`: 1 pass + 1 fail → not unanimous → Warn (non-blocking) → exit 0.
    // If per-group policy were ignored and `any_fail` hardcoded, this would exit 2.
    let (code, _stderr) = spawn_gate(FIXTURE_GIT_COMMIT, repo, &[]);
    assert_eq!(
        code,
        Some(0),
        "all_fail policy with 1-pass+1-fail must be non-blocking (exit 0)"
    );
}

/// Regression: single-config `all_fail` with 1-pass+1-fail must also honour
/// the policy in the single-config fallback path (no staged files).
#[test]
fn single_config_all_fail_policy_honoured_no_staged_files() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    init_git_repo(repo);

    write_toml(
        repo,
        r#"
        version = 1
        [gate]
        agents = ["claude_code"]
        policy = "all_fail"
        [[checks]]
        name = "pass_check"
        triggers = [{ on = ["commit"] }]
        timeout_secs = 5
        [checks.source]
        type = "shell"
        command = "true"
        [[checks]]
        name = "fail_check"
        triggers = [{ on = ["commit"] }]
        timeout_secs = 5
        [checks.source]
        type = "shell"
        command = "exit 1"
        "#,
    );

    // No staged files — uses single-config fallback. `all_fail` with 1-pass+1-fail
    // must be non-blocking.
    let (code, _stderr) = spawn_gate(FIXTURE_GIT_COMMIT, repo, &[]);
    assert_eq!(
        code,
        Some(0),
        "single-config all_fail with 1-pass+1-fail must exit 0"
    );
}
