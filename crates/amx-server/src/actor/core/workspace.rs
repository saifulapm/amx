//! `workspace.*` handlers: create, rename, kill, switch — and the seeding a
//! first attach triggers, which is a create by another name.
//!
//! Create comes in the same two shapes as a split (see [`super::pane`]): the
//! live handler [`Core::handle_workspace_create_live`] spawns a real shell
//! behind the fresh workspace's root pane, and the `_no_spawn` fallback is
//! what [`super::Core::absorb`] uses when no Tokio runtime is in scope to
//! start a `PaneHost` under.

use std::path::{Path, PathBuf};

use amx_core::{Event, WorkspaceId, Worktree};
use amx_proto::control::workspace;
use amx_proto::rpc::RpcError;

use super::Core;
use crate::actor::Reply;

impl Core {
    /// The synchronous fallback [`super::Core::absorb`] uses for a create:
    /// mints the workspace and its root pane, but starts nothing behind the
    /// pane. [`Core::handle_workspace_create_live`] is what
    /// [`super::Core::run`] actually calls.
    pub(super) fn handle_workspace_create_no_spawn(
        &mut self,
        params: workspace::CreateParams,
        reply: Reply<workspace::CreateReply>,
    ) {
        if let Err(err) = check_worktree(params.worktree.as_ref()) {
            let _ = reply.send(Err(err));
            return;
        }
        let (ws, pane, effect) = self.state.open_workspace();
        let _ = self.next_pane_short(pane);
        self.effects.absorb(effect);
        self.finish_workspace_create(ws, params, reply);
    }

    /// The real create handler: a workspace's root pane gets a live shell,
    /// through the same spawn path a split uses, so every workspace made over
    /// the API starts with a process to land in rather than a dead pane.
    pub(super) fn handle_workspace_create_live(
        &mut self,
        params: workspace::CreateParams,
        reply: Reply<workspace::CreateReply>,
    ) {
        if let Err(err) = check_worktree(params.worktree.as_ref()) {
            let _ = reply.send(Err(err));
            return;
        }
        // The worktree block is where this workspace's shell opens (D-M3-10):
        // `amx work` has just checked a branch out there, and a root pane in the
        // server's own directory would make the membership decorative.
        let root_cwd = params.worktree.as_ref().map(|tree| tree.path.clone());
        match self.open_workspace_live(root_cwd) {
            Ok(ws) => self.finish_workspace_create(ws, params, reply),
            Err(err) => {
                let _ = reply.send(Err(err));
            }
        }
    }

    /// The tail both create shapes share: the event, the optional label, the
    /// worktree membership, the reply.
    fn finish_workspace_create(
        &mut self,
        ws: WorkspaceId,
        params: workspace::CreateParams,
        reply: Reply<workspace::CreateReply>,
    ) {
        let mut seq = self.publish(Event::WorkspaceCreated { workspace: ws });
        if let Some(worktree) = params.worktree {
            // Freshly created: the workspace is there to record it on.
            let _ = self.state.set_worktree(ws, Some(worktree));
        }
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
        // The field M0 froze and nothing read until W12 found out the hard way:
        // `amx work` asked for a focused workspace, got an unfocused one, and
        // the next `amx work done` with no branch had nothing to resolve. It is
        // read here rather than deleted from the row because callers already
        // send it meaning what it says, and a documented parameter that does
        // nothing is worse than one that does.
        //
        // After the label, so a status line reading the focus change already
        // has the name; the switch's own `FocusChanged` is the last word.
        if params.focus
            && let Ok(effect) = self.state.switch_workspace(ws)
        {
            self.effects.absorb(effect);
            seq = self.publish(Event::FocusChanged {
                workspace: ws,
                pane: self.state.workspace(ws).and_then(|ws| ws.focus()),
            });
        }
        let short = self.next_workspace_short(ws);
        let _ = reply.send(Ok(workspace::CreateReply {
            workspace: ws,
            short,
            seq,
        }));
    }

    /// An attached client's handshake completed: seed a session that has no
    /// workspaces, with a live shell (see
    /// [`SessionCall::Attached`](crate::actor::SessionCall::Attached)).
    pub(super) fn handle_attached_live(&mut self, reply: Reply<()>) {
        let _ = reply.send(self.seed_first_workspace(true));
    }

