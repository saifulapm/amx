//! Drawing the view.
//!
//! Five bands, top to bottom: what there is, the agents themselves, the closer
//! look at one of them when one is open, the line somebody is typing when they
//! are typing one, and the keys. Everything here is a function of what it is
//! handed, so what the screen says can be read back in a test without a
//! terminal anywhere near it.
//!
//! A surface to a file, and this one only stands them next to each other:
//! [`mod@header`] draws the two bands above the list, [`wall`] the agents
//! themselves, [`empty`] what stands there when there are none, [`card`] the
//! closer look at one of them, [`input`] the line being typed and the
//! keys under it, [`complete`] what the word under its cursor could be, and
//! [`mod@help`] the screen of every key. Under all of those,
//! [`text`] measures and cuts what a row says, [`prose`] draws an agent's
//! markdown into rows, and [`style`] turns what a thing means into the paint
//! that says so.
//!
//! Two kinds of thing are on the screen at once and they are drawn apart:
//! what is happening — the rows, the counters — and what the *next* agent will
//! be started with, which has not happened at all. Each has a row of its own
//! above the list, and the second hangs off the first on a branch glyph and
//! carries the accent on every value, so nobody reads a dial as a fact about
//! the fleet. The one thing of the second kind that is not on that row — what
//! the next agent may do without asking, said under the line that would start
//! it — is the one that wears weight as well, because it is beside a line
//! somebody is about to press enter on.
//!
//! No colour is decided here. A thing is painted for what it means — waiting,
//! done, failed — and which colour that is comes off the theme the screen
//! carries, so a person's palette reaches every one of these without any of
//! them knowing there is such a thing as a palette. Most of the screen is
//! painted in none of it: a wall where everything is coloured is a wall where
//! the colour says nothing.

mod card;
mod complete;
mod empty;
mod header;
mod help;
mod input;
mod prose;
mod style;
mod text;
mod wall;

use ratatui::Frame;
use ratatui::buffer::{Buffer, CellWidth};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Modifier;
use ratatui::widgets::Paragraph;
use std::cell::{Cell, RefCell};

use super::rows;
use super::{Mode, Screen};
use card::{card_height, card_rows, float};
use complete::{band, rows_wanted};
use header::{header, header_rows, space_rows};
use help::help;
use input::{composer_height, composing_line, footer, permission};
use wall::{Moment, agents, first_drawn};

#[cfg(test)]
pub(super) use card::walks;
pub use card::{Body, Card, Hunk, Scroll};
pub use header::title;
/// The table the keys overlay is drawn from, which is also the list of what
/// amx binds: [`keyname`](super::keyname) reads it to refuse a spelling of a
/// key of amx's own. The test up in the view presses everything a terminal can
/// send and holds what acted against it.
pub(super) use help::HELP;
pub use help::Keymap;
pub use input::Notice;
pub use wall::WallScroll;

/// Where the last frame put things, written back by a draw that is otherwise
/// a pure reading of the view, because the mouse arrives in the screen's own
/// coordinates: the band the rows were drawn in, which item its first row
/// held, and the band the card stands in. Cells, for the reason [`Scroll`]'s
/// are.
#[derive(Default)]
pub struct Map {
    /// The band the list was drawn in, and nothing while the keys overlay
    /// has it: a screen of keys has no rows under the pointer.
    list: Cell<Option<Rect>>,
    /// The item index of the band's first drawn row.
    offset: Cell<usize>,
    /// The last rows of that band, where a card is covering them.
    card: Cell<Option<Rect>>,
    /// What the frame says, cell by cell. The whole screen rather than the
    /// list alone: a drag is over the terminal, and what it covers is
    /// whatever was drawn there.
    drawn: RefCell<Buffer>,
}

impl Map {
    fn keep(&self, list: Option<Rect>, offset: usize, card: Option<Rect>) {
        self.list.set(list);
        self.offset.set(offset);
        self.card.set(card);
    }

    /// The text a selection covers, read off the last frame.
    pub(super) fn selected(&self, from: (u16, u16), to: (u16, u16)) -> String {
        selected_text(&self.drawn.borrow(), from, to)
    }

    /// How wide the band the list was drawn in is, which is the width the card
    /// under it has to wrap its words to. Nothing before the first frame.
    pub(super) fn width(&self) -> Option<u16> {
        self.list.get().map(|band| band.width)
    }

