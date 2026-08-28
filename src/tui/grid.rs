//! The character grid the view is drawn on.
//!
//! Every screen in the design is an integer number of cells wide, and the
//! columns a row is cut into are the same columns the heading over it ends in.
//! That arithmetic lives here rather than beside the paint, so a row and a
//! heading cannot drift apart and a test of the geometry does not have to
//! stand up a terminal to read it.
//!
//! Nothing here draws. It answers how wide each column is, fills text to a
//! column, and takes the middle out of a path that will not fit.

// The surfaces that spend these budgets are drawn in `paint`, which takes them
// up next. Until it does, the module's only caller is its own tests.
#![allow(dead_code)]

use ratatui::text::Span;

use super::rows::Axis;

/// The width from which a screen is a wide one.
const WIDE: usize = 100;

/// The name column: 22 cells on a wide screen and 16 below it. Twenty-two
/// rather than the twenty-six a long name can want, so that both axes can
/// share the column and switching axis moves one boundary rather than the
/// whole table.
const WIDE_NAME: usize = 22;
const NARROW_NAME: usize = 16;

/// The state word's column on the dir axis, sized for `starting`, which is the
/// longest of them. It does not shrink with the screen: a state word cut short
/// would be a lie.
const STATE: usize = 8;

/// The age column, which fits everything up to `365d`.
const AGE: usize = 4;

/// What stands before the name: the unread or pin mark, a space, the state
/// glyph, and a space.
const PREFIX: usize = 4;

/// What stands between two columns.
const GAP: usize = 2;

/// The fewest cells a heading's rule is allowed, which is what stops a long
/// path from leaving a heading with no rule to read it as one.
const SHORTEST_RULE: usize = 8;

/// How wide each column of an agent's row is, on a screen this wide and an
/// axis gathered this way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Widths {
    /// What the agent is called.
    pub name: usize,
    /// What state it is in, in a word. Nothing on the state axis, where the
    /// heading over the row already says it and saying it twice would be a
    /// column of noise.
    pub state: usize,
    /// What it is up to. The column that gives way: it absorbs the whole cost
    /// of the state word, so the name, the age and the group's count sit where
    /// they sit on the other axis.
    pub summary: usize,
    /// How long it has worked.
    pub age: usize,
}

/// The columns a row is cut into at this width, on this axis.
pub(super) fn widths(width: usize, axis: Axis) -> Widths {
    let name = match width >= WIDE {
        true => WIDE_NAME,
        false => NARROW_NAME,
    };
    let state = match axis {
        Axis::State => 0,
        Axis::Project => STATE,
    };
    // What the dir axis inserts between the name and the summary: the state
    // word and the gap in front of it, or nothing at all.
    let inserted = match state {
        0 => 0,
        word => word + GAP,
    };
    let spent = PREFIX + name + GAP + inserted + GAP + AGE;
    Widths {
        name,
        state,
        summary: width.saturating_sub(spent),
        age: AGE,
    }
}

/// How many cells a path heading can spend on its path, with `suffix` being
/// whatever the heading says after the path and before its rule.
///
/// What is left of the screen once the space the path stands in, the space
/// after it, the suffix and its own space, the shortest rule a heading is
/// allowed and the count in the age column have been taken out of it.
pub(super) fn path_room(width: usize, suffix: &str) -> usize {
    let said = match suffix.is_empty() {
        true => 0,
        false => width_of(suffix) + 1,
    };
    width.saturating_sub(1 + 1 + said + SHORTEST_RULE + GAP + AGE)
}

/// `text` in a column `width` cells wide, filled with spaces, and cut with an
/// ellipsis where it is too long for the column.
pub(super) fn pad(text: &str, width: usize) -> String {
    let shown = match width_of(text) > width {
        true => cut(text, width),
        false => text.to_string(),
    };
    let short = " ".repeat(width.saturating_sub(width_of(&shown)));
    format!("{shown}{short}")
}

