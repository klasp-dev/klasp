//! Integration test: drive `klasp gate` against the `pre_commit` recipe
//! using captured pre-commit 3.x and 4.x stdout fixtures.
//!
//! Acceptance for issue #30 calls for "captured pre-commit output (multiple
//! pre-commit versions: 3.x and 4.x at minimum)" — this file owns that
//! coverage.
//!
//! ## Strategy
//!
//! Real pre-commit may not be on the CI runner's `PATH`, and even if it is
//! we don't want the test depending on a specific installed version.
//! Instead, the harness writes a tiny shell shim called `pre-commit` to a
//! tempdir, prepends that tempdir to `PATH`, and parameterises the shim
//! with two env vars:
//!
//! - `FAKE_PRE_COMMIT_STDOUT` → path to a fixture file the shim `cat`s.
//! - `FAKE_PRE_COMMIT_EXIT` → exit code the shim returns (default 0).
//!
//! The shim also handles `pre-commit --version` so the recipe's lazy
//! version sniff has something to read — the version we report changes
//! based on which fixture pair is in play, so a single test exercises
//! the 3.x or 4.x branch of the version compatibility check.
//!
//! ## Why a shim and not in-process unit tests
//!
//! The pre-commit recipe's exit-code → verdict mapping is already
//! exercised in `klasp::sources::pre_commit`'s unit tests. What this
//! file adds is:
//!
//! 1. The full `klasp gate` flow over the new recipe (config parse →
//!    registry dispatch → recipe → exit code), proving the new variant
//!    is wired end-to-end.
//! 2. Confidence that real pre-commit stdout (captured from documented
//!    output formats) parses as the recipe's per-hook findings — the
//!    unit tests use synthesised strings, the fixtures are the real
//!    contract.
//! 3. Cross-version coverage (3.8 + 4.0) so a future pre-commit format
//!    change is caught here, not by the user.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use klasp_core::GATE_SCHEMA_VERSION;
use tempfile::TempDir;

const FIXTURE_GIT_COMMIT: &str = include_str!("fixtures/claude_commit_hook.json");

const FIXTURE_3X_PASS: &str = include_str!("fixtures/pre_commit/3x-pass.stdout");
const FIXTURE_3X_FAIL: &str = include_str!("fixtures/pre_commit/3x-fail.stdout");
const FIXTURE_4X_PASS: &str = include_str!("fixtures/pre_commit/4x-pass.stdout");
const FIXTURE_4X_FAIL: &str = include_str!("fixtures/pre_commit/4x-fail.stdout");
const FIXTURE_3X_VERSION: &str = include_str!("fixtures/pre_commit/3x-version.stdout");
const FIXTURE_4X_VERSION: &str = include_str!("fixtures/pre_commit/4x-version.stdout");

fn klasp_bin() -> &'static str {
    env!("CARGO_BIN_EXE_klasp")
}

/// Wrapper around the harness `pre-commit` shim. The shim:
///
/// - Reads `FAKE_PRE_COMMIT_STDOUT` (path) and `FAKE_PRE_COMMIT_EXIT`
///   (integer) at run time, so different tests can swap the captured
///   fixture without rewriting the shim.
/// - Special-cases `pre-commit --version` so the recipe's version sniff
///   finds the right answer for whichever fixture pair the test is
///   exercising.
///
/// Returns the absolute path to the shim's parent directory, ready to be
/// prepended to `PATH`.
fn install_fake_pre_commit(scratch: &TempDir, version_stdout: &str) -> std::path::PathBuf {
    let bin_dir = scratch.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create shim bin dir");
    let shim = bin_dir.join("pre-commit");

    // The shim is intentionally short — it has to dispatch on `--version`
    // and otherwise emit the captured stdout + the configured exit code.
    // Newlines in `version_stdout` survive the heredoc.
    let body = format!(
        r#"#!/usr/bin/env bash
set -u
case "${{1:-}}" in
  --version)
    cat <<'__VERSION_EOF__'
{version_stdout}__VERSION_EOF__
    exit 0
    ;;
esac
if [ -n "${{FAKE_PRE_COMMIT_STDOUT:-}}" ]; then
  cat "$FAKE_PRE_COMMIT_STDOUT"
fi
exit "${{FAKE_PRE_COMMIT_EXIT:-0}}"
"#,
    );
    std::fs::write(&shim, body).expect("write shim");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&shim).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&shim, perms).expect("chmod shim");
    }
    bin_dir
}

