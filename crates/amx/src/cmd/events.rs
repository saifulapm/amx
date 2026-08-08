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
//! # Across a server swap (D-M3-7)
//!
//! A handoff retires the session socket under every consumer (§3 step 11), and
//! a relay that ended there would make an upgrade look like the end of the
//! stream — silently, since a shut-down session is not a failure and this
//! command exits zero on one. So the relay keeps its own cursor — the sequence
//! of the last delivery it *printed* — and redials, resubscribing with
//! `after_seq` set to it. The two outcomes are the two the contract already
//! names, and both are already the consumer's to handle:
//!
//! - the cursor is still inside the successor's replay ring, and the stream
//!   continues with the deliveries published during the swap;
//! - it is not, and the first line after the swap is a `gap` — which is what a
//!   consumer that fell behind gets anyway, and what it already knows to answer
//!   by re-querying state.
//!
//! Neither is a new shape on stdout, so `examples/notify.sh` needed no change:
//! the swap is invisible to a correct consumer and reported to every consumer.
//! A session that stays gone past [`REDIAL_WINDOW`] still ends the command the
//! way a stopped session always did — cleanly, exit zero, because a session
//! ending is not this command failing — but it says so on stderr first. Giving
//! up is never silent even when it is not an error.
//!
//! # Task ownership
//!
//! **V11** filled this, with `conn/events.rs` and the `events.subscribe`
//! dispatch on the server side. The whole path was new: there was no
//! server→client notification anywhere in the tree, and the client dropped any
//! it received (R-M2-4 flagged it as M2's largest unbudgeted piece).
//!
//! V02 planted the file and the clap tree so no wave task edits `cli.rs`.
//!
//! **W09** added the redial.

use std::io::Write as _;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use amx_client::net::{self, NetError, Session};
use amx_core::{Ctx, Delivery, Env, Seq, SessionId};
use amx_proto::Welcome;
use amx_proto::control::wait::{EVENT_METHOD, SubscribeReply};
use amx_proto::rpc::Notification;
use amx_server::session::probe::probe;
use anyhow::Context as _;
use clap::ArgMatches;
use serde_json::{Value, json};

use crate::cli::AFTER_SEQ;
use crate::cmd::attach::client_info;
use crate::ctx_of;

/// How long the relay keeps redialling a socket that is between owners.
///
/// The same figure `super::call` uses and for the same reason: §3's socket-free
/// probe loop is capped at five seconds, so anything short of that would turn a
/// slow-but-successful upgrade into the end of the stream.
const REDIAL_WINDOW: Duration = Duration::from_secs(10);

/// How long the first redial waits, and how long the longest does.
const FIRST_BACKOFF: Duration = Duration::from_millis(20);
const LONGEST_BACKOFF: Duration = Duration::from_millis(250);

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

    let mut cursor = after_seq;
    let mut stdout = std::io::stdout();
    let mut first = true;
    let mut server: Option<SessionId> = None;
    loop {
        let Some((mut session, welcome)) = redial(&ctx).await else {
            // Nothing answers the path and nothing came back. On the first
            // connection that is a failure — the probe above said a server was
            // there — and afterwards it is how this command ends, because a
            // session that stopped is not a failure and never has been. Said
            // on stderr either way: giving up is never silent.
            if first {
                anyhow::bail!(
                    "session {} answered its socket and then refused a connection",
                    ctx.session
                );
            }
            eprintln!("the session stopped answering; ending the stream");
            return Ok(ExitCode::SUCCESS);
        };
        first = false;
        // The `Welcome.session` branch, and it is not optional here. A cold
        // restart begins the sequence space again at zero, so a cursor from
        // the server that is gone names a sequence the new one will reach in
        // its own time and mean something else by — resuming from it would
        // skip everything the new session published first, silently, which is
        // the one thing this stream is not allowed to do. A swap
        // (`session.handoff`) keeps the id *and* the sequence, which is what
        // makes resuming across one exact.
        if server.is_some_and(|was| was != welcome.session) {
            eprintln!("the session restarted; its sequence numbers begin again");
            cursor = None;
        }
        server = Some(welcome.session);
        let params = match cursor {
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
        // stream: stdout is deliveries and nothing else, so a consumer can pipe
        // it straight into a line reader. A human who wants to resume later
        // reads it here; a program reads it from `session.state`.
        eprintln!("subscribed at seq {}", reply.seq);
        // Only after the first subscription: resuming from where the last
        // delivery left off is more exact than resuming from where the
        // subscription opened, and only the first time round is there no
        // "where it left off".
        cursor = cursor.or(Some(reply.seq));

        relay(&mut session, &mut stdout, &mut cursor).await?;
        // The connection ended. Whether that was a swap or the session
        // stopping is answered by trying to redial, not guessed at here — the
        // socket is unlinked for both, and only one of them binds it back.
    }
}

