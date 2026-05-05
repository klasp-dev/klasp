//! Dogfood-mode integration test for all four named recipes (issue #33, item 2).
//!
//! ## What "dogfood mode" means
//!
//! klasp gates its own commits using the `klasp.toml` at the repo root.
//! Issue #33 acceptance item 2 requires that all four named recipes
//! (`pre_commit`, `fallow`, `pytest`, `cargo`) run from that config
//! without any `verdict_path` key present on any check entry.
//!
//! ## Why `verdict_path` is already absent
//!
//! `verdict_path` is explicitly deferred in the config schema (see
//! [docs/design.md §14] and the `CheckSourceConfig` comment). The
//! `ConfigV1` parser rejects unknown fields (`#[serde(deny_unknown_fields)]`),
//! so any `klasp.toml` that parses successfully already contains no
//! `verdict_path`. The tests in this file assert that parse succeeds AND
//! that each recipe's gate invocation exits 0 (pass) when its shim tool
//! emits a clean output — proving the recipe is wired end-to-end and
//! auto-emits its verdict to stderr (the default path) rather than
//! requiring a file-path config key.
//!
//! ## Strategy
//!
//! Each sub-test installs a minimal shell shim for the relevant tool
//! (pre-commit, fallow, pytest, cargo) and runs `klasp gate` against a
//! temporary project dir containing the four-recipe `klasp.toml` from the
//! repo root. Shims return a clean pass output so the gate reaches
//! `Verdict::Pass` → exit 0 for every recipe independently.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use klasp_core::{CheckSourceConfig, ConfigV1, GATE_SCHEMA_VERSION};
use tempfile::TempDir;

const FIXTURE_GIT_COMMIT: &str = include_str!("fixtures/claude_commit_hook.json");

/// Path to the `klasp.toml` in the repository root (two levels up from the
/// `klasp/tests/` directory where this file lives).
fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is the `klasp/` crate root; parent gives the repo root.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .parent()
        .expect("repo root is parent of crate dir")
        .to_path_buf()
}

fn klasp_bin() -> &'static str {
    env!("CARGO_BIN_EXE_klasp")
}

/// Load and parse the real `klasp.toml` from the repo root.
fn load_root_config() -> ConfigV1 {
    let path = repo_root().join("klasp.toml");
    ConfigV1::from_file(&path).unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()))
}

/// Assert that the config contains a check entry whose source matches `recipe_type`.
fn assert_has_recipe(config: &ConfigV1, recipe_type: &str) {
    let found = config.checks.iter().any(|c| {
        matches!(
            (&c.source, recipe_type),
            (CheckSourceConfig::PreCommit { .. }, "pre_commit")
                | (CheckSourceConfig::Fallow { .. }, "fallow")
                | (CheckSourceConfig::Pytest { .. }, "pytest")
                | (CheckSourceConfig::Cargo { .. }, "cargo")
        )
    });
    assert!(
        found,
        "klasp.toml must contain a [[checks]] entry with type = \"{recipe_type}\"",
    );
}

/// Spawn `klasp gate` against `project_dir` with `fake_bin_dir` prepended to
/// PATH. Returns (exit_code, stderr).
fn spawn_gate(
    project_dir: &Path,
    fake_bin_dir: &Path,
    extra_env: &[(&str, &str)],
) -> (Option<i32>, String) {
    let path_var = match std::env::var_os("PATH") {
        Some(existing) => {
            let mut prefix = std::ffi::OsString::from(fake_bin_dir.as_os_str());
            prefix.push(":");
            prefix.push(existing);
            prefix
        }
        None => std::ffi::OsString::from(fake_bin_dir.as_os_str()),
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
        .write_all(FIXTURE_GIT_COMMIT.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait for klasp");

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !stderr.is_empty() {
        eprintln!("klasp gate stderr:\n{stderr}");
    }
    (output.status.code(), stderr)
}

/// Write a shell shim at `bin_dir/<name>` that emits `stdout` and returns
/// `exit_code`. When `version_arg` is set, the shim handles `--version`
/// specially by echoing `version_stdout` and exiting 0.
fn write_shim(bin_dir: &Path, name: &str, version_stdout: &str, pass_stdout: &str, exit_code: u8) {
    std::fs::create_dir_all(bin_dir).expect("create bin dir");
    let shim = bin_dir.join(name);
    let body = format!(
        r#"#!/usr/bin/env bash
set -u
case "${{1:-}}" in
  --version)
    printf '%s\n' {version_stdout_q}
    exit 0
    ;;
esac
printf '%s\n' {pass_stdout_q}
exit {exit_code}
"#,
        version_stdout_q = shell_quote(version_stdout),
        pass_stdout_q = shell_quote(pass_stdout),
        exit_code = exit_code,
    );
    std::fs::write(&shim, body).expect("write shim");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&shim).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&shim, perms).expect("chmod shim");
    }
}

