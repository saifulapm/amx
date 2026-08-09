//! `amx agents --watch`: the read-only mission-control screen.
//!
//! D14's cheapest deliverable — a phone SSH window, no client attached, no
//! panes drawn, just the table and whether it changed. D-M4-11 says what it is
//! built on: "`amx events --json`'s loop packaged: subscribe, redial on EOF,
//! re-query on `gap`", the contract [`crate::cmd::events`] already documents
//! and implements. Nothing here is a second contract.
//!
//! # Why the table is re-queried and never folded
//!
//! This loop holds no model of the session. Every delivery — a status change, a
//! pane's damage, a `gap`, a dropped notification — means exactly one thing for
//! the table: *ask again*. That is what makes the awkward cases free rather
//! than handled. A `gap` is loss the consumer is told about and this consumer's
//! whole response to loss is a fresh `agent.list`, so it can be neither
//! swallowed nor mishandled. A cold restart begins the sequence space again —
//! the case X00's wave-1 boundary handed to this task after watching
//! `amx events` print seq 1093 then seq 122 with nothing between — and a loop
//! that resumed from a cursor would silently skip everything the new server
//! published first. This one subscribes from wherever the successor is and asks
//! again; the restart is *said* on the footer rather than inferred from a hole
//! in the numbers.
//!
//! # What is read off the delivery, and why that is not folding
//!
//! The footer's last clause is not re-queried, because no query answers it. An
//! attention delivery carries D15's identity block, and
//! [`Announcement`](super::announce::Announcement) turns it into the sentence
//! that block was frozen for: `api/backend blocked 4s (permission_dialog)`. A
//! block that clears between two refresh windows is on no table this watch ever
//! draws, and a dequeue caused by a pane exiting names an agent the next
//! `agent.list` has no row for — both are things only the delivery says. That
//! module's header states the division: the block answers *what just happened*
//! about one agent and can never answer *what is on screen now* about the rest,
//! which is why it lands on the footer and not in a row.
//!
//! # One call per window, never one per pane (R-M4-7)
//!
//! Deliveries are coalesced into one `agent.list` per [`REFRESH`] window for
//! every row on screen. The alternative shape — a refresh per damaged pane —
//! is what the plan's own arithmetic refuses: 25 agents at 4 Hz would be 100
//! calls a second each walking 25 panes. X10 measured the call at 12–16 ms with
//! 25 agents on a busy box, so one window's worth is a few percent of a core
//! and only ever spent when the session actually published something. An idle
//! session costs nothing at all: the window ticks, finds no delivery, and
//! repaints only if a displayed age has moved on.
//!
//! # One connection
//!
//! The subscription and the queries share a socket.
//! [`Session::collect_notifications`] exists for exactly this: a notification
//! that lands while a call is in flight is queued rather than discarded, and
//! [`Session::take_notifications`] reports a queue that overflowed as the gap
//! it is. So a second connection would buy nothing and would double what a
//! redial has to put back.

use std::io::Write;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use amx_client::net::{self, NetError, Session};
use amx_client::render::FrameWriter;
use amx_client::term::{Sigwinch, TermSize, TerminalGuard};
use amx_core::agent::EpochMillis;
use amx_core::{Ctx, Delivery, SessionId, WorkspaceId};
use amx_proto::control::agent::{ListParams, ListReply};
use amx_proto::control::wait::{EVENT_METHOD, SubscribeReply};
use amx_proto::rpc::Notification;
use anyhow::Context as _;
use serde_json::json;
use tokio::io::AsyncReadExt as _;
use tokio::signal::unix::{SignalKind, signal};

use super::announce::Announcement;
use super::scope::Scope;
use super::table;
use crate::cmd::attach::{client_info, flush};

/// How often deliveries are turned into one refresh.
///
/// D15 puts the detail lines at "up to 4 Hz", debounced, and this is that
/// figure. It is a ceiling on the *asking*, not a heartbeat: a window with no
/// delivery in it makes no call.
const REFRESH: Duration = Duration::from_millis(250);

/// How long the screen keeps redialling a socket that is between owners.
///
/// The same ten seconds `amx events` and every one-shot verb wait, and for the
/// same reason: a handoff's socket-free probe loop is capped at five, so
/// anything shorter would turn a slow-but-successful upgrade into the end of a
/// watch.
const REDIAL_WINDOW: Duration = Duration::from_secs(10);

/// How long the first redial waits, and how long the longest does.
const FIRST_BACKOFF: Duration = Duration::from_millis(20);
const LONGEST_BACKOFF: Duration = Duration::from_millis(250);

