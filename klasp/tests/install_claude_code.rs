//! End-to-end integration test for `ClaudeCodeSurface::install` / `uninstall`.
//!
//! Per [docs/design.md §10] ("Integration tests"): construct a temp dir
//! containing `.git/` and (optionally) a pre-existing `.claude/`, run the
//! install, assert filesystem state, run again to verify idempotency, then
//! uninstall and verify sibling preservation.

use std::fs;
use std::path::Path;

use klasp_agents_claude::ClaudeCodeSurface;
use klasp_core::{AgentSurface, InstallContext, GATE_SCHEMA_VERSION};
use serde_json::Value;

const KLASP_CMD: &str = ClaudeCodeSurface::HOOK_COMMAND;

fn fresh_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::create_dir(dir.path().join(".claude")).unwrap();
    dir
}

fn ctx_for(root: &Path, dry_run: bool) -> InstallContext {
    InstallContext {
        repo_root: root.to_path_buf(),
        dry_run,
        force: false,
        schema_version: GATE_SCHEMA_VERSION,
    }
}

#[test]
fn install_writes_hook_and_settings() {
    let repo = fresh_repo();
    let surface = ClaudeCodeSurface;

    let report = surface.install(&ctx_for(repo.path(), false)).unwrap();
    assert_eq!(report.agent_id, "claude_code");
    assert!(!report.already_installed);

    let hook_path = repo.path().join(".claude/hooks/klasp-gate.sh");
    let settings_path = repo.path().join(".claude/settings.json");
    assert!(hook_path.exists(), "hook script must be written");
    assert!(settings_path.exists(), "settings.json must be created");

    let script = fs::read_to_string(&hook_path).unwrap();
    assert!(script.contains(&format!("export KLASP_GATE_SCHEMA={GATE_SCHEMA_VERSION}")));
    assert!(script.contains("# klasp:managed"));
    assert!(script.contains("exec klasp gate"));

    let settings: Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(
        settings["hooks"]["PreToolUse"][0]["matcher"], "Bash",
        "Bash matcher created"
    );
    assert_eq!(
        settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        KLASP_CMD,
    );

    assert_eq!(report.paths_written.len(), 2);
}

#[test]
fn second_install_is_no_op() {
    let repo = fresh_repo();
    let surface = ClaudeCodeSurface;
    surface.install(&ctx_for(repo.path(), false)).unwrap();

    let second = surface.install(&ctx_for(repo.path(), false)).unwrap();
    assert!(second.already_installed, "{second:?}");
    assert!(second.paths_written.is_empty());
}

#[test]
fn dry_run_writes_nothing_but_returns_preview() {
    let repo = fresh_repo();
    let surface = ClaudeCodeSurface;

    let report = surface.install(&ctx_for(repo.path(), true)).unwrap();
    assert!(report.preview.is_some());
    assert!(report.paths_written.is_empty());
    assert!(!repo.path().join(".claude/hooks/klasp-gate.sh").exists());
    assert!(!repo.path().join(".claude/settings.json").exists());
}

