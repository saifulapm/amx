//! The `AgentHub` actor: the fifth of 04 §2's five, assembled from the agent
//! layer's pure parts.
//!
//! `docs/08-m2-plan.md` §3 is the specification, and everything below is one of
//! its sentences:
//!
//! - **Owns** the parsed registry, a [`Tracker`] per pane with its
//!   [`SnapshotFeed`], the compiled manifests, the attention queue, the
//!   session-ref table, the per-pane hook tokens and the deadline wheel.
//! - **Is handed** its feeds. `Core` owns the `PaneHost`s and is the only place
//!   a feed can be cloned from, so [`AgentCommand::PaneStarted`] brings one and
//!   nothing here ever asks for one — an ask would be the sibling request the
//!   shutdown discipline forbids.
//! - **Publishes** `agent_status`, `agent_identified`, `attention_enqueued` and
//!   `attention_dequeued`, and is their only publisher (04 §2's one-publisher
//!   rule, read per event kind — the pane actor owns the pane-thread kinds and
//!   `Core` the rest, `docs/09-m3-plan.md` D-M3-2).
//! - **Times** off one wheel, armed only while a deadline exists. There is no
//!   idle tick anywhere in this module: a session of idle agents costs zero
//!   wakeups (03 §5), unlike herdr's permanent 300–500 ms scan.
//!
//! # The two read models
//!
//! The ordering that keeps waits honest lives in [`StatusView::commit`], and
//! this is its only caller. Every transition here is *one* call: the view write
//! and the events it announces go in together, so the publish can never
//! overtake the write a woken waiter is about to read (§3, and the hang it
//! describes).
//!
//! The second consumer is `Core`'s mirror, filled with
//! [`AgentCall`](crate::actor::AgentCall) and `try_send`. It is deliberately
//! slower and deliberately fire-and-forget: `session.state`, the status line
//! and the final capture read it, nothing awaits on it, and because it is
//! filled during normal operation the hub's shutdown has nothing to flush.
//!
//! # Shutdown
//!
//! `Persist`'s discipline to the letter, under the standing wedge-flake
//! constraint (R-M1-2, still undiagnosed): after cancellation the hub arms no
//! timer, publishes nothing, evaluates nothing, and sends nothing to any
//! sibling. It drains its own mailbox until it closes, answering what it can
//! answer without leaving the module, and returns. Detection state is derived
//! and dies with the process.
//!
//! # The files
//!
//! | Module | What lives there |
//! |---|---|
//! | this one | the hub's state, its construction, and a pane's lifecycle |
//! | [`run`] | the loop, the mailbox, the bus, the wheel, the drain |
//! | [`commit`] | one transition folded into the queue and both read models |
//! | [`inherit`] | a predecessor's statuses, taken across a live upgrade |
//! | [`names`] | the labels an attention event carries, and where from |
//! | [`probe`] | the counters a test watches and the report a run returns |
//! | [`detect`] | tier-2 scheduling and evaluation against the published frame |
//! | [`verbs`] | what `agent.next`, `agent.explain` and a refusal answer |

pub mod commit;
pub mod detect;
pub mod inherit;
mod names;
mod probe;
pub mod run;
pub mod verbs;

pub use probe::{AgentProbe, AgentReport};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use amx_core::agent::{AgentKind, AgentSnapshot, EpochMillis, HookToken, SessionRef};
use amx_core::{Ctx, PaneId, Seq, WorkspaceId};
use amx_proto::control::agent as proto;
use tokio::time::Instant;

use crate::actor::{CoreHandle, SnapshotFeed, SpawnedIdentity, StatusView};
use crate::agent::fusion::{Deadline, IDENTITY_GRACE, Input, Tracker};
use crate::agent::identity::program_named;
use crate::agent::manifest::{Manifest, ScreenBuf};
use crate::agent::registry::{AgentStanza, Registry};

/// Where a user's manifest overrides live, beside their `config.toml`.
///
/// Derived from [`Ctx::config_path`] the way
/// [`Registry::override_path`] is, so the registry override, the manifest
/// overrides and the configuration are guaranteed to be one directory's — a
/// test pointing `Ctx` at a tempdir moves all three together. A file in here
/// shadows the bundled manifest of the same name; the directory is re-scanned
/// on the config watcher's reload event, which is as far as M2 takes hot
/// reloading (R-M2-13 — a remote catalog is M4's).
pub const MANIFEST_DIR: &str = "manifests";

