//! U02: the durable write, the snapshot file, the sidecar codec.
//!
//! The claim these tests defend is 04 §6's: a snapshot is "written atomic
//! tmp+rename + fsync of file and directory", read through an N/N−1 window, and
//! a pane's scrollback lives in its own file keyed by UUID. Power loss cannot
//! be staged in CI, so what is pinned here is the *sequence* — the payload
//! complete before the file sync, the file sync before the rename, the rename
//! before the directory sync — through the sync seam, plus every observable
//! consequence of it: a torn staging file is never read, a failed sync leaves
//! the previous snapshot whole, and a file from a newer amx is refused with an
//! error the restore report can print.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::fs;
use std::path::{Path, PathBuf};

use amx_core::{Ctx, RowId, RowRange, SessionName};
use amx_server::persist::io::{Commit, SyncAll, write_atomic};
use amx_server::persist::{
    HISTORY_DIR, PersistError, SIDECAR_MAGIC, SNAPSHOT_NAME, SidecarHeader, Snapshot, VERSION,
    sidecar, sidecar_name, snapshot,
};
use amx_server::session::registry::{self, DeleteError};

// A test crate root's module directory is `tests/`, so the fixtures need their
// path spelled out to live beside their one suite rather than next to
// everybody's.
#[path = "persist_io/fixtures.rs"]
mod fixtures;
mod support;

use fixtures::{
    Recorder, Step, env_under, other_pane_id, packed, pane_id, populated, staging_files,
};
use support::TempDir;

// ------------------------------------------------------------- write_atomic

#[test]
fn write_atomic_syncs_file_before_rename_and_directory_after() {
    let dir = TempDir::new("order");
    let dest = dir.path().join("state.json");
    let recorder = Recorder::watching(&dest);
    let payload = b"the whole payload, all at once";

    let commit = write_atomic(dir.path(), "state.json", payload, &recorder).expect("the write");
    assert!(commit.is_durable());

    let steps = recorder.steps();
    let [file_sync, dir_sync] = steps.as_slice() else {
        panic!("one file sync and one directory sync, in that order: {steps:?}");
    };

    // Step 3: the payload is already whole in the staging file, and the
    // destination name has not been claimed yet — so a crash here leaves the
    // previous file, and a rename can never publish a short one.
    let Step::FileSync {
        staged,
        staged_len,
        dest_exists,
    } = file_sync
    else {
        panic!("the file sync comes first: {steps:?}");
    };
    assert_eq!(*staged_len, payload.len() as u64);
    assert!(!dest_exists, "the rename must not have happened yet");
    // pid and a counter, because a fixed `state.json.tmp` lets two writers to
    // one directory clobber each other's staging file.
    assert!(
        staged.starts_with("state.json.") && staged.ends_with(".tmp"),
        "{staged}",
    );
    assert!(
        staged.contains(&format!(".{}.", std::process::id())),
        "the staging name carries the pid: {staged}",
    );

    // Step 5: the directory holding the committed name is synced, and by then
    // the committed file is whole. Without this the rename itself can vanish.
    let Step::DirSync {
        dir: synced,
        dest_len,
    } = dir_sync
    else {
        panic!("the directory sync comes last: {steps:?}");
    };
    assert_eq!(synced, dir.path());
    assert_eq!(*dest_len, Some(payload.len() as u64));

    assert_eq!(fs::read(&dest).expect("the committed file"), payload);
    assert!(
        staging_files(dir.path()).is_empty(),
        "no litter left behind"
    );
}

#[test]
fn failed_file_sync_leaves_the_previous_snapshot_untouched() {
    let dir = TempDir::new("failsync");
    let dest = dir.path().join(SNAPSHOT_NAME);
    snapshot::save(dir.path(), &populated(), &SyncAll).expect("the first save");
    let before = fs::read(&dest).expect("the first snapshot");

    let mut doomed = populated();
    doomed.panes[0].label = Some("never written".to_owned());
    let recorder = Recorder::failing_file_sync(&dest);
    let err = snapshot::save(dir.path(), &doomed, &recorder).expect_err("the sync failed");
    assert!(matches!(err, PersistError::Io(_)), "{err}");

    // Aborted before the rename: the previous snapshot is whole, byte for
    // byte, and the staging file it would have replaced is gone.
    assert_eq!(fs::read(&dest).expect("still there"), before);
    assert!(staging_files(dir.path()).is_empty());
    assert_eq!(
        snapshot::load(dir.path()).expect("the old snapshot still reads"),
        Some(populated()),
    );
}

