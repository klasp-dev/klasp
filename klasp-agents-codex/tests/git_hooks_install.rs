//! Integration test: drive `CodexSurface::install_detailed` over a
//! tempdir and prove the git-hook contract holds end-to-end.
//!
//! Covers the W2 (#28) acceptance items verbatim:
//!
//! 1. Generated hooks invoke `klasp gate --agent codex` with
//!    `KLASP_GATE_SCHEMA` exported on the same line.
//! 2. Pre-existing hook content is preserved by appending klasp's
//!    managed section.
//! 3. Conflict detection covers husky / lefthook / pre-commit framework
//!    (real-world fixtures under `tests/fixtures/githooks/`).
//! 4. Uninstall strips klasp's managed-section block from each hook
//!    without touching sibling content; collapses-to-shebang-only files
//!    are removed so the round-trip from missing-file install is clean.
//! 5. Fresh-create install + uninstall round-trips to a missing file.

use std::fs;
use std::path::{Path, PathBuf};

use klasp_agents_codex::{
    git_hooks::{self, HookConflict, HookKind, HookWarning, MANAGED_END, MANAGED_START},
    CodexSurface,
};
use klasp_core::{AgentSurface, InstallContext, GATE_SCHEMA_VERSION};

const FIXTURE_HUSKY: &str = include_str!("fixtures/githooks/pre-commit-husky.sh");
const FIXTURE_HUSKY_V9: &str = include_str!("fixtures/githooks/pre-commit-husky-v9.sh");
const FIXTURE_LEFTHOOK: &str = include_str!("fixtures/githooks/pre-commit-lefthook.sh");
const FIXTURE_PRECOMMIT_FRAMEWORK: &str =
    include_str!("fixtures/githooks/pre-commit-pre-commit-framework.sh");
const FIXTURE_USER_BASH: &str = include_str!("fixtures/githooks/pre-commit-user-bash.sh");

