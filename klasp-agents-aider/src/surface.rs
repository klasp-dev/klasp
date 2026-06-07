//! `AiderSurface` — `klasp_core::AgentSurface` impl for Aider.
//!
//! Aider reads `.aider.conf.yml` from the repo root (and optionally from the
//! user home directory, but global config is out of scope for v0.3 W1).
//! The surface edits the `commit-cmd-pre` key using the chain strategy
//! documented in `aider_conf.rs`: klasp is prepended so the gate runs first,
//! and any pre-existing command remains in the array and continues to run.
//!
//! ## Limitations
//!
//! YAML round-trip via `serde_yaml_ng` does not preserve user comments or
//! bespoke whitespace formatting. After an `install`/`uninstall` cycle the
//! logical content (keys + values) is identical to the original but inline
//! comments and blank-line formatting may be lost. This is a known limitation
//! of structured-YAML mutation; the alternative (line-patch text markers)
//! would risk corrupting structured content. See crate README `### Limitations`.

use std::fs;
use std::path::{Path, PathBuf};

use klasp_core::fs::{atomic_write, current_mode, ensure_parent, read_or_empty};
use klasp_core::{AgentSurface, InstallContext, InstallError, InstallReport};

use crate::aider_conf;

/// Aider agent surface. Stateless; the registry stores it as
/// `Box<dyn AgentSurface>`.
pub struct AiderSurface;

impl AiderSurface {
    pub const AGENT_ID: &'static str = "aider";
    pub const CONF_FILENAME: &'static str = ".aider.conf.yml";
}

impl AgentSurface for AiderSurface {
    fn agent_id(&self) -> &'static str {
        Self::AGENT_ID
    }

    /// Detect aider usage by the presence of `.aider.conf.yml` in the repo
    /// root. Global `~/.aider.conf.yml` lookup is out of scope for v0.3 W1.
    fn detect(&self, repo_root: &Path) -> bool {
        repo_root.join(Self::CONF_FILENAME).is_file()
    }

    /// For aider, the "hook" is the `commit-cmd-pre` field in
    /// `.aider.conf.yml`. There is no separate hook-script file; return the
    /// config path for both `hook_path` and `settings_path`.
    fn hook_path(&self, repo_root: &Path) -> PathBuf {
        repo_root.join(Self::CONF_FILENAME)
    }

    fn settings_path(&self, repo_root: &Path) -> PathBuf {
        repo_root.join(Self::CONF_FILENAME)
    }

    /// The canonical `.aider.conf.yml` content for a fresh install (empty doc,
    /// only the `commit-cmd-pre` key set). Used by `klasp doctor` to verify
    /// that the installed file's logical state matches the expected state via
    /// byte-equality of the YAML the surface would write.
    ///
    /// When the user had an existing `.aider.conf.yml` before installing klasp,
    /// the on-disk file will differ from this minimal form (it carries the user's
    /// other keys). In that case, doctor's byte-equality check will differ — that
    /// is expected and `check_hook` produces a `FAIL hook[aider]` line. A
    /// future per-surface health-check trait method will replace this heuristic.
    fn render_hook_script(&self, _ctx: &InstallContext) -> String {
        let mut doc = aider_conf::parse("").unwrap_or_default();
        if aider_conf::install_into_doc(&mut doc).unwrap_or(false) {
            aider_conf::serialize(&doc).unwrap_or_else(|_| aider_conf::KLASP_CMD.to_string())
        } else {
            aider_conf::KLASP_CMD.to_string()
        }
    }

    fn install(&self, ctx: &InstallContext) -> Result<InstallReport, InstallError> {
        let conf_path = self.settings_path(&ctx.repo_root);
        let src = read_or_empty(&conf_path)?;

        let mut doc = aider_conf::parse(&src).map_err(|e| conf_error(&conf_path, e))?;
        let changed =
            aider_conf::install_into_doc(&mut doc).map_err(|e| conf_error(&conf_path, e))?;

        let already_installed = !changed;

        if ctx.dry_run {
            let preview = aider_conf::serialize(&doc).map_err(|e| conf_error(&conf_path, e))?;
            return Ok(InstallReport {
                agent_id: Self::AGENT_ID.to_string(),
                hook_path: conf_path.clone(),
                settings_path: conf_path,
                already_installed,
                paths_written: Vec::new(),
                preview: Some(preview),
            });
        }

        let mut paths_written = Vec::new();
        if changed {
            let serialized = aider_conf::serialize(&doc).map_err(|e| conf_error(&conf_path, e))?;
            ensure_parent(&conf_path)?;
            let mode = current_mode(&conf_path).unwrap_or(0o644);
            atomic_write(&conf_path, serialized.as_bytes(), mode)?;
            paths_written.push(conf_path.clone());
        }

        Ok(InstallReport {
            agent_id: Self::AGENT_ID.to_string(),
            hook_path: conf_path.clone(),
            settings_path: conf_path,
            already_installed,
            paths_written,
            preview: None,
        })
    }

    fn uninstall(&self, repo_root: &Path, dry_run: bool) -> Result<Vec<PathBuf>, InstallError> {
        let conf_path = self.settings_path(repo_root);
        if !conf_path.exists() {
            return Ok(Vec::new());
        }

        let src = fs::read_to_string(&conf_path).map_err(|e| InstallError::Io {
            path: conf_path.clone(),
            source: e,
        })?;
        let mut doc = aider_conf::parse(&src).map_err(|e| conf_error(&conf_path, e))?;
        let changed =
            aider_conf::uninstall_from_doc(&mut doc).map_err(|e| conf_error(&conf_path, e))?;

        if !changed {
            return Ok(Vec::new());
        }

        if !dry_run {
            let serialized = aider_conf::serialize(&doc).map_err(|e| conf_error(&conf_path, e))?;
            let mode = current_mode(&conf_path).unwrap_or(0o644);
            atomic_write(&conf_path, serialized.as_bytes(), mode)?;
        }

        Ok(vec![conf_path])
    }
}

fn conf_error(path: &Path, e: aider_conf::AiderConfError) -> InstallError {
    InstallError::Surface {
        agent_id: AiderSurface::AGENT_ID.to_string(),
        message: format!("{}: {e}", path.display()),
    }
}
