//! `SurfaceRegistry` — owns the set of `AgentSurface` impls the CLI dispatches to.
//!
//! Lives in the binary crate, not in `klasp-core`, because the registry is
//! the place where built-in agent crates are pulled together. v0.3 plugins
//! will register additional surfaces here at startup. See [docs/design.md §5].
//!
//! v0.2 W3 adds [`klasp_agents_codex::CodexSurface`] alongside Claude Code.
//! The order here is the order callers iterate (and the order
//! `klasp install --agent all` walks): Claude first, Codex second. That
//! matches the canonical `[gate].agents = ["claude_code", "codex"]` shape
//! `klasp init` ships.

use klasp_agents_claude::ClaudeCodeSurface;
use klasp_agents_codex::CodexSurface;
use klasp_core::AgentSurface;

pub struct SurfaceRegistry {
    surfaces: Vec<Box<dyn AgentSurface>>,
}

impl Default for SurfaceRegistry {
    fn default() -> Self {
        Self {
            surfaces: vec![Box::new(ClaudeCodeSurface), Box::new(CodexSurface)],
        }
    }
}

impl SurfaceRegistry {
    pub fn iter(&self) -> impl Iterator<Item = &dyn AgentSurface> {
        self.surfaces.iter().map(|s| s.as_ref())
    }

    /// Stable list of agent IDs registered, in install order. Used by the
    /// CLI for help text and unknown-agent error messages.
    pub fn agent_ids(&self) -> Vec<&'static str> {
        self.surfaces.iter().map(|s| s.agent_id()).collect()
    }

    /// Look up a surface by its `agent_id()`. Returns `None` for unknown
    /// agents — callers turn that into a user-facing error.
    pub fn get(&self, agent_id: &str) -> Option<&dyn AgentSurface> {
        self.surfaces
            .iter()
            .map(|s| s.as_ref())
            .find(|s| s.agent_id() == agent_id)
    }
}