fn ctx(repo_root: PathBuf) -> InstallContext {
    InstallContext {
        repo_root,
        dry_run: false,
        force: false,
        schema_version: GATE_SCHEMA_VERSION,
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn pre_commit(repo_root: &Path) -> PathBuf {
    repo_root.join(".git").join("hooks").join("pre-commit")
}

fn pre_push(repo_root: &Path) -> PathBuf {
    repo_root.join(".git").join("hooks").join("pre-push")
}

fn ensure_git_hooks_dir(repo_root: &Path) {
    fs::create_dir_all(repo_root.join(".git").join("hooks"))
        .expect("create .git/hooks/ for fixtures");
}

// ────────────────────────────────────────────────────────────────────
// Fresh-create paths
// ────────────────────────────────────────────────────────────────────

#[test]
fn install_fresh_creates_pre_commit_with_shebang_block_and_executable_bit() {
    let dir = tempfile::tempdir().unwrap();
    let surface = CodexSurface;

    surface.install(&ctx(dir.path().to_path_buf())).unwrap();

    let body = read(&pre_commit(dir.path()));
    assert!(
        body.starts_with("#!/usr/bin/env sh"),
        "fresh hook must start with portable shebang, got: {:?}",
        &body[..body.len().min(40)]
    );
    assert!(body.contains(MANAGED_START));
    assert!(body.contains(MANAGED_END));
    // Schema env-var must be exported on the same line as the exec
    // so the schema-mismatch path in `klasp gate` can detect drift.
    let expected_schema_export = format!("KLASP_GATE_SCHEMA={GATE_SCHEMA_VERSION} exec klasp gate");
    assert!(
        body.contains(&expected_schema_export),
        "schema export must precede the exec on the same line; got:\n{body}",
    );
    assert!(body.contains("--agent codex"));
    assert!(body.contains("--trigger commit"));
    assert!(body.contains("\"$@\""), "must propagate hook args");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(pre_commit(dir.path()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o755, "fresh hook must be executable, got {mode:o}");
    }
}

#[test]
fn install_fresh_creates_pre_push_with_push_trigger() {
    let dir = tempfile::tempdir().unwrap();
    let surface = CodexSurface;
    surface.install(&ctx(dir.path().to_path_buf())).unwrap();

    let body = read(&pre_push(dir.path()));
    assert!(body.starts_with("#!/usr/bin/env sh"));
    assert!(body.contains("--trigger push"));
    assert!(!body.contains("--trigger commit"));
}

#[test]
fn install_fresh_writes_three_paths_in_install_report() {
    let dir = tempfile::tempdir().unwrap();
    let surface = CodexSurface;
    let report = surface
        .install_detailed(&ctx(dir.path().to_path_buf()))
        .unwrap();

    let written: Vec<_> = report
        .report
        .paths_written
        .iter()
        .map(|p| p.file_name().unwrap().to_owned())
        .collect();
    assert!(written.iter().any(|n| n == "AGENTS.md"));
    assert!(written.iter().any(|n| n == "pre-commit"));
    assert!(written.iter().any(|n| n == "pre-push"));
    assert!(report.warnings.is_empty(), "fresh-create hits no conflicts");
}

// ────────────────────────────────────────────────────────────────────
// Idempotency
// ────────────────────────────────────────────────────────────────────

#[test]
fn install_is_idempotent_on_klasp_owned_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let surface = CodexSurface;

    surface.install(&ctx(dir.path().to_path_buf())).unwrap();
    let after_first = read(&pre_commit(dir.path()));
    let after_first_push = read(&pre_push(dir.path()));

    let report2 = surface
        .install_detailed(&ctx(dir.path().to_path_buf()))
        .unwrap();
    let after_second = read(&pre_commit(dir.path()));
    let after_second_push = read(&pre_push(dir.path()));

    assert_eq!(after_first, after_second);
    assert_eq!(after_first_push, after_second_push);
    assert!(
        report2.report.already_installed,
        "second install should report already_installed=true"
    );
    assert!(
        report2.report.paths_written.is_empty(),
        "idempotent re-install must write nothing"
    );
}

// ────────────────────────────────────────────────────────────────────
// Existing user hook (no klasp marker, no recognised tool)
// ────────────────────────────────────────────────────────────────────

#[test]
fn install_appends_block_to_existing_user_hook() {
    let dir = tempfile::tempdir().unwrap();
    ensure_git_hooks_dir(dir.path());
    fs::write(pre_commit(dir.path()), FIXTURE_USER_BASH).unwrap();

    let surface = CodexSurface;
    let report = surface
        .install_detailed(&ctx(dir.path().to_path_buf()))
        .unwrap();

    let body = read(&pre_commit(dir.path()));
    // User content survives byte-for-byte at the head.
    assert!(body.starts_with(FIXTURE_USER_BASH.trim_end_matches('\n')));
    // Original markers / commands still present.
    assert!(body.contains("WIP-do-not-merge"));
    assert!(body.contains("set -euo pipefail"));
    // klasp block tacked on at the end.
    assert!(body.contains(MANAGED_START));
    assert!(body.contains(MANAGED_END));
    assert!(body.contains(&format!("KLASP_GATE_SCHEMA={GATE_SCHEMA_VERSION}")));
    // No warnings — this is a plain user hook, not a foreign tool's.
    assert!(report.warnings.is_empty());
}

// ────────────────────────────────────────────────────────────────────
// Conflict detection — husky / lefthook / pre-commit framework
// ────────────────────────────────────────────────────────────────────

#[test]
fn install_detects_husky_and_skips_with_warning() {
    let dir = tempfile::tempdir().unwrap();
    ensure_git_hooks_dir(dir.path());
    fs::write(pre_commit(dir.path()), FIXTURE_HUSKY).unwrap();

    let surface = CodexSurface;
    let report = surface
        .install_detailed(&ctx(dir.path().to_path_buf()))
        .unwrap();

    // Hook file is byte-for-byte unchanged.
    assert_eq!(read(&pre_commit(dir.path())), FIXTURE_HUSKY);
    // Warning surfaced for the pre-commit hook only — pre-push is still
    // missing in this fixture, so klasp creates it normally.
    let husky_warnings: Vec<_> = report
        .warnings
        .iter()
        .filter(|w| {
            matches!(
                w,
                HookWarning::Skipped {
                    conflict: HookConflict::Husky,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(husky_warnings.len(), 1);
    // `Skipped` is currently the only `HookWarning` variant, so this is
    // an irrefutable destructure rather than an `if let`. Future
    // variants may turn this back into an `if let`/`match` — leaving
    // the field bindings explicit keeps the call-site stable across
    // either evolution.
    let HookWarning::Skipped { kind, path, .. } = husky_warnings[0];
    assert_eq!(*kind, HookKind::Commit);
    assert_eq!(*path, pre_commit(dir.path()));
    // pre-push, having no fixture content, is created normally.
    assert!(read(&pre_push(dir.path())).contains(MANAGED_START));
}

#[test]
fn install_detects_lefthook_and_skips_with_warning() {
    let dir = tempfile::tempdir().unwrap();
    ensure_git_hooks_dir(dir.path());
    fs::write(pre_commit(dir.path()), FIXTURE_LEFTHOOK).unwrap();

    let surface = CodexSurface;
    let report = surface
        .install_detailed(&ctx(dir.path().to_path_buf()))
        .unwrap();

    assert_eq!(read(&pre_commit(dir.path())), FIXTURE_LEFTHOOK);
    assert!(
        report.warnings.iter().any(|w| matches!(
            w,
            HookWarning::Skipped {
                conflict: HookConflict::Lefthook,
                ..
            }
        )),
        "expected a Lefthook skip warning, got {:?}",
        report.warnings,
    );
}

#[test]
fn install_detects_pre_commit_framework_and_skips_with_warning() {
    let dir = tempfile::tempdir().unwrap();
    ensure_git_hooks_dir(dir.path());
    fs::write(pre_commit(dir.path()), FIXTURE_PRECOMMIT_FRAMEWORK).unwrap();

    let surface = CodexSurface;
    let report = surface
        .install_detailed(&ctx(dir.path().to_path_buf()))
        .unwrap();

    assert_eq!(read(&pre_commit(dir.path())), FIXTURE_PRECOMMIT_FRAMEWORK);
    assert!(
        report.warnings.iter().any(|w| matches!(
            w,
            HookWarning::Skipped {
                conflict: HookConflict::PreCommit,
                ..
            }
        )),
        "expected a PreCommit skip warning, got {:?}",
        report.warnings,
    );
}

#[test]
fn install_does_not_fail_when_conflicts_are_present() {
    // Acceptance criterion: klasp returns a structured warning in the
    // install report; doesn't fail the install.
    let dir = tempfile::tempdir().unwrap();
    ensure_git_hooks_dir(dir.path());
    fs::write(pre_commit(dir.path()), FIXTURE_HUSKY).unwrap();
    fs::write(pre_push(dir.path()), FIXTURE_LEFTHOOK).unwrap();

    let surface = CodexSurface;
    let result = surface.install(&ctx(dir.path().to_path_buf()));
    assert!(
        result.is_ok(),
        "install must not fail on conflict; got {result:?}",
    );
    // Both fixtures are untouched.
    assert_eq!(read(&pre_commit(dir.path())), FIXTURE_HUSKY);
    assert_eq!(read(&pre_push(dir.path())), FIXTURE_LEFTHOOK);
}

// ────────────────────────────────────────────────────────────────────
// Uninstall paths
// ────────────────────────────────────────────────────────────────────

#[test]
fn uninstall_strips_klasp_section_preserves_user_content() {
    let dir = tempfile::tempdir().unwrap();
    ensure_git_hooks_dir(dir.path());
    fs::write(pre_commit(dir.path()), FIXTURE_USER_BASH).unwrap();

    let surface = CodexSurface;
    surface.install(&ctx(dir.path().to_path_buf())).unwrap();
    surface.uninstall(dir.path(), false).unwrap();

    let body = read(&pre_commit(dir.path()));
    assert_eq!(
        body, FIXTURE_USER_BASH,
        "uninstall did not restore the user-authored hook byte-for-byte"
    );
}

#[test]
fn uninstall_removes_file_when_klasp_was_only_content() {
    // Round-trip from the fresh-create install: install creates the
    // hook file from scratch (shebang + block); uninstall must remove
    // it again so the repo has no residual klasp shrapnel.
    let dir = tempfile::tempdir().unwrap();
    let surface = CodexSurface;
    surface.install(&ctx(dir.path().to_path_buf())).unwrap();
    assert!(pre_commit(dir.path()).exists());
    assert!(pre_push(dir.path()).exists());

    surface.uninstall(dir.path(), false).unwrap();
    assert!(
        !pre_commit(dir.path()).exists(),
        "fresh-create round-trip left a residual pre-commit file"
    );
    assert!(
        !pre_push(dir.path()).exists(),
        "fresh-create round-trip left a residual pre-push file"
    );
}

#[test]
fn uninstall_leaves_foreign_tool_hook_untouched() {
    let dir = tempfile::tempdir().unwrap();
    ensure_git_hooks_dir(dir.path());
    fs::write(pre_commit(dir.path()), FIXTURE_HUSKY).unwrap();

    let surface = CodexSurface;
    // First install to capture warnings, then uninstall.
    surface.install(&ctx(dir.path().to_path_buf())).unwrap();
    surface.uninstall(dir.path(), false).unwrap();

    // Husky hook is byte-for-byte unchanged across both install and
    // uninstall. klasp never wrote into it, so there's no managed
    // block to strip.
    assert_eq!(read(&pre_commit(dir.path())), FIXTURE_HUSKY);
}

#[test]
fn uninstall_dry_run_does_not_modify_disk() {
    let dir = tempfile::tempdir().unwrap();
    let surface = CodexSurface;
    surface.install(&ctx(dir.path().to_path_buf())).unwrap();

    let pre_install_body = read(&pre_commit(dir.path()));
    surface.uninstall(dir.path(), true).unwrap();
    let post_dry_run_body = read(&pre_commit(dir.path()));

    assert_eq!(pre_install_body, post_dry_run_body);
    assert!(pre_commit(dir.path()).exists());
}

// ────────────────────────────────────────────────────────────────────
// Module-level conflict-detection sanity checks against fixtures
// ────────────────────────────────────────────────────────────────────

#[test]
fn detect_conflict_returns_husky_for_real_husky_fixture() {
    assert_eq!(
        git_hooks::detect_conflict(FIXTURE_HUSKY),
        Some(HookConflict::Husky),
    );
}

#[test]
fn detect_conflict_returns_husky_for_husky_v9_h_shim() {
    // husky v9 shortened the shim path from `_/husky.sh` to `_/h`. Without
    // the `_/h"` substring in `detect_conflict`, klasp would silently
    // append its block to a husky-managed hook on any v9-or-newer repo.
    assert_eq!(
        git_hooks::detect_conflict(FIXTURE_HUSKY_V9),
        Some(HookConflict::Husky),
    );
}

#[test]
fn detect_conflict_does_not_false_positive_on_husky_in_user_comment() {
    // A user comment merely mentioning husky must not trip the husky
    // arm. The pre-fix detection used a bare `.husky/` substring which
    // would have false-positived this hook.
    let user_hook = "#!/usr/bin/env sh\n# Migrated from .husky/ — now managed manually\nnpm test\n";
    assert_eq!(git_hooks::detect_conflict(user_hook), None);
}

#[test]
fn detect_conflict_returns_lefthook_for_real_lefthook_fixture() {
    assert_eq!(
        git_hooks::detect_conflict(FIXTURE_LEFTHOOK),
        Some(HookConflict::Lefthook),
    );
}

#[test]
fn detect_conflict_returns_pre_commit_for_real_pre_commit_framework_fixture() {
    assert_eq!(
        git_hooks::detect_conflict(FIXTURE_PRECOMMIT_FRAMEWORK),
        Some(HookConflict::PreCommit),
    );
}

#[test]
fn detect_conflict_returns_none_for_user_hook() {
    assert_eq!(git_hooks::detect_conflict(FIXTURE_USER_BASH), None);
}
