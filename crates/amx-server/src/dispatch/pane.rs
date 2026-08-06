//! `pane.*` dispatch: split, zoom, swap, move, close.
//!
//! Every handler here is the same shape (04 §4): decode nothing further —
//! that already happened — turn typed parameters into a [`CoreCommand`]
//! carrying a reply channel, hand it to the `Core` actor and return its
//! answer. `Core` (`actor/core.rs`) is where the two behaviors T16 exists for
//! actually live: a split inheriting the source pane's foreground-process cwd
//! (04 §7), and swap/move never touching the process behind either pane.

use amx_proto::control::pane;
use amx_proto::rpc::RpcError;

use crate::actor::{CoreCommand, PaneCall};
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
