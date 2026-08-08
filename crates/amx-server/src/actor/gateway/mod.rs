//! The `Gateway` actor: one socket per session, one task per client.
//!
//! 04 §1: "One socket per session (`$XDG_RUNTIME_DIR/amx/<session>/sock`,
//! 0600). Not two, not three (fixes W3's surface sprawl). Stale-socket
//! disambiguation by connect probe."
//!
//! Both halves of that are here. The socket is created inside a `0700`
//! directory and then set to `0600`, so it is unreachable by another user even
//! during the window between `bind` and `chmod`; [`bind`] holds the rules about
//! what a socket file that already exists means, and nothing is inferred from a
//! pid file.
//!
//! Every accepted connection is spawned into a `JoinSet` this actor owns and
//! joined before [`run`](Gateway::run) returns, so "no leaked connection task"
//! is a counted fact in [`GatewayReport`] rather than a hope (04 §2).
//!
//! Split by responsibility when M3's retirement arrived (R-M3-7): this file is
//! the actor and its accept loop, [`bind`] is the socket-taking rules that a
//! retired gateway reads a second time, and [`retire`] is the mode the export
//! path drives it through.

pub mod bind;
pub mod retire;

use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use amx_core::Ctx;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub use bind::{GatewayError, RUNTIME_DIR_MODE, SOCKET_MODE};
pub use retire::{ControlError, GatewayControl, RetireReport};

use self::bind::take_socket;
use self::retire::{CONTROL_MAILBOX, GatewayCommand};
use crate::actor::{AgentHandle, CoreHandle, StatusView};
use crate::conn;

/// Observable connection accounting, cloneable and cheap to read.
#[derive(Clone, Debug, Default)]
pub struct GatewayProbe {
    live: Arc<AtomicUsize>,
    accepted: Arc<AtomicU64>,
}

impl GatewayProbe {
    /// Connections currently being served.
    #[must_use]
    pub fn live(&self) -> usize {
        self.live.load(Ordering::Acquire)
    }

    /// Connections accepted since the gateway bound.
    #[must_use]
    pub fn accepted(&self) -> u64 {
        self.accepted.load(Ordering::Acquire)
    }
}

/// Decrements the live count however its task ends, panic included.
struct LiveGuard(Arc<AtomicUsize>);

impl Drop for LiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

/// What the accept loop did before it stopped.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct GatewayReport {
    /// Connections accepted.
    pub accepted: u64,
    /// Connection tasks joined.
    ///
    /// Equal to `accepted` on a clean shutdown; a smaller number would mean a
    /// task escaped the `JoinSet`, which is why both are counted separately.
    pub joined: usize,
    /// How many of those tasks panicked.
    pub panicked: usize,
}

impl GatewayReport {
    /// Whether every accepted connection was joined and none panicked.
    #[must_use]
    pub fn clean(&self) -> bool {
        self.panicked == 0 && self.joined as u64 == self.accepted
    }
}

/// The socket surface of one session.
#[derive(Debug)]
pub struct Gateway {
    ctx: Ctx,
    core: CoreHandle,
    listener: Option<UnixListener>,
    probe: GatewayProbe,
    status: StatusView,
    agent: Option<AgentHandle>,
    /// Kept beside the receiver so the control mailbox never closes under the
    /// accept loop: this gateway stops because the session was cancelled, not
    /// because the last handle to it went away.
    control_tx: mpsc::Sender<GatewayCommand>,
    control_rx: mpsc::Receiver<GatewayCommand>,
}

impl Gateway {
    /// Take this session's socket.
    ///
    /// Must be called inside a tokio runtime. Fails rather than stealing the
    /// socket if a live server answers on it; [`bind::take_socket`] has the
    /// rules and the reasons.
    ///
    /// # Errors
    ///
    /// [`GatewayError`], one variant per way the path can already be somebody
    /// else's.
    pub fn bind(ctx: Ctx, core: CoreHandle) -> Result<Self, GatewayError> {
        let listener = take_socket(&ctx)?;
        let (control_tx, control_rx) = mpsc::channel(CONTROL_MAILBOX);
        Ok(Self {
            ctx,
            core,
            listener: Some(listener),
            probe: GatewayProbe::default(),
            status: StatusView::new(),
            agent: None,
            control_tx,
            control_rx,
        })
    }

    /// A handle on this gateway's retirement, for the export orchestrator.
    ///
    /// Taken before [`run`](Gateway::run) consumes the gateway, the way
    /// [`status_view`](Gateway::status_view) is.
    #[must_use]
    pub fn control(&self) -> GatewayControl {
        GatewayControl::new(self.control_tx.clone())
    }

    /// Hand every future connection the session's `AgentHub` mailbox.
    ///
    /// Called once, between the bind and the accept loop, because the hub
    /// cannot exist before the bind: losing the bind race must cost a returned
    /// error rather than a set of actors to tear down (`session/serve.rs`).
    /// A gateway that is never told stays honest rather than broken — its
    /// connections answer `agent.report` with `accepted: false` and every other
    /// `agent.*` row with "this session has no agent hub", which is the true
    /// state of a session assembled without one.
    ///
    /// The handle is dropped the moment the accept loop breaks, before the
    /// connection tasks are joined. It is a live sender into a bounded mailbox,
    /// and a cancelled hub drains its own mailbox *to closure* before it
    /// returns (`docs/08-m2-plan.md` §3): a sender kept alive across the join
    /// would be a mailbox that can never close, which is the shape the
    /// undiagnosed drain wedge (R-M1-2) is made of.
    pub fn set_agent(&mut self, agent: AgentHandle) {
        self.agent = Some(agent);
    }

    /// Live connection accounting.
    #[must_use]
    pub const fn probe(&self) -> &GatewayProbe {
        &self.probe
    }

