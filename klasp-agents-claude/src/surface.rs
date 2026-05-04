//! `ClaudeCodeSurface` — `klasp_core::AgentSurface` impl for Claude Code.
//!
//! Implements the install flow described in [docs/design.md §5]:
//!
//! 1. Compute paths.
//! 2. Render the hook script.
//! 3. Idempotency: managed-marker presence in an existing hook file.
//! 4. Honour `--dry-run` (preview only, no writes).
//! 5. Atomic write of the script + chmod 0o755 (Unix).
//! 6. Surgical merge into `.claude/settings.json`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use klasp_core::{AgentSurface, InstallContext, InstallError, InstallReport};

use crate::hook_template::{self, MANAGED_MARKER};
use crate::settings::{self, SettingsError};

/// Claude Code agent surface. Stateless; the registry stores it as
/// `Box<dyn AgentSurface>`.
pub struct ClaudeCodeSurface;

impl ClaudeCodeSurface {
    pub const AGENT_ID: &'static str = "claude_code";

    /// The literal `command` string klasp embeds in `.claude/settings.json`'s
    /// `hooks.PreToolUse[Bash]` matcher. `${CLAUDE_PROJECT_DIR}` is resolved
    /// by Claude Code at hook-execution time, so the same settings.json works
    /// regardless of the CWD Claude is invoked from. See plan: "Hook entry
    /// `command` value" decision.
    pub const HOOK_COMMAND: &'static str = "${CLAUDE_PROJECT_DIR}/.claude/hooks/klasp-gate.sh";
}

impl AgentSurface for ClaudeCodeSurface {
    fn agent_id(&self) -> &'static str {
        Self::AGENT_ID
    }

    fn detect(&self, repo_root: &Path) -> bool {
        repo_root.join(".claude").is_dir()
    }

    fn hook_path(&self, repo_root: &Path) -> PathBuf {
        repo_root
            .join(".claude")
            .join("hooks")
            .join("klasp-gate.sh")
    }

    fn settings_path(&self, repo_root: &Path) -> PathBuf {
        repo_root.join(".claude").join("settings.json")
    }

    fn render_hook_script(&self, ctx: &InstallContext) -> String {
        hook_template::render(ctx.schema_version)
    }

    fn install(&self, ctx: &InstallContext) -> Result<InstallReport, InstallError> {
        let hook_path = self.hook_path(&ctx.repo_root);
        let settings_path = self.settings_path(&ctx.repo_root);
        let rendered = self.render_hook_script(ctx);

        let hook_state = inspect_hook_file(&hook_path, &rendered, ctx.force)?;

        let settings_input = read_or_empty(&settings_path)?;
        let merged = settings::merge_hook_entry(&settings_input, Self::HOOK_COMMAND)
            .map_err(|e| settings_error(&settings_path, e))?;
        let settings_unchanged = merged == settings_input;

        let already_installed = matches!(hook_state, HookState::Identical) && settings_unchanged;

        if ctx.dry_run {
            return Ok(InstallReport {
                agent_id: Self::AGENT_ID.to_string(),
                hook_path,
                settings_path,
                already_installed,
                paths_written: Vec::new(),
                preview: Some(rendered),
            });
        }

        let mut paths_written = Vec::new();

        if !matches!(hook_state, HookState::Identical) {
            ensure_parent(&hook_path)?;
            atomic_write(&hook_path, rendered.as_bytes(), 0o755)?;
            paths_written.push(hook_path.clone());
        }

        if !settings_unchanged {
            ensure_parent(&settings_path)?;
            // Preserve the user's prior mode rather than overwriting it with
            // NamedTempFile's 0o600 default; fall back to 0o644 for new files.
            let mode = current_mode(&settings_path).unwrap_or(0o644);
            atomic_write(&settings_path, merged.as_bytes(), mode)?;
            paths_written.push(settings_path.clone());
        }

        Ok(InstallReport {
            agent_id: Self::AGENT_ID.to_string(),
            hook_path,
            settings_path,
            already_installed,
            paths_written,
            preview: None,
        })
    }

    fn uninstall(&self, repo_root: &Path, dry_run: bool) -> Result<Vec<PathBuf>, InstallError> {
        let hook_path = self.hook_path(repo_root);
        let settings_path = self.settings_path(repo_root);
        let mut paths = Vec::new();

        if hook_path.exists() {
            let existing = fs::read_to_string(&hook_path).map_err(|e| InstallError::Io {
                path: hook_path.clone(),
                source: e,
            })?;
            if existing.contains(MANAGED_MARKER) {
                if !dry_run {
                    fs::remove_file(&hook_path).map_err(|e| InstallError::Io {
                        path: hook_path.clone(),
                        source: e,
                    })?;
                }
                paths.push(hook_path);
            }
        }

        if settings_path.exists() {
            let existing = fs::read_to_string(&settings_path).map_err(|e| InstallError::Io {
                path: settings_path.clone(),
                source: e,
            })?;
            let new = settings::unmerge_hook_entry(&existing, Self::HOOK_COMMAND)
                .map_err(|e| settings_error(&settings_path, e))?;
            if new != existing {
                if !dry_run {
                    let mode = current_mode(&settings_path).unwrap_or(0o644);
                    atomic_write(&settings_path, new.as_bytes(), mode)?;
                }
                paths.push(settings_path);
            }
        }

        Ok(paths)
    }
}

