//! Drawing the view.
//!
//! Four bands, top to bottom: what there is, the agents themselves, the line
//! somebody is typing when they are typing one, and the keys. Everything here
//! is a function of what it is handed, so what the screen says can be read
//! back in a test without a terminal anywhere near it.
//!
//! A surface to a file, and this one only stands them next to each other:
//! [`mod@header`] draws the two bands above the list, [`wall`] the agents
//! themselves, [`empty`] what stands there when there are none, [`card`] the
//! closer look floated over them, [`input`] the line being typed and the keys
//! under it, and [`mod@help`] the screen of every key. Under all of those,
//! [`text`] measures and cuts what a row says and [`style`] turns what a thing
//! means into the paint that says so.
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
mod empty;
mod header;
mod help;
mod input;
mod style;
mod text;
mod wall;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::widgets::Paragraph;
use std::cell::Cell;

use super::rows;
use super::{Mode, Screen};
use card::{card_height, card_rows, float, over};
use header::{header, header_rows, space_rows};
use help::help;
use input::{composer_height, composing_line, footer, permission};
use wall::{Moment, agents, first_drawn};

#[cfg(test)]
pub(super) use card::walks;
pub use card::{Body, Card, Scroll};
pub use header::title;
pub use input::Notice;

/// Every key, for whoever asked what they are.
///
/// Every key the view binds, and the words it is bound under: a key column
/// that names two keys names both, because what a person looks for here is
/// the one they pressed. A test presses everything a terminal can send and
/// holds what acted against this table, so a binding that is not here is a
/// binding the screen would have to grow a row for.
///
/// In the order [`help::GROUPS`] stands them in, which is the order they are
/// drawn: one table, cut into runs, so a key is in exactly one place and the
/// test that walks every key walks every group with it.
///
/// It stands in this file rather than beside the overlay it is drawn on
/// because the table is not public to the rest of the crate, and the test that
/// checks the README against it reads this path as text.
pub(super) const HELP: [(&str, &str); 29] = [
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

/// Where the last frame put things, written back by a draw that is otherwise
/// a pure reading of the view, because the mouse arrives in the screen's own
/// coordinates: the band the rows were drawn in, which item its first row
/// held, and where the card floats. Cells, for the reason [`Scroll`]'s are.
#[derive(Default)]
pub struct Map {
    /// The band the list was drawn in, and nothing while the keys overlay
    /// has it: a screen of keys has no rows under the pointer.
    list: Cell<Option<Rect>>,
    /// The item index of the band's first drawn row.
    offset: Cell<usize>,
    /// The card's floating box, where one is up.
    card: Cell<Option<Rect>>,
}

impl Map {
    fn keep(&self, list: Option<Rect>, offset: usize, card: Option<Rect>) {
        self.list.set(list);
        self.offset.set(offset);
        self.card.set(card);
    }

    /// The line of the list under this point, as an index into the items.
    ///
    /// The card is in front of the rows it covers, so a point on it names no
    /// line. What comes back can run past the end of the items — the band is
    /// taller than the list — and the caller holds the bound, because only it
    /// has the items.
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

    /// Whether this point is on the floating card.
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
    // Every band that is not the list: the header, the space under it, the
    // keys, the rows under the composer, and the line itself counted at the
    // one row it never goes below.
    let chrome = head + space + 1 + allowing;
    let composing = match banded {
        Some(composer) => composer_height(composer, area, chrome),
        None => 0,
    };

    let [top, _, middle, line, allowed, keys] = Layout::vertical([
        Constraint::Length(head),
        Constraint::Length(space),
        Constraint::Min(1),
        Constraint::Length(composing),
        Constraint::Length(allowing),
        Constraint::Length(1),
    ])
    .areas(area);

    frame.render_widget(Paragraph::new(header(screen, top)), top);
    // The reading behind the card, for the two things the card needs and does
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
    let floating = match (helping, &screen.card) {
        (false, Some(card)) => card_height(
            area.height,
            middle.height,
            card_rows(
                card,
                showing,
                prs,
                screen.answering().is_some(),
                middle.width,
            ),
        ),
        _ => 0,
    };
    let visible = middle.height - floating;
    let card_over = (floating > 0).then(|| over(middle, floating));
    // How many rows the list has in front of the card, told back to it the
    // way the map and the scroll are: the fold in the completed group is cut
    // to this, by the next rebuild rather than under the frame being drawn.
    screen.list.fit(visible as usize);
    // What this frame put where, for the mouse to read back.
    screen.map.keep(
        (!helping).then_some(middle),
        first_drawn(&screen.list, visible),
        card_over,
    );
    match &screen.mode {
        Mode::Keys => help(frame, middle),
        // What the card is covering is still drawn under it, and the rows the
        // cursor walks are the ones it is not.
        _ => agents(
            frame,
            &screen.list,
            middle,
            Moment {
                beat: screen.beat,
                armed: screen.armed(),
                hover: screen.hover,
            },
            visible,
            theme,
        ),
    }
    if let Some(floated) = card_over
        && let Some(card) = &screen.card
    {
        float(
            frame,
            card,
            showing,
            prs,
            screen.answering(),
            &screen.scroll,
            floated,
            theme,
        );
    }
    if let Some(composer) = banded {
        composing_line(frame, composer, line, theme);
    }
    if let Some(row) = permission {
        frame.render_widget(Paragraph::new(row), allowed);
    }
    frame.render_widget(Paragraph::new(footer(screen, keys.width)), keys);
}
