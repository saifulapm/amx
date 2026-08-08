//! What a reattaching client brought with it.
//!
//! [`Resume`] has been on the wire since M2 and nothing read
//! it: every `Hello` in the tree sent `None`, and the server never looked. This
//! is the reader (`docs/09-m3-plan.md` D-M3-7, R-M3-12). A connection whose
//! hello carried a resume block is not a fresh attach — it is the same client
//! continuing, and the two facts it presents change what it is owed:
//!
//! - **`last_seq`** — the last bus sequence it consumed. Its event
//!   subscription opens at [`Bus::subscribe_after`](amx_core::Bus::subscribe_after)
//!   rather than at the head, so the events published while it was gone are
//!   replayed instead of skipped. A `last_seq` the replay ring has already
//!   dropped is not an error: the first delivery is a
//!   [`Gap`](amx_core::Delivery::Gap), which is the loss made visible rather
//!   than a silent hole (04 §2).
//! - **`generations`** — the grid generation it still holds per pane. A
//!   `stream.bind` for one of those panes opens delta-only if the pane's live
//!   generation agrees, and with a
//!   [`Generation`](crate::damage::KeyframeReason::Generation) keyframe if it
//!   does not (04 §6's "keyframes for stale grids").
//!
//! Both are defaults, not overrides, and both are spent when they are used. An
//! explicit `after_seq` on `events.subscribe` and an explicit `generation` on
//! `stream.bind` are the client speaking about *this* subscription or *this*
//! stream, and they win — the hello's block only answers the question nobody
//! asked, and only the first time it is asked — the two accessors below are
//! called `take_*` for that reason. A connection
//! that sent no resume therefore behaves exactly as it did before this existed,
//! which is the property the M2 goldens pin.
//!
//! # What presenting a generation asserts
//!
//! "I hold a complete grid at generation G." The server cannot check that: the
//! generation moves on resize and reset only (`GridGeneration`), so equal
//! generations mean the *geometry* is still the one the client's cells were
//! drawn against — not that the cells are current. Opening delta-only is
//! therefore correct exactly when the client's grid was complete when the
//! connection died, and a client that cannot vouch for that must omit the
//! generation (opening with a `First` keyframe, as every bind did before) or
//! ask for one with [`FlowControl::Resync`](amx_proto::FlowControl::Resync).
//! The handoff makes this cheap to be sure about: panes are quiesced before the
//! exporter retires its gateway (`docs/09-m3-plan.md` §3), so a client that was
//! drained at disconnect time was drained against a grid nothing could still
//! move.

use std::collections::HashMap;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use amx_core::{GridGeneration, PaneId, Seq};
use amx_proto::Resume;

/// The resume block one connection's hello carried, indexed for lookup.
///
/// Shared by the connection's event state and its stream bindings, which is
/// why it is behind an `Arc`: the two ask it different questions, and both
/// questions are about the same one-time claim.
#[derive(Clone, Debug, Default)]
pub struct ConnResume {
    presented: Arc<Mutex<Presented>>,
    resumed: bool,
}

/// What is left of the claim.
///
/// Both fields are *taken*, not read. The hello describes what the client held
/// at the moment it reconnected, and that describes exactly one subscription
/// and one re-bind per pane. A second `events.subscribe` naming no sequence
/// means "from here", not "rewind again"; a second `stream.bind` for a pane
/// naming no generation means the client is asking for that grid afresh, which
/// is a request for a keyframe and not a claim to already have one. Reading
/// rather than taking would answer both of those with stale news.
#[derive(Debug, Default)]
struct Presented {
    last_seq: Option<Seq>,
    generations: HashMap<PaneId, GridGeneration>,
}

impl ConnResume {
    /// A connection that presented nothing: every question falls through to
    /// the behavior a first attach gets.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Read a hello's resume block, if it had one.
    ///
    /// A duplicated pane in `generations` keeps the last entry rather than
    /// being refused; the field is a client's own bookkeeping and the wire
    /// contract is tolerance, not validation.
    #[must_use]
    pub fn from_hello(resume: Option<&Resume>) -> Self {
        match resume {
            Some(resume) => Self {
                presented: Arc::new(Mutex::new(Presented {
                    last_seq: Some(resume.last_seq),
                    generations: resume.generations.iter().copied().collect(),
                })),
                resumed: true,
            },
            None => Self::none(),
        }
    }

    /// Whether this connection is a reattach at all.
    ///
    /// Fixed for the connection's life, unlike everything the claim carries:
    /// this is for logs, not for decisions.
    #[must_use]
    pub const fn is_resume(&self) -> bool {
        self.resumed
    }

    /// Where an event subscription should open when the caller named no
    /// sequence of its own, once.
    #[must_use]
    pub fn take_after_seq(&self) -> Option<Seq> {
        self.lock().last_seq.take()
    }

    /// The generation this client said it holds for `pane`, once.
    #[must_use]
    pub fn take_generation(&self, pane: PaneId) -> Option<GridGeneration> {
        self.lock().generations.remove(&pane)
    }

    fn lock(&self) -> MutexGuard<'_, Presented> {
        // Every critical section is a take or a map removal; a panic cannot
        // happen inside one, so poisoning carries no torn state.
        self.presented
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}
