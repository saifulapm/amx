//! The rig the actor suite drives: a stand-in `Core`, stand-in panes, and the
//! builder that wires a real `Persist` to both.
//!
//! Everything the actor talks to is a seam, which is what makes the suite
//! honest without a running session: `Core` is an ordinary mailbox receiver (so
//! "the capture went through the mailbox" is not an assertion but the only way
//! the request can arrive at all), panes are ordinary [`PaneHandle`]s, and the
//! two `File::sync_all` calls of the durable write are the [`Recorder`] next
//! door. The one thing no seam fakes is the write itself: snapshots land in a
//! real state directory through U02's `write_atomic`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use amx_core::{
    Bus, Config, Ctx, Event, GridGeneration, PaneId, PersistConfig, SessionName, ShortNumber,
    WorkspaceId,
};
use amx_proto::stream::history::put_row;
use amx_server::actor::persist::{PERSIST_MAILBOX, Persist, PersistHandle, PersistReport};
use amx_server::actor::{
    Capture, CoreCommand, CoreHandle, HistoryRows, PaneCommand, PaneHandle, SessionCall,
};
use amx_server::persist::{PaneSnapshot, PersistError, Snapshot, VERSION, snapshot};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::seam::Recorder;
use crate::support::TempDir;

// ------------------------------------------------------------- the fake Core

/// What the stand-in `Core` was asked for.
#[derive(Clone, Debug, Default)]
pub struct Seen {
    /// The `sidecars` flag of every capture request, in order.
    pub captures: Vec<bool>,
    /// Anything else that arrived on the `Core` mailbox — nothing should.
    pub others: usize,
}

/// A window onto the stand-in `Core`.
#[derive(Clone, Debug)]
pub struct Spy {
    seen: Arc<Mutex<Seen>>,
    count: watch::Receiver<usize>,
}

impl Spy {
    /// Everything the stand-in `Core` has been asked so far.
    pub fn seen(&self) -> Seen {
        self.seen.lock().unwrap().clone()
    }

    /// Wait until `n` captures have been requested.
    pub async fn captures(&self, n: usize) {
        let mut count = self.count.clone();
        while *count.borrow() < n {
            count
                .changed()
                .await
                .expect("the stand-in core outlives the wait");
        }
    }
}

/// How the stand-in `Core` answers each capture.
#[derive(Clone, Debug)]
pub enum Answers {
    /// A snapshot one pane larger every time, so no two saves are the same
    /// bytes.
    Growing,
    /// The same snapshot every time — the unchanged-session case.
    Same,
    /// Growing, plus these pane handles whenever sidecars were asked for.
    WithPanes(Vec<(PaneId, PaneHandle)>),
}

impl Answers {
    fn answer(&self, nth: usize, sidecars: bool) -> Capture {
        match self {
            Self::Growing => Capture::without_sidecars(snapshot_of(nth + 1)),
            Self::Same => Capture::without_sidecars(snapshot_of(1)),
            Self::WithPanes(panes) => Capture {
                snapshot: Box::new(snapshot_of(nth + 1)),
                panes: if sidecars { panes.clone() } else { Vec::new() },
            },
        }
    }
}

/// A snapshot holding `panes` panes, distinct for every count.
pub fn snapshot_of(panes: usize) -> Snapshot {
    Snapshot {
        version: VERSION,
        focused_workspace: None,
        workspaces: Vec::new(),
        panes: (0..panes)
            .map(|n| PaneSnapshot {
                id: pane_id(n),
                short: ShortNumber::new(u32::try_from(n).unwrap() + 1),
                label: None,
                cwd: None,
                argv: None,
                agent: None,
            })
            .collect(),
    }
}

/// The `n`th pane of the fixture session, stable across runs.
pub fn pane_id(n: usize) -> PaneId {
    format!("00000000-0000-0000-0000-{:012x}", n + 1)
        .parse()
        .unwrap()
}

/// The workspace the fixture events name.
pub fn workspace_id() -> WorkspaceId {
    "00000000-0000-0000-0000-0000000000b1".parse().unwrap()
}

/// One structural event, of the kind that must schedule a save.
pub fn structural() -> Event {
    Event::LayoutChanged {
        workspace: workspace_id(),
    }
}

// ------------------------------------------------------------- fake panes

/// A stand-in pane actor: answers `HistoryRange` and counts the asking.
pub struct FakePane {
    /// The handle a capture hands to the actor.
    pub handle: PaneHandle,
    asked: Arc<AtomicUsize>,
    _task: JoinHandle<()>,
}

impl FakePane {
    /// One that serves `rows` for every range it is asked for.
    pub fn serving(rows: &[(&str, bool)]) -> Self {
        let packed = pack(rows);
        let (tx, mut rx) = mpsc::channel(8);
        let asked = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&asked);
        let task = tokio::spawn(async move {
            while let Some(command) = rx.recv().await {
                if let PaneCommand::HistoryRange { range, reply } = command {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let _ = reply.send(Ok(HistoryRows {
                        range,
                        rows: packed.clone(),
                    }));
                }
            }
        });
        Self {
            handle: PaneHandle::new(tx),
            asked,
            _task: task,
        }
    }

    /// How many history ranges it has been asked for.
    pub fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}

