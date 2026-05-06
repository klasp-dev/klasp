//! Integration tests: drive `AiderSurface::install` / `uninstall` over a
//! tempdir and verify the `.aider.conf.yml` commit-cmd-pre contract.

use std::fs;
use std::path::PathBuf;

use klasp_agents_aider::aider_conf::KLASP_CMD;
use klasp_agents_aider::AiderSurface;
use klasp_core::{AgentSurface, InstallContext};

fn ctx(repo_root: PathBuf) -> InstallContext {
    InstallContext {
        repo_root,
        dry_run: false,
        force: false,
        schema_version: 2,
    }
}

fn read(path: &std::path::Path) -> String {
    fs::read_to_string(path).expect("read .aider.conf.yml")
}

// ── 1. Empty repo — no .aider.conf.yml ───────────────────────────────────────

#[test]
fn install_into_empty_repo() {
    let dir = tempfile::tempdir().unwrap();
    let surface = AiderSurface;

    let report = surface.install(&ctx(dir.path().to_path_buf())).unwrap();

    assert_eq!(report.agent_id, AiderSurface::AGENT_ID);
    assert!(!report.already_installed);
    assert_eq!(report.paths_written.len(), 1);

    let conf = dir.path().join(".aider.conf.yml");
    assert!(conf.is_file());
    let body = read(&conf);
    assert!(body.contains(KLASP_CMD), "KLASP_CMD not found in: {body:?}");
}

// ── 2. Config exists with other keys but no commit-cmd-pre ───────────────────

#[test]
fn install_with_no_commit_cmd_pre_preserves_other_keys() {
    let dir = tempfile::tempdir().unwrap();
    let conf = dir.path().join(".aider.conf.yml");
    // Note: YAML round-trip does NOT preserve inline comments or bespoke
    // whitespace. Logical key/value content is preserved; formatting may differ.
    fs::write(&conf, "model: gpt-4o\nauto-commits: false\n").unwrap();

    let surface = AiderSurface;
    let report = surface.install(&ctx(dir.path().to_path_buf())).unwrap();

    assert!(!report.already_installed);
    let body = read(&conf);
    assert!(body.contains(KLASP_CMD));
    // Other keys must still be present.
    assert!(body.contains("gpt-4o"), "model key lost: {body:?}");
    assert!(
        body.contains("auto-commits"),
        "auto-commits key lost: {body:?}"
    );
}

// ── 3. Already has klasp value — idempotent ──────────────────────────────────

#[test]
fn install_with_existing_klasp_value_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let conf = dir.path().join(".aider.conf.yml");
    fs::write(&conf, format!("commit-cmd-pre: {KLASP_CMD}\n")).unwrap();

    let surface = AiderSurface;

    // First install: already present, should be a no-op.
    let report = surface.install(&ctx(dir.path().to_path_buf())).unwrap();
    assert!(report.already_installed);
    assert!(report.paths_written.is_empty());

    let body_after = read(&conf);
    // File content should reflect klasp cmd; we don't assert byte-identity
    // since the file was not touched.
    assert!(body_after.contains(KLASP_CMD));
}

// ── 4. Non-klasp scalar value → chain ────────────────────────────────────────

#[test]
fn install_with_existing_non_klasp_value_chains() {
    let dir = tempfile::tempdir().unwrap();
    let conf = dir.path().join(".aider.conf.yml");
    fs::write(&conf, "commit-cmd-pre: pytest -q\n").unwrap();

    let surface = AiderSurface;
    let report = surface.install(&ctx(dir.path().to_path_buf())).unwrap();

    assert!(!report.already_installed);
    let body = read(&conf);
    // klasp must appear before the user command in the serialized output.
    let klasp_pos = body.find(KLASP_CMD).expect("KLASP_CMD not found");
    let user_pos = body.find("pytest -q").expect("user cmd not found");
    assert!(
        klasp_pos < user_pos,
        "klasp must appear before user cmd. body: {body:?}"
    );
}

// ── 5. Non-klasp array value → prepend ───────────────────────────────────────

#[test]
fn install_with_existing_array_value_prepends() {
    let dir = tempfile::tempdir().unwrap();
    let conf = dir.path().join(".aider.conf.yml");
    fs::write(&conf, "commit-cmd-pre:\n  - lint\n  - format\n").unwrap();

    let surface = AiderSurface;
    surface.install(&ctx(dir.path().to_path_buf())).unwrap();

    let body = read(&conf);
    assert!(body.contains(KLASP_CMD));
    assert!(body.contains("lint"));
    assert!(body.contains("format"));
    // klasp must appear before lint.
    let klasp_pos = body.find(KLASP_CMD).expect("KLASP_CMD not found");
    let lint_pos = body.find("lint").expect("lint not found");
    assert!(klasp_pos < lint_pos, "klasp must be first in array");
}

// ── 6. Uninstall removes only the klasp entry ─────────────────────────────────

#[test]
fn uninstall_removes_klasp_entry_only() {
    let dir = tempfile::tempdir().unwrap();
    let conf = dir.path().join(".aider.conf.yml");
    // Start with a user command + other key.
    fs::write(&conf, "model: gpt-4o\ncommit-cmd-pre: pytest -q\n").unwrap();

    let surface = AiderSurface;
    surface.install(&ctx(dir.path().to_path_buf())).unwrap();

    // After install, both klasp and user cmd should be present.
    let body_installed = read(&conf);
    assert!(body_installed.contains(KLASP_CMD));
    assert!(body_installed.contains("pytest -q"));

    // Uninstall should remove only the klasp entry.
    let paths = surface.uninstall(dir.path(), false).unwrap();
    assert_eq!(paths.len(), 1);

    let body_after = read(&conf);
    assert!(
        !body_after.contains(KLASP_CMD),
        "klasp still present: {body_after:?}"
    );
    // User command and other key must survive.
    assert!(
        body_after.contains("pytest -q"),
        "user cmd lost: {body_after:?}"
    );
    assert!(
        body_after.contains("gpt-4o"),
        "model key lost: {body_after:?}"
    );
}

// ── 7. Uninstall when not installed is a noop ─────────────────────────────────

#[test]
fn uninstall_when_not_installed_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let conf = dir.path().join(".aider.conf.yml");
    fs::write(&conf, "model: gpt-4o\ncommit-cmd-pre: pytest -q\n").unwrap();

    let surface = AiderSurface;
    let paths = surface.uninstall(dir.path(), false).unwrap();

    // klasp was never installed — nothing should be touched.
    assert!(paths.is_empty(), "expected empty paths, got: {paths:?}");
    // File content must be unchanged.
    let body = read(&conf);
    assert!(body.contains("pytest -q"));
}

// ── Extra: uninstall on missing file is a noop ───────────────────────────────

#[test]
fn uninstall_on_missing_conf_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let surface = AiderSurface;
    let paths = surface.uninstall(dir.path(), false).unwrap();
    assert!(paths.is_empty());
}

// ── Extra: detect returns false when file absent, true when present ───────────

#[test]
fn detect_follows_file_presence() {
    let dir = tempfile::tempdir().unwrap();
    let surface = AiderSurface;
    assert!(!surface.detect(dir.path()));

    fs::write(dir.path().join(".aider.conf.yml"), "").unwrap();
    assert!(surface.detect(dir.path()));
}
