use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::adopt::plan::AdoptMode;
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
    /// Manage klasp plugins.
    Plugins(PluginsArgs),
    /// One-command first-run setup: detect gates, narrow agents, write config, install, doctor.
    Setup(SetupArgs),
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Overwrite an existing klasp.toml without prompting.
    #[arg(long)]
    pub force: bool,

    /// Detect existing quality gates (pre-commit, Husky, Lefthook,
    /// lint-staged, plain `.git/hooks`) and propose a `klasp.toml`
    /// that mirrors them. Combine with `--mode` to control the
    /// destructiveness. See klasp-dev/klasp#97.
    #[arg(long)]
    pub adopt: bool,

    /// Adoption destructiveness. `inspect` prints the plan only.
    /// `mirror` writes a klasp.toml that mirrors detected gates.
    /// `chain` integrates klasp into existing hook managers (v1: not
    /// supported; rejected with an explanatory message). Implies `--adopt`.
    #[arg(long, value_enum, default_value_t = AdoptModeArg::Inspect)]
    pub mode: AdoptModeArg,
}

/// Clap-visible adoption mode. Controls how destructive `--adopt` is.
///
/// Maps to [`crate::adopt::plan::AdoptMode`] at dispatch time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AdoptModeArg {
    /// Print the detected plan only; never touch the filesystem.
    Inspect,
    /// Write or update `klasp.toml` to mirror detected gates.
    /// Never modifies existing hook files.
    Mirror,
    /// Integrate klasp into existing hook managers (v1: not yet supported).
    Chain,
}

impl From<AdoptModeArg> for AdoptMode {
    fn from(arg: AdoptModeArg) -> Self {
        match arg {
            AdoptModeArg::Inspect => AdoptMode::Inspect,
            AdoptModeArg::Mirror => AdoptMode::Mirror,
            AdoptModeArg::Chain => AdoptMode::Chain,
        }
    }
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Restrict installation to one agent (`claude_code`, `codex`, `aider`) or
    /// pass `all` to install every surface listed in `klasp.toml`'s
    /// `[gate].agents`. Omit to install all auto-detected surfaces.
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
    /// Restrict removal to one agent (`claude_code`, `codex`, `aider`) or pass
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
    /// Stable JSON output (KLASP_OUTPUT_SCHEMA = 1) written to stdout or `--output`.
    Json,
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

    /// Informational: the agent surface that invoked the gate. Klasp resolves
    /// the effective agent from `KLASP_AGENT_ID` and the tool-input payload;
    /// this flag is accepted-and-ignored so installed hooks (Codex git-hooks,
    /// Aider commit-cmd-pre) can pass it without clap rejecting them.
    /// Future versions may use this as a hint for trigger classification.
    #[arg(long)]
    pub agent: Option<String>,

    /// Informational: the trigger kind (`commit` or `push`). Same accept-and-
    /// ignore semantics as `--agent`. Klasp resolves the trigger from the
    /// tool-input payload's `hook_event_name`.
    #[arg(long)]
    pub trigger: Option<String>,
}

#[derive(Debug, Args)]
pub struct DoctorArgs {}

#[derive(Debug, Args)]
pub struct SetupArgs {
    /// Walk through detected gates and agents with y/n prompts before
    /// writing klasp.toml and running install.
    #[arg(long)]
    pub interactive: bool,

    /// Print the detection plan and computed config without writing any files.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct PluginsArgs {
    #[command(subcommand)]
    pub action: PluginsAction,
}

#[derive(Debug, Subcommand)]
pub enum PluginsAction {
    /// List klasp-plugin-* binaries found on $PATH.
    List,
    /// Print the plugin's --describe output (protocol version, capabilities).
    Info {
        /// Plugin name (without the `klasp-plugin-` prefix).
        name: String,
    },
    /// Add a plugin to the per-user disable list.
    ///
    /// Disabled plugins are skipped silently during `klasp gate`. The disable
    /// list is stored at $KLASP_DISABLED_PLUGINS_FILE or
    /// ~/.config/klasp/disabled-plugins.toml.
    Disable {
        /// Plugin name (without the `klasp-plugin-` prefix).
        name: String,
    },
}

pub fn run() -> ExitCode {
    let cli = Cli::parse();
    match &cli.command {
        Cmd::Init(args) => cmd::init::run(args),
        Cmd::Install(args) => cmd::install::run(args),
        Cmd::Uninstall(args) => cmd::uninstall::run(args),
        Cmd::Gate(args) => cmd::gate::run(args),
        Cmd::Doctor(args) => cmd::doctor::run(args),
        Cmd::Plugins(args) => cmd::plugins::run(&args.action),
        Cmd::Setup(args) => cmd::setup::run(args),
    }
}
