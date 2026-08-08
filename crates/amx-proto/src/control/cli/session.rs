//! Flags for the one verb M3 adds to the table.
//!
//! `amx session handoff --binary <PATH>` is typed by hand — by a user testing
//! an upgrade, and by `docs/09-m3-plan.md` §7's exit suite, which spells it
//! exactly that way — so it carries a real flag rather than living behind
//! `--params '{"binary": …}'`. `amx update apply` reaches the same row through
//! the wire, not through this tree.

use clap::{Arg, ArgMatches, Command};
use serde_json::Value;

use crate::control::Method;
use crate::control::session::HandoffParams;

use super::flag::{self, FlagError, FlagRow, from_arg};

/// The staged binary to hand over to.
const BINARY: &str = "binary";

/// `amx session handoff --binary <PATH> [--timeout <DURATION>]`.
pub(super) const HANDOFF: FlagRow = FlagRow {
    method: Method::SessionHandoff,
    fields: &[
        from_arg("binary", BINARY),
        from_arg("timeout_ms", flag::TIMEOUT),
    ],
    args: handoff_args,
    parse: handoff,
};

fn handoff_args(command: Command) -> Command {
    command
        .arg(
            Arg::new(BINARY)
                .long("binary")
                .value_name("PATH")
                .help("The staged binary to hand this session over to"),
        )
        .arg(flag::timeout_arg(
            "How long any one stage of the handoff may take [default: the server's own budget]",
        ))
}

fn handoff(matches: &ArgMatches) -> Result<Value, FlagError> {
    let params = HandoffParams {
        binary: flag::required(matches, BINARY, "--binary")?.into(),
        timeout_ms: flag::timeout(matches)?,
    };
    flag::encode(&params)
}
