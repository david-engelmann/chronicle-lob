//! Binary entry point for `chronicle-cli` (`docs/ARCHITECTURE.md` section 2.3).

#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    chronicle_lob_cli::run_cli(std::env::args_os())
}
