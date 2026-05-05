//! Integration test: drive `klasp gate` against the `cargo` recipe
//! using captured `cargo --message-format=json` and `cargo test`
//! summary fixtures.
//!
//! Acceptance for issue #32 calls for "captured outputs (pytest 7.x
//! and 8.x; cargo current stable)" — this file owns the cargo half.
//!
//! ## Strategy
//!
//! The harness writes a tiny shell shim called `cargo` to a tempdir,
//! prepends that tempdir to `PATH`, and parameterises the shim with:
//!
//! - `FAKE_CARGO_STDOUT` → path to a stdout fixture the shim `cat`s.
//! - `FAKE_CARGO_EXIT` → exit code the shim returns (default 0).
//!
//! The shim handles `cargo --version` (returns the captured banner)
//! and dispatches every other invocation to the configured stdout +
//! exit code.
//!
//! ## Why a shim and not a real cargo run
//!
//! Real cargo is on every contributor's PATH but exec'ing the real
//! binary against a synthesised crate would couple the test to whichever
//! cargo version the CI runner happens to have installed. The recipe's
//! diagnostic-walking logic is what we need to validate — fixtures pin
//! the JSON shape and let us assert on the rendered findings without
//! racing the toolchain.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use klasp_core::GATE_SCHEMA_VERSION;
use tempfile::TempDir;

const FIXTURE_GIT_COMMIT: &str = include_str!("fixtures/claude_commit_hook.json");

const FIXTURE_VERSION: &str = include_str!("fixtures/cargo/version.stdout");
const FIXTURE_CHECK_PASS: &str = include_str!("fixtures/cargo/check-pass.stdout");
const FIXTURE_CHECK_FAIL: &str = include_str!("fixtures/cargo/check-fail.stdout");
const FIXTURE_CLIPPY_FAIL: &str = include_str!("fixtures/cargo/clippy-fail.stdout");
const FIXTURE_TEST_PASS: &str = include_str!("fixtures/cargo/test-pass.stdout");
const FIXTURE_TEST_FAIL: &str = include_str!("fixtures/cargo/test-fail.stdout");

fn klasp_bin() -> &'static str {
    env!("CARGO_BIN_EXE_klasp")
}