/// One pane the hub is tracking.
///
/// The feed, the machine, and the bookkeeping that turns a stream of damage
/// into a bounded number of evaluations.
#[derive(Debug)]
struct Tracked {
    /// The pane's published frames, handed over at `PaneStarted`.
    frames: SnapshotFeed,
    /// V04's state machine for this pane.
    tracker: Tracker,
    /// The token this pane's child was spawned with. A report carrying any
    /// other value is not about this pane (D-M2-4's misattribution guard).
    token: HookToken,
    /// The argv `Core` spawned, kept for the identity guess and for
    /// `agent.explain`.
    argv: Vec<String>,
    /// The compiled manifest for whatever agent was identified here.
    manifest: Option<Arc<Manifest>>,
    /// The rule buffer, reused so a pane evaluated a thousand times allocates
    /// once (HACKING.md's hot-path rule, kept where the frames are).
    screen: ScreenBuf,
    /// The last title the application set, for the `title` region.
    title: String,
    /// The conversation this pane is in, as the last hook said.
    session_ref: Option<SessionRef>,
    /// The bus sequence of this pane's last status transition.
    transition_seq: Seq,
    /// The wall clock at that same transition, in epoch milliseconds.
    ///
    /// The renderable half of [`transition_seq`](Self::transition_seq), stamped
    /// here and not inside the fusion machine because the machine has no clock
    /// and is not getting one: deadlines *arrive* as inputs, which is what lets
    /// V04's property tests enumerate interleavings instead of running them.
    /// `absorb` already reads a clock for the wheel, so the stamp costs one
    /// more `SystemTime::now`.
    ///
    /// `None` until this hub has seen the pane move. D-M4-4's absolute instant
    /// is only honest when somebody observed the edge, and there are exactly
    /// two ways a pane has one: this process watched it happen, or a
    /// predecessor did and handed it over on the handoff manifest. A **cold
    /// restore**
    /// gives neither — the persist snapshot deliberately carries no status
    /// ("never a status, which dies with the process it described",
    /// `crates/amx-server/src/persist/mod.rs:94`) — so a restored pane starts
    /// with no `since` and takes one from its first real transition, rather
    /// than reporting the restore as the moment an agent blocked. R-M4-4 asks
    /// for that answer said out loud: this is it, and the fallback it offers
    /// ("since this server started tracking it") is declined, because an age
    /// counted from a restart is a number that looks measured and is not.
    since: Option<EpochMillis>,
    /// When this pane was last evaluated, which is what the minimum spacing is
    /// measured from.
    last_eval: Option<Instant>,
    /// A damage batch that arrived inside the spacing and is owed an
    /// evaluation. One slot, not a queue: the evaluation reads the *current*
    /// frame, so a hundred coalesced batches and one are the same work.
    eval_due: Option<Instant>,
    /// The deadlines the wheel is holding for this pane.
    deadlines: HashMap<Deadline, Instant>,
}

impl Tracked {
    fn new(frames: SnapshotFeed, spawn: SpawnedIdentity, seq: Seq) -> Self {
        Self {
            frames,
            tracker: Tracker::new(),
            token: spawn.token,
            argv: spawn.argv,
            manifest: None,
            screen: ScreenBuf::new(),
            title: String::new(),
            session_ref: None,
            transition_seq: seq,
            since: None,
            last_eval: None,
            eval_due: None,
            deadlines: HashMap::new(),
        }
    }

    /// This pane's status, as both read models carry it.
    fn snapshot(&self) -> AgentSnapshot {
        AgentSnapshot {
            kind: self.tracker.kind.clone(),
            state: self.tracker.state,
            cause: self.tracker.cause,
            transition_seq: self.transition_seq,
            // Derived from the queue on read, never stored: a dequeue that
            // shifts everyone behind it must not leave a stale number here.
            attention: None,
            session_ref: self.session_ref.clone(),
            // Both halves of the entry edge, and both of them somebody's
            // observation rather than this line's invention: the name comes off
            // the machine, which took it from the detector that won, and the
            // instant comes off `absorb`, which stamped it when the transition
            // was folded (D-M4-3, D-M4-4).
            reason: self.tracker.reason.clone(),
            since: self.since,
        }
    }

    /// The soonest this pane owes the wheel anything.
    fn due(&self) -> Option<Instant> {
        self.deadlines.values().copied().chain(self.eval_due).min()
    }
}

