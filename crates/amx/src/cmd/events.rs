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
//! This command **relays**; it does not resync on the consumer's behalf. It has
//! no state model to re-read, and a relay that quietly swallowed a gap would be
//! hiding the one signal the contract exists to make visible. What it does
//! provide is the other half of the contract: `--after-seq N` resumes the
//! stream, which is what a consumer that re-queried state does with the
//! sequence that reply carried. A sequence the replay buffer has already
//! dropped is not an error — the first line is another `gap`, so resuming is
//! never a way to lose something silently either.
//!
//! `examples/notify.sh` (V16) is the reference consumer: about twenty lines of
//! POSIX sh that filter `attention_enqueued` and call `notify-send`, handling
//! gaps honestly, existing to prove that out-of-terminal notification is an
//! extension rather than a feature (03 §4).
//!
//! # Task ownership
//!
//! **V11** filled this, with `conn/events.rs` and the `events.subscribe`
//! dispatch on the server side. The whole path was new: there was no
//! server→client notification anywhere in the tree, and the client dropped any
//! it received (R-M2-4 flagged it as M2's largest unbudgeted piece).
//!
//! V02 planted the file and the clap tree so no wave task edits `cli.rs`.

use std::io::Write as _;
use std::process::ExitCode;

use amx_client::net::{self, Session};
use amx_core::{Delivery, Env, Seq};
use amx_proto::control::wait::{EVENT_METHOD, SubscribeReply};
use amx_proto::rpc::Notification;
use amx_server::session::probe::probe;
use anyhow::Context as _;
use clap::ArgMatches;
use serde_json::{Value, json};

use crate::cli::AFTER_SEQ;
use crate::cmd::attach::client_info;
use crate::ctx_of;

/// Stream the session's bus deliveries to stdout.
pub async fn run(env: &Env, matches: &ArgMatches, sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    let after_seq = match sub.get_one::<String>(AFTER_SEQ) {
        Some(text) => Some(
            text.parse::<Seq>()
                .context("--after-seq takes a bus sequence number")?,
        ),
        None => None,
    };

    let ctx = ctx_of(env, matches, None)?;
    anyhow::ensure!(
        probe(&ctx.socket)
            .context("probe the session socket")?
            .is_running(),
        "session {} is not running; start it with `amx`",
        ctx.session
    );
    let stream = net::connect(&ctx.socket)
        .await
        .context("connect to the session")?;
    let (mut session, _welcome) = Session::attach(stream, client_info(), false, None)
        .await
        .context("negotiate with the session")?;

    let params = match after_seq {
        Some(seq) => json!({ "after_seq": seq }),
        None => json!({}),
    };
    let reply: SubscribeReply = serde_json::from_value(
        session
            .call("events.subscribe", params)
            .await
            .context("call events.subscribe")?,
    )
    .context("decode the events.subscribe reply")?;

    // The seq the subscription was taken at goes to stderr, not into the
    // stream: stdout is deliveries and nothing else, so a consumer can pipe it
    // straight into a line reader. A human who wants to resume later reads it
    // here; a program reads it from `session.state`.
    eprintln!("subscribed at seq {}", reply.seq);

    relay(&mut session).await
}

/// Read deliveries until the session ends, printing one line each.
///
/// A frame that is not an event notification is skipped rather than refused:
/// the same catch-all rule every consumer in the tree follows, so a newer
/// server saying something this build does not know costs a skipped line and
/// not a dead consumer.
async fn relay(session: &mut Session) -> anyhow::Result<ExitCode> {
    let mut stdout = std::io::stdout();
    let mut buf = Vec::new();
    loop {
        let header = match session.read_frame_into(&mut buf).await {
            Ok(header) => header,
            // The session closed. That is how this command ends: it has no
            // other exit condition, and a shut-down session is not a failure.
            Err(_closed) => return Ok(ExitCode::SUCCESS),
        };
        if !header.is_control() {
            continue;
        }
        let Some(delivery) = decode(&buf) else {
            continue;
        };
        // Flushed per line, because the consumer on the other end of the pipe
        // is reacting to these: a notifier that learned about a block once its
        // buffer filled would be worse than no notifier.
        writeln!(stdout, "{delivery}").context("write a delivery")?;
        stdout.flush().context("flush a delivery")?;
    }
}

/// One control frame as the delivery line it carries, if that is what it is.
fn decode(payload: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(payload).ok()?;
    // JSON-RPC distinguishes a notification from a reply by the absence of an
    // id, not by any tag — the same check the server's reader makes.
    if value.get("id").is_some() {
        return None;
    }
    let notification: Notification = serde_json::from_value(value).ok()?;
    if notification.method != EVENT_METHOD {
        return None;
    }
    let params = notification.params?;
    // Round-tripped through `Delivery` rather than printed as it arrived, so a
    // line this command emits is one this build can also parse — a relay that
    // forwarded bytes it could not read would let a shape it never handled look
    // like one it did.
    let delivery: Delivery = serde_json::from_value(params).ok()?;
    serde_json::to_string(&delivery).ok()
}