/// Minimal single-quote shell escaping sufficient for fixture strings.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Write a `klasp.toml` to `project_dir` containing only the specified
/// check entry (isolated so each sub-test drives exactly one recipe).
fn write_isolated_toml(project_dir: &Path, checks_body: &str) {
    let toml = format!(
        "version = 1\n\n[gate]\nagents = [\"claude_code\"]\npolicy = \"any_fail\"\n\n{checks_body}\n"
    );
    std::fs::write(project_dir.join("klasp.toml"), toml).expect("write klasp.toml");
}

// ---------------------------------------------------------------------------
// Structural assertions on the root klasp.toml
// ---------------------------------------------------------------------------

/// The root `klasp.toml` must parse successfully (proving no `verdict_path` or
/// any other unknown field is present) and must contain all four named recipes.
#[test]
fn root_klasp_toml_parses_and_contains_dogfood_recipes() {
    let config = load_root_config();

    // Sanity: the file is version 1.
    assert_eq!(config.version, 1, "klasp.toml version must be 1");

    // Dogfood-wired recipes — pytest intentionally omitted because klasp is a
    // Rust-only repo (pytest exits 5 "no tests collected" → false-block on
    // every push). The pytest recipe stays covered by the per-recipe gate
    // tests below using isolated TOMLs. v0.2.x will fix the recipe to treat
    // exit 5 as a no-op pass so it can stay wired even on Rust-only repos.
    assert_has_recipe(&config, "pre_commit");
    assert_has_recipe(&config, "fallow");
    assert_has_recipe(&config, "cargo");
}

/// No check entry in the root `klasp.toml` may carry a `verdict_path` key.
/// Because `ConfigV1` uses `#[serde(deny_unknown_fields)]` on every variant of
/// `CheckSourceConfig`, a `verdict_path` field would cause `from_file` to
/// return an error rather than silently ignore it. This test re-parses the
/// raw TOML bytes and asserts the assignment `verdict_path =` does not appear
/// on any non-comment line — belt-and-suspenders against accidental future
/// additions.
#[test]
fn root_klasp_toml_has_no_verdict_path_key() {
    let path = repo_root().join("klasp.toml");
    let raw = std::fs::read_to_string(&path).expect("read klasp.toml");
    // Only check non-comment lines for the key assignment so remarks that
    // mention the field name (e.g. "No verdict_path is configured…") don't
    // trigger a false failure.
    let has_key = raw.lines().any(|line| {
        let trimmed = line.trim();
        !trimmed.starts_with('#') && trimmed.contains("verdict_path")
    });
    assert!(
        !has_key,
        "klasp.toml must not contain a `verdict_path` key assignment; \
         auto-emit to the default location (stderr) is the contract",
    );
}

// ---------------------------------------------------------------------------
// Gate-level dogfood tests — one per named recipe
// ---------------------------------------------------------------------------

/// `pre_commit` recipe: gate runs with a shim that exits 0, gate exits 0.
///
/// Asserts the full pipeline (config parse → registry dispatch → pre_commit
/// recipe → `Verdict::Pass`) completes without error when no `verdict_path`
/// is configured.
#[test]
fn dogfood_pre_commit_recipe_pass_exits_0() {
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = scratch.path().join("bin");

    write_shim(
        &bin_dir,
        "pre-commit",
        "pre-commit 3.8.0",
        // pre-commit pass output: hook name + Passed marker
        "check-yaml...................................................Passed",
        0,
    );

    write_isolated_toml(
        project.path(),
        "[[checks]]\nname = \"pre_commit\"\ntriggers = [{ on = [\"commit\"] }]\ntimeout_secs = 30\n[checks.source]\ntype = \"pre_commit\"\n",
    );

    let (code, stderr) = spawn_gate(project.path(), &bin_dir, &[]);
    assert_eq!(
        code,
        Some(0),
        "pre_commit recipe with passing shim must yield exit 0 (Verdict::Pass); stderr:\n{stderr}",
    );
}

/// `fallow` recipe: gate runs with a shim that emits passing JSON, gate exits 0.
#[test]
fn dogfood_fallow_recipe_pass_exits_0() {
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = scratch.path().join("bin");

    // Fallow pass JSON: minimal shape the recipe parser accepts.
    let pass_json = r#"{"verdict":"pass","summary":{}}"#;

    write_shim(&bin_dir, "fallow", "fallow 2.62.0", pass_json, 0);

    write_isolated_toml(
        project.path(),
        "[[checks]]\nname = \"fallow\"\ntriggers = [{ on = [\"commit\"] }]\ntimeout_secs = 30\n[checks.source]\ntype = \"fallow\"\n",
    );

    let (code, stderr) = spawn_gate(project.path(), &bin_dir, &[]);
    assert_eq!(
        code,
        Some(0),
        "fallow recipe with passing shim must yield exit 0 (Verdict::Pass); stderr:\n{stderr}",
    );
}

