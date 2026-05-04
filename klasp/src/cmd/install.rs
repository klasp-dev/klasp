use std::process::ExitCode;

use crate::cli::InstallArgs;

pub fn run(_args: &InstallArgs) -> ExitCode {
    eprintln!(
        "klasp install: not yet implemented (v0.1 W2). Track https://github.com/klasp-dev/klasp/issues/1 for progress."
    );
    ExitCode::from(1)
}
