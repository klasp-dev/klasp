//! `CodexSurface` — `klasp_core::AgentSurface` impl for Codex.
//!
//! Mirrors the install flow of [`klasp_agents_claude::ClaudeCodeSurface`]:
//! compute paths → render the managed-block body → idempotent
//! merge / replace into the on-disk file → atomic write → report what
//! changed. The two surfaces differ in their target file (Codex writes to
//! `AGENTS.md` markdown, Claude writes to `.claude/settings.json`) and in
//! their hook-script: Codex's executable hook script is owned by W2
//! (issue #28) and is *not* written by this surface yet — `install` here
//! is purely the AGENTS.md managed-block writer.
//!
//! ## v0.2 W1 scope
//!
//! - `install` writes (or updates) the managed block in `AGENTS.md`.
//! - `install` does **not** write a git-hooks script — that's W2 (#28).
//!   Until W2 lands, `render_hook_script` returns a placeholder and
//!   `install` does not touch [`hook_path`].
//! - `uninstall` strips the managed block from `AGENTS.md` and removes the
//!   file when that empties it.
//!
//! ## Windows notes
//!
//! `AGENTS.md` is plain text — no executable bit needed. All `Path::join`
//! calls produce platform-correct separators.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use klasp_core::{AgentSurface, InstallContext, InstallError, InstallReport};

use crate::agents_md::{self, AgentsMdError, DEFAULT_BLOCK_BODY};

/// Codex agent surface. Stateless; the registry stores it as
/// `Box<dyn AgentSurface>`.
pub struct CodexSurface;

impl CodexSurface {
    pub const AGENT_ID: &'static str = "codex";

    /// Filename of the markdown file Codex reads from the repo root.
    pub const AGENTS_MD: &'static str = "AGENTS.md";

    /// Repo-relative path of the git pre-commit hook W2 (#28) will own.
    /// Exposed here so `hook_path` can return a stable value before W2
    /// lands; this surface does **not** write the hook in v0.2 W1.
    pub const HOOK_RELPATH: &'static [&'static str] = &[".git", "hooks", "pre-commit"];
}

impl AgentSurface for CodexSurface {
    fn agent_id(&self) -> &'static str {
        Self::AGENT_ID
    }

    fn detect(&self, repo_root: &Path) -> bool {
        // Codex looks for `AGENTS.md` at the repo root. We treat its
        // presence as the auto-detect signal; `klasp install --force`
        // overrides a `false` return if the user wants to bootstrap the
        // file from scratch.
        repo_root.join(Self::AGENTS_MD).is_file()
    }

    fn hook_path(&self, repo_root: &Path) -> PathBuf {
        let mut p = repo_root.to_path_buf();
        for seg in Self::HOOK_RELPATH {
            p.push(seg);
        }
        p
    }

    fn settings_path(&self, repo_root: &Path) -> PathBuf {
        repo_root.join(Self::AGENTS_MD)
    }

    fn render_hook_script(&self, _ctx: &InstallContext) -> String {
        // W2 (#28) owns the actual pre-commit script body. Returning the
        // empty string here keeps the trait satisfied while making it
        // structurally obvious that v0.2 W1 has nothing to render.
        String::new()
    }

    fn install(&self, ctx: &InstallContext) -> Result<InstallReport, InstallError> {
        let settings_path = self.settings_path(&ctx.repo_root);
        let hook_path = self.hook_path(&ctx.repo_root);

        let existing = read_or_empty(&settings_path)?;
        let merged = agents_md::install_block(&existing, DEFAULT_BLOCK_BODY)
            .map_err(|e| agents_md_error(&settings_path, e))?;

        // `install_block` only returns the input unchanged when a managed
        // block was already present with identical content, so equality
        // implies "already installed" — no second `contains_block` scan.
        let already_installed = merged == existing;

        if ctx.dry_run {
            return Ok(InstallReport {
                agent_id: Self::AGENT_ID.to_string(),
                hook_path,
                settings_path,
                already_installed,
                paths_written: Vec::new(),
                preview: Some(merged),
            });
        }

        let mut paths_written = Vec::new();
        if merged != existing {
            ensure_parent(&settings_path)?;
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
        let settings_path = self.settings_path(repo_root);
        let mut paths = Vec::new();

        // `read_or_empty` collapses missing-file → empty-string. An empty
        // input is also unchanged by `uninstall_block`, so the `stripped ==
        // existing` early-return below covers the missing-file case
        // without a separate `exists()` check.
        let existing = read_or_empty(&settings_path)?;

        let stripped =
            agents_md::uninstall_block(&existing).map_err(|e| agents_md_error(&settings_path, e))?;

        if stripped == existing {
            return Ok(paths);
        }

        if !dry_run {
            if stripped.is_empty() {
                // The file existed only because klasp created it; remove it
                // so uninstall is a true round-trip from the missing-file
                // install path.
                fs::remove_file(&settings_path).map_err(|e| InstallError::Io {
                    path: settings_path.clone(),
                    source: e,
                })?;
            } else {
                let mode = current_mode(&settings_path).unwrap_or(0o644);
                atomic_write(&settings_path, stripped.as_bytes(), mode)?;
            }
        }
        paths.push(settings_path);
        Ok(paths)
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
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
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
        let _ = (path, mode);
    }
    Ok(())
}

fn agents_md_error(path: &Path, error: AgentsMdError) -> InstallError {
    InstallError::Surface {
        agent_id: CodexSurface::AGENT_ID.to_string(),
        message: format!("{}: {error}", path.display()),
    }
}
