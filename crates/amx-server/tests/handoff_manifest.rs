#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
//! The handoff manifest: a pane's frozen state becomes bytes and becomes the
//! same pane again (D-M3-4, D-M3-5).
//!
//! The grid half is property-tested rather than exampled, because R-M3-2 names
//! the risk precisely: wide characters, spacer cells, wrapped rows and grapheme
//! clustering all have to survive synthesize→replay, and a handful of chosen
//! screens proves nothing about the ones a real agent paints. The generator
//! writes the same things a program writes — text, SGR runs, cursor
//! addressing, erases, line feeds — into a small grid where wrapping is
//! frequent, and the assertion is cell-for-cell equality of the published
//! snapshots, cursor included.

use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;

use amx_core::platform::{ProcessId, Pty, PtyCommand, WinSize};
use amx_core::{Bus, Delivery, Event, GridGeneration, PaneId, RowId, RowRange, Subscription};
use amx_proto::stream::history::PackedRows;
use amx_server::actor::{ExportError, PaneCommand, PaneExport, PaneHost, PaneHostConfig};
use amx_server::handoff::manifest::{self, MAX_ROW_BYTES, MAX_ROWS, PaneManifest, PaneSeed};
use amx_server::history::HistoryTracker;
use amx_server::platform::UnixPty;
use amx_vt::{
    Effects, RenderState, Row, Snapshot, SnapshotRef, Snapshots, Terminal, TerminalOptions,
};
use proptest::prelude::*;
use tokio::sync::oneshot;
use tokio::time::Instant;

/// The grid the property test paints into.
///
/// Deliberately tiny: at twelve columns a generated line wraps every few
/// characters and a wide character straddles the last column often enough to be
/// generated rather than contrived.
const COLS: u16 = 12;
const ROWS: u16 = 6;

/// A terminal with the three pieces a published frame needs.
struct Vt {
    terminal: Terminal,
    render: RenderState,
    snapshots: Snapshots,
    effects: Effects,
}

impl Vt {
    fn new(cols: u16, rows: u16, scrollback: usize) -> Self {
        Self {
            terminal: Terminal::new(TerminalOptions {
                cols,
                rows,
                max_scrollback: scrollback,
            })
            .expect("a terminal"),
            render: RenderState::new().expect("a render state"),
            snapshots: Snapshots::new(cols, rows),
            effects: Effects::new(),
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        self.effects.clear();
        self.terminal.write(bytes, &mut self.effects);
    }

    fn publish(&mut self) -> SnapshotRef {
        self.render.update(&mut self.terminal).expect("an update");
        self.snapshots.publish(&mut self.render).expect("a publish");
        self.snapshots.latest()
    }
}

// ------------------------------------------------------------- the generator

/// One thing a program does to a screen.
#[derive(Clone, Debug)]
enum Paint {
    /// Print a run of characters.
    Text(String),
    /// Select graphic rendition: attributes and colours.
    Sgr(Vec<String>),
    /// Address the cursor.
    Move(u16, u16),
    /// Erase characters in place, which paints columns without writing them.
    Erase(u16),
    /// Erase to the end of the line.
    EraseLine,
    /// Move the cursor right without writing.
    Forward(u16),
    /// Carriage return and line feed.
    NewLine,
}

/// The characters the generator prints.
///
/// One narrow ASCII run, one wide (CJK) run, one grapheme cluster with a
/// combining mark, and one astral-plane character — the four classes R-M3-2
/// says have to survive, so the property test generates all four rather than
/// trusting a fixture to remember them.
const GLYPHS: &[&str] = &[
    "a",
    "Z",
    "0",
    " ",
    "~",
    "漢",
    "字",
    "🙂",
    "e\u{0301}",
    "n\u{0303}",
    "|",
    "#",
];

fn paints() -> impl Strategy<Value = Vec<Paint>> {
    let text = proptest::collection::vec(proptest::sample::select(GLYPHS), 1..10)
        .prop_map(|parts| Paint::Text(parts.concat()));
    let sgr = proptest::collection::vec(sgr_code(), 0..4).prop_map(Paint::Sgr);
    let step = prop_oneof![
        6 => text,
        4 => sgr,
        3 => (0..ROWS, 0..COLS).prop_map(|(row, col)| Paint::Move(row, col)),
        2 => (1..COLS).prop_map(Paint::Erase),
        1 => Just(Paint::EraseLine),
        2 => (1..COLS).prop_map(Paint::Forward),
        3 => Just(Paint::NewLine),
    ];
    proptest::collection::vec(step, 1..40)
}

/// One SGR parameter, drawn from every attribute the snapshot carries.
fn sgr_code() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("0".to_owned()),
        Just("1".to_owned()),
        Just("2".to_owned()),
        Just("3".to_owned()),
        Just("4".to_owned()),
        Just("4:3".to_owned()),
        Just("4:4".to_owned()),
        Just("4:5".to_owned()),
        Just("21".to_owned()),
        Just("5".to_owned()),
        Just("7".to_owned()),
        Just("8".to_owned()),
        Just("9".to_owned()),
        Just("53".to_owned()),
        (0u8..=255, 0u8..=255, 0u8..=255).prop_map(|(r, g, b)| format!("38;2;{r};{g};{b}")),
        (0u8..=255, 0u8..=255, 0u8..=255).prop_map(|(r, g, b)| format!("48;2;{r};{g};{b}")),
        (0u8..=255, 0u8..=255, 0u8..=255).prop_map(|(r, g, b)| format!("58;2;{r};{g};{b}")),
        (0u8..=255).prop_map(|index| format!("38;5;{index}")),
        (0u8..=255).prop_map(|index| format!("48;5;{index}")),
    ]
}

