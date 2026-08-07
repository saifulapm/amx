//! The recording sync seam, and the snapshots and rows the suite writes.
//!
//! The seam is the load-bearing piece. Power loss cannot be staged in CI, so
//! what the suite checks is the *sequence* the durability argument rests on,
//! and it checks it by standing in for the two `File::sync_all` calls and
//! looking at the filesystem at each one: at the file sync the payload must
//! already be whole and the destination name unclaimed, at the directory sync
//! the destination must hold it. A [`Recorder`] can also fail either sync on
//! demand, which is how the abort-before-rename and committed-but-unsynced
//! paths get exercised without a hostile disk.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use amx_core::{Direction, Env, Layout, PaneId, ShortNumber, WorkspaceId};
use amx_proto::stream::history::put_row;
use amx_server::persist::io::Syncs;
use amx_server::persist::{PaneSnapshot, Snapshot, VERSION, WorkspaceSnapshot};

/// One observed sync, with what the filesystem looked like at that moment.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// A staging file was synced. `dest_exists` answers "has the rename
    /// happened yet?" — it must not have.
    FileSync {
        /// The staging file's name, which carries the pid and a counter.
        staged: String,
        /// How much of the payload was in it.
        staged_len: u64,
        /// Whether the destination name was already claimed.
        dest_exists: bool,
    },
    /// A directory was synced. `dest_len` answers "is the committed file whole
    /// by now?" — it must be.
    DirSync {
        /// The directory that was synced.
        dir: PathBuf,
        /// The size of the watched destination file, if it exists yet.
        dest_len: Option<u64>,
    },
}

/// A [`Syncs`] that records the sequence and can fail on demand.
pub struct Recorder {
    dest: PathBuf,
    steps: Mutex<Vec<Step>>,
    file_sync_fails: bool,
    dir_sync_fails: bool,
}

impl Recorder {
    /// A recorder watching `dest`, syncing for real.
    pub fn watching(dest: &Path) -> Self {
        Self {
            dest: dest.to_path_buf(),
            steps: Mutex::new(Vec::new()),
            file_sync_fails: false,
            dir_sync_fails: false,
        }
    }

    /// One whose file sync fails, as a disk out of space or in error does.
    pub fn failing_file_sync(dest: &Path) -> Self {
        Self {
            file_sync_fails: true,
            ..Self::watching(dest)
        }
    }

    /// One whose directory sync fails after the rename has landed.
    pub fn failing_dir_sync(dest: &Path) -> Self {
        Self {
            dir_sync_fails: true,
            ..Self::watching(dest)
        }
    }

    /// Everything observed so far, in order.
    pub fn steps(&self) -> Vec<Step> {
        std::mem::take(&mut self.steps.lock().unwrap())
    }
}

impl Syncs for Recorder {
    fn sync_file(&self, file: &File, path: &Path) -> io::Result<()> {
        self.steps.lock().unwrap().push(Step::FileSync {
            staged: path.file_name().unwrap().to_str().unwrap().to_owned(),
            staged_len: file.metadata()?.len(),
            dest_exists: self.dest.exists(),
        });
        if self.file_sync_fails {
            return Err(io::Error::other("the disk said no"));
        }
        file.sync_all()
    }

    fn sync_dir(&self, dir: &File, path: &Path) -> io::Result<()> {
        self.steps.lock().unwrap().push(Step::DirSync {
            dir: path.to_path_buf(),
            dest_len: fs::metadata(&self.dest).ok().map(|meta| meta.len()),
        });
        if self.dir_sync_fails {
            return Err(io::Error::other("the disk said no"));
        }
        dir.sync_all()
    }
}

/// Every `.tmp` file left in `dir`, sorted.
pub fn staging_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("read the directory")
        .map(|entry| entry.expect("an entry").file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    names.sort();
    names
}

/// The first pane of the fixture session.
pub fn pane_id() -> PaneId {
    "00000000-0000-0000-0000-0000000000a1".parse().unwrap()
}

/// The pane it was split into.
pub fn other_pane_id() -> PaneId {
    "00000000-0000-0000-0000-0000000000a2".parse().unwrap()
}

/// The workspace holding both.
pub fn workspace_id() -> WorkspaceId {
    "00000000-0000-0000-0000-0000000000b1".parse().unwrap()
}

/// The session the v1 golden freezes: two panes, one split, labels, a cwd.
///
/// Fixed UUIDs, because a golden of freshly minted ones would freeze nothing.
pub fn populated() -> Snapshot {
    let mut layout = Layout::with_root(pane_id());
    layout
        .split(pane_id(), Direction::Right, other_pane_id(), 0.5)
        .expect("split the fixture layout");
    Snapshot {
        version: VERSION,
        focused_workspace: Some(workspace_id()),
        workspaces: vec![WorkspaceSnapshot {
            id: workspace_id(),
            short: ShortNumber::new(1),
            label: Some("build".to_owned()),
            layout,
            focus: Some(other_pane_id()),
        }],
        panes: vec![
            PaneSnapshot {
                id: pane_id(),
                short: ShortNumber::new(1),
                label: Some("editor".to_owned()),
                cwd: Some(PathBuf::from("/home/s/amx")),
            },
            PaneSnapshot {
                id: other_pane_id(),
                short: ShortNumber::new(2),
                label: None,
                cwd: None,
            },
        ],
    }
}

/// Pack `rows` exactly the way the history stream does.
///
/// Through the wire's own `put_row`, deliberately: if the sidecar had its own
/// encoder the file and the wire could drift, and the whole point of the
/// format is that they cannot.
pub fn packed(rows: &[(&str, bool)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (text, wrapped) in rows {
        put_row(text.as_bytes(), *wrapped, &mut out);
    }
    out
}

/// An `Env` whose every root is under `root`.
pub fn env_under(root: &Path) -> Env {
    Env {
        runtime_dir: Some(root.join("run")),
        state_home: Some(root.join("state")),
        config_home: Some(root.join("config")),
        home: Some(root.join("home")),
        session: None,
        tmpdir: Some(root.join("tmp")),
    }
}
