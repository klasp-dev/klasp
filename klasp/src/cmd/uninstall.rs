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

    // Uninstall is the safety-net: when the user says `--agent all`,
    // walk every REGISTERED surface regardless of `klasp.toml`'s
    // `[gate].agents`. If a user installed `["claude_code", "codex"]`,
    // edited their config to drop one, then ran `uninstall --agent all`,
    // the dropped surface's hook scripts and managed blocks would
    // otherwise be orphaned. Single-agent (`--agent codex`) and the
    // omitted case still flow through `resolve_selection` — only the
    // wildcard branch diverges from install.
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
