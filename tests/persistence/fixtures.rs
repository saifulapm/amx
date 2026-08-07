//! What the crash suite needs before it can crash anything: planted shells,
//! the on-disk snapshot as the rig reads it, and the wire calls that populate
//! a session.
//!
//! Everything here is deliberately *outside* the server: the snapshot is read
//! as bytes from the state directory the way a person with a file manager
//! would, and the session is populated through the same socket a client uses.
//! A helper that reached into `amx-server` to ask what it had saved would be
//! asking the accused to testify.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use amx_core::{PaneId, WorkspaceId};
use amx_server::persist::{HISTORY_DIR, SNAPSHOT_NAME};
use rig::wire::result_of;
use rig::{Env, TempDir, Wire};
use serde_json::{Value, json};

/// The size every attached client in this suite runs at.
pub const ROWS: u16 = 24;
/// See [`ROWS`].
pub const COLS: u16 = 80;

/// Whether a rasterized screen shows a fully painted frame: the status line
/// padded across the whole bottom row (`tests/firstrun.rs`'s definition).
pub fn painted(screen: &rig::Screen) -> bool {
    (0..COLS).all(|col| screen.contains_key(&(ROWS - 1, col)))
}

/// A shell planted for one test, and the marker its path carries.
///
/// The `tests/firstrun.rs` technique, with the addition M1 needs: a *restored*
/// pane runs the user's shell rather than the argv it had, because the
/// snapshot deliberately stores no argv (D-M1-5). A marker in `$SHELL` is
/// therefore the only marker that survives a reboot, which is what makes
/// "the pane's process came back" a claim the process table can answer.
#[derive(Debug)]
pub struct MarkerShell {
    /// Kept so the script outlives the test that planted it.
    _dir: TempDir,
    path: PathBuf,
    marker: String,
}

impl MarkerShell {
    /// Where the script is, for `$SHELL`.
    pub fn path(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    /// The string its processes carry in the process table.
    pub fn marker(&self) -> &str {
        &self.marker
    }
}

/// Plant a shell whose body is `body` and whose path carries a unique marker.
///
/// `body` runs once at startup; the script then blocks on its terminal, which
/// is the shape of a shell — held open with no wall-clock wait for the rig's
/// hygiene guard to object to.
pub fn marker_shell(tag: &str, body: &str) -> MarkerShell {
    let dir = TempDir::new(tag);
    let marker = format!("amx-rig-{tag}-{}", std::process::id());
    let path = dir.path().join(&marker);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\nread _held\n")).expect("write the shell");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("make it executable");
    MarkerShell {
        _dir: dir,
        path,
        marker,
    }
}

/// The snapshot file's bytes, or `None` before the first save has landed.
pub fn snapshot_bytes(env: &Env) -> Option<Vec<u8>> {
    std::fs::read(env.state_dir().join(SNAPSHOT_NAME)).ok()
}

/// The snapshot as JSON, or `None` before the first save has landed.
///
/// A file that exists but does not parse is *not* `None`: that is the torn
/// write this suite exists to prove impossible, and swallowing it here would
/// turn the proof into a poll that waits for the next save.
pub fn snapshot(env: &Env) -> Option<Value> {
    let bytes = snapshot_bytes(env)?;
    Some(serde_json::from_slice(&bytes).unwrap_or_else(|err| {
        panic!(
            "the snapshot on disk is not valid JSON ({err}); {} bytes held:\n{}",
            bytes.len(),
            String::from_utf8_lossy(&bytes)
        )
    }))
}

/// Whether the snapshot names `text` anywhere — the coarse "has the save that
/// includes this change landed yet?" condition.
pub fn snapshot_mentions(env: &Env, text: &str) -> bool {
    snapshot_bytes(env).is_some_and(|bytes| String::from_utf8_lossy(&bytes).contains(text))
}

/// The staging files left in the state directory (`session.json.<pid>.<n>.tmp`).
pub fn staging_files(env: &Env) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(env.state_dir()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(SNAPSHOT_NAME) && name.ends_with(".tmp"))
        })
        .collect()
}

/// The snapshot file's inode, or 0 before the first save.
///
/// The one number that separates "replaced by rename" from "rewritten in
/// place": the content of both looks the same afterwards, and only one of them
/// is safe to read while it happens.
pub fn inode_of(env: &Env) -> u64 {
    use std::os::unix::fs::MetadataExt as _;

    std::fs::metadata(env.state_dir().join(SNAPSHOT_NAME)).map_or(0, |meta| meta.ino())
}

/// The directory the scrollback sidecars live in.
///
/// Named rather than assumed absent-or-empty, because D-M1-6 makes the
/// directory itself the thing that goes away when history is turned off.
pub fn history_dir(env: &Env) -> PathBuf {
    env.state_dir().join(HISTORY_DIR)
}

