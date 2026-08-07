//! The typed flag layer: one row's parameters, from one place.
//!
//! `--params '<JSON>'` reaches every row on the table and always will — it is
//! the escape hatch a generated surface owes the rows nobody types by hand, and
//! the only way to reach a field no flag covers. But amx is keyboard-first
//! (03 §1) and 05's M2 promises `amx wait --until blocked`, `amx agent prompt
//! --wait` and `amx pane wait-output --match/--regex` to a human who types them
//! all day, so the verbs a human drives carry real flags too.
//!
//! The rule that stops this becoming a fifth hand-synced list (W6): **a row's
//! parameters come from one place.** Each [`FlagRow`] owns three things that
//! cannot drift apart —
//!
//! - the clap arguments its leaf carries;
//! - the parse that turns them into the row's *own* payload type, written as a
//!   struct literal, so a field added to that payload later stops compiling
//!   here rather than quietly defaulting;
//! - a [`Field`] per payload field saying where the value comes from: an
//!   argument, or [`Source::ParamsOnly`] with the reason.
//!
//! `crates/amx-proto/tests/flags.rs` closes the loop by serializing a
//! fully-populated payload and asserting the field list covers every key it
//! produced, that every named argument exists on the leaf, and that the leaf
//! carries no argument no field names. A payload field cannot therefore lack a
//! flag *silently*: it either fails the build or fails that test.
//!
//! Rows with no entry here are `--params`-only, which is the right answer for
//! the ones nothing types by hand: `agent.report` is written by `amx _hook`,
//! `client.viewport` by an attached client, `stream.bind` by whatever binds the
//! stream.

use std::fmt::Display;
use std::str::FromStr;

use clap::{Arg, ArgMatches, Command};
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use crate::control::Method;

// ------------------------------------------------------------------ the table

/// Where one payload field's value comes from on the command line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// The leaf argument with this id — a flag, or a positional.
    Arg(&'static str),
    /// Reachable only through `--params`, for the reason it carries.
    ParamsOnly(&'static str),
}

/// One payload field, and where the CLI gets it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Field {
    /// The field's name on the wire, exactly as serde spells it.
    pub name: &'static str,
    /// Where its value comes from.
    pub source: Source,
}

impl Field {
    /// The argument id this field reads, if a flag covers it.
    #[must_use]
    pub const fn arg_id(&self) -> Option<&'static str> {
        match self.source {
            Source::Arg(id) => Some(id),
            Source::ParamsOnly(_) => None,
        }
    }
}

/// A field carried by the leaf argument `id`.
#[must_use]
pub const fn from_arg(name: &'static str, id: &'static str) -> Field {
    Field {
        name,
        source: Source::Arg(id),
    }
}

/// A field only `--params` reaches, and the reason it is not a flag.
#[must_use]
pub const fn params_only(name: &'static str, why: &'static str) -> Field {
    Field {
        name,
        source: Source::ParamsOnly(why),
    }
}

/// One method table row's flags: its arguments, its parse, and its field map.
#[derive(Clone, Copy, Debug)]
pub struct FlagRow {
    /// The row these flags belong to.
    pub method: Method,
    /// Every field of the row's parameter type, and where it comes from.
    pub fields: &'static [Field],
    /// Add this row's arguments to its leaf command.
    pub args: fn(Command) -> Command,
    /// Build the row's parameters from what the leaf parsed.
    pub parse: fn(&ArgMatches) -> Result<Value, FlagError>,
}

// -------------------------------------------------------- the shared vocabulary

/// The id of the pane a driving verb acts on.
pub const TARGET: &str = "target";

/// The id of the text a verb submits.
pub const TEXT: &str = "text";

/// The id of a wait's deadline.
pub const TIMEOUT: &str = "timeout";

/// The `<TARGET>` positional every pane-driving verb leads with.
///
/// Resolution is the server's and its order is fixed (`agent/address.rs`): a
/// pane UUID first, then a label that must be unique. The target is passed
/// through as written, so what a target may *be* stays one decision, made
/// there — a CLI that pre-parsed it would be a second answer to the same
/// question.
#[must_use]
pub fn target_arg(help: &'static str) -> Arg {
    Arg::new(TARGET).value_name("TARGET").help(help)
}

/// The `--text` a verb submits.
#[must_use]
pub fn text_arg(help: &'static str) -> Arg {
    Arg::new(TEXT).long("text").value_name("TEXT").help(help)
}

