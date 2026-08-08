//! Both directions between a session and a layout file.
//!
//! [`project`] reads a `session.state` reply and renders the shape it already
//! describes; [`plan`] turns a file back into the exact sequence of public
//! calls that rebuilds it. Neither talks to a server — one takes a reply and
//! the other produces a list — which is what lets the round trip be tested
//! without one, and what makes a layout applicable to a *remote* session over
//! the bridge with no change at all (D-M3-11).
//!
//! # Why the file is a recipe
//!
//! `pane.split` is the only way to add a pane, and it always cuts the pane it
//! is given in half, putting the new one on the right or below. So a BSP tree
//! is exactly a sequence of splits, and the sequence is what the file holds.
//! Reconstruction is then a replay rather than a solve: [`plan`] never has to
//! work out *which* tree a set of rectangles means.
//!
//! The recipe order is fixed and is the one the tree itself dictates: a split
//! first, then everything inside its first half, then everything inside its
//! second. Reading the tree with [`project`] and building it with [`plan`] walk
//! that same order in opposite directions, which is why an export of an applied
//! file is the file.
//!
//! # The three things a verb cannot do directly
//!
//! - **A workspace's first pane is made by `workspace.create`**, which takes no
//!   cwd. When the file asks for one, apply makes the pane it wants beside the
//!   root and closes the root: two calls to say what one parameter would.
//! - **`agent.start` chooses its own slot** — it splits whatever the workspace
//!   has focused. So an agent leaf is built as a placeholder, then the started
//!   agent is swapped into the placeholder's slot and the placeholder closed,
//!   which restores the tree exactly and leaves the agent where the file put it.
//! - **`pane.split` cuts in half.** A ratio is reached by nudging the fresh
//!   split with `pane.resize` while both its children are still leaves.

use std::path::PathBuf;

use amx_core::agent::AgentKind;
use amx_core::{Axis, PaneId};
use amx_proto::control::pane::{MoveDirection, SplitDirection};
use amx_proto::control::session::StateReply;
use serde::Deserialize;

use crate::layout::schema::{LayoutFile, PaneSpec, RATIO_DEFAULT, SplitSpec, WorkspaceSpec};

/// A workspace apply will create, by the order it creates them in.
pub type WorkspaceSlot = usize;

/// A pane apply will create, by the order it creates them in.
///
/// Slots, not ids: the plan is built before the first call, so nothing in it
/// can name something that does not exist yet. The executor keeps one vector
/// of real ids and indexes it by slot.
pub type PaneSlot = usize;

/// One call apply makes, in the order it makes them.
#[derive(Clone, PartialEq, Debug)]
pub enum Step {
    /// `workspace.create`. Its root pane takes the next pane slot.
    CreateWorkspace {
        /// The label to give it, already de-collided.
        label: Option<String>,
        /// The slot the new workspace takes.
        workspace: WorkspaceSlot,
        /// The slot its root pane takes.
        root: PaneSlot,
    },
    /// `pane.split`.
    Split {
        /// The pane to cut.
        from: PaneSlot,
        /// The slot the new pane takes.
        into: PaneSlot,
        /// Which way to cut it.
        direction: SplitDirection,
        /// Where the new pane starts, when the file said.
        cwd: Option<PathBuf>,
    },
    /// `pane.resize`: give the split that was just made the ratio the file asks
    /// for, while both its children are still leaves.
    Resize {
        /// The pane that was split — the first child of the split to nudge.
        pane: PaneSlot,
        /// The share it should keep.
        ratio: f32,
        /// The direction the split ran, which decides which pair of compass
        /// directions the nudge is spelled with.
        direction: SplitDirection,
    },
    /// `agent.start`. The pane it makes takes the next slot; where that pane
    /// lands is the server's choice, which is what the [`Step::Swap`] after it
    /// is for.
    StartAgent {
        /// Which workspace to start it in.
        workspace: WorkspaceSlot,
        /// The slot the agent's pane takes.
        into: PaneSlot,
        /// Which agent, by registry id.
        kind: AgentKind,
        /// Its name, which becomes its pane's label, already de-collided.
        name: String,
        /// Where to start it, when the file said.
        cwd: Option<PathBuf>,
    },
    /// `pane.swap`: put a started agent where the file wants it.
    Swap {
        /// One pane.
        pane: PaneSlot,
        /// The other.
        with: PaneSlot,
    },
    /// `pane.close`: drop a placeholder once the pane that replaces it is in
    /// its slot.
    Close {
        /// The pane to close.
        pane: PaneSlot,
    },
    /// `pane.rename`.
    Rename {
        /// The pane to label.
        pane: PaneSlot,
        /// Its label, already de-collided.
        label: String,
    },
}

