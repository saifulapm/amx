//! T15 acceptance: the scrollback cache and copy mode over it.
//!
//! These tests drive the cache and the copy-mode engine directly with decoded
//! state — commits, fills, invalidations, evictions — because no wire path
//! delivers any of it yet (`HistoryChunk`'s codec is `todo!()` and no control
//! method requests a range): the same stand-in shape every other suite in
//! this crate documents. The seam test at the bottom is the exception — it
//! drives the real modal input machine through `App` against a real server,
//! proving copy mode is entered from navigate and consumes its keys.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use amx_client::app::{App, Mode};
use amx_client::cache::{RowSlot, Scrollback};
use amx_client::copy::{CopyMode, Outcome};
use amx_client::input::InputEvent;
use amx_core::{RowId, RowRange};
use amx_proto::ClientInfo;

fn row(id: u64) -> RowId {
    RowId::from_raw(id)
}

fn range(first: u64, last: u64) -> RowRange {
    RowRange::new(row(first), row(last))
}

/// A cache whose rows `first..=last` are committed, with `fill_from..` cached
/// as `"row {id}"`.
fn cache_with(first: u64, last: u64, fill_from: u64) -> Scrollback {
    let mut cache = Scrollback::new();
    cache.commit(range(first, last));
    let rows: Vec<String> = (fill_from..=last).map(|id| format!("row {id}")).collect();
    cache.fill(range(fill_from, last), rows.iter().map(String::as_str));
    cache
}

fn text_of(cache: &Scrollback, id: u64) -> &str {
    match cache.slot(row(id)) {
        RowSlot::Cached(text) => text,
        other => panic!("row {id} should be cached, got {other:?}"),
    }
}

#[test]
fn scroll_up_is_served_from_cache_without_a_server_round_trip() {
    // Rows 0..=99 committed, 40..=99 cached.
    let cache = cache_with(0, 99, 40);
    let mut copy = CopyMode::open(&cache, 10).expect("history exists");
    let mut fetch = Vec::new();

    // At the bottom the viewport is fully cached: nothing to ask for.
    copy.wanted(&cache, &mut fetch);
    assert!(fetch.is_empty(), "cached view asked the server: {fetch:?}");

    // Fifty rows up, still inside the cached range: the scroll is served
    // locally, and no range request leaves the client.
    for _ in 0..50 {
        copy.key(b'k', &cache);
    }
    assert_eq!(copy.cursor().row, row(49));
    copy.wanted(&cache, &mut fetch);
    assert!(
        fetch.is_empty(),
        "a locally-served scroll must not round-trip: {fetch:?}"
    );
    assert_eq!(text_of(&cache, 49), "row 49");

    // Scrolling past what is cached asks for exactly the missing rows —
    // range requests fill misses, they do not re-fetch what is held.
    for _ in 0..10 {
        copy.key(b'k', &cache);
    }
    copy.wanted(&cache, &mut fetch);
    assert_eq!(fetch, vec![range(39, 39)], "only the miss is requested");
}

#[test]
fn history_invalidated_drops_cached_ranges_at_or_beyond_from_row() {
    let mut cache = cache_with(0, 99, 0);

    cache.invalidate(row(60));

    // Below `from_row` the cache is intact.
    assert_eq!(text_of(&cache, 0), "row 0");
    assert_eq!(text_of(&cache, 59), "row 59");
    // At and beyond it, the rows are gone — and not refetchable either,
    // because those ids were rebaselined away and asking for them would only
    // be refused.
    for id in [60, 75, 99] {
        assert_eq!(
            cache.slot(row(id)),
            RowSlot::Unavailable,
            "row {id} must be dropped by the invalidation"
        );
    }
    let mut fetch = Vec::new();
    cache.misses(range(0, 99), &mut fetch);
    assert!(
        fetch.is_empty(),
        "invalidated ids must not be requested: {fetch:?}"
    );
}

#[test]
fn history_invalidated_cancels_a_selection_anchored_beyond_from_row() {
    let mut cache = cache_with(0, 99, 0);
    let mut copy = CopyMode::open(&cache, 10).expect("history exists");

    // Anchor a selection on the newest row, then invalidate below it.
    copy.key(b'v', &cache);
    assert!(copy.selection().is_some());
    cache.invalidate(row(70));
    assert!(
        copy.on_invalidated(row(70), &cache),
        "rows survive below 70"
    );
    assert_eq!(
        copy.selection(),
        None,
        "a selection anchored at or beyond from_row must be cancelled"
    );
    assert_eq!(
        copy.cursor().row,
        row(69),
        "the cursor is pulled back onto surviving rows"
    );

    // A selection entirely below `from_row` survives the next invalidation.
    for _ in 0..19 {
        copy.key(b'k', &cache);
    }
    assert_eq!(copy.cursor().row, row(50));
    copy.key(b'v', &cache);
    for _ in 0..5 {
        copy.key(b'j', &cache);
    }
    cache.invalidate(row(60));
    assert!(copy.on_invalidated(row(60), &cache));
    let (start, end) = copy.selection().expect("the selection survives");
    assert_eq!((start.row, end.row), (row(50), row(55)));
}