/// The hub.
///
/// One per session, built before the runtime spawns it and consumed by
/// [`AgentHub::run`].
#[derive(Debug)]
pub struct AgentHub {
    ctx: Ctx,
    core: CoreHandle,
    view: StatusView,
    registry: Registry,
    /// Compiled manifests by file name, so five Claude panes share one.
    manifests: HashMap<String, Arc<Manifest>>,
    panes: HashMap<PaneId, Tracked>,
    /// Statuses a predecessor server carried across a handoff, waiting for the
    /// panes they belong to (R-M3-13).
    ///
    /// Emptied one entry at a time by [`AgentHub::track`], because a status is
    /// only half of a tracked pane: the other half is the frame feed, and only
    /// `Core` can hand one over.
    inherited: HashMap<PaneId, AgentSnapshot>,
    /// The attention queue, in block order (D-M2-8). A re-block re-enters at
    /// the tail, which is why an enqueue removes any existing entry first.
    attention: Vec<PaneId>,
    /// Which workspace each pane was created in, for `agent.next`'s reply.
    workspaces: HashMap<PaneId, WorkspaceId>,
    /// The labels the identity block on an attention event needs.
    names: names::Names,
    probe: AgentProbe,
}

impl AgentHub {
    /// A hub for the session `ctx` names, mirroring into `core` and writing
    /// `view`.
    ///
    /// The registry is parsed here, once, at assembly: the builtin
    /// `agents.toml` merged with the user's override if they have one
    /// (D-M2-2). A malformed override costs its own stanzas and nothing else —
    /// the builtins stay exactly as they were — and says so in the log, which
    /// is M1's per-section lenient config rule applied to the same kind of
    /// file.
    #[must_use]
    pub fn new(ctx: Ctx, core: CoreHandle, view: StatusView) -> Self {
        let registry = load_registry(&ctx.config_path);
        Self {
            ctx,
            core,
            view,
            registry,
            manifests: HashMap::new(),
            panes: HashMap::new(),
            inherited: HashMap::new(),
            attention: Vec::new(),
            workspaces: HashMap::new(),
            names: names::Names::default(),
            probe: AgentProbe::new(),
        }
    }

    /// Run against `registry` instead of the one on disk.
    ///
    /// The seam D-M2-2 named: "the override merge is also the test seam", so a
    /// suite plants a stanza for a scripted agent rather than patching the
    /// binary. Production never calls it.
    #[must_use]
    pub fn with_registry(mut self, registry: Registry) -> Self {
        self.registry = registry;
        self
    }

    /// A window onto this hub's counters, readable while it runs.
    #[must_use]
    pub fn probe(&self) -> AgentProbe {
        self.probe.clone()
    }

    /// What this hub has done so far.
    fn report(&self) -> AgentReport {
        AgentReport {
            reports: self.probe.reports(),
            dropped: self.probe.dropped(),
            evaluations: self.probe.evaluations(),
            wakeups: self.probe.wakeups(),
            transitions: self.probe.transitions(),
            tracked: self.panes.len(),
        }
    }

    // ------------------------------------------------------------- lifecycle

    /// Start tracking `pane`, with the frames and the spawn identity `Core`
    /// handed over.
    fn track(&mut self, pane: PaneId, frames: SnapshotFeed, spawn: SpawnedIdentity) {
        if let Some(carried) = self.inherited.remove(&pane) {
            self.adopt(pane, frames, spawn, &carried);
            return;
        }
        let seq = self.ctx.bus.head();
        let requested = spawn.requested.clone();
        let tracked = Tracked::new(frames, spawn, seq);
        let argv_guess = program_named(&tracked.argv).map(str::to_owned);
        self.panes.insert(pane, tracked);
        // Tier 3 without a process tree: `agent start` says which agent it
        // asked for, and a pane spawned as `claude` says so in its own argv.
        // Neither can see a `claude` *typed into a shell* — that one is named
        // by its first hook report, or by the foreground-job probe V13 wires to
        // its readiness handshake.
        let kind = requested.or_else(|| {
            let program = argv_guess?;
            self.registry
                .stanzas()
                .iter()
                .find(|stanza| stanza.executables.contains(&program))
                .map(|stanza| stanza.id.clone())
        });
        if let Some(kind) = kind {
            self.identify(pane, &kind);
        }
    }

