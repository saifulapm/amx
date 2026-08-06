//! The pane host: one pty, one terminal, one published grid (04 §3).

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;

use amx_core::platform::{Pty, PtyCommand, WinSize};
use amx_core::{Bus, Delivery, Event, GridGeneration, PaneId, RowId, RowRange, Subscription};
use amx_server::actor::{PaneCommand, PaneHost, PaneHostConfig, PaneProbe, SnapshotFeed};
use amx_server::platform::UnixPty;
use amx_vt::{CellWide, Row, Snapshot, SnapshotRef};
use tokio::sync::oneshot;
use tokio::time::Instant;

/// The size every pane starts at.
const SIZE: WinSize = WinSize { rows: 24, cols: 80 };

/// How long a test waits for a pane to do something.
const PATIENCE: Duration = Duration::from_secs(10);

/// A short frame interval, so a test does not spend its patience waiting for
/// output to coalesce.
const FRAME: Duration = Duration::from_millis(2);

// ------------------------------------------------------------------ harness

/// A running pane, its bus, and a subscription taken before it started.
struct Harness {
    host: PaneHost,
    bus: Arc<Bus>,
    events: Subscription,
}

impl Harness {
    /// Start a pane running `script` under `/bin/sh`.
    fn start(script: &str) -> Self {
        Self::with_size(script, SIZE)
    }

    fn with_size(script: &str, size: WinSize) -> Self {
        let bus = Arc::new(Bus::default());
        let events = bus.subscribe();
        let session = UnixPty.spawn(&shell(script, size)).expect("spawn a pty");

        let mut config = PaneHostConfig::new(PaneId::new_v4(), Arc::clone(&bus), size);
        config.frame_interval = FRAME;
        let host = PaneHost::spawn(config, session).expect("start the pane host");
        Self { host, bus, events }
    }

    fn probe(&self) -> PaneProbe {
        self.host.probe().clone()
    }

    /// Publish a frame on the parser thread and take it.
    async fn snapshot(&self) -> SnapshotRef {
        let (reply, answer) = oneshot::channel();
        self.host
            .handle()
            .send(PaneCommand::TakeSnapshot(reply))
            .await
            .expect("the pane is running");
        tokio::time::timeout(PATIENCE, answer)
            .await
            .expect("a snapshot before the deadline")
            .expect("the parser answered")
    }

    /// Wait for a frame the parser published itself to contain `needle`.
    ///
    /// Distinct from [`Harness::wait_for_text`], which forces a publication:
    /// damage is "what changed since the previous published frame", so only a
    /// frame the parser produced at its own boundary carries the damage the
    /// output caused.
    async fn wait_for_frame(&self, feed: &mut SnapshotFeed, needle: &str) -> SnapshotRef {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let frame = feed.latest();
            if screen(&frame).contains(needle) {
                return frame;
            }
            assert!(
                tokio::time::timeout_at(deadline, feed.changed())
                    .await
                    .expect("a frame before the deadline"),
                "the pane stopped before rendering {needle:?}"
            );
        }
    }

    /// Poll frames until one contains `needle`.
    async fn wait_for_text(&self, needle: &str) -> SnapshotRef {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let snapshot = self.snapshot().await;
            if screen(&snapshot).contains(needle) {
                return snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "the pane never rendered {needle:?}: {:?}",
                screen(&snapshot)
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Wait for the first bus event `want` accepts.
    async fn wait_for_event(&mut self, mut want: impl FnMut(&Event) -> bool) -> Event {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let delivery = tokio::time::timeout_at(deadline, self.events.recv())
                .await
                .expect("an event before the deadline")
                .expect("the bus is open");
            match delivery {
                Delivery::Event(envelope) if want(&envelope.event) => return envelope.event,
                Delivery::Event(_) => {}
                Delivery::Gap { from, to } => panic!("the test fell behind the bus: {from}..={to}"),
            }
        }
    }

    async fn stop(self) {
        self.host.shutdown().await.expect("the pane task ended");
        drop(self.bus);
    }
}

/// Unpack served history rows: `u32` length, text, one flag byte per row
/// (`amx_server::history`).
fn unpack(bytes: &[u8]) -> Vec<String> {
    let mut rows = Vec::new();
    let mut at = 0;
    while at + 5 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes")) as usize;
        at += 4;
        rows.push(
            String::from_utf8_lossy(&bytes[at..at + len])
                .trim_end()
                .to_owned(),
        );
        at += len + 1;
    }
    rows
}

