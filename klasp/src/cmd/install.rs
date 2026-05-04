//! `klasp install` — discover agent surfaces and install klasp's gate hook.
//!
//! Implementation follows [docs/design.md §5] verbatim: build a
//! [`SurfaceRegistry`], filter by `--agent` and `detect()`, dispatch to each
//! surface's `install()`, render reports.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use klasp_core::{AgentSurface, InstallContext, InstallReport, GATE_SCHEMA_VERSION};

use crate::cli::InstallArgs;
use crate::registry::SurfaceRegistry;

pub fn run(args: &InstallArgs) -> ExitCode {
    match try_run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("klasp install: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn try_run(args: &InstallArgs) -> Result<()> {
    let repo_root = resolve_repo_root(args.repo_root.as_deref())?;

    let registry = SurfaceRegistry::default();
    let agent_filter = args.agent.as_deref();
    let surfaces: Vec<&dyn AgentSurface> = registry
        .iter()
        .filter(|s| agent_filter.map_or(true, |a| s.agent_id() == a))
        .filter(|s| args.force || s.detect(&repo_root))
        .collect();

    if surfaces.is_empty() {
        return Err(anyhow!(
            "no matching agent surfaces detected at {}; pass --force to install anyway",
            repo_root.display(),
        ));
    }

    let ctx = InstallContext {
        repo_root: repo_root.clone(),
        dry_run: args.dry_run,
        force: args.force,
        schema_version: GATE_SCHEMA_VERSION,
    };

    let mut reports = Vec::with_capacity(surfaces.len());
    for s in &surfaces {
        let report = s
            .install(&ctx)
            .with_context(|| format!("installing {}", s.agent_id()))?;
        reports.push(report);
    }

    print_reports(&reports, args.dry_run);
    Ok(())
}

fn print_reports(reports: &[InstallReport], dry_run: bool) {
    for r in reports {
        if r.already_installed {
            println!("{}: already installed (no changes)", r.agent_id);
            continue;
        }
        if dry_run {
            println!(
                "{}: would write {} and update {}",
                r.agent_id,
                r.hook_path.display(),
                r.settings_path.display(),
            );
            if let Some(preview) = &r.preview {
                println!("--- {} ---", r.hook_path.display());
                print!("{preview}");
            }
            continue;
        }
        println!("{}: installed", r.agent_id);
        for path in &r.paths_written {
            println!("  wrote {}", path.display());
        }
    }
}

pub(crate) fn resolve_repo_root(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    let cwd = std::env::current_dir().context("getting current directory")?;
    let mut probe = cwd.as_path();
    loop {
        if probe.join(".git").exists() {
            return Ok(probe.to_path_buf());
        }
        match probe.parent() {
            Some(parent) => probe = parent,
            None => {
                return Err(anyhow!(
                    "not a git repository (run from inside a repo, or pass --repo-root)"
                ));
            }
        }
    }
}
