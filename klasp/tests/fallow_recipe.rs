//! Integration test: drive `klasp gate` against the `fallow` recipe using
//! captured `fallow audit --format json` fixtures from fallow 2.x.
//!
//! Acceptance for issue #31 calls for "captured fallow audit JSON
//! (fallow 2.x)" — this file owns that coverage.
//!
//! ## Strategy
//!
//! Real fallow may not be on the CI runner's `PATH`, and even if it is we
//! don't want the test depending on a specific installed version. The
//! harness writes a tiny shell shim called `fallow` to a tempdir, prepends
//! that tempdir to `PATH`, and parameterises the shim with two env vars:
//!
//! - `FAKE_FALLOW_STDOUT` → path to a JSON fixture the shim `cat`s.
//! - `FAKE_FALLOW_EXIT` → exit code the shim returns (default 0).
//!
//! The shim also handles `fallow --version` so the recipe's lazy version
//! sniff can read a version pinned to whichever fixture pair the test is
//! exercising.
//!
//! ## Why a shim and not in-process unit tests
//!
//! The fallow recipe's JSON → verdict mapping is already exercised in
//! `klasp::sources::fallow`'s unit tests with synthesised JSON. What this
//! file adds:
//!
//! 1. The full `klasp gate` flow over the new recipe (config parse →
//!    registry dispatch → recipe → exit code), proving the new variant is
//!    wired end-to-end.
//! 2. Confidence that real fallow audit JSON (captured from a fallow 2.62.0
//!    run) parses as the recipe's structured findings.
//! 3. Version-sniff coverage: the unsupported-version branch surfaces as a
//!    non-blocking `Severity::Warn` finding alongside the verdict.

mod common;

use tempfile::TempDir;

use common::{spawn_gate, write_fixture, write_klasp_toml};

const FIXTURE_GIT_COMMIT: &str = include_str!("fixtures/claude_commit_hook.json");

const FIXTURE_2X_PASS: &str = include_str!("fixtures/fallow/2x-pass.json");
const FIXTURE_2X_FAIL: &str = include_str!("fixtures/fallow/2x-fail.json");
const FIXTURE_2X_WARN: &str = include_str!("fixtures/fallow/2x-warn.json");
const FIXTURE_2X_VERSION: &str = include_str!("fixtures/fallow/2x-version.stdout");
const FIXTURE_1X_VERSION: &str = include_str!("fixtures/fallow/1x-version.stdout");

/// Wrapper around the harness `fallow` shim. The shim:
///
/// - Reads `FAKE_FALLOW_STDOUT` (path) and `FAKE_FALLOW_EXIT` (integer) at
///   run time so different tests can swap fixtures without rewriting.
/// - Special-cases `fallow --version` so the recipe's version sniff finds
///   the right answer for whichever fixture pair the test is exercising.
///
/// Returns the absolute path to the shim's parent directory, ready to be
/// prepended to `PATH`.
fn install_fake_fallow(scratch: &TempDir, version_stdout: &str) -> std::path::PathBuf {
    let bin_dir = scratch.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create shim bin dir");
    let shim = bin_dir.join("fallow");
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
if [ -n "${{FAKE_FALLOW_STDOUT:-}}" ]; then
  cat "$FAKE_FALLOW_STDOUT"
fi
exit "${{FAKE_FALLOW_EXIT:-0}}"
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

const FALLOW_KLASP_TOML: &str = r#"
    version = 1

    [gate]
    agents = ["claude_code"]
    policy = "any_fail"

    [[checks]]
    name = "audit"
    triggers = [{ on = ["commit"] }]
    timeout_secs = 30
    [checks.source]
    type = "fallow"
"#;

#[test]
fn fallow_2x_pass_fixture_yields_exit_0() {
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_fallow(&scratch, FIXTURE_2X_VERSION);
    let fixture_path = write_fixture(&scratch, "audit.json", FIXTURE_2X_PASS);

    write_klasp_toml(project.path(), FALLOW_KLASP_TOML);

    let (code, _stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_FALLOW_STDOUT", fixture_path.to_str().unwrap()),
            ("FAKE_FALLOW_EXIT", "0"),
        ],
    );
    assert_eq!(
        code,
        Some(0),
        "fallow 2.x passing fixture must produce Verdict::Pass → exit 0",
    );
}

