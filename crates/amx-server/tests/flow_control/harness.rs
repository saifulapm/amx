//! Scaffolding for the flow-control suite: a real pane, a real writer over a
//! socket-like pipe nobody has to read, and a reference grid that replays what
//! the server emitted.
//!
//! Everything here is deliberately *real*. A synthetic snapshot would let the
//! tests assert whatever they liked about coalescing; a pty running `yes` or
//! `cat /dev/urandom` behind libghostty-vt is the thing the acceptance criteria
//! are actually about.

#![allow(dead_code, reason = "each test uses a subset of the harness")]
#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;

use amx_core::platform::{Pty, PtyCommand, WinSize};
use amx_core::{Bus, PaneId};
use amx_proto::FrameHeader;
use amx_proto::frame::FRAME_HEADER_LEN;
use amx_proto::stream::PackedCell;
use amx_proto::stream::grid::{Decoded, decode};
use amx_proto::stream::{Cursor, DamageRect, StreamId};
use amx_server::actor::{PaneHost, PaneHostConfig, SnapshotFeed};
use amx_server::conn::writer::{self, Outbound, WriteError, WriterReport};
use amx_server::damage::codec::expected_cell;
use amx_server::damage::{GridStream, GridStreamConfig, KeyframePolicy};
use amx_server::platform::UnixPty;
use amx_vt::{Snapshot, SnapshotRef};
use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt, DuplexStream};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// The grid every pane in this suite runs at.
pub const SIZE: WinSize = WinSize { rows: 24, cols: 80 };

/// How long a test waits for a pane to do something.
pub const PATIENCE: Duration = Duration::from_secs(10);

/// How long a poll loop waits between looks at its condition.
pub const TICK: Duration = Duration::from_millis(5);

/// A short frame interval, so a test does not spend its patience waiting.
pub const FRAME: Duration = Duration::from_millis(2);

/// The frame cap the suite negotiates: small enough that "bounded" means
/// something, large enough for a 24×80 keyframe.
pub const MAX_FRAME: u32 = 64 * 1024;

/// A running pane with a pty behind it.
pub struct Pane {
    pub host: PaneHost,
    _bus: Arc<Bus>,
}

impl Pane {
    /// Start a pane running `script` under `/bin/sh`, publishing on its own.
    pub fn start(script: &str) -> Self {
        Self::spawn(script, FRAME)
    }

    /// A pane that publishes a frame only when the test asks for one.
    ///
    /// Two properties, and both matter. `stty -echo; cat` puts exactly what the
    /// test wrote on the grid and nothing else, so "this burst scrolls the
    /// screen" is a fact. And a frame interval longer than any test run means
    /// the parser never publishes on a timer, so [`Pane::snapshot`] is the only
    /// source of publications — which is what lets a test absorb *every* frame
    /// and know it did. A stream that misses a publication is correct (it marks
    /// the whole grid) but it is not the case a test about deltas wants to be
    /// measuring by accident.
    pub async fn controlled() -> Self {
        let pane = Self::spawn("stty -echo; cat", Duration::from_secs(3600));
        // `stty` and `cat` are two processes deep: a line written before the
        // shell reaches `stty` is echoed by the line discipline. Wait for the
        // round trip once, then clear the screen, and everything after is the
        // test's alone.
        pane.write(b"ready\r".as_slice()).await;
        pane.snapshot_until("ready").await;
        pane.write(b"\x1b[2J\x1b[H\r".as_slice()).await;
        let deadline = Instant::now() + PATIENCE;
        loop {
            let snapshot = pane.snapshot().await;
            if screen(&snapshot).trim().is_empty() {
                return pane;
            }
            assert!(Instant::now() < deadline, "the pane never came up clean");
            tokio::time::sleep(TICK).await;
        }
    }

    fn spawn(script: &str, frame_interval: Duration) -> Self {
        let bus = Arc::new(Bus::default());
        let session = UnixPty.spawn(&shell(script)).expect("spawn a pty");
        let mut config = PaneHostConfig::new(PaneId::new_v4(), Arc::clone(&bus), SIZE);
        config.frame_interval = frame_interval;
        let host = PaneHost::spawn(config, session).expect("start the pane host");
        Self { host, _bus: bus }
    }

    pub fn feed(&self) -> SnapshotFeed {
        self.host.frames()
    }