#[test]
fn install_preserves_sibling_settings_keys() {
    let repo = fresh_repo();
    fs::write(
        repo.path().join(".claude/settings.json"),
        r#"{
            "theme": "dark",
            "hooks": {
                "PreToolUse": [
                    { "matcher": "Bash", "hooks": [{ "type": "command", "command": "fallow gate" }] }
                ]
            }
        }"#,
    )
    .unwrap();

    let surface = ClaudeCodeSurface;
    surface.install(&ctx_for(repo.path(), false)).unwrap();

    let settings: Value = serde_json::from_str(
        &fs::read_to_string(repo.path().join(".claude/settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(settings["theme"], "dark");
    let inner = settings["hooks"]["PreToolUse"][0]["hooks"]
        .as_array()
        .unwrap();
    assert_eq!(inner.len(), 2);
    assert_eq!(inner[0]["command"], "fallow gate");
    assert_eq!(inner[1]["command"], KLASP_CMD);
}

#[test]
fn install_refuses_when_existing_hook_lacks_marker_without_force() {
    let repo = fresh_repo();
    let hook_path = repo.path().join(".claude/hooks/klasp-gate.sh");
    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    fs::write(&hook_path, "#!/usr/bin/env bash\necho impostor\n").unwrap();

    let surface = ClaudeCodeSurface;
    let err = surface.install(&ctx_for(repo.path(), false)).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("managed marker"), "{msg}");
}

#[test]
fn install_overwrites_unmarked_hook_with_force() {
    let repo = fresh_repo();
    let hook_path = repo.path().join(".claude/hooks/klasp-gate.sh");
    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    fs::write(&hook_path, "#!/usr/bin/env bash\necho impostor\n").unwrap();

    let surface = ClaudeCodeSurface;
    let ctx = InstallContext {
        repo_root: repo.path().to_path_buf(),
        dry_run: false,
        force: true,
        schema_version: GATE_SCHEMA_VERSION,
    };
    surface.install(&ctx).unwrap();

    let written = fs::read_to_string(&hook_path).unwrap();
    assert!(written.contains("# klasp:managed"));
}

#[test]
fn uninstall_removes_klasp_and_preserves_siblings() {
    let repo = fresh_repo();
    fs::write(
        repo.path().join(".claude/settings.json"),
        r#"{
            "theme": "dark",
            "hooks": {
                "PreToolUse": [
                    { "matcher": "Bash", "hooks": [{ "type": "command", "command": "fallow gate" }] }
                ]
            }
        }"#,
    )
    .unwrap();

    let surface = ClaudeCodeSurface;
    surface.install(&ctx_for(repo.path(), false)).unwrap();

    let touched = surface.uninstall(repo.path(), false).unwrap();
    assert_eq!(touched.len(), 2, "{touched:?}");

    assert!(!repo.path().join(".claude/hooks/klasp-gate.sh").exists());

    let settings: Value = serde_json::from_str(
        &fs::read_to_string(repo.path().join(".claude/settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(settings["theme"], "dark");
    let inner = settings["hooks"]["PreToolUse"][0]["hooks"]
        .as_array()
        .unwrap();
    assert_eq!(inner.len(), 1);
    assert_eq!(inner[0]["command"], "fallow gate");
}

#[cfg(unix)]
#[test]
fn install_preserves_existing_settings_mode() {
    use std::os::unix::fs::PermissionsExt;

    let repo = fresh_repo();
    let settings_path = repo.path().join(".claude/settings.json");
    fs::write(&settings_path, "{}").unwrap();
    fs::set_permissions(&settings_path, fs::Permissions::from_mode(0o644)).unwrap();

    ClaudeCodeSurface
        .install(&ctx_for(repo.path(), false))
        .unwrap();

    let mode = fs::metadata(&settings_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o644,
        "atomic_write must preserve existing settings.json mode, not downgrade to NamedTempFile's 0o600 default"
    );
}

#[cfg(unix)]
#[test]
fn install_creates_fresh_settings_with_0o644() {
    use std::os::unix::fs::PermissionsExt;

    let repo = fresh_repo();
    let settings_path = repo.path().join(".claude/settings.json");
    assert!(!settings_path.exists(), "fresh repo has no settings.json");

    ClaudeCodeSurface
        .install(&ctx_for(repo.path(), false))
        .unwrap();

    let mode = fs::metadata(&settings_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o644,
        "freshly-created settings.json should land at 0o644, not the 0o600 default of NamedTempFile"
    );
}

#[cfg(unix)]
#[test]
fn install_writes_hook_with_0o755() {
    use std::os::unix::fs::PermissionsExt;

    let repo = fresh_repo();
    ClaudeCodeSurface
        .install(&ctx_for(repo.path(), false))
        .unwrap();

    let mode = fs::metadata(repo.path().join(".claude/hooks/klasp-gate.sh"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o755);
}

#[test]
fn uninstall_dry_run_writes_nothing() {
    let repo = fresh_repo();
    let surface = ClaudeCodeSurface;
    surface.install(&ctx_for(repo.path(), false)).unwrap();

    let _ = surface.uninstall(repo.path(), true).unwrap();

    assert!(repo.path().join(".claude/hooks/klasp-gate.sh").exists());
    let settings = fs::read_to_string(repo.path().join(".claude/settings.json")).unwrap();
    assert!(settings.contains(KLASP_CMD));
}