/// Pack rows the way the history stream does, so the file and the wire agree.
pub fn pack(rows: &[(&str, bool)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (text, wrapped) in rows {
        put_row(text.as_bytes(), *wrapped, &mut out);
    }
    out
}

// ------------------------------------------------------------------ the rig

/// A running `Persist` actor with everything it talks to under test control.
pub struct Rig {
    /// The session context — its bus is what a test publishes on, its
    /// cancellation token what a test uses to shut the actor down.
    pub ctx: Ctx,
    /// The actor's mailbox.
    pub persist: PersistHandle,
    /// What the stand-in `Core` has been asked.
    pub spy: Spy,
    /// The live config the actor reads `[persist]` from.
    pub config: watch::Sender<Config>,
    state_dir: PathBuf,
    actor: JoinHandle<PersistReport>,
}

/// Builds a [`Rig`].
pub struct Builder {
    root: PathBuf,
    replay: usize,
    history: bool,
    answers: Answers,
    syncs: Option<Arc<Recorder>>,
    prelude: Box<dyn FnOnce(&Bus)>,
}

impl Rig {
    /// A rig whose state directory lives under `root`.
    pub fn under(root: &TempDir) -> Builder {
        Builder {
            root: root.path().to_path_buf(),
            replay: 64,
            history: false,
            answers: Answers::Growing,
            syncs: None,
            prelude: Box::new(|_| {}),
        }
    }

    /// Publish one event on the session bus.
    pub fn publish(&self, event: Event) {
        self.ctx.bus.publish(event);
    }

    /// Force a save and wait for it to land.
    pub async fn flush(&self) -> Result<(), PersistError> {
        self.persist
            .flush()
            .await
            .expect("the actor answers a flush")
    }

    /// Where snapshots and sidecars go.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// The snapshot currently on disk, if any.
    pub fn on_disk(&self) -> Option<Snapshot> {
        snapshot::load(&self.state_dir).expect("the snapshot on disk parses")
    }

    /// Cancel the session, close the mailbox, and join the actor.
    pub async fn stop(self) -> PersistReport {
        self.ctx.cancel.cancel();
        drop(self.persist);
        self.actor.await.expect("the persist actor must not panic")
    }
}

impl Builder {
    /// How many events the bus's replay buffer holds.
    pub fn replay(mut self, events: usize) -> Self {
        self.replay = events;
        self
    }

    /// Start with `[persist] history` on.
    pub fn history(mut self, on: bool) -> Self {
        self.history = on;
        self
    }

    /// How the stand-in `Core` answers captures.
    pub fn answers(mut self, answers: Answers) -> Self {
        self.answers = answers;
        self
    }

    /// Write through `syncs` rather than plain `File::sync_all`.
    pub fn syncs(mut self, syncs: Arc<Recorder>) -> Self {
        self.syncs = Some(syncs);
        self
    }

    /// Run `prelude` against the bus after the actor's subscription is taken
    /// but before the actor is spawned — which is how a subscription is made
    /// to fall behind its replay buffer.
    pub fn prelude(mut self, prelude: impl FnOnce(&Bus) + 'static) -> Self {
        self.prelude = Box::new(prelude);
        self
    }

    /// Spawn the actor and everything it talks to.
    pub fn start(self) -> Rig {
        let ctx = Ctx {
            session: SessionName::new("test").expect("valid session name"),
            runtime_dir: self.root.join("run"),
            socket: self.root.join("run/sock"),
            state_dir: self.root.join("state"),
            config_path: self.root.join("config/amx/config.toml"),
            bus: Arc::new(Bus::new(self.replay)),
            cancel: CancellationToken::new(),
        };

        let (core_tx, core_rx) = mpsc::channel(64);
        let spy = spawn_core(core_rx, self.answers);

        let config = Config {
            persist: PersistConfig {
                history: self.history,
            },
            ..Config::default()
        };
        let (config_tx, config_rx) = watch::channel(config);

        // Taken before the prelude runs: a subscription positioned here and a
        // bus flooded afterwards is a subscriber that fell behind.
        let events = ctx.bus.subscribe();
        (self.prelude)(&ctx.bus);

        let mut actor = Persist::new(ctx.clone(), CoreHandle::new(core_tx), config_rx);
        if let Some(syncs) = self.syncs {
            actor = actor.with_syncs(syncs);
        }

        let (persist_tx, persist_rx) = mpsc::channel(PERSIST_MAILBOX);
        let state_dir = ctx.state_dir.clone();
        let handle = tokio::spawn(actor.run(persist_rx, events));

        Rig {
            ctx,
            persist: PersistHandle::new(persist_tx),
            spy,
            config: config_tx,
            state_dir,
            actor: handle,
        }
    }
}

/// Run a stand-in `Core` over `mailbox`, answering captures from `answers`.
fn spawn_core(mut mailbox: mpsc::Receiver<CoreCommand>, answers: Answers) -> Spy {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let log = Arc::clone(&seen);
    let (count_tx, count_rx) = watch::channel(0);
    tokio::spawn(async move {
        let mut captures = 0;
        while let Some(command) = mailbox.recv().await {
            match command {
                CoreCommand::Session(SessionCall::Capture { sidecars, reply }) => {
                    let nth = captures;
                    captures += 1;
                    log.lock().unwrap().captures.push(sidecars);
                    let _ = reply.send(answers.answer(nth, sidecars));
                    let _ = count_tx.send(captures);
                }
                _ => log.lock().unwrap().others += 1,
            }
        }
    });
    Spy {
        seen,
        count: count_rx,
    }
}

/// Deliver a burst of non-structural events, for a subscription to miss.
pub fn flood(bus: &Bus, events: usize) {
    for n in 0..events {
        bus.publish(Event::PaneDamage {
            pane: pane_id(n),
            generation: GridGeneration::FIRST,
        });
    }
}
