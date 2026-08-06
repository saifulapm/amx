//! Acceptance tests for the session state tree's `Effect` contract (T07).

mod support;

use std::collections::BTreeMap;

use amx_core::{Direction, Effect, Level, PaneId, Rect, SessionState, WorkspaceId};
use proptest::prelude::*;

use support::grid_workspace;

#[test]
fn move_pane_between_workspaces_preserves_the_pane_uuid() {
    let mut state = SessionState::new();
    let (ws_a, pane_a, _) = state.open_workspace();
    let (ws_b, pane_b, _) = state.open_workspace();

    let effect = state
        .move_pane(pane_a, ws_a, ws_b, Some(pane_b), Direction::Right)
        .unwrap();
    assert_eq!(effect, Effect::Layout);

    // `pane_a` is gone from its old workspace...
    assert!(state.workspace(ws_a).unwrap().layout().is_empty());
    // ...and present, under the same id, in the new one.
    assert!(state.workspace(ws_b).unwrap().layout().contains(pane_a));
    assert!(state.workspace(ws_b).unwrap().layout().contains(pane_b));
    // The pane's own record — same UUID key — survived the move untouched.
    assert!(state.pane(pane_a).is_some());
}

#[test]
fn focus_move_hjkl_picks_the_geometric_neighbour() {
    // `bottom_right`'s tree-adjacent sibling is `right` (they share a direct
    // parent split); its geometric neighbours are different panes entirely,
    // which is exactly what a correct neighbour search has to notice.
    let mut grid = grid_workspace();
    assert_eq!(
        grid.state.workspace(grid.workspace).unwrap().focus(),
        Some(grid.bottom_right)
    );

    let effect = grid
        .state
        .move_focus(grid.workspace, Direction::Left)
        .unwrap();
    assert_eq!(effect, Effect::Layout);
    assert_eq!(
        grid.state.workspace(grid.workspace).unwrap().focus(),
        Some(grid.bottom_left)
    );

    let mut grid = grid_workspace();
    let effect = grid
        .state
        .move_focus(grid.workspace, Direction::Up)
        .unwrap();
    assert_eq!(effect, Effect::Layout);
    assert_eq!(
        grid.state.workspace(grid.workspace).unwrap().focus(),
        Some(grid.right)
    );

    // At the bottom-right corner, `Down` and `Right` have no neighbour: the
    // no-op case the `Effect` contract has to get right, not just the
    // successful moves.
    let mut grid = grid_workspace();
    assert_eq!(
        grid.state
            .move_focus(grid.workspace, Direction::Down)
            .unwrap(),
        Effect::Nothing
    );
    assert_eq!(
        grid.state
            .move_focus(grid.workspace, Direction::Right)
            .unwrap(),
        Effect::Nothing
    );
    assert_eq!(
        grid.state.workspace(grid.workspace).unwrap().focus(),
        Some(grid.bottom_right)
    );
}

#[derive(Clone, Copy, Debug)]
enum Op {
    OpenWorkspace,
    Split {
        ws: usize,
        target: usize,
        dir: Direction,
        ratio: f32,
    },
    Close {
        ws: usize,
        pane: usize,
    },
    Swap {
        ws: usize,
        a: usize,
        b: usize,
    },
    Move {
        pane: usize,
        from: usize,
        to: usize,
        target: usize,
        dir: Direction,
    },
    Resize {
        ws: usize,
        target: usize,
        delta: f32,
    },
    Zoom {
        ws: usize,
        target: usize,
    },
    Unzoom {
        ws: usize,
    },
    MoveFocus {
        ws: usize,
        dir: Direction,
    },
}

/// One workspace's rects (sorted by pane id), focus and zoom state.
type WorkspaceSnapshot = (Vec<(PaneId, Rect)>, Option<PaneId>, Option<PaneId>);

/// The session plus every id it has ever minted, so op indices always have
/// something to resolve against (`idx % pool.len()`) even as the pools grow.
struct Model {
    state: SessionState,
    workspaces: Vec<WorkspaceId>,
    panes: Vec<PaneId>,
}

impl Model {
    fn seeded() -> Self {
        let mut state = SessionState::new();
        let (ws_a, pane_a, _) = state.open_workspace();
        let (ws_b, pane_b, _) = state.open_workspace();
        Self {
            state,
            workspaces: vec![ws_a, ws_b],
            panes: vec![pane_a, pane_b],
        }
    }

    fn ws_at(&self, idx: usize) -> WorkspaceId {
        self.workspaces[idx % self.workspaces.len()]
    }

    fn pane_at(&self, idx: usize) -> PaneId {
        self.panes[idx % self.panes.len()]
    }

