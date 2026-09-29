//! Column widths for the wall, and fitting text into a column.
//!
//! Rows and the headings over them take their columns from here, so the two
//! cannot drift apart and the geometry is testable without a terminal.
//! Nothing here draws. All widths are in terminal cells, not chars.

use ratatui::text::Span;

use super::rows::Axis;

/// Screens at least this wide get the wide name column.
const WIDE: usize = 100;

/// The name column: 22 cells on a wide screen, 16 below it. The same on every
/// axis, so turning the axis never moves it.
const WIDE_NAME: usize = 22;
const NARROW_NAME: usize = 16;

/// The state word's column on the path axes, sized for `starting`, the
/// longest. It never shrinks, because a truncated state word misleads.
const STATE: usize = 8;

/// The vendor column, sized for `claude sonnet high` (program, model, effort),
/// the longest the registry produces. It never shrinks: it is only shown on
/// request.
const VENDOR: usize = 18;

/// The age column, which fits up to `365d`.
const AGE: usize = 4;

/// Before the name: one cell of indent, the state glyph and a space.
const PREFIX: usize = 3;

/// Space between two columns.
const GAP: usize = 2;

/// The fewest cells a path heading leaves free after its path.
const SHORTEST_RULE: usize = 8;

/// The column widths of an agent's row for one screen width and axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Widths {
    /// The name, at a root. A child's name gives up [`NEST`] cells per level
    /// so the columns after it stay aligned with its parent's.
    pub name: usize,
    /// Vendor, model and effort. Zero unless the vendor column is toggled on,
    /// since most walls run one vendor and model.
    pub vendor: usize,
    /// The state word. Zero on the state axis, where the heading says it.
    pub state: usize,
    /// What the agent is doing. The only column that gives way: it absorbs
    /// the vendor and state columns, so name and age never move.
    pub summary: usize,
    /// How long the agent has worked.
    pub age: usize,
    /// Levels of nesting every root is padded by; the summary pays for them.
    pub depth: usize,
}

/// The columns of a row at this width and axis, with or without the vendor
/// column, and every root padded by `depth` levels of [`NEST`] cells.
pub(super) fn widths(width: usize, axis: Axis, vendor: bool, depth: usize) -> Widths {
    let name = match width >= WIDE {
        true => WIDE_NAME,
        false => NARROW_NAME,
    };
    let state = match axis {
        Axis::State => 0,
        Axis::Project | Axis::Repo => STATE,
    };
    let vendor = match vendor {
        true => VENDOR,
        false => 0,
    };
    // The vendor and state columns sit between name and summary, each with
    // its own gap, and cost nothing when absent.
    let inserted = [vendor, state]
        .into_iter()
        .filter(|column| *column > 0)
        .map(|column| column + GAP)
        .sum::<usize>();
    let spent = PREFIX + nest(depth) + name + GAP + inserted + GAP + AGE;
    Widths {
        name,
        vendor,
        state,
        summary: width.saturating_sub(spent),
        age: AGE,
        depth,
    }
}

/// Cells one level of nesting adds before the glyph: `├─`, `└─` or `│ `.
pub(super) const NEST: usize = 2;

/// Cells a root spends on `depth` levels of padding.
pub(super) fn nest(depth: usize) -> usize {
    NEST * depth
}

/// Cells a path heading can spend on the path itself, given the `suffix`
/// that follows it.
///
/// The width less a space, the suffix and its space, [`SHORTEST_RULE`], and
/// the count in the age column.
pub(super) fn path_room(width: usize, suffix: &str) -> usize {
    let said = match suffix.is_empty() {
        true => 0,
        false => width_of(suffix) + 1,
    };
    width.saturating_sub(1 + said + SHORTEST_RULE + GAP + AGE)
}

/// Left-aligns `text` in `width` cells, padding with spaces or cutting with
/// an ellipsis.
pub(super) fn pad(text: &str, width: usize) -> String {
    let shown = match width_of(text) > width {
        true => cut(text, width),
        false => text.to_string(),
    };
    let short = " ".repeat(width.saturating_sub(width_of(&shown)));
    format!("{shown}{short}")
}

/// Right-aligns `text` in `width` cells, for numbers that line up. Too long a
/// text is cut without an ellipsis.
pub(super) fn padl(text: &str, width: usize) -> String {
    let shown = match width_of(text) > width {
        true => head(text, width),
        false => text.to_string(),
    };
    let short = " ".repeat(width.saturating_sub(width_of(&shown)));
    format!("{short}{shown}")
}

