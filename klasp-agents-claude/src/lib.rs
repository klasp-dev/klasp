//! `klasp-agents-claude` — Claude Code `AgentSurface` impl.
//!
//! See [docs/design.md] §3.1 (trait), §5 (install flow), §7 (hook script).

pub mod conflict;
pub mod hook_template;
pub mod settings;
pub mod surface;

pub use conflict::{conflict_message, detect_conflicts, HookConflict};
pub use hook_template::{render as render_hook_script, MANAGED_MARKER};
pub use settings::{merge_hook_entry, unmerge_hook_entry, SettingsError};
pub use surface::ClaudeCodeSurface;