    /// Every observable piece of structural state, sorted so two snapshots
    /// taken around one op compare equal iff nothing actually changed.
    fn snapshot(&self) -> BTreeMap<WorkspaceId, WorkspaceSnapshot> {
        self.state
            .workspaces()
            .map(|ws| {
                let mut rects = ws.layout().rects(ws.area());
                rects.sort_by_key(|(id, _)| *id);
                (ws.id(), (rects, ws.focus(), ws.layout().zoomed()))
            })
            .collect()
    }
}

fn apply(model: &mut Model, op: Op) -> Option<Effect> {
    match op {
        Op::OpenWorkspace => {
            let (ws, pane, effect) = model.state.open_workspace();
            model.workspaces.push(ws);
            model.panes.push(pane);
            Some(effect)
        }
        Op::Split {
            ws,
            target,
            dir,
            ratio,
        } => {
            let ws = model.ws_at(ws);
            let target = model.pane_at(target);
            match model.state.split(ws, target, dir, ratio) {
                Ok((pane, effect)) => {
                    model.panes.push(pane);
                    Some(effect)
                }
                Err(_) => None,
            }
        }
        Op::Close { ws, pane } => {
            let ws = model.ws_at(ws);
            let pane = model.pane_at(pane);
            model.state.close(ws, pane).ok()
        }
        Op::Swap { ws, a, b } => {
            let ws = model.ws_at(ws);
            let a = model.pane_at(a);
            let b = model.pane_at(b);
            model.state.swap(ws, a, b).ok()
        }
        Op::Move {
            pane,
            from,
            to,
            target,
            dir,
        } => {
            let pane = model.pane_at(pane);
            let from = model.ws_at(from);
            let to = model.ws_at(to);
            let target = model.pane_at(target);
            model
                .state
                .move_pane(pane, from, to, Some(target), dir)
                .ok()
        }
        Op::Resize { ws, target, delta } => {
            let ws = model.ws_at(ws);
            let target = model.pane_at(target);
            model.state.resize(ws, target, delta).ok()
        }
        Op::Zoom { ws, target } => {
            let ws = model.ws_at(ws);
            let target = model.pane_at(target);
            model.state.zoom(ws, target).ok()
        }
        Op::Unzoom { ws } => {
            let ws = model.ws_at(ws);
            model.state.unzoom(ws).ok()
        }
        Op::MoveFocus { ws, dir } => {
            let ws = model.ws_at(ws);
            model.state.move_focus(ws, dir).ok()
        }
    }
}

fn direction_strategy() -> impl Strategy<Value = Direction> {
    prop_oneof![
        Just(Direction::Left),
        Just(Direction::Down),
        Just(Direction::Up),
        Just(Direction::Right),
    ]
}

fn op_strategy() -> impl Strategy<Value = Op> {
    let idx = 0usize..32;
    prop_oneof![
        Just(Op::OpenWorkspace),
        (idx.clone(), idx.clone(), direction_strategy(), 0.0f32..1.0).prop_map(
            |(ws, target, dir, ratio)| Op::Split {
                ws,
                target,
                dir,
                ratio
            }
        ),
        (idx.clone(), idx.clone()).prop_map(|(ws, pane)| Op::Close { ws, pane }),
        (idx.clone(), idx.clone(), idx.clone()).prop_map(|(ws, a, b)| Op::Swap { ws, a, b }),
        (
            idx.clone(),
            idx.clone(),
            idx.clone(),
            idx.clone(),
            direction_strategy()
        )
            .prop_map(|(pane, from, to, target, dir)| Op::Move {
                pane,
                from,
                to,
                target,
                dir
            }),
        (idx.clone(), idx.clone(), -1.0f32..1.0).prop_map(|(ws, target, delta)| Op::Resize {
            ws,
            target,
            delta
        }),
        (idx.clone(), idx.clone()).prop_map(|(ws, target)| Op::Zoom { ws, target }),
        idx.clone().prop_map(|ws| Op::Unzoom { ws }),
        (idx, direction_strategy()).prop_map(|(ws, dir)| Op::MoveFocus { ws, dir }),
    ]
}

proptest! {
    /// Every handler folds through `Effect`, never a flag (D2): whenever an
    /// op actually changes a workspace's rects, focus or zoom, the effect it
    /// returned must be at least `Level::Layout` — the level every T07
    /// handler uses for a real structural change. Over-reporting (e.g.
    /// `Layout` on a true no-op) is allowed by the contract and not checked;
    /// under-reporting is the bug this test exists to catch.
    #[test]
    fn every_op_returns_an_effect_at_least_as_strong_as_its_change(ops in proptest::collection::vec(op_strategy(), 1..40)) {
        let mut model = Model::seeded();
        for op in ops {
            let before = model.snapshot();
            let effect = apply(&mut model, op);
            let after = model.snapshot();

            if before != after {
                let level = effect.map(Effect::level).unwrap_or(Level::Nothing);
                prop_assert!(
                    level >= Level::Layout,
                    "state changed under {op:?} but effect was {effect:?}"
                );
            }
        }
    }
}