/// Fits `path` into `room` cells by dropping middle segments: the first
/// segment, `…`, then as much of the tail as fits, down to the last two.
///
/// The end is always kept: a worktree is named by its last segments, and
/// cutting them would make every worktree of a project read the same. If even
/// that does not fit, the result is `…` and the last cells of the path.
pub(super) fn elide(path: &str, room: usize) -> String {
    if width_of(path) <= room {
        return path.to_string();
    }
    // The leading `/` is not a segment. A `~` is the first segment, so it is
    // kept.
    let (root, rest) = match path.strip_prefix('/') {
        Some(rest) => ("/", rest),
        None => ("", path),
    };
    let mut segments: Vec<&str> = rest.split('/').collect();
    while segments.len() > 3 {
        segments.remove(1);
        let shown = format!("{root}{}/…/{}", segments[0], segments[1..].join("/"));
        if width_of(&shown) <= room {
            return shown;
        }
    }
    match room {
        0 => String::new(),
        room => format!("…{}", tail(path, room - 1)),
    }
}

/// Display width of `text` in cells, which differs from its char count for
/// wide characters. Uses ratatui's measure so fitting and drawing agree.
fn width_of(text: &str) -> usize {
    Span::raw(text).width()
}

/// The longest prefix of `text` that fits in `width` cells.
fn head(text: &str, width: usize) -> String {
    let mut kept = String::new();
    let mut used = 0;
    for one in text.chars() {
        let wide = width_of(one.encode_utf8(&mut [0; 4]));
        if used + wide > width {
            break;
        }
        used += wide;
        kept.push(one);
    }
    kept
}

/// The longest suffix of `text` that fits in `width` cells.
fn tail(text: &str, width: usize) -> String {
    let mut kept = String::new();
    let mut used = 0;
    for one in text.chars().rev() {
        let wide = width_of(one.encode_utf8(&mut [0; 4]));
        if used + wide > width {
            break;
        }
        used += wide;
        kept.insert(0, one);
    }
    kept
}

