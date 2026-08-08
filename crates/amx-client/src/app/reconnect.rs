//! Riding a server swap: redial, re-`Hello` with what this client still holds,
//! and keep the screen (D-M3-7).
//!
//! Nothing in the tree reconnected before M3. A client whose socket ended read
//! `Closed` and the process exited, which was honest while the only way a
//! server went away was `session stop`. The handoff makes it routine: §3 step 11
//! retires the gateway — stop accepting, close every connection, unlink the
//! socket — and the successor binds the same path a moment later. Every attached
//! client sees a clean EOF and has a few hundred milliseconds of nothing to
//! answer for.
//!
//! # What this client presents, and what it is asserting
//!
//! The re-`Hello` carries [`Resume`]: the last bus sequence consumed, and the
//! grid generation held for each pane. The server reads both as one-shot
//! defaults (`amx-server`'s `conn::resume`), and the second is a claim:
//!
//! > Presenting a generation asserts "I hold a **complete** grid at generation
//! > G." The server cannot check it — the generation moves on resize and reset
//! > only, so agreement means the geometry still matches, not that the cells are
//! > current.
//!
//! So the judgement is this client's, and [`App::vouched`] is where it is made.
//! A pane is presented only when its cached grid was filled by a keyframe
//! ([`PaneGrid::complete`](crate::model::PaneGrid::complete)) and the dead
//! connection was not part way through a frame on that pane's stream
//! ([`Torn`]). Everything else is omitted, which costs one keyframe and buys a
//! screen that is not a guess. It is cheap to be sure about across a handoff:
//! panes are quiesced *before* the exporter retires its gateway, so a client
//! drained at disconnect time was drained against a grid nothing could still
//! move.
//!
//! # The two branches
//!
//! [`Welcome::session`] decides everything, and is read before anything else is
//! used. The same [`SessionId`] is the session continuing — its pane ids, its
//! generations and its bus sequence all still mean what this client thinks they
//! mean, so the caches stay and the presentation stays
//! ([`resync_state`](App::resync_state), the M2 reading: a reconnect is not a
//! request to look somewhere else). A different one is a different server: pane
//! ids from another id space cannot collide, but presenting a generation from
//! one would be a lie, so the caches go and it is a fresh attach
//! ([`sync_state`](App::sync_state)).
//!
//! # Order, and why
//!
//! `events.subscribe` first, naming no sequence, because that is what spends the
//! hello's `last_seq` and replays what was published while this client was gone
//! — or answers with a [`Gap`](amx_core::Delivery::Gap), which the drain below
//! turns into a resync exactly as it turns any other gap into one. Then state,
//! which re-binds the streams the dead connection took with it. Then the
//! viewport, because the successor has never heard of this terminal. The overlap
//! between the replayed events and the snapshot costs nothing: folding is
//! idempotent by construction ([`super::events`]).
//!
//! # Task ownership
//!
//! **W09** owns this file, split out of [`super::wired`] on arrival — R-M3-7
//! named the split before the code existed, that file having been at 487 lines
//! of a 500-line soft budget.

use std::io::Write;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use amx_core::{Effect, GridGeneration, PaneId, Seq, SessionId};
use amx_proto::{ClientInfo, Resume, Welcome};

use super::{App, AppError};
use crate::net::{self, NetError, Session, Torn};

/// How a reattach turned out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reattached {
    /// The same session, continued: caches kept, only stale panes repainted.
    Resumed,
    /// A different server on the same path: caches dropped, fresh attach.
    Fresh,
}

/// How long a client keeps redialling, and how hard.
///
/// The deadline is generous on purpose and the backoff is not: a handoff's
/// socket-free window is milliseconds in the ordinary case and five seconds at
/// its documented worst (`docs/09-m3-plan.md` §3), while the stage timeouts
/// that bound an *aborting* swap are thirty. A client that gave up at five
/// would turn a slow-but-successful upgrade into a lost session; one that
/// waited forever would turn a server that is never coming back into a
/// terminal nobody can get out of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ReconnectPolicy {
    /// How long to wait before the first redial.
    pub first: Duration,
    /// The longest gap between redials.
    pub longest: Duration,
    /// How long to keep trying before giving up.
    pub deadline: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            first: Duration::from_millis(20),
            longest: Duration::from_millis(250),
            deadline: Duration::from_secs(30),
        }
    }
}

