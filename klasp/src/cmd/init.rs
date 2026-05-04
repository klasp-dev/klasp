use std::process::ExitCode;

use crate::cli::InitArgs;

pub fn run(_args: &InitArgs) -> ExitCode {
    eprintln!(
        "klasp init: not yet implemented (v0.1 W4-W5). Track https://github.com/klasp-dev/klasp/issues/1 for progress."
    );
    ExitCode::from(1)
}
