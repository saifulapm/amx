//! Drawing the view.
//!
//! Bands top to bottom: the header, the list of agents, the card over the
//! foot of the list, the typed line, and the keys. [`draw`] lays them out and
//! each submodule draws one: [`mod@header`], [`wall`] (the list), [`empty`]
//! (an empty list), [`card`], [`input`] (the typed line and the keys row),
//! [`complete`] (completions under the line) and [`mod@help`] (the keys
//! overlay). [`text`], [`prose`] and [`style`] are shared helpers.
//!
//! - Drawing is a pure function of the [`Screen`], apart from the `Cell`s it
//!   writes back (scroll clamps, the mouse [`Map`]), so tests read the screen
//!   off a `TestBackend`.
//! - No colour is chosen here: [`style`] maps meanings to the [`Theme`] roles
//!   the screen carries.
//!
//! [`Theme`]: crate::theme::Theme

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
/// Every key amx binds. [`keyname`](super::keyname) reads it to refuse a user
/// binding that shadows one.
pub(super) use help::HELP;
pub use help::Keymap;
pub use input::Notice;
pub use wall::WallScroll;

/// Where the last frame put things, for mapping mouse coordinates back to the
/// list and the card. Written by [`draw`] through `Cell`s.
#[derive(Default)]
pub struct Map {
    /// The list's band; `None` while the keys overlay covers it.
    list: Cell<Option<Rect>>,
    /// The item index of the band's first drawn row.
    offset: Cell<usize>,
    /// The card's band over the foot of the list, if one is up.
    card: Cell<Option<Rect>>,
    /// The whole frame, kept while a drag is up so the release can read the
    /// selected text back.
    drawn: RefCell<Buffer>,
}

impl Map {
    fn keep(&self, list: Option<Rect>, offset: usize, card: Option<Rect>) {
        self.list.set(list);
        self.offset.set(offset);
        self.card.set(card);
    }

    /// The text a selection covers, read off the last frame drawn with it.
    pub(super) fn selected(&self, from: (u16, u16), to: (u16, u16)) -> String {
        selected_text(&self.drawn.borrow(), from, to)
    }

    /// The width of the list's band, which the card wraps its text to. `None`
    /// before the first frame.
    pub(super) fn width(&self) -> Option<u16> {
        self.list.get().map(|band| band.width)
    }

    /// The item index of the list line under this point.
    ///
    /// `None` on the card. The index may run past the end of the items (the
    /// band can be taller than the list); the caller bounds it.
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

/// The text a selection covers in `drawn`.
///
/// In reading order whichever way the drag went: the rest of the first row,
/// the rows between in full, and the start of the last row. Trailing blanks
/// are trimmed from each row.
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

/// A selection's two ends, the earlier one first.
fn in_reading_order(from: (u16, u16), to: (u16, u16)) -> ((u16, u16), (u16, u16)) {
    match (from.1, from.0) <= (to.1, to.0) {
        true => (from, to),
        false => (to, from),
    }
}

/// The first and last column of `row` a selection covers, given its ends in
/// reading order. Rows between the ends run to `u16::MAX`.
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
    let theme = screen.theme;
    let helping = matches!(screen.mode, Mode::Keys);
    let head = header_rows(area.height);
    let space = space_rows(area.height);
    permission(screen);

    // The typed line in its own band; a card's answer line is drawn on the card.
    let banded = screen.banded();
    // The list's reading of the carded agent, for what the card itself does
    // not carry.
    let on = screen
        .card
        .as_ref()
        .and_then(|card| screen.list.agent_by_id(&card.id));
    // The recorded question the card shows, if it is asking.
    let showing = on
        .filter(|_| screen.card.as_ref().is_some_and(Card::asks))
        .and_then(rows::showing);
    let prs = on.map_or(&[][..], |view| screen.list.requests(view));

    // Every band but the list. The card is not one: it covers the foot of the
    // list instead of taking rows from it.
    let chrome = head + space + space + 1;
    let composing = match banded {
        Some(composer) => composer_height(composer, area, chrome),
        None => 0,
    };
    // Completions for either the banded line or the card's line. Like the
    // composer they take rows from the list, which always keeps one.
    let suggest = banded
        .or(screen.answering())
        .and_then(|composer| composer.suggest.as_ref());
    let offering = rows_wanted(suggest).min(area.height.saturating_sub(chrome + composing + 1));

    let [top, _, middle, line, offered, _, keys] = Layout::vertical([
        Constraint::Length(head),
        Constraint::Length(space),
        Constraint::Min(1),
        Constraint::Length(composing),
        Constraint::Length(offering),
        Constraint::Length(space),
        Constraint::Length(1),
    ])
    .areas(area);

    // The card covers the last rows of the list band, so opening, closing or
    // walking it never moves a row of the list.
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
    // Computed once so the rows and the mouse map agree.
    let offset = first_drawn(&screen.list, middle.height, &screen.wall);
    screen.map.keep(
        (!helping).then_some(middle),
        offset,
        (carding > 0).then_some(carded),
    );
    match &screen.mode {
        Mode::Keys => help(frame, middle, &screen.keymap, &screen.bound),
        // The whole band; a card is drawn over its foot afterwards.
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
            on,
            screen.beat,
            // The bare id when the list has lost the agent.
            on.map_or(card.id.as_str(), rows::called),
            // The wall's vendor words, with the rule's separator.
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
    frame.render_widget(Paragraph::new(footer(screen, keys.width)), keys);

    // The selection is drawn last, over every band. The frame is kept only
    // while a drag is up: the event loop draws after every drag event, so
    // the release always reads a frame drawn with the final selection.
    let buffer = frame.buffer_mut();
    if let Some((from, to)) = screen.selection {
        reverse(buffer, from, to);
        screen.map.drawn.borrow_mut().clone_from(buffer);
    }
}

/// Draw the cells a selection covers in reverse video.
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

    /// A three-row frame: two list rows and a blank one.
    fn drawn() -> Buffer {
        Buffer::with_lines([
            " ● fix-login-a1b   wrote the parser  ".to_string(),
            " ● port-import-b2c did what was asked".to_string(),
            " ".repeat(37),
        ])
    }

    #[test]
    fn a_selection_on_one_row_is_the_cells_between_its_ends() {
        // Both end cells are included.
        assert_eq!(selected_text(&drawn(), (3, 0), (15, 0)), "fix-login-a1b");
        assert_eq!(selected_text(&drawn(), (15, 0), (3, 0)), "fix-login-a1b");
        assert_eq!(selected_text(&drawn(), (3, 0), (3, 0)), "f");
    }

    #[test]
    fn a_selection_across_two_rows_is_read_in_reading_order() {
        // The rest of the first row, then the second up to the release.
        assert_eq!(
            selected_text(&drawn(), (3, 0), (18, 1)),
            "fix-login-a1b   wrote the parser\n ● port-import-b2c"
        );
        assert_eq!(
            selected_text(&drawn(), (18, 1), (3, 0)),
            "fix-login-a1b   wrote the parser\n ● port-import-b2c"
        );
        // A blank row copies as an empty line.
        assert_eq!(
            selected_text(&drawn(), (23, 1), (10, 2)),
            "what was asked\n"
        );
    }

    #[test]
    fn a_selection_wider_than_the_row_says_stops_where_it_stops() {
        // Blank cells past the row's text are not copied.
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