impl ReconnectPolicy {
    /// The delay after `attempts` failed redials.
    #[must_use]
    fn backoff(&self, attempts: u32) -> Duration {
        // Doubling, saturating at `longest`. `2^attempts` is computed on the
        // millis rather than by multiplying `Duration`s so a long-running
        // reconnect cannot overflow into a nap nobody asked for.
        let grown = self
            .first
            .as_millis()
            .saturating_mul(1_u128 << attempts.min(16));
        Duration::from_millis(u64::try_from(grown).unwrap_or(u64::MAX)).min(self.longest)
    }
}

/// Where an attached client can redial, and as whom.
///
/// Absent for a client assembled around a connection that has no path to redial
/// — the ssh bridge hands [`App::assemble`](super::App::assemble) a socketpair
/// whose far end is a subprocess, and redialling that means respawning `ssh`,
/// which is the bridge's business and not this module's. Such a client keeps
/// M2's behavior exactly: a transport failure ends the loop.
#[derive(Clone, Debug)]
pub(super) struct Origin {
    /// The session socket to redial.
    socket: PathBuf,
    /// How this client identifies itself in the handshake.
    client: ClientInfo,
    /// The server instance this client last spoke to.
    session: SessionId,
    /// How long to keep trying.
    policy: ReconnectPolicy,
}

impl Origin {
    /// What a live attach records so it can be redialled.
    pub(super) fn new(socket: PathBuf, client: ClientInfo, session: SessionId) -> Self {
        Self {
            socket,
            client,
            session,
            policy: ReconnectPolicy::default(),
        }
    }

    /// Take a different server as the one to compare the next `Welcome`
    /// against.
    pub(super) const fn adopt(&mut self, session: SessionId) {
        self.session = session;
    }
}

impl<Fd: AsFd, W: Write> App<Fd, W> {
    /// Change how long this client keeps redialling.
    ///
    /// Only meaningful on a client that has somewhere to redial; a bridged one
    /// silently keeps its nothing.
    pub fn set_reconnect_policy(&mut self, policy: ReconnectPolicy) {
        if let Some(origin) = self.origin.as_mut() {
            origin.policy = policy;
        }
    }

    /// Whether this client can redial at all.
    #[must_use]
    pub const fn can_reconnect(&self) -> bool {
        self.origin.is_some()
    }

    /// Note that `seq` has been consumed off the event stream.
    ///
    /// The cursor only ever moves forward: a resync folds a snapshot captured
    /// at a sequence the subscription has already passed, and taking that
    /// literally would ask the successor to replay events this client has
    /// already seen — harmless, since folding is idempotent, but it would also
    /// widen the window a `Gap` is measured against for no reason.
    pub(super) const fn consumed(&mut self, seq: Seq) {
        if seq > self.cursor {
            self.cursor = seq;
        }
    }

    /// The last bus sequence this client consumed.
    #[must_use]
    pub const fn cursor(&self) -> Seq {
        self.cursor
    }

    /// How many times this client has reattached.
    ///
    /// Exposed for the same reason [`App::resyncs`](super::App::resyncs) is: a
    /// swap that the screen rode out is invisible in the rendered frame and
    /// exact in this counter.
    #[must_use]
    pub const fn reconnects(&self) -> u64 {
        self.reconnects
    }

    /// The panes whose grids this client is willing to swear are complete.
    ///
    /// The claim `Resume.generations` makes, and the only place it is decided.
    #[must_use]
    pub fn vouched(&self) -> Vec<(PaneId, GridGeneration)> {
        // A header cut in half names no channel, so nothing this connection
        // was carrying can be told apart from what it dropped.
        let torn = self.session.torn();
        if torn == Torn::Unknown {
            return Vec::new();
        }
        let mid_delta = match torn {
            Torn::Channel(channel) => self.bindings.grid_pane(channel),
            Torn::Nothing | Torn::Unknown => None,
        };
        self.model
            .panes()
            .filter(|&(pane, grid)| grid.complete() && Some(pane) != mid_delta)
            .map(|(pane, grid)| (pane, grid.generation()))
            .collect()
    }

    /// The claim this client's next `Hello` carries.
    #[must_use]
    pub fn resume_claim(&self) -> Resume {
        Resume {
            last_seq: self.cursor,
            generations: self.vouched(),
        }
    }

