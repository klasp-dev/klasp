//! `klasp uninstall` — remove klasp's gate hook from every agent surface.
//!
//! Symmetric counterpart to `install` (see [docs/design.md §5]). Honours
//! `--agent` to scope removal, `--dry-run` to preview, and never touches
//! sibling tools' hooks (those are filtered out at the
//! `klasp_agents_claude::settings::unmerge_hook_entry` layer).

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use klasp_core::AgentSurface;

use crate::cli::UninstallArgs;
use crate::cmd::install::resolve_repo_root;
use crate::registry::SurfaceRegistry;

pub fn run(args: &UninstallArgs) -> ExitCode {
    match try_run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("klasp uninstall: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn try_run(args: &UninstallArgs) -> Result<()> {
    let repo_root = resolve_repo_root(args.repo_root.as_deref())?;

    let registry = SurfaceRegistry::default();
    let agent_filter = args.agent.as_deref();
    let surfaces: Vec<&dyn AgentSurface> = registry
        .iter()
        .filter(|s| agent_filter.map_or(true, |a| s.agent_id() == a))
        .collect();

    if surfaces.is_empty() {
        return Err(anyhow!(
            "no matching agent surfaces (filter: {:?})",
            agent_filter,
        ));
    }

    for s in &surfaces {
        let touched = s
            .uninstall(&repo_root, args.dry_run)
            .with_context(|| format!("uninstalling {}", s.agent_id()))?;
        report(s.agent_id(), &touched, args.dry_run);
    }

    Ok(())
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