#[test]
fn range_below_the_eviction_floor_is_reported_unavailable_not_blank() {
    let mut cache = cache_with(0, 99, 0);

    cache.evict(row(40));

    // Evicted rows are *unavailable* — distinguishable from cached-empty, so
    // no renderer can pass a blank off as content.
    for id in [0, 10, 39] {
        let slot = cache.slot(row(id));
        assert!(
            !matches!(slot, RowSlot::Cached(_)),
            "row {id} below the floor must not read as content: {slot:?}"
        );
        assert_eq!(slot, RowSlot::Unavailable);
    }
    assert_eq!(text_of(&cache, 40), "row 40", "the floor row survives");

    // Fetch planning never asks below the floor: those rows are not gaps.
    let mut cache = Scrollback::new();
    cache.commit(range(0, 99));
    let rows: Vec<String> = (50..=99).map(|id| format!("row {id}")).collect();
    cache.fill(range(50, 99), rows.iter().map(String::as_str));
    cache.evict(row(40));
    let mut fetch = Vec::new();
    cache.misses(range(0, 99), &mut fetch);
    assert_eq!(
        fetch,
        vec![range(40, 49)],
        "only fetchable rows are requested, nothing below the floor"
    );

    // A stale transfer cannot resurrect evicted rows as content.
    let stale: Vec<String> = (0..40).map(|_| String::from("stale")).collect();
    cache.fill(range(0, 39), stale.iter().map(String::as_str));
    assert_eq!(cache.slot(row(39)), RowSlot::Unavailable);
}

#[test]
fn selection_survives_output_arriving_while_scrolled_back() {
    let mut cache = cache_with(0, 49, 0);
    let mut copy = CopyMode::open(&cache, 10).expect("history exists");

    // Scroll back to row 25 and select "row 25\nrow " across two rows.
    for _ in 0..24 {
        copy.key(b'k', &cache);
    }
    assert_eq!(copy.cursor().row, row(25));
    copy.key(b'v', &cache);
    copy.key(b'j', &cache);
    for _ in 0..3 {
        copy.key(b'l', &cache);
    }
    let held = copy.selection().expect("a selection is active");
    let top = copy.top();

    // Thirty rows of new output commit and arrive while we are scrolled
    // back. Stable-row coordinates mean nothing we hold moves.
    cache.commit(range(50, 79));
    let fresh: Vec<String> = (50..=79).map(|id| format!("row {id}")).collect();
    cache.fill(range(50, 79), fresh.iter().map(String::as_str));

    assert_eq!(copy.selection(), Some(held), "the selection did not move");
    assert_eq!(copy.top(), top, "the view did not move");
    let mut fetch = Vec::new();
    copy.wanted(&cache, &mut fetch);
    assert!(fetch.is_empty());

    // And the yank copies exactly the rows selected before the output came:
    // OSC 52 with the base64 of "row 25\nrow ".
    let Outcome::Yank(bytes) = copy.key(b'y', &cache) else {
        panic!("y with a selection must yank");
    };
    assert_eq!(bytes, b"\x1b]52;c;cm93IDI1CnJvdyA=\x1b\\");
}

/// The seam T14 declared: `c` in navigate enters copy mode, its keys are
/// consumed by the mode (never forwarded to the pane), and the key table's
/// leave keys — `Esc`, `y` — return to terminal mode.
#[tokio::test]
async fn copy_mode_is_entered_from_navigate_and_consumes_its_keys() {
    let server = support::Server::start("copy-seam").await;
    let pty = support::open_pty();
    let mut app: App<std::fs::File, Vec<u8>> = App::attach(
        server.socket(),
        pty.slave,
        Vec::new(),
        ClientInfo {
            name: "amx-copy-seam-test".to_owned(),
            version: "0.0.0".to_owned(),
            term: None,
        },
    )
    .await
    .expect("attach to the real server");

    let mut evs = Vec::new();
    let drive = |app: &mut App<std::fs::File, Vec<u8>>, bytes: &[u8]| {
        let mut out = Vec::new();
        app.handle_input(bytes, &mut |event| {
            if let InputEvent::Forward { bytes, .. } = event {
                out.push(bytes.to_vec());
            }
        });
        out
    };

    app.handle_input(b"\x01wc", &mut |_| {});
    assert_eq!(app.mode(), Mode::Copy, "navigate `c` enters copy mode");

    evs.extend(drive(&mut app, b"jkhl0$gGv"));
    assert!(evs.is_empty(), "copy-mode keys leaked to a pane: {evs:?}");
    assert_eq!(app.mode(), Mode::Copy, "movement keys do not end the mode");

    drive(&mut app, b"\x1b");
    assert_eq!(app.mode(), Mode::Terminal, "Esc leaves copy mode");

    drive(&mut app, b"\x01wc");
    assert_eq!(app.mode(), Mode::Copy);
    let leaked = drive(&mut app, b"y");
    assert!(leaked.is_empty(), "y leaked to a pane: {leaked:?}");
    assert_eq!(app.mode(), Mode::Terminal, "y yanks on its way out");

    drop(pty.master);
    server.shutdown().await;
}