enum HookState {
    Identical,
    Writable,
}

fn inspect_hook_file(
    hook_path: &Path,
    rendered: &str,
    force: bool,
) -> Result<HookState, InstallError> {
    if !hook_path.exists() {
        return Ok(HookState::Writable);
    }
    let existing = fs::read_to_string(hook_path).map_err(|e| InstallError::Io {
        path: hook_path.to_path_buf(),
        source: e,
    })?;
    if existing.contains(MANAGED_MARKER) {
        if existing == rendered {
            Ok(HookState::Identical)
        } else {
            Ok(HookState::Writable)
        }
    } else if force {
        Ok(HookState::Writable)
    } else {
        Err(InstallError::MarkerConflict {
            path: hook_path.to_path_buf(),
        })
    }
}

fn read_or_empty(path: &Path) -> Result<String, InstallError> {
    if !path.exists() {
        return Ok(String::new());
    }
    fs::read_to_string(path).map_err(|e| InstallError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

fn ensure_parent(path: &Path) -> Result<(), InstallError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::create_dir_all(parent).map_err(|e| InstallError::Io {
        path: parent.to_path_buf(),
        source: e,
    })
}

/// Atomic write via tempfile + rename. `mode` is applied after the rename
/// (Unix only) — without it the destination silently inherits
/// `NamedTempFile`'s `0o600` default.
fn atomic_write(path: &Path, contents: &[u8], mode: u32) -> Result<(), InstallError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tf = tempfile::NamedTempFile::new_in(dir).map_err(|e| InstallError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;
    tf.write_all(contents).map_err(|e| InstallError::Io {
        path: tf.path().to_path_buf(),
        source: e,
    })?;
    tf.flush().map_err(|e| InstallError::Io {
        path: tf.path().to_path_buf(),
        source: e,
    })?;
    tf.persist(path).map_err(|e| InstallError::Io {
        path: path.to_path_buf(),
        source: e.error,
    })?;
    apply_mode(path, mode)?;
    Ok(())
}

/// The file's current Unix mode (low 12 bits), or `None` if the file
/// doesn't exist or we're not on Unix. Called *before* `atomic_write`
/// so we can restore the user's prior mode after the rename.
#[cfg(unix)]
fn current_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).ok().map(|m| m.permissions().mode())
}

#[cfg(not(unix))]
fn current_mode(_path: &Path) -> Option<u32> {
    None
}

fn apply_mode(path: &Path, mode: u32) -> Result<(), InstallError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(mode);
        fs::set_permissions(path, perms).map_err(|e| InstallError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
    }
    #[cfg(not(unix))]
    {
        // Windows: bash interprets the shebang regardless of the NTFS bit.
        // Per design.md §14, the Windows path/permission audit lands at W4.
        let _ = (path, mode);
    }
    Ok(())
}

fn settings_error(path: &Path, error: SettingsError) -> InstallError {
    match error {
        SettingsError::Parse(source) => InstallError::SettingsParse {
            path: path.to_path_buf(),
            source,
        },
        shape @ SettingsError::Shape { .. } => InstallError::Surface {
            agent_id: ClaudeCodeSurface::AGENT_ID.to_string(),
            message: shape.to_string(),
        },
    }
}