#[test]
fn a_failed_directory_sync_is_reported_but_the_content_is_committed() {
    let dir = TempDir::new("faildir");
    let dest = dir.path().join(SNAPSHOT_NAME);
    let recorder = Recorder::failing_dir_sync(&dest);

    // The rename already happened, so rolling back would replace correct
    // content with nothing. It is reported instead: only durability against a
    // power loss in the next moments is weaker.
    let commit = snapshot::save(dir.path(), &populated(), &recorder).expect("committed anyway");
    assert!(!commit.is_durable());
    assert!(matches!(commit, Commit::DirectoryUnsynced(_)));
    assert!(commit.directory_error().is_some());
    assert_eq!(snapshot::load(dir.path()).unwrap(), Some(populated()));
}

#[test]
fn first_save_creates_the_state_directory_and_syncs_the_parents_it_creates() {
    let dir = TempDir::new("mkdir");
    let state_dir = dir.path().join("state/amx/work");
    let recorder = Recorder::watching(&state_dir.join(SNAPSHOT_NAME));

    snapshot::save(&state_dir, &Snapshot::empty(), &recorder).expect("the first save");

    // A directory whose parent was never synced can be gone after a power
    // loss, taking a perfectly synced snapshot inside it with it — so every
    // level this created is followed by a sync of the level above.
    let synced: Vec<PathBuf> = recorder
        .steps()
        .into_iter()
        .filter_map(|step| match step {
            Step::DirSync { dir, .. } => Some(dir),
            Step::FileSync { .. } => None,
        })
        .collect();
    for created in [dir.path().join("state"), dir.path().join("state/amx")] {
        let parent = created.parent().unwrap().to_path_buf();
        assert!(synced.contains(&parent), "{parent:?} not in {synced:?}");
    }
    assert!(synced.contains(&state_dir), "the file's own directory too");
}

#[test]
fn stray_tmp_files_are_ignored_by_load_and_removed_by_the_next_save() {
    let dir = TempDir::new("stray");

    // What a crash between staging and rename leaves: a half-written file
    // under a name nothing ever reads.
    let torn = dir.path().join(format!("{SNAPSHOT_NAME}.4242.0.tmp"));
    fs::write(&torn, b"{\"version\": 1, \"workspa").expect("plant a torn staging file");
    let unrelated = dir.path().join("other.json.4242.0.tmp");
    fs::write(&unrelated, b"not ours").expect("plant a foreign staging file");

    assert_eq!(
        snapshot::load(dir.path()).expect("a torn staging file is not a snapshot"),
        None,
        "load reads the committed name and nothing else",
    );

    snapshot::save(dir.path(), &populated(), &SyncAll).expect("the save");
    assert_eq!(snapshot::load(dir.path()).unwrap(), Some(populated()));
    assert!(!torn.exists(), "the next save sweeps its own litter");
    assert!(
        unrelated.exists(),
        "and only its own: staging files for another name are not ours to remove",
    );
}

// ------------------------------------------------------------ the snapshot

/// The checked-in goldens, shared with the protocol suite's tree.
fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/goldens/persist")
        .join(name)
}

#[test]
fn snapshot_v1_golden_matches() {
    let dir = TempDir::new("golden");
    snapshot::save(dir.path(), &populated(), &SyncAll).expect("save the fixture");
    let written = fs::read(dir.path().join(SNAPSHOT_NAME)).expect("read it back");
    let path = golden_path("session-v1.json");

    if std::env::var_os("AMX_UPDATE_GOLDENS").is_some() {
        fs::create_dir_all(path.parent().unwrap()).expect("the goldens directory");
        fs::write(&path, &written).unwrap_or_else(|e| panic!("writing golden {path:?}: {e}"));
        return;
    }

    let expected = fs::read(&path).unwrap_or_else(|e| {
        panic!("reading golden {path:?}: {e} (run with AMX_UPDATE_GOLDENS=1 to create it)")
    });
    assert_eq!(
        String::from_utf8_lossy(&written),
        String::from_utf8_lossy(&expected),
        "the v1 bytes changed; regenerate with AMX_UPDATE_GOLDENS=1 after an intentional change",
    );

    // The frozen bytes are not just a shape: this build reads them back into
    // the same session it would have restored yesterday.
    let read_back = TempDir::new("goldrd");
    fs::write(read_back.path().join(SNAPSHOT_NAME), &expected).expect("plant the golden");
    assert_eq!(snapshot::load(read_back.path()).unwrap(), Some(populated()));
}

