//! The `amx` binary.
//!
//! Three lines over [`amx::run`]: everything a test would want to reach lives
//! in the library beside it.

use std::process::ExitCode;

fn main() -> ExitCode {
    match amx::run(std::env::args_os()) {
        Ok(code) => code,
        Err(err) => match err.downcast::<clap::Error>() {
            // clap already formatted the usage message (and `--help` is not a
            // failure), so printing an `anyhow` chain over the top of it would
            // say everything twice.
            Ok(usage) => usage.exit(),
            Err(err) => {
                eprintln!("amx: {err:#}");
                ExitCode::FAILURE
            }
        },
    }
}
