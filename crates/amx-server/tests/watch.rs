//! The config watcher, against a real directory and the real platform API.
//!
//! One suite, both platforms: macOS is tier 1 (03) and CI runs exactly this
//! file there, which is the point — inotify and kqueue have different shapes
//! and the same contract, so the contract is what gets tested.
//!
//! Nothing here naps on the wall clock to *reach* a condition; the waits are
//! either a bounded wait on a notification (`expect_change`) or a bounded
//! window in which no notification may arrive (`expect_quiet`), and a negative
//! assertion has no other honest form.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use amx_server::platform::watch::{
    ConfigWatcher, Debounce, MAX_DELAY, QUIET_WINDOW, watch_config, watch_path,
};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

mod support;

use support::{PATIENCE, TempDir, ctx_under};

/// The debounce the suite drives.
///
/// The same shape as the shipped pair, wound down so nine tests do not spend
/// their lives waiting out half-second quiet windows;
/// `the_shipped_watcher_uses_the_documented_debounce` pins the real values.
const FAST: Debounce = Debounce {
    quiet: Duration::from_millis(30),
    cap: Duration::from_millis(500),
};

/// How long a "nothing should arrive" window stays open.
///
/// Comfortably past `FAST.cap`, so a notification the watcher intended to send
/// has had every chance to land before the window closes.
const SETTLE: Duration = Duration::from_millis(750);

/// A watcher on `config.toml` under a fresh temporary directory.
///
/// The config directory is deliberately *not* created first: `watch_path` owns
/// creating it, and every test therefore exercises that.
fn watching(tag: &str) -> (TempDir, PathBuf, ConfigWatcher) {
    let dir = TempDir::new(tag);
    let path = dir.path().join("config/amx/config.toml");
    let watcher =
        watch_path(&path, CancellationToken::new(), FAST).expect("establish the config watch");
    (dir, path, watcher)
}

/// Fail unless a notification arrives.
async fn expect_change(watcher: &mut ConfigWatcher, after: &str) {
    let change = timeout(PATIENCE, watcher.changed())
        .await
        .unwrap_or_else(|_| panic!("no notification after {after}"));
    assert!(
        change.is_some(),
        "the watcher stopped instead of reporting {after}",
    );
}

/// Fail if a notification arrives within [`SETTLE`].
async fn expect_quiet(watcher: &mut ConfigWatcher, about: &str) {
    assert!(
        timeout(SETTLE, watcher.changed()).await.is_err(),
        "the watcher reported a change for {about}",
    );
}

/// Write a file, failing the test if it cannot be written.
fn write(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write the file");
}

#[tokio::test]
async fn rename_over_the_config_file_is_observed() {
    // The atomic writer's shape (D-M1-1) and every editor's: a staging file
    // beside the target, renamed over the top. A watch scoped to the file
    // itself would still be watching the replaced inode after this.
    let (dir, path, mut watcher) = watching("rnov");
    write(&path, "[persist]\nhistory = false\n");
    expect_change(&mut watcher, "the file first appeared").await;

    let staging = path.with_extension("toml.4242.1.tmp");
    write(&staging, "[persist]\nhistory = true\n");
    fs::rename(&staging, &path).expect("rename over the config file");
    expect_change(&mut watcher, "a rename over the config file").await;

    assert_eq!(
        fs::read_to_string(&path).expect("read"),
        "[persist]\nhistory = true\n",
        "the watcher reports the name, and the name now holds the new bytes",
    );
    drop(dir);
}

#[tokio::test]
async fn in_place_write_is_observed() {
    let (dir, path, mut watcher) = watching("inpl");
    write(&path, "[terminal]\nshell = \"/bin/sh\"\n");
    expect_change(&mut watcher, "the file first appeared").await;

    write(&path, "[terminal]\nshell = \"/bin/zsh\"\n");
    expect_change(&mut watcher, "an in-place rewrite").await;
    drop(dir);
}

#[tokio::test]
async fn delete_then_recreate_resumes_watching() {
    let (dir, path, mut watcher) = watching("delr");
    write(&path, "[persist]\nhistory = true\n");
    expect_change(&mut watcher, "the file first appeared").await;

    fs::remove_file(&path).expect("remove the config file");
    expect_change(&mut watcher, "the file was deleted").await;

    write(&path, "[persist]\nhistory = false\n");
    expect_change(&mut watcher, "the file was recreated").await;

    // The re-armed watch has to survive a plain write, not just the recreate:
    // on darwin this is the descriptor the recreate re-opened being live.
    write(&path, "[persist]\nhistory = true\n");
    expect_change(&mut watcher, "a write to the recreated file").await;
    drop(dir);
}

