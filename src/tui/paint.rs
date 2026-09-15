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
}
