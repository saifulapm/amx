//! U05: the `Persist` actor — what it saves, when, and how it stops.
//!
//! The claims defended here are `docs/07-m1-plan.md` §2's, and each one is a
//! failure mode somebody else already shipped:
//!
//! - **Dirtiness is observed, never pushed.** Structure schedules a save, damage
//!   does not, and a bus gap counts as dirt — so no call site anywhere has to
//!   remember persistence exists.
//! - **The debounce has a ceiling.** herdr's trailing-only window never fires
//!   while a session keeps changing; here a continuously busy session still
//!   saves, at the cap, and the timing is asserted in tokio's *virtual* time —
//!   the rig forbids sleeping on the wall clock, and a 5-second real nap in a
//!   unit suite would be its own kind of wrong.
//! - **Capture goes through `Core`'s mailbox.** The stand-in `Core` here is a
//!   plain `mpsc::Receiver`, so a back door would not merely fail an assertion,
//!   it would have nowhere to arrive.
//! - **fsync happens on the blocking pool.** Asserted the only way that claim
//!   can honestly be asserted: the suite runs on a current-thread runtime and
//!   stalls a sync, then checks that async work still ran. An inline fsync
//!   would have parked the whole runtime.
//! - **Shutdown is receive-only.** After cancellation the actor drains its own
//!   mailbox, writes once, and returns without asking any sibling for anything
//!   — the rule that keeps it out of the shutdown-wedge flake's way (R-M1-2).

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::time::Duration;

use amx_core::{Event, InvalidationCause, RowId, RowRange};
use amx_server::actor::PersistCommand;
use amx_server::actor::persist::{QUIET_WINDOW, STALENESS_CAP};
use amx_server::persist::{SNAPSHOT_NAME, sidecar};
use tokio::time::Instant;

// A test crate root's module directory is `tests/`, so the fixtures need their
// path spelled out to live beside their one suite rather than next to
// everybody's.
#[path = "persist_actor/fixtures.rs"]
mod fixtures;
#[path = "persist_actor/seam.rs"]
mod seam;
mod support;

use fixtures::{Answers, FakePane, Rig, flood, pane_id, snapshot_of, structural};
use seam::{Recorder, Sync};
use support::TempDir;

/// How long a shutdown may take before the suite calls it wedged.
const SHUTDOWN_LIMIT: Duration = Duration::from_secs(10);

/// How long a test waits for a save it expects.
///
/// Spent in tokio's *virtual* time, so an actor that schedules nothing costs
/// the suite no real seconds — and, more to the point, fails the assertion
/// instead of hanging the test binary.
const WAIT_LIMIT: Duration = Duration::from_secs(60);

/// Wait until `n` captures have been asked for.
async fn captures(rig: &Rig, n: usize) {
    tokio::time::timeout(WAIT_LIMIT, rig.spy.captures(n))
        .await
        .expect("the actor must schedule the save this test is waiting for");
}

/// Let every other task run to a standstill at the current virtual instant.
async fn settle() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

// --------------------------------------------------------------- the debounce

#[tokio::test(start_paused = true)]
async fn a_structural_event_schedules_a_save_after_the_quiet_window() {
    let root = TempDir::new("quiet");
    let rig = Rig::under(&root).start();

    // Three changes inside one burst, 200 ms apart: the quiet window is
    // re-armed by each one, so the burst costs exactly one save, timed from
    // the last change rather than the first.
    let began = Instant::now();
    rig.publish(structural());
    tokio::time::advance(Duration::from_millis(200)).await;
    rig.publish(structural());
    tokio::time::advance(Duration::from_millis(200)).await;
    rig.publish(structural());

    captures(&rig, 1).await;
    assert_eq!(
        began.elapsed(),
        Duration::from_millis(400) + QUIET_WINDOW,
        "the save fires one quiet window after the *last* change, not the first",
    );

    let spy = rig.spy.clone();
    let report = rig.stop().await;
    assert_eq!(report.saves, 1, "a burst coalesces into one save");
    assert_eq!(spy.seen().captures.len(), 1, "and into one capture");
}