/// Wrapper around the harness `cargo` shim. The shim:
///
/// - Reads `FAKE_CARGO_STDOUT` (path) and `FAKE_CARGO_EXIT` (integer)
///   at run time so different tests can swap fixtures without rewriting.
/// - Special-cases `cargo --version` so the recipe's lazy version
///   sniff finds the right answer.
fn install_fake_cargo(scratch: &TempDir, version_stdout: &str) -> std::path::PathBuf {
    let bin_dir = scratch.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create shim bin dir");
    let shim = bin_dir.join("cargo");
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
if [ -n "${{FAKE_CARGO_STDOUT:-}}" ]; then
  cat "$FAKE_CARGO_STDOUT"
fi
exit "${{FAKE_CARGO_EXIT:-0}}"
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

/// Spawn `klasp gate` with the configured fake cargo on PATH.
fn spawn_gate(
    stdin_payload: &str,
    project_dir: &Path,
    fake_dir: &Path,
    extra_env: &[(&str, &str)],
) -> (Option<i32>, String) {
    let path_var = match std::env::var_os("PATH") {
        Some(existing) => {
            let mut prefix = std::ffi::OsString::from(fake_dir.as_os_str());
            prefix.push(":");
            prefix.push(existing);
            prefix
        }
        None => std::ffi::OsString::from(fake_dir.as_os_str()),
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

fn write_fixture(scratch: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
    let path = scratch.path().join(name);
    std::fs::write(&path, body).expect("write fixture");
    path
}

fn write_klasp_toml(project_dir: &Path, body: &str) {
    std::fs::write(project_dir.join("klasp.toml"), body).expect("write klasp.toml");
}

fn klasp_toml_for_subcommand(subcommand: &str) -> String {
    format!(
        r#"
            version = 1

            [gate]
            agents = ["claude_code"]
            policy = "any_fail"

            [[checks]]
            name = "build"
            triggers = [{{ on = ["commit"] }}]
            timeout_secs = 60
            [checks.source]
            type = "cargo"
            subcommand = "{subcommand}"
        "#
    )
}

#[test]
fn cargo_check_pass_yields_exit_0() {
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_cargo(&scratch, FIXTURE_VERSION);
    let fixture_path = write_fixture(&scratch, "stdout.txt", FIXTURE_CHECK_PASS);

    write_klasp_toml(project.path(), &klasp_toml_for_subcommand("check"));

    let (code, _stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_CARGO_STDOUT", fixture_path.to_str().unwrap()),
            ("FAKE_CARGO_EXIT", "0"),
        ],
    );
    assert_eq!(code, Some(0), "cargo check pass → exit 0");
}

#[test]
fn cargo_check_fail_blocks_with_diagnostic_findings() {
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_cargo(&scratch, FIXTURE_VERSION);
    let fixture_path = write_fixture(&scratch, "stdout.txt", FIXTURE_CHECK_FAIL);

    write_klasp_toml(project.path(), &klasp_toml_for_subcommand("check"));

    let (code, stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_CARGO_STDOUT", fixture_path.to_str().unwrap()),
            ("FAKE_CARGO_EXIT", "101"),
        ],
    );
    assert_eq!(code, Some(2), "cargo check fail → exit 2");
    // The compiler-message JSON pins `cannot find value` + E0425 + the
    // src/lib.rs:7 location; all three should ride through.
    assert!(
        stderr.contains("E0425"),
        "expected E0425 code in block, got: {stderr}",
    );
    assert!(
        stderr.contains("cannot find value"),
        "expected diagnostic message in block, got: {stderr}",
    );
    assert!(
        stderr.contains("src/lib.rs:7"),
        "expected src/lib.rs:7 location in block, got: {stderr}",
    );
}

#[test]
fn cargo_clippy_fail_renders_warnings_and_errors() {
    // Clippy's fixture has one warning + one error; the rendered block
    // should surface both classes so the agent can act on each.
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_cargo(&scratch, FIXTURE_VERSION);
    let fixture_path = write_fixture(&scratch, "stdout.txt", FIXTURE_CLIPPY_FAIL);

    write_klasp_toml(project.path(), &klasp_toml_for_subcommand("clippy"));

    let (code, stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_CARGO_STDOUT", fixture_path.to_str().unwrap()),
            ("FAKE_CARGO_EXIT", "101"),
        ],
    );
    assert_eq!(code, Some(2));
    assert!(
        stderr.contains("clippy::"),
        "expected clippy lint code in block, got: {stderr}",
    );
}

#[test]
fn cargo_test_pass_yields_exit_0() {
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_cargo(&scratch, FIXTURE_VERSION);
    let fixture_path = write_fixture(&scratch, "stdout.txt", FIXTURE_TEST_PASS);

    write_klasp_toml(project.path(), &klasp_toml_for_subcommand("test"));

    let (code, _stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_CARGO_STDOUT", fixture_path.to_str().unwrap()),
            ("FAKE_CARGO_EXIT", "0"),
        ],
    );
    assert_eq!(code, Some(0), "cargo test pass → exit 0");
}

#[test]
fn cargo_test_fail_blocks_with_summary_count() {
    // `cargo test`'s JSON test reporter is still nightly-only; the
    // recipe parses the trailing `test result: …` summary line
    // instead — `3 passed; 1 failed` should appear in the block.
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_cargo(&scratch, FIXTURE_VERSION);
    let fixture_path = write_fixture(&scratch, "stdout.txt", FIXTURE_TEST_FAIL);

    write_klasp_toml(project.path(), &klasp_toml_for_subcommand("test"));

    let (code, stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_CARGO_STDOUT", fixture_path.to_str().unwrap()),
            ("FAKE_CARGO_EXIT", "101"),
        ],
    );
    assert_eq!(code, Some(2));
    assert!(
        stderr.contains("1 failed"),
        "expected `1 failed` from summary line, got: {stderr}",
    );
}

