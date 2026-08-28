//! The screen of keys, for whoever asked what they are.
//!
//! Not a band: it stands where the list stands, because a person who has asked
//! what the keys are is not reading the wall. The table of every key is here,
//! and beside it how that table is stood in columns on a terminal of any shape,
//! and what it gives up first when there is not room for all of it.
//!
//! It is drawn as the wall it replaces. A group of keys carries the heading a
//! group of agents carries — the label uppercase and bold, a dim rule run out
//! to the number under it — so the overlay reads as the same screen showing
//! something else rather than as a manual somebody opened. What the two bands
//! above it say does not change, and the row under it already says how to get
//! back, so neither is said again here.
//!
//! What it does say for itself is the page it is on. A screen too short for
//! every key gives up none of them: they are paged, and the foot of each page
//! names the key that turns it, because a key nobody can reach is a key the
//! screen may as well not list.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::cell::Cell;
use std::ops::Range;

use super::style::{bold, dim};
use super::text::{fit, said, width_of};
use crate::tui::grid;

/// Every key, for whoever asked what they are.
///
/// Every key the view binds, and the words it is bound under: a key column
/// that names two keys names both, because what a person looks for here is
/// the one they pressed. A test presses everything a terminal can send and
/// holds what acted against this table, so a binding that is not here is a
/// binding the screen would have to grow a row for.
///
/// In the order [`GROUPS`] stands them in, which is the order they are drawn:
/// one table, cut into runs, so a key is in exactly one place and the test that
/// walks every key walks every group with it.
///
/// The table is not public to the rest of the crate, so the test that checks
/// the README against it reads this file as text.
pub(in crate::tui) const HELP: [(&str, &str); 29] = [
    // walk
    ("↑ ↓", "walk the agents"),
    ("alt+1..9", "reach one by where it is on the wall"),
    ("esc", "put the card away · leave a line alone"),
    ("?", "these keys"),
    ("q ctrl+c", "close the view"),
    // look
    ("space", "the card: what one is asking, and the answer"),
    ("enter →", "bring its window forward · shut a group"),
    ("d", "what it has changed"),
    ("pgup ctrl+b", "page the card, when it holds more"),
    ("pgdn ctrl+f", "and the other way"),
    // start
    ("n", "start an agent"),
    ("alt+n", "start the line and go to the agent"),
    ("r", "reply: a message, or an answer on the card"),
    ("alt+enter", "a newline in the line, without sending it"),
    ("ctrl+g", "write the line in $EDITOR"),
    // arrange
    ("ctrl+s", "gather them by state or by project"),
    ("ctrl+t", "hold it at the top of its group"),
    ("shift+↑", "move it up its group"),
    ("shift+↓", "move it down its group"),
    ("ctrl+r", "call it something else"),
    ("ctrl+x", "stop it · again forgets · a heading, the group"),
    ("s: a:", "narrow by state or name, on the task line"),
    // dials
    ("alt+v", "which vendor the next agent runs"),
    ("alt+m", "which model the next agent is given"),
    ("alt+w", "whether it gets a worktree of its own"),
    ("shift+tab", "what it may do without asking"),
    ("m: p: w:", "model, permission and worktree, for one spawn"),
    ("d:", "where one spawn runs, on the task line"),
    ("agent:", "which vendor runs it, for one spawn"),
];

/// What the keys are for, and how many of [`HELP`] each of those answers for.
///
/// A flat list of twenty-nine is a list somebody reads all of to find one, so
/// the table is cut into what a person is trying to do: get about the wall,
/// read one agent, put work in, arrange what is already there, and set what
/// the next agent runs. Five short lists are five places to not look.
///
/// Runs rather than tables of their own, so nothing here can hold a key twice
/// or drop one between two headings.
pub(super) const GROUPS: [(&str, usize); 5] = [
    ("walk", 5),
    ("look", 5),
    ("start", 5),
    ("arrange", 7),
    ("dials", 7),
];

/// Every key stands under exactly one heading.
const _: () = {
    let (mut under, mut at) = (0, 0);
    while at < GROUPS.len() {
        under += GROUPS[at].1;
        at += 1;
    }
    assert!(under == HELP.len());
};

