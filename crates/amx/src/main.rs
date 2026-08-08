//! The `amx` binary.
//!
//! Three lines over [`amx::run`], plus the one decision that has to be made
//! before a parse can happen: **which machine is this command line for.**
//! `--remote HOST` is lifted off `argv` here and what is left goes to the
//! ordinary tree, on this host or the far one ([`amx::remote`] explains why the
//! flag is not a clap argument). Everything else a test would want to reach
//! lives in the library beside this.

use std::process::ExitCode;

fn main() -> ExitCode {
    let outcome = match amx::remote::split(std::env::args_os()) {
        Ok((Some(remote), rest)) => amx::remote::run(&remote, rest),
        Ok((None, rest)) => amx::run(rest),
        Err(err) => Err(err),
    };
    match outcome {
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