fn render(paints: &[Paint]) -> Vec<u8> {
    let mut out = Vec::new();
    for paint in paints {
        match paint {
            Paint::Text(text) => out.extend_from_slice(text.as_bytes()),
            Paint::Sgr(codes) => {
                out.extend_from_slice(format!("\x1b[{}m", codes.join(";")).as_bytes());
            }
            Paint::Move(row, col) => {
                out.extend_from_slice(format!("\x1b[{};{}H", row + 1, col + 1).as_bytes());
            }
            Paint::Erase(count) => out.extend_from_slice(format!("\x1b[{count}X").as_bytes()),
            Paint::EraseLine => out.extend_from_slice(b"\x1b[K"),
            Paint::Forward(count) => out.extend_from_slice(format!("\x1b[{count}C").as_bytes()),
            Paint::NewLine => out.extend_from_slice(b"\r\n"),
        }
    }
    out
}

// -------------------------------------------------------------- the compare

/// A row rendered for a failure message: one line per cell that carries
/// anything, so a mismatch names the column and the reason.
fn describe(row: &Row) -> String {
    let mut out = String::new();
    for (at, cell) in row.cells().iter().enumerate() {
        let text = String::from_utf8_lossy(row.text(cell));
        if text.is_empty()
            && cell.foreground.is_none()
            && cell.background.is_none()
            && cell.wide == amx_vt::CellWide::Narrow
            && cell.style == amx_vt::Style::default()
        {
            continue;
        }
        out.push_str(&format!(
            "\n    [{at}] {text:?} {:?} fg={:?} bg={:?} {:?}",
            cell.wide, cell.foreground, cell.background, cell.style
        ));
    }
    if row.wrapped() {
        out.push_str("\n    (wrapped)");
    }
    out
}

