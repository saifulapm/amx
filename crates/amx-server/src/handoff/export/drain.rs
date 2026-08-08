//! The post-commit drain, bounded (W01's watchdog).
//!
//! `docs/09-m3-plan.md` §2's outcome tree and
//! `docs/notes/m3-shutdown-wedge.md` §6 between them say exactly what this is
//! for. W01 found and fixed one wedge — a connection handshake that watched no
//! cancellation token — and that was the hole the export path was most exposed
//! to, because retiring the gateway while clients reconnect is precisely a
//! burst of freshly-connected peers that have not yet said hello. It also
//! showed, from seven corpses, that the *found* wedges were parked somewhere
//! else, in a connection's epilogue. So a second path is still open, and this
//! is the bound on it.
//!
//! [`Runtime::shutdown`](crate::runtime::Runtime::shutdown) is deliberately
//! unbounded, and stays that way: bounding it everywhere would turn a wedge
//! into a silent leak of whatever the unjoined task owns, which is the trade
//! 04 §2 refuses. After a handoff commits, that trade reverses. The exporter
//! owns nothing the session needs any more — the successor holds the
//! descriptors, the socket and the snapshot — so a task that will not return
//! is holding a corpse, and the useful thing to do with it is to say which task
//! it is and exit non-zero.
//!
//! The census is what makes that actionable rather than a bare timeout. W01
//! taught `Runtime` to write `<runtime_dir>/drain-census` when a drain overruns
//! its interval, because a daemonized server's stderr is `/dev/null`; this
//! shortens that interval to fit inside the bound, so a wedge that is caught
//! here is caught *with the names attached*. On W01's evidence the names to
//! expect are `gateway`, with `agent-hub` beside it when a connection is the
//! holder.

use std::time::Duration;

use amx_core::Ctx;
use thiserror::Error;

use crate::runtime::{CENSUS_FILE, CENSUS_INTERVAL, Runtime, ShutdownReport};

/// How long a post-commit drain may take before it is called a wedge.
///
/// §3's own patience, reused: every stage of the protocol is allowed 30 s, and
/// a drain that has not emptied in the time a whole stage is given is not going
/// to. A healthy stop is milliseconds — cancelling a token and joining six
/// actors — so this is three orders of magnitude of headroom, which is what
/// keeps a loaded runner from being reported as a wedge.
pub const POST_COMMIT_DRAIN: Duration = Duration::from_secs(30);

/// The drain did not empty inside its bound after ownership had transferred.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
#[error(
    "the exporter's shutdown drain did not empty within {waited:?} after the handoff committed; \
     {census}"
)]
pub struct DrainWedged {
    /// The bound it overran.
    pub waited: Duration,
    /// What the drain said it was holding, verbatim from the census.
    pub census: String,
}

/// Drain `runtime`, and refuse to wait forever about it.
///
/// Used only on the path where ownership has already moved. The census
/// interval is shortened to fit inside `bound` so that the file W01 added is
/// written *before* this gives up, and read back into the error — a wedge that
/// only said "timed out" would be the diagnosis the last three milestones
/// already failed to make.
///
/// Returning drops the `JoinSet`, which aborts whatever had not returned. That
/// is the point: post-commit there is nothing left to protect, and a leaked
/// process holding dead descriptors is worse than a task that never got to
/// finish.
///
/// # Errors
///
/// [`DrainWedged`], with the census, when the bound expires.
pub async fn bounded(
    mut runtime: Runtime,
    ctx: &Ctx,
    bound: Duration,
) -> Result<ShutdownReport, DrainWedged> {
    // Two reports inside the bound, so the file exists by the time it is read,
    // and never coarser than the runtime's own interval, so an ordinary stop
    // still reports on W01's schedule.
    let interval = (bound / 2)
        .max(Duration::from_millis(1))
        .min(CENSUS_INTERVAL);
    runtime.set_census_interval(interval);
    let census_file = ctx.runtime_dir.join(CENSUS_FILE);
    match tokio::time::timeout(bound, runtime.shutdown()).await {
        Ok(report) => Ok(report),
        Err(_elapsed) => {
            let census = read_census(&census_file);
            tracing::error!(
                waited_ms = bound.as_millis(),
                census = %census,
                "the drain did not empty after the handoff committed; exiting rather than wedging",
            );
            Err(DrainWedged {
                waited: bound,
                census,
            })
        }
    }
}

/// What the drain census says, or why it says nothing.
fn read_census(path: &std::path::Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => text.trim().replace('\n', "; "),
        // A census that is missing is itself a fact worth printing: it means
        // the drain never even reached its first reporting interval, which
        // narrows where to look next.
        Ok(_) | Err(_) => format!("no census was written to {}", path.display()),
    }
}