/// Connect and negotiate, redialling a socket that is between owners.
///
/// `None` when [`REDIAL_WINDOW`] passes with nothing answering. A refusal that
/// is not the transport — a protocol nobody agreed on — is not redialled and
/// not swallowed: it is reported here, because no amount of waiting fixes it.
async fn redial(ctx: &Ctx) -> Option<(Session, Welcome)> {
    let started = Instant::now();
    let mut attempts = 0_u32;
    loop {
        if attempts > 0 {
            let delay = backoff(attempts - 1);
            if started.elapsed() + delay > REDIAL_WINDOW {
                return None;
            }
            tokio::time::sleep(delay).await;
        }
        attempts += 1;

        match dial(ctx).await {
            Ok(pair) => return Some(pair),
            Err(err) if err.is_transport() => {}
            Err(err) => {
                eprintln!("amx events: {err}");
                return None;
            }
        }
    }
}

/// One connect-and-negotiate attempt.
async fn dial(ctx: &Ctx) -> Result<(Session, Welcome), NetError> {
    let stream = net::connect(&ctx.socket).await?;
    Session::attach(stream, client_info(), false, None).await
}

/// The delay after `attempts` failed redials: doubling, capped.
fn backoff(attempts: u32) -> Duration {
    let grown = FIRST_BACKOFF
        .as_millis()
        .saturating_mul(1_u128 << attempts.min(16));
    Duration::from_millis(u64::try_from(grown).unwrap_or(u64::MAX)).min(LONGEST_BACKOFF)
}

/// Read deliveries until the connection ends, printing one line each and
/// recording how far the stream got.
///
/// A frame that is not an event notification is skipped rather than refused:
/// the same catch-all rule every consumer in the tree follows, so a newer
/// server saying something this build does not know costs a skipped line and
/// not a dead consumer.
///
/// `cursor` moves to the sequence of every delivery *printed* — an envelope's
/// own seq, and a gap's `to`, because a gap is loss that has already moved the
/// stream on. It is what a redial resumes from, so it must never run ahead of
/// what stdout has actually carried.
async fn relay(
    session: &mut Session,
    stdout: &mut std::io::Stdout,
    cursor: &mut Option<Seq>,
) -> anyhow::Result<()> {
    let mut buf = Vec::new();
    loop {
        let header = match session.read_frame_into(&mut buf).await {
            Ok(header) => header,
            Err(_closed) => return Ok(()),
        };
        if !header.is_control() {
            continue;
        }
        let Some((delivery, seq)) = decode(&buf) else {
            continue;
        };
        // Flushed per line, because the consumer on the other end of the pipe
        // is reacting to these: a notifier that learned about a block once its
        // buffer filled would be worse than no notifier.
        writeln!(stdout, "{delivery}").context("write a delivery")?;
        stdout.flush().context("flush a delivery")?;
        *cursor = Some(seq);
    }
}

/// One control frame as the delivery line it carries and the sequence that
/// line leaves the stream at, if that is what it is.
fn decode(payload: &[u8]) -> Option<(String, Seq)> {
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
    // A gap's `to` and not its `from`: the events between are gone, and asking
    // a successor to replay from before them would ask for a window nothing
    // can still produce.
    let seq = match delivery {
        Delivery::Event(ref envelope) => envelope.seq,
        Delivery::Gap { to, .. } => to,
    };
    serde_json::to_string(&delivery)
        .ok()
        .map(|line| (line, seq))
}
