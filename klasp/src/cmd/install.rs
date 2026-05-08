//! `klasp install` — discover agent surfaces and install klasp's gate hook.
//!
//! Implementation follows [docs/design.md §5]: build a [`SurfaceRegistry`],
//! resolve the user's `--agent` choice (single agent / `all` / omitted) into
//! the set of surfaces to drive, dispatch to each surface's `install()`, and
//! render reports + non-fatal warnings.
//!
//! ## Selection rules
//!
//! - `--agent <name>` — install exactly that surface. Unknown name → hard
//!   error with the list of supported agents.
//! - `--agent all` — read `klasp.toml`, intersect `[gate].agents` with the
//!   registry. Unknown entries in the config fail loudly. An empty
//!   `[gate].agents = []` array is a no-op + warning, not an error
//!   (acceptance #4 of issue #29).
//! - omitted — fall back to the v0.1 behaviour: every registered surface
//!   that auto-detects (or all of them under `--force`).
//!
//! ## Codex warnings
//!
//! [`klasp_agents_codex::CodexSurface`] returns
//! [`HookWarning::Skipped`](klasp_agents_codex::HookWarning) when a foreign
//! hook manager (husky / lefthook / pre-commit framework) owns the
//! `.git/hooks/pre-commit` (or `pre-push`) file. We render those to stderr
//! as a non-fatal `warning:` line per acceptance #2 of issue #28; the
//! install completes successfully.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use klasp_agents_codex::{CodexSurface, HookKind, HookWarning};
use klasp_core::{
    AgentSurface, ConfigV1, InstallContext, InstallReport, KlaspError, GATE_SCHEMA_VERSION,
};

use crate::cli::InstallArgs;
use crate::registry::SurfaceRegistry;

/// Special value of `--agent` that fans installation out across every
/// surface declared in `klasp.toml`'s `[gate].agents` array.
pub const AGENT_ALL: &str = "all";

pub fn run(args: &InstallArgs) -> ExitCode {
    match try_run(args) {
        Ok(exit) => exit,
        Err(e) => {
            eprintln!("klasp install: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn try_run(args: &InstallArgs) -> Result<ExitCode> {
    let repo_root = resolve_repo_root(args.repo_root.as_deref())?;
    let registry = SurfaceRegistry::default();

    let selection = resolve_selection(args.agent.as_deref(), &registry, &repo_root)?;
    let surfaces = match selection {
        Selection::Empty { reason } => {
            eprintln!("warning: {reason}; nothing to install");
            return Ok(ExitCode::SUCCESS);
        }
        Selection::Surfaces(s) => s,
    };

    // Warn when the user installs a single specific agent but klasp.toml lists
    // additional agents that this install will not cover. Doctor would FAIL for
    // those uncovered agents if left uninstalled. The install itself succeeds.
    if let Some(agent_name) = args.agent.as_deref() {
        if agent_name != AGENT_ALL {
            warn_if_narrower_than_config(agent_name, &repo_root, &registry);
        }
    }

    // Auto-detection is only meaningful when the user did NOT name a
    // specific selection. When `--agent <name>` (or `--agent all`) is
    // explicit, the user has told us exactly which surfaces to drive;
    // a missing-AGENTS.md or missing-settings.json is a bootstrap case,
    // not a "skip this surface" signal. Filter only the no-arg path.
    let surfaces = if args.agent.is_some() {
        surfaces
    } else {
        filter_by_detect(surfaces, &repo_root, args.force)
    };
    if surfaces.is_empty() {
        return Err(anyhow!(
            "no agent surfaces auto-detected at {}; pass --force or --agent <name>",
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
        let (report, warnings) = install_one_surface(*s, &ctx)?;
        for warning in &warnings {
            print_hook_warning(warning);
        }
        reports.push(report);
    }

    print_reports(&reports, args.dry_run);
    Ok(ExitCode::SUCCESS)
}

/// Resolved agent-selection state for one invocation of `klasp install`
/// or `klasp uninstall`.
pub(crate) enum Selection<'a> {
    /// At least one surface to act on.
    Surfaces(Vec<&'a dyn AgentSurface>),
    /// Nothing to do — `[gate].agents = []` under `--agent all`. Render
    /// the carried `reason` as a `warning:` line and exit 0.
    Empty { reason: String },
}

/// Resolve `--agent` into the list of surfaces the CLI must drive.
///
/// Errors map onto user-facing scenarios:
///
/// - unknown agent name → `"unknown agent ..."` listing supported names
/// - `[gate].agents` entries the registry doesn't recognise → same shape
/// - `[gate].agents` missing config when `--agent all` was requested →
///   the underlying [`KlaspError::ConfigNotFound`]
pub(crate) fn resolve_selection<'a>(
    requested: Option<&str>,
    registry: &'a SurfaceRegistry,
    repo_root: &Path,
) -> Result<Selection<'a>> {
    match requested {
        None => Ok(Selection::Surfaces(registry.iter().collect())),
        Some(name) if name == AGENT_ALL => resolve_all(registry, repo_root),
        Some(name) => match registry.get(name) {
            Some(s) => Ok(Selection::Surfaces(vec![s])),
            None => Err(unknown_agent(name, registry)),
        },
    }
}

fn resolve_all<'a>(registry: &'a SurfaceRegistry, repo_root: &Path) -> Result<Selection<'a>> {
    let config = ConfigV1::load(repo_root).map_err(map_config_err)?;

    if config.gate.agents.is_empty() {
        return Ok(Selection::Empty {
            reason: "`[gate].agents = []` in klasp.toml".to_string(),
        });
    }

    let mut surfaces = Vec::with_capacity(config.gate.agents.len());
    for name in &config.gate.agents {
        match registry.get(name) {
            Some(s) => surfaces.push(s),
            None => return Err(unknown_agent(name, registry)),
        }
    }
    Ok(Selection::Surfaces(surfaces))
}

/// Translate a [`KlaspError`] from `ConfigV1::load` into an `anyhow::Error`
/// with a top-level message that reads naturally after `klasp install: `.
fn map_config_err(e: KlaspError) -> anyhow::Error {
    match e {
        KlaspError::ConfigNotFound { searched } => {
            let paths: Vec<String> = searched.iter().map(|p| p.display().to_string()).collect();
            anyhow!(
                "--agent all requires klasp.toml; not found (searched: {})",
                paths.join(", ")
            )
        }
        other => anyhow!(other),
    }
}

fn unknown_agent(name: &str, registry: &SurfaceRegistry) -> anyhow::Error {
    let supported = registry.agent_ids().join(", ");
    anyhow!("unknown agent \"{name}\"; supported: {supported} (or \"all\" to fan out across [gate].agents)")
}

/// Apply auto-detection unless the user passed `--force`. `--force` keeps
/// every surface in the selection so the user can bootstrap a missing
/// surface from scratch.
fn filter_by_detect<'a>(
    surfaces: Vec<&'a dyn AgentSurface>,
    repo_root: &Path,
    force: bool,
) -> Vec<&'a dyn AgentSurface> {
    if force {
        return surfaces;
    }
    surfaces
        .into_iter()
        .filter(|s| s.detect(repo_root))
        .collect()
}

