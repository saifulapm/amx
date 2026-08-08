//! `amx apply <file>` — replay a layout through the public verbs (D-M3-11).
//!
//! The whole file is parsed and validated before the session is even
//! connected to, so a malformed layout names its line and changes nothing: a
//! half-applied layout is worse than a refused one, and the only way to be sure
//! is to have made no call at all.
//!
//! What it does then is a replay and nothing cleverer.
//! [`plan`](crate::layout::build::plan) turns the file into a list of calls —
//! `workspace.create`, `pane.split`, `pane.resize`, `pane.rename`,
//! `agent.start`, and the `pane.swap`/`pane.close` pair that puts a started
//! agent where the file wants it — and this module is the loop that makes them,
//! carrying one vector of real ids that the plan's slots index into.
//!
//! It **adds** to the running session and suffixes names on collision, which is
//! the conservative semantic D-M3-11 chose: replacing a session is `amx session
//! stop` and then this, two explicit steps a person can mean separately.
//!
//! A refusal *after* the first call is the server's, not the file's — a cwd
//! that does not exist there, an agent kind no registry names. Apply stops at
//! it and says which step failed rather than pressing on, because the panes
//! after it would land in a tree the earlier ones did not build.

use std::process::ExitCode;

use amx_client::net::{self, Session};
use amx_core::{Env, PaneId, WorkspaceId};
use amx_proto::control::session::StateReply;
use amx_proto::control::{Method, agent, pane, workspace};
use amx_server::session::probe::probe;
use anyhow::Context as _;
use clap::ArgMatches;
use serde::Serialize;
use serde_json::Value;

use crate::cmd::attach::client_info;
use crate::ctx_of;
use crate::layout::build::{self, Step, Taken};
use crate::layout::schema;

/// Run `amx apply`.
///
/// # Errors
///
/// If the file cannot be read or parsed (naming the line, before anything is
/// connected to), if the session is not running, or if the server refuses one
/// of the calls the layout replays.
pub async fn run(env: &Env, root: &ArgMatches, sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    let path = sub
        .get_one::<String>("file")
        .context("amx apply needs a layout file")?;
    let text =
        std::fs::read_to_string(path).with_context(|| format!("read the layout file {path}"))?;
    // Everything the file can be wrong about is decided here, with nothing
    // connected and nothing created.
    let file = schema::parse(&text).map_err(|err| anyhow::anyhow!("{path}: {err}"))?;

    let ctx = ctx_of(env, root, None)?;
    anyhow::ensure!(
        probe(&ctx.socket)
            .context("probe the session socket")?
            .is_running(),
        "session {} is not running; start it with `amx`",
        ctx.session
    );
    let stream = net::connect(&ctx.socket)
        .await
        .context("connect to the session")?;
    // `attach: false`: a one-shot verb must not mutate the session by
    // connecting, and an attaching connection seeds a workspace into an empty
    // session — which would land in the middle of the layout being built.
    let (mut session, _welcome) = Session::attach(stream, client_info(), false, None)
        .await
        .context("negotiate with the session")?;

    let taken = Taken::of(&state(&mut session).await?);
    let steps = build::plan(&file, &taken);
    let made = replay(&mut session, &steps).await?;

    println!(
        "applied {path}: {} workspace{}, {} pane{}",
        made.workspaces,
        plural(made.workspaces),
        made.panes,
        plural(made.panes)
    );
    Ok(ExitCode::SUCCESS)
}

/// What a replay built.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Made {
    /// Workspaces created.
    workspaces: usize,
    /// Panes that outlived the replay — placeholders are not counted.
    panes: usize,
}

