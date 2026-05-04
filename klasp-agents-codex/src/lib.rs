//! `klasp-agents-codex` — Codex `AgentSurface` impl.
//!
//! v0.2 W1 scope: crate skeleton + the AGENTS.md managed-block writer.
//! W2 (#28) adds the git pre-commit / pre-push writer; W3 (#29) wires
//! `klasp install --agent codex` through the CLI.

pub mod agents_md;
pub mod surface;

pub use agents_md::{
    contains_block, install_block, render_managed_block, uninstall_block, AgentsMdError,
    DEFAULT_BLOCK_BODY, MANAGED_END, MANAGED_START,
};
pub use surface::CodexSurface;
