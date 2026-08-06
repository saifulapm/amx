//! Damage coalescing, keyframes and flow control (04 §4).
//!
//! The M0 exit criterion these tests exist for: a `cat /dev/urandom` pane with
//! a client that has stopped reading must neither grow server memory without
//! bound nor corrupt any client's grid. Everything else here is the mechanism
//! that makes those two true — that damage *coalesces into a set* instead of
//! queueing deltas, that cells are read from the authoritative grid at send
//! time, and that a keyframe is what recovers when a delta will not do.
//!
//! Every test drives a real pty behind a real libghostty-vt terminal through
//! the real priority writer. The only thing the harness fakes is the socket,
//! and only in the sense that it is a pipe with a small buffer.
//!
//! Most panes here are [`Pane::controlled`]: they echo exactly what the test
//! writes and publish a frame only when the test asks. That makes both halves
//! of the mechanism observable — the test knows precisely how much damage it
//! caused, and it absorbs every published frame rather than sampling them, so a
//! delta under test is a delta and not a missed publication in disguise.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

// A test crate root's module directory is `tests/`, so the harness needs its
// path spelled out to live beside its one suite rather than next to everybody's.
#[path = "flow_control/alloc.rs"]
mod alloc;
#[path = "flow_control/drive.rs"]
mod drive;
#[path = "flow_control/harness.rs"]
mod harness;

use std::time::Duration;

use amx_proto::stream::{FlowControl, Priority, StreamId};
use amx_server::damage::{
    self, GridStream, GridStreamConfig, KeyframePolicy, KeyframeReason, SentKind,
};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use drive::{await_client, catch_up, emit, paint, sample, scroll_burst, settle, step};
use harness::{MAX_FRAME, PATIENCE, Pane, ReferenceGrid, Wire, grid_stream, screen, text_of};

/// A pipe small enough that one grid frame fills it.
const TINY_PIPE: usize = 512;

/// A pipe a keeping-up client drains comfortably.
const ROOMY_PIPE: usize = 256 * 1024;

/// What "bounded" means for one client watching one 24×80 pane: the dirty set,
/// the encoder's scratch, and whatever the writer has not put on the wire.
///
/// Generous on purpose. The point is not the constant — it is that the number
/// does not move when the pane produces a thousand times more output.
const MEMORY_BOUND: usize = 512 * 1024;

/// The stream every test binds.
const STREAM: StreamId = StreamId::new(1);