    /// The [`StatusView`] every connection this gateway accepts will read.
    ///
    /// Created here rather than passed in, because it is a handle on shared
    /// state and the gateway is what hands it to connections. `AgentHub` takes
    /// a clone of *this* view at assembly and writes through it
    /// (`docs/08-m2-plan.md` §3's "update `StatusView`, then publish the
    /// event"); an empty view is exactly what a session with no hub yet has,
    /// and a wait against one simply finds no status, which is the truth.
    #[must_use]
    pub fn status_view(&self) -> StatusView {
        self.status.clone()
    }

    /// The socket this gateway is listening on.
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.ctx.socket
    }

    /// Accept connections until the session is cancelled, then join every
    /// connection task and remove the socket.
    ///
    /// A retirement (§3 step 11) is a stretch of this same loop with no
    /// listener: nothing is accepted, the control mailbox is still read, and a
    /// restore puts a fresh listener back. The `JoinSet` and the accounting
    /// span both modes, so [`GatewayReport::clean`] still means "every
    /// connection this gateway ever accepted was joined" after a handoff that
    /// aborted.
    pub async fn run(self) -> GatewayReport {
        // Destructured rather than kept as `self`, because the accept future
        // and the control mailbox are polled in the same `select!` and one of
        // them needs a unique borrow.
        let Self {
            ctx,
            core,
            mut listener,
            probe,
            status,
            mut agent,
            control_tx,
            mut control_rx,
        } = self;
        // Connections observe a child of the session token, so a fatal accept
        // error can stop the connections this gateway owns without cancelling
        // the whole session — and cancelling the session still stops them,
        // because cancelling a parent cancels its children.
        let mut clients = ctx.cancel.child_token();
        let mut tasks = JoinSet::new();
        let mut report = GatewayReport::default();

        loop {
            tokio::select! {
                () = ctx.cancel.cancelled() => break,
                Some(joined) = tasks.join_next(), if !tasks.is_empty() => {
                    account(&mut report, joined);
                }
                command = control_rx.recv() => match command {
                    // Unreachable while `control_tx` is held right here, and
                    // breaking rather than spinning is what makes that safe to
                    // be wrong about.
                    None => break,
                    Some(GatewayCommand::Retire(reply)) => {
                        let retired = retire::retire(
                            &ctx, &mut listener, &mut clients, &mut tasks, &mut report,
                        ).await;
                        let _ = reply.send(retired);
                    }
                    Some(GatewayCommand::Restore(reply)) => {
                        let restored = take_socket(&ctx).map(|taken| {
                            listener = Some(taken);
                            tracing::info!("the session socket is this server's again");
                        });
                        let _ = reply.send(restored);
                    }
                },
                accepted = accept_from(listener.as_ref()) => match accepted {
                    Ok((stream, _addr)) => {
                        report.accepted += 1;
                        probe.accepted.fetch_add(1, Ordering::Release);
                        probe.live.fetch_add(1, Ordering::Release);
                        tasks.spawn(client_task(
                            stream,
                            client_ctx(&ctx, &clients),
                            core.clone(),
                            status.clone(),
                            agent.clone(),
                            LiveGuard(Arc::clone(&probe.live)),
                        ));
                    }
                    Err(err) if transient(&err) => {
                        tracing::debug!(error = %err, "transient accept failure");
                    }
                    Err(err) => {
                        tracing::error!(error = %err, "accept failed, closing the gateway");
                        break;
                    }
                },
            }
        }

        clients.cancel();
        // Before the join, never after: see [`set_agent`](Self::set_agent). The
        // connection tasks hold their own clones and are being cancelled; this
        // one belongs to a loop that has already stopped accepting.
        drop(agent.take());
        drop(control_tx);
        while let Some(joined) = tasks.join_next().await {
            account(&mut report, joined);
        }
        // The socket is this process's to remove: a later `bind` probes what it
        // finds, so leaving it behind would make the next start pay for a
        // connect that can only be refused. A retired gateway has already
        // removed it and has no listener; removing it then would take the
        // successor's socket out from under it, which is why this asks the
        // listener first.
        if listener.is_some() {
            let _ = fs::remove_file(&ctx.socket);
        }
        report
    }
}

/// Accept on `listener`, or wait forever when there is none.
///
/// A retired gateway has no socket, and a `select!` arm that returned
/// immediately would spin the loop at the speed of the scheduler. Pending is
/// the honest answer: nothing will ever be accepted here again unless a
/// restore hands it a listener.
async fn accept_from(
    listener: Option<&UnixListener>,
) -> io::Result<(UnixStream, tokio::net::unix::SocketAddr)> {
    match listener {
        Some(listener) => listener.accept().await,
        None => std::future::pending().await,
    }
}

/// The context a connection task runs under: this session's paths and bus,
/// with the gateway's own client cancellation token.
fn client_ctx(ctx: &Ctx, clients: &CancellationToken) -> Ctx {
    Ctx {
        cancel: clients.clone(),
        ..ctx.clone()
    }
}

async fn client_task(
    stream: UnixStream,
    ctx: Ctx,
    core: CoreHandle,
    status: StatusView,
    agent: Option<AgentHandle>,
    guard: LiveGuard,
) {
    let _guard = guard;
    if let Err(err) = conn::serve(stream, ctx, core, status, agent).await {
        tracing::debug!(error = %err, "connection ended");
    }
}

fn account(report: &mut GatewayReport, joined: Result<(), tokio::task::JoinError>) {
    report.joined += 1;
    if joined.is_err() {
        report.panicked += 1;
    }
}

/// Whether an accept failure is worth retrying rather than closing over.
fn transient(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::ConnectionAborted | io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
    )
}