#[test]
fn fallow_2x_fail_fixture_blocks_with_exit_2() {
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_fallow(&scratch, FIXTURE_2X_VERSION);
    let fixture_path = write_fixture(&scratch, "audit.json", FIXTURE_2X_FAIL);

    write_klasp_toml(project.path(), FALLOW_KLASP_TOML);

    let (code, stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_FALLOW_STDOUT", fixture_path.to_str().unwrap()),
            ("FAKE_FALLOW_EXIT", "1"),
        ],
    );
    assert_eq!(
        code,
        Some(2),
        "fallow 2.x failing fixture must produce Verdict::Fail → exit 2",
    );
    // The block message should name the failed function, the dead export,
    // and the duplication clone group — all three categories should ride
    // through the recipe → block message.
    assert!(
        stderr.contains("tooComplex"),
        "expected complexity finding in block message, got: {stderr}",
    );
    assert!(
        stderr.contains("unused"),
        "expected dead-code finding in block message, got: {stderr}",
    );
    assert!(
        stderr.contains("duplication"),
        "expected duplication finding in block message, got: {stderr}",
    );
    // File locations must propagate so the agent can navigate to them.
    assert!(
        stderr.contains("src/index.ts:7"),
        "expected `src/index.ts:7` location in block message, got: {stderr}",
    );
}

#[test]
fn fallow_2x_warn_fixture_does_not_block() {
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_fallow(&scratch, FIXTURE_2X_VERSION);
    let fixture_path = write_fixture(&scratch, "audit.json", FIXTURE_2X_WARN);

    write_klasp_toml(project.path(), FALLOW_KLASP_TOML);

    let (code, stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_FALLOW_STDOUT", fixture_path.to_str().unwrap()),
            ("FAKE_FALLOW_EXIT", "0"),
        ],
    );
    assert_eq!(
        code,
        Some(0),
        "fallow 2.x warn fixture is non-blocking → exit 0 (warn renders, doesn't block)",
    );
    assert!(
        stderr.contains("warnings"),
        "expected `warnings` summary line, got: {stderr}",
    );
    assert!(
        stderr.contains("legacyHelper"),
        "expected legacy complexity finding in warn block, got: {stderr}",
    );
}

#[test]
fn fallow_unsupported_version_surfaces_warn_alongside_fail() {
    // 1.x is below MIN_SUPPORTED_VERSION (2.0); the recipe must keep
    // running but prepend a `Severity::Warn` finding so the operator sees
    // the version gap. Acceptance bullet 4 from issue #31.
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = install_fake_fallow(&scratch, FIXTURE_1X_VERSION);
    let fixture_path = write_fixture(&scratch, "audit.json", FIXTURE_2X_FAIL);

    write_klasp_toml(project.path(), FALLOW_KLASP_TOML);

    let (code, stderr) = spawn_gate(
        FIXTURE_GIT_COMMIT,
        project.path(),
        &bin_dir,
        &[
            ("FAKE_FALLOW_STDOUT", fixture_path.to_str().unwrap()),
            ("FAKE_FALLOW_EXIT", "1"),
        ],
    );
    assert_eq!(
        code,
        Some(2),
        "fail verdict still blocks even when version is unsupported",
    );
    assert!(
        stderr.contains("older than the minimum tested version"),
        "expected version warning in stderr, got: {stderr}",
    );
}

