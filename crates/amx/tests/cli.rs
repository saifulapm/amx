//! The parts of the CLI worth testing as values: the tree, the session
//! selection, the detach chord and the chrome-free paint.
//!
//! Everything here would otherwise be asserted by scraping the output of a
//! process, which is a worse test of the same thing.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::collections::HashMap;
use std::path::PathBuf;

use amx::cmd::detach::{Chord, DETACH_PANE, PREFIX};
use amx::cmd::viewport::paint;
use amx_client::model::{Cell, PaneGrid};
use amx_client::render::FrameWriter;
use amx_core::{Env, GridGeneration, Rect, SessionName};
use amx_proto::stream::{Cursor, CursorShape};

// ------------------------------------------------------------------ the tree

#[test]
fn the_verb_tree_is_the_method_table_plus_the_lifecycle_verbs() {
    let cli = amx::cli::cli();
    let names: Vec<String> = cli
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .collect();

    // Hand-written: process lifecycle, which has no wire method behind it.
    for verb in ["attach", "server", "session"] {
        assert!(
            names.contains(&verb.to_owned()),
            "missing {verb}: {names:?}"
        );
    }
    // Generated: every domain in the method table, without naming one here.
    for spec in amx_proto::control::SPECS {
        let head = spec.cli[0].to_owned();
        assert!(
            names.contains(&head),
            "the table declares {head} and the tree does not have it"
        );
    }
}

#[test]
fn every_generated_verb_takes_its_parameters_as_json() {
    let cli = amx::cli::cli();
    for spec in amx_proto::control::SPECS {
        let mut command = &cli;
        let mut found = None;
        for segment in spec.cli {
            found = command
                .get_subcommands()
                .find(|sub| sub.get_name() == *segment);
            let Some(next) = found else {
                panic!("{} is not in the tree", spec.wire);
            };
            command = next;
        }
        let leaf = found.expect("a leaf");
        assert!(
            leaf.get_arguments()
                .any(|arg| arg.get_id() == amx::cli::PARAMS),
            "{} takes no --params, so it cannot be called at all",
            spec.wire
        );
    }
}

#[test]
fn the_session_flag_outranks_the_environment_which_outranks_the_default() {
    let env_named = Env {
        session: Some(SessionName::new("from-env").expect("valid")),
        ..Env::default()
    };

    let bare = amx::cli::cli()
        .try_get_matches_from(["amx"])
        .expect("parse");
    assert_eq!(
        amx::session_of(&env_named, &bare, None)
            .expect("select")
            .as_str(),
        "from-env"
    );
    assert_eq!(
        amx::session_of(&Env::default(), &bare, None)
            .expect("select")
            .as_str(),
        SessionName::DEFAULT
    );

    // Global, so it may come before or after the subcommand.
    for argv in [
        vec!["amx", "--session", "from-flag", "session", "list"],
        vec!["amx", "session", "list", "--session", "from-flag"],
    ] {
        let matches = amx::cli::cli().try_get_matches_from(argv).expect("parse");
        assert_eq!(
            amx::session_of(&env_named, &matches, None)
                .expect("select")
                .as_str(),
            "from-flag"
        );
    }

    // And a `session stop <name>` positional outranks both.
    assert_eq!(
        amx::session_of(&env_named, &bare, Some("positional"))
            .expect("select")
            .as_str(),
        "positional"
    );
    assert!(
        amx::session_of(&env_named, &bare, Some("not/a/name")).is_err(),
        "a name that cannot be a directory component is refused up front"
    );
}

#[test]
fn a_session_selects_its_own_runtime_and_state_directories() {
    let env = Env {
        runtime_dir: Some(PathBuf::from("/run/user/1000")),
        state_home: Some(PathBuf::from("/home/u/.local/state")),
        config_home: Some(PathBuf::from("/home/u/.config")),
        ..Env::default()
    };
    let matches = amx::cli::cli()
        .try_get_matches_from(["amx", "--session", "work"])
        .expect("parse");
    let ctx = amx::ctx_of(&env, &matches, None).expect("derive");

    assert_eq!(ctx.socket, PathBuf::from("/run/user/1000/amx/work/sock"));
    assert_eq!(
        ctx.state_dir,
        PathBuf::from("/home/u/.local/state/amx/work")
    );
    // Config is per user, so it takes no session component — the one file
    // every server this user runs reloads independently.
    assert_eq!(
        ctx.config_path,
        PathBuf::from("/home/u/.config/amx/config.toml")
    );
}

// ----------------------------------------------------------- the detach chord