/// Whether two rows are the same row, cell for cell.
///
/// Two differences are allowed, and only two. Both are cell classes that
/// **cannot round-trip through the C API** — the finding R-M3-2 asks for rather
/// than a looser assertion — and both are recorded in `handoff::grid`'s
/// fidelity bounds and in `docs/notes/m3-wave-outcomes.md`:
///
/// - a column that is **blank but carries SGR attributes** crosses as a space
///   carrying the same attributes. libghostty-vt's `Screen.blankCell` returns
///   `style.bgCell()`, so an erase paints a *background* into a column and can
///   never paint an attribute into one, and nothing else writes a column
///   without writing a character. The space keeps every attribute and every
///   colour, so the column renders exactly as it did; what it loses is the
///   never-written bit;
/// - a **spacer head** — the padding before a wide character that wrapped — is
///   a derived cell, and the replay re-derives it by printing the same wide
///   character at the same column. So it comes back with *that* character's
///   paint, and where the exporter's grid holds a head the C API can no longer
///   re-derive at all — because the wide character it padded has since been
///   erased — the column crosses blank. Its paint is set by the print that
///   wraps and by nothing else, and every sequence that could touch the cell
///   afterwards clears it outright: `Screen.splitCellBoundary` erases a spacer
///   head whenever the wide character below it is disturbed. What is *not*
///   allowed to differ is the wrap flag, which is the thing the padding is a
///   side effect of.
fn same_row(a: &Row, b: &Row) -> bool {
    if a == b {
        return true;
    }
    if a.wrapped() != b.wrapped() || a.cells().len() != b.cells().len() {
        return false;
    }
    a.cells().iter().zip(b.cells()).all(|(one, two)| {
        // Padding: what matters is that the column holds no glyph, which is
        // all a spacer head ever held.
        if one.wide == amx_vt::CellWide::SpacerHead {
            return b.text(two).is_empty() || b.text(two) == b" ";
        }
        if one.wide != two.wide {
            return false;
        }
        if one.foreground != two.foreground
            || one.background != two.background
            || one.style != two.style
        {
            return false;
        }
        let (left, right) = (a.text(one), b.text(two));
        left == right
            || (left.is_empty()
                && right == b" "
                && (one.style != amx_vt::Style::default() || one.foreground.is_some()))
    })
}

/// Assert two published frames are the same screen, cell for cell.
fn assert_same(want: &Snapshot, got: &Snapshot, bytes: &[u8]) {
    let replay = String::from_utf8_lossy(bytes).into_owned();
    assert_eq!(
        (want.cols(), want.rows()),
        (got.cols(), got.rows()),
        "the replay changed the grid size"
    );
    for (index, (a, b)) in want.grid().iter().zip(got.grid()).enumerate() {
        assert!(
            same_row(a, b),
            "row {index} differs\n  wanted:{}\n  got:{}\n  replay: {replay:?}",
            describe(a),
            describe(b),
        );
    }
    assert_eq!(
        want.cursor(),
        got.cursor(),
        "the cursor differs; replay: {replay:?}"
    );
}

// ------------------------------------------------------------- a real pane

/// How long a test waits for a pane to do something.
const PATIENCE: Duration = Duration::from_secs(10);

/// A short frame interval, so a test does not spend its patience waiting for
/// output to coalesce.
const FRAME: Duration = Duration::from_millis(2);

/// A running pane over a real pty, plus the bus it publishes on.
///
/// The child is a `sleep`: everything these tests paint is written straight
/// into the pane's *terminal* through `PaneCommand::Seed`, which is the restore
/// path (D-M1-6) and the one way to put an exact screen in front of the capture
/// without racing a shell.
struct Pane {
    host: PaneHost,
    bus: Arc<Bus>,
    events: Subscription,
}

impl Pane {
    fn start(size: WinSize, seed: Option<PaneSeed>) -> Self {
        let bus = Arc::new(Bus::default());
        let events = bus.subscribe();
        let command = PtyCommand {
            program: OsString::from("/bin/sh"),
            args: vec![OsString::from("-c"), OsString::from("sleep 300")],
            env: vec![
                (OsString::from("TERM"), OsString::from("xterm-256color")),
                (OsString::from("LC_ALL"), OsString::from("C")),
            ],
            cwd: None,
            size,
        };
        let session = UnixPty.spawn(&command).expect("spawn a pty");
        let mut config = PaneHostConfig::new(PaneId::new_v4(), Arc::clone(&bus), size);
        config.frame_interval = FRAME;
        config.seed = seed;
        Self {
            host: PaneHost::spawn(config, session).expect("start the pane host"),
            bus,
            events,
        }
    }

    /// Resize the grid, which is what bumps the pane's generation.
    async fn resize(&self, size: WinSize) {
        self.host
            .handle()
            .send(PaneCommand::Resize {
                rows: size.rows,
                cols: size.cols,
            })
            .await
            .expect("the pane is running");
        // The bump happens on the parser thread when the terminal reflows, so
        // a frame taken afterwards is what orders the two.
        let _ = self.snapshot().await;
    }

    /// Write bytes into the pane's terminal, as restore does.
    async fn paint(&self, bytes: Vec<u8>) {
        self.host
            .handle()
            .send(PaneCommand::Seed(bytes))
            .await
            .expect("the pane is running");
    }

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

