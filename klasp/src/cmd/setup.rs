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

use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use klasp_core::{ConfigV1, InstallContext, GATE_SCHEMA_VERSION};

use crate::adopt::detect_agents::detect_installed_agents;
use crate::cli::SetupArgs;
use crate::cmd::doctor::{check_paths, check_surfaces, Counters};
use crate::cmd::install::{install_one_surface, print_hook_warning};
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

    // Step 3: print the gate detection plan.
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
        println!("[gate].agents would be: {}", detected_agents.join(", "));
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
        Some(&detected_agents),
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

    // Load config once after write; reuse for both install and doctor so
    // ConfigV1::load is called exactly once despite two downstream consumers.
    let config = ConfigV1::load(&repo_root).context("loading config after write")?;

    // Step 5: install all declared surfaces.
    let install_exit = run_install_all(&repo_root, &detected_agents)?;
    if install_exit != ExitCode::SUCCESS {
        eprintln!("klasp setup: install step failed — see above");
        return Ok(install_exit);
    }

    // Step 6: run doctor and report, reusing the already-loaded config.
    println!("\n--- klasp doctor ---");
    let doctor_exit = run_doctor_with_config(&repo_root, config);

    if doctor_exit == ExitCode::SUCCESS {
        println!("\nsetup complete — `klasp doctor` passed with no FAILs.");
    } else {
        println!("\nsetup finished with doctor failures — see above for details.");
    }

    Ok(doctor_exit)
}

/// Run install for each agent in `agents`. Loops once, collecting reports and
/// warnings via the shared `install_one_surface` helper (no duplicated Codex
/// dispatch). Returns `ExitCode::SUCCESS` unless a hard error occurs.
fn run_install_all(repo_root: &Path, agents: &[String]) -> Result<ExitCode> {
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

        let (report, warnings) =
            install_one_surface(surface, &ctx).with_context(|| format!("installing {agent_id}"))?;
        for warning in &warnings {
            print_hook_warning(warning);
        }
        println!("{}: {}", agent_id, install_result_label(&report));
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

/// Run the doctor checks using an already-parsed `ConfigV1`, avoiding a second
/// `ConfigV1::load` call. The config check step (`check_config`) is bypassed
/// because setup just wrote and validated the file; we print its OK line here.
fn run_doctor_with_config(repo_root: &Path, config: ConfigV1) -> ExitCode {
    let mut c = Counters::new();
    // Config was just written and validated by setup — emit the OK line directly.
    c.ok("config: klasp.toml loaded OK");
    check_surfaces(repo_root, Some(&config), &mut c);
    check_paths(&config, &mut c);

    if c.fails > 0 || c.warns > 0 {
        eprintln!("doctor: {} FAIL, {} WARN", c.fails, c.warns);
    } else {
        eprintln!("doctor: all checks passed");
    }

    if c.fails > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
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