/// The width from which the groups stand in two columns.
///
/// The same width the rows change shape at, and for the same reason: below it
/// there is not room for two of anything. Two columns here are a key column
/// and a stub of a description against it, and the half a person came for is
/// the half that goes — so the second column is given up whole rather than
/// squeezed, and the one that is left says what every key does in full.
const WIDE: usize = 100;

/// The key column, sized for the widest pair of keys a row names and the space
/// that holds the description off it.
const KEY: usize = 12;

/// What a key is indented by, so it reads as standing under its heading rather
/// than beside it.
const INDENT: usize = 2;

/// The column a group's count is right-aligned in, and what stands between it
/// and the rule that runs out to it.
const COUNT: usize = 2;
const GAP: usize = 2;

/// Every key and what it does, under the heading that says what it is for.
///
/// Down before across, for the reason a list is a column. Somebody looking for
/// one key reads the heading, runs their eye down the keys under it and on to
/// the next; a table filled the other way would put the second key beside the
/// first and the rest of them anywhere at all.
///
/// Which page of them is drawn is the view's to hold, because it is where
/// somebody left off reading rather than a fact about the screen. The clamp is
/// here: only the paint knows how many pages a screen this shape made of them,
/// so the key that turns them only adds and subtracts.
pub(super) fn help(frame: &mut Frame, area: Rect, page: &Cell<usize>) {
    let width = (area.width as usize).max(1);
    let dealt = dealt(width);
    let share = width / dealt.len();
    let columns: Vec<Vec<Vec<Span<'static>>>> = dealt
        .iter()
        .enumerate()
        .map(|(n, groups)| column(groups.clone(), room(width, share, n, dealt.len())))
        .collect();
    let deep = columns.iter().map(Vec::len).max().unwrap_or(0);

    // The rows the keys themselves have: the screen's, less the one the foot
    // takes to say there are more of them.
    let height = area.height as usize;
    let rows = match deep > height {
        true => height.saturating_sub(1),
        false => height,
    }
    .max(1);
    let pages = deep.div_ceil(rows);
    let at = page.get().min(pages.saturating_sub(1));
    page.set(at);

    let from = at * rows;
    let mut lines: Vec<Line> = (from..(from + rows).min(deep))
        .map(|row| line(&columns, row, share))
        .collect();
    if pages > 1 {
        lines.push(foot(at, pages));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// The foot of a screen that could not hold them all: which page of them this
/// is, and the keys that turn it.
///
/// A page that did not say there was another would be answering half of what
/// somebody asked, and the keys are what they came for. Only the ones that do
/// something are named: there is nothing above the first page and nothing
/// below the last.
fn foot(at: usize, pages: usize) -> Line<'static> {
    let turns = match (at > 0, at + 1 < pages) {
        (true, true) => "pgup pgdn",
        (true, false) => "pgup",
        _ => "pgdn",
    };
    Line::styled(format!(" page {} of {pages} · {turns}", at + 1), dim())
}

/// Which groups each column holds: either side of the cut that leaves the two
/// of them nearest the same number of keys, or all five in one column on a
/// screen too narrow for that.
///
/// Two rather than a column each, because five short lists side by side is a
/// wall of keys again and one column of thirty-eight rows is the flat list the
/// headings were put in to break up. Two columns is what a page of keys looks
/// like.
///
/// A group is what the eye follows down, so it is never cut in half to make
/// the columns even.
fn dealt(width: usize) -> Vec<Range<usize>> {
    // A narrow screen is the cut falling off the end of the table: everything
    // stands in the first column and the second one comes out empty, which is
    // a column the screen does not have.
    let cut = match width >= WIDE {
        true => cut(),
        false => GROUPS.len(),
    };
    [0..cut, cut..GROUPS.len()]
        .into_iter()
        .filter(|column| !column.is_empty())
        .collect()
}

/// Where those two columns part, which for the table as it stands is after
/// `start`: fifteen keys against fourteen.
fn cut() -> usize {
    let total: usize = GROUPS.iter().map(|(_, under)| under).sum();
    (1..GROUPS.len())
        .min_by_key(|at| {
            let left: usize = GROUPS[..*at].iter().map(|(_, under)| under).sum();
            left.abs_diff(total - left)
        })
        .unwrap_or(1)
}

/// How many cells one column has to say a key and what it does in.
///
/// The last takes whatever the division left over and runs to the edge of the
/// screen; every other one keeps a column of air between it and the next.
fn room(width: usize, share: usize, n: usize, columns: usize) -> usize {
    match n + 1 == columns {
        true => width.saturating_sub(n * share),
        false => share.saturating_sub(1),
    }
}

/// One column: its groups in order, each headed and each standing off from the
/// one before it. An empty row is that space rather than a key.
fn column(groups: Range<usize>, room: usize) -> Vec<Vec<Span<'static>>> {
    let does = room.saturating_sub(INDENT + KEY);
    let mut told = Vec::new();
    for group in groups {
        if !told.is_empty() {
            told.push(Vec::new());
        }
        told.push(heading(group, room));
        told.extend(under(group).map(|(key, said)| {
            vec![
                Span::raw(" ".repeat(INDENT)),
                Span::styled(grid::pad(key, KEY), bold()),
                Span::styled(fit(said, does), dim()),
            ]
        }));
    }
    told
}

