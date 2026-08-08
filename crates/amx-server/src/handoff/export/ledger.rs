//! What a `Core` and its export orchestrator share about live upgrades.
//!
//! Three facts, one owner, no mailbox. They are shared state rather than
//! messages because each of them is read on a path that must not be able to
//! wait for `Core` to get round to it:
//!
//! - the **commit fence** is read by the final Persist push on the way down
//!   (R-M3-10). A message would have to arrive before that push, and the whole
//!   hazard is that the push happens while nobody is looking;
//! - the **in-flight flag** answers "is a handoff already running" for the
//!   second caller, and the orchestrator clears it from its own task;
//! - the **attempt** is what `session report` says about the last handoff, and
//!   the orchestrator is what learns it — after the connection that asked for
//!   the handoff has already been disconnected by the gateway's retirement.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use amx_proto::control::session::{HandoffAttempt, HandoffOutcome, HandoffStage};

/// The state one session keeps about handing itself over.
///
/// Held behind an `Arc` by `Core`, by the orchestrator, and by the serve
/// assembly's drain watchdog — the three places that need to agree about
/// whether ownership has moved.
#[derive(Debug, Default)]
pub struct Ledger {
    /// Armed immediately before `committed` goes out (D-M3-6 point 5).
    ///
    /// Past this line the snapshot on disk belongs to the successor, and the
    /// exporter's dying view of the session would overwrite it.
    fenced: AtomicBool,
    /// Whether an export orchestrator holds this session's panes.
    running: AtomicBool,
    /// The last attempt, as `session.report` tells it.
    attempt: Mutex<Option<HandoffAttempt>>,
}

impl Ledger {
    /// Whether persistence is fenced: ownership has transferred.
    #[must_use]
    pub fn fenced(&self) -> bool {
        self.fenced.load(Ordering::Acquire)
    }

    /// Fence persistence. Called before `committed` is written, never after.
    pub fn fence(&self) {
        self.fenced.store(true, Ordering::Release);
    }

    /// Unfence: the commit did not land, and this server goes on serving.
    ///
    /// The only failure past the fence is `commit`'s own write, and §3's table
    /// puts that on the abort side ([`HandoffError::ending`](
    /// crate::handoff::protocol::HandoffError::ending)): the importer never
    /// heard the word, so it strict-aborts and the snapshot on disk is this
    /// server's again.
    pub fn unfence(&self) {
        self.fenced.store(false, Ordering::Release);
    }

    /// Claim the session for a handoff, or report that one already has it.
    ///
    /// Compare-and-swap rather than read-then-write: two `session.handoff`
    /// calls on two connections reach `Core` one after another, but nothing in
    /// the type system says so, and a session quiesced twice is a session whose
    /// second orchestrator captures descriptors the first is already sending.
    #[must_use]
    pub fn claim(&self) -> bool {
        self.running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Give the session back: this handoff is over, whatever it did.
    pub fn release(&self) {
        self.running.store(false, Ordering::Release);
    }

    /// Record what an attempt did, for `session.report`.
    pub fn record(&self, attempt: HandoffAttempt) {
        if let Ok(mut slot) = self.attempt.lock() {
            *slot = Some(attempt);
        }
    }

    /// What the last attempt did, if this server has seen one.
    #[must_use]
    pub fn attempt(&self) -> Option<HandoffAttempt> {
        self.attempt.lock().ok().and_then(|slot| slot.clone())
    }

    /// Record a refusal that never touched a pane, and answer with its reason.
    ///
    /// The two callers are `Core`'s own arm — a second handoff, a session with
    /// no orchestrator wired — and the pre-flight probe, which is the only
    /// stage that runs before anything is frozen.
    pub fn refuse(&self, binary: PathBuf, reason: String) -> String {
        self.record(HandoffAttempt {
            outcome: HandoffOutcome::Refused,
            stage: HandoffStage::PreFlight,
            binary,
            reason: Some(reason.clone()),
        });
        reason
    }
}
