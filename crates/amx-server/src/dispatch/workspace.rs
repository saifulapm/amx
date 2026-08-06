//! `workspace.*` dispatch: create, rename, kill, switch.
//!
//! Same shape as [`super::pane`]: a typed translation into a [`CoreCommand`],
//! nothing more. `Core` (`actor/core.rs`) owns the state mutation and the
//! event this produces.

use amx_proto::control::workspace;
use amx_proto::rpc::RpcError;

use crate::actor::{CoreCommand, WorkspaceCall};
use crate::dispatch::Router;

pub(super) async fn create(
    router: &Router,
    params: workspace::CreateParams,
) -> Result<workspace::CreateReply, RpcError> {
    router
        .call(|reply| CoreCommand::Workspace(WorkspaceCall::Create { params, reply }))
        .await
}

pub(super) async fn rename(
    router: &Router,
    params: workspace::RenameParams,
) -> Result<workspace::RenameReply, RpcError> {
    router
        .call(|reply| CoreCommand::Workspace(WorkspaceCall::Rename { params, reply }))
        .await
}

pub(super) async fn kill(
    router: &Router,
    params: workspace::KillParams,
) -> Result<workspace::KillReply, RpcError> {
    router
        .call(|reply| CoreCommand::Workspace(WorkspaceCall::Kill { params, reply }))
        .await
}

pub(super) async fn switch(
    router: &Router,
    params: workspace::SwitchParams,
) -> Result<workspace::SwitchReply, RpcError> {
    router
        .call(|reply| CoreCommand::Workspace(WorkspaceCall::Switch { params, reply }))
        .await
}
