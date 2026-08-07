//! Flags for the pane-driving verbs.
//!
//! 05's M2: "Pane driving: `pane send-text|send-keys|run|read`". Each leads
//! with the `<TARGET>` positional of [`flag::target_arg`], because the thing
//! being driven is the first thing a human says.
//!
//! The layout verbs (`pane split`, `pane move`, …) are deliberately absent:
//! their parameters are pane UUIDs a client already holds and layout ratios
//! nobody types, and 04 §7 gives a human the prefix keys for them instead.

use clap::{Arg, ArgMatches, Command};
use serde_json::Value;

use crate::control::Method;
use crate::control::pane::{PaneTarget, ReadParams, RunParams, SendKeysParams, SendTextParams};

use super::flag::{self, FlagError, FlagRow, from_arg};

/// How much of a pane's screen to read.
const LINES: &str = "lines";
/// The key combos to send.
const KEYS: &str = "keys";

/// `amx pane run <TARGET> --text <TEXT>`.
pub(super) const RUN: FlagRow = FlagRow {
    method: Method::PaneRun,
    fields: &[
        from_arg("target", flag::TARGET),
        from_arg("text", flag::TEXT),
    ],
    args: run_args,
    parse: run,
};

/// `amx pane read <TARGET> [--lines N]`.
pub(super) const READ: FlagRow = FlagRow {
    method: Method::PaneRead,
    fields: &[from_arg("target", flag::TARGET), from_arg("lines", LINES)],
    args: read_args,
    parse: read,
};

/// `amx pane send-text <TARGET> --text <TEXT>`.
pub(super) const SEND_TEXT: FlagRow = FlagRow {
    method: Method::PaneSendText,
    fields: &[
        from_arg("target", flag::TARGET),
        from_arg("text", flag::TEXT),
    ],
    args: send_text_args,
    parse: send_text,
};

/// `amx pane send-keys <TARGET> <KEY>…`.
pub(super) const SEND_KEYS: FlagRow = FlagRow {
    method: Method::PaneSendKeys,
    fields: &[from_arg("target", flag::TARGET), from_arg("keys", KEYS)],
    args: send_keys_args,
    parse: send_keys,
};

fn run_args(command: Command) -> Command {
    command
        .arg(flag::target_arg("Which pane, by UUID or by label"))
        .arg(flag::text_arg("The line to type and submit"))
}

fn run(matches: &ArgMatches) -> Result<Value, FlagError> {
    let params = RunParams {
        target: target(matches)?,
        text: flag::required(matches, flag::TEXT, "--text")?.clone(),
    };
    flag::encode(&params)
}

fn read_args(command: Command) -> Command {
    command
        .arg(flag::target_arg("Which pane, by UUID or by label"))
        .arg(
            Arg::new(LINES)
                .long("lines")
                .value_name("N")
                .help("How many rows from the bottom [default: the whole visible screen]"),
        )
}

fn read(matches: &ArgMatches) -> Result<Value, FlagError> {
    let params = ReadParams {
        target: target(matches)?,
        lines: flag::parse_opt::<u16>(matches, LINES, "--lines")?,
    };
    flag::encode(&params)
}

fn send_text_args(command: Command) -> Command {
    command
        .arg(flag::target_arg("Which pane, by UUID or by label"))
        .arg(flag::text_arg(
            "The text, delivered to the child unchanged and not submitted",
        ))
}

fn send_text(matches: &ArgMatches) -> Result<Value, FlagError> {
    let params = SendTextParams {
        target: target(matches)?,
        text: flag::required(matches, flag::TEXT, "--text")?.clone(),
    };
    flag::encode(&params)
}

fn send_keys_args(command: Command) -> Command {
    command
        .arg(flag::target_arg("Which pane, by UUID or by label"))
        .arg(
            Arg::new(KEYS)
                .value_name("KEY")
                .num_args(1..)
                // A combo may be a bare `-`, and a lone hyphen is a key like
                // any other here — not the start of a flag.
                .allow_hyphen_values(true)
                .help("The combos to send, in order: `ctrl+c`, `f1`, `alt+shift+tab`, `enter`"),
        )
}

fn send_keys(matches: &ArgMatches) -> Result<Value, FlagError> {
    let params = SendKeysParams {
        target: target(matches)?,
        keys: matches
            .get_many::<String>(KEYS)
            .map(|keys| keys.cloned().collect())
            .unwrap_or_default(),
    };
    if params.keys.is_empty() {
        return Err(FlagError::Missing { arg: "<KEY>" });
    }
    flag::encode(&params)
}

/// The `<TARGET>` positional these four share.
fn target(matches: &ArgMatches) -> Result<PaneTarget, FlagError> {
    Ok(PaneTarget::new(
        flag::required(matches, flag::TARGET, "<TARGET>")?.as_str(),
    ))
}