/// The `--timeout` a waiting verb takes.
///
/// The caller's `help` says what is being waited for and what absence means,
/// because that differs per row: the long-polls wait forever without one and
/// `agent start` falls back to the server's readiness budget.
#[must_use]
pub fn timeout_arg(help: &'static str) -> Arg {
    Arg::new(TIMEOUT)
        .long("timeout")
        .value_name("DURATION")
        .help(help)
        .long_help(format!(
            "{help}.\n\n\
             Written as `500ms`, `30s`, `2m` or `1h`; a bare number is seconds."
        ))
}

// -------------------------------------------------------------------- reading

/// The value of a required argument.
///
/// # Errors
///
/// [`FlagError::Missing`], naming the argument as a user would type it.
pub fn required<'a>(
    matches: &'a ArgMatches,
    id: &str,
    shown: &'static str,
) -> Result<&'a String, FlagError> {
    matches
        .get_one::<String>(id)
        .ok_or(FlagError::Missing { arg: shown })
}

/// The value of an optional argument, parsed.
///
/// # Errors
///
/// [`FlagError::Invalid`] carrying the parse's own complaint, which for the id
/// types is the one that says what a UUID has to look like.
pub fn parse_opt<T>(
    matches: &ArgMatches,
    id: &str,
    shown: &'static str,
) -> Result<Option<T>, FlagError>
where
    T: FromStr,
    T::Err: Display,
{
    match matches.get_one::<String>(id) {
        None => Ok(None),
        Some(text) => text
            .parse::<T>()
            .map(Some)
            .map_err(|why| FlagError::invalid(shown, why.to_string())),
    }
}

/// The `--timeout` this leaf carries, in milliseconds.
///
/// # Errors
///
/// [`FlagError::Invalid`] when the duration is not one this understands.
pub fn timeout(matches: &ArgMatches) -> Result<Option<u64>, FlagError> {
    match matches.get_one::<String>(TIMEOUT) {
        None => Ok(None),
        Some(text) => millis(text)
            .map(Some)
            .ok_or_else(|| FlagError::invalid("--timeout", format!("{text:?} is not a duration"))),
    }
}

/// Milliseconds from a written duration: `500ms`, `30s`, `2m`, `1h`, or a bare
/// number of seconds.
///
/// Integers only, and seconds for a bare one. A human typing `--timeout 30`
/// means half a minute, not a thirtieth of one; a fractional `1.5s` would be a
/// second spelling of `1500ms` and buys nothing.
#[must_use]
pub fn millis(text: &str) -> Option<u64> {
    let text = text.trim();
    let (digits, unit) = if let Some(rest) = text.strip_suffix("ms") {
        (rest, 1)
    } else if let Some(rest) = text.strip_suffix('s') {
        (rest, 1_000)
    } else if let Some(rest) = text.strip_suffix('m') {
        (rest, 60_000)
    } else if let Some(rest) = text.strip_suffix('h') {
        (rest, 3_600_000)
    } else {
        (text, 1_000)
    };
    digits.trim().parse::<u64>().ok()?.checked_mul(unit)
}

/// Encode a row's payload, once the flags have filled it in.
///
/// # Errors
///
/// [`FlagError::Encode`], which cannot happen for a payload built from strings
/// and numbers and is reported rather than unwrapped because "cannot happen" is
/// not a thing this crate is allowed to assert.
pub fn encode<T: Serialize>(params: &T) -> Result<Value, FlagError> {
    serde_json::to_value(params).map_err(|why| FlagError::Encode {
        reason: why.to_string(),
    })
}

// --------------------------------------------------------------------- errors

/// Why a set of flags does not name a call.
///
/// Every message ends by pointing at `--params`, because it is always the
/// answer: whatever the flags cannot say, the JSON object can.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum FlagError {
    /// A required argument was not given.
    #[error("{arg} is required here; give it, or pass the whole call as --params '<JSON>'")]
    Missing {
        /// The argument, spelled as a user types it.
        arg: &'static str,
    },
    /// Two arguments that answer the same question were both given.
    #[error("{one} and {two} cannot be combined; use one, or pass --params '<JSON>'")]
    Exclusive {
        /// One of them.
        one: &'static str,
        /// The other.
        two: &'static str,
    },
    /// An argument's value is not one it can be.
    #[error("{arg}: {reason}")]
    Invalid {
        /// The argument, spelled as a user types it.
        arg: &'static str,
        /// What was wrong with the value.
        reason: String,
    },
    /// The payload the flags built could not be encoded.
    #[error("could not encode the call's parameters: {reason}")]
    Encode {
        /// serde's complaint.
        reason: String,
    },
}

impl FlagError {
    /// An argument whose value is not one it can be.
    #[must_use]
    pub fn invalid(arg: &'static str, reason: impl Into<String>) -> Self {
        Self::Invalid {
            arg,
            reason: reason.into(),
        }
    }
}
