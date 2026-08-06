//! `workspace.*` handlers: create, rename, kill, switch.

use amx_core::Event;
use amx_proto::control::workspace;
use amx_proto::rpc::RpcError;

use super::Core;
use crate::actor::{PaneCommand, Reply};

impl Core {
    pub(super) fn handle_workspace_create(
        &mut self,
        params: workspace::CreateParams,
        reply: Reply<workspace::CreateReply>,
    ) {
        let (ws, _pane, effect) = self.state.open_workspace();
        self.effects.absorb(effect);
        let mut seq = self.publish(Event::WorkspaceCreated { workspace: ws });
        if let Some(label) = params.label {
            // Freshly created: renaming it cannot fail.
            if let Ok(rename_effect) = self.state.rename_workspace(ws, Some(label.clone())) {
                self.effects.absorb(rename_effect);
                seq = self.publish(Event::WorkspaceRenamed {
                    workspace: ws,
                    label,
                });
            }
        }
        let short = self.next_workspace_short();
        let _ = reply.send(Ok(workspace::CreateReply {
            workspace: ws,
            short,
            seq,
        }));
    }

    pub(super) fn handle_workspace_rename(
        &mut self,
        params: workspace::RenameParams,
        reply: Reply<workspace::RenameReply>,
    ) {
        match self
            .state
            .rename_workspace(params.workspace, Some(params.label.clone()))
        {
            Ok(effect) => {
                self.effects.absorb(effect);
                let seq = self.publish(Event::WorkspaceRenamed {
                    workspace: params.workspace,
                    label: params.label,
                });
                let _ = reply.send(Ok(workspace::RenameReply { seq }));
            }
            Err(err) => {
                let _ = reply.send(Err(RpcError::new(
                    RpcError::INVALID_PARAMS,
                    err.to_string(),
                )));
            }
        }
    }

    pub(super) fn handle_workspace_kill(
        &mut self,
        params: workspace::KillParams,
        reply: Reply<workspace::KillReply>,
    ) {
        match self.state.kill_workspace(params.workspace) {
            Ok((panes, effect)) => {
                self.effects.absorb(effect);
                for pane in &panes {
                    if let Some(handle) = self.panes.remove(pane) {
                        let _ = handle.try_send(PaneCommand::Kill);
                    }
                }
                let seq = self.publish(Event::WorkspaceClosed {
                    workspace: params.workspace,
                });
                let _ = reply.send(Ok(workspace::KillReply { panes, seq }));
            }
            Err(err) => {
                let _ = reply.send(Err(RpcError::new(
                    RpcError::INVALID_PARAMS,
                    err.to_string(),
                )));
            }
        }
    }

    pub(super) fn handle_workspace_switch(
        &mut self,
        params: workspace::SwitchParams,
        reply: Reply<workspace::SwitchReply>,
    ) {
        match self.state.switch_workspace(params.workspace) {
            Ok(effect) => {
                self.effects.absorb(effect);
                let focused_pane = self
                    .state
                    .workspace(params.workspace)
                    .and_then(|w| w.focus());
                let seq = self.publish(Event::FocusChanged {
                    workspace: params.workspace,
                    pane: focused_pane,
                });
                let _ = reply.send(Ok(workspace::SwitchReply { focused_pane, seq }));
            }
            Err(err) => {
                let _ = reply.send(Err(RpcError::new(
                    RpcError::INVALID_PARAMS,
                    err.to_string(),
                )));
            }
        }
    }
}
