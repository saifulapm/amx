//! Shared helpers for the layout and state acceptance tests.
//!
//! Not every test binary that includes this module uses every helper in it,
//! so dead-code warnings here are expected and not a sign of a bug.
#![allow(dead_code)]
#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::{Direction, PaneId, Rect, SessionState, WorkspaceId};

/// Assert that `rects` partitions `area` exactly: every cell of `area` is
/// covered by exactly one rect, and no rect extends past `area`.
///
/// This is a from-scratch geometric check (a coverage grid), independent of
/// how the rects were produced, so it cannot be satisfied by a bug that
/// merely preserves total area while overlapping or gapping.
pub fn assert_tiles_exactly(rects: &[(PaneId, Rect)], area: Rect) {
    let mut coverage = vec![0u32; area.w as usize * area.h as usize];
    for (id, r) in rects {
        assert!(
            r.x >= area.x
                && r.y >= area.y
                && r.x + r.w <= area.x + area.w
                && r.y + r.h <= area.y + area.h,
            "pane {id} rect {r:?} falls outside area {area:?}"
        );
        for yy in r.y..r.y + r.h {
            for xx in r.x..r.x + r.w {
                let idx = (yy - area.y) as usize * area.w as usize + (xx - area.x) as usize;
                coverage[idx] += 1;
            }
        }
    }
    assert!(
        coverage.iter().all(|&c| c == 1),
        "rects do not exactly tile {area:?}: some cell is covered {} times, expected 1",
        coverage.iter().max().copied().unwrap_or(0)
    );
}

/// A 2x2 grid of panes in one workspace: `root` top-left, `right` top-right,
/// `bl` bottom-left, `bottom_right` bottom-right — set up as two independent
/// splits so a neighbour search has to use geometry, not tree adjacency, to
/// get the diagonal pairs right.
pub struct Grid {
    pub state: SessionState,
    pub workspace: WorkspaceId,
    pub root: PaneId,
    pub right: PaneId,
    pub bottom_left: PaneId,
    pub bottom_right: PaneId,
}

/// Build the grid described by [`Grid`], focused on `bottom_right`.
pub fn grid_workspace() -> Grid {
    let mut state = SessionState::new();
    let (workspace, root, _) = state.open_workspace();
    let (right, _) = state
        .split(workspace, root, Direction::Right, 0.5)
        .expect("split root right");
    let (bottom_left, _) = state
        .split(workspace, root, Direction::Down, 0.5)
        .expect("split root down");
    let (bottom_right, _) = state
        .split(workspace, right, Direction::Down, 0.5)
        .expect("split right down");
    Grid {
        state,
        workspace,
        root,
        right,
        bottom_left,
        bottom_right,
    }
}