/// Bytes that end the watch.
///
/// `q` is what D15 names. `ctrl+c` is here because raw mode has taken the
/// terminal's own interrupt away — `make_raw` clears `ISIG`, so the byte
/// arrives as a byte and a reflex that works everywhere else must work here.
/// `Esc` is deliberately **not** one of them: every arrow key starts with it.
const QUIT: [u8; 3] = [b'q', b'Q', 0x03];

/// Watch `ctx`'s session until the user quits or it stops coming back.
pub async fn run(ctx: &Ctx, scope: &Scope) -> anyhow::Result<ExitCode> {
    let mut screen = Screen::enter()?;
    let outcome = watch(ctx, scope, &mut screen).await;
    // Before anything is printed, and before an error message would be: an
    // error written onto the alternate screen goes away with it.
    screen.leave();
    outcome
}

/// The loop, with the terminal already taken.
async fn watch(ctx: &Ctx, scope: &Scope, screen: &mut Screen) -> anyhow::Result<ExitCode> {
    let mut stdin = tokio::io::stdin();
    let mut terminate = signal(SignalKind::terminate()).context("watch for SIGTERM")?;
    let mut input = [0_u8; 256];
    let mut frame = Vec::new();
    let mut notifications = Vec::new();

    let mut server: Option<SessionId> = None;
    let mut view = View::default();
    loop {
        let dialled = redial(ctx, screen, &view)
            .await
            .map_err(|err| anyhow::Error::new(err).context("negotiate with the session"))?;
        let Some((mut session, welcome)) = dialled else {
            screen.leave();
            eprintln!("the session stopped answering; ending the watch");
            return Ok(ExitCode::SUCCESS);
        };
        // A restart is a different server behind the same path. Nothing here
        // resumes from a cursor, so it costs no correctness — but a reader
        // watching a table blink is owed the reason.
        if server.is_some_and(|was| was != welcome) {
            view.note = Some("the session restarted");
            // A restart is a different session's panes under the same path, so
            // the agent the footer was naming is not one this server has heard
            // of. Announcements do not survive it; the table does not either,
            // and the difference is that the table is about to be re-read.
            view.announced = None;
        } else {
            view.note = None;
        }
        server = Some(welcome);

        // Every step of taking a session up can meet the socket dying under it
        // — a handoff retires the gateway on its own schedule — and every one
        // of them answers a transport failure the same way this loop does.
        let params = match open(&mut session, scope, &mut view).await {
            Ok(params) => params,
            Err(Refused::Gone) => {
                lost(screen, &mut view)?;
                continue;
            }
            Err(Refused::Answered(err)) => return Err(err),
        };
        screen.repaint(&view)?;

        let mut window = tokio::time::interval(REFRESH);
        window.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut pending = false;
        loop {
            tokio::select! {
                read = stdin.read(&mut input) => {
                    let Ok(n @ 1..) = read else { return Ok(ExitCode::SUCCESS) };
                    if input[..n].iter().any(|byte| QUIT.contains(byte)) {
                        return Ok(ExitCode::SUCCESS);
                    }
                }
                _ = terminate.recv() => return Ok(ExitCode::SUCCESS),
                resized = screen.resized() => {
                    resized?;
                    screen.repaint(&view)?;
                }
                header = session.read_frame_into(&mut frame) => {
                    match header {
                        Ok(_) => {}
                        Err(err) if err.is_transport() => break,
                        Err(err) => {
                            return Err(anyhow::Error::new(err).context("read from the session"));
                        }
                    }
                    pending |= drain(&mut session, &mut notifications, &mut view);
                }
                _ = window.tick() => {
                    // The queue is drained here as well as after a read: a
                    // notification can be absorbed by a call rather than by
                    // the read arm, and a delivery that only ever reached the
                    // queue would otherwise wake nothing.
                    pending |= drain(&mut session, &mut notifications, &mut view);
                    if pending {
                        match query(&mut session, &params).await {
                            Ok(reply) => view.take(reply),
                            Err(Refused::Gone) => break,
                            Err(Refused::Answered(err)) => return Err(err),
                        }
                        pending = false;
                    }
                    screen.paint_if_changed(&view)?;
                }
            }
        }
        lost(screen, &mut view)?;
    }
}