#[tokio::test(start_paused = true)]
async fn continuous_change_saves_at_the_staleness_cap_not_never() {
    let root = TempDir::new("cap");
    let rig = Rig::under(&root).start();

    let began = Instant::now();
    rig.publish(structural());
    // Observed before the clock moves, so the cap is measured from *this*
    // instant — which is the whole claim: five seconds after the first unsaved
    // change, not five seconds after whenever the actor got around to it.
    settle().await;

    // Change every 100 ms forever — far inside the quiet window, so a
    // trailing-only debounce (herdr's `SESSION_SAVE_DEBOUNCE`) re-arms itself
    // on every one and never fires at all. The clock is stepped rather than
    // slept on: this is five seconds of virtual time and no real ones.
    loop {
        tokio::time::advance(Duration::from_millis(100)).await;
        settle().await;
        if !rig.spy.seen().captures.is_empty() {
            break;
        }
        assert!(
            began.elapsed() < STALENESS_CAP * 4,
            "a session that keeps changing must still save; four caps in, it had not",
        );
        rig.publish(structural());
    }

    assert_eq!(
        began.elapsed(),
        STALENESS_CAP,
        "a session that never goes quiet still saves, at the cap",
    );

    let report = rig.stop().await;
    assert_eq!(report.saves, 1);
}

#[tokio::test(start_paused = true)]
async fn pane_damage_events_never_schedule_a_save() {
    let root = TempDir::new("noise");
    let rig = Rig::under(&root).start();

    // Everything a busy pane produces, none of which is the session's shape.
    // The committed rows are in the list on purpose: `[persist] history` is
    // off on this rig, and a pane's scrollback moving is only worth a save to
    // somebody who asked for it on disk.
    for n in 0..8 {
        rig.publish(Event::PaneDamage {
            pane: pane_id(n),
            generation: amx_core::GridGeneration::FIRST,
        });
        rig.publish(Event::PaneTitle {
            pane: pane_id(n),
            title: "vim".to_owned(),
        });
        rig.publish(Event::HistoryCommitted {
            pane: pane_id(n),
            range: RowRange::new(RowId::FIRST, RowId::from_raw(9)),
        });
    }
    settle().await;
    tokio::time::advance(STALENESS_CAP * 4).await;
    settle().await;

    assert!(
        rig.spy.seen().captures.is_empty(),
        "damage is not structure: four staleness caps of it must schedule nothing",
    );

    // And the actor is not merely asleep — one structural event still lands.
    let began = Instant::now();
    rig.publish(structural());
    captures(&rig, 1).await;
    assert_eq!(began.elapsed(), QUIET_WINDOW);

    let report = rig.stop().await;
    assert_eq!(report.saves, 1);
}

#[tokio::test(start_paused = true)]
async fn a_gap_on_the_bus_is_treated_as_dirty() {
    let root = TempDir::new("gap");
    // A replay buffer of four, flooded with ten events before the actor is
    // spawned: its subscription's first delivery is a `Gap`. Every flooded
    // event is damage, so nothing *inside* the gap was structural — if the
    // actor re-read the gap's contents rather than treating the gap itself as
    // dirt, it would save nothing.
    let rig = Rig::under(&root)
        .replay(4)
        .prelude(|bus| flood(bus, 10))
        .start();

    let began = Instant::now();
    captures(&rig, 1).await;
    assert_eq!(
        began.elapsed(),
        QUIET_WINDOW,
        "a gap is dirt, debounced like any other change",
    );

    let report = rig.stop().await;
    assert_eq!(report.saves, 1);
}

// ----------------------------------------------------------------- the capture

#[tokio::test]
async fn capture_request_goes_through_the_core_mailbox_not_a_back_door() {
    let root = TempDir::new("mbox");
    let rig = Rig::under(&root).start();

    rig.flush().await.expect("the save");

    let seen = rig.spy.seen();
    assert_eq!(
        seen.captures,
        vec![false],
        "one capture arrived on the ordinary Core mailbox, asking for no sidecars",
    );
    assert_eq!(seen.others, 0, "and nothing else was sent to Core");

    // The flag follows the live config rather than a value read once at start.
    rig.config
        .send_modify(|config| config.persist.history = true);
    rig.flush().await.expect("the save");
    assert_eq!(rig.spy.seen().captures, vec![false, true]);

    rig.stop().await;
}

