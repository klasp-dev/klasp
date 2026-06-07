//! Integration test: drive `ClaudeCodeSurface::install_with_warnings` over
//! a tempdir and prove the foreign-hook-manager advisory contract holds
//! end-to-end (issue #92).
//!
//! Mirrors the Codex conflict suite (`klasp-agents-codex/tests/
//! git_hooks_install.rs`), adapted to Claude's install shape: Claude writes
//! `.claude/settings.json` + `.claude/hooks/klasp-gate.sh` and never touches
//! `.git/hooks/`, so the "conflict" is the presence of a co-resident hook
//! manager (`.husky/`, `lefthook.yml`/`lefthook.yaml`, `.pre-commit-config.yaml`)
//! at the repo root. Detection emits a non-fatal `SurfaceWarning`; the
//! install still completes and writes klasp's gate.

use std::fs;
use std::path::Path;

use klasp_agents_claude::{ClaudeCodeSurface, HookConflict};
use klasp_core::{AgentSurface, InstallContext, SurfaceWarning, GATE_SCHEMA_VERSION};

fn fresh_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::create_dir(dir.path().join(".claude")).unwrap();
    dir
}

fn ctx_for(root: &Path) -> InstallContext {
    InstallContext {
        repo_root: root.to_path_buf(),
        dry_run: false,
        force: false,
        schema_version: GATE_SCHEMA_VERSION,
    }
}

/// Did any warning mention the given tool's canonical name?
fn warns_about(warnings: &[SurfaceWarning], tool: &str) -> bool {
    warnings.iter().any(|w| w.message.contains(tool))
}

// ────────────────────────────────────────────────────────────────────
// Clean repo: no foreign manager → no warning, install succeeds.
// ────────────────────────────────────────────────────────────────────

#[test]
fn clean_repo_install_emits_no_conflict_warning() {
    let repo = fresh_repo();
    let surface = ClaudeCodeSurface;

    let (report, warnings) = surface.install_with_warnings(&ctx_for(repo.path())).unwrap();

    assert!(warnings.is_empty(), "clean repo must not warn: {warnings:?}");
    // Install still does its job.
    assert!(!report.already_installed);
    assert!(repo.path().join(".claude/settings.json").exists());
    assert!(repo.path().join(".claude/hooks/klasp-gate.sh").exists());
}

// ────────────────────────────────────────────────────────────────────
// Conflict detection — husky / lefthook / pre-commit framework.
// Each asserts: warning surfaced for that tool AND install still completes
// (the gate is written; the foreign manager's config is untouched).
// ────────────────────────────────────────────────────────────────────

#[test]
fn install_detects_husky_and_warns() {
    let repo = fresh_repo();
    fs::create_dir(repo.path().join(".husky")).unwrap();
    fs::write(repo.path().join(".husky").join("pre-commit"), "npm test\n").unwrap();

    let surface = ClaudeCodeSurface;
    let (report, warnings) = surface.install_with_warnings(&ctx_for(repo.path())).unwrap();

    assert!(
        warns_about(&warnings, "husky"),
        "expected a husky advisory, got {warnings:?}"
    );
    // Non-fatal: klasp's Claude gate is still installed.
    assert!(!report.already_installed);
    assert!(repo.path().join(".claude/settings.json").exists());
    assert!(repo.path().join(".claude/hooks/klasp-gate.sh").exists());
    // Foreign manager's config is left byte-for-byte alone.
    assert_eq!(
        fs::read_to_string(repo.path().join(".husky").join("pre-commit")).unwrap(),
        "npm test\n"
    );
}

#[test]
fn install_detects_lefthook_and_warns() {
    for name in ["lefthook.yml", "lefthook.yaml"] {
        let repo = fresh_repo();
        fs::write(repo.path().join(name), "pre-commit:\n  commands:\n").unwrap();

        let surface = ClaudeCodeSurface;
        let (_report, warnings) = surface.install_with_warnings(&ctx_for(repo.path())).unwrap();

        assert!(
            warns_about(&warnings, "lefthook"),
            "expected a lefthook advisory for {name}, got {warnings:?}"
        );
        assert!(repo.path().join(".claude/settings.json").exists());
    }
}

#[test]
fn install_detects_pre_commit_framework_and_warns() {
    let repo = fresh_repo();
    fs::write(
        repo.path().join(".pre-commit-config.yaml"),
        "repos:\n  - repo: local\n",
    )
    .unwrap();

    let surface = ClaudeCodeSurface;
    let (_report, warnings) = surface.install_with_warnings(&ctx_for(repo.path())).unwrap();

    assert!(
        warns_about(&warnings, "pre-commit"),
        "expected a pre-commit advisory, got {warnings:?}"
    );
    assert!(repo.path().join(".claude/settings.json").exists());
}

// ────────────────────────────────────────────────────────────────────
// Install must not fail when conflicts are present (parity with the
// Codex acceptance criterion: structured warning, never a hard error).
// ────────────────────────────────────────────────────────────────────

#[test]
fn install_does_not_fail_when_all_three_managers_present() {
    let repo = fresh_repo();
    fs::create_dir(repo.path().join(".husky")).unwrap();
    fs::write(repo.path().join("lefthook.yml"), "pre-commit:\n").unwrap();
    fs::write(repo.path().join(".pre-commit-config.yaml"), "repos: []\n").unwrap();

    let surface = ClaudeCodeSurface;
    let result = surface.install_with_warnings(&ctx_for(repo.path()));
    assert!(
        result.is_ok(),
        "install must not fail on conflict; got {result:?}"
    );

    let (_report, warnings) = result.unwrap();
    // One advisory per detected manager, in stable order.
    assert_eq!(warnings.len(), 3, "one warning per manager: {warnings:?}");
    assert!(warns_about(&warnings, "husky"));
    assert!(warns_about(&warnings, "lefthook"));
    assert!(warns_about(&warnings, "pre-commit"));

    // The gate is installed regardless.
    assert!(repo.path().join(".claude/hooks/klasp-gate.sh").exists());
}

// ────────────────────────────────────────────────────────────────────
// Warning payload sanity: each warning points at the foreign marker and
// carries the canonical tool name (the value docs/conformance-matrix.md
// claims).
// ────────────────────────────────────────────────────────────────────

#[test]
fn husky_warning_points_at_marker_path() {
    let repo = fresh_repo();
    fs::create_dir(repo.path().join(".husky")).unwrap();

    let surface = ClaudeCodeSurface;
    let (_report, warnings) = surface.install_with_warnings(&ctx_for(repo.path())).unwrap();

    let husky = warnings
        .iter()
        .find(|w| w.message.contains("husky"))
        .expect("husky warning present");
    assert!(
        husky.path.starts_with(repo.path()),
        "warning path must be rooted at the repo: {:?}",
        husky.path
    );
    assert!(husky.path.ends_with(".husky"));
}

#[test]
fn tool_name_strings_match_codex_canonical_vocabulary() {
    // These are the strings docs/conformance-matrix.md lists in the Claude
    // row's Conflict-handling cell; keep them load-bearing.
    assert_eq!(HookConflict::Husky.tool(), "husky");
    assert_eq!(HookConflict::Lefthook.tool(), "lefthook");
    assert_eq!(HookConflict::PreCommit.tool(), "pre-commit");
}
