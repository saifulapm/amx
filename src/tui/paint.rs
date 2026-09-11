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
use ratatui::widgets::Paragraph;
use std::cell::Cell;

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
pub use card::{Body, Card, Scroll, body_width};
pub use header::title;
/// The table the keys overlay is drawn from, for the test up in the view that
/// presses everything a terminal can send and holds what acted against it.
#[cfg(test)]
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
    /// The band the card stands in, where one is up.
    card: Cell<Option<Rect>>,
}

impl Map {
    fn keep(&self, list: Option<Rect>, offset: usize, card: Option<Rect>) {
        self.list.set(list);
        self.offset.set(offset);
        self.card.set(card);
    }

    /// How wide the band the list was drawn in is, which is the width the card
    /// under it has to wrap its words to. Nothing before the first frame.
    pub(super) fn width(&self) -> Option<u16> {
        self.list.get().map(|band| band.width)
    }

    /// The line of the list under this point, as an index into the items.
    ///
    /// The card is a band of its own under the list rather than a row of it, so
    /// a point on it names no line and no line of the list stands anywhere but
    /// where it would stand with no card up. What comes back can run past the
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

    // Every band that is not the list or the card: the header, the space under
    // it, the keys, and the permission row. What the card may take is measured
    // against what is left, so it can never be so tall that the list it was
    // opened from is gone.
    let chrome = head + space + 1 + allowing;
    let carding = match (helping, &screen.card) {
        (false, Some(card)) => card_height(
            area.height,
            area.height.saturating_sub(chrome),
            card_rows(card, showing, prs, screen.answering(), area.width),
        ),
        _ => 0,
    };
    // And the composer under the card takes what is left of the same room:
    // the rows under it, and the line itself counted at the one row it never
    // goes below.
    let chrome = chrome + carding;
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

    let [top, _, middle, carded, line, offered, allowed, keys] = Layout::vertical([
        Constraint::Length(head),
        Constraint::Length(space),
        Constraint::Min(1),
        Constraint::Length(carding),
        Constraint::Length(composing),
        Constraint::Length(offering),
        Constraint::Length(allowing),
        Constraint::Length(1),
    ])
    .areas(area);

    frame.render_widget(Paragraph::new(header(screen, top)), top);
    // How many rows the list has, told back to it the way the map and the
    // scroll are: the fold in the completed group is cut to this, by the next
    // rebuild rather than under the frame being drawn. The rows the card
    // stands on are counted in, because the card stands over the list rather
    // than taking rows off it: what is laid out is the wall as it is with no
    // card up, and the card covers the foot of it. So opening one folds
    // nothing and moves nothing but the scroll that keeps the cursor's row
    // above the card.
    screen.list.fit((middle.height + carding) as usize);
    // What this frame put where, for the mouse to read back.
    screen.map.keep(
        (!helping).then_some(middle),
        first_drawn(&screen.list, middle.height),
        (carding > 0).then_some(carded),
    );
    match &screen.mode {
        Mode::Keys => help(frame, middle, &screen.page),
        // The card stands under the list rather than among the rows, so every
        // row is drawn where it would stand with no card up at all.
        _ => agents(
            frame,
            &screen.list,
            middle,
            Moment {
                beat: screen.beat,
                armed: screen.armed(),
                swept: screen.swept(),
                hover: screen.hover,
                lent: screen.lent.as_deref(),
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
            // What the list calls it, which is what its rule says. The id
            // where the list has lost the agent the card was taken from, so
            // the rule is never bare.
            on.map_or(card.id.as_str(), rows::called),
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
}
