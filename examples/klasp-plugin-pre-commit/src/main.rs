//! `klasp-plugin-pre-commit` — reference plugin for the klasp v0 plugin protocol.
//!
//! This crate is the canonical "fork me" starting point for third-party klasp
//! plugin authors. It implements `--describe` and `--gate` exactly as the
//! protocol specification requires.
//!
//! ## Protocol
//!
//! Two flags are required:
//! - `--describe` → print [`protocol::PluginDescribe`] JSON to stdout, exit 0.
//! - `--gate` → read [`protocol::PluginGateInput`] from stdin, print
//!   [`protocol::PluginGateOutput`] to stdout, exit 0.
//!
//! The plugin MUST exit 0 in all cases — even when the verdict is `fail`.
//! Non-zero exit is an infrastructure error from klasp's perspective.
//!
//! ## Forking this plugin
//!
//! Copy this directory into its own repository and adjust:
//! 1. `Cargo.toml` — rename the package and binary.
//! 2. `src/protocol.rs` — no changes needed (the types are spec-level).
//! 3. `src/runner.rs` — replace the pre-commit invocation with your own check.
//! 4. `src/main.rs` — update [`PLUGIN_NAME`] and [`CONFIG_TYPES`].
//! 5. `README.md` — document your plugin.

mod protocol;
mod runner;

use anyhow::Result;
use clap::Parser;

use protocol::{PluginDescribe, PluginSupports, PROTOCOL_VERSION};

/// Canonical binary name (must match the `klasp-plugin-` prefix convention).
const PLUGIN_NAME: &str = "klasp-plugin-pre-commit";

/// Config `type` values this plugin supports (informational at v0).
const CONFIG_TYPES: &[&str] = &["pre-commit"];

#[derive(Parser, Debug)]
#[command(
    name = PLUGIN_NAME,
    about = "Reference klasp plugin that wraps pre-commit (v0 protocol)",
    long_about = None,
)]
struct Args {
    /// Print plugin capabilities as JSON and exit.
    #[arg(long, conflicts_with = "gate")]
    describe: bool,

    /// Read PluginGateInput from stdin, run pre-commit, print PluginGateOutput.
    #[arg(long, conflicts_with = "describe")]
    gate: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.describe {
        return cmd_describe();
    }

    if args.gate {
        return cmd_gate();
    }

    // No subcommand: print short help and exit 0.
    println!(
        "{PLUGIN_NAME}: a reference klasp plugin.\n\
         Usage: {PLUGIN_NAME} --describe | --gate\n\
         See README.md for documentation."
    );
    Ok(())
}

/// Emit `PluginDescribe` JSON to stdout.
fn cmd_describe() -> Result<()> {
    let describe = PluginDescribe {
        protocol_version: PROTOCOL_VERSION,
        name: PLUGIN_NAME.to_string(),
        config_types: CONFIG_TYPES.iter().map(|s| s.to_string()).collect(),
        supports: PluginSupports { verdict_v0: true },
    };
    println!("{}", serde_json::to_string(&describe)?);
    Ok(())
}

/// Read `PluginGateInput` from stdin, run pre-commit, emit `PluginGateOutput`.
fn cmd_gate() -> Result<()> {
    let input: protocol::PluginGateInput = serde_json::from_reader(std::io::stdin())?;
    let output = runner::run_gate(&input);
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}
