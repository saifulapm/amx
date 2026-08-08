//! `pane.*` dispatch: split, zoom, swap, move, rename, close.
//!
//! Every handler here is the same shape (04 §4): decode nothing further —
//! that already happened — turn typed parameters into a [`CoreCommand`]
//! carrying a reply channel, hand it to the `Core` actor and return its
//! answer. `Core` (`actor/core.rs`) is where the two behaviors T16 exists for
//! actually live: a split inheriting the source pane's foreground-process cwd
//! (04 §7), and swap/move never touching the process behind either pane.
//!
//! M2 adds the *driving* verbs of 04 §8 at the bottom of the file —
//! `send_text`, `send_keys`, `run`, `read`. They do not follow the shape above:
//! they reach the pane's own actor rather than going through `Core`, for the
//! same reason a keystroke does (04 §4's round-trip budget). `Core`'s part is
//! the small one `stream.bind` already established — resolving a pane to its
//! live plumbing — plus, on the label path only, the `session.state` read that
//! turns a name into a pane.

use amx_core::PaneId;
use amx_proto::control::pane;
use amx_proto::control::session;
use amx_proto::rpc::RpcError;
use tokio::sync::oneshot;

use crate::actor::pane_host::drive::PASTE_END;
use crate::actor::{
    CoreCommand, Drive, DriveError, Driven, KeyStroke, PaneCall, PaneCommand, PaneWiring,
    SessionCall, StreamCall,
};
use crate::agent::address::{self, Scope};
use crate::dispatch::Router;

pub(super) async fn split(
    router: &Router,
    params: pane::SplitParams,
) -> Result<pane::SplitReply, RpcError> {
    router
        .call(|reply| CoreCommand::Pane(PaneCall::Split { params, reply }))
        .await
}

pub(super) async fn zoom(
    router: &Router,
    params: pane::ZoomParams,
) -> Result<pane::ZoomReply, RpcError> {
    router
        .call(|reply| CoreCommand::Pane(PaneCall::Zoom { params, reply }))
        .await
}

pub(super) async fn swap(
    router: &Router,
    params: pane::SwapParams,
) -> Result<pane::SwapReply, RpcError> {
    router
        .call(|reply| CoreCommand::Pane(PaneCall::Swap { params, reply }))
        .await
}

pub(super) async fn move_pane(
    router: &Router,
    params: pane::MoveParams,
) -> Result<pane::MoveReply, RpcError> {
    router
        .call(|reply| CoreCommand::Pane(PaneCall::Move { params, reply }))
        .await
}

pub(super) async fn rename(
    router: &Router,
    params: pane::RenameParams,
) -> Result<pane::RenameReply, RpcError> {
    router
        .call(|reply| CoreCommand::Pane(PaneCall::Rename { params, reply }))
        .await
}

pub(super) async fn close(
    router: &Router,
    params: pane::CloseParams,
) -> Result<pane::CloseReply, RpcError> {
    router
        .call(|reply| CoreCommand::Pane(PaneCall::Close { params, reply }))
        .await
}

pub(super) async fn focus(
    router: &Router,
    params: pane::FocusParams,
) -> Result<pane::FocusReply, RpcError> {
    router
        .call(|reply| CoreCommand::Pane(PaneCall::Focus { params, reply }))
        .await
}

pub(super) async fn resize(
    router: &Router,
    params: pane::ResizeParams,
) -> Result<pane::ResizeReply, RpcError> {
    router
        .call(|reply| CoreCommand::Pane(PaneCall::Resize { params, reply }))
        .await
}

// ------------------------------------------- the driving verbs (04 §8, M2)
//
// The primitives everything else in M2 rides — `agent.prompt` submits through
// `run`, the resume path types a planned argv through it, and the exit suite
// drives its fake agents with `send_keys`.
//
// `read` is the one that does not reach the parser thread: it serves V05's text
// view straight off the pane's published snapshot feed, which is lock-free and
// contends with nothing. The other three are writes, and writes are serialized
// with query replies on the parser thread so driven input can never reorder
// against an out-of-band reply (04 §3's retained `response_order` guarantee) —
// `actor/pane_host/drive.rs` is where that happens and why.