/// The scrollback sidecars on disk, by file name.
pub fn sidecars(env: &Env) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(history_dir(env)) else {
        return Vec::new();
    };
    entries.flatten().map(|entry| entry.path()).collect()
}

/// Whether any sidecar on disk holds `text`.
pub fn sidecar_holds(env: &Env, text: &str) -> bool {
    sidecars(env).iter().any(|path| {
        std::fs::read(path).is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(text))
    })
}

/// Wait until a `kill -9` at this instant would leave a reboot everything it
/// needs to replay `pane`'s scrollback: a snapshot that names the pane, and a
/// sidecar that holds `mark`.
///
/// Both, and immediately before the kill, because a save writes them in the
/// opposite order and may skip the second entirely. `Persist::save` dumps the
/// scrollback first and then writes the snapshot *only if the session's shape
/// changed*, so a sidecar appearing on disk is no evidence at all that the pane
/// it belongs to is on disk yet. The gap is as wide as a snapshot's two fsyncs:
/// measured under fsync load, the snapshot did not yet name the pane at the
/// moment its sidecar appeared, in every run.
///
/// A reboot that lands in that gap finds no snapshot, seeds a fresh session,
/// and replays nothing — a pane that renders empty forever with no restore loss
/// to report, because nothing was lost: nothing had been saved.
pub fn crash_durable(env: &Env, pane: PaneId, mark: &str) {
    let named = || snapshot_mentions(env, &pane.to_string());
    rig::wait_until_or(
        "the reboot's evidence is all on disk: the snapshot names the pane and \
         its sidecar holds the scrollback",
        || named() && sidecar_holds(env, mark),
        || {
            format!(
                "the snapshot {} the pane, and {}",
                if named() { "names" } else { "does not name" },
                sidecar_sizes(env)
            )
        },
    );
}

/// What the state directory holds, for a failure that has to name a cause.
///
/// A client that painted an empty frame and went quiet looks the same whether
/// the reboot found no snapshot, no sidecar or an empty one, and its own output
/// cannot tell them apart. This can.
pub fn restore_evidence(env: &Env, pane: PaneId) -> String {
    format!(
        "on disk: the snapshot {} the pane, {}; `session report` says: {}",
        if snapshot_mentions(env, &pane.to_string()) {
            "names"
        } else {
            "does not name"
        },
        sidecar_sizes(env),
        env.run(&["session", "report"]).stdout.trim()
    )
}

/// Every sidecar on disk with its size, which is what tells an empty dump from
/// a missing one.
fn sidecar_sizes(env: &Env) -> String {
    let listed: Vec<String> = sidecars(env)
        .iter()
        .map(|path| {
            let bytes = std::fs::metadata(path).map_or(0, |meta| meta.len());
            format!("{} ({bytes} bytes)", path.display())
        })
        .collect();
    if listed.is_empty() {
        "the history directory holds no sidecars".to_owned()
    } else {
        format!("the history directory holds {}", listed.join(", "))
    }
}

/// Complain, in full, that a snapshot is not a whole one.
///
/// The torn-write check, and the only assertion in this suite that inspects
/// the snapshot's *shape*: a half-written file is either unparseable (caught
/// in [`snapshot`]) or parses into a session whose layouts point at panes the
/// pane table does not hold. Both are the same failure — the file is neither
/// the old state nor the new one — and this is where it is named.
pub fn assert_snapshot_is_whole(env: &Env, when: &str) -> Value {
    let snapshot = snapshot(env).unwrap_or_else(|| panic!("no snapshot on disk {when}"));
    assert_eq!(
        snapshot["version"], 1,
        "the snapshot {when} is not a version this build writes: {snapshot}"
    );
    let panes: Vec<&str> = snapshot["panes"]
        .as_array()
        .unwrap_or_else(|| panic!("the snapshot {when} has no pane table: {snapshot}"))
        .iter()
        .filter_map(|pane| pane["id"].as_str())
        .collect();
    for workspace in snapshot["workspaces"]
        .as_array()
        .unwrap_or_else(|| panic!("the snapshot {when} has no workspace table: {snapshot}"))
    {
        for leaf in leaves(&workspace["layout"]["root"]) {
            assert!(
                panes.contains(&leaf.as_str()),
                "the snapshot {when} has a layout leaf {leaf} with no pane behind it — \
                 it is neither the old state nor the new one: {snapshot}"
            );
        }
    }
    snapshot
}

/// Every pane id a layout tree points at.
fn leaves(node: &Value) -> Vec<String> {
    if let Some(leaf) = node["leaf"].as_str() {
        return vec![leaf.to_owned()];
    }
    let mut found = Vec::new();
    for child in ["first", "second"] {
        let child = &node["split"][child];
        if !child.is_null() {
            found.extend(leaves(child));
        }
    }
    found
}

