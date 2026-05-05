//! Integration test: drive `klasp gate` against the `pytest` recipe
//! using captured pytest 7.x and 8.x stdout + JUnit XML fixtures.
//!
//! Acceptance for issue #32 calls for "captured outputs (pytest 7.x and
//! 8.x; cargo current stable)" — this file owns the pytest half.
//!
//! ## Strategy
//!
//! Real pytest may not be on the CI runner's `PATH`, and even if it is
//! we don't want the test depending on a specific installed version.
//! The harness writes a tiny shell shim called `pytest` to a tempdir,
//! prepends that tempdir to `PATH`, and parameterises the shim with
//! three env vars:
//!
//! - `FAKE_PYTEST_STDOUT` → path to a stdout fixture the shim `cat`s.
//! - `FAKE_PYTEST_JUNIT_SRC` → path to a JUnit XML fixture; the shim
//!   copies it to whatever `--junitxml=<path>` location the recipe
//!   requests so the parser can consume it.
//! - `FAKE_PYTEST_EXIT` → exit code the shim returns (default 0).
//!
//! The shim handles `pytest --version` (returns the captured banner)
//! and supports `--junitxml=<path>` for the JUnit-emission test path.
//!
//! ## Why a shim and not in-process unit tests
//!
//! The pytest recipe's exit-code → verdict mapping is already covered
//! in `klasp::sources::pytest::verdict`'s unit tests. What this file
//! adds is:
//!
//! 1. The full `klasp gate` flow over the new recipe (config parse →
//!    registry dispatch → recipe → exit code), proving the new variant
//!    is wired end-to-end.
//! 2. Confidence that real pytest JUnit XML (captured from documented
//!    7.x and 8.x output) parses as the recipe's per-failure findings.
//! 3. Cross-version coverage (7.4 + 8.3) so a future pytest format
//!    change is caught here, not by the user.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use klasp_core::GATE_SCHEMA_VERSION;
use tempfile::TempDir;

const FIXTURE_GIT_COMMIT: &str = include_str!("fixtures/claude_commit_hook.json");

const FIXTURE_7X_PASS: &str = include_str!("fixtures/pytest/7x-pass.stdout");
const FIXTURE_7X_FAIL: &str = include_str!("fixtures/pytest/7x-fail.stdout");
const FIXTURE_7X_JUNIT_FAIL: &str = include_str!("fixtures/pytest/7x-fail.junit.xml");
const FIXTURE_8X_PASS: &str = include_str!("fixtures/pytest/8x-pass.stdout");
const FIXTURE_8X_FAIL: &str = include_str!("fixtures/pytest/8x-fail.stdout");
const FIXTURE_8X_JUNIT_FAIL: &str = include_str!("fixtures/pytest/8x-fail.junit.xml");
const FIXTURE_7X_VERSION: &str = include_str!("fixtures/pytest/7x-version.stdout");
const FIXTURE_8X_VERSION: &str = include_str!("fixtures/pytest/8x-version.stdout");
const FIXTURE_6X_VERSION: &str = include_str!("fixtures/pytest/6x-version.stdout");

fn klasp_bin() -> &'static str {
    env!("CARGO_BIN_EXE_klasp")
}

/// Wrapper around the harness `pytest` shim. The shim:
///
/// - Reads `FAKE_PYTEST_STDOUT` (path) and `FAKE_PYTEST_EXIT` (integer)
///   at run time so different tests can swap fixtures without rewriting.
/// - When `FAKE_PYTEST_JUNIT_SRC` is set and `--junitxml=<path>` is on
///   the command line, copies that fixture to the requested path so
///   the recipe's JUnit parser has something to consume.
/// - Special-cases `pytest --version` so the recipe's lazy version
///   sniff finds the right answer for whichever fixture pair the
///   test is exercising.
///
/// Returns the absolute path to the shim's parent directory, ready to
/// be prepended to `PATH`.
fn install_fake_pytest(scratch: &TempDir, version_stdout: &str) -> std::path::PathBuf {
    let bin_dir = scratch.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create shim bin dir");
    let shim = bin_dir.join("pytest");
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
# If the recipe asked for JUnit XML, copy the fixture to the requested
# path so the parser can read it.
if [ -n "${{FAKE_PYTEST_JUNIT_SRC:-}}" ]; then
  for arg in "$@"; do
    case "$arg" in
      --junitxml=*)
        target="${{arg#--junitxml=}}"
        cp "$FAKE_PYTEST_JUNIT_SRC" "$target"
        ;;
    esac
  done