    /// The line of the list under this point, as an index into the items.
    ///
    /// The card covers the last rows of the list rather than standing among
    /// them, so a point on it names no line and every line of the list is
    /// where it would be with no card up. What comes back can run past the
    /// end of the items — the band is taller than the list — and the caller
    /// holds the bound, because only it has the items.
    pub(super) fn line_under(&self, column: u16, row: u16) -> Option<usize> {
        if self.over_the_card(column, row) {
            return None;
        }
        let band = self.list.get()?;
        if !band.contains(Position { x: column, y: row }) {
            return None;
        }
        Some(self.offset.get() + (row - band.y) as usize)
    }

    /// Whether this point is on the card's band.
    pub(super) fn over_the_card(&self, column: u16, row: u16) -> bool {
        self.card
            .get()
            .is_some_and(|card| card.contains(Position { x: column, y: row }))
    }
}

/// The text a selection covers, as the last frame drew it.
///
/// Reading order, whichever way the hand dragged: the rest of the first row
/// from where the press landed, every row between it and the release whole,
/// and the head of the last one. A row gives up its trailing blanks, because
/// the cells past the end of what a row says are the screen's rather than the
/// row's and nobody dragged over them on purpose.
pub(super) fn selected_text(drawn: &Buffer, from: (u16, u16), to: (u16, u16)) -> String {
    let (from, to) = in_reading_order(from, to);
    let area = drawn.area;
    (from.1..=to.1)
        .map(|row| {
            let (first, last) = span(row, from, to);
            let mut said = String::new();
            // Cells still covered by a wide symbol to their left.
            let mut hidden = 0;
            for column in area.left()..area.right().min(last.saturating_add(1)) {
                let Some(cell) = drawn.cell(Position { x: column, y: row }) else {
                    break;
                };
                if hidden > 0 {
                    hidden -= 1;
                    continue;
                }
                hidden = cell.cell_width().saturating_sub(1);
                if column >= first {
                    said.push_str(cell.symbol());
                }
            }
            said.trim_end().to_string()
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// A selection's two ends, in the order a reader would take them.
///
/// Which way the hand dragged is not a fact about the text: a drag up the
/// screen and a drag down it over the same cells copy the same words.
fn in_reading_order(from: (u16, u16), to: (u16, u16)) -> ((u16, u16), (u16, u16)) {
    match (from.1, from.0) <= (to.1, to.0) {
        true => (from, to),
        false => (to, from),
    }
}

/// The first and last column of `row` a selection covers, its ends already in
/// reading order. A row between the two ends is covered end to end, which is
/// as far right as the frame goes.
fn span(row: u16, from: (u16, u16), to: (u16, u16)) -> (u16, u16) {
    let first = match row == from.1 {
        true => from.0,
        false => 0,
    };
    let last = match row == to.1 {
        true => to.0.max(first),
        false => u16::MAX,
    };
    (first, last)
}

/// Draw everything.
pub fn draw(frame: &mut Frame, screen: &Screen) {
    let area = frame.area();
    // The palette this frame is painted in, handed down to everything that
    // draws: a colour is a role the theme answers for, and nothing under here
    // holds one of its own.
    let theme = screen.theme;
    let helping = matches!(screen.mode, Mode::Keys);
    let head = header_rows(area.height);
    let space = space_rows(area.height);
    let permission = permission(screen);
    let allowing = u16::from(permission.is_some());

    // The line being typed, where it is not the one the card is holding: an
    // answer is typed on the card itself, so it is not a band as well.
    let banded = screen.banded();
    // The reading behind the card, for the three things the card needs and does
    // not carry. A card is a picture of one agent, and the reading is what the
    // list is already holding.
    let on = screen
        .card
        .as_ref()
        .and_then(|card| screen.list.agent_by_id(&card.id));
    // What the record holds about the question the card is showing, which is
    // the half of a question no pane carries.
    let showing = on
        .filter(|_| screen.card.as_ref().is_some_and(Card::asks))
        .and_then(rows::showing);
    // And what its branch has open, which no pane carries either: a pull
    // request is a fact about the agent rather than about the turn.
    let prs = on.map_or(&[][..], |view| screen.list.requests(view));

    // Every band that is not the list: the header, the space under it, the
    // space over the keys, the keys, and the permission row. The card is not
    // among them — it is drawn over the foot of the list rather than taking
    // rows off it — so nothing here is measured against how tall it is.
    let chrome = head + space + space + 1 + allowing;
    // The composer takes what is left of that room: the rows under it, and the
    // line itself counted at the one row it never goes below.
    let composing = match banded {
        Some(composer) => composer_height(composer, area, chrome),
        None => 0,
    };
    // And what the word under the cursor could be, under the line it would be
    // written on — the band's own line, or the one at the foot of the card,
    // which offers the same words. It takes its rows off the list as the
    // composer does and stops where the composer stops: whatever else is
    // open, the list keeps a row, because the list is what the view is for.
    let suggest = banded
        .or(screen.answering())
        .and_then(|composer| composer.suggest.as_ref());
    let offering = rows_wanted(suggest).min(area.height.saturating_sub(chrome + composing + 1));

    let [top, _, middle, line, offered, allowed, _, keys] = Layout::vertical([
        Constraint::Length(head),
        Constraint::Length(space),
        Constraint::Min(1),
        Constraint::Length(composing),
        Constraint::Length(offering),
        Constraint::Length(allowing),
        Constraint::Length(space),
        Constraint::Length(1),
    ])
    .areas(area);

    // How much of that band the card covers, measured against the band itself
    // so it can never be so tall that the list it was opened from is gone. It
    // stands on the last rows of the list rather than beside them, which is
    // what keeps the wall still while a card opens, closes and is walked.
    let carding = match (helping, &screen.card) {
        (false, Some(card)) => card_height(
            area.height,
            middle.height,
            card_rows(card, showing, prs, screen.answering(), area.width),
        ),
        _ => 0,
    };
    let carded = Rect {
        y: middle.bottom() - carding,
        height: carding,
        ..middle
    };

    frame.render_widget(Paragraph::new(header(screen, top)), top);
    // Where the window stands over the list, clamped to a band this tall and
    // brought after the cursor where a move is owed one. Answered once and
    // handed to both the rows and the map, so the mouse reads back the rows
    // the frame drew.
    let offset = first_drawn(&screen.list, middle.height, &screen.wall);
    // What this frame put where, for the mouse to read back.
    screen.map.keep(
        (!helping).then_some(middle),
        offset,
        (carding > 0).then_some(carded),
    );
    match &screen.mode {
        Mode::Keys => help(frame, middle, &screen.keymap, &screen.bound),
        // The whole band, card or no card: the rows are laid out as if none
        // were up, and the card is drawn over the last of them.
        _ => agents(
            frame,
            &screen.list,
            middle,
            offset,
            Moment {
                beat: screen.beat,
                armed: screen.armed(),
                why: screen.why(),
                held: screen.held(),
                swept: screen.swept(),
                hover: screen.hover,
                lent: screen.lent.as_deref(),
                vendor: screen.vendor,
            },
            theme,
        ),
    }
    if carding > 0
        && let Some(card) = &screen.card
    {
        float(
            frame,
            card,
            // The reading behind it and the frame the wall is pulsing on, for
            // the mark the rule opens with and the words at the end of it.
            on,
            screen.beat,
            // What the list calls it, which is what its rule says. The id
            // where the list has lost the agent the card was taken from, so
            // the rule is never bare.
            on.map_or(card.id.as_str(), rows::called),
            // And what it runs, in the words the wall's own column says them
            // in, separated the way the rule separates everything else on it.
            // Nothing at all where the list has lost the row: the record is
            // where those words come from.
            &on.map_or(String::new(), |view| {
                rows::vendor_words(&view.meta).replace(' ', text::SEPARATOR)
            }),
            showing,
            prs,
            screen.answering(),
            &screen.scroll,
            carded,
            theme,
        );
    }
    if let Some(composer) = banded {
        composing_line(frame, composer, line, theme);
    }
    if let Some(suggest) = suggest.filter(|_| offering > 0) {
        frame.render_widget(
            Paragraph::new(band(suggest, offered.width as usize, theme)),
            offered,
        );
    }
    if let Some(row) = permission {
        frame.render_widget(Paragraph::new(row), allowed);
    }
    frame.render_widget(Paragraph::new(footer(screen, keys.width)), keys);

    // Last of all, because a selection is over the screen rather than over
    // any one band of it: the cells a hand is holding are turned about where
    // every widget has already had its say, and what the frame ended up
    // saying is kept for the release to read the text back off. Only while a
    // drag is up: every drag event is drawn before the release is read.
    let buffer = frame.buffer_mut();
    if let Some((from, to)) = screen.selection {
        reverse(buffer, from, to);
        screen.map.drawn.borrow_mut().clone_from(buffer);
    }
}

/// Turn the cells a selection covers about, so somebody dragging can see what
/// they have.
fn reverse(buffer: &mut Buffer, from: (u16, u16), to: (u16, u16)) {
    let (from, to) = in_reading_order(from, to);
    let area = buffer.area;
    for row in from.1..=to.1 {
        let (first, last) = span(row, from, to);
        for column in first..=last.min(area.right().saturating_sub(1)) {
            if let Some(cell) = buffer.cell_mut(Position { x: column, y: row }) {
                cell.modifier.insert(Modifier::REVERSED);
            }
        }
    }
}

/// Screens and readings the paint tests share.
#[cfg(test)]
mod fixtures {
    use super::{Card, draw};
    use crate::derive::{Evidence, Verdict, View};
    use crate::pr::{Pr, Standing};
    use crate::store::{Kind, Meta, Phase, State};
    use crate::theme::Theme;
    use crate::tmux::{PaneId, Socket};
    use crate::tui::Screen;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Modifier;
    use std::path::PathBuf;

    /// The default palette, which every screen built here is painted in.
    pub(super) fn theme() -> Theme {
        Theme::default()
    }

    /// A claude agent's reading in `phase`, saying `said`, `age` seconds old.
    pub(super) fn view(id: &str, phase: Phase, said: Option<&str>, age: u64) -> View {
        View {
            meta: Meta {
                role: None,
                parent: None,
                depth: 0,
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                agent: Some("claude".to_string()),
                model: None,
                effort: None,
                dir: PathBuf::from("/srv/app"),
                worktree: None,
                branch: None,
                base: None,
                socket: Socket::Name("amx".to_string()),
                pane: PaneId::new("%1").unwrap(),
                bg: false,
                session: None,
                transcript: None,
                created: 1,
            },
            state: State {
                state: phase,
                summary: said.map(str::to_string),
                since: 1,
                last_event: 1,
                ..State::default()
            },
            verdict: Verdict {
                phase,
                evidence: Evidence::Hooks,
                rule: None,
                age,
                // The rows print the worked seconds; both clocks get `age`.
                worked: age,
            },
            doing: None,
        }
    }

    /// A row run by a shell command (`!cmd` or `--exec`): no agent on it.
    pub(super) fn command(id: &str, phase: Phase) -> View {
        let mut view = view(id, phase, Some("cargo build"), 5);
        view.meta.agent = None;
        view
    }

    /// The same reading, on a branch of its own.
    pub(super) fn on_a_branch(mut view: View, branch: &str) -> View {
        view.meta.branch = Some(branch.to_string());
        view
    }

    /// A waiting agent and a working one, for a list to draw behind a card.
    pub(super) fn a_fleet() -> Vec<View> {
        vec![
            view("ask-a1b", Phase::Waiting, None, 29),
            view("busy-b2c", Phase::Working, Some("Running Bash"), 3),
        ]
    }

    /// The card a waiting agent's row opens: a question, its choices, and the
    /// pane it is asked on.
    pub(super) fn asking(options: &[&str], kind: Option<Kind>) -> Card {
        Card {
            id: "ask-a1b".to_string(),
            phase: Phase::Waiting,
            question: Some("Which fixture should the port keep?".to_string()),
            options: options.iter().map(|label| (*label).to_string()).collect(),
            walked: false,
            kind,
            body: "$ cargo test\nDo you want to proceed?".to_string(),
            changes: false,
            answer: false,
            listening: true,
            queued: Vec::new(),
        }
    }

    /// A forge with one failing request for `ask-a1b`, and a live one plus
    /// an older merged one for `busy-b2c`.
    pub(super) fn a_forge(meta: &Meta) -> Vec<Pr> {
        match meta.branch.as_deref() {
            Some("amx/ask-a1b") => vec![Pr {
                number: 12,
                standing: Standing::Failing,
            }],
            Some("amx/busy-b2c") => vec![
                Pr {
                    number: 40,
                    standing: Standing::Open,
                },
                Pr {
                    number: 7,
                    standing: Standing::Merged,
                },
            ],
            _ => Vec::new(),
        }
    }

    /// The view over these readings, with the card read as the view reads one.
    pub(super) fn showing(views: Vec<View>, card: Option<Card>) -> Screen {
        let mut screen = Screen::default();
        screen.list.show(views);
        screen.card = card.map(Card::read);
        screen
    }

    /// The same, over [`a_forge`].
    pub(super) fn over_the_forge(views: Vec<View>, card: Option<Card>) -> Screen {
        let mut screen = Screen::default();
        screen.list.asking(a_forge);
        screen.list.show(views);
        screen.card = card.map(Card::read);
        screen
    }

    /// The view with a launch profile opened on `~/code/amx`.
    pub(super) fn launching(views: Vec<View>) -> Screen {
        let mut screen = showing(views, None);
        screen.profile.dir = "~/code/amx".to_string();
        screen
    }

    /// The cells a view of this size draws.
    pub(super) fn cells(screen: &Screen, size: (u16, u16)) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
        terminal.draw(|frame| draw(frame, screen)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// The rows a view of this size draws, trailing blanks trimmed.
    pub(super) fn painted(screen: &Screen, size: (u16, u16)) -> Vec<String> {
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

    /// The rows drawn for these readings at this size.
    pub(super) fn drawn(views: Vec<View>, card: Option<Card>, size: (u16, u16)) -> Vec<String> {
        painted(&showing(views, card), size)
    }

    /// A heading row without its indent.
    pub(super) fn heading_of(line: &str) -> &str {
        line.trim()
    }

    /// The column of this row drawn in reverse video, which is the cursor
    /// block.
    pub(super) fn block(screen: &Screen, size: (u16, u16), row: u16) -> Option<u16> {
        let cells = cells(screen, size);
        (0..size.0).find(|column| cells[(*column, row)].modifier.contains(Modifier::REVERSED))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three rows of a wall, each the width the frame was drawn at: what a
    /// draw leaves behind for the mouse to read a selection out of.
    fn drawn() -> Buffer {
        Buffer::with_lines([
            " ● fix-login-a1b   wrote the parser  ".to_string(),
            " ● port-import-b2c did what was asked".to_string(),
            " ".repeat(37),
        ])
    }

    #[test]
    fn a_selection_on_one_row_is_the_cells_between_its_ends() {
        // The id on the first row: past the indent and the glyph, and the
        // last cell of the word is the one the release landed on.
        assert_eq!(selected_text(&drawn(), (3, 0), (15, 0)), "fix-login-a1b");
        // Dragged the other way, which is the same selection.
        assert_eq!(selected_text(&drawn(), (15, 0), (3, 0)), "fix-login-a1b");
        // One cell is one character.
        assert_eq!(selected_text(&drawn(), (3, 0), (3, 0)), "f");
    }

    #[test]
    fn a_selection_across_two_rows_is_read_in_reading_order() {
        // From the id on the first row to the id on the second: the rest of
        // the first row, then the second row up to where the release landed.
        assert_eq!(
            selected_text(&drawn(), (3, 0), (18, 1)),
            "fix-login-a1b   wrote the parser\n ● port-import-b2c"
        );
        // Whichever end the hand started at.
        assert_eq!(
            selected_text(&drawn(), (18, 1), (3, 0)),
            "fix-login-a1b   wrote the parser\n ● port-import-b2c"
        );
        // A row with nothing on it under the selection is a blank line
        // rather than a run of spaces.
        assert_eq!(
            selected_text(&drawn(), (23, 1), (10, 2)),
            "what was asked\n"
        );
    }

    #[test]
    fn a_selection_wider_than_the_row_says_stops_where_it_stops() {
        // The cells past the end of a row are the screen's, so a drag that
        // ran out over them copies the row and none of them.
        assert_eq!(
            selected_text(&drawn(), (3, 0), (36, 0)),
            "fix-login-a1b   wrote the parser"
        );
    }

    #[test]
    fn a_selection_copies_a_wide_character_once() {
        let drawn = Buffer::with_lines(["日本 ab"]);
        assert_eq!(selected_text(&drawn, (0, 0), (6, 0)), "日本 ab");
        assert_eq!(selected_text(&drawn, (2, 0), (4, 0)), "本");
    }
}
