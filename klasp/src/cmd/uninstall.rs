use std::process::ExitCode;

use crate::cli::UninstallArgs;

pub fn run(_args: &UninstallArgs) -> ExitCode {
    eprintln!(
        "klasp uninstall: not yet implemented (v0.1 W2). Track https://github.com/klasp-dev/klasp/issues/1 for progress."
    );
    ExitCode::from(1)
}