fn shell(script: &str, size: WinSize) -> PtyCommand {
    PtyCommand {
        program: OsString::from("/bin/sh"),
        args: vec![OsString::from("-c"), OsString::from(script)],
        env: vec![
            (OsString::from("TERM"), OsString::from("xterm-256color")),
            (OsString::from("LC_ALL"), OsString::from("C")),
        ],
        cwd: None,
        size,
    }
}

/// One row of a snapshot as text, trailing blanks trimmed.
fn line(row: &Row) -> String {
    let mut out = String::new();
    for cell in row.cells() {
        if cell.wide == CellWide::SpacerTail {
            continue;
        }
        match std::str::from_utf8(row.text(cell)) {
            Ok("") | Err(_) => out.push(' '),
            Ok(text) => out.push_str(text),
        }
    }
    out.trim_end().to_owned()
}

/// The whole visible grid as text.
fn screen(snapshot: &Snapshot) -> String {
    snapshot
        .grid()
        .iter()
        .map(line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// A shell loop that prints `count` numbered lines and then goes quiet.
fn numbered(count: u32) -> String {
    format!("i=1; while [ $i -le {count} ]; do echo \"line $i\"; i=$((i+1)); done; sleep 60")
}

// -------------------------------------------------------------------- tests

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pane_host_publishes_a_snapshot_after_output() {
    let mut pane = Harness::start("printf 'hello from amx\\n'; sleep 60");
    let mut feed = pane.host.frames();

    let snapshot = pane.wait_for_frame(&mut feed, "hello from amx").await;
    assert_eq!(snapshot.rows(), SIZE.rows);
    assert_eq!(snapshot.cols(), SIZE.cols);
    assert_eq!(line(&snapshot.grid()[0]), "hello from amx");
    assert!(
        snapshot.damage().contains(&0),
        "the frame carrying the output names the row it landed on: {:?}",
        snapshot.damage()
    );

    // And the transition is on the bus, not only in the snapshot.
    let event = pane
        .wait_for_event(|event| matches!(event, Event::PaneDamage { .. }))
        .await;
    assert!(matches!(
        event,
        Event::PaneDamage { generation, .. } if generation == GridGeneration::FIRST
    ));

    pane.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_range_command_is_served_from_the_parser_thread() {
    let mut pane = Harness::start(&numbered(60));

    // Wait until enough rows have scrolled off the top to ask for a range.
    pane.wait_for_event(
        |event| matches!(event, Event::HistoryCommitted { range, .. } if range.last.get() >= 9),
    )
    .await;
    // The pane is quiet now: nothing below can be riding on a pty read.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let parses = pane.probe().parses();

    let (reply, answer) = oneshot::channel();
    pane.host
        .handle()
        .send(PaneCommand::HistoryRange {
            range: RowRange::new(RowId::FIRST, RowId::from_raw(4)),
            reply,
        })
        .await
        .expect("the pane is running");
    let rows = tokio::time::timeout(PATIENCE, answer)
        .await
        .expect("history before the deadline")
        .expect("the parser answered")
        .expect("the range is committed");

    assert_eq!(
        unpack(&rows.rows),
        ["line 1", "line 2", "line 3", "line 4", "line 5"]
    );
    assert_eq!(rows.range.row_count(), Some(5));

    let probe = pane.probe();
    assert!(
        probe.history_thread().is_some(),
        "a thread recorded itself serving the range"
    );
    assert_eq!(
        probe.history_thread(),
        probe.parser_thread(),
        "the range was read on the thread that owns the terminal"
    );
    assert_ne!(
        probe.history_thread(),
        Some(PaneProbe::thread_key()),
        "and not on the caller's thread"
    );
    assert_eq!(
        probe.parses(),
        parses,
        "an idle pane serves history without waiting for pty output"
    );

    pane.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_reader_ever_blocks_the_parser_on_a_pane_state_mutex() {
    let pane = Harness::start("while :; do printf 'amx heavy output line\\n'; done");
    let probe = pane.probe();
    pane.wait_for_text("amx heavy output").await;

    // A reader takes a frame and keeps it, reading every cell of it over and
    // over. If publication and reading shared a pane-state mutex, this is
    // exactly the hold that would stall the parser.
    let held = pane.host.frames().latest();
    let before = probe.frames();
    let start = Instant::now();
    let hold = Duration::from_millis(300);
    let mut reads = 0u64;
    while start.elapsed() < hold {
        assert!(!screen(&held).is_empty());
        reads += 1;
        tokio::task::yield_now().await;
    }
    let published = probe.frames() - before;

    assert!(
        reads > 0,
        "the reader did some work while holding the frame"
    );
    assert!(
        published >= 10,
        "the parser published {published} frames while a reader held one for {hold:?}"
    );
    assert!(
        probe.slowest_publish() < hold / 2,
        "no publication ever waited on the reader: slowest was {:?}",
        probe.slowest_publish()
    );
    assert!(
        probe.parser_thread().is_some(),
        "the parser recorded its own thread"
    );
    assert_ne!(
        probe.parser_thread(),
        Some(PaneProbe::thread_key()),
        "and it is not this one"
    );

    pane.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resize_bumps_the_grid_generation_exactly_once() {
    let mut pane = Harness::start("printf 'before\\n'; sleep 60");
    pane.wait_for_text("before").await;
    assert_eq!(pane.host.frames().generation(), GridGeneration::FIRST);

    pane.host
        .handle()
        .send(PaneCommand::Resize {
            rows: 30,
            cols: 100,
        })
        .await
        .expect("the pane is running");

    let resized = pane
        .wait_for_event(|event| matches!(event, Event::PaneResized { .. }))
        .await;
    let Event::PaneResized {
        rows,
        cols,
        generation,
        ..
    } = resized
    else {
        panic!("expected a resize event");
    };
    assert_eq!((rows, cols), (30, 100));
    assert_eq!(
        generation,
        GridGeneration::FIRST.next(),
        "one resize is one bump"
    );
    assert_eq!(pane.host.frames().generation(), generation);

    // The grid follows, and nothing bumps it again on the way.
    let snapshot = pane.snapshot().await;
    assert_eq!((snapshot.rows(), snapshot.cols()), (30, 100));
    let damage = pane
        .wait_for_event(|event| matches!(event, Event::PaneDamage { .. }))
        .await;
    assert!(matches!(
        damage,
        Event::PaneDamage { generation: seen, .. } if seen == generation
    ));
    assert_eq!(pane.host.frames().generation(), generation);

    pane.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pane_exit_emits_pane_exited_with_status() {
    let mut pane = Harness::start("exit 7");

    let exited = pane
        .wait_for_event(|event| matches!(event, Event::PaneExited { .. }))
        .await;
    let Event::PaneExited { pane: id, status } = exited else {
        panic!("expected an exit event");
    };
    assert_eq!(id, pane.host.pane());
    assert_eq!(status, Some(7), "the child's status survives its terminal");

    // The pane stops itself once its child is gone.
    pane.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn snapshot_read_during_heavy_output_is_internally_consistent() {
    let pane = Harness::start("while :; do printf 'consistency check line\\n'; done");
    pane.wait_for_text("consistency check").await;

    let held = pane.host.frames().latest();
    let captured = (*held).clone();

    for (index, row) in held.grid().iter().enumerate() {
        assert!(
            row.cells().len() <= usize::from(held.cols()),
            "row {index} has more cells than the grid has columns"
        );
        for cell in row.cells() {
            let text = row.text(cell);
            assert!(
                std::str::from_utf8(text).is_ok(),
                "row {index} holds a cell whose text is not valid UTF-8"
            );
        }
    }
    for row in held.damage() {
        assert!(
            *row < held.rows(),
            "damage names row {row} of a {}-row grid",
            held.rows()
        );
    }
    let cursor = held.cursor();
    if cursor.visible_in_viewport {
        assert!(cursor.x < held.cols() && cursor.y < held.rows());
    }

    // Heavy output keeps flowing; a published snapshot is plain data and does
    // not change under the reader that holds it.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        captured, *held,
        "a held snapshot was mutated while the parser kept publishing"
    );
    assert!(
        pane.probe().frames() > 1,
        "the parser kept publishing during the hold"
    );

    pane.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreground_cwd_reads_the_directory_the_child_is_in() {
    let home = std::fs::canonicalize(std::env::temp_dir()).expect("a real temp directory");
    let bus = Arc::new(Bus::default());
    let mut command = shell("sleep 60", SIZE);
    command.cwd = Some(home.clone());
    let session = UnixPty.spawn(&command).expect("spawn a pty");
    let host = PaneHost::spawn(
        PaneHostConfig::new(PaneId::new_v4(), Arc::clone(&bus), SIZE),
        session,
    )
    .expect("start the pane host");

    let (reply, answer) = oneshot::channel();
    host.handle()
        .send(PaneCommand::ForegroundCwd(reply))
        .await
        .expect("the pane is running");
    let cwd = tokio::time::timeout(PATIENCE, answer)
        .await
        .expect("an answer before the deadline")
        .expect("the pane answered");

    assert_eq!(
        cwd,
        Some(home),
        "a split inherits the foreground process's directory (04 §7)"
    );

    host.shutdown().await.expect("the pane task ended");
}

/// The published frame and the grid generation travel in one slot: a reader
/// must never observe the post-resize generation paired with a pre-resize
/// grid. Two separate reads (`latest()` + `generation()`) cannot promise
/// that — [`SnapshotFeed::frame`] does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn published_frame_and_generation_change_together() {
    let pane = Harness::start("printf 'steady\\n'; sleep 60");
    pane.wait_for_text("steady").await;
    let feed = pane.host.frames();
    let (snapshot, generation) = feed.frame();
    assert_eq!(generation, GridGeneration::FIRST);
    assert_eq!((snapshot.rows(), snapshot.cols()), (SIZE.rows, SIZE.cols));

    pane.host
        .handle()
        .send(PaneCommand::Resize {
            rows: 30,
            cols: 100,
        })
        .await
        .expect("the pane is running");

    // From here every observed pair must be internally consistent: the old
    // generation only with the old geometry, the new generation only with the
    // new one. The bump-before-publish window is exactly where a torn read
    // would show the new generation on the old grid.
    let deadline = Instant::now() + PATIENCE;
    loop {
        let (snapshot, generation) = feed.frame();
        if generation == GridGeneration::FIRST.next() {
            assert_eq!(
                (snapshot.rows(), snapshot.cols()),
                (30, 100),
                "the bumped generation must never be paired with the pre-resize grid"
            );
            break;
        }
        assert_eq!(
            (snapshot.rows(), snapshot.cols()),
            (SIZE.rows, SIZE.cols),
            "the old generation must never be paired with the post-resize grid"
        );
        assert!(
            Instant::now() < deadline,
            "the resized frame never published"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    pane.stop().await;
}

/// The dedicated hang-up path works with a mailbox nothing can be queued
/// into: `PaneCommand::Kill` can be refused exactly then, and a kill that
/// can be refused is a child that can be orphaned.
#[tokio::test]
async fn kill_bypasses_a_full_command_mailbox() {
    let mut pane = Harness::start("sleep 60");
    // Let the shell's startup output drain so the actor is parked.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Single-threaded scheduler, no awaits: the actor cannot drain between
    // these sends, so the mailbox is genuinely full afterwards.
    while pane
        .host
        .handle()
        .try_send(PaneCommand::Write(bytes::Bytes::from_static(b"x")))
        .is_ok()
    {}
    assert!(
        pane.host.handle().try_send(PaneCommand::Kill).is_err(),
        "the mailbox kill must be refused here, or this test is not testing anything"
    );

    pane.host.kill();
    let exited = pane
        .wait_for_event(|event| matches!(event, Event::PaneExited { .. }))
        .await;
    assert!(matches!(exited, Event::PaneExited { status: None, .. }));

    pane.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_hangs_the_pane_up_and_reports_the_exit() {
    let mut pane = Harness::start("sleep 60");

    pane.host
        .handle()
        .send(PaneCommand::Kill)
        .await
        .expect("the pane is running");

    let exited = pane
        .wait_for_event(|event| matches!(event, Event::PaneExited { .. }))
        .await;
    assert!(
        matches!(exited, Event::PaneExited { status: None, .. }),
        "a hangup is not a normal exit status: {exited:?}"
    );

    pane.stop().await;
}
