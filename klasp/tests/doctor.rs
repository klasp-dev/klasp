//! Integration tests for `klasp doctor`.
//!
//! Tests run the compiled `klasp` binary via `env!("CARGO_BIN_EXE_klasp")`
//! so the full clap parse + cmd dispatch path is exercised end-to-end.
//! Test repos are constructed by calling `ClaudeCodeSurface::install`
//! directly (matches `tests/install_claude_code.rs`), then mutating the
//! resulting state to drive each FAIL/WARN path.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use klasp_agents_claude::ClaudeCodeSurface;
use klasp_core::{AgentSurface, InstallContext, GATE_SCHEMA_VERSION};

const VALID_TOML: &str = r#"version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
"#;

fn fresh_repo_with_claude() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::create_dir(dir.path().join(".claude")).unwrap();
    dir
}

fn fresh_repo_no_claude() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir(dir.path().join(".git")).unwrap();
    dir
}

fn install_seeded(repo_root: &Path) {
    let ctx = InstallContext {
        repo_root: repo_root.to_path_buf(),
        dry_run: false,
        force: false,
        schema_version: GATE_SCHEMA_VERSION,
    };
    ClaudeCodeSurface
        .install(&ctx)
        .expect("seed install must succeed");
}

fn write_toml(repo_root: &Path, body: &str) {
    fs::write(repo_root.join("klasp.toml"), body).unwrap();
}

fn run_doctor(repo_root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_klasp"))
        .current_dir(repo_root)
        .arg("doctor")
        // ConfigV1::load checks $CLAUDE_PROJECT_DIR before repo_root; strip
        // it so harness env doesn't leak in.
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("spawn klasp")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[test]
fn doctor_healthy_repo_exits_0() {
    let repo = fresh_repo_with_claude();
    write_toml(repo.path(), VALID_TOML);
    install_seeded(repo.path());

    let out = run_doctor(repo.path());
    assert!(
        out.status.success(),
        "expected exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout(&out),
        stderr(&out)
    );
    let so = stdout(&out);
    assert!(so.contains("OK    config:"), "stdout:\n{so}");
    assert!(so.contains("OK    hook[claude_code]:"), "stdout:\n{so}");
    assert!(so.contains("OK    settings[claude_code]:"), "stdout:\n{so}");
    assert!(!so.contains("FAIL"), "no FAIL lines expected:\n{so}");
}

#[test]
fn doctor_missing_config_exits_1() {
    let repo = fresh_repo_with_claude();
    install_seeded(repo.path());
    // No klasp.toml.

    let out = run_doctor(repo.path());
    assert!(!out.status.success(), "expected non-zero exit");
    let so = stdout(&out);
    assert!(so.contains("FAIL  config:"), "stdout:\n{so}");
    assert!(so.contains("not found"), "stdout:\n{so}");
}

#[test]
fn doctor_hook_missing_exits_1() {
    let repo = fresh_repo_with_claude();
    write_toml(repo.path(), VALID_TOML);
    install_seeded(repo.path());

    fs::remove_file(repo.path().join(".claude/hooks/klasp-gate.sh")).unwrap();

    let out = run_doctor(repo.path());
    assert!(!out.status.success());
    let so = stdout(&out);
    assert!(so.contains("FAIL  hook[claude_code]:"), "stdout:\n{so}");
    assert!(so.contains("not found"), "stdout:\n{so}");
}

#[test]
fn doctor_schema_drift_exits_1() {
    let repo = fresh_repo_with_claude();
    write_toml(repo.path(), VALID_TOML);
    install_seeded(repo.path());

    // Overwrite hook with an older-schema render so the byte-equality
    // check trips even though the managed marker is still present.
    let stale = klasp_agents_claude::render_hook_script(0);
    fs::write(repo.path().join(".claude/hooks/klasp-gate.sh"), stale).unwrap();

    let out = run_doctor(repo.path());
    assert!(!out.status.success());
    let so = stdout(&out);
    assert!(so.contains("FAIL  hook[claude_code]:"), "stdout:\n{so}");
    assert!(so.contains("schema drift"), "stdout:\n{so}");
}

#[test]
fn doctor_settings_entry_missing_exits_1() {
    let repo = fresh_repo_with_claude();
    write_toml(repo.path(), VALID_TOML);
    install_seeded(repo.path());

    // Replace settings.json with an empty object so the JSON walk fails.
    fs::write(repo.path().join(".claude/settings.json"), "{}\n").unwrap();

    let out = run_doctor(repo.path());
    assert!(!out.status.success());
    let so = stdout(&out);
    assert!(so.contains("FAIL  settings[claude_code]:"), "stdout:\n{so}");
}

#[test]
fn doctor_no_surfaces_detected_exits_0_with_warn() {
    let repo = fresh_repo_no_claude();
    write_toml(repo.path(), VALID_TOML);

    let out = run_doctor(repo.path());
    assert!(
        out.status.success(),
        "expected exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout(&out),
        stderr(&out)
    );
    let so = stdout(&out);
    assert!(
        so.contains("INFO  claude_code: surface not detected"),
        "stdout:\n{so}"
    );
    assert!(
        so.contains("WARN  no agent surfaces detected"),
        "stdout:\n{so}"
    );
}

#[test]
fn doctor_not_a_git_repo_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_doctor(dir.path());
    assert!(!out.status.success());
    let se = stderr(&out);
    assert!(
        se.contains("not a git repository"),
        "stderr should mention git repo: {se}"
    );
}