#[tokio::test]
async fn a_burst_of_writes_coalesces_to_one_notification() {
    // A wider quiet window than the rest of the suite uses: no plausible
    // scheduling hiccup opens 200 ms of silence inside a burst written 2 ms
    // apart, so a second notification here means the burst really was split.
    let debounce = Debounce {
        quiet: Duration::from_millis(200),
        cap: Duration::from_secs(1),
    };
    let dir = TempDir::new("brst");
    let path = dir.path().join("config/amx/config.toml");
    let mut watcher =
        watch_path(&path, CancellationToken::new(), debounce).expect("establish the config watch");

    // Written while the consumer is already waiting, which is what a real
    // editor's save looks like: a watcher with no quiet window would report the
    // first write of the burst and leave the other seven queued behind it. The
    // yield hands the runtime to that consumer between writes — a scheduling
    // point, not a wall-clock window, so nothing here depends on how long a
    // write takes.
    let target = path.clone();
    let burst = tokio::spawn(async move {
        for n in 0..8 {
            write(&target, &format!("[persist]\nhistory = {}\n", n % 2 == 0));
            tokio::task::yield_now().await;
        }
    });
    expect_change(&mut watcher, "a burst of eight writes").await;
    burst.await.expect("the burst finished");
    expect_quiet(&mut watcher, "the tail of a burst already reported").await;
    drop(dir);
}

#[tokio::test]
async fn unrelated_files_in_the_directory_are_ignored() {
    // The watch is on the directory, so everything in it is visible; only
    // `config.toml` may be reported. On darwin this is the whole reason the
    // file is re-identified by path after a directory event.
    let (dir, path, mut watcher) = watching("unre");
    let neighbour = path.with_file_name("colors.toml");
    write(&neighbour, "unrelated\n");
    write(&path.with_file_name("notes.txt"), "also unrelated\n");
    // A staging file that is never renamed: the same name shape the atomic
    // writer uses, which must not be mistaken for the file it stages.
    write(&path.with_file_name("config.toml.99.1.tmp"), "staged\n");
    fs::remove_file(&neighbour).expect("remove the neighbour");
    expect_quiet(&mut watcher, "files that are not config.toml").await;

    // …and the watch is still live, which is what makes the silence above
    // evidence of filtering rather than of a dead watcher.
    write(&path, "[persist]\nhistory = true\n");
    expect_change(&mut watcher, "the config file itself").await;
    drop(dir);
}

#[tokio::test]
async fn watcher_stops_promptly_on_cancellation() {
    let dir = TempDir::new("canc");
    let path = dir.path().join("config/amx/config.toml");
    let cancel = CancellationToken::new();
    let mut watcher = watch_path(&path, cancel.clone(), FAST).expect("establish the config watch");

    cancel.cancel();
    let stopped = timeout(PATIENCE, watcher.changed())
        .await
        .expect("the watcher answered cancellation");
    assert!(stopped.is_none(), "cancellation ends the stream of changes");

    // Idempotent, and still `None`: a cancelled watcher has joined its thread
    // and has nothing left to report.
    assert!(
        timeout(PATIENCE, watcher.changed())
            .await
            .expect("a second wait returns at once")
            .is_none(),
    );
    drop(dir);
}

#[tokio::test]
async fn a_change_survives_a_dropped_wait() {
    // U08 drives the watcher from a `select!`, which drops the pending
    // `changed()` future every time another branch wins. A change observed
    // inside a dropped future must not be lost with it.
    let (dir, path, mut watcher) = watching("drop");
    write(&path, "[persist]\nhistory = true\n");

    // Shorter than `FAST.quiet`, so this always expires mid-coalesce.
    assert!(
        timeout(Duration::from_millis(1), watcher.changed())
            .await
            .is_err(),
    );
    expect_change(&mut watcher, "a wait that was dropped mid-coalesce").await;
    drop(dir);
}

#[tokio::test]
async fn a_change_is_reported_at_the_cap_when_quiet_never_comes() {
    // herdr's trailing-only debounce starves under sustained change
    // (D-M1-7). With a quiet window longer than the test's whole patience
    // budget, the only thing that can deliver this notification is the cap.
    let dir = TempDir::new("cap");
    let path = dir.path().join("config/amx/config.toml");
    let debounce = Debounce {
        quiet: Duration::from_secs(600),
        cap: Duration::from_millis(200),
    };
    let mut watcher =
        watch_path(&path, CancellationToken::new(), debounce).expect("establish the config watch");

    write(&path, "[persist]\nhistory = true\n");
    expect_change(&mut watcher, "the staleness cap elapsed").await;
    drop(dir);
}

#[tokio::test]
async fn the_shipped_watcher_uses_the_documented_debounce() {
    assert_eq!(QUIET_WINDOW, Duration::from_millis(500));
    assert_eq!(MAX_DELAY, Duration::from_secs(5));
    assert_eq!(
        Debounce::default(),
        Debounce {
            quiet: QUIET_WINDOW,
            cap: MAX_DELAY,
        },
    );

    // `watch_config` derives the watched path from the passed `Ctx` and creates
    // the config directory, so a config file that appears later is still seen.
    let dir = TempDir::new("ctx");
    let ctx = ctx_under(dir.path());
    let mut watcher = watch_config(&ctx, ctx.cancel.clone()).expect("watch the session's config");
    assert!(
        ctx.config_path
            .parent()
            .expect("the config path has a parent")
            .is_dir(),
        "the config directory is created so the watch always has one",
    );

    // The shipped quiet window is 500 ms; this proves the wiring reaches the
    // real file, not that the constant is short.
    write(&ctx.config_path, "[persist]\nhistory = true\n");
    expect_change(&mut watcher, "a write to the session's config file").await;
    drop(dir);
}
