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
pub use card::{Body, Card, Hunk, Scroll, body_width};
pub use header::title;
/// The table the keys overlay is drawn from, which is also the list of what
/// amx binds: [`keyname`](super::keyname) reads it to refuse a spelling of a
/// key of amx's own. The test up in the view presses everything a terminal can
/// send and holds what acted against it.
pub(super) use help::HELP;
pub use input::Notice;

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
    /// What the frame says, one string to a row of the screen. The whole
    /// screen rather than the list alone: a drag is over the terminal, and
    /// what it covers is whatever was drawn there.
    drawn: RefCell<Vec<String>>,
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
pub(super) fn selected_text(drawn: &[String], from: (u16, u16), to: (u16, u16)) -> String {
    let (from, to) = in_reading_order(from, to);
    (from.1..=to.1)
        .map(|row| {
            let said = drawn.get(row as usize).map_or("", String::as_str);
            let (first, last) = span(row, from, to);
            said.chars()
                .skip(first as usize)
                .take((last - first) as usize + 1)
                .collect::<String>()
                .trim_end()
                .to_string()
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
    // What this frame put where, for the mouse to read back.
    screen.map.keep(
        (!helping).then_some(middle),
        first_drawn(&screen.list, middle.height),
        (carding > 0).then_some(carded),
    );
    match &screen.mode {
        Mode::Keys => help(frame, middle, &screen.page, &screen.bound),
        // The whole band, card or no card: the rows are laid out as if none
        // were up, and the card is drawn over the last of them.
        _ => agents(
            frame,
            &screen.list,
            middle,
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
    // saying is kept for the release to read the text back off.
    let buffer = frame.buffer_mut();
    if let Some((from, to)) = screen.selection {
        reverse(buffer, from, to);
    }
    let area = buffer.area;
    screen.map.drawn.replace(
        (area.top()..area.bottom())
            .map(|row| {
                (area.left()..area.right())
                    .map(|column| buffer[(column, row)].symbol())
                    .collect()
            })
            .collect(),
    );
}

/// Turn the cells a selection covers about, so somebody dragging can see what
/// they have.
fn reverse(buffer: &mut ratatui::buffer::Buffer, from: (u16, u16), to: (u16, u16)) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Three rows of a wall, each the width the frame was drawn at: what a
    /// draw leaves behind for the mouse to read a selection out of.
    fn drawn() -> Vec<String> {
        vec![
            " ● fix-login-a1b   wrote the parser  ".to_string(),
            " ● port-import-b2c did what was asked".to_string(),
            " ".repeat(37),
        ]
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
}
