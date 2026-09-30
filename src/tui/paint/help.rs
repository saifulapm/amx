//! The keys overlay, drawn in the list's band.
//!
//! Holds the table of every key amx binds. Groups of keys wear the same
//! heading style as groups of agents. The overlay is one column, scrolled
//! with the list's movement keys, with a status row at the top saying which
//! keys are on screen out of how many. `/` narrows the table by key spelling
//! or description as it is typed.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::cell::Cell;

use super::style::{bold, dim};
use super::text::{RULE, fit, width_of};
use crate::tui::grid;
use crate::tui::keyname::Bound;

/// Every key the view binds and what it does.
///
/// A test presses every key a terminal can send and checks that whatever
/// acted is listed here. Ordered by [`GROUPS`], which cuts it into runs, so
/// each key is in exactly one group. The README test reads this file as text,
/// since the table is not visible outside the crate's `tui` module.
pub(in crate::tui) const HELP: [(&str, &str); 55] = [
    // walk
    ("↑ ↓ j k", "walk the agents"),
    ("gg G", "top and bottom of the list"),
    ("alt+1..9", "the agent at that position on the wall"),
    ("w", "the first agent that needs you"),
    ("backspace", "the agent you were last in"),
    ("esc", "close the card · leave the line"),
    ("?", "these keys"),
    ("q ctrl+c", "close the view"),
    // look
    ("space", "open the card, to answer or send a message"),
    ("v", "which vendor, model and effort each one runs"),
    ("enter → l", "go into the agent · open or shut a group"),
    ("d", "what it has changed"),
    ("o", "open its pull request in the browser"),
    ("alt+d", "the patch in the viewer the diff key sets"),
    ("pgup ctrl+b", "page the card up"),
    ("pgdn ctrl+f", "page the card down"),
    ("ctrl+u", "half a page up"),
    ("ctrl+d", "half a page down"),
    ("ctrl+n", "the next hunk of the patch"),
    ("ctrl+p", "the previous hunk, or the top"),
    // start
    ("n", "start an agent"),
    ("alt+n", "start it and go into it"),
    ("f", "fork the agent onto a task"),
    ("!", "run the line as a command, not a task"),
    ("shift+enter", "a newline, without sending the line"),
    ("alt+enter", "a newline, if shift+enter does not arrive"),
    ("ctrl+j", "a newline, if neither of those arrives"),
    ("tab", "the word offered · on an empty line, @ offers"),
    ("← → ctrl+←", "move by character or word · home end"),
    ("backspace", "a character · delete ahead · ctrl+w a word"),
    ("ctrl+g", "edit the line in $EDITOR"),
    ("alt+↑ alt+↓", "earlier lines · ↑ ↓ too on a task line"),
    // arrange
    ("ctrl+s", "group by state, directory, state, repo"),
    ("ctrl+t", "pin it to the top · again unpins it"),
    ("z", "put it to sleep · again wakes it"),
    ("shift+↑", "move it up its group"),
    ("shift+↓", "move it down its group"),
    ("ctrl+r", "rename it"),
    ("i", "interrupt the turn it is on"),
    ("ctrl+x", "stop it · again forgets · a heading, the group"),
    ("c", "mark the finished · again clears them"),
    ("/", "find by name, task or #12, as you type"),
    ("s:", "filter by state, on the find line"),
    // dials
    ("alt+a", "which vendor the next agent runs"),
    ("alt+m", "which model the next agent uses"),
    ("alt+e", "how hard the next agent thinks"),
    ("alt+w", "whether it gets a worktree of its own"),
    ("shift+tab", "what it may do without asking"),
    ("m: p: w:", "model, permission and worktree, for one spawn"),
    ("e:", "effort, for one spawn"),
    ("b: pr:", "a base ref · a pull request to start on"),
    ("on:", "an existing branch to start on"),
    ("w:changes", "bring along the uncommitted changes"),
    ("d:", "the directory for one spawn"),
    ("agent:", "which vendor runs it, for one spawn"),
];

/// The groups [`HELP`] is cut into, in order, and how many keys each holds.
pub(super) const GROUPS: [(&str, usize); 5] = [
    ("walk", 8),
    ("look", 12),
    ("start", 12),
    ("arrange", 11),
    ("dials", 12),
];

