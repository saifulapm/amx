//! The `amx` binary.
//!
//! 04 §1: one binary, two roles — `amx` probes the session socket, daemonizes
//! `amx server` if nothing answers, and attaches; `amx <verb>` is a one-shot or
//! streaming call over the same socket. The verb tree is not written by hand:
//! the generated part comes from the protocol's method table, and the binary
//! adds only the verbs that have no wire method behind them.

use clap::Command;

fn cli() -> Command {
    Command::new("amx")
        .about("A minimal, keyboard-only agent terminal multiplexer")
        .subcommand_required(false)
        .subcommands(amx_proto::control::cli::method_commands())
}

fn main() -> anyhow::Result<()> {
    let matches = cli().get_matches();
    let _ = matches;
    Ok(())
}
