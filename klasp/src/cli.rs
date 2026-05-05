use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::cmd;

#[derive(Debug, Parser)]
#[command(
    name = "klasp",
    version,
    about = "Block AI coding agents on the same quality gates your humans hit.",
    long_about = None,
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Scaffold a klasp.toml in the current repo.
    Init(InitArgs),
    /// Install klasp's gate hook into the detected agent surfaces.
    Install(InstallArgs),
    /// Remove klasp's gate hook from the detected agent surfaces.
    Uninstall(UninstallArgs),
    /// Gate runtime — invoked by the generated hook script with tool-call JSON on stdin.
    Gate(GateArgs),
    /// Diagnose the local install (config, hook script, schema version).
    Doctor(DoctorArgs),
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Overwrite an existing klasp.toml without prompting.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Restrict installation to one agent (`claude_code`, `codex`) or pass
    /// `all` to install every surface listed in `klasp.toml`'s `[gate].agents`.
    /// Omit to install all auto-detected surfaces.
    #[arg(long)]
    pub agent: Option<String>,
    /// Print what would be written without touching the filesystem.
    #[arg(long)]
    pub dry_run: bool,
    /// Install even when the agent surface is not auto-detected, or overwrite
    /// a non-managed hook file.
    #[arg(long)]
    pub force: bool,
    /// Override the repo root (defaults to `git rev-parse --show-toplevel`).
    #[arg(long)]
    pub repo_root: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct UninstallArgs {
    /// Restrict removal to one agent (`claude_code`, `codex`) or pass
    /// `all` to uninstall every surface listed in `klasp.toml`'s
    /// `[gate].agents`. Omit to walk every registered surface.
    #[arg(long)]
    pub agent: Option<String>,
    /// Print what would be removed without touching the filesystem.
    #[arg(long)]
    pub dry_run: bool,
    /// Override the repo root.
    #[arg(long)]
    pub repo_root: Option<PathBuf>,
}

/// Output format for `klasp gate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum OutputFormat {
    /// Human-readable terminal text written to stderr (default).
    #[default]
    Terminal,
    /// JUnit XML (Surefire schema) written to stdout or `--output`.
    Junit,
    /// SARIF 2.1.0 JSON written to stdout or `--output`.
    Sarif,
}

#[derive(Debug, Args)]
pub struct GateArgs {
    /// Output format. Default is human-readable terminal text.
    #[arg(long, value_enum, default_value_t = OutputFormat::Terminal)]
    pub format: OutputFormat,

    /// Write formatter output to this path. Defaults to stdout for `junit`/`sarif`,
    /// stderr for `terminal`.
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct DoctorArgs {}

pub fn run() -> ExitCode {
    let cli = Cli::parse();
    match &cli.command {
        Cmd::Init(args) => cmd::init::run(args),
        Cmd::Install(args) => cmd::install::run(args),
        Cmd::Uninstall(args) => cmd::uninstall::run(args),
        Cmd::Gate(args) => cmd::gate::run(args),
        Cmd::Doctor(args) => cmd::doctor::run(args),
    }
}
