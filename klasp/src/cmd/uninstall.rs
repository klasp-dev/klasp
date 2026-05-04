//! `klasp uninstall` — remove klasp's gate hook from every agent surface.
//!
//! Symmetric counterpart to `install` (see [docs/design.md §5]). Shares the
//! `--agent` resolution rules with install via
//! [`crate::cmd::install::resolve_selection`]: single agent, `all` (driven
//! from `klasp.toml`'s `[gate].agents`), or omitted (every registered
//! surface). Sibling tools' hooks are filtered out at the surface level —
//! `klasp_agents_claude::settings::unmerge_hook_entry` and
//! `klasp_agents_codex::git_hooks::uninstall_block` both refuse to touch
//! foreign content.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use klasp_core::AgentSurface;

use crate::cli::UninstallArgs;
use crate::cmd::install::{resolve_repo_root, resolve_selection, Selection, AGENT_ALL};
use crate::registry::SurfaceRegistry;

pub fn run(args: &UninstallArgs) -> ExitCode {
    match try_run(args) {
        Ok(exit) => exit,
        Err(e) => {
            eprintln!("klasp uninstall: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn try_run(args: &UninstallArgs) -> Result<ExitCode> {
    let repo_root = resolve_repo_root(args.repo_root.as_deref())?;
    let registry = SurfaceRegistry::default();

    // Uninstall safety-net: `--agent all` walks every REGISTERED
    // surface, ignoring `klasp.toml`'s `[gate].agents`. Today this is
    // operationally equivalent to the no-`--agent` path (which also
    // returns `registry.iter().collect()` via `resolve_selection`),
    // but the explicit branch is forward-defensive: if a future change
    // ever makes the no-arg path config-driven, the wildcard branch
    // here still catches orphans (a surface the user once installed,
    // then dropped from `[gate].agents` before running uninstall).
    // Install can't carry the same orphan-safety contract because it
    // would surprise users by writing surfaces they didn't ask for.
    let selection = if args.agent.as_deref() == Some(AGENT_ALL) {
        Selection::Surfaces(registry.iter().collect())
    } else {
        resolve_selection(args.agent.as_deref(), &registry, &repo_root)?
    };
    let surfaces: Vec<&dyn AgentSurface> = match selection {
        Selection::Empty { reason } => {
            eprintln!("warning: {reason}; nothing to remove");
            return Ok(ExitCode::SUCCESS);
        }
        Selection::Surfaces(s) => s,
    };

    for s in &surfaces {
        let touched = s
            .uninstall(&repo_root, args.dry_run)
            .with_context(|| format!("uninstalling {}", s.agent_id()))?;
        report(s.agent_id(), &touched, args.dry_run);
    }

    Ok(ExitCode::SUCCESS)
}

fn report(agent_id: &str, paths: &[PathBuf], dry_run: bool) {
    if paths.is_empty() {
        println!("{agent_id}: nothing to remove");
        return;
    }
    let verb = if dry_run { "would touch" } else { "removed" };
    println!("{agent_id}: {verb}");
    for p in paths {
        println!("  {}", p.display());
    }
}
