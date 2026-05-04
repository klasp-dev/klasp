//! `klasp-agents-codex` — Codex `AgentSurface` impl.
//!
//! v0.2 W1 shipped the AGENTS.md managed-block writer. W2 (#28, this
//! module set) layers the git `pre-commit` / `pre-push` hook writer on
//! top, with conflict detection for husky / lefthook / pre-commit
//! framework. W3 (#29) wires `klasp install --agent codex` through the
//! CLI and surfaces the [`git_hooks::HookWarning`]s in the install
//! output.

pub mod agents_md;
pub mod git_hooks;
pub mod surface;

// Crate-root re-exports below mirror the W1 shape: AGENTS.md helpers
// keep their unprefixed names (the v0.2 W1 PR exported them at the
// crate root, and downstream callers — including W3 — depend on that).
// The git-hook helpers, which name-clash on `MANAGED_START` /
// `MANAGED_END` / `install_block` / `uninstall_block`, are exposed via
// the `git_hooks` submodule path only — `git_hooks::install_block(…)`,
// not a root-level `install_hook_block`. Keeping the file-format choice
// in the import path makes mis-routing (e.g. shell helpers applied to
// markdown) impossible at the type level.
pub use agents_md::{
    contains_block, install_block, render_managed_block, uninstall_block, AgentsMdError,
    DEFAULT_BLOCK_BODY, MANAGED_END, MANAGED_START,
};
pub use git_hooks::{HookConflict, HookError, HookKind, HookWarning};
pub use surface::{CodexInstallReport, CodexSurface};
