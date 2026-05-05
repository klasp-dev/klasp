//! v0.1 success-criteria regression test suite.
//!
//! Asserts that the acceptance criteria from docs/roadmap.md §v0.1 success
//! criteria continue to hold, guarding against regressions as v0.2 work
//! progresses.
//!
//! Criteria covered here:
//!
//! 1. `klasp install` is idempotent — run twice, state is identical, second
//!    run does not error.
//! 2. `klasp uninstall` preserves sibling hooks — non-klasp PreToolUse hooks
//!    in `.claude/settings.json` survive install+uninstall.
//! 3. `klasp doctor` diagnoses three failure modes:
//!    - missing config (no `klasp.toml`)
//!    - missing hook (settings.json present but klasp's hook entry is absent)
//!    - schema mismatch (hook present but written with a stale schema version)
//!
//! Criterion 4 (five-platform CI matrix green) is a CI-level acceptance check
//! and is not unit-testable here. It is verified by the GitHub Actions release
//! workflow (.github/workflows/release.yml) and not represented in this file.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use klasp_agents_claude::{render_hook_script, ClaudeCodeSurface};
use klasp_core::{AgentSurface, InstallContext, GATE_SCHEMA_VERSION};
use serde_json::Value;

// ─── Shared helpers (copied from other test files — no shared harness exists) ─

const KLASP_CMD: &str = ClaudeCodeSurface::HOOK_COMMAND;

const VALID_TOML: &str = r#"version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
"#;

/// Minimal seeded repo: `.git/` + `.claude/` directories, no `klasp.toml`.
fn fresh_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::create_dir(dir.path().join(".claude")).unwrap();
    dir
}

/// Build an `InstallContext` for `root`, not a dry run.
fn ctx_for(root: &Path) -> InstallContext {
    InstallContext {
        repo_root: root.to_path_buf(),
        dry_run: false,
        force: false,
        schema_version: GATE_SCHEMA_VERSION,
    }
}

/// Run `klasp doctor` in `repo_root`, stripping harness env.
fn run_doctor(repo_root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_klasp"))
        .current_dir(repo_root)
        .arg("doctor")
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("spawn klasp doctor")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

// ─── Criterion 1: idempotent install ─────────────────────────────────────────

/// Running `klasp install` twice must produce byte-identical on-disk state and
/// must not error on the second invocation.
#[test]
fn install_is_idempotent() {
    let repo = fresh_repo();
    let surface = ClaudeCodeSurface;
    let ctx = ctx_for(repo.path());

    // First install.
    surface.install(&ctx).expect("first install must succeed");

    let hook_after_first =
        fs::read(repo.path().join(".claude/hooks/klasp-gate.sh")).expect("hook after first");
    let settings_after_first =
        fs::read(repo.path().join(".claude/settings.json")).expect("settings after first");

    // Second install — must not error.
    let second = surface
        .install(&ctx)
        .expect("second install must not error");
    assert!(
        second.already_installed,
        "second install must report already_installed=true: {second:?}"
    );
    assert!(
        second.paths_written.is_empty(),
        "second install must write no files: {second:?}"
    );

    // On-disk state must be byte-identical.
    let hook_after_second =
        fs::read(repo.path().join(".claude/hooks/klasp-gate.sh")).expect("hook after second");
    let settings_after_second =
        fs::read(repo.path().join(".claude/settings.json")).expect("settings after second");

    assert_eq!(
        hook_after_first, hook_after_second,
        "klasp-gate.sh must be byte-identical after two installs"
    );
    assert_eq!(
        settings_after_first, settings_after_second,
        "settings.json must be byte-identical after two installs"
    );
}

// ─── Criterion 2: uninstall preserves sibling hooks ──────────────────────────

/// If `.claude/settings.json` already contains a non-klasp PreToolUse hook
/// before install, that hook must survive `klasp uninstall`.
#[test]
fn uninstall_preserves_sibling_hooks() {
    let repo = fresh_repo();

    // Pre-seed a non-klasp PreToolUse hook entry.
    fs::write(
        repo.path().join(".claude/settings.json"),
        r#"{
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            { "type": "command", "command": "acme-ci gate" }
                        ]
                    }
                ]
            }
        }"#,
    )
    .unwrap();

    let surface = ClaudeCodeSurface;
    let ctx = ctx_for(repo.path());

    // Install klasp on top of the existing settings.
    surface.install(&ctx).expect("install must succeed");

    // Verify klasp's entry is present alongside the sibling.
    let after_install: Value = serde_json::from_str(
        &fs::read_to_string(repo.path().join(".claude/settings.json")).unwrap(),
    )
    .unwrap();
    let hooks_after_install = after_install["hooks"]["PreToolUse"][0]["hooks"]
        .as_array()
        .expect("PreToolUse[0].hooks must be an array after install");
    assert_eq!(
        hooks_after_install.len(),
        2,
        "both sibling and klasp hooks must be present after install"
    );

    // Uninstall klasp.
    surface
        .uninstall(repo.path(), false)
        .expect("uninstall must succeed");

    // The sibling hook must survive; klasp's entry must be gone.
    let after_uninstall: Value = serde_json::from_str(
        &fs::read_to_string(repo.path().join(".claude/settings.json")).unwrap(),
    )
    .unwrap();
    let hooks_after_uninstall = after_uninstall["hooks"]["PreToolUse"][0]["hooks"]
        .as_array()
        .expect("PreToolUse[0].hooks must be an array after uninstall");

    assert_eq!(
        hooks_after_uninstall.len(),
        1,
        "only sibling hook must remain after uninstall: {hooks_after_uninstall:?}"
    );
    assert_eq!(
        hooks_after_uninstall[0]["command"], "acme-ci gate",
        "sibling hook command must be preserved verbatim"
    );

    // klasp's command must not appear anywhere.
    let raw_settings = fs::read_to_string(repo.path().join(".claude/settings.json")).unwrap();
    assert!(
        !raw_settings.contains(KLASP_CMD),
        "klasp hook command must not appear in settings.json after uninstall"
    );
}

