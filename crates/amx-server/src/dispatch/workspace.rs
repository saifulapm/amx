//! `workspace.*` dispatch: create, rename, kill, switch.
//!
//! Same shape as [`super::pane`]: a typed translation into a [`CoreCommand`],
//! nothing more. `Core` (`actor/core.rs`) owns the state mutation and the
//! event this produces.
//!
//! That "nothing more" is why the worktree block of D-M3-10 needed no arm of
//! its own. `amx work <branch>` sends it as an additive optional field on
//! [`workspace::CreateParams`], and every function here forwards its parameters
//! whole — so the membership rides the create it belongs to, arrives on the same
//! serialized mailbox as the workspace it describes, and cannot be recorded
//! against a workspace that failed to open. The validation and the recording are
//! `Core`'s, in `actor/core/workspace.rs`, beside the label they sit next to on
//! the wire.

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