#[test]
fn newer_version_is_refused_with_a_reportable_error() {
    let dir = TempDir::new("newer");
    // Version first, shape second: a v2 file may hold fields this build cannot
    // parse at all, and answering "malformed" would send the user hunting for
    // corruption that is not there.
    fs::write(
        dir.path().join(SNAPSHOT_NAME),
        br#"{"version": 2, "workspaces": {"keyed": "differently"}}"#,
    )
    .expect("plant a snapshot from the future");

    let err = snapshot::load(dir.path()).expect_err("a newer snapshot is refused");
    assert!(
        matches!(err, PersistError::NewerVersion { found } if found == VERSION + 1),
        "{err}",
    );
    // Restore prints this, so it has to name what happened.
    assert!(err.to_string().contains("newer amx"), "{err}");
}

#[test]
fn corrupt_snapshot_is_refused_not_partially_applied() {
    let dir = TempDir::new("corrupt");
    let path = dir.path().join(SNAPSHOT_NAME);

    for (what, bytes) in [
        ("truncated mid-object", &br#"{"version": 1, "workspaces": [{"id":"#[..]),
        ("not json at all", &b"\x00\x01\x02 binary garbage"[..]),
        ("empty", &b""[..]),
        ("no version field", &br#"{"workspaces": [], "panes": []}"#[..]),
        (
            "a workspace missing its layout",
            &br#"{"version": 1, "workspaces": [{"id": "00000000-0000-0000-0000-0000000000b1", "short": 1}], "panes": []}"#[..],
        ),
    ] {
        fs::write(&path, bytes).expect("plant the corrupt file");
        let err = snapshot::load(dir.path()).expect_err(what);
        assert!(matches!(err, PersistError::Malformed(_)), "{what}: {err}");
    }
}

#[test]
fn a_v1_snapshot_with_unknown_fields_still_loads() {
    let dir = TempDir::new("unknown");
    fs::write(
        dir.path().join(SNAPSHOT_NAME),
        br#"{
          "version": 1,
          "written_by": "amx 9.0",
          "workspaces": [],
          "panes": [{
            "id": "00000000-0000-0000-0000-0000000000a1",
            "short": 1,
            "argv": ["nvim", "."]
          }]
        }"#,
    )
    .expect("plant a snapshot with fields this build has never heard of");

    // The wire's tolerance rule, applied to the file (04 §4): inside the
    // window, a later version adding fields must not strand this reader.
    let loaded = snapshot::load(dir.path()).expect("unknown fields are ignored");
    let loaded = loaded.expect("there is a snapshot");
    assert_eq!(loaded.panes[0].id, pane_id());
    assert!(loaded.panes[0].cwd.is_none());
}

#[test]
fn a_missing_snapshot_is_the_first_run_case_not_an_error() {
    let dir = TempDir::new("firstrun");
    assert_eq!(snapshot::load(dir.path()).expect("no file, no error"), None);
    assert_eq!(
        snapshot::load(&dir.path().join("never/created")).expect("no directory either"),
        None,
    );
}

// -------------------------------------------------------------- the sidecar

#[test]
fn sidecar_round_trips_rows_and_detects_truncation() {
    let dir = TempDir::new("sidecar");
    let range = RowRange::new(RowId::from_raw(40), RowId::from_raw(42));
    let header = SidecarHeader::new(pane_id(), range);
    let rows = packed(&[
        ("a long line that soft", true),
        ("wraps", false),
        ("$ ls", false),
    ]);

    sidecar::save(dir.path(), &header, &rows, &SyncAll).expect("dump the sidecar");
    let path = dir.path().join(HISTORY_DIR).join(sidecar_name(pane_id()));
    assert!(path.exists(), "history/<pane-uuid>.rows");
    assert_eq!(fs::read(&path).unwrap()[..4], SIDECAR_MAGIC);

    let loaded = sidecar::load(dir.path(), pane_id())
        .expect("read it back")
        .expect("there is one");
    assert_eq!(loaded.header, header);
    assert_eq!(loaded.header.range, range);
    assert!(!loaded.truncated);
    assert_eq!(loaded.rows.len(), 3);
    assert_eq!(loaded.rows[0].text, b"a long line that soft");
    assert!(loaded.rows[0].wrapped, "the wrap flag is what joins lines");
    assert!(!loaded.rows[1].wrapped);
    assert_eq!(loaded.rows[2].text, b"$ ls");

    // A file cut mid-row: everything up to the tear replays, and the tear is
    // reported so restore can degrade the pane instead of losing it.
    let whole = fs::read(&path).unwrap();
    fs::write(&path, &whole[..whole.len() - 6]).expect("tear off the last row");
    let torn = sidecar::load(dir.path(), pane_id()).unwrap().unwrap();
    assert!(torn.truncated, "the tear is reported, not swallowed");
    assert_eq!(torn.rows.len(), 2, "every complete row still replays");
    assert_eq!(torn.rows[1].text, b"wraps");
}

#[test]
fn a_sidecar_that_is_not_one_is_refused_before_a_single_row_is_replayed() {
    let dir = TempDir::new("badside");
    let history = dir.path().join(HISTORY_DIR);
    fs::create_dir_all(&history).expect("the history directory");
    let path = history.join(sidecar_name(pane_id()));

    for (what, bytes) in [
        ("shorter than a header", vec![b'A'; 8]),
        ("no magic", vec![0_u8; SidecarHeader::ENCODED_LEN + 4]),
    ] {
        fs::write(&path, &bytes).expect("plant it");
        let err = sidecar::load(dir.path(), pane_id()).expect_err(what);
        assert!(matches!(err, PersistError::BadSidecar(_)), "{what}: {err}");
    }

    // A future format version, which by D-M1-6 is when rows learn styling.
    // Replaying rows packed a way this build does not understand would put
    // garbage on a terminal, so it refuses instead of guessing.
    let mut future = Vec::new();
    SidecarHeader {
        version: 2,
        ..SidecarHeader::new(pane_id(), RowRange::new(RowId::FIRST, RowId::FIRST))
    }
    .encode(&mut future);
    fs::write(&path, &future).expect("plant it");
    let err = sidecar::load(dir.path(), pane_id()).expect_err("a newer sidecar format");
    assert!(matches!(err, PersistError::BadSidecar(_)), "{err}");

    // The pane is in the header as well as the name, so a file that arrived
    // under the wrong name never replays into somebody else's scrollback.
    let mine = SidecarHeader::new(other_pane_id(), RowRange::new(RowId::FIRST, RowId::FIRST));
    fs::write(&path, sidecar::encode(&mine, &packed(&[("theirs", false)])))
        .expect("plant another pane's dump under this pane's name");
    let err = sidecar::load(dir.path(), pane_id()).expect_err("the pane does not match");
    assert!(matches!(err, PersistError::BadSidecar(_)), "{err}");
}

#[test]
fn sidecars_are_per_pane_and_wiped_independently_of_the_snapshot() {
    let dir = TempDir::new("wipe");
    snapshot::save(dir.path(), &populated(), &SyncAll).expect("the snapshot");
    for pane in [pane_id(), other_pane_id()] {
        let header = SidecarHeader::new(pane, RowRange::new(RowId::FIRST, RowId::FIRST));
        sidecar::save(dir.path(), &header, &packed(&[("secret", false)]), &SyncAll)
            .expect("dump one pane");
    }

    // One file per pane, keyed by UUID: no positional zipping to desync, and
    // one pane's loss costs exactly one pane's scrollback.
    assert!(sidecar::load(dir.path(), pane_id()).unwrap().is_some());
    assert!(sidecar::remove(dir.path(), pane_id(), &SyncAll).expect("remove one"));
    assert!(sidecar::load(dir.path(), pane_id()).unwrap().is_none());
    assert!(
        sidecar::load(dir.path(), other_pane_id())
            .unwrap()
            .is_some()
    );
    assert!(!sidecar::remove(dir.path(), pane_id(), &SyncAll).unwrap());

    // Turning history off wipes scrollback immediately — it holds secrets —
    // and leaves the session itself restorable.
    assert!(sidecar::wipe(dir.path()).expect("wipe"));
    assert!(!sidecar::dir(dir.path()).exists());
    assert!(
        sidecar::load(dir.path(), other_pane_id())
            .unwrap()
            .is_none()
    );
    assert_eq!(snapshot::load(dir.path()).unwrap(), Some(populated()));
    assert!(!sidecar::wipe(dir.path()).expect("wiping twice is not an error"));
}

// ----------------------------------------------------------- session delete

#[test]
fn session_delete_removes_populated_state_dir() {
    let dir = TempDir::new("del");
    let env = env_under(dir.path());
    let ctx = Ctx::for_session(SessionName::new("doomed").unwrap(), &env).expect("derive a ctx");

    // Before M1 the state directory was always empty, so `remove_dir` was
    // enough; it now holds a snapshot and a sidecar per pane.
    snapshot::save(&ctx.state_dir, &populated(), &SyncAll).expect("save state");
    let header = SidecarHeader::new(pane_id(), RowRange::new(RowId::FIRST, RowId::FIRST));
    sidecar::save(
        &ctx.state_dir,
        &header,
        &packed(&[("$ ls", false)]),
        &SyncAll,
    )
    .expect("dump scrollback");
    assert!(snapshot::path(&ctx.state_dir).exists());

    registry::delete(&ctx).expect("a populated state directory deletes");
    assert!(!ctx.state_dir.exists(), "the whole tree is gone");

    let missing = registry::delete(&ctx).expect_err("nothing left to delete");
    assert!(
        matches!(missing, DeleteError::NoSuchSession { .. }),
        "{missing}"
    );
}

#[test]
fn session_delete_refuses_a_directory_the_session_does_not_own() {
    let dir = TempDir::new("pin");
    let env = env_under(dir.path());
    let mut ctx = Ctx::for_session(SessionName::new("doomed").unwrap(), &env).expect("ctx");
    let precious = dir.path().join("home/Documents");
    fs::create_dir_all(&precious).expect("something worth not deleting");
    fs::write(precious.join("thesis.txt"), b"years of work").expect("write it");

    // `Ctx`'s fields are public, so the recursive delete re-derives its own
    // permission: the path must be the `<root>/amx/<session>` directory this
    // context names, not whatever a caller put on the struct.
    ctx.state_dir = precious.clone();
    let err = registry::delete(&ctx).expect_err("a hand-assembled path is refused");
    assert!(matches!(err, DeleteError::NotSessionOwned { .. }), "{err}");
    assert!(
        precious.join("thesis.txt").exists(),
        "and nothing was touched"
    );

    // The session's own name under amx's own level is what passes.
    ctx.state_dir = dir.path().join("state/amx/doomed");
    fs::create_dir_all(&ctx.state_dir).expect("the real state dir");
    registry::delete(&ctx).expect("the session's own directory deletes");
    assert!(!ctx.state_dir.exists());
}

#[test]
fn session_delete_does_not_follow_symlinks_out_of_the_state_dir() {
    let dir = TempDir::new("link");
    let env = env_under(dir.path());
    let ctx = Ctx::for_session(SessionName::new("doomed").unwrap(), &env).expect("ctx");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&outside).expect("a directory outside the session");
    fs::write(outside.join("keep.txt"), b"not yours").expect("write it");

    snapshot::save(&ctx.state_dir, &Snapshot::empty(), &SyncAll).expect("save state");
    std::os::unix::fs::symlink(&outside, ctx.state_dir.join("escape")).expect("plant a link");

    registry::delete(&ctx).expect("delete");
    assert!(!ctx.state_dir.exists(), "the session's tree is gone");
    assert!(
        outside.join("keep.txt").exists(),
        "the link was unlinked, not descended",
    );
}