// ─── Criterion 3a: doctor diagnoses missing config ───────────────────────────

/// `klasp doctor` must report FAIL and diagnose "not found" when `klasp.toml`
/// is absent, even when the hook is correctly installed.
#[test]
fn doctor_diagnoses_missing_config() {
    let repo = fresh_repo();
    let surface = ClaudeCodeSurface;

    // Install the hook — no klasp.toml present.
    surface.install(&ctx_for(repo.path())).unwrap();

    let out = run_doctor(repo.path());
    assert!(
        !out.status.success(),
        "doctor must exit non-zero when klasp.toml is missing\nstdout:\n{}",
        stdout(&out)
    );

    let so = stdout(&out);
    assert!(
        so.contains("FAIL  config:"),
        "doctor stdout must contain 'FAIL  config:'\nstdout:\n{so}"
    );
    assert!(
        so.contains("not found"),
        "doctor stdout must mention 'not found'\nstdout:\n{so}"
    );
}

// ─── Criterion 3b: doctor diagnoses missing hook entry ───────────────────────

/// `klasp doctor` must report FAIL when `klasp.toml` exists and is valid but
/// `.claude/settings.json` does not contain klasp's hook entry.
#[test]
fn doctor_diagnoses_missing_hook_entry() {
    let repo = fresh_repo();
    let surface = ClaudeCodeSurface;

    // Write a valid config.
    fs::write(repo.path().join("klasp.toml"), VALID_TOML).unwrap();

    // Install so the hook file exists, then strip the settings.json entry.
    surface.install(&ctx_for(repo.path())).unwrap();
    fs::write(repo.path().join(".claude/settings.json"), "{}\n").unwrap();

    let out = run_doctor(repo.path());
    assert!(
        !out.status.success(),
        "doctor must exit non-zero when hook entry is missing from settings.json\nstdout:\n{}",
        stdout(&out)
    );

    let so = stdout(&out);
    assert!(
        so.contains("FAIL  settings[claude_code]:"),
        "doctor stdout must contain 'FAIL  settings[claude_code]:'\nstdout:\n{so}"
    );
}

// ─── Criterion 3c: doctor diagnoses schema mismatch ──────────────────────────

/// `klasp doctor` must report FAIL with "schema drift" when the installed
/// `klasp-gate.sh` was rendered with a different schema version than the
/// current binary expects.
#[test]
fn doctor_diagnoses_schema_mismatch() {
    let repo = fresh_repo();
    let surface = ClaudeCodeSurface;

    // Write a valid config and install.
    fs::write(repo.path().join("klasp.toml"), VALID_TOML).unwrap();
    surface.install(&ctx_for(repo.path())).unwrap();

    // Overwrite the hook file with a stale schema render (version 0).
    let stale_hook = render_hook_script(0);
    fs::write(repo.path().join(".claude/hooks/klasp-gate.sh"), stale_hook).unwrap();

    let out = run_doctor(repo.path());
    assert!(
        !out.status.success(),
        "doctor must exit non-zero on schema mismatch\nstdout:\n{}",
        stdout(&out)
    );

    let so = stdout(&out);
    assert!(
        so.contains("FAIL  hook[claude_code]:"),
        "doctor stdout must contain 'FAIL  hook[claude_code]:'\nstdout:\n{so}"
    );
    assert!(
        so.contains("schema drift"),
        "doctor stdout must mention 'schema drift'\nstdout:\n{so}"
    );
}