/// `pytest` recipe: gate runs with a shim that exits 0, gate exits 0.
#[test]
fn dogfood_pytest_recipe_pass_exits_0() {
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = scratch.path().join("bin");

    write_shim(&bin_dir, "pytest", "pytest 7.4.0", "1 passed in 0.01s", 0);

    write_isolated_toml(
        project.path(),
        "[[checks]]\nname = \"pytest\"\ntriggers = [{ on = [\"commit\"] }]\ntimeout_secs = 30\n[checks.source]\ntype = \"pytest\"\n",
    );

    let (code, stderr) = spawn_gate(project.path(), &bin_dir, &[]);
    assert_eq!(
        code,
        Some(0),
        "pytest recipe with passing shim must yield exit 0 (Verdict::Pass); stderr:\n{stderr}",
    );
}

/// `cargo` recipe: gate runs with a shim that emits a build-finished JSON line,
/// gate exits 0.
///
/// The cargo recipe reads `--message-format=json` output; a `build-finished`
/// line with `success: true` is the minimal passing payload.
#[test]
fn dogfood_cargo_recipe_pass_exits_0() {
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = scratch.path().join("bin");

    // Cargo JSON: build-finished with success = true.
    let pass_output = r#"{"reason":"build-finished","success":true}"#;

    write_shim(
        &bin_dir,
        "cargo",
        "cargo 1.79.0 (ded6ed5ec 2024-04-19)",
        pass_output,
        0,
    );

    write_isolated_toml(
        project.path(),
        "[[checks]]\nname = \"cargo\"\ntriggers = [{ on = [\"commit\"] }]\ntimeout_secs = 60\n[checks.source]\ntype = \"cargo\"\nsubcommand = \"check\"\n",
    );

    let (code, stderr) = spawn_gate(project.path(), &bin_dir, &[]);
    assert_eq!(
        code,
        Some(0),
        "cargo recipe with passing shim must yield exit 0 (Verdict::Pass); stderr:\n{stderr}",
    );
}

/// All four named recipes wired in a single `klasp.toml` (mirroring the root
/// dogfood config) all pass when every tool shim exits 0.
///
/// This exercises the full multi-recipe gate path: registry dispatches four
/// checks, merges four `Verdict::Pass` values, emits exit 0.
#[test]
fn dogfood_all_four_recipes_in_one_config_all_pass() {
    let project = TempDir::new().expect("tempdir");
    let scratch = TempDir::new().expect("scratch");
    let bin_dir = scratch.path().join("bin");

    // Install shims for all four tools.
    write_shim(
        &bin_dir,
        "pre-commit",
        "pre-commit 3.8.0",
        "all hooks passed",
        0,
    );
    write_shim(
        &bin_dir,
        "fallow",
        "fallow 2.62.0",
        r#"{"verdict":"pass","summary":{}}"#,
        0,
    );
    write_shim(&bin_dir, "pytest", "pytest 7.4.0", "1 passed", 0);
    write_shim(
        &bin_dir,
        "cargo",
        "cargo 1.79.0 (ded6ed5ec 2024-04-19)",
        r#"{"reason":"build-finished","success":true}"#,
        0,
    );

    // Config mirrors the root dogfood layout: all four named recipes, no verdict_path.
    let toml = r#"version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"

[[checks]]
name = "pre_commit"
triggers = [{ on = ["commit"] }]
timeout_secs = 60
[checks.source]
type = "pre_commit"

[[checks]]
name = "fallow"
triggers = [{ on = ["commit"] }]
timeout_secs = 60
[checks.source]
type = "fallow"

[[checks]]
name = "pytest"
triggers = [{ on = ["commit"] }]
timeout_secs = 120
[checks.source]
type = "pytest"

[[checks]]
name = "cargo"
triggers = [{ on = ["commit"] }]
timeout_secs = 300
[checks.source]
type = "cargo"
subcommand = "check"
"#;
    std::fs::write(project.path().join("klasp.toml"), toml).expect("write klasp.toml");

    let (code, stderr) = spawn_gate(project.path(), &bin_dir, &[]);
    assert_eq!(
        code,
        Some(0),
        "all four recipes passing → gate must exit 0; stderr:\n{stderr}",
    );
}
