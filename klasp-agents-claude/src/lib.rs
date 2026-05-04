//! `klasp-agents-claude` — Claude Code `AgentSurface` impl.
//!
//! W1 ships only the crate skeleton; the real implementation (settings.json
//! merge, hook script template, idempotency checks) lands in W2 per
//! [docs/roadmap.md] §"Timeline". The empty struct exists so the workspace
//! compiles and the `klasp` binary can take this crate as a dependency
//! today.

/// Claude Code agent surface. The `AgentSurface` impl lands in W2 — see
/// [docs/design.md] §3.1 and §5 for the contract this struct must satisfy.
pub struct ClaudeCodeSurface;

impl ClaudeCodeSurface {
    pub const AGENT_ID: &'static str = "claude_code";
}