/// Take a fresh session up: subscribe, resolve the scope, take the first
/// reply, and hand back the parameters every later refresh reuses.
///
/// The subscription is opened **before** the first query, and
/// [`Session::collect_notifications`] before that, so a transition published
/// between the two is queued rather than missed. Losing one would leave the
/// table one refresh behind with nothing to wake it.
async fn open(
    session: &mut Session,
    scope: &Scope,
    view: &mut View,
) -> Result<serde_json::Value, Refused> {
    session.collect_notifications();
    let subscribed = session
        .call("events.subscribe", json!({}))
        .await
        .map_err(|err| Refused::of(err, "subscribe to the session's events"))?;
    let subscribed: SubscribeReply = serde_json::from_value(subscribed).map_err(|err| {
        Refused::Answered(anyhow::Error::new(err).context("decode the events.subscribe reply"))
    })?;
    view.seq = Some(subscribed.seq);
    // Resolved on every connection and never once: a cold restart is a new
    // session whose workspace ids are new too, and a watch holding the old one
    // would ask the successor about a workspace it has never heard of.
    let params = scope.params(session).await.map_err(Refused::from)?;
    // Read back out of the parameters rather than resolved a second time:
    // whatever this watch is about to ask `agent.list` for is exactly the scope
    // an announcement has to belong to, and two resolutions could differ.
    view.scope = serde_json::from_value::<ListParams>(params.clone())
        .ok()
        .and_then(|params| params.workspace);
    view.take(query(session, &params).await?);
    Ok(params)
}

/// Say on the screen that the session went away, before anything waits.
fn lost(screen: &mut Screen, view: &mut View) -> anyhow::Result<()> {
    // Whether it was a swap, a restart or a session stopping is answered by
    // trying to dial again, not guessed at here.
    view.note = Some("reconnecting…");
    view.stale = true;
    screen.repaint(view)
}

/// One `agent.list`, told apart from the two ways it can fail.
async fn query(session: &mut Session, params: &serde_json::Value) -> Result<ListReply, Refused> {
    let value = session
        .call("agent.list", params.clone())
        .await
        .map_err(|err| Refused::of(err, "call agent.list"))?;
    serde_json::from_value(value)
        .map_err(|err| Refused::Answered(anyhow::Error::new(err).context("decode agent.list")))
}

/// Why a call did not answer.
enum Refused {
    /// The connection went away. A redial is the response, and the error
    /// itself is not carried: the screen says `reconnecting…` and the only
    /// thing left to do about it is try again.
    Gone,
    /// The session answered, and its answer was no. A `--workspace` whose
    /// workspace has been killed is the case this exists for, and it ends the
    /// watch: the thing being watched is not there any more, and redrawing an
    /// older copy of it would be a lie.
    Answered(anyhow::Error),
}

impl Refused {
    /// Sort one network error into the two.
    fn of(err: NetError, what: &'static str) -> Self {
        if err.is_transport() {
            Self::Gone
        } else {
            Self::Answered(anyhow::Error::new(err).context(what))
        }
    }
}

impl From<anyhow::Error> for Refused {
    /// Sort an error that already wears its context.
    ///
    /// The calls in [`Scope`] answer with `anyhow`, so the transport question
    /// has to be asked of the *cause* rather than of the top — the same shape
    /// `crate::cmd::viewport` uses for the same reason.
    fn from(err: anyhow::Error) -> Self {
        if err
            .chain()
            .filter_map(|cause| cause.downcast_ref::<NetError>())
            .any(NetError::is_transport)
        {
            Self::Gone
        } else {
            Self::Answered(err)
        }
    }
}

/// Move every queued notification out of `session`, reporting whether anything
/// on it means the table is out of date.
///
/// Everything does — that is the point of the module header — so the return is
/// "was there anything at all", including a queue that overflowed, which
/// [`Session::take_notifications`] reports as the gap it is.
///
/// The deliveries are also *read* on the way past, which is the other half of
/// that header: an attention transition puts its identity block on the footer
/// here, and the `agent.list` the return value asks for answers a different
/// question about the same delivery.
fn drain(session: &mut Session, into: &mut Vec<Notification>, view: &mut View) -> bool {
    let lost = session.take_notifications(into);
    for notification in into.iter() {
        if notification.method != EVENT_METHOD {
            continue;
        }
        let Some(delivery) = notification
            .params
            .clone()
            .and_then(|params| serde_json::from_value::<Delivery>(params).ok())
        else {
            continue;
        };
        match delivery {
            Delivery::Event(envelope) => {
                view.seq = Some(envelope.seq);
                // Only an attention transition answers with anything, and only
                // then is the footer's sentence replaced: an announcement is
                // the last thing that happened to the queue, not the last thing
                // that happened.
                if let Some(said) = Announcement::of(&envelope.event, view.scope) {
                    view.announced = Some(said);
                }
            }
            // Said out loud rather than absorbed silently: the re-query below
            // is the recovery the contract names, and a reader is entitled to
            // know their stream lost something even though their table did
            // not.
            Delivery::Gap { to, .. } => {
                view.seq = Some(to);
                view.note = Some("the event stream gapped; re-queried");
                // The table recovers by asking again; the sentence cannot. What
                // was lost is exactly the deliveries that would have replaced
                // it, so the agent the footer names may have stopped waiting
                // inside the hole. Dropped rather than left standing: a
                // consumer that has been told it lost events should not be
                // shown one it might no longer be entitled to.
                view.announced = None;
            }
        }
    }
    lost || !into.is_empty()
}

