//! The sync seam the suite writes through: it can watch, stall, or slow down.
//!
//! U02 built [`Syncs`] so a test could assert the *ordering* of a durable
//! write. The actor suite reuses it for two more questions U02 had no reason to
//! ask: did the write happen at all (a save that found nothing changed must not
//! sync anything), and did it happen somewhere that is allowed to block? The
//! stall is how the second one is answered — a sync that stands still on a
//! channel parks a runtime thread if and only if the write was never handed to
//! the blocking pool.

use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use amx_server::persist::io::Syncs;
use tokio::sync::Notify;

/// How long a stalled sync waits before giving up and letting the assertion
/// speak. Long enough that a loaded machine does not trip it, short enough that
/// a genuinely blocked runtime fails the test instead of hanging it.
const STALL_LIMIT: Duration = Duration::from_secs(2);

/// One observed sync, in order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sync {
    /// A staging file was synced (step 3 of the durable write).
    File,
    /// A directory was synced (step 5).
    Dir,
}

/// What a [`Recorder`] saw.
#[derive(Clone, Debug, Default)]
pub struct Recorded {
    /// Every sync, in the order they happened.
    pub steps: Vec<Sync>,
    /// The most writes ever inside the seam at one moment.
    pub max_in_flight: usize,
    /// For a stalled recorder: whether the runtime made async progress while
    /// each file sync was standing still.
    pub progress_while_stalled: Vec<bool>,
    in_flight: usize,
}

impl Recorded {
    /// How many staging files were synced — one per write that happened.
    pub fn file_syncs(&self) -> usize {
        self.steps.iter().filter(|s| **s == Sync::File).count()
    }
}

/// A stall: the file sync waits here until somebody opens it.
struct Stall {
    entered: Arc<Notify>,
    open: Mutex<std::sync::mpsc::Receiver<()>>,
    progressed: Arc<AtomicBool>,
}

/// The other end of a stall.
pub struct Opener {
    entered: Arc<Notify>,
    open: std::sync::mpsc::Sender<()>,
    progressed: Arc<AtomicBool>,
}

impl Opener {
    /// Wait until a write is standing still inside the sync.
    pub async fn stalled(&self) {
        self.entered.notified().await;
    }

    /// Let it go, recording that this ran while the write was stalled.
    pub fn open(&self) {
        self.progressed.store(true, Ordering::SeqCst);
        let _ = self.open.send(());
    }
}

/// A [`Syncs`] that records the sequence, and can stall or slow a write down.
pub struct Recorder {
    state: Mutex<Recorded>,
    stall: Option<Stall>,
    slow: Option<Duration>,
}

impl Recorder {
    /// One that only watches.
    pub fn watching() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(Recorded::default()),
            stall: None,
            slow: None,
        })
    }

    /// One whose file sync blocks — really blocks, on a channel, the way an
    /// `F_FULLFSYNC` on a busy disk does — until the [`Opener`] releases it.
    pub fn stalling() -> (Arc<Self>, Opener) {
        let (tx, rx) = std::sync::mpsc::channel();
        let entered = Arc::new(Notify::new());
        let progressed = Arc::new(AtomicBool::new(false));
        let recorder = Arc::new(Self {
            state: Mutex::new(Recorded::default()),
            stall: Some(Stall {
                entered: Arc::clone(&entered),
                open: Mutex::new(rx),
                progressed: Arc::clone(&progressed),
            }),
            slow: None,
        });
        (
            recorder,
            Opener {
                entered,
                open: tx,
                progressed,
            },
        )
    }

    /// One whose file sync takes `delay`, so two overlapping writes would be
    /// visible as overlapping rather than merely possible.
    pub fn slow(delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(Recorded::default()),
            stall: None,
            slow: Some(delay),
        })
    }

    /// Everything observed so far.
    pub fn recorded(&self) -> Recorded {
        self.state.lock().unwrap().clone()
    }

    fn enter(&self, step: Sync) {
        let mut state = self.state.lock().unwrap();
        state.steps.push(step);
        state.in_flight += 1;
        state.max_in_flight = state.max_in_flight.max(state.in_flight);
    }

    fn leave(&self) {
        self.state.lock().unwrap().in_flight -= 1;
    }
}

impl Syncs for Recorder {
    fn sync_file(&self, file: &File, _path: &Path) -> io::Result<()> {
        self.enter(Sync::File);
        if let Some(stall) = &self.stall {
            stall.entered.notify_one();
            let _ = stall.open.lock().unwrap().recv_timeout(STALL_LIMIT);
            let progressed = stall.progressed.load(Ordering::SeqCst);
            self.state
                .lock()
                .unwrap()
                .progress_while_stalled
                .push(progressed);
        }
        // A window held open on purpose: it widens the moment during which a
        // *second* write would be observable inside the seam, which is what
        // makes "the actor never overlaps two writes" a real observation
        // rather than a coincidence of both being fast. Never load-bearing for
        // green — a shorter window can only make the check weaker, a longer
        // one only slower, and neither can turn a sequential actor red.
        if let Some(delay) = self.slow {
            std::thread::sleep(delay); // deliberate
        }
        let out = file.sync_all();
        self.leave();
        out
    }

    fn sync_dir(&self, dir: &File, _path: &Path) -> io::Result<()> {
        self.enter(Sync::Dir);
        let out = dir.sync_all();
        self.leave();
        out
    }
}