/// The labels a session is already using.
///
/// Apply *adds* to a running session (D-M3-11), so every name the file brings
/// has to be checked against the ones already there. A collision suffixes
/// rather than merges: two workspaces called `dev` are two workspaces, and
/// quietly folding a layout into somebody's running one would be the single
/// most destructive thing this verb could do.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Taken {
    /// Workspace labels in use.
    pub workspaces: Vec<String>,
    /// Pane labels in use — which is also the agent-name namespace (D-M2-9).
    pub panes: Vec<String>,
}

impl Taken {
    /// What a session reply says is already spoken for.
    #[must_use]
    pub fn of(state: &StateReply) -> Self {
        Self {
            workspaces: state
                .workspaces
                .iter()
                .filter_map(|workspace| workspace.label.clone())
                .collect(),
            panes: state
                .panes
                .iter()
                .filter_map(|pane| pane.label.clone())
                .collect(),
        }
    }

    /// `wanted`, or the first free `wanted-N`, claimed.
    ///
    /// Two is where the suffix starts, because the unsuffixed name is the
    /// first. The loop ends because every turn proposes a name it has not
    /// proposed before and the list it checks against is finite.
    fn claim(taken: &mut Vec<String>, wanted: &str) -> String {
        let mut candidate = wanted.to_owned();
        let mut n = 1_u32;
        while taken.contains(&candidate) {
            n += 1;
            candidate = format!("{wanted}-{n}");
        }
        taken.push(candidate.clone());
        candidate
    }
}

/// The calls that build `file` in a session whose names are `taken`.
///
/// `taken` is consumed as the plan is built, so two workspaces in one file
/// asking for the same label get different ones — the collision rule applies
/// within a layout exactly as it does against the session.
#[must_use]
pub fn plan(file: &LayoutFile, taken: &Taken) -> Vec<Step> {
    let mut taken = taken.clone();
    let mut steps = Vec::new();
    let mut panes: PaneSlot = 0;
    for (workspace, spec) in file.workspaces.iter().enumerate() {
        plan_workspace(workspace, spec, &mut taken, &mut steps, &mut panes);
    }
    steps
}

