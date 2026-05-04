//! `AgentSurface` — abstraction over an AI-agent integration surface.
//!
//! Design: [docs/design.md §3.1, §5]. Each agent (Claude Code, Codex,
//! Cursor, Aider) has a structurally different install path: Claude merges
//! into a JSON file, Codex writes a managed-block markdown into AGENTS.md,
//! Cursor writes `.cursor/rules/*.mdc`, Aider edits `.aider.conf.yml`. A
//! trait — not an enum + match — keeps each impl free to share no state
//! with the others, and lets v0.3 plugins ship third-party `AgentSurface`
//! implementations without forking klasp.

use std::path::{Path, PathBuf};

/// Inputs handed to every `install` invocation. Holds enough context that
/// an `AgentSurface` impl needs no further filesystem probing.
#[derive(Debug, Clone)]
pub struct InstallContext {
    pub repo_root: PathBuf,
    pub dry_run: bool,
    pub force: bool,
    /// The wire-protocol schema version the generated hook script should
    /// export. Sourced from [`crate::protocol::GATE_SCHEMA_VERSION`] at the
    /// caller; passed in here to keep the trait pure.
    pub schema_version: u32,
}

/// What an install (or dry-run) actually did. The `paths_written` field is
/// empty when `dry_run` was true.
#[derive(Debug, Clone)]
pub struct InstallReport {
    pub agent_id: String,
    pub hook_path: PathBuf,
    pub settings_path: PathBuf,
    pub already_installed: bool,
    pub paths_written: Vec<PathBuf>,
    /// In dry-run mode, the rendered hook-script content for preview.
    pub preview: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "{path} exists but does not contain klasp's managed marker. \
         Re-run with --force to overwrite, or remove the file manually."
    )]
    MarkerConflict { path: PathBuf },

    #[error("could not parse {path} as JSON: {source}")]
    SettingsParse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("agent surface `{agent_id}` reports: {message}")]
    Surface { agent_id: String, message: String },
}

/// Object-safe trait. The surface registry stores impls as
/// `Box<dyn AgentSurface>`; built-in surfaces (Claude in v0.1, Codex in
/// v0.2, etc.) are registered statically, and v0.3 subprocess plugins add
/// dynamic registrations at startup.
pub trait AgentSurface: Send + Sync {
    /// Stable agent identifier (e.g. `"claude_code"`, `"codex"`).
    fn agent_id(&self) -> &'static str;

    /// Auto-detect whether this surface is relevant to the given repo
    /// (e.g. presence of `.claude/` for Claude Code, `AGENTS.md` for Codex).
    /// `klasp install --force` overrides a `false` here.
    fn detect(&self, repo_root: &Path) -> bool;

    /// Perform the install. Must be idempotent: running twice with the same
    /// input yields the same on-disk state and returns
    /// `already_installed = true` on the second run.
    fn install(&self, ctx: &InstallContext) -> Result<InstallReport, InstallError>;

    /// Remove klasp's managed entries. Returns the list of paths that were
    /// (or would be, in `dry_run`) modified. Sibling hooks must be
    /// preserved.
    fn uninstall(&self, repo_root: &Path, dry_run: bool) -> Result<Vec<PathBuf>, InstallError>;

    /// Render the hook-script content this surface would write. Pure —
    /// no filesystem access. Used by `install` and by `--dry-run`.
    fn render_hook_script(&self, ctx: &InstallContext) -> String;

    /// Path to the hook-script file this surface owns.
    fn hook_path(&self, repo_root: &Path) -> PathBuf;

    /// Path to the agent's settings/config file this surface mutates.
    fn settings_path(&self, repo_root: &Path) -> PathBuf;
}
