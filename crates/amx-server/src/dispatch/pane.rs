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
//! `send_text`, `send_keys`, `run`, `read` — as seams **V12** fills. They do
//! not follow the shape above: three of them reach the pane's actor directly
//! rather than through `Core`, for the same reason a keystroke does (04 §4's
//! round-trip budget), and V12 wires them that way.

use amx_proto::control::{Method, pane};
use amx_proto::rpc::RpcError;

use crate::actor::{CoreCommand, PaneCall};
use crate::dispatch::{Router, seam};

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
// A seam each; **V12** fills all four. They are the primitives everything else
// in M2 rides — `agent.prompt` submits through `run`, the resume path types a
// planned argv through it, and the exit suite drives its fake agents with
// `send_keys` — so V12's tests assert what the *child* received, not what the
// server sent.
//
// `read` is the one that does not reach the parser thread: it serves V05's text
// view straight off the pane's published snapshot feed, which is lock-free and
// contends with nothing. The other three are writes, and writes are serialized
// with query replies on the parser thread so driven input can never reorder
// against an out-of-band reply (04 §3's retained `response_order` guarantee).

/// `pane.send_text`: bytes to the child, verbatim.
///
/// **V12** fills this.
pub(super) async fn send_text(
    router: &Router,
    params: pane::SendTextParams,
) -> Result<pane::SendTextReply, RpcError> {
    let _ = (router, params);
    seam(Method::PaneSendText, "V12")
}

/// `pane.send_keys`: the key-combo grammar of 04 §8, encoded and sent.
///
/// **V12** fills this. The encoding happens on the *parser thread*, through a
/// new `ParserCommand` serialized like `History`, because it depends on the
/// pane's kitty-keyboard flags — which the parser owns — and because that is
/// also what keeps driven input ordered against query replies.
pub(super) async fn send_keys(
    router: &Router,
    params: pane::SendKeysParams,
) -> Result<pane::SendKeysReply, RpcError> {
    let _ = (router, params);
    seam(Method::PaneSendKeys, "V12")
}

/// `pane.run`: bracketed-paste-aware atomic text-plus-submit.
///
/// **V12** fills this. Bracket **only** when the application in the pane
/// enabled paste mode; bracketing one that did not types `[200~` into it.
pub(super) async fn run(
    router: &Router,
    params: pane::RunParams,
) -> Result<pane::RunReply, RpcError> {
    let _ = (router, params);
    seam(Method::PaneRun, "V12")
}

/// `pane.read`: the visible grid as text.
///
/// **V12** fills this, over V05's `Row::line()`/`Snapshot::tail(n)`. The
/// visible grid is by construction the live bottom — scrollback and scroll
/// position are client-side (04 §3) — so there is no scrolled-viewport hazard
/// here to test against, unlike herdr, which had to anchor its detection buffer
/// to the scrollback bottom and regression-test the anchor.
pub(super) async fn read(
    router: &Router,
    params: pane::ReadParams,
) -> Result<pane::ReadReply, RpcError> {
    let _ = (router, params);
    seam(Method::PaneRead, "V12")
}