    /// Publish one frame on the parser thread and take it.
    pub async fn snapshot(&self) -> SnapshotRef {
        let (reply, answer) = tokio::sync::oneshot::channel();
        self.host
            .handle()
            .send(amx_server::actor::PaneCommand::TakeSnapshot(reply))
            .await
            .expect("the pane is running");
        tokio::time::timeout(PATIENCE, answer)
            .await
            .expect("a snapshot before the deadline")
            .expect("the parser answered")
    }

    /// Publish frames until one shows `needle`.
    pub async fn snapshot_until(&self, needle: &str) -> SnapshotRef {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let snapshot = self.snapshot().await;
            if screen(&snapshot).contains(needle) {
                return snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "the pane never rendered {needle:?}"
            );
            tokio::time::sleep(TICK).await;
        }
    }

    /// Write bytes into the pane's pty.
    pub async fn write(&self, bytes: impl Into<Bytes>) {
        self.host
            .handle()
            .send(amx_server::actor::PaneCommand::Write(bytes.into()))
            .await
            .expect("the pane is running");
    }

    /// Wait for a frame the parser published to contain `needle`.
    pub async fn wait_for(&self, feed: &mut SnapshotFeed, needle: &str) -> SnapshotRef {
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

    /// Wait until the pane stops publishing frames.
    ///
    /// Quiescence is observed, not assumed: a script that "should have
    /// finished" can still have bytes in the pty, and comparing a client's grid
    /// against a moving server grid would be a race dressed up as a test.
    pub async fn quiesce(feed: &mut SnapshotFeed) {
        let deadline = Instant::now() + PATIENCE;
        while tokio::time::timeout(Duration::from_millis(200), feed.changed())
            .await
            .unwrap_or(false)
        {
            assert!(Instant::now() < deadline, "the pane never went quiet");
        }
    }

    pub async fn stop(self) {
        let _ = self.host.shutdown().await;
    }
}

fn shell(script: &str) -> PtyCommand {
    PtyCommand {
        program: OsString::from("/bin/sh"),
        args: vec![OsString::from("-c"), OsString::from(script)],
        env: vec![
            (OsString::from("TERM"), OsString::from("xterm-256color")),
            (OsString::from("LC_ALL"), OsString::from("C")),
        ],
        cwd: None,
        size: SIZE,
    }
}

