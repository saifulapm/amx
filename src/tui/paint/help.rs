//! The screen of keys, for whoever asked what they are.
//!
//! Not a band: it stands where the list stands, because a person who has asked
//! what the keys are is not reading the wall. The table of every key is here,
//! and beside it how that table is stood on a terminal of any shape.
//!
//! It is drawn as the wall it replaces. A group of keys carries the heading a
//! group of agents carries — the label uppercase and bold, a dim rule run out
//! to the number under it — so the overlay reads as the same screen showing
//! something else rather than as a manual somebody opened.
//!
//! **One column, scrolled, with a way to search it.** It was two columns
//! paged with `pgup` and `pgdn`, and that was a wall of fifty-five keys with
//! no sign that there was a second screenful and no key anybody guessed for
//! reaching it (Saiful, 2026-09-21). So: one column, walked with the keys that
//! walk the list — `j` and `k`, the arrows, the half and whole pages, `gg` and
//! `G` — and a row at the top that says which of them are on the screen out of
//! how many there are. Fifty-five keys is a document, and a document nobody
//! can tell the length of is one nobody scrolls.
//!
//! And `/`, which narrows them as it is typed, on the spelling of a key and on
//! what it does alike. Somebody at this screen is looking for one key and has
//! a word for what they want it to do; reading five headings to find out which
//! list that word is under is the work the search is here to not do.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::cell::Cell;

use super::style::{bold, dim};
use super::text::{RULE, fit, width_of};
use crate::tui::grid;
use crate::tui::keyname::Bound;

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
pub(in crate::tui) const HELP: [(&str, &str); 55] = [
    // walk
    ("↑ ↓ j k", "walk the agents"),
    ("gg G", "the top of the list, and the foot"),
    ("alt+1..9", "reach one by where it is on the wall"),
    ("w", "the first agent that needs you"),
    ("backspace", "the agent you were last in"),
    ("esc", "put the card away · leave a line alone"),
    ("?", "these keys"),
    ("q ctrl+c", "close the view"),
    // look
    ("space", "the card, and an answer or a message on it"),
    ("v", "which vendor, model and effort each one runs"),
    ("enter → l", "bring its window forward · shut a group"),
    ("d", "what it has changed"),
    ("o", "open its pull request in the browser"),
    ("alt+d", "the patch in the viewer the config names"),
    ("pgup ctrl+b", "page the card, when it holds more"),
    ("pgdn ctrl+f", "and the other way"),
    ("ctrl+u", "half a page of it, toward the edge"),
    ("ctrl+d", "and half a page away"),
    ("ctrl+n", "the next hunk of the patch"),
    ("ctrl+p", "and the one before it, or the top"),
    // start
    ("n", "start an agent"),
    ("alt+n", "start the line and go to the agent"),
    ("f", "start a copy of it on a task"),
    ("!", "run the line as a command, not a task"),
    ("shift+enter", "a newline in the line, without sending it"),
    ("alt+enter", "the same, where shift+enter does not arrive"),
    ("ctrl+j", "the same, where neither of those arrives"),
    ("tab", "the word offered · on nothing, the agents"),
    ("← → ctrl+←", "the cursor: a character, a word, home end"),
    ("backspace", "a character · delete ahead · ctrl+w a word"),
    ("ctrl+g", "write the line in $EDITOR"),
    ("alt+↑ alt+↓", "lines sent before · ↑ ↓ too on a task line"),
    // arrange
    ("ctrl+s", "gather them: state, directory, state, repo"),
    ("ctrl+t", "pin it over the wall · again lets it go"),
    ("z", "put it under everything · again wakes it"),
    ("shift+↑", "move it up its group"),
    ("shift+↓", "move it down its group"),
    ("ctrl+r", "call it something else"),
    ("i", "cut short the turn it is on"),
    ("ctrl+x", "stop it · again forgets · a heading, the group"),
    ("c", "clear the finished · again takes them"),
    ("/", "find by name, task or #12, as you type"),
    ("s:", "narrow by state, on the find line"),
    // dials
    ("alt+a", "which vendor the next agent runs"),
    ("alt+m", "which model the next agent is given"),
    ("alt+e", "how hard the next agent thinks"),
    ("alt+w", "whether it gets a worktree of its own"),
    ("shift+tab", "what it may do without asking"),
    ("m: p: w:", "model, permission and worktree, for one spawn"),
    ("e:", "effort, for one spawn"),
    ("b: pr:", "a ref to cut from · a request to start on"),
    ("on:", "a branch that already exists, to start on"),
    ("w:changes", "take the uncommitted work"),
    ("d:", "where one spawn runs, on the task line"),
    ("agent:", "which vendor runs it, for one spawn"),
];