/// `pane.send_text`: bytes to the child, verbatim.
pub(super) async fn send_text(
    router: &Router,
    params: pane::SendTextParams,
) -> Result<pane::SendTextReply, RpcError> {
    let pane = resolve(router, &params.target).await?;
    drive(router, pane, Drive::Text(params.text)).await?;
    Ok(pane::SendTextReply {
        pane,
        seq: router.head().await?,
    })
}

/// `pane.send_keys`: the key-combo grammar of 04 §8, encoded and sent.
///
/// The combos are parsed here, before the pane is touched, so an unreadable one
/// is `INVALID_PARAMS` naming the offending element and *nothing* is delivered.
/// The encoding itself happens on the parser thread, which owns the terminal
/// whose Kitty-keyboard and cursor-key modes decide what the bytes are.
pub(super) async fn send_keys(
    router: &Router,
    params: pane::SendKeysParams,
) -> Result<pane::SendKeysReply, RpcError> {
    let mut strokes = Vec::with_capacity(params.keys.len());
    for combo in &params.keys {
        strokes.push(KeyStroke::parse(combo).map_err(|err| {
            RpcError::new(
                RpcError::INVALID_PARAMS,
                format!("pane.send_keys: {err}, in the combo {combo:?}"),
            )
        })?);
    }
    let keys = u32::try_from(strokes.len()).unwrap_or(u32::MAX);
    let pane = resolve(router, &params.target).await?;
    drive(router, pane, Drive::Keys(strokes)).await?;
    Ok(pane::SendKeysReply {
        pane,
        keys,
        seq: router.head().await?,
    })
}

/// `pane.run`: bracketed-paste-aware atomic text-plus-submit.
///
/// Whether to bracket is the pane's answer, not this handler's: only the
/// terminal knows whether the application enabled paste mode, and bracketing
/// one that did not types `[200~` into it.
pub(super) async fn run(
    router: &Router,
    params: pane::RunParams,
) -> Result<pane::RunReply, RpcError> {
    // A paste terminator inside the text would end the bracket early and hand
    // the rest of the line to the application as ordinary keystrokes, which is
    // the one way an atomic submit can come apart. Refused rather than
    // silently rewritten: the caller's text is not ours to edit.
    if params
        .text
        .as_bytes()
        .windows(PASTE_END.len())
        .any(|w| w == PASTE_END)
    {
        return Err(RpcError::new(
            RpcError::INVALID_PARAMS,
            "pane.run: the text carries a bracketed-paste terminator",
        ));
    }
    let pane = resolve(router, &params.target).await?;
    let driven = drive(router, pane, Drive::Run(params.text)).await?;
    Ok(pane::RunReply {
        pane,
        bracketed: driven.bracketed,
        seq: router.head().await?,
    })
}

/// `pane.read`: the visible grid as text.
///
/// Served from the pane's published snapshot — an `Arc` clone out of a
/// lock-free slot — over V05's `Row::line()`/`Snapshot::tail(n)`, with no
/// parser round trip. The visible grid is by construction the live bottom,
/// since scrollback and scroll position are client-side (04 §3); scrollback
/// proper is `pane.history`.
pub(super) async fn read(
    router: &Router,
    params: pane::ReadParams,
) -> Result<pane::ReadReply, RpcError> {
    let pane = resolve(router, &params.target).await?;
    let wiring = wiring(router, pane).await?;
    let frame = wiring.frames.latest();
    let wanted = params.lines.unwrap_or_else(|| frame.rows());
    // Trailing blanks are trimmed: a row is padded to the grid's width, and
    // every consumer of this reply — a script grepping output, an agent reading
    // a screen — wants the line the user sees, not the padding.
    let rows = frame
        .tail(wanted)
        .map(|row| row.line().trim_end().to_owned())
        .collect();
    Ok(pane::ReadReply {
        pane,
        rows,
        seq: router.head().await?,
    })
}

// ------------------------------------------------------------- the plumbing

