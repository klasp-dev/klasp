//! Tiny git helpers used by the gate runtime.
//!
//! Just enough to answer "what's the working repo root?" without pulling
//! `git2` into the dependency closure. The gate path runs once per
//! tool-call, so a single `git rev-parse` subprocess is cheap; the failure
//! path falls back to `CLAUDE_PROJECT_DIR` so klasp gates work even outside
//! a git checkout (worktrees, sparse clones, the test harness).

use std::path::PathBuf;
use std::process::Command;

/// Resolve a working repo root from the current working directory.
///
/// Lookup order, mirroring [docs/design.md §6, §14]:
///
/// 1. `CLAUDE_PROJECT_DIR` — Claude Code sets this when invoking hooks, and
///    it's the strongest signal because it doesn't depend on git being
///    initialised.
/// 2. `git rev-parse --show-toplevel` from the current dir.
/// 3. The current dir itself, last-resort fallback.
///
/// Returns `None` only when every attempt fails (no env var, no git, no
/// readable cwd). The gate runtime treats `None` as a fail-open signal.
pub fn find_repo_root_from_cwd() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("CLAUDE_PROJECT_DIR") {
        let candidate = PathBuf::from(&dir);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }

    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
    {
        if output.status.success() {
            let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !raw.is_empty() {
                let candidate = PathBuf::from(raw);
                if candidate.is_dir() {
                    return Some(candidate);
                }
            }
        }
    }

    std::env::current_dir().ok()
}