fi
if [ -n "${{FAKE_PYTEST_STDOUT:-}}" ]; then
  cat "$FAKE_PYTEST_STDOUT"
fi
exit "${{FAKE_PYTEST_EXIT:-0}}"
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

/// Spawn `klasp gate` with the configured fake pytest on PATH.
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

const PYTEST_KLASP_TOML: &str = r#"
    version = 1

    [gate]
    agents = ["claude_code"]
    policy = "any_fail"

    [[checks]]
    name = "tests"
    triggers = [{ on = ["commit"] }]
    timeout_secs = 30
    [checks.source]
    type = "pytest"
"#;

const PYTEST_KLASP_TOML_WITH_JUNIT: &str = r#"
    version = 1

    [gate]
    agents = ["claude_code"]
    policy = "any_fail"

    [[checks]]
    name = "tests"
    triggers = [{ on = ["commit"] }]
    timeout_secs = 30
    [checks.source]
    type = "pytest"
    junit_xml = true
"#;

#[test]
fn pytest_7x_pass_fixture_yields_exit_0() {
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_pytest(&scratch, FIXTURE_7X_VERSION);
    let fixture_path = write_fixture(&scratch, "stdout.txt", FIXTURE_7X_PASS);

    write_klasp_toml(project.path(), PYTEST_KLASP_TOML);

    let (code, _stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_PYTEST_STDOUT", fixture_path.to_str().unwrap()),
            ("FAKE_PYTEST_EXIT", "0"),
        ],
    );
    assert_eq!(
        code,
        Some(0),
        "pytest 7.x passing fixture must produce Verdict::Pass → exit 0",
    );
}

#[test]
fn pytest_8x_pass_fixture_yields_exit_0() {
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_pytest(&scratch, FIXTURE_8X_VERSION);
    let fixture_path = write_fixture(&scratch, "stdout.txt", FIXTURE_8X_PASS);

    write_klasp_toml(project.path(), PYTEST_KLASP_TOML);

    let (code, _stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_PYTEST_STDOUT", fixture_path.to_str().unwrap()),
            ("FAKE_PYTEST_EXIT", "0"),
        ],
    );
    assert_eq!(
        code,
        Some(0),
        "pytest 8.x passing fixture must produce Verdict::Pass → exit 0",
    );
}

#[test]
fn pytest_7x_fail_without_junit_blocks_with_generic_finding() {
    // No `junit_xml = true` → recipe falls back to the exit-code-only
    // path; the block message is generic but exit 2 still fires.
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_pytest(&scratch, FIXTURE_7X_VERSION);
    let fixture_path = write_fixture(&scratch, "stdout.txt", FIXTURE_7X_FAIL);

    write_klasp_toml(project.path(), PYTEST_KLASP_TOML);

    let (code, stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_PYTEST_STDOUT", fixture_path.to_str().unwrap()),
            ("FAKE_PYTEST_EXIT", "1"),
        ],
    );
    assert_eq!(
        code,
        Some(2),
        "pytest 7.x failing fixture must produce Verdict::Fail → exit 2",
    );
    assert!(
        stderr.contains("test failures"),
        "expected generic test-failures message, got: {stderr}",
    );
}

#[test]
fn pytest_8x_fail_with_junit_yields_per_failure_findings() {
    // `junit_xml = true` → recipe writes a JUnit XML report path and
    // parses it for per-failure findings (`test_add` and `test_sub`).
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_pytest(&scratch, FIXTURE_8X_VERSION);
    let stdout_path = write_fixture(&scratch, "stdout.txt", FIXTURE_8X_FAIL);
    let junit_path = write_fixture(&scratch, "src.xml", FIXTURE_8X_JUNIT_FAIL);

    write_klasp_toml(project.path(), PYTEST_KLASP_TOML_WITH_JUNIT);

    let (code, stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_PYTEST_STDOUT", stdout_path.to_str().unwrap()),
            ("FAKE_PYTEST_JUNIT_SRC", junit_path.to_str().unwrap()),
            ("FAKE_PYTEST_EXIT", "1"),
        ],
    );
    assert_eq!(code, Some(2));
    assert!(
        stderr.contains("test_add"),
        "expected `test_add` finding from JUnit XML, got: {stderr}",
    );
    assert!(
        stderr.contains("test_sub"),
        "expected `test_sub` finding from JUnit XML, got: {stderr}",
    );
    // File / line locations must propagate so the agent can navigate.
    assert!(
        stderr.contains("tests/test_math.py"),
        "expected file path in block message, got: {stderr}",
    );
}

