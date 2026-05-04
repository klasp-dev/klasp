use std::process::ExitCode;

use crate::cli::DoctorArgs;

pub fn run(_args: &DoctorArgs) -> ExitCode {
    eprintln!(
        "klasp doctor: not yet implemented (v0.1 W4). Track https://github.com/klasp-dev/klasp/issues/1 for progress."
    );
    ExitCode::from(1)
}