/// Resolve a [`PaneTarget`](pane::PaneTarget) to a pane: UUID, then label.
///
/// The order is fixed and it is the reason a pane labelled with another pane's
/// UUID cannot shadow it. A label must match exactly one pane — the driving
/// verbs take the wider rule of [`Scope::AnyPane`] (any pane, not only agent
/// panes), because a script driving a plain shell has no agent to be unique
/// among — and an ambiguous one is an error that *names the candidates*, since
/// "ambiguous target" without them is a message nobody can act on.
///
/// The rule itself lives in `agent/address.rs`, which V13 lifted it into so the
/// agent verbs' narrower scope could be the same function with a different
/// [`Scope`] rather than a second implementation of the same order.
///
/// Reading `session.state` is how the labels are learned, which costs one
/// `Core` round trip on the label path and none on the UUID path — the early
/// parse below is what keeps that promise, and it is the reason the UUID case
/// never touches the actor.
pub(super) async fn resolve(
    router: &Router,
    target: &pane::PaneTarget,
) -> Result<PaneId, RpcError> {
    if let Ok(pane) = target.as_str().parse::<PaneId>() {
        return Ok(pane);
    }
    let state = router
        .call(|reply| {
            CoreCommand::Session(SessionCall::State {
                params: session::StateParams::default(),
                reply,
            })
        })
        .await?;
    Ok(address::resolve(target, &state.panes, Scope::AnyPane)?)
}

/// Hand one drive to the pane and wait for its outcome.
async fn drive(router: &Router, pane: PaneId, what: Drive) -> Result<Driven, RpcError> {
    let wiring = wiring(router, pane).await?;
    let (tx, rx) = oneshot::channel();
    wiring
        .handle
        .send(PaneCommand::Drive { what, reply: tx })
        .await
        .map_err(|_| RpcError::new(RpcError::INTERNAL_ERROR, "the pane is gone"))?;
    rx.await
        .map_err(|_| RpcError::new(RpcError::INTERNAL_ERROR, "the pane dropped the write"))?
        .map_err(|err| RpcError::new(code_of(&err), err.to_string()))
}

/// The RPC code a refused drive answers with.
///
/// The three answers are three different sentences, and DR-16 is the record of
/// what it cost to spell two of them the same way:
///
/// - [`NotAccepting`](DriveError::NotAccepting) is the session holding still —
///   a pane quiesced for a handoff, which comes back. The caller asked
///   correctly and the drive did not happen, so it is
///   [`RETRIABLE`](RpcError::RETRIABLE): every mutating verb becomes
///   retriable across a swap, `agent.prompt` included, and a caller that has
///   never heard of the code sees an ordinary error exactly as it did before.
///   `INVALID_PARAMS` told it the opposite — that it had asked wrong — so
///   every CLI path that could have waited a moment reported a user error
///   instead.
/// - [`Full`](DriveError::Full) is *not* the same sentence, and is deliberately
///   left where it was. A full input queue is the child not reading: a caller
///   that retried on it would spin against a process that may never read
///   again, and the queue's depth is a fact about what was asked for as much as
///   about when.
/// - [`Gone`](DriveError::Gone) and [`Encode`](DriveError::Encode) are the
///   session failing, which no amount of asking again repairs.
const fn code_of(err: &DriveError) -> i32 {
    match err {
        DriveError::NotAccepting => RpcError::RETRIABLE,
        DriveError::Full => RpcError::INVALID_PARAMS,
        DriveError::Gone | DriveError::Encode(_) => RpcError::INTERNAL_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::{DriveError, RpcError, code_of};

    #[test]
    fn a_quiesced_pane_is_retriable_and_a_full_queue_is_not() {
        assert_eq!(code_of(&DriveError::NotAccepting), RpcError::RETRIABLE);
        assert_eq!(code_of(&DriveError::Full), RpcError::INVALID_PARAMS);
        assert_eq!(code_of(&DriveError::Gone), RpcError::INTERNAL_ERROR);
        assert_eq!(
            code_of(&DriveError::Encode(String::new())),
            RpcError::INTERNAL_ERROR
        );
    }
}

/// The pane's live plumbing: its mailbox and its published frames.
async fn wiring(router: &Router, pane: PaneId) -> Result<PaneWiring, RpcError> {
    router
        .call(|reply| CoreCommand::Stream(StreamCall::Wiring { pane, reply }))
        .await
}
