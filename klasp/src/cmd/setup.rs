//! `klasp setup` — one-command first-run orchestrator.
//!
//! Runs the full detect → narrow → write → install → doctor sequence in
//! order, with sensible defaults at every step. Eliminates the manual
//! 3-command flow for new users.
//!
//! ```text
//! klasp setup                  # non-interactive: detect, write, install, doctor
//! klasp setup --interactive    # y/n prompts before write + install
//! klasp setup --dry-run        # print plan only, write nothing
//! ```
//!
//! The three individual commands (`init`, `install`, `doctor`) remain fully
//! supported and unchanged for scriptable/CI use. `setup` is additive sugar.
//!
//! See klasp-dev/klasp#103.

use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use klasp_core::{AgentSurface, ConfigV1, InstallContext, KlaspError, GATE_SCHEMA_VERSION};
use serde_json::Value;

use crate::adopt::detect_agents::detect_installed_agents;
use crate::cli::SetupArgs;
use crate::registry::SurfaceRegistry;

/// Entry point for `klasp setup`.
pub fn run(args: &SetupArgs) -> ExitCode {
    match try_run(args) {
        Ok(exit) => exit,
        Err(e) => {
            eprintln!("klasp setup: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn try_run(args: &SetupArgs) -> Result<ExitCode> {
    let repo_root = crate::cmd::install::resolve_repo_root(None).context("resolving repo root")?;

    // Step 1: detect existing gates.
    let plan = crate::adopt::detect::detect_all(&repo_root).context("detecting existing gates")?;

    println!("klasp setup — detected {} gate(s)", plan.findings.len());
    if args.dry_run {
        println!("(--dry-run: printing plan only, writing nothing)");
    }

    // Step 2: detect installed agents on this machine.
    let home = crate::fs_util::home_dir();
    let detected_agents = detect_installed_agents(home.as_deref());
    println!(
        "detected agents: {}",
        if detected_agents.is_empty() {
            "(none)".to_string()
        } else {
            detected_agents.join(", ")
        }
    );

    // Step 3: compute intersection — [gate].agents = detected agents only.
    // (detect_installed_agents already returns only what is installed.)
    let agents_to_write = &detected_agents;

    // Step 4: print the gate detection plan.
    print!("{}", crate::adopt::render::render_plan(&plan));

    // Interactive: gate selection prompt.
    let gates_to_use = if args.interactive {
        let confirmed = prompt_yes_no(&format!(
            "Mirror {} detected gate(s) into klasp.toml?",
            plan.findings.len()
        ))?;
        if !confirmed {
            println!("Skipping gate mirroring — klasp.toml will have no checks.");
            // Use an empty plan to still write a valid klasp.toml with agent narrowing.
            crate::adopt::plan::AdoptionPlan::default()
        } else {
            plan
        }
    } else {
        plan
    };

    // Dry-run: show plan and exit.
    if args.dry_run {
        println!("\n--- dry-run plan ---");
        println!("[gate].agents would be: {}", agents_to_write.join(", "));
        println!(
            "checks: {}",
            gates_to_use
                .findings
                .iter()
                .flat_map(|f| f.proposed_checks.iter().map(|c| c.name.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
                .if_empty("(none)")
        );
        println!("No files written (--dry-run).");
        return Ok(ExitCode::SUCCESS);
    }

    // Interactive: confirm before write.
    if args.interactive {
        let confirmed = prompt_yes_no("Write klasp.toml now?")?;
        if !confirmed {
            println!("Aborted — klasp.toml not written.");
            return Ok(ExitCode::SUCCESS);
        }
    }

    // Step 4: write klasp.toml (force=true so setup is re-runnable).
    let toml_path = crate::adopt::writer::write_klasp_toml(
        &repo_root,
        &gates_to_use,
        true, // force: setup is idempotent by design
        Some(agents_to_write),
    )
    .context("writing klasp.toml")?;
    println!("wrote {}", toml_path.display());

    // Interactive: confirm before install.
    if args.interactive {
        let confirmed = prompt_yes_no("Install agent surfaces now?")?;
        if !confirmed {
            println!("Skipping install. Run `klasp install --agent all` when ready.");
            return Ok(ExitCode::SUCCESS);
        }
    }

    // Step 5: install all declared surfaces.
    let install_exit = run_install_all(&repo_root, &detected_agents)?;
    if install_exit != ExitCode::SUCCESS {
        eprintln!("klasp setup: install step failed — see above");
        return Ok(install_exit);
    }

    // Step 6: run doctor and report.
    println!("\n--- klasp doctor ---");
    let doctor_exit = run_doctor_inline(&repo_root);

    if doctor_exit == ExitCode::SUCCESS {
        println!("\nsetup complete — `klasp doctor` passed with no FAILs.");
    } else {
        println!("\nsetup finished with doctor failures — see above for details.");
    }

    Ok(doctor_exit)
}

/// Run install for each agent in `agents`. Logs each install and continues
/// even if individual surfaces produce warnings (matching `klasp install`
/// behaviour). Returns `ExitCode::SUCCESS` unless a hard error occurs.
fn run_install_all(repo_root: &Path, agents: &[String]) -> Result<ExitCode> {
    use klasp_agents_codex::CodexSurface;

    let registry = SurfaceRegistry::default();
    let ctx = InstallContext {
        repo_root: repo_root.to_path_buf(),
        dry_run: false,
        force: false,
        schema_version: GATE_SCHEMA_VERSION,
    };

    for agent_id in agents {
        let surface = match registry.get(agent_id) {
            Some(s) => s,
            None => {
                eprintln!("warning: unknown agent '{agent_id}' in detected list — skipping");
                continue;
            }
        };

        if surface.agent_id() == CodexSurface::AGENT_ID {
            let detailed = CodexSurface
                .install_detailed(&ctx)
                .with_context(|| format!("installing {agent_id}"))?;
            for warning in &detailed.warnings {
                print_hook_warning(warning);
            }
            println!("{}: {}", agent_id, install_result_label(&detailed.report));
        } else {
            let report = surface
                .install(&ctx)
                .with_context(|| format!("installing {agent_id}"))?;
            println!("{}: {}", agent_id, install_result_label(&report));
        }
    }

    Ok(ExitCode::SUCCESS)
}

fn install_result_label(report: &klasp_core::InstallReport) -> &'static str {
    if report.already_installed {
        "already installed (no changes)"
    } else {
        "installed"
    }
}

fn print_hook_warning(warning: &HookWarning) {
    use klasp_agents_codex::HookKind;
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
            eprintln!(
                "warning: skipping {hook_label} hook ({}) — owned by {}.",
                path.display(),
                conflict.tool()
            );
        }
    }
}

/// Run doctor checks inline and return exit code. This mirrors the logic in
/// `cmd/doctor.rs` but prints its own section header so the output is clearly
/// scoped to setup.
fn run_doctor_inline(repo_root: &Path) -> ExitCode {
    use klasp_core::{CheckSourceConfig, GATE_SCHEMA_VERSION};

    let registry = SurfaceRegistry::default();
    let mut fails = 0usize;
    let mut warns = 0usize;

    // Config check.
    let config = match ConfigV1::load(repo_root) {
        Ok(cfg) => {
            println!("OK    config: klasp.toml loaded OK");
            Some(cfg)
        }
        Err(KlaspError::ConfigNotFound { searched }) => {
            let paths: Vec<_> = searched.iter().map(|p| p.display().to_string()).collect();
            println!(
                "FAIL  config: klasp.toml not found (searched: {})",
                paths.join(", ")
            );
            fails += 1;
            None
        }
        Err(e) => {
            println!("FAIL  config: {e}");
            fails += 1;
            None
        }
    };

    // Surface checks.
    if let Some(ref cfg) = config {
        let declared = &cfg.gate.agents;
        for surface in registry.iter() {
            let agent_id = surface.agent_id();
            if !declared.iter().any(|a| a == agent_id) {
                println!("INFO  {agent_id}: not in [gate].agents, skipping");
                continue;
            }

            let hook_path = surface.hook_path(repo_root);
            match std::fs::read_to_string(&hook_path) {
                Ok(actual) => {
                    let ctx = InstallContext {
                        repo_root: repo_root.to_path_buf(),
                        dry_run: false,
                        force: false,
                        schema_version: GATE_SCHEMA_VERSION,
                    };
                    if actual == surface.render_hook_script(&ctx) {
                        println!("OK    hook[{agent_id}]: current (schema v{GATE_SCHEMA_VERSION})");
                    } else {
                        println!("FAIL  hook[{agent_id}]: schema drift (re-run `klasp install`)");
                        fails += 1;
                    }
                }
                Err(_) => {
                    println!(
                        "FAIL  hook[{agent_id}]: {} not found; re-run `klasp install`",
                        hook_path.display()
                    );
                    fails += 1;
                }
            }

            // Settings check (Claude Code only).
            if agent_id == klasp_agents_claude::ClaudeCodeSurface::AGENT_ID {
                check_claude_settings(repo_root, surface, agent_id, &mut fails);
            }
        }
    }

    // PATH checks.
    if let Some(ref cfg) = config {
        for check in &cfg.checks {
            if let CheckSourceConfig::Shell { command } = &check.source {
                if let Some(argv0) = extract_argv0(command) {
                    match which::which(argv0) {
                        Ok(_) => {
                            println!("OK    path[{}]: `{argv0}` found in PATH", check.name)
                        }
                        Err(_) => {
                            println!("WARN  path[{}]: `{argv0}` not found in PATH", check.name);
                            warns += 1;
                        }
                    }
                }
            }
        }
    }

    if fails > 0 || warns > 0 {
        eprintln!("doctor: {fails} FAIL, {warns} WARN");
    } else {
        eprintln!("doctor: all checks passed");
    }

    if fails > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn check_claude_settings(
    repo_root: &Path,
    surface: &dyn AgentSurface,
    agent_id: &str,
    fails: &mut usize,
) {
    let settings_path = surface.settings_path(repo_root);
    let raw = match std::fs::read_to_string(&settings_path) {
        Ok(s) => s,
        Err(_) => {
            println!(
                "FAIL  settings[{agent_id}]: {} not found; re-run `klasp install`",
                settings_path.display()
            );
            *fails += 1;
            return;
        }
    };

    let root: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            println!("FAIL  settings[{agent_id}]: parse error: {e}");
            *fails += 1;
            return;
        }
    };

    let hook_command = klasp_agents_claude::ClaudeCodeSurface::HOOK_COMMAND;
    let has_entry = root
        .get("hooks")
        .and_then(|h| h.get("PreToolUse"))
        .and_then(Value::as_array)
        .is_some_and(|arr| {
            arr.iter().any(|entry| {
                entry.get("matcher").and_then(Value::as_str) == Some("Bash")
                    && entry
                        .get("hooks")
                        .and_then(Value::as_array)
                        .is_some_and(|inner| {
                            inner.iter().any(|h| {
                                h.get("command").and_then(Value::as_str) == Some(hook_command)
                            })
                        })
            })
        });

    if has_entry {
        println!("OK    settings[{agent_id}]: hook entry present");
    } else {
        println!("FAIL  settings[{agent_id}]: hook entry missing; re-run `klasp install`");
        *fails += 1;
    }
}

/// Extract the first non-`KEY=VALUE` token from a shell command.
fn extract_argv0(command: &str) -> Option<&str> {
    command
        .split_ascii_whitespace()
        .find(|token| !token.contains('='))
}

/// Prompt the user with a yes/no question. Returns `true` for "y"/"yes",
/// `false` for "n"/"no". Repeats on unrecognised input.
fn prompt_yes_no(question: &str) -> io::Result<bool> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        print!("{question} [y/N] ");
        stdout.flush()?;

        let mut line = String::new();
        stdin.read_line(&mut line)?;

        match line.trim().to_lowercase().as_str() {
            "y" | "yes" => return Ok(true),
            "n" | "no" | "" => return Ok(false),
            _ => println!("Please enter y or n."),
        }
    }
}

/// Trait extension for empty-string handling in dry-run output.
trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

// Re-export HookWarning for the print helper above.
use klasp_agents_codex::HookWarning;