/// Make every call in `steps`, in order.
///
/// The plan names panes and workspaces by slot, in creation order; this keeps
/// the two vectors those slots index. A step that names a slot the vectors do
/// not have is a plan this executor does not understand, which is a bug worth
/// stopping on rather than a call worth guessing at.
async fn replay(session: &mut Session, steps: &[Step]) -> anyhow::Result<Made> {
    let mut panes: Vec<PaneId> = Vec::new();
    let mut workspaces: Vec<WorkspaceId> = Vec::new();
    let mut closed = 0_usize;

    for (n, step) in steps.iter().enumerate() {
        let step_of = |what: &str| format!("step {} of {}: {what}", n + 1, steps.len());
        match step {
            Step::CreateWorkspace {
                label,
                workspace,
                root,
            } => {
                anyhow::ensure!(
                    workspaces.len() == *workspace && panes.len() == *root,
                    "{}",
                    step_of("the plan and the replay disagree about slots")
                );
                let reply: workspace::CreateReply = call(
                    session,
                    Method::WorkspaceCreate,
                    &workspace::CreateParams {
                        label: label.clone(),
                        // Applying a layout must not move anybody's focus.
                        focus: false,
                    },
                )
                .await
                .with_context(|| step_of("create a workspace"))?;
                workspaces.push(reply.workspace);
                // `workspace.create` answers with the workspace, not with the
                // pane it comes with, and the splits that follow need that
                // pane. One extra query per workspace buys the whole recipe.
                panes.push(
                    root_pane(session, reply.workspace)
                        .await
                        .with_context(|| step_of("find the new workspace's pane"))?,
                );
            }
            Step::Split {
                from,
                into,
                direction,
                cwd,
            } => {
                anyhow::ensure!(
                    panes.len() == *into,
                    "{}",
                    step_of("the plan and the replay disagree about slots")
                );
                let reply: pane::SplitReply = call(
                    session,
                    Method::PaneSplit,
                    &pane::SplitParams {
                        pane: slot(&panes, *from, &step_of)?,
                        direction: *direction,
                        command: None,
                        cwd: cwd.clone(),
                    },
                )
                .await
                .with_context(|| step_of("split a pane"))?;
                panes.push(reply.pane);
            }
            Step::Resize {
                pane,
                ratio,
                direction,
            } => {
                let (direction, delta) = build::nudge(*direction, *ratio);
                let _: pane::ResizeReply = call(
                    session,
                    Method::PaneResize,
                    &pane::ResizeParams {
                        pane: slot(&panes, *pane, &step_of)?,
                        direction,
                        delta,
                    },
                )
                .await
                .with_context(|| step_of("set a split's ratio"))?;
            }
            Step::StartAgent {
                workspace,
                into,
                kind,
                name,
                cwd,
            } => {
                anyhow::ensure!(
                    panes.len() == *into,
                    "{}",
                    step_of("the plan and the replay disagree about slots")
                );
                let reply: agent::StartReply = call(
                    session,
                    Method::AgentStart,
                    &agent::StartParams {
                        name: name.clone(),
                        kind: kind.clone(),
                        args: Vec::new(),
                        cwd: cwd.clone(),
                        workspace: Some(slot(&workspaces, *workspace, &step_of)?),
                        timeout_ms: None,
                    },
                )
                .await
                .with_context(|| step_of("start an agent"))?;
                // A start that timed out still made its pane and still holds
                // the slot the layout gave it; saying so is better than either
                // pretending it came up or unwinding a session the user asked
                // for (V13's rule: the pane is left running for inspection).
                if reply.readiness != agent::Readiness::Ready {
                    eprintln!("amx: agent {name} did not report ready; its pane is running");
                }
                panes.push(reply.pane);
            }
            Step::Swap { pane, with } => {
                let _: pane::SwapReply = call(
                    session,
                    Method::PaneSwap,
                    &pane::SwapParams {
                        pane: slot(&panes, *pane, &step_of)?,
                        with: slot(&panes, *with, &step_of)?,
                    },
                )
                .await
                .with_context(|| step_of("put an agent in its slot"))?;
            }
            Step::Close { pane } => {
                let _: pane::CloseReply = call(
                    session,
                    Method::PaneClose,
                    &pane::CloseParams {
                        pane: slot(&panes, *pane, &step_of)?,
                    },
                )
                .await
                .with_context(|| step_of("close a placeholder"))?;
                closed += 1;
            }
            Step::Rename { pane, label } => {
                let _: pane::RenameReply = call(
                    session,
                    Method::PaneRename,
                    &pane::RenameParams {
                        pane: slot(&panes, *pane, &step_of)?,
                        label: label.clone(),
                    },
                )
                .await
                .with_context(|| step_of("label a pane"))?;
            }
        }
    }
    Ok(Made {
        workspaces: workspaces.len(),
        panes: panes.len().saturating_sub(closed),
    })
}

/// The id a slot names.
fn slot<T: Copy>(ids: &[T], index: usize, step_of: &impl Fn(&str) -> String) -> anyhow::Result<T> {
    ids.get(index)
        .copied()
        .with_context(|| step_of("the plan names something the replay never made"))
}

/// Make one control call with typed parameters and a typed reply.
async fn call<P, R>(session: &mut Session, method: Method, params: &P) -> anyhow::Result<R>
where
    P: Serialize,
    R: serde::de::DeserializeOwned,
{
    let params: Value = serde_json::to_value(params).context("encode the call")?;
    let reply = session.call(method.wire_name(), params).await?;
    serde_json::from_value(reply).context("read the reply")
}

/// The session as it stands.
async fn state(session: &mut Session) -> anyhow::Result<StateReply> {
    call(
        session,
        Method::SessionState,
        &amx_proto::control::session::StateParams {},
    )
    .await
    .context("read the session's state")
}

/// The pane a freshly created workspace comes with.
async fn root_pane(session: &mut Session, workspace: WorkspaceId) -> anyhow::Result<PaneId> {
    let state = state(session).await?;
    state
        .workspaces
        .iter()
        .find(|known| known.workspace == workspace)
        .and_then(|known| known.layout.panes().first().copied())
        .context("the workspace was created without a pane")
}

/// The plural `s`, or nothing.
const fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}
