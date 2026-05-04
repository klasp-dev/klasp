//! klasp CLI — entry point.
//!
//! v0.1 W1 ships clap definitions for the five subcommands; the runtime
//! behaviour lands in W2-W5. Each subcommand prints a "not yet implemented"
//! notice and exits with status 1.

mod cli;
mod cmd;
mod registry;

fn main() -> std::process::ExitCode {
    cli::run()
}
