//! Flags for the two long-polls a human waits on.
//!
//! 05's M2 promises both by name: "`amx wait --until blocked|idle|exited`" and
//! "`amx pane wait-output --match/--regex`". They live together here for the
//! reason their payloads do (`control/wait.rs`): one contract, 04 §2's — the
//! predicate is evaluated against current state at subscribe time and events
//! are only the notification, so neither of these ever polls and neither can be
//! hung by a transition that fell inside a bus gap.
//!
//! `events.subscribe` is the third row of that module and takes no flags here:
//! `amx events [--json] [--after-seq N]` is the verb a human uses, and it is a
//! long-lived consumer written by hand in the binary rather than a one-shot
//! call.

use clap::{Arg, ArgMatches, Command};
use serde_json::Value;

use crate::control::Method;
use crate::control::pane::PaneTarget;
use crate::control::wait::{WaitOutputParams, WaitParams, WaitUntil};

use super::flag::{self, FlagError, FlagRow, from_arg};

/// What `wait` waits for.
const UNTIL: &str = "until";
/// A literal substring to wait for.
const MATCH: &str = "match";
/// A regular expression to wait for.
const REGEX: &str = "regex";

/// `amx wait --until blocked|idle|exited --target <TARGET>`.
pub(super) const WAIT: FlagRow = FlagRow {
    method: Method::Wait,
    fields: &[
        from_arg("until", UNTIL),
        from_arg("target", flag::TARGET),
        from_arg("timeout_ms", flag::TIMEOUT),
    ],
    args: wait_args,
    parse: wait,
};

/// `amx pane wait-output <TARGET> --match <TEXT> | --regex <PATTERN>`.
pub(super) const WAIT_OUTPUT: FlagRow = FlagRow {
    method: Method::PaneWaitOutput,
    fields: &[
        from_arg("target", flag::TARGET),
        from_arg("match", MATCH),
        from_arg("regex", REGEX),
        from_arg("timeout_ms", flag::TIMEOUT),
    ],
    args: wait_output_args,
    parse: wait_output,
};

fn wait_args(command: Command) -> Command {
    command
        .arg(
            Arg::new(UNTIL)
                .long("until")
                .value_name("STATE")
                .value_parser(["blocked", "idle", "exited"])
                .help("What to wait for: the agent blocking, going idle, or the process ending"),
        )
        .arg(
            // A flag rather than the positional the pane verbs take: `amx wait
            // --until idle build` reads as though `build` were part of the
            // condition, and 05's M2 spells the verb with `--target`.
            Arg::new(flag::TARGET)
                .long("target")
                .value_name("TARGET")
                .help("Which pane, by UUID or by label"),
        )
        .arg(flag::timeout_arg("How long to wait [default: no limit]"))
}

fn wait(matches: &ArgMatches) -> Result<Value, FlagError> {
    let params = WaitParams {
        until: match flag::required(matches, UNTIL, "--until")?.as_str() {
            "blocked" => WaitUntil::Blocked,
            "idle" => WaitUntil::Idle,
            "exited" => WaitUntil::Exited,
            other => {
                return Err(FlagError::invalid(
                    "--until",
                    format!("{other:?} is not one of blocked, idle, exited"),
                ));
            }
        },
        target: PaneTarget::new(flag::required(matches, flag::TARGET, "--target")?.as_str()),
        timeout_ms: flag::timeout(matches)?,
    };
    flag::encode(&params)
}

fn wait_output_args(command: Command) -> Command {
    command
        .arg(flag::target_arg("Which pane, by UUID or by label"))
        .arg(
            Arg::new(MATCH)
                .long("match")
                .value_name("TEXT")
                .conflicts_with(REGEX)
                .help("A literal substring to wait for"),
        )
        .arg(
            Arg::new(REGEX)
                .long("regex")
                .value_name("PATTERN")
                .help("A regular expression to wait for, compiled once per call"),
        )
        .arg(flag::timeout_arg("How long to wait [default: no limit]"))
}

fn wait_output(matches: &ArgMatches) -> Result<Value, FlagError> {
    let params = WaitOutputParams {
        target: PaneTarget::new(flag::required(matches, flag::TARGET, "<TARGET>")?.as_str()),
        match_text: matches.get_one::<String>(MATCH).cloned(),
        regex: matches.get_one::<String>(REGEX).cloned(),
        timeout_ms: flag::timeout(matches)?,
    };
    // The wire contract is "exactly one"; clap has already refused both, so
    // what is left to catch is neither.
    if params.match_text.is_none() && params.regex.is_none() {
        return Err(FlagError::Missing {
            arg: "--match or --regex",
        });
    }
    flag::encode(&params)
}