/// What the keys are for, and how many of [`HELP`] each of those answers for.
///
/// A flat list of fifty-odd is a list somebody reads all of to find one, so
/// the table is cut into what a person is trying to do: get about the wall,
/// read one agent, put work in, arrange what is already there, and set what
/// the next agent runs. Five short lists are five places to not look.
///
/// Runs rather than tables of their own, so nothing here can hold a key twice
/// or drop one between two headings.
pub(super) const GROUPS: [(&str, usize); 5] = [
    ("walk", 8),
    ("look", 12),
    ("start", 12),
    ("arrange", 11),
    ("dials", 12),
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

/// The widest the one column is let grow.
///
/// A key and a sentence about it on a 200-column terminal is a line whose two
/// halves are half a screen apart, and the eye loses which description belongs
/// to which key somewhere in the middle. So the column stops, and the rest of
/// the screen is margin.
const WIDEST: usize = 72;

/// What the keys screen is showing, and what somebody is looking for on it.
///
/// The clamps are the paint's, the way the card's are: only the paint knows
/// how many rows a screen this shape gave the keys, so the keys that scroll
/// only add and subtract and this is where the answer is kept between frames.
#[derive(Debug, Default)]
pub struct Keymap {
    /// How far down the keys the screen stands, in rows.
    away: Cell<usize>,
    /// The rows the keys had last frame, which is what one page moves by.
    page: Cell<usize>,
    /// What somebody has typed to narrow them, once they have pressed `/`.
    ///
    /// `None` and `Some("")` are different states: the first is a screen
    /// nobody is searching and the second is an open line with nothing on it
    /// yet, which narrows nothing but takes the next letter.
    finding: Option<String>,
    /// How many keys somebody bound themselves, which the paint writes here
    /// for the row over the keys to count them in: what a narrowing left is
    /// only worth saying against the whole of what there was.
    yours: Cell<usize>,
}

impl Keymap {
    /// Open at the top, with nothing being looked for.
    ///
    /// The question is what the keys are, not where somebody stopped reading
    /// them the last time they asked.
    pub fn opened(&mut self) {
        self.away.set(0);
        self.finding = None;
    }

    /// Whether somebody is typing what they are looking for, which is what
    /// decides whether a letter is a search or a key.
    pub fn finding(&self) -> bool {
        self.finding.is_some()
    }

    /// Open the find line, or leave it open if it already is.
    pub fn find(&mut self) {
        self.finding.get_or_insert_with(String::new);
        self.away.set(0);
    }

    /// Put a letter on it, and go back to the top: what somebody has just
    /// narrowed to is a list they have not read any of yet.
    pub fn typed(&mut self, letter: char) {
        if let Some(finding) = self.finding.as_mut() {
            finding.push(letter);
            self.away.set(0);
        }
    }

    /// Take the last letter off it. The line stays open with nothing on it,
    /// because a line that closed itself on the last backspace would be a
    /// screen that jumped out from under somebody mid-word.
    pub fn rubbed(&mut self) {
        if let Some(finding) = self.finding.as_mut() {
            finding.pop();
            self.away.set(0);
        }
    }

    /// Drop what was being looked for and give every key back.
    pub fn found_nothing(&mut self) {
        self.finding = None;
        self.away.set(0);
    }

    /// Close the line and keep the narrowing, which is what `enter` on it
    /// means: somebody has typed what they wanted and is about to read it.
    pub fn kept(&mut self) {
        if self.finding.as_deref() == Some("") {
            self.finding = None;
        }
    }

    /// What is being looked for, which is nothing where the line is shut.
    fn sought(&self) -> &str {
        self.finding.as_deref().unwrap_or_default()
    }

    /// Move `by` rows, either way. The clamp is the paint's, so a press past
    /// the end lands where the press before it did.
    pub fn scrolled(&self, up: bool, by: usize) {
        let away = self.away.get();
        self.away.set(match up {
            true => away.saturating_sub(by),
            false => away.saturating_add(by),
        });
    }

    /// The rows one page key moves by, which is the rows the keys were given.
    pub fn page(&self) -> usize {
        self.page.get().max(1)
    }

    /// The top of them, and the foot. The foot is anywhere past the end: the
    /// paint clamps it to the last screenful there is.
    pub fn to_the_end(&self, foot: bool) {
        self.away.set(match foot {
            true => usize::MAX,
            false => 0,
        });
    }
}

/// One line of the screen before it knows how wide it is.
enum Row {
    /// A group's label and how many keys are under it here.
    Heading(&'static str, usize),
    /// A key, and what it does.
    Key(&'static str, &'static str),
    /// A key somebody bound themselves, against the command it runs.
    Yours(String, String),
    /// The row one group stands off the next by.
    Blank,
}

impl Row {
    /// Whether this row is one of the keys, which is what the count at the top
    /// counts: a heading is not a key and neither is the air around it.
    fn key(&self) -> bool {
        matches!(self, Row::Key(..) | Row::Yours(..))
    }
}

/// Every key and what it does, under the heading that says what it is for.
///
/// Which rows are on the screen is the view's to hold, because it is where
/// somebody left off reading rather than a fact about the screen. The clamp is
/// here: only the paint knows how many rows a screen this shape gave them, so
/// the keys that scroll only add and subtract.
pub(super) fn help(frame: &mut Frame, area: Rect, keymap: &Keymap, bound: &[Bound]) {
    let room = (area.width as usize).clamp(1, WIDEST);
    let sought = keymap.sought().to_lowercase();
    keymap.yours.set(bound.len());
    let rows = rows(&sought, bound);
    let keys = rows.iter().filter(|row| row.key()).count();

    // One row at the top for what is being looked for and how much of the
    // document is on the screen, and the air under it that a heading stands
    // off by. A screen with room for neither gives them up in that order: the
    // keys are what somebody came for.
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

/// The row over the keys: what somebody is looking for, and where in the
/// document they are standing.
///
/// The second half is the whole reason this row exists. Fifty-five keys do not
/// fit on a terminal and the screen said nothing about the ones that were not
/// on it, so the keys below the fold were keys nobody knew to look for.
///
/// Where nothing is being looked for and everything fits, the row is empty —
/// which is a document with no fold and nothing to say about one.
fn marker(keymap: &Keymap, above: usize, shown: usize, keys: usize, room: usize) -> Line<'static> {
    let sought = match keymap.finding.as_deref() {
        // The block is the cursor: the line is open and taking letters.
        Some(sought) => format!(" find {sought}▌"),
        None => String::new(),
    };
    // Three things to say and one row to say them in: that nothing answered,
    // that what is on the screen is part of a longer document, or how many
    // keys there are — and where a narrowing is on, how many of the whole
    // table that is, because `3 keys` about a table of fifty-five reads as a
    // program with three keys.
    let whole = HELP.len() + keymap.yours.get();
    let standing = match (keys, shown < keys, keys < whole) {
        (0, ..) => " nothing answers to that".to_string(),
        (keys, true, _) => format!("{}–{} of {keys} ", above + 1, above + shown),
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

/// Every row of the document, in the order it is read: each group's heading
/// and the keys under it, then the ones somebody bound themselves.
///
/// Narrowed by what is being looked for, on the spelling of a key and on what
/// it does alike — the two halves of what somebody has in their head when they
/// come here, and no reason to make them guess which half they are typing. A
/// group with nothing left in it goes with its keys: a heading over nothing is
/// a heading in everybody's way.
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

    // The keys somebody bound themselves, after the last of amx's own and
    // under the same kind of heading: a group like the five, in the place the
    // eye gets to last, because the keys the view binds are the ones on every
    // machine.
    //
    // The command is the row: a bound key has no name but what it runs, and a
    // second one somebody had to write would be a name that goes stale the day
    // they change the command.
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

/// Whether a key answers to what is being looked for.
fn matches(sought: &str, key: &str, said: &str) -> bool {
    sought.is_empty() || key.to_lowercase().contains(sought) || said.to_lowercase().contains(sought)
}

/// One row of the document, drawn for a column this wide.
fn line(row: &Row, room: usize) -> Line<'static> {
    match row {
        Row::Heading(label, under) => Line::from(heading(label, *under, room)),
        Row::Key(key, said) => Line::from(keyed(key, said, room)),
        Row::Yours(key, said) => Line::from(keyed(key, said, room)),
        Row::Blank => Line::raw(""),
    }
}

/// One key and what it does: the key in a column of its own, and what is left
/// of the room for the rest.
fn keyed(key: &str, said: &str, room: usize) -> Vec<Span<'static>> {
    let does = room.saturating_sub(INDENT + KEY);
    vec![
        Span::raw(" ".repeat(INDENT)),
        Span::styled(grid::pad(key, KEY), bold()),
        Span::styled(fit(said, does), dim()),
    ]
}

/// A heading over a run of keys: what they are for, a rule, and how many of
/// them there are at the column's own right edge.
///
/// The shape a group of agents wears on the wall, so a person who has learned
/// to read one heading has learned to read the other.
fn heading(label: &str, under: usize, room: usize) -> Vec<Span<'static>> {
    let label = label.to_uppercase();
    // What the rule is left: the space in front of the label, the label, the
    // space after it, and the gap and the count at the far end.
    let spent = 1 + width_of(&label) + 1 + GAP + COUNT;
    vec![
        Span::styled(format!(" {label} "), bold()),
        Span::styled(RULE.repeat(room.saturating_sub(spent).max(1)), dim()),
        Span::raw(" ".repeat(GAP)),
        Span::styled(grid::padl(&under.to_string(), COUNT), dim()),
    ]
}

/// The keys one group stands over, which is its run of [`HELP`].
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

    /// The overlay on a screen this size.
    fn overlay(size: (u16, u16)) -> Vec<String> {
        overlay_of(size, Vec::new())
    }

    /// The same, with keys somebody bound in their config file.
    fn overlay_of(size: (u16, u16), bound: Vec<Bound>) -> Vec<String> {
        let mut screen = asking();
        screen.bound = bound;
        painted(&screen, size)
    }

    /// The view with the keys on the screen, which is what `?` opens.
    fn asking() -> Screen {
        let mut screen = showing(Vec::new(), None);
        screen.mode = Mode::Keys;
        screen
    }

    /// One key somebody bound, read the way the config file's own table is.
    fn bound(spelling: &str, command: &str) -> Bound {
        Bound {
            spelling: spelling.to_string(),
            key: spelt(spelling).expect("a spelling the view can read"),
            command: command.to_string(),
        }
    }

    /// A screen tall enough for every key at once, worked out rather than
    /// counted off one somebody looked at: it grows every time the table does,
    /// and a number written here would send the next key that joins the table
    /// scrolling.
    fn tall_screen() -> (u16, u16) {
        // The document, the two chrome rows over it, the chrome the view
        // draws around the whole overlay, and room for a `yours` group of two
        // so the one test that grows one is not the one test that scrolls.
        (100, document() + 2 + 5 + 4)
    }

    /// How many rows the document is: every key, a heading over each group,
    /// and the row that stands each group off from the one above it.
    fn document() -> u16 {
        (HELP.len() + 2 * GROUPS.len() - 1) as u16
    }

    /// The screen most people have, which is far too short for a document this
    /// long and is the shape the scrolling is for. It is what a terminal opens
    /// at.
    const SHORT_SCREEN: (u16, u16) = (80, 24);

    /// Every key the screen shows at this size, walked down with `key` until
    /// the screen stops changing.
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

        // Every key was on the screen at some point on the way down, and what
        // it does with it.
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

        // And the first screenful says how much of the document it is holding,
        // which is the whole of what a fold nobody can see needs to say.
        assert!(
            seen[0].contains(&format!("of {}", HELP.len())),
            "the first screenful says how many keys there are:\n{}",
            seen[0]
        );
    }

    #[test]
    fn keymap_walks_with_the_keys_that_walk_the_list_and_stops_at_either_end() {
        // The same keys the wall answers to, because somebody who has walked a
        // wall has already learned them. Each pair is walked to its end and
        // back, and landing where it started is what says both halves moved.
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

            // And neither walks off its end: a press past the top or the foot
            // lands where the press before it did.
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

        // G, and the gg under it: the two the wall answers to.
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

        // What was typed, on the line, with the block that says it is still
        // taking letters.
        assert!(drawn.contains("find wo▌"), "{drawn}");
        assert!(
            matches!(screen.mode, Mode::Keys),
            "and the letters are the search rather than keys of the wall"
        );

        // Both halves of a row are searched, because somebody looking for a
        // key has a word for what they want it to do as often as a spelling.
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

        // A heading over nothing goes with its keys, and the count over it is
        // what the narrowing left rather than what the table holds.
        assert!(!drawn.contains("WALK"), "{drawn}");
        assert!(
            drawn.contains("6 of 55"),
            "and it is said against the whole of the table, because `6 keys` \
             about a table of fifty-five reads as a program with six:\n{drawn}"
        );

        // Backspace takes a letter back, and esc gives every key back.
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
        assert!(drawn.contains("nothing answers to that"), "{drawn}");
        assert!(drawn.contains("find zzzz▌"), "{drawn}");
    }

    #[test]
    fn keymap_stands_every_key_in_one_column_under_its_own_heading() {
        let painted = overlay(tall_screen());
        let all = painted.join("\n");

        // Every key and what it does, whole: a screen this tall has room for
        // the longest of them, so nothing on it is cut short.
        for (key, does) in HELP {
            assert!(all.contains(key), "{key} is missing:\n{all}");
            assert!(all.contains(does), "{does} is missing:\n{all}");
        }
        assert!(!all.contains('…'), "nothing is elided this tall:\n{all}");

        // One column: every heading is against the left edge, and every group
        // is under the one before it rather than beside it.
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

        // And each group stands off from the one above it, with its keys
        // indented under its own heading.
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
        // A key and a sentence about it half a screen apart is a pair the eye
        // loses in the middle.
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
        // Tall enough for every key and every heading over them, plus the
        // chrome the overlay is drawn inside: the header, the blank row under
        // it, the row that says where in the document this is and the air
        // under that, and the blank row over the keys at the foot and the keys
        // themselves.
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

        // The heading amx's own groups wear, counting the keys under it: what
        // somebody bound is a group of keys like any other.
        let heading = painted[row(" YOURS ┈")].clone();
        assert!(
            heading.trim_end().ends_with('2'),
            "and how many stand under it: {heading:?}"
        );

        // Under the last of amx's own rather than over them: the keys the view
        // binds are the ones every machine has.
        assert!(
            row(" DIALS ┈") < row(" YOURS ┈"),
            "the group somebody wrote stands after the ones amx ships:\n{all}"
        );

        // The spelling in the key column and the command against it, because
        // the command is what a bound key is: there is no second name for it.
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
        // The question is what the keys are, not where somebody stopped
        // reading them the last time they asked.
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