/// One `[[workspace]]`, from `workspace.create` to the last agent.
fn plan_workspace(
    workspace: WorkspaceSlot,
    spec: &WorkspaceSpec,
    taken: &mut Taken,
    steps: &mut Vec<Step>,
    panes_made: &mut PaneSlot,
) {
    // Slots are handed out in creation order, across every workspace in the
    // file, so a step never names a pane an earlier step did not make.
    let mut next_pane = || {
        let slot = *panes_made;
        *panes_made += 1;
        slot
    };
    let label = spec
        .label
        .as_deref()
        .map(|label| Taken::claim(&mut taken.workspaces, label));
    let root = next_pane();
    steps.push(Step::CreateWorkspace {
        label,
        workspace,
        root,
    });
    let Some(first) = spec.panes.first() else {
        // A workspace with no panes in the file is the one `workspace.create`
        // just made, and nothing more is owed.
        return;
    };

    // Slot per file pane. The root is the first one until something replaces
    // it, which is the only case where a file pane's slot ever changes.
    let mut slots: Vec<PaneSlot> = vec![root; spec.panes.len()];

    // The root replacement: `workspace.create` takes neither a cwd nor an
    // agent, so when the first pane wants either, the pane that wants it is
    // made beside the root and the root is closed under it.
    if let Some(kind) = &first.agent {
        let into = next_pane();
        steps.push(Step::StartAgent {
            workspace,
            into,
            kind: kind.clone(),
            name: agent_name(first, kind, taken),
            cwd: first.cwd.clone(),
        });
        steps.push(Step::Close { pane: root });
        slots[0] = into;
    } else if first.cwd.is_some() {
        let into = next_pane();
        steps.push(Step::Split {
            from: root,
            into,
            direction: SplitDirection::Vertical,
            cwd: first.cwd.clone(),
        });
        steps.push(Step::Close { pane: root });
        slots[0] = into;
    }

    // The tree, split by split, in the file's order — which is the order the
    // tree dictates (see the module docs).
    for (index, pane) in spec.panes.iter().enumerate().skip(1) {
        // Validated at parse time: every pane but the first carries a split.
        let Some(split) = &pane.split else { continue };
        let into = next_pane();
        let from = slots[split.from];
        steps.push(Step::Split {
            from,
            into,
            direction: split.direction,
            // An agent leaf is a placeholder that the agent replaces; the cwd
            // rides `agent.start` instead, where it belongs.
            cwd: pane.agent.is_none().then(|| pane.cwd.clone()).flatten(),
        });
        if !same_ratio(split.ratio, RATIO_DEFAULT) {
            steps.push(Step::Resize {
                pane: from,
                ratio: split.ratio,
                direction: split.direction,
            });
        }
        slots[index] = into;
    }

    // Labels for the panes that are not agents. An agent's label is its name,
    // and `agent.start` sets it.
    for (index, pane) in spec.panes.iter().enumerate() {
        if pane.agent.is_some() {
            continue;
        }
        if let Some(label) = &pane.label {
            steps.push(Step::Rename {
                pane: slots[index],
                label: Taken::claim(&mut taken.panes, label),
            });
        }
    }

    // The agents, swapped into the slots their placeholders hold. The first
    // pane's agent was started above, against the root.
    for (index, pane) in spec.panes.iter().enumerate().skip(1) {
        let Some(kind) = &pane.agent else { continue };
        let into = next_pane();
        steps.push(Step::StartAgent {
            workspace,
            into,
            kind: kind.clone(),
            name: agent_name(pane, kind, taken),
            cwd: pane.cwd.clone(),
        });
        steps.push(Step::Swap {
            pane: into,
            with: slots[index],
        });
        steps.push(Step::Close { pane: slots[index] });
        slots[index] = into;
    }
}

/// What to call an agent: the label the file gave it, or its kind.
///
/// The name is the pane's label (D-M2-9) and `agent.start` refuses a duplicate
/// outright, so it goes through the same collision rule every other label does.
fn agent_name(pane: &PaneSpec, kind: &AgentKind, taken: &mut Taken) -> String {
    let wanted = pane.label.as_deref().unwrap_or_else(|| kind.as_str());
    Taken::claim(&mut taken.panes, wanted)
}

/// How to spell a ratio as a `pane.resize`: which way, and by how much.
///
/// A fresh split is at [`RATIO_DEFAULT`] and resize moves it by a delta whose
/// sign is the direction's, so the delta is the distance and the direction is
/// which side of half the target sits on. The compass pair follows the split's
/// own axis, so an `explain`-style trace of a plan reads the way the layout
/// looks.
#[must_use]
pub fn nudge(direction: SplitDirection, ratio: f32) -> (MoveDirection, f32) {
    let grow = ratio >= RATIO_DEFAULT;
    let way = match (direction, grow) {
        (SplitDirection::Vertical, true) => MoveDirection::Right,
        (SplitDirection::Vertical, false) => MoveDirection::Left,
        (SplitDirection::Horizontal, true) => MoveDirection::Down,
        (SplitDirection::Horizontal, false) => MoveDirection::Up,
    };
    (way, (ratio - RATIO_DEFAULT).abs())
}

