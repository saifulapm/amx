//! Driving one or more grid streams by hand.
//!
//! The suite steps the mechanism itself rather than spawning
//! [`amx_server::damage::pump`], except in the one test that is about the pump:
//! a failure then names the step it happened at instead of a timing window
//! inside somebody else's loop.

#![allow(dead_code, reason = "each test uses a subset of the harness")]
#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::time::Duration;

use amx_server::actor::SnapshotFeed;
use amx_server::conn::writer::Outbound;
use amx_server::damage::{GridStream, Sent};
use amx_vt::SnapshotRef;
use tokio::time::Instant;

use super::harness::{Frames, PATIENCE, Pane, ReferenceGrid, TICK, Wire, screen};

/// One published frame, folded into every stream, with a drain offered to each.
pub async fn step(
    pane: &Pane,
    feed: &SnapshotFeed,
    streams: &mut [(&mut GridStream, &Outbound)],
) -> SnapshotRef {
    let snapshot = pane.snapshot().await;
    let generation = feed.generation();
    for (stream, out) in streams.iter_mut() {
        stream.absorb(&snapshot, generation);
        stream
            .flush(&snapshot, out)
            .expect("the stream built a frame that fits");
    }
    snapshot
}

/// Fold whatever the pane published last into every stream. For free-running
/// panes, which publish on their own timer.
pub fn sample(feed: &SnapshotFeed, streams: &mut [(&mut GridStream, &Outbound)]) {
    let snapshot = feed.latest();
    let generation = feed.generation();
    for (stream, out) in streams.iter_mut() {
        stream.absorb(&snapshot, generation);
        stream
            .flush(&snapshot, out)
            .expect("the stream built a frame that fits");
    }
}

/// Step until every stream is up to date, or fail.
pub async fn settle(
    pane: &Pane,
    feed: &SnapshotFeed,
    streams: &mut [(&mut GridStream, &Outbound)],
) -> SnapshotRef {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let snapshot = step(pane, feed, streams).await;
        if streams.iter().all(|(stream, _)| !stream.owes()) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "a stream never caught up: {:?}",
            streams
                .iter()
                .map(|(stream, _)| stream.stats())
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(TICK).await;
    }
}

/// Keep stepping until the stream actually emits something.
pub async fn emit(
    pane: &Pane,
    feed: &SnapshotFeed,
    stream: &mut GridStream,
    out: &Outbound,
) -> (Sent, SnapshotRef) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let snapshot = pane.snapshot().await;
        stream.absorb(&snapshot, feed.generation());
        if let Some(sent) = stream.flush(&snapshot, out).expect("a frame that fits") {
            return (sent, snapshot);
        }
        assert!(
            Instant::now() < deadline,
            "the stream never sent anything: {:?}",
            stream.stats()
        );
        tokio::time::sleep(TICK).await;
    }
}

/// Thirty lines: more than the grid is tall, so it scrolls every row.
pub fn scroll_burst(round: usize) -> Vec<u8> {
    (0..30)
        .map(|line| format!("burst {round} line {line}\r"))
        .collect::<String>()
        .into_bytes()
}

/// Paint one row in place, leaving the rest of the grid alone.
///
/// Absolute cursor placement rather than a newline, because a full screen
/// scrolls on every line and a scroll damages every row. Partial damage is the
/// only condition under which a delta is the interesting answer, so a test
/// about deltas has to stop the terminal scrolling.
pub fn paint(row: u16, tag: &str) -> Vec<u8> {
    format!("\x1b[{row};1H{tag}    \r").into_bytes()
}

/// Step until the stream owes nothing, the writer has drained, and `tag` is on
/// the grid.
#[allow(clippy::too_many_arguments, reason = "test")]
pub async fn catch_up(
    pane: &Pane,
    feed: &SnapshotFeed,
    stream: &mut GridStream,
    wire: &Wire,
    frames: &mut Frames,
    reference: &mut ReferenceGrid,
    tag: &str,
) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let snapshot = step(pane, feed, &mut [(stream, &wire.out)]).await;
        frames.drain_into(reference);
        if !stream.owes() && wire.out.is_empty() && screen(&snapshot).contains(tag) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{tag:?} never landed: {:?}",
            stream.stats()
        );
        tokio::time::sleep(TICK).await;
    }
}

/// Drain frames into `reference` until `done` accepts it.
pub async fn await_client(
    frames: &mut Frames,
    reference: &mut ReferenceGrid,
    what: &str,
    mut done: impl FnMut(&ReferenceGrid) -> bool,
) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        frames.drain_into(reference);
        if done(reference) {
            return;
        }
        assert!(Instant::now() < deadline, "the client never saw {what}");
        tokio::time::sleep(TICK).await;
    }
}