/// The same column with `text` at its right end, which is where a number that
/// has to line up with the numbers above it goes.
pub(super) fn padl(text: &str, width: usize) -> String {
    let shown = match width_of(text) > width {
        true => head(text, width),
        false => text.to_string(),
    };
    let short = " ".repeat(width.saturating_sub(width_of(&shown)));
    format!("{short}{shown}")
}

/// `path` in `room` cells, with the middle taken out of it where it does not
/// fit: the first segment, an ellipsis, and as much of the tail as there is
/// room for, down to the last two segments.
///
/// The end is never what goes. A worktree is identified by its last segment
/// and its parent, and a path cut at the right would leave every worktree of
/// one project reading the same.
pub(super) fn elide(path: &str, room: usize) -> String {
    if width_of(path) <= room {
        return path.to_string();
    }
    // The leading slash of an absolute path is not a segment and is not the
    // segment that gets eaten, and a path already shortened to `~` keeps the
    // `~` it was shortened to.
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
    // A path whose first segment and last two are already too long for the
    // heading. What is left to keep is the end of it.
    match room {
        0 => String::new(),
        room => format!("…{}", tail(path, room - 1)),
    }
}

/// The columns `text` takes on a screen, which is not its characters: an emoji
/// is one char and two columns, and a row measured in characters pushes its
/// last column off the terminal's edge. ratatui's own measure, so a row is cut
/// by the same arithmetic it is drawn with.
fn width_of(text: &str) -> usize {
    Span::raw(text).width()
}

/// As much of the front of `text` as fits in `width` columns.
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

/// And as much of the back of it.
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

/// `text` in `width` columns with an ellipsis standing for what did not fit.
fn cut(text: &str, width: usize) -> String {
    match width {
        0 => String::new(),
        width => format!("{}…", head(text, width - 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a row spends on everything except the summary, which is what the
    /// summary is left over from.
    fn spent(widths: Widths) -> usize {
        let inserted = match widths.state {
            0 => 0,
            state => state + GAP,
        };
        PREFIX + widths.name + GAP + inserted + GAP + widths.age
    }

    #[test]
    fn name_column_is_22_cells_at_100_and_wider() {
        for width in [100, 120, 200] {
            assert_eq!(
                widths(width, Axis::State).name,
                22,
                "a {width}-cell screen has room for the wide name column"
            );
        }
    }

    #[test]
    fn name_column_drops_to_16_cells_below_100() {
        for width in [80, 99] {
            assert_eq!(
                widths(width, Axis::State).name,
                16,
                "a {width}-cell screen does not"
            );
        }
    }

    #[test]
    fn state_word_is_8_cells_on_the_dir_axis_and_nothing_on_the_state_axis() {
        assert_eq!(
            widths(100, Axis::Project).state,
            8,
            "which is what `starting` needs"
        );
        assert_eq!(
            widths(100, Axis::State).state,
            0,
            "the heading over the row says it there"
        );
    }

    #[test]
    fn state_word_keeps_its_8_cells_on_a_narrow_screen() {
        assert_eq!(
            widths(80, Axis::Project).state,
            8,
            "a cut state word would be a lie"
        );
    }

    #[test]
    fn age_column_is_4_cells_on_either_axis() {
        assert_eq!(widths(100, Axis::State).age, 4);
        assert_eq!(widths(80, Axis::Project).age, 4);
    }

    #[test]
    fn summary_pays_for_the_state_word_and_nothing_else_moves() {
        for width in [80, 100, 160] {
            let state = widths(width, Axis::State);
            let dir = widths(width, Axis::Project);
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
                let widths = widths(width, axis);
                assert_eq!(
                    spent(widths) + widths.summary,
                    width,
                    "{axis:?} at {width} leaves no cell unspoken for"
                );
            }
        }
    }

    #[test]
    fn summary_stops_at_nothing_below_the_designs_floor() {
        assert_eq!(
            widths(30, Axis::Project).summary,
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
            room + 1 + 1 + SHORTEST_RULE + GAP + AGE,
            100,
            "a space, the path, a space, the shortest rule, and the count"
        );
        assert_eq!(
            path_room(100, "· 2 failed"),
            room - 11,
            "a suffix costs the path its own width and the space before it"
        );
    }
}
