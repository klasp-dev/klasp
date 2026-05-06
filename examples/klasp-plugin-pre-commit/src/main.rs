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

use std::io::{BufReader, Write};

use clap::Parser;

use protocol::{PluginDescribe, PluginGateOutput, PluginSupports, PROTOCOL_VERSION};

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

fn main() {
    let args = Args::parse();

    if args.describe {
        cmd_describe();
        std::process::exit(0);
    }

    if args.gate {
        cmd_gate();
        std::process::exit(0);
    }

    // No subcommand: print short help to stderr (stdout is reserved for
    // protocol output) and exit 0.
    eprintln!(
        "{PLUGIN_NAME}: a reference klasp plugin.\n\
         Usage: {PLUGIN_NAME} --describe | --gate\n\
         See README.md for documentation."
    );
}

/// Emit `PluginDescribe` JSON to stdout. Infallible — `PluginDescribe`
/// serialization cannot fail given the static `PluginSupports` shape.
fn cmd_describe() {
    let describe = PluginDescribe {
        protocol_version: PROTOCOL_VERSION,
        name: PLUGIN_NAME.to_string(),
        config_types: CONFIG_TYPES.iter().map(|s| s.to_string()).collect(),
        supports: PluginSupports { verdict_v0: true },
    };
    let json = serde_json::to_string(&describe).unwrap_or_else(|_| {
        serde_json::to_string(&runner::infra_warn(
            "describe-serialize-failed",
            "internal error: failed to serialize PluginDescribe",
        ))
        .unwrap_or_else(|_| String::from("{}"))
    });
    write_stdout_line(&json);
}

/// Read `PluginGateInput` from stdin, run pre-commit, emit `PluginGateOutput`.
/// All error paths convert to a `Verdict::Warn` JSON output and exit 0 — the
/// plugin protocol mandates that the gate continues running other checks even
/// when this plugin's input is malformed or its environment is broken.
fn cmd_gate() {
    let stdin = std::io::stdin();
    let output: PluginGateOutput =
        match serde_json::from_reader::<_, protocol::PluginGateInput>(BufReader::new(stdin.lock()))
        {
            Ok(input) => runner::run_gate(&input),
            Err(e) => runner::infra_warn(
                "input-parse-error",
                format!("failed to parse PluginGateInput from stdin: {e}"),
            ),
        };

    let json = serde_json::to_string(&output)
        .unwrap_or_else(|_| String::from(
            r#"{"protocol_version":0,"verdict":"warn","findings":[{"severity":"warn","rule":"klasp-plugin-pre-commit/output-serialize-failed","message":"internal error: PluginGateOutput serialize failed"}]}"#,
        ));
    write_stdout_line(&json);
}

/// Write `s + "\n"` to a locked stdout in a single flushed sequence so
/// klasp's reader sees a complete JSON line without interleaving.
fn write_stdout_line(s: &str) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = out.write_all(s.as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}