#[test]
fn doctor_check_command_warn_for_missing() {
    let repo = fresh_repo_with_claude();
    let toml = r#"version = 1

[gate]
agents = ["claude_code"]

[[checks]]
name = "missing-tool"
[checks.source]
type = "shell"
command = "xklasp-nonexistent-tool-abc123 ."
"#;
    write_toml(repo.path(), toml);
    install_seeded(repo.path());

    let out = run_doctor(repo.path());
    assert!(out.status.success(), "WARN should not fail doctor");
    let so = stdout(&out);
    assert!(so.contains("WARN  path[missing-tool]:"), "stdout:\n{so}");
    assert!(
        so.contains("xklasp-nonexistent-tool-abc123"),
        "stdout:\n{so}"
    );
}

#[test]
fn doctor_check_command_env_prefix_skipped() {
    let repo = fresh_repo_with_claude();
    let toml = r#"version = 1

[gate]
agents = ["claude_code"]

[[checks]]
name = "env-prefixed"
[checks.source]
type = "shell"
command = "PYTHONPATH=. xklasp-nonexistent-tool-abc123 -q"
"#;
    write_toml(repo.path(), toml);
    install_seeded(repo.path());

    let out = run_doctor(repo.path());
    assert!(out.status.success(), "WARN should not fail doctor");
    let so = stdout(&out);
    let warn_line = so
        .lines()
        .find(|l| l.starts_with("WARN  path[env-prefixed]:"))
        .unwrap_or_else(|| panic!("no WARN line for env-prefixed:\n{so}"));
    assert!(
        warn_line.contains("xklasp-nonexistent-tool-abc123"),
        "WARN line should mention the executable name: {warn_line}"
    );
    // The diagnostic mentions argv0 specifically; the original command
    // appears too (as context) but argv0 must be the one looked up.
    assert!(
        warn_line.contains("`xklasp-nonexistent-tool-abc123`"),
        "argv0 should be quoted in the WARN line: {warn_line}"
    );
}

#[test]
fn doctor_output_prefix_invariant() {
    // Across the healthy + missing-config cases, every non-empty stdout
    // line must start with one of the four canonical prefixes.
    let repo = fresh_repo_with_claude();
    write_toml(repo.path(), VALID_TOML);
    install_seeded(repo.path());

    let out_healthy = run_doctor(repo.path());

    // Missing-config case
    let repo2 = fresh_repo_with_claude();
    install_seeded(repo2.path());
    let out_missing = run_doctor(repo2.path());

    for (label, out) in [("healthy", &out_healthy), ("missing-config", &out_missing)] {
        for line in stdout(out).lines() {
            if line.is_empty() {
                continue;
            }
            assert!(
                line.starts_with("OK  ")
                    || line.starts_with("WARN")
                    || line.starts_with("FAIL")
                    || line.starts_with("INFO"),
                "{label}: stdout line lacks canonical prefix: {line:?}"
            );
        }
    }
}