    async fn history(&self, range: RowRange) -> Vec<String> {
        let (reply, answer) = oneshot::channel();
        self.host
            .handle()
            .send(PaneCommand::HistoryRange { range, reply })
            .await
            .expect("the pane is running");
        let rows = tokio::time::timeout(PATIENCE, answer)
            .await
            .expect("history before the deadline")
            .expect("the parser answered")
            .expect("the range is serveable");
        unpack(&rows.rows)
    }

    /// Ask for the export without quiescing first.
    async fn try_export(&self) -> Result<PaneExport, ExportError> {
        let (reply, answer) = oneshot::channel();
        self.host
            .handle()
            .send(PaneCommand::ExportHandoff { reply })
            .await
            .expect("the pane is running");
        tokio::time::timeout(PATIENCE, answer)
            .await
            .expect("an export before the deadline")
            .expect("the parser answered")
    }

    /// Freeze the pane the way the orchestrator will, then take the manifest.
    async fn export(&self) -> PaneExport {
        self.quiesce().await;
        self.try_export().await.expect("a quiesced pane exports")
    }

    /// The pty handle's quiesce blocks for the drain, so it goes to the
    /// blocking pool rather than parking a runtime worker.
    async fn quiesce(&self) {
        let host = &self.host;
        tokio::task::block_in_place(|| host.quiesce()).expect("the pane quiesces");
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

/// Unpack the server's row packing: `u32` length, text, one flag byte.
fn unpack(bytes: &[u8]) -> Vec<String> {
    PackedRows::new(bytes)
        .map(|row| {
            let row = row.expect("the packing is self-delimiting");
            String::from_utf8_lossy(row.text).trim_end().to_owned()
        })
        .collect()
}

/// The screen of a snapshot, one trimmed line per row.
fn screen(snapshot: &SnapshotRef) -> Vec<String> {
    snapshot
        .grid()
        .iter()
        .map(|row| row.line().trim_end().to_owned())
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// Synthesize → replay → publish must give back the same cells.
    #[test]
    fn a_styled_grid_synthesizes_replays_and_matches_cell_for_cell(paints in paints()) {
        let mut source = Vt::new(COLS, ROWS, 64 * 1024);
        source.write(&render(&paints));
        let want = source.publish();

        let mut bytes = Vec::new();
        amx_server::handoff::grid::synthesize(&want, &mut bytes);
        // The seeding path applies the cursor after the modes; there are none
        // here, so the two runs are simply concatenated.
        amx_server::handoff::grid::put_cursor(want.cursor(), &mut bytes);

        let mut target = Vt::new(COLS, ROWS, 64 * 1024);
        target.write(&bytes);
        let got = target.publish();

        assert_same(&want, &got, &bytes);
    }
}

// ------------------------------------------------------------------- modes

/// The size the pane tests run at.
const PANE: WinSize = WinSize { rows: 8, cols: 40 };

/// Every mode, flag and title the round trip has to carry, plus a screen to
/// carry them on: the alternate screen, bracketed paste, two mouse modes,
/// application cursor keys, wraparound *off*, kitty keyboard flags, a title, a
/// styled line, a cursor shape and a hidden cursor.
///
/// Written into the pane's terminal rather than sent to the child: these tests
/// are about what the *terminal* holds, and a shell would only add noise
/// between the paint and the capture.
const MODES: &[u8] = b"\x1b[?1049h\x1b[?2004h\x1b[?1002h\x1b[?1006h\x1b[?1h\x1b[?7l\
\x1b[=13;1u\x1b]2;handoff title\x1b\\\x1b[3;5H\x1b[1;38;2;10;20;30malt screen\x1b[0m\
\x1b[4 q\x1b[?25l";

/// The modes the assertion also names one by one, on top of the whole set.
const NAMED: &[u16] = &[2004, 1002, 1006, 1, 7];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn modes_survive_the_round_trip() {
    let before = Pane::start(PANE, None);
    before.paint(MODES.to_vec()).await;
    // Move both counters off their starting values, or seeding them and not
    // seeding them would look the same: two applied resizes bump the grid
    // generation twice and land back on the size the successor is built at, and
    // a run of published frames puts the frame counter somewhere a fresh buffer
    // cannot reach in the handful of frames the successor publishes below.
    before.resize(WinSize { rows: 8, cols: 41 }).await;
    before.resize(PANE).await;
    for _ in 0..40 {
        let _ = before.snapshot().await;
    }
    let painted = screen(&before.snapshot().await);
    let export = before.export().await;

    // The successor's pane: a fresh terminal, a fresh pty, and nothing but the
    // manifest entry to rebuild from.
    let after = Pane::start(PANE, Some(export.manifest.seed().expect("a seed")));
    let replayed = screen(&after.snapshot().await);
    assert_eq!(replayed, painted, "the alternate screen did not come back");

    // Capture → seed → capture is a fixed point: comparing the two entries
    // compares every mode, flag and title through the reader the exporter used,
    // rather than through a second implementation living in this file.
    let round = after.export().await;
    assert!(
        round.manifest.modes.alternate,
        "the pane came back on the primary screen"
    );
    assert_eq!(
        round.manifest.modes.modes, export.manifest.modes.modes,
        "a carried mode changed across the round trip"
    );
    for &number in NAMED {
        assert_eq!(
            round.manifest.modes.is_set(number, false),
            export.manifest.modes.is_set(number, false),
            "DEC mode {number} did not survive"
        );
    }
    assert!(
        !round.manifest.modes.is_set(7, true),
        "wraparound came back on, so the modes were replayed as defaults"
    );
    assert_eq!(round.manifest.modes.kitty_flags, 13, "kitty keyboard flags");
    assert_eq!(
        round.manifest.modes.title.as_deref(),
        Some("handoff title"),
        "the title did not survive"
    );

    let (want, got) = (before.snapshot().await, after.snapshot().await);
    assert_eq!(want.cursor(), got.cursor(), "the cursor did not survive");
    assert!(!got.cursor().visible, "DECTCEM did not survive");

    // The two counters a successor continues rather than restarts: the grid
    // generation is 04 §4's delta contract, and the frame counter is what a
    // reader compares to tell "nothing changed" from "I have not looked yet".
    assert_eq!(
        round.manifest.generation, export.manifest.generation,
        "the grid generation restarted on the successor",
    );
    assert!(
        round.manifest.frame > export.manifest.frame,
        "the frame counter restarted: {} then {}",
        export.manifest.frame,
        round.manifest.frame,
    );

    before.stop().await;
    after.stop().await;
}

// --------------------------------------------------------------- scrollback

/// How many lines the scrollback test writes.
const LINES: usize = 60;

fn lines(from: usize, count: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for line in from..from + count {
        out.extend_from_slice(format!("line {line}\r\n").as_bytes());
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recent_rows_ride_packed_and_land_with_contiguous_row_ids() {
    let before = Pane::start(PANE, None);
    before.paint(lines(0, LINES)).await;
    // A frame boundary is where the tracker looks, so rows are only committed
    // once one has happened.
    let _ = before.snapshot().await;
    let export = before.export().await;

    let carried = export.manifest.history.clone();
    assert!(
        carried.count >= 3 && carried.count <= MAX_ROWS,
        "nothing rode along: {carried:?}"
    );
    let packed = unpack(&carried.rows().expect("the packed rows decode"));
    assert_eq!(
        u64::try_from(packed.len()).expect("a row count"),
        carried.count,
        "the packed rows and the count disagree"
    );
    // Row ids and line numbers coincide here — nothing was evicted, so the
    // floor is zero — which is what lets the assertion name rows by content.
    // The newest *committed* row is the one above the live grid, not the last
    // line written: the bottom rows are still on screen.
    assert_eq!(
        packed.last().map(String::as_str),
        Some(format!("line {}", carried.head.get() - 1).as_str()),
        "the newest carried row is not the newest committed row"
    );
    assert_eq!(
        packed.first().map(String::as_str),
        Some(format!("line {}", carried.first.get()).as_str()),
        "the carried range does not start where it says it does"
    );

    let mut after = Pane::start(PANE, Some(export.manifest.seed().expect("a seed")));
    // The id space continues: a row the exporter would have served under some
    // id is the same row on the successor, under the same id.
    let sampled = RowRange::new(carried.first, RowId::from_raw(carried.first.get() + 2));
    assert_eq!(
        after.history(sampled).await,
        before.history(sampled).await,
        "row range {sampled:?} means something different on the successor",
    );

    // And the next row committed takes the next id, never a restarted one.
    after.paint(lines(LINES, 1)).await;
    let _ = after.snapshot().await;
    let committed = after
        .wait_for_event(|event| matches!(event, Event::HistoryCommitted { .. }))
        .await;
    let Event::HistoryCommitted { range, .. } = committed else {
        unreachable!("the predicate matched this variant")
    };
    assert_eq!(
        range.first, carried.head,
        "the successor's next commit restarted the id space",
    );

    before.stop().await;
    after.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_refuses_a_running_pane() {
    let pane = Pane::start(PANE, None);
    pane.paint(b"still moving\r\n".to_vec()).await;
    let _ = pane.snapshot().await;

    let refused = pane.try_export().await;
    assert!(
        matches!(refused, Err(ExportError::NotQuiesced)),
        "a running pane must not export: {refused:?}",
    );

    // The same rule `DupFd` answers to, from the same actor: once the terminal
    // is held still the very same call succeeds.
    pane.quiesce().await;
    let export = pane.try_export().await.expect("a quiesced pane exports");
    assert_eq!(export.manifest.rows, PANE.rows);
    assert_eq!(export.manifest.cols, PANE.cols);

    pane.stop().await;
}

// ------------------------------------------------------------------ budgets

/// A terminal wide enough that the byte cap binds before the row cap.
///
/// A packed row is its whole width — `read_row` pads to the grid — plus five
/// bytes of framing, so 600 columns puts [`MAX_ROWS`] rows well past
/// [`MAX_ROW_BYTES`] and the two budgets can be told apart.
const WIDE: u16 = 600;

/// Driven against the terminal directly, like `history.rs` next door: every
/// assertion here is about *which rows* the budget kept, and a pty would only
/// add a child between the write and the read.
#[test]
fn the_per_pane_byte_cap_truncates_oldest_first_and_says_so() {
    let mut vt = Vt::new(WIDE, 4, 8 * 1024 * 1024);
    let mut tracker = HistoryTracker::new(WIDE, 4);
    let mut found = Vec::new();
    let filler = "x".repeat(usize::from(WIDE) - 12);
    for line in 0..MAX_ROWS + 200 {
        vt.write(format!("line {line} {filler}\r\n").as_bytes());
        if line % 64 == 0 {
            tracker.observe(&mut vt.terminal, &mut found);
            found.clear();
        }
    }
    tracker.observe(&mut vt.terminal, &mut found);
    let snapshot = vt.publish();

    let entry = PaneManifest::capture(manifest::Capture {
        pane: PaneId::new_v4(),
        child: ProcessId(1),
        generation: GridGeneration::FIRST,
        terminal: &vt.terminal,
        tracker: &mut tracker,
        snapshot: &snapshot,
    })
    .expect("a capture");

    let history = &entry.history;
    let packed = history.rows().expect("the packed rows decode");
    assert!(
        packed.len() <= MAX_ROW_BYTES,
        "the byte cap did not bind: {} bytes",
        packed.len()
    );
    assert!(
        history.truncated && history.dropped > 0,
        "truncation was silent: {history:?}"
    );
    assert!(
        history.count < MAX_ROWS,
        "the byte cap must bind before the row cap for this test to mean \
         anything: {} rows in {} bytes",
        history.count,
        packed.len(),
    );

    // Oldest first: the newest row is the newest line, and the first row
    // carried is exactly the id the count says it is.
    let rows = unpack(&packed);
    assert_eq!(
        u64::try_from(rows.len()).expect("a row count"),
        history.count
    );
    assert_eq!(
        history.first.get() + history.count,
        history.head.get(),
        "the carried range and the head disagree"
    );
    // Row ids and line numbers coincide until the terminal's own scrollback
    // evicts, and it is sized here so that it does not. The newest carried row
    // is therefore the newest committed one, and the oldest is the id the
    // truncation left standing.
    assert!(
        rows.last()
            .is_some_and(|row| row.starts_with(&format!("line {} ", history.head.get() - 1))),
        "the newest row was dropped instead of the oldest: {:?}",
        rows.last().map(|row| row.get(..12))
    );
    assert!(
        rows.first()
            .is_some_and(|row| row.starts_with(&format!("line {} ", history.first.get()))),
        "the carried range does not start where it says it does: {:?}",
        rows.first().map(|row| row.get(..12))
    );
    assert!(
        history.first > history.floor,
        "a truncation that dropped {} rows left the floor above it: {history:?}",
        history.dropped,
    );
}