/// Spawn `klasp gate` with the configured fake pre-commit on PATH.
///
/// `extra_env` lets the caller wire the fixture stdout / exit code without
/// the harness having to know which fixture is in play.
fn spawn_gate(
    stdin_payload: &str,
    project_dir: &Path,
    fake_pre_commit_dir: &Path,
    extra_env: &[(&str, &str)],
) -> (Option<i32>, String) {
    let path_var = match std::env::var_os("PATH") {
        Some(existing) => {
            let mut prefix = std::ffi::OsString::from(fake_pre_commit_dir.as_os_str());
            prefix.push(":");
            prefix.push(existing);
            prefix
        }
        None => std::ffi::OsString::from(fake_pre_commit_dir.as_os_str()),
    };

    let mut cmd = Command::new(klasp_bin());
    cmd.arg("gate")
        .env("KLASP_GATE_SCHEMA", GATE_SCHEMA_VERSION.to_string())
        .env("CLAUDE_PROJECT_DIR", project_dir)
        .env("PATH", &path_var)
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
    if !stderr.is_empty() {
        eprintln!("klasp gate stderr:\n{stderr}");
    }
    (output.status.code(), stderr)
}

/// Write a fixture file and return its path. Pre-commit shim reads from
/// here at run time, so the path must outlive the gate child.
fn write_fixture(scratch: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
    let path = scratch.path().join(name);
    std::fs::write(&path, body).expect("write fixture");
    path
}

fn write_klasp_toml(project_dir: &Path, body: &str) {
    std::fs::write(project_dir.join("klasp.toml"), body).expect("write klasp.toml");
}

const PRE_COMMIT_KLASP_TOML: &str = r#"
    version = 1

    [gate]
    agents = ["claude_code"]
    policy = "any_fail"

    [[checks]]
    name = "lint"
    triggers = [{ on = ["commit"] }]
    timeout_secs = 30
    [checks.source]
    type = "pre_commit"
"#;

#[test]
fn pre_commit_3x_pass_fixture_yields_exit_0() {
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_pre_commit(&scratch, FIXTURE_3X_VERSION);
    let fixture_path = write_fixture(&scratch, "stdout.txt", FIXTURE_3X_PASS);

    write_klasp_toml(project.path(), PRE_COMMIT_KLASP_TOML);

    let (code, _stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_PRE_COMMIT_STDOUT", fixture_path.to_str().unwrap()),
            ("FAKE_PRE_COMMIT_EXIT", "0"),
        ],
    );
    assert_eq!(
        code,
        Some(0),
        "pre-commit 3.x passing fixture must produce Verdict::Pass → exit 0",
    );
}

#[test]
fn pre_commit_3x_fail_fixture_blocks_with_exit_2() {
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_pre_commit(&scratch, FIXTURE_3X_VERSION);
    let fixture_path = write_fixture(&scratch, "stdout.txt", FIXTURE_3X_FAIL);

    write_klasp_toml(project.path(), PRE_COMMIT_KLASP_TOML);

    let (code, stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_PRE_COMMIT_STDOUT", fixture_path.to_str().unwrap()),
            ("FAKE_PRE_COMMIT_EXIT", "1"),
        ],
    );
    assert_eq!(
        code,
        Some(2),
        "pre-commit 3.x failing fixture must produce Verdict::Fail → exit 2",
    );
    // The block message should name the failed hooks parsed from stdout —
    // the fixture has `ruff` and `ruff-format` failing.
    assert!(
        stderr.contains("ruff"),
        "expected `ruff` finding in block message, got: {stderr}",
    );
    assert!(
        stderr.contains("ruff-format"),
        "expected `ruff-format` finding in block message, got: {stderr}",
    );
}

