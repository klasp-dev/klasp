use std::process::ExitCode;

use crate::cli::GateArgs;

pub fn run(_args: &GateArgs) -> ExitCode {
    eprintln!(
        "klasp gate: not yet implemented (v0.1 W3). Track https://github.com/klasp-dev/klasp/issues/1 for progress."
    );
    ExitCode::from(1)
}