// ------------------------------------------------------------------ the write

#[tokio::test]
async fn a_save_syncs_the_file_and_the_directory_through_the_durable_writer() {
    let root = TempDir::new("sync");
    let recorder = Recorder::watching();
    let rig = Rig::under(&root)
        .syncs(std::sync::Arc::clone(&recorder))
        .start();

    assert!(
        !rig.state_dir().exists(),
        "the state directory is made on first write"
    );
    rig.flush().await.expect("the save");

    // The whole D-M1-1 sequence, as the actor drives it: the parent of the
    // state directory is synced as that directory is created (a directory whose
    // parent was never synced can take a perfectly synced snapshot with it),
    // then the staging file before the rename and the directory after it.
    assert_eq!(
        recorder.recorded().steps,
        vec![Sync::Dir, Sync::File, Sync::Dir],
        "the actor writes through U02's durable sequence, not around it",
    );
    assert!(rig.state_dir().join(SNAPSHOT_NAME).is_file());
    assert_eq!(rig.on_disk(), Some(snapshot_of(1)));

    rig.stop().await;
}

#[tokio::test]
async fn an_unchanged_snapshot_is_not_written_again() {
    let root = TempDir::new("same");
    let recorder = Recorder::watching();
    let rig = Rig::under(&root)
        .answers(Answers::Same)
        .syncs(std::sync::Arc::clone(&recorder))
        .start();

    rig.flush().await.expect("the first save");
    rig.flush().await.expect("the second save");

    assert_eq!(
        recorder.recorded().file_syncs(),
        1,
        "a session that did not change costs no write, and no fsync",
    );
    let report = rig.stop().await;
    assert_eq!((report.saves, report.unchanged), (1, 1));
}

#[tokio::test]
async fn writes_happen_on_the_blocking_pool_and_serialize_behind_one_another() {
    // Part one: an fsync must not park the runtime. This test runs on a
    // current-thread runtime, so if the write ran inline the task below could
    // never be polled and the stalled sync would time out with `false`.
    {
        let root = TempDir::new("pool");
        let (recorder, opener) = Recorder::stalling();
        let rig = Rig::under(&root)
            .syncs(std::sync::Arc::clone(&recorder))
            .start();

        tokio::spawn(async move {
            opener.stalled().await;
            opener.open();
        });
        rig.flush().await.expect("the save");

        assert_eq!(
            recorder.recorded().progress_while_stalled,
            vec![true],
            "the runtime kept running while the fsync stood still",
        );
        rig.stop().await;
    }

    // Part two: two saves in flight at once still happen one at a time. The
    // sync is slowed down so genuine overlap would be observable rather than
    // merely possible.
    {
        let root = TempDir::new("seq");
        let recorder = Recorder::slow(Duration::from_millis(30));
        let rig = Rig::under(&root)
            .syncs(std::sync::Arc::clone(&recorder))
            .start();

        let first = tokio::spawn({
            let handle = rig.persist.clone();
            async move { handle.flush().await }
        });
        let second = tokio::spawn({
            let handle = rig.persist.clone();
            async move { handle.flush().await }
        });
        first
            .await
            .unwrap()
            .expect("the first flush")
            .expect("the first save");
        second
            .await
            .unwrap()
            .expect("the second flush")
            .expect("the second save");

        let recorded = recorder.recorded();
        assert_eq!(
            recorded.file_syncs(),
            2,
            "two different sessions, two writes"
        );
        assert_eq!(
            recorded.max_in_flight, 1,
            "the actor is sequential: two writes can never overlap",
        );
        rig.stop().await;
    }
}

// --------------------------------------------------------------- the sidecars