/// The whole visible grid as text, for `contains` assertions.
pub fn screen(snapshot: &Snapshot) -> String {
    snapshot
        .grid()
        .iter()
        .map(|row| {
            let mut line = String::new();
            for cell in row.cells() {
                match std::str::from_utf8(row.text(cell)) {
                    Ok("") | Err(_) => line.push(' '),
                    Ok(text) => line.push_str(text),
                }
            }
            line.trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A connection writer over a pipe with a fixed buffer, plus the read end.
///
/// A client that stops reading is exactly a full pipe: the writer's
/// `write_all` stops making progress, its queue stops draining, and every grid
/// stream on the connection sees a writer that is not ready. No mock needed —
/// a test that wants a stalled client simply never calls [`Wire::reader`].
pub struct Wire {
    pub out: Outbound,
    client: Option<DuplexStream>,
    cancel: CancellationToken,
    task: JoinHandle<Result<WriterReport, WriteError>>,
}

impl Wire {
    /// A wire whose pipe holds `capacity` bytes before the writer blocks.
    pub fn new(capacity: usize) -> Self {
        let (server, client) = tokio::io::duplex(capacity);
        let (out, queue) = writer::channel();
        let cancel = CancellationToken::new();
        let task = tokio::spawn(writer::run(server, queue, cancel.clone()));
        Self {
            out,
            client: Some(client),
            cancel,
            task,
        }
    }

    /// Start reading the far end as fast as the pipe delivers.
    ///
    /// A client keeping up. For one that falls behind, see [`Wire::paced`].
    pub fn reader(&mut self) -> Frames {
        let (frames, pace) = self.paced();
        pace.release();
        frames
    }

    /// Start reading the far end one frame per issued permit.
    ///
    /// A client falls behind because the test says so, not because a sleep
    /// happened to be longer than a round. DR-19: `delay` per frame made "slow"
    /// a comparison between two wall clocks — the reader's nap and the round's
    /// own duration — and under `cargo test --workspace` on a loaded core the
    /// round is the slower of the two, so the slow client keeps up, coalesces
    /// nothing, and the suite reports a mechanism failure that is really a
    /// machine speed. Reproduced 4 rounds in 4 at eight copies on one core;
    /// see `docs/notes/m4-wave-outcomes.md`.
    ///
    /// A permit is one frame off the wire. Issue them at whatever fraction of
    /// the rounds the scenario wants and the client is exactly that far behind
    /// on any machine; [`Pace::release`] opens the tap for the catch-up the
    /// test ends on.
    pub fn paced(&mut self) -> (Frames, Pace) {
        let mut client = self.client.take().expect("the wire is only read once");
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let permits = Arc::new(Semaphore::new(0));
        let pace = Pace {
            permits: Arc::clone(&permits),
        };
        let task = tokio::spawn(async move {
            loop {
                let Ok(permit) = permits.acquire().await else {
                    break;
                };
                permit.forget();
                let Some((_, payload)) = read_frame(&mut client).await else {
                    break;
                };
                if tx.send(payload).is_err() {
                    break;
                }
            }
        });
        (Frames { rx, task }, pace)
    }

    /// Wait until nothing is queued for the writer.
    ///
    /// Cancellation races enqueueing: `writer::run` picks between "a frame was
    /// queued" and "we were cancelled" with no priority between them, so a test
    /// that cancels while a frame is still in the queue can lose it. Draining
    /// first is the caller's job, not the writer's.
    pub async fn drained(&self) {
        let deadline = Instant::now() + PATIENCE;
        while !self.out.is_empty() {
            assert!(Instant::now() < deadline, "the writer never drained");
            tokio::time::sleep(TICK).await;
        }
    }

    /// Stop the writer and close the pipe.
    ///
    /// The result is a `Result` and not a report because closing a pipe the
    /// writer is blocked on is exactly what a client that walked away looks
    /// like: `BrokenPipe` here is the expected outcome of the stalled-client
    /// tests, not a failure of one.
    pub async fn finish(self) -> Result<WriterReport, WriteError> {
        self.cancel.cancel();
        drop(self.client);
        self.task.await.expect("the writer task ended")
    }
}

/// How many frames a [`Wire::paced`] reader is allowed off the wire.
pub struct Pace {
    permits: Arc<Semaphore>,
}

impl Pace {
    /// Let the reader take `frames` more.
    pub fn allow(&self, frames: usize) {
        self.permits.add_permits(frames);
    }

    /// Let the reader take everything, for good.
    ///
    /// The catch-up phase, and the only honest way to end: a reader still
    /// waiting on a permit when [`Wire::finish`] closes the pipe would leave
    /// [`Frames::finish_into`] waiting on frames nobody is going to read.
    pub fn release(&self) {
        self.permits.add_permits(Semaphore::MAX_PERMITS / 2);
    }
}

/// Frames a reader task has pulled off the wire.
pub struct Frames {
    rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    task: JoinHandle<()>,
}

impl Frames {
    /// Apply everything that has arrived so far, without waiting.
    pub fn drain_into(&mut self, reference: &mut ReferenceGrid) -> usize {
        let mut frames = 0;
        while let Ok(payload) = self.rx.try_recv() {
            reference.apply(decode(&payload).expect("a decodable grid message"));
            frames += 1;
        }
        frames
    }

    /// Apply everything, waiting for the wire to close first.
    ///
    /// Call after [`Wire::finish`]: the reader task ends at end of stream, the
    /// channel closes, and this returns once the last frame is applied.
    pub async fn finish_into(mut self, reference: &mut ReferenceGrid) -> usize {
        let mut frames = 0;
        while let Some(payload) = self.rx.recv().await {
            reference.apply(decode(&payload).expect("a decodable grid message"));
            frames += 1;
        }
        let _ = self.task.await;
        frames
    }
}

/// Read one whole frame, or `None` if the peer is gone.
pub async fn read_frame<R: AsyncRead + Unpin>(source: &mut R) -> Option<(FrameHeader, Vec<u8>)> {
    let mut header = [0_u8; FRAME_HEADER_LEN];
    source.read_exact(&mut header).await.ok()?;
    let header = FrameHeader::decode(header).expect("a decodable header");
    let mut payload = vec![0_u8; header.payload_len()];
    source.read_exact(&mut payload).await.ok()?;
    Some((header, payload))
}

/// Build a grid stream for `pane` on stream 1 at the suite's frame cap.
pub fn grid_stream(pane: &Pane, policy: KeyframePolicy) -> GridStream {
    let mut config = GridStreamConfig::new(pane.host.pane(), StreamId::new(1));
    config.max_frame = MAX_FRAME;
    config.policy = policy;
    GridStream::new(config).expect("stream 1 has a channel byte")
}

// -------------------------------------------------------------- reference

/// A client grid rebuilt from nothing but the emitted stream.
///
/// Deliberately independent of the server's own bookkeeping: it holds decoded
/// cells, applies keyframes and deltas the way a client must, and refuses a
/// delta whose generation is not the one it is at — which is the rule that
/// makes `no_client_grid_is_corrupted_after_coalescing` meaningful rather than
/// self-fulfilling.
#[derive(Debug, Default)]
pub struct ReferenceGrid {
    pub rows: u16,
    pub cols: u16,
    pub grid: Vec<Vec<PackedCell>>,
    pub cursor: Option<Cursor>,
    pub generation: Option<amx_core::GridGeneration>,
    pub keyframes: usize,
    pub deltas: usize,
}

impl ReferenceGrid {
    /// Apply one decoded message.
    pub fn apply(&mut self, message: Decoded) {
        match message {
            Decoded::Reset {
                generation,
                rows,
                cols,
                grid,
                cursor,
            } => {
                self.rows = rows;
                self.cols = cols;
                self.grid = grid;
                self.cursor = Some(cursor);
                self.generation = Some(generation);
                self.keyframes += 1;
            }
            Decoded::Delta {
                generation,
                rects,
                grid,
                cursor,
            } => {
                assert_eq!(
                    self.generation,
                    Some(generation),
                    "a delta arrived for a generation the client is not at"
                );
                let mut rows = grid.into_iter();
                for rect in rects {
                    for index in rect.row..rect.row.saturating_add(rect.rows) {
                        let row = rows.next().expect("one row per rect row");
                        let slot = self
                            .grid
                            .get_mut(usize::from(index))
                            .expect("a delta named a row the client does not have");
                        *slot = row;
                    }
                }
                assert!(rows.next().is_none(), "the delta carried unnamed rows");
                self.cursor = Some(cursor);
                self.deltas += 1;
            }
            Decoded::Cursor(cursor) => self.cursor = Some(cursor),
            Decoded::Scrolled { .. } => {}
        }
    }

    /// Read every frame still in the pipe and apply it.
    pub async fn drain(&mut self, client: &mut DuplexStream) -> usize {
        let mut frames = 0;
        while let Ok(Some((_, payload))) =
            tokio::time::timeout(Duration::from_millis(50), read_frame(client)).await
        {
            self.apply(decode(&payload).expect("a decodable grid message"));
            frames += 1;
        }
        frames
    }

    /// Compare, cell for cell, against the grid the server was reading from.
    pub fn assert_matches(&self, snapshot: &Snapshot) {
        assert_eq!(self.rows, snapshot.rows(), "row count");
        assert_eq!(self.cols, snapshot.cols(), "column count");
        assert_eq!(
            self.grid.len(),
            snapshot.grid().len(),
            "the client holds a different number of rows"
        );
        for (index, (theirs, ours)) in self.grid.iter().zip(snapshot.grid()).enumerate() {
            let expected: Vec<PackedCell> = ours
                .cells()
                .iter()
                .map(|cell| expected_cell(ours, cell))
                .collect();
            assert_eq!(
                theirs,
                &expected,
                "row {index} differs: client {:?}, server {:?}",
                text_of(theirs),
                text_of(&expected)
            );
        }
    }
}

/// A decoded row as text, for a readable assertion failure.
pub fn text_of(row: &[PackedCell]) -> String {
    row.iter()
        .map(|cell| {
            if cell.text.is_empty() {
                " "
            } else {
                cell.text.as_str()
            }
        })
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// A rect list as `(row, rows)` pairs, for assertions.
pub fn spans(rects: &[DamageRect]) -> Vec<(u16, u16)> {
    rects.iter().map(|rect| (rect.row, rect.rows)).collect()
}