#[test]
fn fallow_recipe_commit_trigger_omits_base_in_argv() {
    // Commit trigger: --base must NOT be present. The staged working tree
    // (not committed history) is the correct scope at PreToolUse time. Issue #64.
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = scratch.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create shim bin");
    let shim = bin_dir.join("fallow");
    let argv_log = scratch.path().join("argv.log");
    let body = format!(
        r#"#!/usr/bin/env bash
case "${{1:-}}" in
  --version) echo "fallow 2.62.0"; exit 0 ;;
esac
printf '%s\n' "$@" > "{argv_log}"
echo '{{"verdict":"pass","summary":{{}}}}'
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

    write_klasp_toml(project.path(), FALLOW_KLASP_TOML);

    let (code, _stderr) = spawn_gate(FIXTURE_GIT_COMMIT, project.path(), &bin_dir, &[]);
    assert_eq!(code, Some(0));
    let argv = std::fs::read_to_string(&argv_log).expect("read argv log");
    assert!(
        !argv.contains("--base"),
        "commit trigger must not pass --base to fallow, got:\n{argv}",
    );
    // The subcommand and format flags must still be present.
    assert!(
        argv.contains("audit"),
        "expected `audit` subcommand in argv, got:\n{argv}",
    );
    assert!(
        argv.contains("--format\njson"),
        "expected --format json in argv, got:\n{argv}",
    );
}

#[test]
fn fallow_recipe_commit_trigger_omits_base_even_when_explicit_base_configured() {
    // When `base = "origin/main"` is set but the trigger is a commit, --base
    // must still be omitted — only the push trigger uses --base. Issue #64.
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = scratch.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create shim bin");
    let shim = bin_dir.join("fallow");
    let argv_log = scratch.path().join("argv.log");
    let body = format!(
        r#"#!/usr/bin/env bash
case "${{1:-}}" in
  --version) echo "fallow 2.62.0"; exit 0 ;;
esac
printf '%s\n' "$@" > "{argv_log}"
echo '{{"verdict":"pass","summary":{{}}}}'
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
            name = "audit"
            triggers = [{ on = ["commit"] }]
            timeout_secs = 30
            [checks.source]
            type = "fallow"
            base = "origin/main"
            config_path = "tools/.fallowrc.json"
        "#,
    );

    let (code, _stderr) = spawn_gate(FIXTURE_GIT_COMMIT, project.path(), &bin_dir, &[]);
    assert_eq!(code, Some(0), "shim returns pass JSON → gate must exit 0");

    let argv = std::fs::read_to_string(&argv_log).expect("read argv log");
    assert!(
        argv.contains("audit"),
        "expected `audit` subcommand in argv, got:\n{argv}",
    );
    assert!(
        argv.contains("--format\njson"),
        "expected --format json in argv, got:\n{argv}",
    );
    // Commit trigger: no --base even when `base` is set in TOML.
    assert!(
        !argv.contains("--base"),
        "commit trigger must not pass --base even when base is configured, got:\n{argv}",
    );
    assert!(
        argv.contains("-c\ntools/.fallowrc.json"),
        "expected -c tools/.fallowrc.json in argv, got:\n{argv}",
    );
}

#[test]
fn fallow_recipe_push_trigger_includes_base_in_argv() {
    // Push trigger: --base must be present so fallow scopes to the ref-range.
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = scratch.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create shim bin");
    let shim = bin_dir.join("fallow");
    let argv_log = scratch.path().join("argv.log");
    let body = format!(
        r#"#!/usr/bin/env bash
case "${{1:-}}" in
  --version) echo "fallow 2.62.0"; exit 0 ;;
esac
printf '%s\n' "$@" > "{argv_log}"
echo '{{"verdict":"pass","summary":{{}}}}'
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
            name = "audit"
            triggers = [{ on = ["push"] }]
            timeout_secs = 30
            [checks.source]
            type = "fallow"
        "#,
    );

    let push_payload = r#"{
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "git push origin main",
        "description": "Push the branch."
      },
      "session_id": "klasp-fixture-push",
      "transcript_path": "/tmp/klasp-fixture/transcript.jsonl",
      "cwd": "/tmp/klasp-fixture/repo"
    }"#;

    let (code, _stderr) = spawn_gate(push_payload, project.path(), &bin_dir, &[]);
    assert_eq!(code, Some(0), "shim returns pass → gate must exit 0");

    let argv = std::fs::read_to_string(&argv_log).expect("read argv log");
    assert!(
        argv.contains("--base"),
        "push trigger must pass --base to fallow, got:\n{argv}",
    );
}