/// `text` cut to `width` cells, ending in `…`.
fn cut(text: &str, width: usize) -> String {
    match width {
        0 => String::new(),
        width => format!("{}…", head(text, width - 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cells a row spends on everything but the summary.
    fn spent(widths: Widths) -> usize {
        let inserted: usize = [widths.vendor, widths.state]
            .into_iter()
            .filter(|column| *column > 0)
            .map(|column| column + GAP)
            .sum();
        PREFIX + nest(widths.depth) + widths.name + GAP + inserted + GAP + widths.age
    }

    #[test]
    fn name_column_is_22_cells_at_100_and_wider() {
        for width in [100, 120, 200] {
            assert_eq!(
                widths(width, Axis::State, false, 0).name,
                22,
                "a {width}-cell screen has room for the wide name column"
            );
        }
    }

    #[test]
    fn name_column_drops_to_16_cells_below_100() {
        for width in [80, 99] {
            assert_eq!(
                widths(width, Axis::State, false, 0).name,
                16,
                "a {width}-cell screen does not"
            );
        }
    }

    #[test]
    fn state_word_is_8_cells_on_the_dir_axis_and_nothing_on_the_state_axis() {
        assert_eq!(
            widths(100, Axis::Project, false, 0).state,
            8,
            "which is what `starting` needs"
        );
        assert_eq!(
            widths(100, Axis::State, false, 0).state,
            0,
            "the heading over the row says it there"
        );
    }

    #[test]
    fn state_word_keeps_its_8_cells_on_a_narrow_screen() {
        assert_eq!(
            widths(80, Axis::Project, false, 0).state,
            8,
            "a cut state word would be a lie"
        );
    }

    #[test]
    fn vendor_column_is_18_cells_when_it_is_asked_for_and_nothing_when_it_is_not() {
        assert_eq!(
            widths(100, Axis::State, true, 0).vendor,
            18,
            "which is what `claude sonnet high` needs"
        );
        assert_eq!(
            widths(100, Axis::State, false, 0).vendor,
            0,
            "and a column nobody asked for costs the row nothing"
        );
    }

    #[test]
    fn vendor_column_keeps_its_18_cells_on_a_narrow_screen() {
        assert_eq!(
            widths(80, Axis::Project, true, 0).vendor,
            18,
            "a key somebody pressed is answered at whatever width they pressed it"
        );
    }

    #[test]
    fn summary_pays_for_the_vendor_column_and_nothing_else_moves() {
        for width in [80, 100, 160] {
            for axis in [Axis::State, Axis::Project] {
                let off = widths(width, axis, false, 0);
                let on = widths(width, axis, true, 0);
                assert_eq!(off.name, on.name, "the name column does not move");
                assert_eq!(off.age, on.age, "nor does the age column");
                assert_eq!(off.state, on.state, "nor does the state word");
                assert_eq!(
                    off.summary - on.summary,
                    VENDOR + GAP,
                    "the summary absorbs the whole 20 cells at {width} on {axis:?}"
                );
            }
        }
    }

    #[test]
    fn age_column_is_4_cells_on_either_axis() {
        assert_eq!(widths(100, Axis::State, false, 0).age, 4);
        assert_eq!(widths(80, Axis::Project, false, 0).age, 4);
    }

    #[test]
    fn nesting_takes_its_cells_out_of_the_summary_and_nothing_else_moves() {
        for depth in 0..=3 {
            let flat = widths(100, Axis::State, false, 0);
            let deep = widths(100, Axis::State, false, depth);
            assert_eq!(deep.name, flat.name, "the name column does not move");
            assert_eq!(deep.age, flat.age, "nor does the age column");
            assert_eq!(
                flat.summary - deep.summary,
                nest(depth),
                "the summary pays for {depth} levels of nesting"
            );
            assert_eq!(deep.depth, depth, "and the rows are told how deep");
        }
    }

    #[test]
    fn summary_pays_for_the_state_word_and_nothing_else_moves() {
        for width in [80, 100, 160] {
            let state = widths(width, Axis::State, false, 0);
            let dir = widths(width, Axis::Project, false, 0);
            assert_eq!(state.name, dir.name, "the name column does not move");
            assert_eq!(state.age, dir.age, "nor does the age column");
            assert_eq!(
                state.summary - dir.summary,
                STATE + GAP,
                "the summary absorbs the whole 10 cells at {width}"
            );
        }
    }

    #[test]
    fn columns_fill_the_screen_they_are_given() {
        for width in 80..=200 {
            for axis in [Axis::State, Axis::Project] {
                for vendor in [false, true] {
                    let widths = widths(width, axis, vendor, 0);
                    assert_eq!(
                        spent(widths) + widths.summary,
                        width,
                        "{axis:?} at {width} leaves no cell unspoken for"
                    );
                }
            }
        }
    }

    #[test]
    fn summary_stops_at_nothing_below_the_designs_floor() {
        assert_eq!(
            widths(30, Axis::Project, false, 0).summary,
            0,
            "the summary is the first column to go and the last to be missed"
        );
    }

    #[test]
    fn pad_fills_a_short_name_out_to_its_column() {
        assert_eq!(pad("api", 6), "api   ");
        assert_eq!(pad("", 3), "   ");
    }

    #[test]
    fn pad_cuts_a_long_name_with_an_ellipsis() {
        assert_eq!(pad("fix-the-login-bug", 8), "fix-the…");
        assert_eq!(
            pad("fix-the-login-bug", 8).chars().count(),
            8,
            "and still fills the column"
        );
    }

    #[test]
    fn padl_puts_an_age_at_the_right_of_its_column() {
        assert_eq!(padl("2m", 4), "  2m");
        assert_eq!(padl("365d", 4), "365d");
    }

    #[test]
    fn elide_leaves_a_path_that_already_fits() {
        assert_eq!(elide("~/src/amx", 20), "~/src/amx");
    }

    #[test]
    fn elide_takes_the_middle_out_of_a_long_path() {
        assert_eq!(
            elide("/home/dev/src/github/amx/worktrees/t1", 30),
            "/home/…/amx/worktrees/t1",
            "the first segment stays, and the tail stays whole"
        );
    }

    #[test]
    fn elide_eats_the_middle_one_segment_at_a_time() {
        assert_eq!(
            elide("/home/dev/src/github/amx/worktrees/t1", 20),
            "/home/…/worktrees/t1",
            "down to the first segment and the last two"
        );
    }

    #[test]
    fn elide_keeps_the_tilde_a_path_was_shortened_to() {
        assert_eq!(
            elide("~/src/github/amx/worktrees/t1", 22),
            "~/…/amx/worktrees/t1",
            "a home-relative path is not turned into an absolute one"
        );
    }

    #[test]
    fn elide_never_cuts_the_end_of_a_path() {
        let path = "/home/dev/src/github/amx/worktrees/t1";
        for room in 0..=40 {
            let shown = elide(path, room);
            let kept = match shown.rfind('…') {
                Some(mark) => &shown[mark + '…'.len_utf8()..],
                None => shown.as_str(),
            };
            assert!(
                path.ends_with(kept),
                "at {room} cells `{shown}` still ends where the path ends"
            );
        }
    }

    #[test]
    fn elide_falls_back_to_the_tail_when_even_that_will_not_fit() {
        assert_eq!(
            elide("/home/dev/src/github/amx/worktrees/t1", 10),
            "…ktrees/t1",
            "the last cells of the path, and a mark saying what went"
        );
    }

    #[test]
    fn elide_fits_the_room_it_is_given() {
        let path = "/home/dev/src/github/amx/worktrees/t1";
        for room in 0..=40 {
            assert!(
                elide(path, room).chars().count() <= room,
                "{room} cells is what the heading has"
            );
        }
    }

    #[test]
    fn path_room_leaves_the_heading_its_rule_and_its_count() {
        let room = path_room(100, "");
        assert_eq!(
            room + 1 + SHORTEST_RULE + GAP + AGE,
            100,
            "the path, a space, the shortest rule, and the count"
        );
        assert_eq!(
            path_room(100, "· 2 failed"),
            room - 11,
            "a suffix costs the path its own width and the space before it"
        );
    }
}