    /// The synchronous fallback [`super::Core::absorb`] uses for an attach:
    /// the same seeding, minus the process — `absorb` never spawns.
    pub(super) fn handle_attached_no_spawn(&mut self, reply: Reply<()>) {
        let _ = reply.send(self.seed_first_workspace(false));
    }

    /// Seed a session with no workspaces with its first one, switched to.
    ///
    /// A session that already has a workspace is left exactly as it is, which
    /// is what makes two racing first attaches seed one workspace and not
    /// two: the `Core` serializes its mailbox, so the second attach runs this
    /// after the first one's workspace exists.
    fn seed_first_workspace(&mut self, spawn: bool) -> Result<(), RpcError> {
        if self.state.workspaces().next().is_some() {
            return Ok(());
        }
        let ws = if spawn {
            self.open_workspace_live(None)?
        } else {
            let (ws, pane, effect) = self.state.open_workspace();
            let _ = self.next_pane_short(pane);
            self.effects.absorb(effect);
            ws
        };
        let _ = self.next_workspace_short(ws);
        self.publish(Event::WorkspaceCreated { workspace: ws });
        // Freshly created and present: switching to it cannot fail.
        if let Ok(effect) = self.state.switch_workspace(ws) {
            self.effects.absorb(effect);
            let pane = self.state.workspace(ws).and_then(|w| w.focus());
            self.publish(Event::FocusChanged {
                workspace: ws,
                pane,
            });
        }
        Ok(())
    }

    /// Mint a workspace and spawn the shell behind its root pane, focused,
    /// rolling the state back if the spawn fails.
    ///
    /// The shell is whichever [`config_rt::shell`](crate::config_rt::shell)
    /// picks, spawned through the same [`Core::spawn_pane`] path a split
    /// takes; the root pane's cwd is `cwd`, or this process's own when the
    /// caller named none — a fresh workspace has no source pane to inherit a
    /// foreground cwd from (04 §7) — and is recorded so later splits fall back
    /// to it.
    fn open_workspace_live(&mut self, cwd: Option<PathBuf>) -> Result<WorkspaceId, RpcError> {
        let (ws, root, effect) = self.state.open_workspace();
        let _ = self.next_pane_short(root);
        let cwd = cwd
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/"));
        match self.spawn_pane(root, cwd.clone(), None) {
            Ok(host) => {
                self.panes.insert(root, host);
                // The pane was just minted: recording its cwd cannot fail.
                let _ = self.state.set_pane_cwd(root, cwd);
                self.effects.absorb(effect);
                Ok(ws)
            }
            Err(err) => {
                // The state mutation succeeded but the shell it was for could
                // not start: undo it rather than leave a workspace whose root
                // pane has nothing behind it.
                let _ = self.state.kill_workspace(ws);
                Err(RpcError::new(RpcError::INTERNAL_ERROR, err.to_string()))
            }
        }
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
                    self.hang_up_pane(*pane);
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

/// Refuse a worktree block that does not describe a directory on this machine.
///
/// The server learns no git (D-M3-10) and this is not it learning any: it is the
/// one thing the server can check about a path it is asked to remember, and
/// checking it here is what keeps `amx work done` and restore honest later.
/// Membership recorded for a path that was never there would send a destructive
/// verb at a directory nobody chose, and restore would report a loss that never
/// happened.
///
/// Absolute, because both later readers join on the path as a value: the block
/// is compared against a freshly derived path by `amx work done` and stat'd by
/// restore, and a relative path would be resolved against whichever process was
/// asking.
fn check_worktree(worktree: Option<&Worktree>) -> Result<(), RpcError> {
    let Some(tree) = worktree else {
        return Ok(());
    };
    let refuse = |what: &str, path: &Path| {
        Err(RpcError::new(
            RpcError::INVALID_PARAMS,
            format!("worktree {what}: {}", path.display()),
        ))
    };
    if !tree.repo.is_absolute() {
        return refuse("repository path is not absolute", &tree.repo);
    }
    if !tree.path.is_absolute() {
        return refuse("path is not absolute", &tree.path);
    }
    if tree.branch.trim().is_empty() {
        return Err(RpcError::new(
            RpcError::INVALID_PARAMS,
            "worktree block names no branch".to_owned(),
        ));
    }
    if !tree.path.is_dir() {
        return refuse("is not a directory", &tree.path);
    }
    Ok(())
}