/// A heading over a run of keys: what they are for, a rule, and how many of
/// them there are at the column's own right edge.
///
/// The shape a group of agents wears on the wall, so a person who has learned
/// to read one heading has learned to read the other.
fn heading(group: usize, room: usize) -> Vec<Span<'static>> {
    let (label, under) = GROUPS[group];
    let label = label.to_uppercase();
    // What the rule is left: the space in front of the label, the label, the
    // space after it, and the gap and the count at the far end.
    let spent = 1 + width_of(&label) + 1 + GAP + COUNT;
    vec![
        Span::styled(format!(" {label} "), bold()),
        Span::styled("─".repeat(room.saturating_sub(spent).max(1)), dim()),
        Span::raw(" ".repeat(GAP)),
        Span::styled(grid::padl(&under.to_string(), COUNT), dim()),
    ]
}

/// The keys one group stands over, which is its run of [`HELP`].
fn under(group: usize) -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    let from: usize = GROUPS[..group].iter().map(|(_, under)| under).sum();
    HELP[from..from + GROUPS[group].1].iter()
}

/// One row of the overlay, which is one row of each column stood side by side.
///
/// A column that has run out of keys, or is standing one group off from the
/// next, leaves the ones beside it where they were: the columns are what the
/// eye follows down.
fn line(columns: &[Vec<Vec<Span<'static>>>], at: usize, share: usize) -> Line<'static> {
    let mut spans = Vec::new();
    let mut column = 0;
    for (n, told) in columns.iter().enumerate() {
        let told = match told.get(at) {
            Some(told) if !told.is_empty() => told,
            _ => continue,
        };
        if n * share > column {
            spans.push(Span::raw(" ".repeat(n * share - column)));
        }
        column = n * share + said(told);
        spans.extend(told.iter().cloned());
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::View;
    use crate::tui::paint::header::{header_rows, space_rows};
    use crate::tui::paint::{Card, draw};
    use crate::tui::{Mode, Screen};
    use crossterm::event::{KeyCode, KeyEvent};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Modifier;

    /// The view, with a reading in it. The card is read as it is planted,
    /// the way the view itself builds one.
    fn showing(views: Vec<View>, card: Option<Card>) -> Screen {
        let mut screen = Screen::default();
        screen.list.show(views);
        screen.card = card.map(Card::read);
        screen
    }

    /// What a view of this size draws, cell by cell.
    fn cells(screen: &Screen, size: (u16, u16)) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
        terminal.draw(|frame| draw(frame, screen)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// What a view of this size puts on the screen, line by line.
    fn painted(screen: &Screen, size: (u16, u16)) -> Vec<String> {
        let buffer = cells(screen, size);
        (0..size.1)
            .map(|row| {
                (0..size.0)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    /// The overlay on a screen this size, and the rows it was drawn on.
    fn overlay(size: (u16, u16)) -> Vec<String> {
        let mut screen = showing(Vec::new(), None);
        screen.mode = Mode::Keys;
        painted(&screen, size)
    }

    /// The cells between two columns of a drawn screen, as their own lines:
    /// what one band of the overlay says, with whatever stands beside it cut
    /// away.
    fn between(painted: &[String], from: usize, to: usize) -> String {
        painted
            .iter()
            .map(|line| line.chars().skip(from).take(to - from).collect::<String>())
            .collect::<Vec<String>>()
            .join("\n")
    }

    /// A screen wide enough for the two columns and tall enough for all of
    /// them, which is the shape the overlay is drawn for.
    const WIDE_SCREEN: (u16, u16) = (120, 40);

    /// The screen most people have: too narrow for two columns and far too
    /// short for one of thirty-eight rows, which is the shape the paging is
    /// for. It is what a terminal opens at.
    const SHORT_SCREEN: (u16, u16) = (80, 24);

    #[test]
    fn keymap_reaches_every_key_by_paging_a_screen_too_short_to_hold_them() {
        let mut screen = showing(Vec::new(), None);
        screen.mode = Mode::Keys;

        // Every page pgdn reaches, in order. The last one is where the screen
        // stops changing: the paint holds the page at the last one it made, so
        // a press past the end lands where the press before it did.
        let mut pages: Vec<String> = Vec::new();
        for _ in 0..HELP.len() {
            let drawn = painted(&screen, SHORT_SCREEN).join("\n");
            if pages.last() == Some(&drawn) {
                break;
            }
            pages.push(drawn);
            let _ = screen.reading_the_keys(KeyEvent::from(KeyCode::PageDown));
        }
        assert!(
            pages.len() > 1,
            "a screen this short does not hold them all at once:\n{}",
            pages.join("\n")
        );
        assert!(
            matches!(screen.mode, Mode::Keys),
            "and paging is not the key that puts the agents back"
        );

        // Every key is on one of those pages, and what it does with it.
        let paged = pages.join("\n");
        for (key, does) in HELP {
            assert!(
                paged.contains(key),
                "{key} is on none of the pages:\n{paged}"
            );
            assert!(
                paged.contains(does),
                "{does} is on none of the pages:\n{paged}"
            );
        }

        // And the first of them says there are more and which key brings them:
        // a page nobody knows to turn is a page holding keys nobody finds.
        assert!(
            pages[0].contains("page 1 of") && pages[0].contains("pgdn"),
            "the first page says there is another and how to turn to it:\n{}",
            pages[0]
        );
    }

    #[test]
    fn keymap_holds_the_last_page_so_the_way_back_is_the_one_press() {
        let mut screen = showing(Vec::new(), None);
        screen.mode = Mode::Keys;

        // Somebody leaning on the key, which is what pressing past the end is.
        // The paint holds the page at the last one it made, so the presses that
        // went nowhere are not presses to be taken back.
        for _ in 0..HELP.len() {
            let _ = screen.reading_the_keys(KeyEvent::from(KeyCode::PageDown));
            let _ = painted(&screen, SHORT_SCREEN);
        }
        let last = painted(&screen, SHORT_SCREEN).join("\n");

        let _ = screen.reading_the_keys(KeyEvent::from(KeyCode::PageUp));
        assert_ne!(
            painted(&screen, SHORT_SCREEN).join("\n"),
            last,
            "one press off the last page is a page back:\n{last}"
        );
    }

    #[test]
    fn keymap_stands_the_keys_in_two_columns_of_fifteen_and_fourteen() {
        let painted = overlay(WIDE_SCREEN);
        let share = WIDE_SCREEN.0 as usize / 2;
        let left = between(&painted, 0, share);
        let right = between(&painted, share, WIDE_SCREEN.0 as usize);

        // Cut where the two columns come out nearest the same number of keys,
        // which for this table is after `start`: fifteen against fourteen.
        let cut: usize = GROUPS[..3].iter().map(|(_, under)| under).sum();
        assert_eq!(
            cut, 15,
            "the groups are not the runs this test is written for"
        );
        for (key, does) in &HELP[..cut] {
            assert!(
                left.contains(does),
                "{key} is not down the first column:\n{left}"
            );
            assert!(
                !right.contains(does),
                "{key} is down both of them:\n{right}"
            );
        }
        for (key, does) in &HELP[cut..] {
            assert!(
                right.contains(does),
                "{key} is not down the second column:\n{right}"
            );
            assert!(!left.contains(does), "{key} is down both of them:\n{left}");
        }

        // And every one of them whole: a screen this wide has room for the
        // longest thing a key does, so nothing on it is cut short.
        let all = painted.join("\n");
        assert!(
            !all.contains('…'),
            "nothing is elided at this width:\n{all}"
        );
        for (key, _) in HELP {
            assert!(all.contains(key), "{key} is missing:\n{all}");
        }
    }

    #[test]
    fn keymap_heads_each_column_the_way_the_wall_heads_a_group() {
        let painted = overlay(WIDE_SCREEN);
        let share = WIDE_SCREEN.0 as usize / 2;

        // The heading a group of agents carries: the label uppercase, a rule
        // run out from it, and how many stand under it at the column's own
        // right edge.
        let first = between(&painted, 0, share);
        let heading = first.lines().nth(3).expect("the first heading").to_string();
        assert!(heading.starts_with(" WALK ─"), "{heading:?}");
        assert!(heading.trim_end().ends_with('5'), "{heading:?}");

        let second = between(&painted, share, WIDE_SCREEN.0 as usize);
        let beside = second
            .lines()
            .nth(3)
            .expect("the heading beside it")
            .to_string();
        assert!(beside.starts_with(" ARRANGE ─"), "{beside:?}");
        assert!(beside.trim_end().ends_with('7'), "{beside:?}");

        // A group stands off from the one under it rather than running into
        // it, and the keys are indented under their own heading.
        assert!(
            painted[9].chars().take(share).all(char::is_whitespace),
            "one group stands off from the next: {:?}",
            painted[9]
        );
        assert!(painted[10].starts_with(" LOOK ─"), "{:?}", painted[10]);
        assert!(painted[4].starts_with("  "), "{:?}", painted[4]);
    }

    #[test]
    fn keymap_gives_up_the_second_column_whole_rather_than_squeezing_it() {
        // Below the width the rows themselves change shape at there is no room
        // for two of anything: two columns here would be a key with a stub of
        // a description against it, and the description is the half somebody
        // came for.
        let painted = overlay((80, 46));
        let all = painted.join("\n");
        for (key, does) in HELP {
            assert!(all.contains(key), "{key} is missing:\n{all}");
            assert!(all.contains(does), "{does} is missing:\n{all}");
        }
        assert!(!all.contains('…'), "and none of it is cut short:\n{all}");
        for line in &painted {
            assert!(line.chars().count() <= 80, "{line:?}");
        }

        // One column: every heading is against the left edge, and the last
        // group is under the first rather than beside it.
        for (label, _) in GROUPS {
            let heading = format!(" {} ─", label.to_uppercase());
            assert!(
                painted.iter().any(|line| line.starts_with(&heading)),
                "{heading:?} is not at the edge:\n{all}"
            );
        }
    }

    #[test]
    fn view_lists_every_key_when_somebody_asks_for_them() {
        let mut screen = showing(Vec::new(), None);
        screen.mode = Mode::Keys;

        // Tall and wide enough for every key and every heading over them,
        // so each of them has the row to itself and every description is
        // whole.
        let tall = (HELP.len() + GROUPS.len()) as u16 + header_rows(24) + space_rows(24) + 1;
        let painted = painted(&screen, (140, tall)).join("\n");
        for (key, does) in HELP {
            assert!(painted.contains(key), "{key} is missing:\n{painted}");
            assert!(painted.contains(does), "{does} is missing:\n{painted}");
        }
    }

    #[test]
    fn keymap_carries_the_weight_on_the_label_and_the_key_and_none_of_it_elsewhere() {
        let mut screen = showing(Vec::new(), None);
        screen.mode = Mode::Keys;
        let buffer = cells(&screen, WIDE_SCREEN);

        let label = buffer[(1, 3)].clone();
        assert!(
            label.modifier.contains(Modifier::BOLD),
            "a heading is what makes a group out of a run of keys: {:?}",
            label.modifier
        );
        let rule = buffer[(7, 3)].clone();
        assert_eq!(rule.symbol(), "─", "the rule runs out to the count");
        assert!(
            rule.modifier.contains(Modifier::DIM),
            "and carries none of the weight: {:?}",
            rule.modifier
        );

        let key = buffer[(INDENT as u16, 4)].clone();
        assert!(
            key.modifier.contains(Modifier::BOLD),
            "the key itself is what somebody came here to find: {:?}",
            key.modifier
        );
        let does = buffer[((INDENT + KEY) as u16, 4)].clone();
        assert!(
            does.modifier.contains(Modifier::DIM),
            "and what it does stands behind it: {:?}",
            does.modifier
        );
    }
}