#[test]
fn cargo_unknown_subcommand_blocks_without_running_cargo() {
    // The recipe rejects unknown subcommands at run time with a
    // descriptive Fail rather than letting cargo emit "no such
    // subcommand". The shim would fail on an unknown subcommand
    // anyway, but we want the recipe-level error path to fire first.
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_cargo(&scratch, FIXTURE_VERSION);

    write_klasp_toml(
        project.path(),
        r#"
            version = 1
            [gate]
            agents = ["claude_code"]
            policy = "any_fail"

            [[checks]]
            name = "build"
            triggers = [{ on = ["commit"] }]
            [checks.source]
            type = "cargo"
            subcommand = "uninstall"
        "#,
    );

    let (code, stderr) = spawn_gate(FIXTURE_GIT_COMMIT, project.path(), &bin_dir, &[]);
    assert_eq!(code, Some(2));
    assert!(
        stderr.contains("uninstall"),
        "expected unknown-subcommand detail in stderr, got: {stderr}",
    );
    assert!(
        stderr.contains("expected one of"),
        "expected list of allowed subcommands in stderr, got: {stderr}",
    );
}

#[test]
fn cargo_recipe_with_explicit_package_and_extra_args() {
    // Round-trip the optional fields: `package` and `extra_args` should
    // make it from TOML through the recipe to the rendered shell
    // command. The shim records its argv to a sentinel file.
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = scratch.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create shim bin");
    let shim = bin_dir.join("cargo");
    let argv_log = scratch.path().join("argv.log");
    let body = format!(
        r#"#!/usr/bin/env bash
case "${{1:-}}" in
  --version) echo "cargo 1.79.0"; exit 0 ;;
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
            timeout_secs = 60
            [checks.source]
            type = "cargo"
            subcommand = "clippy"
            package = "klasp-core"
            extra_args = "-- -D warnings"
        "#,
    );

    let (code, _stderr) = spawn_gate(FIXTURE_GIT_COMMIT, project.path(), &bin_dir, &[]);
    assert_eq!(code, Some(0), "shim exit 0 → gate pass");

    let argv = std::fs::read_to_string(&argv_log).expect("read argv log");
    // First arg is the subcommand; -p / package / message-format / extra
    // args should follow in order.
    assert!(
        argv.starts_with("clippy\n"),
        "expected clippy as first argv, got:\n{argv}",
    );
    assert!(
        argv.contains("-p\nklasp-core"),
        "expected `-p klasp-core` in argv, got:\n{argv}",
    );
    // Workspace flag is mutually exclusive with -p; should NOT appear.
    assert!(
        !argv.contains("--workspace"),
        "did not expect --workspace when -p is set, got:\n{argv}",
    );
    assert!(
        argv.contains("--message-format=json"),
        "expected --message-format=json in argv, got:\n{argv}",
    );
    assert!(
        argv.contains("-D\nwarnings"),
        "expected `-D warnings` in argv, got:\n{argv}",
    );
}

#[test]
fn cargo_recipe_default_uses_workspace_flag() {
    // When `package` is omitted the recipe runs across the workspace.
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = scratch.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create shim bin");
    let shim = bin_dir.join("cargo");
    let argv_log = scratch.path().join("argv.log");
    let body = format!(
        r#"#!/usr/bin/env bash
case "${{1:-}}" in
  --version) echo "cargo 1.79.0"; exit 0 ;;
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

    write_klasp_toml(project.path(), &klasp_toml_for_subcommand("check"));

    let (code, _stderr) = spawn_gate(FIXTURE_GIT_COMMIT, project.path(), &bin_dir, &[]);
    assert_eq!(code, Some(0));
    let argv = std::fs::read_to_string(&argv_log).expect("read argv log");
    assert!(
        argv.contains("--workspace"),
        "expected --workspace in default invocation, got:\n{argv}",
    );
}