    /// Retire `pane`'s tracker: it left the layout, or its process ended.
    ///
    /// A queue holding a dead pane would send `next-attention` somewhere that
    /// is not there, so the exit runs through the machine like any other input
    /// and its `Dequeue` directive is honoured on the way out.
    fn retire(&mut self, pane: PaneId) {
        if !self.panes.contains_key(&pane) {
            return;
        }
        let directives = self.apply(pane, Input::Exited);
        self.absorb(pane, &directives);
        // `absorb` drops an exited tracker; this catches the pane that was
        // already exited when the close arrived.
        self.panes.remove(&pane);
        self.workspaces.remove(&pane);
        // After `absorb`, not before: the dequeue this exit publishes still
        // names the pane a person would recognise.
        self.names.forget_pane(pane);
    }

    /// Name `pane`'s agent and adopt its stanza's fusion parameters.
    ///
    /// The class and the grace land in the same step as the identity, through
    /// [`Tracker::identify`], because a window in which a pane is known to be
    /// `claude` and still ignoring its hooks is a window in which its status is
    /// wrong. The manifest follows from the `Identified` effect, in
    /// [`Self::absorb`].
    fn identify(&mut self, pane: PaneId, kind: &AgentKind) {
        let Some(stanza) = self.registry.resolve(kind) else {
            return;
        };
        let (id, coverage, grace) = (stanza.id.clone(), stanza.coverage, startup_grace(stanza));
        let Some(tracked) = self.panes.get_mut(&pane) else {
            return;
        };
        let effects = tracked.tracker.identify(id, coverage, grace);
        self.absorb(pane, &effects);
    }

    /// The stanza a hook report claims, if the report is entitled to claim it.
    ///
    /// D-M2-7's source allowlist at the first of its three gates: a ref is only
    /// accepted from the hook path of the agent it claims, cross-checked
    /// against the stanza. A report whose `source` names a different agent than
    /// its `agent` field is not a report about anything.
    fn claimed(&self, report: &proto::ReportParams) -> Option<&AgentStanza> {
        let stanza = self.registry.resolve(&report.agent)?;
        (report.source.agent() == stanza.id).then_some(stanza)
    }

    /// Whether `report` is about the pane it names.
    ///
    /// Not a security boundary — the socket is 0600 and any same-user process
    /// can already drive every pane — but a misattribution guard: a stale hook
    /// config in a nested or foreign session, or a pane id recycled across
    /// restarts, reports into the void instead of into the wrong pane's status
    /// (D-M2-4).
    fn token_matches(&self, pane: PaneId, token: &HookToken) -> bool {
        self.panes
            .get(&pane)
            .is_some_and(|tracked| &tracked.token == token)
    }
}

/// The registry this session runs on: the builtin, plus the user's override.
fn load_registry(config_path: &Path) -> Registry {
    let builtin = Registry::builtin();
    let path = Registry::override_path(config_path);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return builtin;
    };
    let (merged, diagnostics) = builtin.with_override(&text);
    for diagnostic in diagnostics {
        tracing::warn!(path = %path.display(), "{diagnostic}");
    }
    merged
}

/// Compile the manifest called `name`: the user's override if they have one,
/// the bundled copy otherwise.
fn load_manifest(config_path: &Path, name: &str) -> Option<Manifest> {
    let path = manifest_dir(config_path).join(name);
    if let Ok(text) = std::fs::read_to_string(&path) {
        match Manifest::parse_override(&text, &path) {
            Ok(manifest) => return Some(manifest),
            Err(err) => {
                // The bundled manifest below is a working detector; refusing to
                // detect at all because a user's edit does not compile would
                // turn a typo into a broken status line.
                tracing::warn!(path = %path.display(), error = %err, "manifest override ignored");
            }
        }
    }
    match Manifest::load_bundled(name) {
        Ok(manifest) => Some(manifest),
        Err(err) => {
            tracing::warn!(manifest = name, error = %err, "no screen rules for this agent");
            None
        }
    }
}

/// Where manifest overrides live.
fn manifest_dir(config_path: &Path) -> PathBuf {
    config_path.with_file_name(MANIFEST_DIR)
}

/// How long after identification this agent's screen is worth reading.
///
/// Per-stanza because V01 §6 measured the two shipped agents differing enough
/// to matter — Claude Code is ready about 1.1 s after launch, Codex took
/// 2.8–4.6 s — with [`IDENTITY_GRACE`] as the fallback for a stanza that says
/// nothing.
fn startup_grace(stanza: &AgentStanza) -> Duration {
    stanza
        .startup_grace_ms
        .map_or(IDENTITY_GRACE, Duration::from_millis)
}