/// Every key is in exactly one group.
const _: () = {
    let (mut under, mut at) = (0, 0);
    while at < GROUPS.len() {
        under += GROUPS[at].1;
        at += 1;
    }
    assert!(under == HELP.len());
};

/// Width of the key column, including the gap before the description.
const KEY: usize = 12;

/// Indent of a key under its heading.
const INDENT: usize = 2;

/// Width of a heading's right-aligned count, and the gap before it.
const COUNT: usize = 2;
const GAP: usize = 2;

/// The widest the column grows; on a wide terminal a key and its description
/// would otherwise sit too far apart to read as a pair.
const WIDEST: usize = 72;

/// Scroll position and search state of the keys overlay.
///
/// The keys only add and subtract; [`help`] clamps to the rows it was given
/// and keeps the result here between frames.
#[derive(Debug, Default)]
pub struct Keymap {
    /// Rows scrolled from the top.
    away: Cell<usize>,
    /// Rows shown last frame, which is one page.
    page: Cell<usize>,
    /// The search text once `/` was pressed. `Some("")` is an open, empty
    /// search line; `None` is no search.
    finding: Option<String>,
    /// How many user-bound keys there are, written by the paint for the
    /// status row's total.
    yours: Cell<usize>,
}

impl Keymap {
    /// Reset to the top with no search.
    pub fn opened(&mut self) {
        self.away.set(0);
        self.finding = None;
    }

    /// Whether the search line is open, so letters go to it.
    pub fn finding(&self) -> bool {
        self.finding.is_some()
    }

    /// Open the search line, or keep it open.
    pub fn find(&mut self) {
        self.finding.get_or_insert_with(String::new);
        self.away.set(0);
    }

    /// Append a letter to the search and scroll back to the top.
    pub fn typed(&mut self, letter: char) {
        if let Some(finding) = self.finding.as_mut() {
            finding.push(letter);
            self.away.set(0);
        }
    }

    /// Delete the search's last letter. The line stays open when empty.
    pub fn rubbed(&mut self) {
        if let Some(finding) = self.finding.as_mut() {
            finding.pop();
            self.away.set(0);
        }
    }

    /// Drop the search and show every key.
    pub fn found_nothing(&mut self) {
        self.finding = None;
        self.away.set(0);
    }

    /// Enter on the search line: keep the narrowing (an empty search closes).
    pub fn kept(&mut self) {
        if self.finding.as_deref() == Some("") {
            self.finding = None;
        }
    }

    /// The search text, empty when there is none.
    fn sought(&self) -> &str {
        self.finding.as_deref().unwrap_or_default()
    }

    /// Scroll `by` rows. The paint clamps the result.
    pub fn scrolled(&self, up: bool, by: usize) {
        let away = self.away.get();
        self.away.set(match up {
            true => away.saturating_sub(by),
            false => away.saturating_add(by),
        });
    }

    /// Rows one page key moves by.
    pub fn page(&self) -> usize {
        self.page.get().max(1)
    }

    /// Jump to the top, or to the foot (`usize::MAX`, clamped by the paint).
    pub fn to_the_end(&self, foot: bool) {
        self.away.set(match foot {
            true => usize::MAX,
            false => 0,
        });
    }
}