    /// Redial the session socket and take up where this client left off.
    ///
    /// Returns which of the two branches ran; the error is the deadline
    /// expiring with the last reason the redial failed, which is what a client
    /// with nowhere to go prints before it puts the terminal back.
    ///
    /// # Errors
    ///
    /// [`AppError::Unreachable`] when the deadline passes without a `Welcome`,
    /// or when this client has no socket to redial. Anything the resync itself
    /// fails with propagates unchanged: a successor that answered and then
    /// refused `session.state` is not something backing off would fix.
    pub async fn reconnect(&mut self) -> Result<Reattached, AppError> {
        let Some(origin) = self.origin.clone() else {
            return Err(AppError::Unreachable {
                after: Duration::ZERO,
                why: "this connection has no socket to redial".to_owned(),
            });
        };
        // Read before the old session is dropped: both halves describe the
        // connection that just died.
        let resume = self.resume_claim();
        let (session, welcome) = redial(&origin, resume).await?;
        self.session = session;
        self.reconnects += 1;
        // Everything: this terminal has been showing a frame drawn from a
        // server that is gone, and what replaces it has yet to be folded.
        self.absorb(Effect::Full);

        // Every stream binding died with the socket. The table is rebuilt from
        // nothing rather than trimmed, because channel numbers are the
        // *server's* to hand out and the successor's have no relation to the
        // ones this table holds.
        self.bindings = crate::stream::Bindings::new();

        if welcome.session == origin.session {
            self.resume_onto(&welcome).await?;
            Ok(Reattached::Resumed)
        } else {
            self.restart_onto(&welcome).await?;
            Ok(Reattached::Fresh)
        }
    }

    /// The same session, continued.
    async fn resume_onto(&mut self, welcome: &Welcome) -> Result<(), AppError> {
        // Naming no sequence is what spends the hello's `last_seq`; naming one
        // here would override the claim this connection just made.
        self.subscribe_events(None).await?;
        // Whatever the replay held is in the session's queue now, including a
        // `Gap` — which `drain_events` answers with a resync, the same answer
        // it gives every other gap.
        self.drain_events().await?;
        // Keeps this terminal's presentation (04 §3): a swap is not a request
        // to look at a different workspace. Re-binds the visible panes, which
        // is where the vouched generations are spent.
        self.resync_state().await?;
        self.report_viewport().await?;
        self.consumed(welcome.seq);
        Ok(())
    }

    /// A different server on the same path: nothing cached means anything.
    async fn restart_onto(&mut self, welcome: &Welcome) -> Result<(), AppError> {
        self.forget_session(welcome.session);
        self.cursor = welcome.seq;
        // Adopting, not keeping: there is no presentation to preserve when
        // every workspace id this terminal was showing belongs to a session
        // that is gone.
        let seq = self.sync_state().await?;
        self.subscribe_events(Some(seq)).await?;
        self.report_viewport().await?;
        self.consumed(seq);
        Ok(())
    }
}

/// Redial until a `Welcome` lands or the deadline passes.
///
/// The same `Resume` on every attempt: it describes the connection that died,
/// and nothing about waiting changes what this client holds.
async fn redial(origin: &Origin, resume: Resume) -> Result<(Session, Welcome), AppError> {
    let started = Instant::now();
    let mut attempts = 0_u32;
    let mut last: Option<NetError> = None;
    loop {
        if attempts > 0 {
            let delay = origin.policy.backoff(attempts - 1);
            // Checked against the deadline before sleeping into it, so a client
            // gives up when it said it would rather than one backoff later.
            if started.elapsed() + delay > origin.policy.deadline {
                return Err(AppError::Unreachable {
                    after: started.elapsed(),
                    why: match last {
                        Some(err) => err.to_string(),
                        None => "the session socket never answered".to_owned(),
                    },
                });
            }
            tokio::time::sleep(delay).await;
        }
        attempts += 1;

        match dial(origin, resume.clone()).await {
            Ok(pair) => return Ok(pair),
            // A refusal is not a redial: the socket answered and this build
            // and that server do not agree on a protocol, which no amount of
            // waiting fixes.
            Err(err) if !err.is_transport() => return Err(err.into()),
            Err(err) => last = Some(err),
        }
    }
}

/// One connect-and-negotiate attempt.
async fn dial(origin: &Origin, resume: Resume) -> Result<(Session, Welcome), NetError> {
    let stream = net::connect(&origin.socket).await?;
    Session::attach(stream, origin.client.clone(), true, Some(resume)).await
}