// ----------------------------------------------------------------- tests

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_client_coalesces_damage_instead_of_queueing_deltas() {
    let pane = Pane::controlled().await;
    let feed = pane.feed();
    let wire = Wire::new(TINY_PIPE);
    let mut stream = grid_stream(&pane, KeyframePolicy::default());

    // Nobody ever reads the far end: once the pipe is full the writer is stuck
    // mid-`write_all` and its grid queue stops draining for good.
    for round in 0..60 {
        pane.write(scroll_burst(round)).await;
        step(&pane, &feed, &mut [(&mut stream, &wire.out)]).await;
    }

    let stats = stream.stats();
    assert_eq!(stats.absorbed, 60, "one publication per round: {stats:?}");
    assert_eq!(stats.skipped, 0, "the test absorbed every frame: {stats:?}");
    assert!(
        stats.frames() * 5 < stats.absorbed,
        "a stalled client must not be sent a frame per damage event: {stats:?}"
    );
    assert!(
        stats.stalls > 20,
        "most flushes should have found the writer busy: {stats:?}"
    );
    assert!(
        stats.coalesced * 10 >= stats.absorbed * 9,
        "almost every published frame should have merged into a set that already \
         held damage rather than becoming a frame of its own: {stats:?}"
    );
    assert!(
        !stream.dirty().is_empty(),
        "the damage is waiting in the set, not queued at the writer"
    );
    assert!(
        wire.out.depth(Priority::Grid) <= 1,
        "at most one frame is queued ahead of the stalled write"
    );

    let _ = wire.finish().await;
    pane.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_memory_is_bounded_under_a_stalled_client() {
    // The adversarial pane from the M0 exit criterion, free-running at full
    // speed, and a client that never reads a byte of it.
    let pane = Pane::start("cat /dev/urandom");
    let mut feed = pane.feed();
    let wire = Wire::new(TINY_PIPE);
    let mut stream = grid_stream(&pane, KeyframePolicy::default());

    let mut peak = 0;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        sample(&feed, &mut [(&mut stream, &wire.out)]);
        peak = peak.max(stream.accounted_bytes(&wire.out));
        assert!(
            peak < MEMORY_BOUND,
            "accounted bytes reached {peak}, over the {MEMORY_BOUND} byte bound: {:?}",
            stream.stats()
        );
        let _ = tokio::time::timeout(Duration::from_millis(5), feed.changed()).await;
    }

    let stats = stream.stats();
    assert!(
        stats.absorbed > 100,
        "the flood should have produced many frames: {stats:?}"
    );
    assert!(
        stats.frames() * 20 < stats.absorbed,
        "a client that never reads is sent only what the writer took before it \
         blocked, however long the flood runs: {stats:?}"
    );
    // The bookkeeping alone — the part that would grow if damage were queued —
    // stays the size of a dirty bitmap plus one frame's scratch.
    assert!(
        stream.bookkeeping_bytes() < 4 * MAX_FRAME as usize,
        "bookkeeping grew with the flood: {} bytes after {} frames",
        stream.bookkeeping_bytes(),
        stats.absorbed
    );

    let _ = wire.finish().await;
    pane.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_client_grid_is_corrupted_after_coalescing() {
    let pane = Pane::controlled().await;
    let feed = pane.feed();
    let mut wire = Wire::new(TINY_PIPE);
    // Thirty milliseconds a frame into a 512 byte pipe: the client is reading,
    // and losing badly.
    let mut frames = wire.reader(Duration::from_millis(30));
    let mut stream = grid_stream(&pane, KeyframePolicy::default());
    let mut reference = ReferenceGrid::default();

    // Flood until the client is well behind.
    for round in 0..60 {
        pane.write(scroll_burst(round)).await;
        step(&pane, &feed, &mut [(&mut stream, &wire.out)]).await;
        frames.drain_into(&mut reference);
    }
    assert!(
        stream.stats().coalesced > 10,
        "the run must actually have coalesced: {:?}",
        stream.stats()
    );

    // Then a quiet stretch of in-place changes, which is where deltas live.
    // This half is the one that would catch a corrupt grid: a delta applied on
    // top of a coalesced keyframe has to land on exactly the right rows.
    for round in 0..10 {
        let tag = format!("tail {round}");
        pane.write(paint(6 + round as u16, &tag)).await;
        catch_up(
            &pane,
            &feed,
            &mut stream,
            &wire,
            &mut frames,
            &mut reference,
            &tag,
        )
        .await;
    }
    assert!(
        stream.stats().deltas > 0,
        "in-place damage should have gone out as deltas: {:?}",
        stream.stats()
    );

    // Finally, publications the stream never sees. `SnapshotFeed` is a `watch`
    // slot: a reader that falls behind gets the newest frame and never the ones
    // between, so the damage lists in between are simply gone. Row 17 is only
    // ever named by a frame that is skipped — it survives the round trip if and
    // only if a gap in the publication counter makes the stream distrust the
    // damage list it did get.
    for round in 0..5 {
        let unseen = format!("unseen {round}");
        pane.write(paint(17, &unseen)).await;
        // Published, and deliberately never absorbed: this is the frame whose
        // damage list the stream loses.
        pane.snapshot_until(&unseen).await;
        let tag = format!("gap {round}");
        pane.write(paint(18, &tag)).await;
        let snapshot = pane.snapshot().await;
        stream.absorb(&snapshot, feed.generation());
        catch_up(
            &pane,
            &feed,
            &mut stream,
            &wire,
            &mut frames,
            &mut reference,
            &tag,
        )
        .await;
    }
    assert!(
        stream.stats().skipped >= 5,
        "the stream should have noticed the frames it missed: {:?}",
        stream.stats()
    );

    let settled = settle(&pane, &feed, &mut [(&mut stream, &wire.out)]).await;
    wire.drained().await;
    let _ = wire.finish().await;
    frames.finish_into(&mut reference).await;

    assert!(
        reference.keyframes >= 1 && reference.deltas >= 1,
        "the stream should have carried both kinds: {} keyframes, {} deltas",
        reference.keyframes,
        reference.deltas
    );
    reference.assert_matches(&settled);
    assert!(
        screen(&settled).contains("unseen 4"),
        "the row only a skipped frame ever named must be on the client's grid too"
    );

    pane.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn threshold_crossing_emits_a_keyframe_not_a_delta() {
    let pane = Pane::controlled().await;
    let feed = pane.feed();
    let mut wire = Wire::new(ROOMY_PIPE);
    let frames = wire.reader(Duration::ZERO);
    // Half the grid: twelve of twenty-four rows.
    let mut stream = grid_stream(&pane, KeyframePolicy::new(50));

    // The opening keyframe, which every client is owed.
    pane.write(b"first\r".as_slice()).await;
    pane.snapshot_until("first").await;
    let (opening, _) = emit(&pane, &feed, &mut stream, &wire.out).await;
    assert_eq!(opening.kind, SentKind::Keyframe(KeyframeReason::First));
    settle(&pane, &feed, &mut [(&mut stream, &wire.out)]).await;

    // A few rows of damage is a delta.
    pane.write(b"a\rb\rc\r".as_slice()).await;
    let (sent, _) = emit(&pane, &feed, &mut stream, &wire.out).await;
    assert_eq!(
        sent.kind,
        SentKind::Delta,
        "a handful of damaged rows is a delta, not a keyframe"
    );
    assert!(
        sent.rows < 12,
        "the delta should be well under the threshold: {sent:?}"
    );
    settle(&pane, &feed, &mut [(&mut stream, &wire.out)]).await;

    // Now scroll the whole screen without offering a drain, so damage
    // accumulates inside the set until it crosses the threshold.
    pane.write(scroll_burst(0)).await;
    let crossed = Instant::now() + PATIENCE;
    while stream.owed_keyframe().is_none() {
        let snapshot = pane.snapshot().await;
        stream.absorb(&snapshot, feed.generation());
        assert!(
            Instant::now() < crossed,
            "damage never crossed the threshold: {} of {} rows",
            stream.dirty().marked(),
            stream.dirty().rows()
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(
        stream.owed_keyframe(),
        Some(KeyframeReason::Threshold),
        "the set grew past half the grid, so the next frame is a keyframe"
    );

    let (sent, snapshot) = emit(&pane, &feed, &mut stream, &wire.out).await;
    assert_eq!(sent.kind, SentKind::Keyframe(KeyframeReason::Threshold));

    let mut reference = ReferenceGrid::default();
    wire.drained().await;
    let _ = wire.finish().await;
    frames.finish_into(&mut reference).await;
    reference.assert_matches(&snapshot);

    pane.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resync_request_emits_a_keyframe() {
    let pane = Pane::controlled().await;
    let feed = pane.feed();
    let mut wire = Wire::new(ROOMY_PIPE);
    let frames = wire.reader(Duration::ZERO);
    let mut stream = grid_stream(&pane, KeyframePolicy::default());

    pane.write(b"resync me\r".as_slice()).await;
    pane.snapshot_until("resync me").await;
    settle(&pane, &feed, &mut [(&mut stream, &wire.out)]).await;
    assert_eq!(
        stream.stats().keyframes,
        1,
        "only the opening keyframe so far"
    );
    assert!(!stream.owes(), "the client is up to date");

    // A client whose grid it does not trust asks for a fresh one.
    stream.control(FlowControl::Resync { stream: STREAM });
    assert_eq!(stream.owed_keyframe(), Some(KeyframeReason::Resync));
    let (sent, _) = emit(&pane, &feed, &mut stream, &wire.out).await;
    assert_eq!(sent.kind, SentKind::Keyframe(KeyframeReason::Resync));
    assert_eq!(stream.stats().keyframes, 2);

    // A pause is honoured, and the damage waits rather than being dropped.
    stream.control(FlowControl::Pause { stream: STREAM });
    pane.write(b"while paused\r".as_slice()).await;
    let snapshot = pane.snapshot_until("while paused").await;
    stream.absorb(&snapshot, feed.generation());
    assert!(
        stream
            .flush(&snapshot, &wire.out)
            .expect("no frame")
            .is_none(),
        "a paused stream sends nothing"
    );
    assert!(stream.owes(), "and keeps the damage");
    stream.control(FlowControl::Resume { stream: STREAM });

    let mut reference = ReferenceGrid::default();
    let settled = settle(&pane, &feed, &mut [(&mut stream, &wire.out)]).await;
    wire.drained().await;
    let _ = wire.finish().await;
    frames.finish_into(&mut reference).await;
    reference.assert_matches(&settled);
    assert!(screen(&settled).contains("while paused"));

    pane.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_clients_at_different_speeds_each_stay_consistent() {
    let pane = Pane::controlled().await;
    let feed = pane.feed();

    let mut quick = Wire::new(ROOMY_PIPE);
    let mut quick_frames = quick.reader(Duration::ZERO);
    let mut quick_stream = grid_stream(&pane, KeyframePolicy::default());
    let mut quick_grid = ReferenceGrid::default();

    let mut slow = Wire::new(TINY_PIPE);
    let mut slow_frames = slow.reader(Duration::from_millis(20));
    let mut slow_stream = grid_stream(&pane, KeyframePolicy::default());
    let mut slow_grid = ReferenceGrid::default();

    for round in 0..60 {
        pane.write(scroll_burst(round)).await;
        step(
            &pane,
            &feed,
            &mut [
                (&mut quick_stream, &quick.out),
                (&mut slow_stream, &slow.out),
            ],
        )
        .await;
        quick_frames.drain_into(&mut quick_grid);
        slow_frames.drain_into(&mut slow_grid);
    }

    assert!(
        quick_stream.stats().frames() > slow_stream.stats().frames(),
        "the fast client should have been sent more: {:?} vs {:?}",
        quick_stream.stats(),
        slow_stream.stats()
    );
    assert!(
        slow_stream.stats().coalesced > 10,
        "the slow client should have coalesced: {:?}",
        slow_stream.stats()
    );

    let catch_up = Instant::now() + PATIENCE;
    let settled = loop {
        let snapshot = settle(
            &pane,
            &feed,
            &mut [
                (&mut quick_stream, &quick.out),
                (&mut slow_stream, &slow.out),
            ],
        )
        .await;
        quick_frames.drain_into(&mut quick_grid);
        slow_frames.drain_into(&mut slow_grid);
        if quick.out.is_empty() && slow.out.is_empty() {
            break snapshot;
        }
        assert!(
            Instant::now() < catch_up,
            "a client never drained: {:?} / {:?}",
            quick_stream.stats(),
            slow_stream.stats()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    };

    let _ = quick.finish().await;
    let _ = slow.finish().await;
    quick_frames.finish_into(&mut quick_grid).await;
    slow_frames.finish_into(&mut slow_grid).await;

    // Two clients, two very different histories on the wire, one grid.
    quick_grid.assert_matches(&settled);
    slow_grid.assert_matches(&settled);

    pane.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pump_drives_a_stream_and_answers_flow_control() {
    // The one test that spawns the driver rather than stepping the mechanism:
    // it is what a connection task will do, so the loop, the retry timer and
    // the flow-control mailbox all have to work together unattended.
    let pane = Pane::start("printf 'pumped through\\n'; sleep 60");
    let mut wire = Wire::new(ROOMY_PIPE);
    let mut frames = wire.reader(Duration::ZERO);
    let stream = grid_stream(&pane, KeyframePolicy::default());
    let (flow, signals) = mpsc::channel(4);
    let cancel = CancellationToken::new();
    let driver = tokio::spawn(damage::pump(
        stream,
        pane.feed(),
        wire.out.clone(),
        signals,
        cancel.clone(),
    ));

    let mut reference = ReferenceGrid::default();
    await_client(&mut frames, &mut reference, "the pane's output", |grid| {
        grid.grid
            .iter()
            .any(|row| text_of(row).contains("pumped through"))
    })
    .await;

    // And it answers the client on the way through.
    flow.send(FlowControl::Resync { stream: STREAM })
        .await
        .expect("the pump is listening");
    let before = reference.keyframes;
    await_client(&mut frames, &mut reference, "a resync keyframe", |grid| {
        grid.keyframes > before
    })
    .await;

    cancel.cancel();
    let stats = driver.await.expect("the pump ended");
    assert!(
        stats.keyframes >= 2,
        "the opening keyframe and the resync: {stats:?}"
    );

    let _ = wire.finish().await;
    frames.finish_into(&mut reference).await;
    pane.stop().await;
}

/// A paused stream that is owed damage parks on its wake sources — a flow
/// signal or a new frame — instead of arming the drain-retry timer: flushing
/// is a no-op until the client resumes, so a timer wakeup is a spin at 250 Hz
/// dressed as patience. The retry counter in the stats is the spin, counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_paused_stream_with_pending_damage_sleeps_until_a_flow_signal() {
    let pane = Pane::controlled().await;
    let mut wire = Wire::new(ROOMY_PIPE);
    let mut frames = wire.reader(Duration::ZERO);
    let stream = grid_stream(&pane, KeyframePolicy::default());
    let (flow, signals) = mpsc::channel(4);
    let cancel = CancellationToken::new();
    let driver = tokio::spawn(damage::pump(
        stream,
        pane.feed(),
        wire.out.clone(),
        signals,
        cancel.clone(),
    ));

    let mut reference = ReferenceGrid::default();
    await_client(
        &mut frames,
        &mut reference,
        "the opening keyframe",
        |grid| grid.keyframes == 1,
    )
    .await;

    // Pause first and give the signal time to be applied, then damage the
    // grid: from here the stream owes a delta it is not allowed to send.
    // `snapshot_until` publishes until the echo round trip has landed, so a
    // publication that carries the damage has definitely been absorbed.
    flow.send(FlowControl::Pause { stream: STREAM })
        .await
        .expect("the pump is listening");
    tokio::time::sleep(Duration::from_millis(150)).await;
    pane.write(paint(2, "held-back")).await;
    pane.snapshot_until("held-back").await;

    // Half a second paused: an armed retry timer would fire here 100+ times.
    tokio::time::sleep(Duration::from_millis(500)).await;
    frames.drain_into(&mut reference);
    assert_eq!(reference.deltas, 0, "nothing may be sent while paused");

    // The resume signal is the wake: the owed delta goes out on it.
    flow.send(FlowControl::Resume { stream: STREAM })
        .await
        .expect("the pump is listening");
    await_client(&mut frames, &mut reference, "the resumed delta", |grid| {
        grid.deltas > 0
    })
    .await;

    cancel.cancel();
    let stats = driver.await.expect("the pump ended");
    assert!(
        stats.retries < 10,
        "a paused stream must not spin on the retry timer: {stats:?}"
    );

    let _ = wire.finish().await;
    frames.finish_into(&mut reference).await;
    pane.stop().await;
}

/// `max_frame: 0` (or anything below the protocol floor) would make every
/// keyframe unbuildable forever; the cap is clamped at `control()` time so a
/// hostile or buggy client cannot wedge its own stream into a retry loop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stream_cap_below_the_floor_is_clamped_not_honored() {
    let pane = Pane::controlled().await;
    let wire = Wire::new(ROOMY_PIPE);
    let mut stream = grid_stream(&pane, KeyframePolicy::default());
    stream.control(FlowControl::StreamCap {
        stream: STREAM,
        max_frame: 0,
    });

    // The opening keyframe still fits: the floor is sized for the default
    // grid, so a clamped cap keeps the stream deliverable.
    let (sent, _) = emit(&pane, &pane.feed(), &mut stream, &wire.out).await;
    assert!(
        matches!(sent.kind, SentKind::Keyframe(KeyframeReason::First)),
        "the clamped cap must still carry the opening keyframe: {sent:?}"
    );

    let _ = wire.finish().await;
    pane.stop().await;
}

/// A keyframe that genuinely cannot fit the negotiated cap is a terminal
/// stream error, not an infinite retry: the pump gives the client a bounded
/// number of publications to raise the cap and then ends the stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unfittable_keyframe_ends_the_stream_instead_of_retrying_forever() {
    let pane = Pane::start("yes unfittable");
    let wire = Wire::new(ROOMY_PIPE);
    // Below any keyframe, bypassing the control-time clamp: the pump has to
    // survive a cap that was wrong from bind time too.
    let mut config = GridStreamConfig::new(pane.host.pane(), STREAM);
    config.max_frame = 64;
    let stream = GridStream::new(config).expect("stream 1 has a channel byte");
    let (_flow, signals) = mpsc::channel::<FlowControl>(4);
    let cancel = CancellationToken::new();
    let driver = tokio::spawn(damage::pump(
        stream,
        pane.feed(),
        wire.out.clone(),
        signals,
        cancel.clone(),
    ));

    let stats = tokio::time::timeout(PATIENCE, driver)
        .await
        .expect("the pump must end on its own: an unfittable keyframe is terminal, not a loop")
        .expect("the pump did not panic");
    assert_eq!(stats.keyframes, 0, "nothing can have fit: {stats:?}");

    let _ = wire.finish().await;
    pane.stop().await;
}