/// Whether two ratios are the same to the precision the file records.
fn same_ratio(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.000_5
}

/// Render a `session.state` reply as the layout it describes.
///
/// # Errors
///
/// Only if a layout tree on the wire is not the shape `amx-core` serializes —
/// which would mean the reply and this build disagree about the protocol, and
/// is worth reporting rather than papering over.
pub fn project(state: &StateReply) -> Result<LayoutFile, serde_json::Error> {
    let mut workspaces = Vec::with_capacity(state.workspaces.len());
    for workspace in &state.workspaces {
        let tree: Tree = serde_json::from_value(serde_json::to_value(&workspace.layout)?)?;
        let mut panes = Vec::new();
        if let Some(root) = &tree.root {
            let leaf = leftmost(root);
            panes.push(leaf_spec(state, leaf, None));
            emit(state, root, 0, &mut panes);
        }
        workspaces.push(WorkspaceSpec {
            label: workspace.label.clone(),
            panes,
        });
    }
    Ok(LayoutFile { workspaces })
}

/// Walk `node`, emitting one pane per split in the order apply replays them.
///
/// `source` is the index of the pane occupying this node's slot — the pane a
/// split here would cut.
fn emit(state: &StateReply, node: &Node, source: usize, panes: &mut Vec<PaneSpec>) {
    let Node::Split {
        axis,
        ratio,
        first,
        second,
    } = node
    else {
        return;
    };
    let index = panes.len();
    panes.push(leaf_spec(
        state,
        leftmost(second),
        Some(SplitSpec {
            from: source,
            direction: match axis {
                Axis::X => SplitDirection::Vertical,
                Axis::Y => SplitDirection::Horizontal,
            },
            ratio: *ratio,
        }),
    ));
    emit(state, first, source, panes);
    emit(state, second, index, panes);
}

/// The pane occupying a subtree's slot: the one every split inside it was cut
/// from, which is its first leaf.
fn leftmost(node: &Node) -> PaneId {
    match node {
        Node::Leaf(pane) => *pane,
        Node::Split { first, .. } => leftmost(first),
    }
}

/// What the file says about one pane.
///
/// The state reply's pane rows carry the label and the agent snapshot; the
/// snapshot's `session_ref` is read by nothing here, and there is no key for
/// it to be written to (D-M3-11).
fn leaf_spec(state: &StateReply, pane: PaneId, split: Option<SplitSpec>) -> PaneSpec {
    let row = state.panes.iter().find(|row| row.pane == pane);
    PaneSpec {
        split,
        label: row.and_then(|row| row.label.clone()),
        agent: row
            .and_then(|row| row.agent.as_ref())
            .and_then(|agent| agent.kind.clone()),
        // The cwd the server last recorded for the pane's process. W13 wrote
        // `None` here because `session.state` carried no such field — the
        // fourth of D-M3-11's five, missing; W14 added it, and this is the
        // caller it was added for.
        cwd: row.and_then(|row| row.cwd.clone()),
    }
}

/// The layout tree as `session.state` puts it on the wire.
///
/// `amx_core::Layout` keeps its node type private — the tree is data a client
/// mirrors, not an algebra it re-implements — so this reads the serialized
/// form, which is the frozen wire shape the goldens pin (04 §3: "the client
/// mirrors it as state"). `zoomed` is ignored: zoom is where somebody is
/// looking.
#[derive(Debug, Deserialize)]
struct Tree {
    #[serde(default)]
    root: Option<Node>,
}

/// See [`Tree`].
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Node {
    /// A pane filling its slot.
    Leaf(PaneId),
    /// Two subtrees sharing a slot.
    Split {
        /// Which dimension the cut divides.
        axis: Axis,
        /// The first child's share of it.
        ratio: f32,
        /// The top/left subtree.
        first: Box<Node>,
        /// The bottom/right subtree.
        second: Box<Node>,
    },
}
