//! `amx events --json` — the event stream as NDJSON.
//!
//! 04 §8's first sentence about extensions: "Any program can `amx events
//! --json` (subscribe stream) and call any API method." This is that program's
//! side of the socket, and D-M2-5 is the design.
//!
//! What makes it worth a command rather than a documentation note is the
//! **resync contract**, which the help text has to state because the people
//! writing consumers cannot read this file:
//!
//! - every delivery is one line: an envelope, *or* a `gap{from,to}`;
//! - a `gap` is not an error and not something to skip. It means the consumer
//!   fell behind the replay buffer. The recovery is fixed: re-query
//!   `session.state`, which carries the bus sequence it was captured at, and
//!   resume from there (04 §2). A consumer that ignored gaps would silently
//!   miss transitions, which is exactly the failure the bus's design refuses to
//!   allow — loss must be visible *and* recoverable.
//!
//! `examples/notify.sh` (V16) is the reference consumer: about twenty lines of
//! POSIX sh that filter `attention_enqueued` and call `notify-send`, handling
//! gaps honestly, existing to prove that out-of-terminal notification is an
//! extension rather than a feature (03 §4).
//!
//! # Task ownership
//!
//! **V11** fills this, with `conn/events.rs` and the `events.subscribe`
//! dispatch on the server side. The whole path is new: there is no
//! server→client notification anywhere in the tree today, and the client drops
//! any it receives (R-M2-4 flags it as M2's largest unbudgeted piece).
//!
//! V02 planted the file and the clap tree so no wave task edits `cli.rs`.

use std::process::ExitCode;

use amx_core::Env;
use clap::ArgMatches;

/// Stream the session's bus deliveries to stdout.
///
/// **V11 fills this.**
pub async fn run(env: &Env, matches: &ArgMatches, sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    let _ = (env, matches, sub);
    anyhow::bail!("amx events is not implemented yet; V11 fills it")
}