#[test]
fn pytest_7x_fail_with_junit_yields_per_failure_findings() {
    // 7.x JUnit XML carries the same shape — make sure both versions
    // parse identically.
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_pytest(&scratch, FIXTURE_7X_VERSION);
    let stdout_path = write_fixture(&scratch, "stdout.txt", FIXTURE_7X_FAIL);
    let junit_path = write_fixture(&scratch, "src.xml", FIXTURE_7X_JUNIT_FAIL);

    write_klasp_toml(project.path(), PYTEST_KLASP_TOML_WITH_JUNIT);

    let (code, stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_PYTEST_STDOUT", stdout_path.to_str().unwrap()),
            ("FAKE_PYTEST_JUNIT_SRC", junit_path.to_str().unwrap()),
            ("FAKE_PYTEST_EXIT", "1"),
        ],
    );
    assert_eq!(code, Some(2));
    assert!(stderr.contains("test_add"));
    assert!(stderr.contains("test_sub"));
}

#[test]
fn pytest_unsupported_version_surfaces_warn_alongside_fail() {
    // 6.x is below MIN_SUPPORTED_VERSION (7.0); the recipe must keep
    // running but prepend a `Severity::Warn` finding so the operator
    // sees the version gap.
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_pytest(&scratch, FIXTURE_6X_VERSION);
    let fixture_path = write_fixture(&scratch, "stdout.txt", FIXTURE_7X_FAIL);

    write_klasp_toml(project.path(), PYTEST_KLASP_TOML);

    let (code, stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_PYTEST_STDOUT", fixture_path.to_str().unwrap()),
            ("FAKE_PYTEST_EXIT", "1"),
        ],
    );
    assert_eq!(
        code,
        Some(2),
        "fail still blocks even when version is unsupported",
    );
    assert!(
        stderr.contains("older than the minimum tested version"),
        "expected version warning in stderr, got: {stderr}",
    );
}

#[test]
fn pytest_collection_error_exit_5_blocks_with_descriptive_detail() {
    // pytest exit 5 = "no tests collected" — should map to a Fail with
    // the documented exit-code semantic ("no tests").
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_pytest(&scratch, FIXTURE_8X_VERSION);
    let fixture_path = write_fixture(&scratch, "stdout.txt", "");

    write_klasp_toml(project.path(), PYTEST_KLASP_TOML);

    let (code, stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_PYTEST_STDOUT", fixture_path.to_str().unwrap()),
            ("FAKE_PYTEST_EXIT", "5"),
        ],
    );
    assert_eq!(code, Some(2));
    assert!(
        stderr.contains("no tests"),
        "expected 'no tests' detail for exit 5, got: {stderr}",
    );
}

#[test]
fn pytest_recipe_with_explicit_extra_args_and_config_path() {
    // Round-trip the optional fields: `extra_args` and `config_path`
    // should make it from TOML through the recipe to the rendered
    // shell command. The shim records its argv to a sentinel file so
    // the test can assert on the flags klasp passed.
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = scratch.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create shim bin");
    let shim = bin_dir.join("pytest");
    let argv_log = scratch.path().join("argv.log");
    let body = format!(
        r#"#!/usr/bin/env bash
case "${{1:-}}" in
  --version) echo "pytest 8.3.2"; exit 0 ;;
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
            name = "tests"
            triggers = [{ on = ["commit"] }]
            timeout_secs = 30
            [checks.source]
            type = "pytest"
            extra_args = "-x -q"
            config_path = "pytest.ini"
        "#,
    );

    let (code, _stderr) = spawn_gate(FIXTURE_GIT_COMMIT, project.path(), &bin_dir, &[]);
    assert_eq!(code, Some(0), "shim returns exit 0 → gate must pass");

    let argv = std::fs::read_to_string(&argv_log).expect("read argv log");
    assert!(
        argv.contains("-c\npytest.ini"),
        "expected -c pytest.ini in argv, got:\n{argv}",
    );
    // Extra args come through as a single shell-quoted segment which sh
    // re-tokenises; the shim's argv log captures the post-shell tokens.
    assert!(argv.contains("-x"), "expected -x in argv, got:\n{argv}",);
    assert!(argv.contains("-q"), "expected -q in argv, got:\n{argv}",);
}
