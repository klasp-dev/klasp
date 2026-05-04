//! klasp CLI — entry point.
//!
//! W3 wires up the gate runtime (`klasp gate`) and its supporting modules
//! (`git`, `sources`). The remaining subcommands (`init`, `install`,
//! `uninstall`, `doctor`) still return their W1 placeholder — those land in
//! W2 / W4 per [docs/roadmap.md §"Timeline"].

mod cli;
mod cmd;
mod git;
mod registry;
mod sources;

fn main() -> std::process::ExitCode {
    cli::run()
}
