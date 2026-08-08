//! Retirement: the gateway's half of the socket changing hands (§3 steps
//! 11–12).
//!
//! A live upgrade hands the session socket to a successor process, and the
//! exporter's half of that is to stop accepting, close the connections it has,
//! and unlink the file — while going on running, because the swap can still be
//! aborted and this same gateway then has to answer again.
//!
//! That is why this is a *mode* and not a second gateway. The connection
//! accounting, the `JoinSet`, the `StatusView` and the hub handle all survive a
//! retirement, so a session that aborted a handoff is the session it was rather
//! than a rebuilt one that happens to look similar — and
//! [`GatewayReport::clean`](super::GatewayReport::clean) still means "every
//! connection this gateway ever accepted was joined" across both modes.

use std::fs;
use std::time::Duration;

use amx_core::Ctx;
use thiserror::Error;
use tokio::net::UnixListener;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::{GatewayError, GatewayReport, account};

/// How many control messages may queue for a running gateway.
///
/// One handoff runs at a time and asks for at most two transitions, so this is
/// slack rather than a queue.
pub(super) const CONTROL_MAILBOX: usize = 4;

/// How long a retirement waits for its cancelled connections before it stops
/// counting.
///
/// The successor is waiting for this socket path, so a connection that will not
/// return promptly must not hold the swap open. It is not abandoned — it stays
/// in the same `JoinSet` and is accounted whenever it does return, or by the
/// final join — but the retirement stops waiting for it, and
/// [`RetireReport::outstanding`] says how many were left.
const RETIRE_JOIN_BOUND: Duration = Duration::from_secs(5);

/// What retiring the session socket did (§3 step 11).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct RetireReport {
    /// Whether the socket file was removed, so the successor may bind it.
    pub unlinked: bool,
    /// Connection tasks joined during the retirement.
    pub joined: usize,
    /// Connection tasks still running when the join bound expired.
    ///
    /// Zero on every ordinary retirement. A non-zero count is the near miss the
    /// drain wedge is made of (`docs/notes/m3-shutdown-wedge.md` §4) seen early,
    /// and the handoff goes on regardless — the successor is waiting for the
    /// socket, not for this process's bookkeeping.
    pub outstanding: usize,
}

/// What the export path asks of a running gateway.
#[derive(Debug)]
pub(super) enum GatewayCommand {
    /// Stop accepting, close the connections, unlink the socket file.
    Retire(oneshot::Sender<RetireReport>),
    /// Take the socket back, because the handoff aborted.
    Restore(oneshot::Sender<Result<(), GatewayError>>),
}

/// A gateway transition could not be asked for, or could not be done.
#[derive(Debug, Error)]
pub enum ControlError {
    /// The accept loop has already stopped.
    #[error("the session gateway is not running")]
    Gone,
    /// The socket could not be taken back.
    #[error(transparent)]
    Bind(#[from] GatewayError),
}

/// A handle on retiring and restoring one session's socket.
///
/// Cloneable and cheap, like every other mailbox handle here. Held by the export
/// orchestrator and by nothing else: retiring the socket of a session that is
/// not handing itself over would be a session that stopped answering for no
/// reason.
#[derive(Clone, Debug)]
pub struct GatewayControl {
    tx: mpsc::Sender<GatewayCommand>,
}

impl GatewayControl {
    /// Wrap the sending half of a gateway's control mailbox.
    pub(super) const fn new(tx: mpsc::Sender<GatewayCommand>) -> Self {
        Self { tx }
    }

    /// §3 step 11: stop accepting, close connections, unlink the socket file.
    ///
    /// Returns once the path is free, which is what the successor's probe loop
    /// is waiting for.
    ///
    /// # Errors
    ///
    /// [`ControlError::Gone`] if the accept loop has already stopped, which
    /// means the session is shutting down and there is nothing to hand over.
    pub async fn retire(&self) -> Result<RetireReport, ControlError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(GatewayCommand::Retire(tx))
            .await
            .map_err(|_| ControlError::Gone)?;
        rx.await.map_err(|_| ControlError::Gone)
    }

    /// Take the socket back after an abort, and answer on it again.
    ///
    /// herdr's restore-sockets move (§3's 11–12 row), and the reason the gateway
    /// retires into a mode rather than into nothing. It can fail: the successor
    /// may have bound the path and then died without committing, and a bind that
    /// would steal a live server's socket is refused by
    /// [`take_socket`](super::bind::take_socket)'s own probe. That failure is
    /// reported, never worked around.
    ///
    /// # Errors
    ///
    /// [`ControlError::Gone`] as above, or [`ControlError::Bind`] with the
    /// reason the path could not be taken.
    pub async fn restore(&self) -> Result<(), ControlError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(GatewayCommand::Restore(tx))
            .await
            .map_err(|_| ControlError::Gone)?;
        rx.await
            .map_err(|_| ControlError::Gone)?
            .map_err(Into::into)
    }
}

/// §3 step 11: stop accepting, close the connections, unlink the socket file.
///
/// In that order, and the join last: the successor's probe loop is waiting for
/// the path, and a connection that is slow to notice it has been cancelled must
/// not hold the swap open ([`RETIRE_JOIN_BOUND`]).
///
/// The client token is replaced rather than reused. It has just been cancelled,
/// and a cancelled token cancels everything given it afterwards — a restored
/// gateway handing that token to a new connection would be a gateway that
/// accepts and immediately hangs up.
pub(super) async fn retire(
    ctx: &Ctx,
    listener: &mut Option<UnixListener>,
    clients: &mut CancellationToken,
    tasks: &mut JoinSet<()>,
    report: &mut GatewayReport,
) -> RetireReport {
    drop(listener.take());
    clients.cancel();
    let unlinked = fs::remove_file(&ctx.socket).is_ok();
    let deadline = tokio::time::Instant::now() + RETIRE_JOIN_BOUND;
    let mut joined = 0;
    while !tasks.is_empty() {
        match tokio::time::timeout_at(deadline, tasks.join_next()).await {
            Ok(Some(done)) => {
                account(report, done);
                joined += 1;
            }
            // Nothing left to join, or the bound expired: either way this
            // retirement stops counting and the swap goes on.
            Ok(None) | Err(_) => break,
        }
    }
    *clients = ctx.cancel.child_token();
    let retired = RetireReport {
        unlinked,
        joined,
        outstanding: tasks.len(),
    };
    tracing::info!(
        unlinked = retired.unlinked,
        joined = retired.joined,
        outstanding = retired.outstanding,
        socket = %ctx.socket.display(),
        "the session socket is retired; the successor may bind it",
    );
    retired
}