/// Connect and negotiate, redialling a socket that is between owners.
///
/// `Ok(None)` when [`REDIAL_WINDOW`] passes with nothing answering, which is
/// how a watch ends when its session does not come back. An error is a refusal
/// no amount of waiting fixes — a protocol nobody agreed on — and is reported
/// rather than waited out.
async fn redial(
    ctx: &Ctx,
    screen: &mut Screen,
    view: &View,
) -> Result<Option<(Session, SessionId)>, NetError> {
    let started = Instant::now();
    let mut attempts = 0_u32;
    loop {
        if attempts > 0 {
            let delay = backoff(attempts - 1);
            if started.elapsed() + delay > REDIAL_WINDOW {
                return Ok(None);
            }
            tokio::time::sleep(delay).await;
            // The ages on a stale table keep moving while this waits, and a
            // screen that froze would look like a watch that had died.
            let _ = screen.paint_if_changed(view);
        }
        attempts += 1;

        let dialled = async {
            let stream = net::connect(&ctx.socket).await?;
            Session::attach(stream, client_info(), false, None).await
        }
        .await;
        match dialled {
            Ok((session, welcome)) => return Ok(Some((session, welcome.session))),
            Err(err) if err.is_transport() => {}
            Err(err) => return Err(err),
        }
    }
}

/// The delay after `attempts` failed redials: doubling, capped.
fn backoff(attempts: u32) -> Duration {
    let grown = FIRST_BACKOFF
        .as_millis()
        .saturating_mul(1_u128 << attempts.min(16));
    Duration::from_millis(u64::try_from(grown).unwrap_or(u64::MAX)).min(LONGEST_BACKOFF)
}

/// The newest reply, and what has happened to the stream since.
#[derive(Default)]
struct View {
    reply: Option<ListReply>,
    /// When [`reply`](Self::reply) arrived, on the *monotonic* clock.
    ///
    /// D-M4-4's second half: the age a row shows is `now − since` from inside
    /// one reply, advanced between refreshes by elapsed time and never by this
    /// machine's own wall clock. That is what makes the answer identical at
    /// both ends of an SSH link, and it is why nothing in this file calls
    /// `SystemTime::now`.
    read_at: Option<Instant>,
    /// The last sequence this watch saw, for the footer.
    seq: Option<amx_core::Seq>,
    /// A sentence for the footer: a gap, a restart, a redial in progress.
    note: Option<&'static str>,
    /// The last attention transition, said from the delivery that carried it.
    ///
    /// D15's identity block, and this is its reader. Kept until another
    /// transition replaces it, so a reader who looked away still learns what
    /// moved — with an age beside it, since a sentence that stays on screen has
    /// to say how old it is.
    announced: Option<Announcement>,
    /// The workspace `--workspace` narrowed this watch to, if it did.
    ///
    /// Only [`announced`](Self::announced) reads it: the table is narrowed by
    /// the server, which is asked with these same parameters, and a second
    /// opinion about the scope held on this side is a second opinion that can
    /// disagree.
    scope: Option<WorkspaceId>,
    /// Whether the table on screen is known to be behind the session.
    stale: bool,
}

impl View {
    /// Adopt a fresh reply.
    fn take(&mut self, reply: ListReply) {
        self.seq = Some(reply.seq);
        self.reply = Some(reply);
        self.read_at = Some(Instant::now());
        self.stale = false;
    }

    /// The server's clock as of now: the reply's own, plus how long ago it was
    /// read.
    fn now(&self) -> EpochMillis {
        let Some(reply) = &self.reply else {
            return 0;
        };
        let elapsed = self
            .read_at
            .map_or(0, |at| u64::try_from(at.elapsed().as_millis()).unwrap_or(0));
        reply.now.saturating_add(elapsed)
    }