#[tokio::test]
async fn sidecar_dump_skips_panes_whose_history_did_not_move() {
    let root = TempDir::new("side");
    let quiet = FakePane::serving(&[("never committed", false)]);
    let busy = FakePane::serving(&[("first line", false), ("second line", false)]);
    let panes = vec![
        (pane_id(0), busy.handle.clone()),
        (pane_id(1), quiet.handle.clone()),
    ];
    let rig = Rig::under(&root)
        .history(true)
        .answers(Answers::WithPanes(panes))
        .start();

    // Only one of the two panes ever committed a row.
    rig.publish(Event::HistoryCommitted {
        pane: pane_id(0),
        range: RowRange::new(RowId::FIRST, RowId::from_raw(1)),
    });
    settle().await;
    rig.flush().await.expect("the save");

    assert_eq!(busy.asked(), 1, "the pane whose history moved is dumped");
    assert_eq!(
        quiet.asked(),
        0,
        "a pane that never committed a row is never asked for one",
    );
    let dumped = sidecar::load(rig.state_dir(), pane_id(0))
        .expect("the sidecar reads back")
        .expect("the sidecar exists");
    assert_eq!(dumped.rows.len(), 2);
    assert!(!dumped.truncated);
    assert!(
        sidecar::load(rig.state_dir(), pane_id(1))
            .expect("no sidecar is not a failure")
            .is_none(),
    );

    // A second save with nothing committed in between asks nobody anything.
    rig.flush().await.expect("the second save");
    assert_eq!(busy.asked(), 1, "an unmoved history head is not re-dumped");

    // Move it, and the dump happens again.
    rig.publish(Event::HistoryCommitted {
        pane: pane_id(0),
        range: RowRange::new(RowId::from_raw(2), RowId::from_raw(3)),
    });
    settle().await;
    rig.flush().await.expect("the third save");
    assert_eq!(busy.asked(), 2);

    // So does a reflow, which changes what the already-dumped rows mean.
    rig.publish(Event::HistoryInvalidated {
        pane: pane_id(0),
        from_row: RowId::from_raw(2),
        cause: InvalidationCause::WidthReflow,
    });
    settle().await;
    rig.flush().await.expect("the fourth save");
    assert_eq!(busy.asked(), 3);

    let report = rig.stop().await;
    assert_eq!(report.dumps, 3);
}

#[tokio::test]
async fn sidecars_are_not_dumped_while_history_is_off() {
    let root = TempDir::new("off");
    let pane = FakePane::serving(&[("a line", false)]);
    let rig = Rig::under(&root)
        .answers(Answers::WithPanes(vec![(pane_id(0), pane.handle.clone())]))
        .start();

    rig.publish(Event::HistoryCommitted {
        pane: pane_id(0),
        range: RowRange::single(RowId::FIRST),
    });
    settle().await;
    rig.flush().await.expect("the save");

    assert_eq!(rig.spy.seen().captures, vec![false]);
    assert_eq!(
        pane.asked(),
        0,
        "no capture asked for sidecars, so none are dumped"
    );
    assert!(!sidecar::dir(rig.state_dir()).exists());

    let report = rig.stop().await;
    assert_eq!(report.dumps, 0);
    assert_eq!(
        report.saves, 1,
        "the flush saved; the committed row scheduled nothing of its own",
    );
}

/// The gap M1 shipped with, in one sentence: a session settles into a shape
/// and then spends the rest of the day producing nothing but output, so
/// scrollback that could only ride *structural* dirt to disk never rode
/// anything at all. Every session is in this state almost all of the time,
/// which is why the same claim is also made over the real binary
/// (`tests/persistence/scrollback.rs`).
#[tokio::test(start_paused = true)]
async fn committed_scrollback_alone_schedules_a_save() {
    let root = TempDir::new("late");
    let pane = FakePane::serving(&[("scrolled off", false)]);
    let rig = Rig::under(&root)
        .history(true)
        .answers(Answers::WithPanes(vec![(pane_id(0), pane.handle.clone())]))
        .start();

    // No structural event, ever. The session's shape is whatever it was.
    let began = Instant::now();
    rig.publish(Event::HistoryCommitted {
        pane: pane_id(0),
        range: RowRange::single(RowId::FIRST),
    });
    captures(&rig, 1).await;
    assert_eq!(
        began.elapsed(),
        QUIET_WINDOW,
        "scrollback arms the one debounce, not a schedule of its own",
    );
    assert_eq!(
        rig.spy.seen().captures,
        vec![true],
        "the save it armed asks for the pane handles it needs",
    );

    let state_dir = rig.state_dir().to_path_buf();
    let report = rig.stop().await;
    assert_eq!(report.dumps, 1);
    assert!(
        sidecar::load(&state_dir, pane_id(0))
            .expect("the sidecar reads back")
            .is_some(),
        "the pane's scrollback is on disk with nothing structural having happened",
    );
}