/// Everything a reboot must reproduce, projected out of `session.state`.
///
/// Volatile fields are dropped on purpose: the bus sequence, the grid size and
/// the history window belong to the *run*, not to the session, and comparing
/// them would assert that a restart is a no-op rather than that the session
/// came back.
pub fn durable_view(state: &Value) -> Value {
    let mut workspaces: Vec<Value> = state["workspaces"]
        .as_array()
        .expect("session.state carries workspaces")
        .iter()
        .map(|ws| {
            json!({
                "workspace": ws["workspace"],
                "short": ws["short"],
                "label": ws["label"],
                "layout": ws["layout"],
                "focus": ws["focus"],
            })
        })
        .collect();
    workspaces.sort_by_key(|ws| ws["workspace"].to_string());
    let mut panes: Vec<Value> = state["panes"]
        .as_array()
        .expect("session.state carries panes")
        .iter()
        .map(|pane| {
            json!({
                "pane": pane["pane"],
                "short": pane["short"],
                "label": pane["label"],
            })
        })
        .collect();
    panes.sort_by_key(|pane| pane["pane"].to_string());
    json!({
        "focused_workspace": state["focused_workspace"],
        "workspaces": workspaces,
        "panes": panes,
    })
}

/// A raw client, connected and past the handshake.
pub async fn connected(env: &Env) -> Wire {
    let mut wire = Wire::connect(&env.socket()).await;
    wire.hello(amx_proto::version::window()).await;
    wire
}

/// `session.state`, as the reply object.
pub async fn session_state(wire: &mut Wire) -> Value {
    result_of(&wire.request("session.state", json!({})).await).clone()
}

/// The pane the session currently has focused, over a connection of its own.
///
/// For the suites that let the *client* create the session — the first attach
/// seeds a workspace, and its root pane is the one the test then asks the disk
/// about — where there is no reply to read the pane id out of.
pub async fn focused_pane(env: &Env) -> PaneId {
    let mut wire = connected(env).await;
    let state = session_state(&mut wire).await;
    let focused = state["focused_workspace"]
        .as_str()
        .expect("an attached session has a focused workspace");
    let workspace = state["workspaces"]
        .as_array()
        .expect("session.state carries workspaces")
        .iter()
        .find(|ws| ws["workspace"].as_str() == Some(focused))
        .expect("the focused workspace is in the table");
    serde_json::from_value(workspace["focus"].clone()).expect("the focused workspace has a pane")
}

/// Create a labelled, focused workspace and learn its root pane.
pub async fn workspace(wire: &mut Wire, label: &str) -> (WorkspaceId, PaneId) {
    let created = wire
        .request("workspace.create", json!({ "label": label, "focus": true }))
        .await;
    let workspace: WorkspaceId =
        serde_json::from_value(result_of(&created)["workspace"].clone()).expect("a workspace id");
    let root = switch(wire, workspace).await;
    (workspace, root)
}

/// Focus `workspace`, and learn the pane focused inside it.
pub async fn switch(wire: &mut Wire, workspace: WorkspaceId) -> PaneId {
    let switched = wire
        .request("workspace.switch", json!({ "workspace": workspace }))
        .await;
    serde_json::from_value(result_of(&switched)["focused_pane"].clone())
        .expect("a focused workspace has a focused pane")
}

/// Split `pane`, putting the new pane's shell in `cwd`.
pub async fn split_in(wire: &mut Wire, pane: PaneId, cwd: &Path) -> PaneId {
    let reply = wire
        .request(
            "pane.split",
            json!({ "pane": pane, "direction": "vertical", "cwd": cwd }),
        )
        .await;
    serde_json::from_value(result_of(&reply)["pane"].clone()).expect("the new pane's id")
}

/// Give `pane` a label over the wire.
pub async fn rename(wire: &mut Wire, pane: PaneId, label: &str) {
    let reply = wire
        .request("pane.rename", json!({ "pane": pane, "label": label }))
        .await;
    result_of(&reply);
}

/// A directory under `root`, created.
pub fn dir(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::create_dir_all(&path).expect("create a working directory");
    path
}

/// `path` with every symlink resolved, as a string.
///
/// What a shell's `pwd -P` prints, and the only form two sides of a cwd
/// comparison can agree on: darwin's `$TMPDIR` lives under a symlinked
/// `/var`, so the path a test hands the server and the path the process
/// reports differ by that link alone.
pub fn physical(path: &Path) -> String {
    std::fs::canonicalize(path)
        .expect("canonicalize a directory the test made")
        .to_string_lossy()
        .into_owned()
}