#[test]
fn the_viewport_detach_chord_is_the_prefix_and_then_q() {
    let mut chord = Chord::new(DETACH_PANE);
    assert!(!chord.feed(b"hello"), "ordinary keys are not a chord");
    assert!(
        !chord.feed(&[DETACH_PANE]),
        "nor is the key without the prefix"
    );

    assert!(!chord.feed(&[PREFIX]));
    assert!(chord.is_armed());
    assert!(chord.feed(&[DETACH_PANE]), "prefix then q detaches");
    assert!(!chord.is_armed(), "and disarms");

    // Split across reads, because a terminal delivers what it delivers.
    let mut split = Chord::new(DETACH_PANE);
    assert!(!split.feed(&[PREFIX]));
    assert!(split.feed(&[DETACH_PANE]));

    // A different key after the prefix is not a detach, and disarms.
    let mut other = Chord::new(DETACH_PANE);
    assert!(!other.feed(&[PREFIX, b'x']));
    assert!(!other.is_armed());
    assert!(!other.feed(&[DETACH_PANE]));

    // A doubled prefix re-arms: `ctrl+a ctrl+a` is how a literal prefix is
    // sent, so the state after it is "prefix pending", not "back to normal".
    let mut doubled = Chord::new(DETACH_PANE);
    assert!(!doubled.feed(&[PREFIX, PREFIX]));
    assert!(doubled.is_armed());
    assert!(doubled.feed(&[DETACH_PANE]));

    // The full client's detach is the input machine's prefix `d` verb, not a
    // chord here; `d` after the prefix means nothing to this recogniser.
    let mut full = Chord::new(DETACH_PANE);
    assert!(!full.feed(&[PREFIX, b'd']));
}

// -------------------------------------------------------------- the viewport

#[test]
fn the_one_pane_viewport_paints_the_whole_area_and_no_chrome() {
    let rows = 6;
    let cols = 10;
    let mut grid = PaneGrid::blank(rows, cols);
    let filled: Vec<Cell> = (0..usize::from(rows) * usize::from(cols))
        .map(|i| Cell {
            ch: char::from(b'a' + (i % 26) as u8),
            ..Cell::default()
        })
        .collect();
    grid.apply_reset(
        GridGeneration::FIRST,
        rows,
        cols,
        &filled,
        Cursor {
            row: 2,
            col: 3,
            visible: true,
            shape: CursorShape::default(),
            blink: false,
        },
    );

    let mut writer = FrameWriter::new();
    let area = Rect::new(0, 0, cols, rows);
    paint(&mut writer, &grid, area);
    let painted = rasterize(writer.bytes());

    // Every cell of the terminal is the pane's, including the bottom row the
    // chrome client reserves for its status line and the edges it would draw a
    // border on.
    for row in 0..rows {
        for col in 0..cols {
            let expected = grid.cell(row, col).expect("a cell").ch;
            assert_eq!(
                painted.get(&(row, col)).copied(),
                Some(expected),
                "cell ({row}, {col}) is the pane's, not chrome's"
            );
        }
    }
    for glyph in ['┌', '┐', '└', '┘', '─', '│'] {
        assert!(
            !painted.values().any(|ch| *ch == glyph),
            "no border is drawn, and {glyph} is one"
        );
    }
    assert!(
        !writer.bytes().windows(4).any(|w| w == b"\x1b[7m"),
        "no status line, which is the only reverse-video thing the client draws"
    );
}

#[test]
fn a_viewport_hides_the_cursor_when_the_pane_does() {
    let mut grid = PaneGrid::blank(4, 4);
    grid.apply_cursor(Cursor {
        row: 1,
        col: 1,
        visible: false,
        shape: CursorShape::default(),
        blink: false,
    });
    let mut writer = FrameWriter::new();
    paint(&mut writer, &grid, Rect::new(0, 0, 4, 4));
    assert!(
        writer.bytes().windows(6).any(|w| w == b"\x1b[?25l"),
        "an invisible pane cursor is an invisible terminal cursor"
    );
}

/// Reconstruct a `(row, col) -> char` map from a frame's ANSI bytes, tracking
/// the two sequences `FrameWriter` emits: `CSI row;col H` and `CSI … m`.
fn rasterize(bytes: &[u8]) -> HashMap<(u16, u16), char> {
    let text = std::str::from_utf8(bytes).expect("frame bytes are valid utf-8");
    let mut cells = HashMap::new();
    let (mut row, mut col) = (0_u16, 0_u16);
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            cells.insert((row, col), c);
            col += 1;
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
        }
        let mut params = String::new();
        let mut terminator = '\0';
        for c2 in chars.by_ref() {
            if c2.is_ascii_digit() || c2 == ';' || c2 == '?' {
                params.push(c2);
            } else {
                terminator = c2;
                break;
            }
        }
        if terminator == 'H' {
            let mut parts = params.split(';');
            row = parts
                .next()
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(1)
                .saturating_sub(1);
            col = parts
                .next()
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(1)
                .saturating_sub(1);
        }
    }
    cells
}