    /// The lines this view puts on a terminal `size` cells across.
    fn lines(&self, size: TermSize) -> Vec<String> {
        let width = usize::from(size.cols);
        let rows = usize::from(size.rows);
        let mut lines = match &self.reply {
            Some(reply) => table::render(reply, self.now(), Some(width)),
            None => vec!["connecting…".to_owned()],
        };
        // The footer is the last row of the terminal, always, so a table long
        // enough to fill the screen loses its own tail rather than the line
        // that says how to leave.
        lines.truncate(rows.saturating_sub(1));
        while lines.len() + 1 < rows {
            lines.push(String::new());
        }
        lines.push(self.footer(width));
        lines
    }

    /// The last row: how to leave, where the stream is, what last moved on the
    /// queue, and anything that has happened to the stream itself.
    ///
    /// The announcement sits ahead of `stale` and the note because it is the
    /// only clause carrying a fact rather than a condition, and a 45-column
    /// window has room for about one of them. The two behind it describe a
    /// screen a reader can already see is not moving.
    fn footer(&self, width: usize) -> String {
        let mut out = String::from("q quits");
        if let Some(seq) = self.seq {
            out.push_str(&format!(" · seq {seq}"));
        }
        if let Some(said) = &self.announced {
            out.push_str(&format!(" · {}", said.line(self.now())));
        }
        if self.stale {
            out.push_str(" · stale");
        }
        if let Some(note) = self.note {
            out.push_str(&format!(" · {note}"));
        }
        out.truncate(
            out.char_indices()
                .nth(width)
                .map_or(out.len(), |(at, _)| at),
        );
        out
    }
}

/// The terminal, and what was last drawn on it.
struct Screen {
    term: TerminalGuard<std::io::Stdin, std::io::Stdout>,
    out: std::io::Stdout,
    writer: FrameWriter,
    sigwinch: Sigwinch,
    size: TermSize,
    painted: Vec<String>,
}

impl Screen {
    /// Take the terminal.
    fn enter() -> anyhow::Result<Self> {
        let term = TerminalGuard::enter(std::io::stdin(), std::io::stdout())
            .context("amx agents --watch needs a terminal on stdin")?;
        let size = term.size().context("read the terminal size")?;
        Ok(Self {
            term,
            out: std::io::stdout(),
            writer: FrameWriter::new(),
            sigwinch: Sigwinch::install().context("watch for terminal resizes")?,
            size,
            painted: Vec::new(),
        })
    }

    /// Wait for the terminal to be resized, and adopt the new size.
    async fn resized(&mut self) -> anyhow::Result<()> {
        if self.sigwinch.recv().await.is_none() {
            // The signal stream cannot close on Unix; if it ever did, waiting
            // forever is better than spinning this arm of the select.
            std::future::pending::<()>().await;
        }
        self.size = self.term.size().context("read the terminal size")?;
        // The old frame described a different terminal, so nothing on screen
        // may be trusted to still be where it was.
        self.painted.clear();
        Ok(())
    }

    /// Draw `view`, whatever is on screen.
    fn repaint(&mut self, view: &View) -> anyhow::Result<()> {
        self.painted.clear();
        self.paint_if_changed(view)
    }

    /// Draw `view` if it does not already say exactly what is on screen.
    ///
    /// This is also the age tick. Nothing schedules a repaint for the clock:
    /// the refresh window recomputes the lines every time it runs, and an age
    /// crossing from `59s` to `1m` is a line that differs, so it paints — while
    /// a session where nothing at all is happening paints not at all.
    fn paint_if_changed(&mut self, view: &View) -> anyhow::Result<()> {
        let lines = view.lines(self.size);
        if lines == self.painted {
            return Ok(());
        }
        self.writer.begin_frame();
        self.writer.set_cursor_visible(false);
        for (row, line) in lines.iter().enumerate() {
            // Only the rows that changed. A watch on a quiet session repaints
            // once a second because one age moved on, and sending the whole
            // screen for that is a kilobyte a second down the SSH link D14
            // exists for. `painted` is cleared whenever the size changes, so a
            // positional comparison is always against the same terminal.
            if self.painted.get(row) == Some(line) {
                continue;
            }
            let Ok(row) = u16::try_from(row) else { break };
            self.writer.move_to(row, 0);
            self.writer.write_raw(line.as_bytes());
            // Erase to the end of the row rather than clearing the screen
            // first: a full clear between frames is what makes a watch flicker.
            self.writer.write_raw(b"\x1b[K");
        }
        flush(&mut self.out, self.writer.bytes())?;
        self.painted = lines;
        Ok(())
    }

    /// Give the terminal back. Idempotent, like the guard beneath it.
    fn leave(&mut self) {
        self.term.restore();
        let _ = self.out.flush();
    }
}
