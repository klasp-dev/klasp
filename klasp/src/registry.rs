//! `SurfaceRegistry` — owns the set of `AgentSurface` impls the CLI dispatches to.
//!
//! Lives in the binary crate, not in `klasp-core`, because the registry is
//! the place where built-in agent crates are pulled together. v0.3 plugins
//! will register additional surfaces here at startup. See [docs/design.md §5].

use klasp_agents_claude::ClaudeCodeSurface;
use klasp_core::AgentSurface;

pub struct SurfaceRegistry {
    surfaces: Vec<Box<dyn AgentSurface>>,
}

impl Default for SurfaceRegistry {
    fn default() -> Self {
        Self {
            surfaces: vec![Box::new(ClaudeCodeSurface)],
        }
    }
}

impl SurfaceRegistry {
    pub fn iter(&self) -> impl Iterator<Item = &dyn AgentSurface> {
        self.surfaces.iter().map(|s| s.as_ref())
    }
}