#[test]
fn pre_commit_4x_pass_fixture_yields_exit_0() {
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_pre_commit(&scratch, FIXTURE_4X_VERSION);
    let fixture_path = write_fixture(&scratch, "stdout.txt", FIXTURE_4X_PASS);

    write_klasp_toml(project.path(), PRE_COMMIT_KLASP_TOML);

    let (code, _stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_PRE_COMMIT_STDOUT", fixture_path.to_str().unwrap()),
            ("FAKE_PRE_COMMIT_EXIT", "0"),
        ],
    );
    assert_eq!(
        code,
        Some(0),
        "pre-commit 4.x passing fixture must produce Verdict::Pass → exit 0",
    );
}

#[test]
fn pre_commit_4x_fail_fixture_blocks_with_exit_2() {
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_pre_commit(&scratch, FIXTURE_4X_VERSION);
    let fixture_path = write_fixture(&scratch, "stdout.txt", FIXTURE_4X_FAIL);

    write_klasp_toml(project.path(), PRE_COMMIT_KLASP_TOML);

    let (code, stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_PRE_COMMIT_STDOUT", fixture_path.to_str().unwrap()),
            ("FAKE_PRE_COMMIT_EXIT", "1"),
        ],
    );
    assert_eq!(
        code,
        Some(2),
        "pre-commit 4.x failing fixture must produce Verdict::Fail → exit 2",
    );
    // 4.x fixture fails `ruff (legacy alias)` and `prettier`; both should
    // appear in the rendered block message.
    assert!(
        stderr.contains("ruff"),
        "expected `ruff` finding in block message, got: {stderr}",
    );
    assert!(
        stderr.contains("prettier"),
        "expected `prettier` finding in block message, got: {stderr}",
    );
}

#[test]
fn pre_commit_recipe_with_custom_hook_stage_and_config_path() {
    // Round-trip the optional fields: `hook_stage` and `config_path` should
    // make it from TOML through the recipe to the rendered shell command.
    // The shim records its argv to a sentinel file so the test can assert
    // on the flags klasp passed.
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = scratch.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create shim bin");
    let shim = bin_dir.join("pre-commit");
    let argv_log = scratch.path().join("argv.log");
    let body = format!(
        r#"#!/usr/bin/env bash
case "${{1:-}}" in
  --version) echo "pre-commit 3.8.0"; exit 0 ;;
esac
printf '%s\n' "$@" > "{argv_log}"
exit 0
"#,
        argv_log = argv_log.display(),
    );
    std::fs::write(&shim, body).expect("write shim");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&shim).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&shim, perms).expect("chmod shim");
    }

    write_klasp_toml(
        project.path(),
        r#"
            version = 1

            [gate]
            agents = ["claude_code"]
            policy = "any_fail"

            [[checks]]
            name = "lint"
            triggers = [{ on = ["commit"] }]
            timeout_secs = 30
            [checks.source]
            type = "pre_commit"
            hook_stage = "pre-push"
            config_path = "tools/pre-commit.yaml"
        "#,
    );

    let (code, _stderr) = spawn_gate(FIXTURE_GIT_COMMIT, project.path(), &bin_dir, &[]);
    assert_eq!(code, Some(0), "shim returns 0 → gate must exit 0");

    let argv = std::fs::read_to_string(&argv_log).expect("read argv log");
    assert!(
        argv.contains("--hook-stage\npre-push"),
        "expected --hook-stage pre-push in argv, got:\n{argv}",
    );
    assert!(
        argv.contains("-c\ntools/pre-commit.yaml"),
        "expected -c tools/pre-commit.yaml in argv, got:\n{argv}",
    );
    // KLASP_BASE_REF is the documented contract; pre-commit must see it
    // expanded by `sh -c` before argv parsing. Without an upstream the
    // gate falls back to `HEAD~1`, which is what should appear here.
    assert!(
        argv.contains("--from-ref\nHEAD~1") || argv.contains("--from-ref"),
        "expected --from-ref in argv, got:\n{argv}",
    );
}