/// One row of the overlay, before layout.
enum Row {
    /// A group's label and how many of its keys are shown.
    Heading(&'static str, usize),
    /// A key and what it does.
    Key(&'static str, &'static str),
    /// A user-bound key and the command it runs.
    Yours(String, String),
    /// The blank row between groups.
    Blank,
}

impl Row {
    /// Whether this row is a key, which is what the status row counts.
    fn key(&self) -> bool {
        matches!(self, Row::Key(..) | Row::Yours(..))
    }
}

/// Draw the keys overlay into `area`, clamping the [`Keymap`] scroll to it.
pub(super) fn help(frame: &mut Frame, area: Rect, keymap: &Keymap, bound: &[Bound]) {
    let room = (area.width as usize).clamp(1, WIDEST);
    let sought = keymap.sought().to_lowercase();
    keymap.yours.set(bound.len());
    let rows = rows(&sought, bound);
    let keys = rows.iter().filter(|row| row.key()).count();

    // The status row and a blank row under it, dropped on a very short band.
    let height = area.height as usize;
    let chrome = match height {
        0..=3 => 0,
        _ => 2,
    };
    let visible = height.saturating_sub(chrome).max(1);
    keymap.page.set(visible);

    let away = keymap.away.get().min(rows.len().saturating_sub(visible));
    keymap.away.set(away);

    if chrome > 0 {
        let above = rows[..away].iter().filter(|row| row.key()).count();
        let shown = rows[away..(away + visible).min(rows.len())]
            .iter()
            .filter(|row| row.key())
            .count();
        frame.render_widget(
            Paragraph::new(marker(keymap, above, shown, keys, room)),
            Rect { height: 1, ..area },
        );
    }

    let lines: Vec<Line> = rows[away..(away + visible).min(rows.len())]
        .iter()
        .map(|row| line(row, room))
        .collect();
    frame.render_widget(
        Paragraph::new(lines),
        Rect {
            y: area.y + chrome as u16,
            height: area.height.saturating_sub(chrome as u16),
            ..area
        },
    );
}

/// The status row: the search on the left, and on the right which keys are on
/// screen out of how many, or how many matched.
fn marker(keymap: &Keymap, above: usize, shown: usize, keys: usize, room: usize) -> Line<'static> {
    let sought = match keymap.finding.as_deref() {
        // The block stands for the cursor.
        Some(sought) => format!(" find {sought}▌"),
        None => String::new(),
    };
    // No match, a scrolled range, a narrowed count against the whole table,
    // or the plain total.
    let whole = HELP.len() + keymap.yours.get();
    let standing = match (keys, shown < keys, keys < whole) {
        (0, ..) => " no keys match".to_string(),
        (keys, true, _) => format!("{}-{} of {keys} ", above + 1, above + shown),
        (keys, false, true) => format!("{keys} of {whole} "),
        (keys, false, false) => format!("{keys} keys "),
    };
    let air = room
        .saturating_sub(width_of(&sought))
        .saturating_sub(width_of(&standing));
    Line::from(vec![
        Span::styled(sought, bold()),
        Span::raw(" ".repeat(air)),
        Span::styled(standing, dim()),
    ])
}

/// Every row of the overlay: each group's heading and keys, then the
/// user-bound keys under `yours`.
///
/// `sought` filters on key spelling or description; a group left empty is
/// dropped with its heading.
fn rows(sought: &str, bound: &[Bound]) -> Vec<Row> {
    let mut rows = Vec::new();
    for (group, (label, _)) in GROUPS.iter().enumerate() {
        let keys: Vec<&(&'static str, &'static str)> = under(group)
            .filter(|(key, said)| matches(sought, key, said))
            .collect();
        if keys.is_empty() {
            continue;
        }
        if !rows.is_empty() {
            rows.push(Row::Blank);
        }
        rows.push(Row::Heading(label, keys.len()));
        rows.extend(keys.into_iter().map(|(key, said)| Row::Key(key, said)));
    }

    // User-bound keys come last. A bound key is described by its command.
    let yours: Vec<&Bound> = bound
        .iter()
        .filter(|one| matches(sought, &one.spelling, &one.command))
        .collect();
    if !yours.is_empty() {
        if !rows.is_empty() {
            rows.push(Row::Blank);
        }
        rows.push(Row::Heading("yours", yours.len()));
        rows.extend(
            yours
                .into_iter()
                .map(|one| Row::Yours(one.spelling.clone(), one.command.clone())),
        );
    }
    rows
}

/// Whether a key or its description contains `sought`, ignoring case.
fn matches(sought: &str, key: &str, said: &str) -> bool {
    sought.is_empty() || key.to_lowercase().contains(sought) || said.to_lowercase().contains(sought)
}

fn line(row: &Row, room: usize) -> Line<'static> {
    match row {
        Row::Heading(label, under) => Line::from(heading(label, *under, room)),
        Row::Key(key, said) => Line::from(keyed(key, said, room)),
        Row::Yours(key, said) => Line::from(keyed(key, said, room)),
        Row::Blank => Line::raw(""),
    }
}