// --------------------------------------------------------------- the shutdown

#[tokio::test]
async fn cancellation_drains_the_mailbox_flushes_once_and_returns() {
    let root = TempDir::new("drain");
    let rig = Rig::under(&root).start();

    // Exactly `Core`'s shutdown break: cancel, then a non-blocking push of a
    // final capture, then the mailbox is dropped. Two of them here, because
    // "writes once" has to mean once for the whole drain, not once per message.
    rig.ctx.cancel.cancel();
    rig.persist
        .try_send(PersistCommand::Snapshot(Box::new(snapshot_of(2))))
        .expect("the push has somewhere to land");
    rig.persist
        .try_send(PersistCommand::Snapshot(Box::new(snapshot_of(3))))
        .expect("the push has somewhere to land");

    let on_disk = rig.state_dir().join(SNAPSHOT_NAME);
    assert!(!on_disk.exists(), "nothing has been written yet");

    let state_dir = rig.state_dir().to_path_buf();
    let report = tokio::time::timeout(SHUTDOWN_LIMIT, rig.stop())
        .await
        .expect("the actor returns promptly after its mailbox closes");

    assert_eq!(report.saves, 1, "the whole drain costs exactly one write");
    assert_eq!(
        amx_server::persist::snapshot::load(&state_dir).expect("it parses"),
        Some(snapshot_of(3)),
        "the last capture pushed is the one that survives",
    );
}

#[tokio::test]
async fn no_request_is_sent_to_another_actor_after_cancellation() {
    let root = TempDir::new("quiet2");
    let pane = FakePane::serving(&[("a line", false)]);
    let rig = Rig::under(&root)
        .history(true)
        .answers(Answers::WithPanes(vec![(pane_id(0), pane.handle.clone())]))
        .start();

    // A live save first, so the actor is warm and both counters have a
    // baseline that a post-cancellation request would move.
    rig.publish(Event::HistoryCommitted {
        pane: pane_id(0),
        range: RowRange::single(RowId::FIRST),
    });
    settle().await;
    rig.flush().await.expect("the save");
    assert_eq!(rig.spy.seen().captures.len(), 1);
    assert_eq!(pane.asked(), 1);

    // Now cancel and dirty the actor over the bus — structure and history
    // both — while deliberately pushing *nothing*. This is the tempting case:
    // the actor knows the session changed and the only way to learn what
    // changed is to ask `Core`, which is draining too. It must not ask.
    rig.ctx.cancel.cancel();
    rig.publish(structural());
    rig.publish(Event::HistoryCommitted {
        pane: pane_id(0),
        range: RowRange::new(RowId::from_raw(1), RowId::from_raw(4)),
    });

    let spy = rig.spy.clone();
    let report = tokio::time::timeout(SHUTDOWN_LIMIT, rig.stop())
        .await
        .expect("the actor returns promptly after its mailbox closes");

    assert_eq!(
        spy.seen().captures.len(),
        1,
        "a cancelled Persist must never ask Core for a capture — Core is draining too",
    );
    assert_eq!(
        pane.asked(),
        1,
        "and never ask a pane for history: the panes are on their way down",
    );
    assert_eq!(
        report.saves, 1,
        "with nothing pushed there is nothing to write, and nowhere to get it",
    );
}