/// Emit a stderr WARN when `--agent <name>` installs a single surface but
/// `klasp.toml`'s `[gate].agents` declares additional agents that will remain
/// without a gate hook after this install. The install itself still succeeds.
///
/// Silently skips when: klasp.toml is absent, unparseable, or `[gate].agents`
/// is empty — those are handled by other error paths.
fn warn_if_narrower_than_config(installing: &str, repo_root: &Path, registry: &SurfaceRegistry) {
    let config = match ConfigV1::load(repo_root) {
        Ok(c) => c,
        Err(_) => return,
    };

    let uncovered: Vec<&str> = config
        .gate
        .agents
        .iter()
        .filter(|a| a.as_str() != installing)
        .filter(|a| registry.get(a.as_str()).is_some()) // only known agents
        .map(String::as_str)
        .collect();

    if !uncovered.is_empty() {
        eprintln!(
            "warning: klasp.toml lists agents {} that this install will NOT cover; \
             doctor will report them as missing. \
             Run `klasp install --agent all` to cover all declared agents.",
            uncovered.join(", ")
        );
    }
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

/// Install a single surface. For Codex, uses the detailed entry-point that
/// returns hook-conflict warnings. Returns `(report, warnings)`.
pub(crate) fn install_one_surface(
    surface: &dyn AgentSurface,
    ctx: &InstallContext,
) -> Result<(InstallReport, Vec<HookWarning>)> {
    if surface.agent_id() == CodexSurface::AGENT_ID {
        let detailed = CodexSurface
            .install_detailed(ctx)
            .with_context(|| format!("installing {}", surface.agent_id()))?;
        Ok((detailed.report, detailed.warnings))
    } else {
        let report = surface
            .install(ctx)
            .with_context(|| format!("installing {}", surface.agent_id()))?;
        Ok((report, vec![]))
    }
}

/// Render a non-fatal hook conflict to stderr. The format matches the
/// "actionable suggestion is mandatory" requirement from issue #29: tell
/// the user *exactly* what to do next.
pub(crate) fn print_hook_warning(warning: &HookWarning) {
    match warning {
        HookWarning::Skipped {
            path,
            kind,
            conflict,
        } => {
            let hook_label = match kind {
                HookKind::Commit => "pre-commit",
                HookKind::Push => "pre-push",
            };
            let trigger = kind.trigger_arg();
            let tool = conflict.tool();
            eprintln!(
                "warning: skipping {hook_label} hook ({}) — file is managed by {tool}.",
                path.display()
            );
            eprintln!(
                "         Install klasp's gate manually by adding `klasp gate \
                 --agent codex --trigger {trigger} \"$@\"`"
            );
            eprintln!(
                "         to your existing hook, or remove the foreign tool and \
                 re-run `klasp install --agent codex`."
            );
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