/// A key in its column, then its description cut to fit.
fn keyed(key: &str, said: &str, room: usize) -> Vec<Span<'static>> {
    let does = room.saturating_sub(INDENT + KEY);
    vec![
        Span::raw(" ".repeat(INDENT)),
        Span::styled(grid::pad(key, KEY), bold()),
        Span::styled(fit(said, does), dim()),
    ]
}

/// A group heading: the label in bold capitals, a dim rule, and the key
/// count at the column's right edge.
fn heading(label: &str, under: usize, room: usize) -> Vec<Span<'static>> {
    let label = label.to_uppercase();
    // Everything on the row but the rule.
    let spent = 1 + width_of(&label) + 1 + GAP + COUNT;
    vec![
        Span::styled(format!(" {label} "), bold()),
        Span::styled(RULE.repeat(room.saturating_sub(spent).max(1)), dim()),
        Span::raw(" ".repeat(GAP)),
        Span::styled(grid::padl(&under.to_string(), COUNT), dim()),
    ]
}

/// A group's run of [`HELP`].
fn under(group: usize) -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    let from: usize = GROUPS[..group].iter().map(|(_, under)| under).sum();
    HELP[from..from + GROUPS[group].1].iter()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::keyname::spelt;
    use crate::tui::paint::fixtures::{cells, painted, showing};
    use crate::tui::paint::header::{header_rows, space_rows};
    use crate::tui::{Mode, Screen};
    use crossterm::event::{KeyCode, KeyEvent};
    use ratatui::style::Modifier;

    /// The overlay drawn at this size.
    fn overlay(size: (u16, u16)) -> Vec<String> {
        overlay_of(size, Vec::new())
    }

    /// The overlay with these user-bound keys.
    fn overlay_of(size: (u16, u16), bound: Vec<Bound>) -> Vec<String> {
        let mut screen = asking();
        screen.bound = bound;
        painted(&screen, size)
    }

    /// An empty view with the keys overlay open.
    fn asking() -> Screen {
        let mut screen = showing(Vec::new(), None);
        screen.mode = Mode::Keys;
        screen
    }

    /// A user binding, parsed as the config file's is.
    fn bound(spelling: &str, command: &str) -> Bound {
        Bound {
            spelling: spelling.to_string(),
            key: spelt(spelling).expect("a spelling the view can read"),
            command: command.to_string(),
        }
    }

    /// A screen tall enough for every key, computed from the table so it grows
    /// with it.
    fn tall_screen() -> (u16, u16) {
        // The overlay, its two status rows, the view's own chrome, and room
        // for a `yours` group of two.
        (100, document() + 2 + 5 + 4)
    }

    /// Rows in the overlay: every key, a heading per group, and a blank row
    /// between groups.
    fn document() -> u16 {
        (HELP.len() + 2 * GROUPS.len() - 1) as u16
    }

    /// A default terminal size, too short to hold every key.
    const SHORT_SCREEN: (u16, u16) = (80, 24);

    /// Each distinct screen seen while pressing `key` until nothing changes.
    fn walked(screen: &mut Screen, key: KeyCode, size: (u16, u16)) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..HELP.len() * 2 {
            let drawn = painted(screen, size).join("\n");
            if seen.last() == Some(&drawn) {
                break;
            }
            seen.push(drawn);
            let _ = screen.reading_the_keys(KeyEvent::from(key));
        }
        seen
    }

    #[test]
    fn keymap_reaches_every_key_by_scrolling_a_screen_too_short_to_hold_them() {
        let mut screen = asking();
        let seen = walked(&mut screen, KeyCode::Char('j'), SHORT_SCREEN);
        assert!(
            seen.len() > 1,
            "a screen this short does not hold them all at once:\n{}",
            seen.join("\n")
        );
        assert!(
            matches!(screen.mode, Mode::Keys),
            "and walking them is not the key that puts the agents back"
        );

        // Every key and description appeared on the way down.
        let scrolled = seen.join("\n");
        for (key, does) in HELP {
            assert!(
                scrolled.contains(key),
                "{key} was never on the screen:\n{scrolled}"
            );
            assert!(
                scrolled.contains(does),
                "{does} was never on the screen:\n{scrolled}"
            );
        }

        // The status row says how many keys there are in total.
        assert!(
            seen[0].contains(&format!("of {}", HELP.len())),
            "the first screenful says how many keys there are:\n{}",
            seen[0]
        );
    }

    #[test]
    fn keymap_walks_with_the_keys_that_walk_the_list_and_stops_at_either_end() {
        // The list's movement keys. Each pair goes down and back; returning to
        // the start shows both halves moved.
        for (down, up) in [
            (
                KeyEvent::from(KeyCode::Char('j')),
                KeyEvent::from(KeyCode::Char('k')),
            ),
            (KeyEvent::from(KeyCode::Down), KeyEvent::from(KeyCode::Up)),
            (
                KeyEvent::from(KeyCode::PageDown),
                KeyEvent::from(KeyCode::PageUp),
            ),
            (
                KeyEvent::new(KeyCode::Char('d'), crossterm::event::KeyModifiers::CONTROL),
                KeyEvent::new(KeyCode::Char('u'), crossterm::event::KeyModifiers::CONTROL),
            ),
        ] {
            let mut screen = asking();
            let top = painted(&screen, SHORT_SCREEN).join("\n");

            let _ = screen.reading_the_keys(down);
            let moved = painted(&screen, SHORT_SCREEN).join("\n");
            assert_ne!(moved, top, "{down:?} moves the screen");
            assert!(matches!(screen.mode, Mode::Keys), "{down:?}");

            let _ = screen.reading_the_keys(up);
            assert_eq!(
                painted(&screen, SHORT_SCREEN).join("\n"),
                top,
                "and {up:?} puts it back"
            );

            // Pressing past the top stays at the top.
            for _ in 0..HELP.len() {
                let _ = screen.reading_the_keys(up);
                let _ = painted(&screen, SHORT_SCREEN);
            }
            assert_eq!(painted(&screen, SHORT_SCREEN).join("\n"), top);
        }
    }

    #[test]
    fn keymap_reaches_both_ends_of_the_document_in_one_press_each() {
        let mut screen = asking();
        let top = painted(&screen, SHORT_SCREEN).join("\n");

        let _ = screen.reading_the_keys(KeyEvent::from(KeyCode::Char('G')));
        let foot = painted(&screen, SHORT_SCREEN);
        assert_ne!(foot.join("\n"), top, "G is the foot of them");
        let last = HELP.last().expect("the last key there is");
        assert!(
            foot.join("\n").contains(last.1),
            "which is the last key in the table:\n{}",
            foot.join("\n")
        );

        for _ in 0..2 {
            let _ = screen.reading_the_keys(KeyEvent::from(KeyCode::Char('g')));
        }
        assert_eq!(
            painted(&screen, SHORT_SCREEN).join("\n"),
            top,
            "and gg is the top"
        );
        assert!(matches!(screen.mode, Mode::Keys));
    }

    #[test]
    fn keymap_narrows_to_what_somebody_typed_on_the_key_and_on_what_it_does() {
        let mut screen = asking();
        for key in [KeyCode::Char('/'), KeyCode::Char('w'), KeyCode::Char('o')] {
            let _ = screen.reading_the_keys(KeyEvent::from(key));
        }
        let drawn = painted(&screen, SHORT_SCREEN).join("\n");

        assert!(drawn.contains("find wo▌"), "{drawn}");
        assert!(
            matches!(screen.mode, Mode::Keys),
            "and the letters are the search rather than keys of the wall"
        );

        // Both the key and its description are searched.
        assert!(
            drawn.contains("the word offered"),
            "matched on what it does:\n{drawn}"
        );
        assert!(
            drawn.contains("alt+w") && drawn.contains("worktree of its own"),
            "and on the spelling of a key:\n{drawn}"
        );
        assert!(
            !drawn.contains("start an agent"),
            "and nothing else is on the screen:\n{drawn}"
        );

        // An emptied group loses its heading; the count is against the table.
        assert!(!drawn.contains("WALK"), "{drawn}");
        assert!(
            drawn.contains("5 of 55"),
            "and it is said against the whole of the table, because `5 keys` \
             about a table of fifty-five reads as a program with five:\n{drawn}"
        );

        // Backspace removes a letter; esc drops the search.
        let _ = screen.reading_the_keys(KeyEvent::from(KeyCode::Backspace));
        assert!(
            painted(&screen, SHORT_SCREEN)
                .join("\n")
                .contains("find w▌"),
            "a letter at a time"
        );
        let _ = screen.reading_the_keys(KeyEvent::from(KeyCode::Esc));
        let back = painted(&screen, SHORT_SCREEN).join("\n");
        assert!(matches!(screen.mode, Mode::Keys), "esc drops the search");
        assert!(
            !back.contains('▌'),
            "the line is gone, block and all:\n{back}"
        );
        assert!(back.contains("WALK"), "and every key is back:\n{back}");
    }

    #[test]
    fn keymap_says_so_rather_than_going_blank_when_nothing_answers() {
        let mut screen = asking();
        let _ = screen.reading_the_keys(KeyEvent::from(KeyCode::Char('/')));
        for letter in "zzzz".chars() {
            let _ = screen.reading_the_keys(KeyEvent::from(KeyCode::Char(letter)));
        }
        let drawn = painted(&screen, SHORT_SCREEN).join("\n");
        assert!(drawn.contains("no keys match"), "{drawn}");
        assert!(drawn.contains("find zzzz▌"), "{drawn}");
    }

    #[test]
    fn keymap_stands_every_key_in_one_column_under_its_own_heading() {
        let painted = overlay(tall_screen());
        let all = painted.join("\n");

        // Nothing is cut at this size.
        for (key, does) in HELP {
            assert!(all.contains(key), "{key} is missing:\n{all}");
            assert!(all.contains(does), "{does} is missing:\n{all}");
        }
        assert!(!all.contains('…'), "nothing is elided this tall:\n{all}");

        // One column: each heading is below the previous group.
        let row = |said: &str| {
            painted
                .iter()
                .position(|line| line.contains(said))
                .unwrap_or_else(|| panic!("{said} is not on the screen:\n{all}"))
        };
        let mut last = 0;
        for (label, _) in GROUPS {
            let heading = format!(" {} ┈", label.to_uppercase());
            let at = row(&heading);
            assert!(at > last, "{heading:?} is not under the one before it");
            last = at;
        }

        // Keys are indented; a blank row separates groups.
        let walk = row(" WALK ┈");
        assert!(painted[walk + 1].starts_with("  "), "{:?}", painted[walk]);
        assert!(
            painted[walk + 1 + GROUPS[0].1].trim().is_empty(),
            "one group stands off from the next: {:?}",
            painted[walk + 1 + GROUPS[0].1]
        );
    }

    #[test]
    fn keymap_keeps_its_column_off_the_edge_of_a_very_wide_terminal() {
        let painted = overlay((200, tall_screen().1));
        let from = painted
            .iter()
            .position(|line| line.starts_with(" WALK ┈"))
            .expect("the first heading");
        for line in &painted[from..] {
            assert!(
                line.chars().count() <= WIDEST,
                "the column stops and the rest is margin: {line:?}"
            );
        }
    }

    #[test]
    fn view_lists_every_key_when_somebody_asks_for_them() {
        let screen = asking();
        // The overlay plus the header, spacing rows, status rows and keys row.
        let tall = document() + header_rows(24) + 2 * space_rows(24) + 1 + 2;
        let painted = painted(&screen, (100, tall)).join("\n");
        for (key, does) in HELP {
            assert!(painted.contains(key), "{key} is missing:\n{painted}");
            assert!(painted.contains(does), "{does} is missing:\n{painted}");
        }
    }

    #[test]
    fn keymap_stands_the_keys_somebody_bound_under_a_heading_of_their_own() {
        let painted = overlay_of(
            tall_screen(),
            vec![
                bound("alt+g", "lazygit"),
                bound("alt+t", "cargo test 2>&1 | less"),
            ],
        );
        let all = painted.join("\n");
        let row = |said: &str| {
            painted
                .iter()
                .position(|line| line.contains(said))
                .unwrap_or_else(|| panic!("{said} is not on the screen:\n{all}"))
        };

        // A heading like the built-in groups', with its count.
        let heading = painted[row(" YOURS ┈")].clone();
        assert!(
            heading.trim_end().ends_with('2'),
            "and how many stand under it: {heading:?}"
        );

        // After the built-in groups.
        assert!(
            row(" DIALS ┈") < row(" YOURS ┈"),
            "the group somebody wrote stands after the ones amx ships:\n{all}"
        );

        // The spelling in the key column, the command as the description.
        for (spelling, command) in [("alt+g", "lazygit"), ("alt+t", "cargo test 2>&1 | less")] {
            let line = &painted[row(spelling)];
            assert!(
                line.starts_with(&format!("{}{spelling}", " ".repeat(INDENT))),
                "the spelling stands in the key column: {line:?}"
            );
            assert!(
                line.contains(command),
                "{spelling} is not against what it runs: {line:?}"
            );
        }
    }

    #[test]
    fn keymap_finds_a_key_somebody_bound_the_way_it_finds_one_amx_binds() {
        let mut screen = asking();
        screen.bound = vec![bound("alt+g", "lazygit")];
        let _ = screen.reading_the_keys(KeyEvent::from(KeyCode::Char('/')));
        for letter in "lazy".chars() {
            let _ = screen.reading_the_keys(KeyEvent::from(KeyCode::Char(letter)));
        }
        let drawn = painted(&screen, SHORT_SCREEN).join("\n");
        assert!(drawn.contains("lazygit"), "{drawn}");
        assert!(drawn.contains("YOURS"), "{drawn}");
        assert!(!drawn.contains("WALK"), "{drawn}");
    }

    #[test]
    fn keymap_grows_nothing_for_a_config_that_bound_no_keys() {
        let painted = overlay(tall_screen()).join("\n");
        assert!(
            !painted.contains("YOURS"),
            "a heading over nothing is a heading in everybody's way:\n{painted}"
        );
    }

    #[test]
    fn keymap_opens_at_the_top_with_nothing_being_looked_for() {
        let mut screen = asking();
        let _ = screen.reading_the_keys(KeyEvent::from(KeyCode::Char('G')));
        let _ = screen.reading_the_keys(KeyEvent::from(KeyCode::Char('/')));
        let _ = screen.reading_the_keys(KeyEvent::from(KeyCode::Char('w')));
        let _ = screen.reading_the_keys(KeyEvent::from(KeyCode::Char('q')));

        let mut opened = asking();
        opened.keymap.opened();
        assert_eq!(
            painted(&opened, SHORT_SCREEN),
            painted(&asking(), SHORT_SCREEN)
        );
    }

    #[test]
    fn keymap_carries_the_weight_on_the_label_and_the_key_and_none_of_it_elsewhere() {
        let screen = asking();
        let size = tall_screen();
        let buffer = cells(&screen, size);
        let painted = painted(&screen, size);
        let row = painted
            .iter()
            .position(|line| line.starts_with(" WALK ┈"))
            .expect("the first heading") as u16;

        let label = buffer[(1, row)].clone();
        assert!(
            label.modifier.contains(Modifier::BOLD),
            "a heading is what makes a group out of a run of keys: {:?}",
            label.modifier
        );
        let rule = buffer[(7, row)].clone();
        assert_eq!(rule.symbol(), "┈", "the rule runs out to the count");
        assert!(
            rule.modifier.contains(Modifier::DIM),
            "and carries none of the weight: {:?}",
            rule.modifier
        );

        let key = buffer[(INDENT as u16, row + 1)].clone();
        assert!(
            key.modifier.contains(Modifier::BOLD),
            "the key itself is what somebody came here to find: {:?}",
            key.modifier
        );
        let does = buffer[((INDENT + KEY) as u16, row + 1)].clone();
        assert!(
            does.modifier.contains(Modifier::DIM),
            "and what it does stands behind it: {:?}",
            does.modifier
        );
    }
}
