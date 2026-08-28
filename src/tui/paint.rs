//! Drawing the view.
//!
//! Four bands, top to bottom: what there is, the agents themselves, the line
//! somebody is typing when they are typing one, and the keys. Everything here
//! is a function of what it is handed, so what the screen says can be read
//! back in a test without a terminal anywhere near it.
//!
//! A closer look at one agent is not a band. It is a card floated over the
//! bottom of the list, because it is about one row of a list that is still
//! there behind it — and because what a person does with it is answer the
//! question on it and go back to the wall.
//!
//! A row is one line, always: an agent's answer is a paragraph, and a
//! paragraph in a list is how a list stops being one.
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
//! done, failed — and which colour that is comes off the [`Theme`] the screen
//! carries, so a person's palette reaches every one of these without any of
//! them knowing there is such a thing as a palette. Most of the screen is
//! painted in none of it: a wall where everything is coloured is a wall where
//! the colour says nothing.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};
use std::cell::Cell;
use std::ops::Range;
use std::sync::OnceLock;

use super::act::{self, Asking, Composer};
use super::grid::{self, Widths};
use super::rows::{self, Group, Item, List, Showing, Tally, Under};
use super::{Mode, Profile, Screen};
use crate::ansi::{self, Colour, Painted};
use crate::derive::{self, View};
use crate::furniture::cut;
use crate::pr::{Pr, Standing};
use crate::registry::DEFAULT;
use crate::store::{Kind, Phase};
use crate::theme::Theme;
use crate::verbs::send::numbered;

/// The key the hint row keeps whatever else it has to shed, because the
/// overlay behind it is where every key is.
const MORE: &str = "? keys";

/// What the card's own keys do, under the card, while it is holding a line.
///
/// What may be typed *into* that line is the question's business and is said
/// on the line itself. Only these two are offered: alt+enter puts a newline in
/// the line like anywhere else in the view, and a prompt that reads one key
/// would refuse whatever a newline was typed into, so a row that named it
/// would be naming a key that cannot work where it was read.
const ANSWERS: &str = "enter answers it · esc closes it";

/// Every key, for whoever asked what they are.
///
/// Every key the view binds, and the words it is bound under: a key column
/// that names two keys names both, because what a person looks for here is
/// the one they pressed. A test presses everything a terminal can send and
/// holds what acted against this table, so a binding that is not here is a
/// binding the screen would have to grow a row for.
///
/// In the order [`GROUPS`] stands them in, which is the order they are drawn:
/// one table, cut into runs, so a key is in exactly one place and the test
/// that walks every key walks every group with it.
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

/// What the keys are for, and how many of [`HELP`] each of those answers for.
///
/// A flat list of twenty-nine is a list somebody reads all of to find one, so
/// the table is cut into what a person is trying to do: get about the wall,
/// read one agent, put work in, arrange what is already there, and set what
/// the next agent runs. Five short lists are five places to not look.
///
/// Runs rather than tables of their own, so nothing here can hold a key twice
/// or drop one between two headings.
const GROUPS: [(&str, usize); 5] = [
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

/// What the view has to say for itself, and how loudly.
///
/// Two channels in the one slot at the foot of the screen, a severity apart:
/// an action that was attempted and failed is louder than a refusal or a piece
/// of advice. A view that paints "nothing was deleted" the same red as a git
/// error is teaching people to read neither.
pub enum Notice {
    /// It was attempted and it failed.
    Failed(String),
    /// Advice, or a refusal that is not a failure.
    Advice(String),
}

/// A closer look at one agent, as the card floated over the list.
///
/// A card carries its body in one of two states, which is what `B` says. A
/// card is *built* from text — a pane capture, a recorded answer, a patch —
/// and it is *drawn* from [`Body`], that text already walked out of its
/// escapes. Everything the paint takes is the second: the walk happens once,
/// where the card is made, and no frame pays for it again.
pub struct Card<B = String> {
    pub id: String,
    pub phase: Phase,
    /// How long since anything was heard from it, so the card says how old the
    /// question on it is without anybody going back to the row.
    pub age: u64,
    /// What it is waiting to be told, when it is waiting to be told anything.
    pub question: Option<String>,
    /// The choices that question offers, in the order the screen lists them.
    pub options: Vec<String>,
    /// What kind of question it is, which is what decides the answers it will
    /// take.
    pub kind: Option<Kind>,
    /// The screen it is sitting on, the answer it left behind, or what it has
    /// changed.
    pub body: B,
    /// Whether the body is that diff, which is read from the top down rather
    /// than from the bottom up.
    pub changes: bool,
    /// Whether the body is the answer the record holds — a turn's own words,
    /// whole, rather than a picture of the pane it was said on.
    pub answer: bool,
}

impl<B> Card<B> {
    /// Whether this card is one somebody can answer. A patch is not a
    /// question, and neither is a look at an agent that is getting on with it.
    pub fn asks(&self) -> bool {
        !self.changes && self.phase == Phase::Waiting
    }

    /// Whether the body reads forward, from its top: a patch does, and so
    /// does a recorded answer. Only a live screen is read up from its
    /// bottom, where the newest of it is.
    pub fn forward(&self) -> bool {
        self.changes || self.answer
    }
}

impl Card<String> {
    /// The same card with its body read, which is the form the paint draws.
    ///
    /// For a card built out of text somebody already holds, which is what a
    /// patch is. A card built from a record or a pane walks the words where it
    /// takes them, and never makes the copy this one is handed.
    pub fn read(self) -> Card<Body> {
        Card {
            // A patch is amx's own reading of a repository, not a pane; a
            // recorded answer and a finished agent's last words are whole,
            // with no vendor furniture under them; and what is left is a
            // picture of a pane somebody is still working in.
            body: match (self.changes, self.answer || self.phase.is_terminal()) {
                (true, _) => Body::patch(&self.body),
                (_, true) => Body::said(&self.body),
                _ => Body::screen(&self.body),
            },
            id: self.id,
            phase: self.phase,
            age: self.age,
            question: self.question,
            options: self.options,
            kind: self.kind,
            changes: self.changes,
            answer: self.answer,
        }
    }
}

/// A card's body, walked out of its escapes once — when the card was built.
///
/// The rows are ready to draw: neutralised, in the paint the vendor drew them
/// in, with amx's own text dimmed. A frame windows them and nothing else, so
/// an open card costs a redraw the same whether it is holding four rows of
/// answer or four thousand of patch.
pub struct Body {
    /// Every row of it, in order.
    rows: Vec<Line<'static>>,
    /// How many of them the card reads from its natural edge: the vendor's
    /// own furniture is off the end of a live capture, and the blank rows a
    /// pane is padded out with are off the end of everything.
    kept: usize,
    /// Whether the cut took furniture off. A pane holding nothing but the
    /// vendor's own chrome is a different fact from an agent that has said
    /// nothing yet, and the card says the first out loud.
    chrome: bool,
}

impl Body {
    /// Nothing under everything else, which is what a card holding a question
    /// has.
    pub(super) fn none() -> Body {
        Body {
            rows: Vec::new(),
            kept: 0,
            chrome: false,
        }
    }

    /// A patch: amx's own reading of a repository rather than a pane, so there
    /// is no paint on it to keep and no furniture under it to cut.
    pub(super) fn patch(text: &str) -> Body {
        let rows: Vec<Line<'static>> = text
            .lines()
            .map(|text| Line::styled(inert(text), dim()))
            .collect();
        Body {
            kept: rows.len(),
            rows,
            chrome: false,
        }
    }

    /// A live pane, in the paint the vendor drew it in, with the vendor's own
    /// furniture cut off the bottom.
    pub(super) fn screen(text: &str) -> Body {
        Body::walk(text, true)
    }

    /// What an agent said: a recorded answer, or whatever an agent whose
    /// command has ended left behind. Nothing is cut off it — there is no
    /// pane under it to hold furniture.
    pub(super) fn said(text: &str) -> Body {
        Body::walk(text, false)
    }

    /// The walk itself. `live` is whether the text came off a pane the vendor
    /// is still drawing on, which is the only body the furniture cut is taken
    /// off.
    fn walk(text: &str, live: bool) -> Body {
        #[cfg(test)]
        WALKS.with(|walks| walks.set(walks.get() + 1));
        // The escapes are walked into styling here and nowhere else, so
        // nothing downstream of this line is holding a control sequence.
        let read = ansi::painted(text);
        let said: Vec<String> = read.iter().map(|row| words(row)).collect();
        let plain: Vec<&str> = said.iter().map(String::as_str).collect();
        // What the vendor drew on, with its own furniture off the bottom.
        let drawn = match live {
            true => cut(&plain).len(),
            false => plain.len(),
        };
        // The blank rows a pane is padded out with go the same way, so what
        // is left ends on the last row anybody wrote on: the edge both ends
        // of the body are measured from.
        let mut kept = drawn;
        while kept > 0 && plain[kept - 1].trim().is_empty() {
            kept -= 1;
        }
        Body {
            rows: read.iter().map(|row| as_painted(row)).collect(),
            kept,
            chrome: drawn < plain.len(),
        }
    }

    /// How many rows it has to give a card, which is what the last page is
    /// measured against. The one row the card says it found nothing but
    /// furniture on counts: it is a row, and a card of one row does not page.
    fn length(&self) -> usize {
        self.kept.max(usize::from(self.chrome))
    }

    /// What the body says, for the tests that ask a card what it is holding.
    #[cfg(test)]
    pub(super) fn says(&self) -> String {
        self.rows
            .iter()
            .map(|row| {
                row.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Where the card's body stands against its natural edge — the bottom of a
/// screen or an answer, the top of a patch — and how far one page is.
///
/// The keys add and subtract; the paint owns the clamp, because only the
/// paint knows how many rows the body was given. Cells, so a draw that is
/// otherwise a pure reading of the view can write back what it kept: a body
/// that fits is pinned to its edge, and a press past the end lands on the
/// last page rather than beyond it.
#[derive(Default)]
pub struct Scroll {
    /// Rows between what the card shows and the body's natural edge.
    pub away: Cell<usize>,
    /// The rows the body had last frame, which is what one press moves by.
    pub page: Cell<usize>,
}

impl Scroll {
    /// Clamp the offset to the last page this body and window allow, remember
    /// what a page is, and say where the card now stands.
    fn kept(&self, length: usize, window: usize) -> usize {
        let away = self.away.get().min(length.saturating_sub(window));
        self.away.set(away);
        self.page.set(window.max(1));
        away
    }
}

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

/// How much of the screen the card takes: what it has to show, up to about
/// half, and never so much that the list it was opened from is gone.
///
/// What it has to show comes into it because a card is over a wall somebody is
/// reading: an agent whose answer is one line does not need seven rows of box
/// to say it in, and every row the card does not take is a row of the list
/// still on the screen. Below the room for its two borders and a row between
/// them there is no card at all.
fn card_height(total: u16, band: u16, wanted: u16) -> u16 {
    let room = (total / 2)
        .clamp(CARD_SHORT, CARD_TALL)
        .min(wanted.max(CARD_SHORT))
        .min(band.saturating_sub(1));
    match room >= CARD_SHORT {
        true => room,
        false => 0,
    }
}

/// How many rows the card would take to say everything it has: its two
/// borders, what its branch has open, which question of the call this is, what
/// the agent is asking, the choices under that, the row the vendor adds under
/// them, the line the answer goes on, and the screen it is all happening on.
fn card_rows(
    card: &Card<Body>,
    showing: Option<Showing>,
    prs: &[Pr],
    answering: bool,
    width: u16,
) -> u16 {
    let inner = width.saturating_sub(2 + 2 * PADDING);
    let asked = card
        .question
        .as_deref()
        .map_or(0, |question| wrapped(question, inner).min(ASKED_TALL));
    let listed = choices(&card.options, inner as usize, boxed(showing)).len();
    // Counted no further than the card could ever grow: the body can be a
    // patch of thousands of rows, and this runs on every frame.
    let shown = length(card).min(CARD_TALL as usize);

    let rows = 2
        + usize::from(!prs.is_empty())
        + asked as usize
        + usize::from(tab(showing).is_some())
        + listed
        + usize::from(added(card, showing).is_some())
        + usize::from(answering)
        + shown;
    rows.min(u16::MAX as usize) as u16
}

/// The bottom `height` rows of the list, which is where the card floats.
///
/// The bottom because a list is read from the top: the rows above it are the
/// ones the cursor is kept among, and the card stands between them and the row
/// it was opened from rather than over the whole wall.
fn over(band: Rect, height: u16) -> Rect {
    Rect {
        y: band.y + band.height - height,
        height,
        ..band
    }
}

/// The two borders of the card and one row between them.
const CARD_SHORT: u16 = 3;

/// And the most of a screen it will take, however tall the terminal is.
const CARD_TALL: u16 = 14;

/// The column of air inside each border, so what the card says is not written
/// against the box it is written in.
const PADDING: u16 = 1;

/// Below this many rows the header is what there is and nothing else. Two rows
/// of chrome over a screen that short is a third of it, and the list is what
/// the view is for.
const SHORT: usize = 10;

/// From this many rows up there is a blank one between the header and the
/// list. The groups stand off from each other that way, and the first of them
/// has the chrome above it rather than nothing at all.
///
/// It is the first row to go on a screen running out of them, for the reason
/// [`SHORT`] is a rule: four rows of chrome over ten of terminal is most of
/// what a person opened the view to read, and a row of air is worth less than
/// a row of agents.
const SPACED: usize = 12;

/// Fewer columns than this left for a directory and it is not on the row at
/// all: a path cut to three characters is not a path.
const SHORTEST_DIR: usize = 8;

/// How many rows the header takes at this height.
fn header_rows(height: u16) -> u16 {
    match (height as usize) < SHORT {
        true => 1,
        false => 2,
    }
}

/// And how many stand between it and the list.
fn space_rows(height: u16) -> u16 {
    u16::from((height as usize) >= SPACED)
}

/// What is above the list: what there is, and under it what the next agent
/// will be started with.
///
/// Two rows where there is room for two, and one kind of thing on each. The
/// first is the present tense — the tool's name, the directory the view was
/// opened on, what the fleet is doing, and the count that wants a person set in
/// reverse video at the far end of it. The second hangs off it on a branch
/// glyph and holds every dial.
///
/// The row that goes on a short screen is the second: the count that wants
/// somebody is why anybody opened the view, and a dial is one keypress from
/// being read in the row under the composer.
fn header(screen: &Screen, area: Rect) -> Vec<Line<'static>> {
    let width = area.width as usize;
    // The fleet's half is worked out first: it is what there is, and the name
    // and the directory are what fit beside it.
    let fleet = fleet(screen, width);
    let room = width.saturating_sub(said(&fleet) + 1);
    let mut lines = vec![spread(here(&screen.profile, room), fleet, width)];
    if area.height >= 2 {
        lines.push(Line::from(dials(&screen.profile, width, screen.theme)));
    }
    lines
}

/// The right of band 1: what the fleet is doing, why the list is short where it
/// is short, and the count that wants a person.
///
/// The badge and the narrowing are about the screen in front of somebody — the
/// one number they opened it for, and the words that say why what is under it
/// is holding less than it has. The counts are readings about a fleet. So the
/// counts are what goes where the row will not hold all three and the name as
/// well: a narrow terminal that answered every question but the one it was
/// opened for would be answering nobody.
fn fleet(screen: &Screen, width: usize) -> Vec<Span<'static>> {
    // What the list was narrowed to, in the words it was narrowed with, so
    // somebody who has forgotten why it is short can read why.
    let mut kept = match screen.list.narrowing() {
        Some(narrowing) => vec![Span::styled(format!("{narrowing}{APART}"), dim())],
        None => Vec::new(),
    };
    kept.extend(badge(&screen.list, screen.theme));

    let counts = counters(&screen.list, screen.profile.max);
    let together = said(&counts) + APART.chars().count() + said(&kept) + NAME.chars().count() + 1;
    match together <= width {
        true => [counts, vec![Span::raw(APART)], kept].concat(),
        false => kept,
    }
}

/// How many agents are waiting on somebody.
fn waiting(list: &List) -> usize {
    list.counts()
        .iter()
        .find(|(group, _)| *group == Group::NeedsInput)
        .map_or(0, |&(_, count)| count)
}

/// The one number the view is opened to read, in the one treatment nothing
/// else on the screen wears.
///
/// Reverse video in the waiting colour, with a space either side of the words
/// so it reads as a block rather than as a phrase somebody has coloured in.
/// Everything else above the list is a fact about a fleet; this is a fact
/// about the person reading it, and it is the whole of what the one-second
/// test rests on.
///
/// At zero it is the words `nothing waiting`, dim, in the same place: the
/// question is asked every time the screen is looked at, and an answer that
/// vanished when it was `no` would leave somebody reading the row to find out
/// whether it had been drawn yet.
fn badge(list: &List, theme: Theme) -> Vec<Span<'static>> {
    match waiting(list) {
        0 => vec![Span::styled(NOBODY, dim())],
        count => vec![Span::styled(
            format!(" {count} WAITING "),
            Style::new()
                .fg(theme.waiting)
                .add_modifier(Modifier::REVERSED | Modifier::BOLD),
        )],
    }
}

/// What stands where the badge stands when nothing is waiting.
const NOBODY: &str = "nothing waiting";

/// Two blocks on one row, the left where it starts and the right against the
/// far edge, with at least one column between them.
///
/// The right block is what goes when they will not both fit: two blocks
/// touching read as one, and half a sentence pushed off the screen reads as a
/// word that ends where the terminal does. Where the left block has already
/// given up every column it had, the right one has the row rather than the row
/// being drawn blank.
fn spread(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut spans = left;
    match width.checked_sub(said(&spans) + said(&right)) {
        Some(gap) if gap >= 1 => {
            spans.push(Span::raw(" ".repeat(gap)));
            spans.extend(right);
        }
        _ if said(&spans) == 0 => return Line::from(right),
        _ => {}
    }
    Line::from(spans)
}

/// How many columns a block of spans takes.
fn said(spans: &[Span<'static>]) -> usize {
    spans.iter().map(|span| span.content.chars().count()).sum()
}

/// Whose screen this is and where it was opened, which is where an agent
/// started from here will run.
///
/// The name is never cut and the directory is: `AMX` is three columns and the
/// one word that says what somebody is looking at, and a path cut to a few
/// characters is not a path.
///
/// The name carries weight and the directory does not, which is the whole of
/// the hierarchy in the corner: one is what the screen is and the other is a
/// fact about this run of it.
fn here(profile: &Profile, room: usize) -> Vec<Span<'static>> {
    let name = Span::styled(fit(NAME, room), Style::new().add_modifier(Modifier::BOLD));
    let left = room.saturating_sub(NAME.chars().count() + BESIDE.chars().count());
    match !profile.dir.is_empty() && left >= SHORTEST_DIR {
        true => vec![
            name,
            Span::styled(format!("{BESIDE}{}", fit(&profile.dir, left)), dim()),
        ],
        false => vec![name],
    }
}

/// What the view is called, which is what the top left corner of it says.
const NAME: &str = "AMX";

/// What stands between the name and the directory under it.
const BESIDE: &str = "  ";

/// What stands between two things said on one band above the list.
///
/// Three columns rather than a glyph: the counts are a list of readings with
/// nothing between them to say, and air separates them without adding a fourth
/// kind of mark to a screen that already carries three.
const APART: &str = "   ";

/// What every dial the next agent will be started with says, in one row.
///
/// The row hangs off the one above it on a [`BRANCH`] in the first column,
/// which is what marks it as being about an agent that has not been started:
/// a glyph a person reads as subordinate without a word of explanation, where
/// a label at the front used to say it. The labels are dim and the values
/// carry the accent, because the value is the reading and the label is the
/// question a person already knows the order of.
///
/// A dial its vendor does not declare is not on the row at all. One resting
/// where the vendor left it reads `default`, which is the vendor's own answer
/// said as a value rather than as a hole in the row.
///
/// Where every label will not fit they all go but `next`, and the pairs are
/// separated by a mark instead: the order of four dials is learned once, and
/// the values are what change. An `agent` is a command line, and a command is
/// routinely a long one — it takes the columns the dials beside it leave, but
/// never fewer than [`SHORTEST_AGENT`] of them, because which program runs is
/// the first thing this row is for.
fn dials(profile: &Profile, width: usize, theme: Theme) -> Vec<Span<'static>> {
    let mut pairs: Vec<(&'static str, String)> = Vec::new();
    if profile.model_dial().is_some() {
        pairs.push(("model", profile.model.clone()));
    }
    if profile.permission_dial().is_some() {
        pairs.push(("permission", profile.permission.clone()));
    }
    pairs.push((
        "worktree",
        match profile.worktree {
            true => TREE,
            false => NO_TREE,
        }
        .to_string(),
    ));

    // What the row costs before the vendor's own value is written into it,
    // with the labels and without them.
    let chrome = |labelled: bool| {
        BRANCH.chars().count()
            + NEXT.chars().count()
            + BESIDE.chars().count()
            + pairs
                .iter()
                .map(|(label, at)| {
                    at.chars().count()
                        + match labelled {
                            true => {
                                APART.chars().count()
                                    + label.chars().count()
                                    + BESIDE.chars().count()
                            }
                            false => MARKED.chars().count(),
                        }
                })
                .sum::<usize>()
    };
    let labelled = chrome(true) + SHORTEST_AGENT <= width;
    let room = width.saturating_sub(chrome(labelled)).max(SHORTEST_AGENT);

    let turned = Style::new().fg(theme.accent);
    let mut spans = vec![
        Span::styled(format!("{BRANCH}{NEXT}{BESIDE}"), dim()),
        Span::styled(fit(&profile.agent, room), turned),
    ];
    for (label, at) in pairs {
        spans.push(Span::styled(
            match labelled {
                true => format!("{APART}{label}{BESIDE}"),
                false => MARKED.to_string(),
            },
            dim(),
        ));
        spans.push(Span::styled(at, turned));
    }
    clipped(spans, width)
}

/// Fewer columns than this for the vendor and the dials give way instead: a
/// command cut to three characters is not a command, and a row that had put
/// every dial on the screen by leaving off what runs would be a row about
/// nothing.
const SHORTEST_AGENT: usize = 8;

/// What hangs the dials off the row above them.
const BRANCH: &str = "└ ";

/// What the first dial is called, which is the one label the row never sheds.
const NEXT: &str = "next";

/// What the worktree dial reads at either end of its travel.
const TREE: &str = "new";
const NO_TREE: &str = "none";

/// What stands between two dials on a row with no room to name them.
const MARKED: &str = "  ·  ";

/// What stands between two things said on one row.
const SEPARATOR: &str = " · ";

/// A row of spans cut to the columns there are.
///
/// The last span standing is the one that carries the cut, so the end of the
/// row says it was cut the way the end of any other cut thing on the screen
/// does.
fn clipped(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    if said(&spans) <= width {
        return spans;
    }
    let mut left = width;
    let mut kept = Vec::new();
    for span in spans {
        if left == 0 {
            break;
        }
        let taken = span.content.chars().count();
        if taken <= left {
            left -= taken;
            kept.push(span);
            continue;
        }
        kept.push(Span::styled(fit(&span.content, left), span.style));
        left = 0;
    }
    kept
}

/// What the fleet is: a count per group, in the word the list can be narrowed
/// by, and the gate the next agent will meet.
///
/// Every group but the one the badge beside it is already counting, and no
/// colour on any of them: a count is a reading about a fleet, and a row where
/// four readings are coloured is a row where the one that wants a person is
/// not.
fn counters(list: &List, max: usize) -> Vec<Span<'static>> {
    let mut said: Vec<String> = list
        .counts()
        .iter()
        .filter(|(group, _)| *group != Group::NeedsInput)
        .map(|&(group, count)| format!("{count} {}", group.state()))
        .collect();

    // The limit that refuses a spawn, said before it refuses one.
    said.push(format!("{}/{max} running", list.live()));
    vec![Span::styled(said.join(APART), dim())]
}

/// What the terminal the view is drawn on is called: the program, and how many
/// agents are waiting on somebody where any are.
///
/// The waiting count and nothing else. A title is read out of the corner of an
/// eye, from a tab bar or a window list with the terminal behind something
/// else, and the one thing worth pulling a window forward for is an agent that
/// has stopped and cannot go on. What is merely running does not need a person
/// and does not go here; the wall itself says the rest.
///
/// Counted as the list has it, which is what the header counts too: a view
/// opened about one directory is a question about those agents, and a title
/// answering a wider one would be answering a question nobody on this screen
/// asked.
pub fn title(list: &List) -> String {
    match waiting(list) {
        0 => "amx".to_string(),
        count => format!("amx{SEPARATOR}{count} waiting"),
    }
}

/// What a wall nobody has put anything on says for itself.
///
/// One line of amx's own, where four headings with a sentence each used to
/// stand: there is nothing to read off the rows, and a view that explains the
/// list before there is a list is doing the manual's job on the screen a
/// person came to work at. What is worth knowing about this wall is that it is
/// the good one, and the keys under it already say which one starts an agent.
const WELCOME: &str = "nothing running, nothing broken, nobody asking. enjoy it";

/// The agents themselves.
///
/// `visible` is how many of the rows are not behind the card, and it is what
/// the cursor is kept inside. The rest are drawn anyway: a card is in front of
/// a list, not instead of one, and the rows it covers are the ones somebody
/// gets back by closing it.
fn agents(frame: &mut Frame, list: &List, area: Rect, moment: Moment, visible: u16, theme: Theme) {
    if list.is_empty() {
        // Nothing to show is one thing while a narrowing is holding every
        // agent back, another while nobody has started one, and the line for
        // the second of those is a sentence rather than a label — so it is
        // said whole or not at all, and a screen too narrow for it gets the
        // label instead of two thirds of a joke.
        let room = area.width as usize >= WELCOME.chars().count();
        let said = match (list.narrowing(), list.unstarted() && room) {
            (Some(narrowing), _) => format!("nothing matches {narrowing}"),
            (None, true) => WELCOME.to_string(),
            (None, false) => "no agents".to_string(),
        };
        frame.render_widget(Paragraph::new(Line::styled(said, dim())), area);
        return;
    }

    let height = area.height as usize;
    let offset = first_drawn(list, visible);
    let width = area.width as usize;
    let widths = grid::widths(width, list.axis());
    let requests = request_column(list);

    let lines: Vec<Line> = list
        .items()
        .iter()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(at, item)| {
            line(
                list,
                *item,
                At {
                    selected: at == list.cursor(),
                    hovered: moment.hover == Some(at),
                },
                widths,
                requests,
                width,
                moment,
                theme,
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// The first item a band this tall draws: enough of the top scrolled away to
/// keep the cursor on the screen, and in front of the card rather than behind
/// it. Shared with the map the mouse reads, so a click lands on the row the
/// frame actually drew there.
fn first_drawn(list: &List, visible: u16) -> usize {
    list.cursor()
        .saturating_sub((visible.max(1) as usize).saturating_sub(1))
}

/// What the clock has made of the list at the moment it is drawn: which frame
/// of the working pulse the rows are on, and which of them a press has armed —
/// one row, or every finished row under the heading the press was on.
///
/// Neither is a fact about an agent, and neither is worth writing down: they
/// are what the view is doing while somebody watches it, so they are handed to
/// the rows and forgotten with the frame.
#[derive(Clone, Copy)]
struct Moment<'a> {
    beat: usize,
    armed: &'a [String],
    /// The line the pointer is resting on, if it is resting on an agent's.
    hover: Option<usize>,
}

/// How the cursor and the pointer stand to one line: on it, or over it.
#[derive(Clone, Copy, Default)]
struct At {
    selected: bool,
    hovered: bool,
}

/// One line of the list, whatever kind of line it is.
#[allow(clippy::too_many_arguments)]
fn line(
    list: &List,
    item: Item,
    at: At,
    widths: Widths,
    requests: usize,
    width: usize,
    moment: Moment,
    theme: Theme,
) -> Line<'static> {
    let line = match item {
        Item::Heading(under, tally) => match under {
            Under::Group(group) => heading(group, tally, widths, width, theme),
            // A path is not a word and does not uppercase, so it is drawn its
            // own way — which is the dir axis's business, and the dir axis is
            // redrawn next.
            Under::Project(_) => path_heading(list.title(under), tally),
        },
        Item::Fold(hidden) => Line::styled(format!("{GUTTER}… {hidden} more"), dim()),
        Item::Agent(_) => match list.agent(item) {
            Some(view) => row(
                view,
                list.requests(view),
                list.holding(view),
                at,
                widths,
                requests,
                moment,
                theme,
            ),
            None => Line::raw(""),
        },
        Item::Blank => Line::raw(""),
    };
    match at.selected {
        true => barred(line, width, theme),
        false => line,
    }
}

/// The line the cursor is on, with the bar that says so under it.
///
/// A background colour the width of the list rather than a reversal of what
/// the line already says. The two look alike on a row, which is nearly as wide
/// as the list, and they part company on a heading: a reversal there marks a
/// short label, and what the cursor is on is a line. So both wear the bar, and
/// the cursor looks like one thing wherever it is.
///
/// The colour is the theme's, which by default is the vendor's own for a
/// selected line, measured from the 2.1.237 bundle for the reason the rest of
/// them are.
fn barred(line: Line<'static>, width: usize, theme: Theme) -> Line<'static> {
    let said: usize = line
        .spans
        .iter()
        .map(|span| span.content.chars().count())
        .sum();
    let mut line = line;
    if said < width {
        line.spans.push(Span::raw(" ".repeat(width - said)));
    }
    line.style(Style::new().bg(theme.cursor))
}

/// A heading: what it stands for, and what it is answerable for.
///
/// Uppercase and bold, which is what makes a heading out of a label without a
/// second type size — the only uppercase words on the screen. Then a dim rule
/// run out to the group's count, right-aligned in the column the ages under it
/// are right-aligned in, so the right edge of the screen is one line of
/// numbers rather than two. The count is there open or shut: a person reading
/// down the margin is asking how many, and a number that came and went with
/// the rows would make them count instead.
///
/// The failures are said in front of the rule, because that is the one thing a
/// heading is worth reading without opening it — an agent that failed is the
/// reason somebody came to the screen.
fn heading(
    group: Group,
    tally: Tally,
    widths: Widths,
    width: usize,
    theme: Theme,
) -> Line<'static> {
    let label = group.title().to_uppercase();
    let failures = match tally.failures {
        0 => String::new(),
        failures => format!("· {failures} failed "),
    };
    // What the rule is left: the space in front of the label, the label, the
    // space after it, the failures, and the gap and the count at the far end.
    let spent = 1 + width_of(&label) + 1 + width_of(&failures) + GAP + widths.age;
    let rule = "─".repeat(width.saturating_sub(spent).max(1));
    // The group that wants a person carries the one colour up here, on the
    // label and on the count alike. The group nothing is going to happen to
    // again takes the colour of a thing that has ended, so the margin says
    // which of its numbers is still moving.
    let (label_paint, count_paint) = match group {
        Group::NeedsInput => {
            let waiting = Style::new().fg(theme.waiting).add_modifier(Modifier::BOLD);
            (waiting, waiting)
        }
        Group::Completed => (bold(), Style::new().fg(theme.stopped)),
        _ => (bold(), dim()),
    };
    Line::from(vec![
        Span::styled(format!(" {label} "), label_paint),
        Span::styled(failures, Style::new().fg(theme.failed)),
        Span::styled(rule, dim()),
        Span::raw(" ".repeat(GAP)),
        Span::styled(
            grid::padl(&tally.members.to_string(), widths.age),
            count_paint,
        ),
    ])
}

/// The heading over a project, which is a path rather than a word: it is not
/// uppercased and it is not ruled here. The dir axis is redrawn next, and this
/// is what it was until then.
fn path_heading(title: String, tally: Tally) -> Line<'static> {
    let counted = match (tally.shut, tally.failures) {
        (false, 0) => String::new(),
        (false, failures) => format!(" · {failures} failed"),
        (true, 0) => format!(" {}", tally.members),
        (true, failures) => format!(" {} · {failures} failed", tally.members),
    };
    Line::styled(format!("{title}{counted}"), dim())
}

/// An agent's row: what state it is in, what it is called, what its work is
/// waiting on out in the world, what it is up to, and how long it has worked.
///
/// Four cells before the name — the two marks, the state glyph and the space
/// after it — then the name, the summary, and the age right-aligned at the
/// edge, all on the widths the grid fixes for the screen. Fixed rather than
/// measured off the fleet, so the columns stand where they stood when the last
/// agent ended and the row a person learned wide is the row they get narrow.
///
/// The weight goes where the work is. A row that is asking carries the one
/// colour and the one bold name on the wall, and its question is at full
/// strength because that is the sentence somebody opened the view to read.
/// Every other row is its name in the terminal's own and what it said dim
/// under it, with the state on the glyph alone. The exceptions earn their
/// colour: a failed name says so without its glyph being read, the pull
/// request's number answers how the work went, and under a project heading
/// the state word keeps the phase colour because it replaces the glyph's job
/// there. What the cursor is on is said by the bar under it, not by the row
/// changing its tones.
///
/// A row a press has armed says that instead of what the agent said, in the
/// colour of a thing waiting on a person. The summary is the one part of a row
/// amx is free to speak over: the state, the name and the age are what the row
/// is for, and a warning that took a column of its own would move every row
/// under it for as long as it was up.
#[allow(clippy::too_many_arguments)]
fn row(
    view: &View,
    prs: &[Pr],
    held: bool,
    at: At,
    widths: Widths,
    requests: usize,
    moment: Moment,
    theme: Theme,
) -> Line<'static> {
    let phase = view.phase();
    // The reading's own number and the reading's own units: a row and a table
    // that worked the words out for themselves would agree until one of them
    // was edited. The worked seconds, not the age — an idle agent's clock
    // climbing was timing the silence, and the wait stays on the card.
    let worked = derive::in_words(view.verdict.worked);
    // The one word on a row a person typed rather than amx minting it, so it
    // is neutralised here as well as where it was written down.
    let name = grid::pad(&inert(rows::called(view)), widths.name);
    // The pull request is not one of the design's columns, so it is paid for
    // the way the state word is: out of the summary, which is the column that
    // gives way. Name, age and count stay where they are whether or not there
    // is a forge on the machine.
    let room = widths.summary.saturating_sub(match requests {
        0 => 0,
        column => column + GAP,
    });
    let armed = moment.armed.iter().any(|id| id == view.id());
    let said = match armed {
        true => AGAIN.to_string(),
        false => first_line(view.line().unwrap_or("")).to_string(),
    };

    let asking = phase == Phase::Waiting;
    let [read, top] = marks(view, held, theme);
    let mut spans = vec![
        read,
        top,
        Span::styled(
            format!("{} ", icon(phase, moment.beat)),
            match asking {
                true => colour(theme, phase).add_modifier(Modifier::BOLD),
                false => colour(theme, phase),
            },
        ),
        Span::styled(
            format!("{name}{}", " ".repeat(GAP)),
            // The pointer resting on a row gives its name weight without the
            // bar or the cursor, which is the whole of what a hover is.
            match at.hovered {
                true => name_colour(theme, phase).add_modifier(Modifier::BOLD),
                false => name_colour(theme, phase),
            },
        ),
    ];
    if widths.state > 0 {
        spans.push(Span::styled(
            format!(
                "{}{}",
                grid::pad(phase.as_str(), widths.state),
                " ".repeat(GAP)
            ),
            colour(theme, phase),
        ));
    }
    if requests > 0 {
        // The one this branch is being read for, which is whatever of them is
        // still live. The rest are on the card, where there is room to list
        // them and to say what each is waiting on.
        let (label, paint) = match prs.first() {
            Some(first) => (first.label(), request_colour(theme, first.standing)),
            None => (String::new(), Style::new()),
        };
        spans.push(Span::styled(
            format!("{}{}", grid::pad(&label, requests), " ".repeat(GAP)),
            paint,
        ));
    }
    let summary = match (armed, asking) {
        (true, _) => Style::new().fg(theme.waiting),
        (false, true) => Style::new(),
        (false, false) => dim(),
    };
    spans.push(Span::styled(
        format!("{}{}", grid::pad(&said, room), " ".repeat(GAP)),
        summary,
    ));
    spans.push(Span::styled(grid::padl(&worked, widths.age), dim()));
    Line::from(spans)
}

/// What a row's name is painted in: the colour of a thing waiting on a person,
/// and the weight to go with it, where that is what the row is; the colour of a
/// failure where the work ended in one; and the terminal's own everywhere else.
///
/// Two states out of eight, because a column of names in eight colours is a
/// column nobody reads. Those two are the ones a person scanning the wall is
/// looking for, and the rest have said all they have to say on the glyph.
fn name_colour(theme: Theme, phase: Phase) -> Style {
    match phase {
        Phase::Waiting => Style::new().fg(theme.waiting).add_modifier(Modifier::BOLD),
        Phase::Failed => Style::new().fg(theme.failed),
        _ => Style::new(),
    }
}

/// What an armed row says where its summary was: the key again, and what it
/// does this time.
///
/// The words claude's own agent view uses for the same two presses, because a
/// person who has met one of these screens should not have to learn the other.
const AGAIN: &str = "ctrl+x again forgets";

/// The two columns every row is already indented by, and what each of them is
/// for: a row nobody has been to read is marked in the first, and one somebody
/// is holding at the top of its group in the second.
///
/// They cost the list no width, and down a wall of rows each lines up into a
/// column of its own — which is the thing worth reading here: not what this
/// agent is, but which of them somebody has not caught up with, and which of
/// them they said to keep in front of them. The first takes the colour of a
/// thing waiting on a person only on a row that is waiting on one, and is dim
/// on the rest: at forty rows, a column of amber dots against finished work
/// would be competing with the one thing that colour is for. The second is not
/// about the agent at all but about how somebody laid the wall out, so it is
/// drawn in the terminal's own.
fn marks(view: &View, held: bool, theme: Theme) -> [Span<'static>; MARKS] {
    [
        match rows::unread(view) {
            true => Span::styled(
                UNREAD,
                match view.phase() {
                    Phase::Waiting => Style::new().fg(theme.waiting),
                    _ => dim(),
                },
            ),
            false => Span::raw(" "),
        },
        match held {
            true => Span::raw(HELD),
            false => Span::raw(" "),
        },
    ]
}

/// What those marks are drawn with. One column each, and neither a frame of
/// the pulse nor a resting glyph: they sit beside both, and a wall where two
/// things are told apart by size would be a wall nobody reads either off.
const UNREAD: &str = "•";
const HELD: &str = "▲";

/// How many of them a row carries, which is what the gutter is wide.
const MARKS: usize = 2;

/// How many rows of a wrapped question the card gives before it stops: the
/// words of it a person needs to decide, with the pane underneath for the rest.
const ASKED_TALL: u16 = 3;

/// The card: what its branch has open, which question of the call this is,
/// what one agent is asking, the choices it offers, the row the vendor adds
/// under them, the line the answer is typed on, and the screen it is all
/// happening on — or, when that is what was asked for, what it has changed.
///
/// Full width, because the bottom of it is a picture of a terminal and a
/// terminal cut down the middle is a picture of nothing. It floats over the
/// rows rather than pushing them up, so the wall is where it was when the card
/// closes.
#[allow(clippy::too_many_arguments)]
fn float(
    frame: &mut Frame,
    card: &Card<Body>,
    showing: Option<Showing>,
    prs: &[Pr],
    answering: Option<&Composer>,
    scroll: &Scroll,
    area: Rect,
    theme: Theme,
) {
    let title = match card.changes {
        true => format!(" {} · what it has changed ", card.id),
        false => format!(
            " {} · {} {} ",
            card.id,
            card.phase.as_str(),
            derive::in_words(card.age)
        ),
    };
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(dim())
        .padding(Padding::horizontal(PADDING))
        .title(Span::styled(title, colour(theme, card.phase)));
    let inner = block.inner(area);

    // What the card is for comes first and the pane takes what is left. The
    // row being typed on comes before even the question: the question is on
    // the agent's own row behind the card, and a line somebody is typing into
    // is nowhere else at all.
    let mut room = inner.height;
    let mut take = |wanted: u16| {
        let taken = wanted.min(room);
        room -= taken;
        taken
    };
    let typing = take(u16::from(answering.is_some()));
    // Every request this branch has, above everything the card says about the
    // turn: what happened to the work after the turn ended is the question
    // somebody opening a finished agent's card came with.
    let open = requests(prs, theme);
    let opened = take(u16::from(!open.is_empty()));
    // The question and the choices are the agent's own words, and the choices
    // are the keys a person is about to press: both go through `inert` before
    // anything draws them. ratatui would *delete* the invisible format
    // characters on its own, which is exactly the wrong treatment — deleting
    // a zero-width lets one choice wear another's spelling.
    let question = card.question.as_deref().map(inert);
    let options: Vec<String> = card.options.iter().map(|option| inert(option)).collect();
    let asked = take(
        question
            .as_deref()
            .map_or(0, |question| wrapped(question, inner.width).min(ASKED_TALL)),
    );
    // Which question of the call this is comes before the choices, because it
    // decides what the choices mean: the tab behind this one asks something
    // else and offers somebody else's answers.
    let strip = tab(showing);
    let tabbed = take(u16::from(strip.is_some()));
    let choices = choices(&options, inner.width as usize, boxed(showing));
    let listed = take(choices.len() as u16);
    let added = added(card, showing);
    let adding = take(u16::from(added.is_some()));

    // What is left is the body's window, which is what a page key moves by
    // and what the offset is clamped against. A card paged away from its
    // natural edge says so on its frame, because what it is showing is no
    // longer the newest of the screen or the first of the patch.
    let held = scroll.kept(length(card), room as usize);
    if held > 0 {
        let edge = match card.forward() {
            true => '↑',
            false => '↓',
        };
        block = block
            .title_bottom(Line::styled(format!(" {edge} {held} more "), dim()).right_aligned());
    }
    // Whatever the list drew here, the card is in front of it.
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    let [requesting, tabbing, asking, listing, adds, answer, screen] = Layout::vertical([
        Constraint::Length(opened),
        Constraint::Length(tabbed),
        Constraint::Length(asked),
        Constraint::Length(listed),
        Constraint::Length(adding),
        Constraint::Length(typing),
        Constraint::Min(0),
    ])
    .areas(inner);

    if opened > 0 {
        frame.render_widget(Paragraph::new(Line::from(open)), requesting);
    }
    if let Some(strip) = strip.filter(|_| tabbed > 0) {
        frame.render_widget(Paragraph::new(Line::styled(strip, dim())), tabbing);
    }
    if let Some(question) = question {
        frame.render_widget(
            Paragraph::new(question)
                .wrap(Wrap { trim: true })
                .style(Style::new().fg(theme.waiting)),
            asking,
        );
    }
    if listed > 0 {
        let lines: Vec<Line> = choices
            .into_iter()
            .take(listed as usize)
            .map(Line::raw)
            .collect();
        frame.render_widget(Paragraph::new(lines), listing);
    }
    if let Some(added) = added.filter(|_| adding > 0) {
        frame.render_widget(Paragraph::new(Line::styled(added, dim())), adds);
    }
    if let Some(composer) = answering.filter(|_| typing > 0) {
        answer_row(frame, card, showing, composer, answer, theme);
    }

    frame.render_widget(
        Paragraph::new(body(card, screen.height as usize, held)),
        screen,
    );
}

/// How many rows the body could give a card, which is what the last page is
/// measured against. Asked of the body itself rather than of a window of it,
/// so measuring a patch of thousands of rows does not build them.
fn length(card: &Card<Body>) -> usize {
    match card.asks() && card.question.is_some() {
        true => 0,
        false => card.body.length(),
    }
}

/// Every pull request the agent's branch has, as the one row the card gives
/// them.
///
/// The row says the number in its own colour and then, in words, which of the
/// four questions that colour came from — a row has only the colour, and two
/// standings share one. All of them and not the first: a branch that has been
/// through this twice is a branch where the second attempt is the news and the
/// first is the reason there was a second.
///
/// Nothing here comes off a pane, so nothing here is neutralised: the numbers
/// are amx's own formatting of an integer, and the words are this file's.
fn requests(prs: &[Pr], theme: Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for pr in prs {
        if !spans.is_empty() {
            spans.push(Span::styled(SEPARATOR, dim()));
        }
        spans.push(Span::styled(
            format!("{} {}", pr.label(), pr.standing.says()),
            request_colour(theme, pr.standing),
        ));
    }
    spans
}

/// What the card has under everything else, in the paint it was drawn in and
/// cut to the rows the card has for it.
///
/// A screen is read from the bottom, where the newest of it is; a diff from
/// the top, where the first file it touched is; and a recorded answer from
/// its top too, because an answer reads forward.
///
/// A card holding a question has nothing under everything else at all. The
/// question block — the tab strip, the question, the choices and the rows
/// under them — is the whole of what that card is for, and the pane beneath
/// it is the vendor's drawing of the same box behind an echo of the prompt:
/// every row of it is noise below the answer line. Only the waiting card
/// whose question amx has not read keeps its capture, because the pane is
/// the one place that question is written at all.
///
/// claude's own furniture came off the screen before it was ever counted, in
/// [`Body::screen`]. After would be worse than not at all: the card would
/// spend its window on the vendor's composer and then have nothing left for
/// the work.
fn body(card: &Card<Body>, rows: usize, away: usize) -> Vec<Line<'static>> {
    if card.asks() && card.question.is_some() {
        return Vec::new();
    }

    // A patch and a recorded answer both read forward, so both are windowed
    // from their top; a screen from its bottom, where the newest of it is.
    let window = match card.forward() {
        true => head(card.body.kept, rows, away),
        false => tail(card.body.kept, rows, away),
    };
    let shown = card.body.rows[window].to_vec();

    // Said only where the walk actually cut. An agent that has said nothing
    // yet is a different fact from a pane holding nothing but furniture, and
    // a card that answered both with the same sentence would be lying about
    // one of them.
    match shown.is_empty() && card.body.chrome {
        true => vec![Line::styled(ALL_CHROME, dim())],
        false => shown,
    }
}

#[cfg(test)]
thread_local! {
    /// How many bodies this thread has walked out of ANSI, which is the whole
    /// cost of a card: a pane capture is a few thousand bytes of escape
    /// sequences, and walking them is the one piece of work a card does that
    /// grows with what the agent wrote. Counted so a test can say where the
    /// walk happens and not only what it produces.
    ///
    /// Per thread, because the tests run side by side in one process and a
    /// count they shared would be a count none of them could assert on.
    static WALKS: Cell<usize> = const { Cell::new(0) };
}

/// How many walks this thread has paid for so far.
#[cfg(test)]
pub(super) fn walks() -> usize {
    WALKS.with(Cell::get)
}

/// What a captured row says, which is what the cut reads it for. The runs of
/// one row joined, so the words and the paint can never disagree about where a
/// row begins or what is on it.
fn words(row: &[Painted]) -> String {
    row.iter().map(|run| run.text.as_str()).collect()
}

/// One captured row, drawn the way the vendor drew it.
fn as_painted(row: &[Painted]) -> Line<'static> {
    let spans: Vec<Span<'static>> = row
        .iter()
        .map(|run| Span::styled(inert(&run.text), paint(run)))
        .collect();
    Line::from(spans)
}

/// The paint one run was written in, as the renderer's own styling.
fn paint(run: &Painted) -> Style {
    let mut style = Style::new();
    for (on, modifier) in [
        (run.bold, Modifier::BOLD),
        (run.dim, Modifier::DIM),
        (run.italic, Modifier::ITALIC),
        (run.underline, Modifier::UNDERLINED),
        (run.reverse, Modifier::REVERSED),
    ] {
        if on {
            style = style.add_modifier(modifier);
        }
    }
    if let Some(fg) = run.fg {
        style = style.fg(shade(fg));
    }
    if let Some(bg) = run.bg {
        style = style.bg(shade(bg));
    }
    style
}

/// A colour the vendor named, as the renderer names it. The first sixteen are
/// named rather than numbered, so a person's own palette decides what red
/// looks like on their terminal, the way it does in the pane itself.
fn shade(colour: Colour) -> Color {
    match colour {
        Colour::Ansi(n) => ANSI[usize::from(n) & 0x0f],
        Colour::Indexed(n) => Color::Indexed(n),
        Colour::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// The sixteen SGR names them, in the order ANSI numbers them.
const ANSI: [Color; 16] = [
    Color::Black,
    Color::Red,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::Magenta,
    Color::Cyan,
    Color::Gray,
    Color::DarkGray,
    Color::LightRed,
    Color::LightGreen,
    Color::LightYellow,
    Color::LightBlue,
    Color::LightMagenta,
    Color::LightCyan,
    Color::White,
];

/// Text out of an agent's own screen, made safe to hand a terminal. The paint
/// is gone by the time this runs, so what is left to neutralise is the
/// characters that were never paint: the controls and the invisible format
/// characters a row can be written to carry.
fn inert(text: &str) -> String {
    crate::tmux::sanitize(text)
}

/// What the card says where the walk finds nothing underneath the chrome.
const ALL_CHROME: &str = "amx captured nothing but claude's own chrome";

/// Which question of the call the card is showing, and how many there are.
///
/// The one thing on the card that is nowhere on the pane under it. Measured
/// against claude 2.1.240, the vendor's tab strip elides its own headers as
/// the pane narrows and at 24 columns draws the showing tab's name as an
/// ellipsis and nothing else, so no reader can count or name the tabs from a
/// screen. A call of one question is not a strip and says nothing here.
fn tab(showing: Option<Showing>) -> Option<String> {
    let showing = showing.filter(|showing| showing.of > 1)?;
    let counted = format!("{} of {}", showing.at, showing.of);
    Some(match showing.header() {
        Some(header) => format!("{header}{SEPARATOR}{counted}"),
        None => counted,
    })
}

/// Whether the choices are boxes to check rather than a choice to make.
fn boxed(showing: Option<Showing>) -> bool {
    showing.is_some_and(|showing| showing.ask.multi)
}

/// The vendor's own empty box, drawn between the number and the label the way
/// 2.1.240 draws it, so the row on the card reads as the row on the pane.
///
/// Empty, always. What amx holds is the payload, and the payload names the
/// choices and never says which of them are checked — the boxes themselves are
/// on the pane at the bottom of the card, where they are being checked.
const BOX: &str = "[ ]";

/// The row the vendor draws under the choices that no payload accounts for.
///
/// Every menu the tool draws carries one free-text row as its last choice, and
/// a question whose choices carry a preview draws a notes field in its place
/// and no free-text row at all — neither is in the payload, and both are what
/// somebody about to answer needs to know is there. A permission box and the
/// trust screen have neither, and choices amx has not read yet have nothing
/// for this to stand under.
fn added(card: &Card<Body>, showing: Option<Showing>) -> Option<&'static str> {
    if card.options.is_empty() || card.kind != Some(Kind::Question) {
        return None;
    }
    match showing.is_some_and(|showing| showing.ask.takes_notes()) {
        true => Some(NOTES),
        false => Some(OTHER),
    }
}

/// The free-text row, named as the vendor's rather than the agent's: the
/// payload does not carry it, so the pane below the card has a numbered row
/// the choices above it do not.
const OTHER: &str = "and under them, the vendor's row for words of your own";

/// And the field the vendor draws where a choice carries a preview, which is
/// the one layout that has no free-text row at all.
const NOTES: &str = "and beside them, the vendor's field for a note";

/// The choices under the question, numbered the way every surface numbers them
/// and packed onto as few rows as the card is wide.
///
/// From [`numbered`] like the rest of them, so the number a person presses on
/// the card is the number `amx answer` takes and the number `ls` printed. One
/// too wide for the card is cut with the ellipsis that says it was: a choice
/// nobody can read is still a choice they can press, and its number is at the
/// front where the cut cannot reach it.
///
/// `boxed` puts the vendor's box between the number and the label, on the
/// question that takes more than one choice. A number pressed there checks a
/// box and submits nothing, and a row that looked the same either way would be
/// a screen telling somebody they had answered.
fn choices(options: &[String], width: usize, boxed: bool) -> Vec<String> {
    let labels: Vec<String> = match boxed {
        true => options
            .iter()
            .map(|label| format!("{BOX} {label}"))
            .collect(),
        false => options.to_vec(),
    };

    let mut rows: Vec<String> = Vec::new();
    for choice in numbered(&labels) {
        let room = width.saturating_sub(choice.chars().count() + BETWEEN.len());
        match rows.last_mut() {
            Some(row) if row.chars().count() <= room => {
                row.push_str(BETWEEN);
                row.push_str(&choice);
            }
            _ => rows.push(fit(&choice, width)),
        }
    }
    rows
}

/// What stands between two choices sitting on one row.
const BETWEEN: &str = "   ";

/// What the row an answer is typed on begins with.
const ANSWER: &str = "❯ ";

/// The line the answer is being typed on, with the terminal's own cursor at
/// the end of it.
///
/// Empty, it says what this question will take instead — which is the one
/// thing somebody looking at a prompt they did not draw cannot work out for
/// themselves, and it is said from the same place the refusal is written.
fn answer_row(
    frame: &mut Frame,
    card: &Card<Body>,
    showing: Option<Showing>,
    composer: &Composer,
    area: Rect,
    theme: Theme,
) {
    let width = area.width as usize;
    let room = width.saturating_sub(ANSWER.chars().count()).max(1);
    // The end of the line, because the end is where somebody is typing.
    let typed = composer_lines(&composer.text, room)
        .pop()
        .unwrap_or_default();
    let asked = showing.map(|showing| showing.ask);
    let said = match composer.text.is_empty() {
        true => Span::styled(act::invitation(card.kind, &card.options, asked), dim()),
        false => Span::raw(typed.clone()),
    };

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(ANSWER, Style::new().fg(theme.waiting)),
            said,
        ])),
        area,
    );
    let at = match composer.text.is_empty() {
        true => 0,
        false => typed.chars().count(),
    };
    frame.set_cursor_position((
        area.x + (ANSWER.chars().count() + at).min(width.saturating_sub(1)) as u16,
        area.y,
    ));
}

/// How narrow a band may be before a screen has no room for another one beside
/// it: the widest key a column can hold, the air after it, and a character of
/// what it does.
///
/// A floor rather than a comfortable width, because of what the other end of
/// it costs. Short of a band the keys that would have gone in it are cut off
/// the bottom of the screen, and a key nobody can find is the one thing this
/// screen may not lose; a band this narrow is a key column with a stub against
/// it, and the key is the half somebody came here for.
const BAND: usize = 12;

/// How many bands the groups stand in wherever the width will take that many.
///
/// Two, because five short lists side by side is a wall of keys again and one
/// column of thirty-eight rows is the flat list the headings were put in to
/// break up. Two columns is what a page of keys looks like.
const COLUMNS: usize = 2;

/// One row of the overlay: a heading, a key with what it does, or the blank
/// that stands one group off from the next.
enum Told {
    Heading(String),
    Key(String, String),
    Air,
}

/// Every key and what it does, under the heading that says what it is for, in
/// bands read down and then across.
///
/// The height decides how many bands there are and the width decides how much
/// of each description survives, because what this screen is for is being
/// complete: a key cut off the bottom is one the view has and nobody can find,
/// where a description cut short still leaves its key where it can be read.
///
/// Down before across for the reason a list is a column. Somebody looking for
/// one key reads the heading, runs their eye down the keys under it and on to
/// the next; a table filled the other way would put the second key beside the
/// first and the rest of them anywhere at all.
fn help(frame: &mut Frame, area: Rect) {
    let bands = bands(area);
    let share = (area.width as usize / bands.len().max(1)).max(1);
    let deep = bands.iter().map(Vec::len).max().unwrap_or(0);

    let lines: Vec<Line> = (0..deep.min(area.height as usize))
        .map(|at| {
            let mut spans = Vec::new();
            let mut column = 0;
            for (n, band) in bands.iter().enumerate() {
                // A band that has run out of keys, or is standing one group
                // off from the next, leaves the ones beside it where they
                // were: the columns are what the eye follows down.
                let told = match band.get(at) {
                    Some(Told::Heading(name)) => vec![Span::styled(name.clone(), dim())],
                    Some(Told::Key(key, does)) => vec![
                        Span::styled(key.clone(), Style::new().add_modifier(Modifier::BOLD)),
                        Span::styled(does.clone(), dim()),
                    ],
                    Some(Told::Air) | None => continue,
                };
                if n * share > column {
                    spans.push(Span::raw(" ".repeat(n * share - column)));
                }
                column = n * share + said(&told);
                spans.extend(told);
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// The keys as the bands they are drawn in: the groups dealt into as few bands
/// as the height needs and the width will take, each key padded to line up
/// under the one above it, and each description cut to what its own band was
/// given.
fn bands(area: Rect) -> Vec<Vec<Told>> {
    let width = area.width as usize;
    let height = (area.height as usize).max(1);
    // A heading and the keys under it, which is what a group costs a band.
    let depths: Vec<usize> = GROUPS.iter().map(|(_, under)| under + 1).collect();
    // Two bands wherever there is width for two, and another whenever the
    // groups will not stand in the rows the band has. A group is what the eye
    // follows down, so it is never cut in half to make the columns even.
    let most = (width / BAND).max(1);
    let mut count = COLUMNS.min(most);
    while count < most && deepest(&depths, count) > height {
        count += 1;
    }
    let share = (width / count).max(1);

    let bands = dealt(&depths, count);
    bands
        .iter()
        .enumerate()
        .map(|(n, groups)| {
            // The key column is worked out over the whole band, so every key
            // in it lines up under the one above.
            let column = groups
                .iter()
                .flat_map(|group| under(*group))
                .map(|(key, _)| key.chars().count())
                .max()
                .unwrap_or(0)
                + 1;
            // The last band takes whatever the division left over, and every
            // other one keeps a column of air between it and the next.
            let room = match n + 1 == bands.len() {
                true => width.saturating_sub(n * share + column),
                false => share.saturating_sub(column + 1),
            };
            let mut told = Vec::new();
            for group in groups {
                if !told.is_empty() {
                    told.push(Told::Air);
                }
                told.push(Told::Heading(fit(GROUPS[*group].0, column + room)));
                told.extend(
                    under(*group)
                        .map(|(key, does)| Told::Key(format!("{key:<column$}"), fit(does, room))),
                );
            }
            told
        })
        .collect()
}

/// The keys one group stands over, which is its run of [`HELP`].
fn under(group: usize) -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    let from: usize = GROUPS[..group].iter().map(|(_, under)| under).sum();
    HELP[from..from + GROUPS[group].1].iter()
}

/// The shallowest a band can be when these groups are dealt into `count` of
/// them, which is what says whether that many bands will fit the screen.
fn deepest(depths: &[usize], count: usize) -> usize {
    let most = depths.iter().sum::<usize>() + depths.len().saturating_sub(1);
    let least = depths.iter().copied().max().unwrap_or(0);
    (least..=most)
        .find(|deep| taken(depths, *deep) <= count)
        .unwrap_or(most)
}

/// How many bands the groups take when none of them may be deeper than this.
fn taken(depths: &[usize], deep: usize) -> usize {
    let (mut bands, mut used) = (1, 0);
    for &depth in depths {
        let wanted = match used {
            0 => depth,
            used => used + 1 + depth,
        };
        match wanted <= deep || used == 0 {
            true => used = wanted,
            false => {
                bands += 1;
                used = depth;
            }
        }
    }
    bands
}

/// Which groups each band holds, in order: as level as groups this size deal,
/// and never a group in two bands.
fn dealt(depths: &[usize], count: usize) -> Vec<Vec<usize>> {
    let deep = deepest(depths, count);
    let mut bands: Vec<Vec<usize>> = vec![Vec::new()];
    let mut used = 0;
    for (group, &depth) in depths.iter().enumerate() {
        let wanted = match used {
            0 => depth,
            used => used + 1 + depth,
        };
        match wanted <= deep || used == 0 {
            true => used = wanted,
            false => {
                bands.push(Vec::new());
                used = depth;
            }
        }
        bands.last_mut().expect("a band to deal into").push(group);
    }
    bands
}

/// How tall the composer may grow before it stops and scrolls instead: ten
/// rows, or a third of the screen where that is less. A composer that could
/// take the whole terminal would be a list nobody could see past the task
/// they are typing at it.
const COMPOSER_CAP: usize = 10;

/// What the composer's rows begin with: the prompt on the first of them, and
/// the same width of nothing under it, so a line that wrapped reads as one
/// line.
fn gutter(composer: &Composer) -> String {
    format!("{} ▸ ", composer.prompt())
}

/// How wide the text itself is drawn, which is the same on every row of the
/// composer whether the prompt or the indent is in front of it.
fn composer_room(composer: &Composer, width: u16) -> usize {
    (width as usize)
        .saturating_sub(gutter(composer).chars().count())
        .max(1)
}

/// The line being typed, cut into the rows a screen this wide draws it on.
///
/// A newline starts a row, pasted or typed, and anything past the width
/// carries onto the next one. An empty paragraph is a row of its own: it is
/// where the cursor sits after a newline, and a row nobody drew would put the
/// cursor on the line above.
fn composer_lines(text: &str, room: usize) -> Vec<String> {
    let room = room.max(1);
    let mut rows = Vec::new();
    for paragraph in text.split('\n') {
        let chars: Vec<char> = paragraph.chars().collect();
        match chars.is_empty() {
            true => rows.push(String::new()),
            false => rows.extend(chars.chunks(room).map(|row| row.iter().collect())),
        }
    }
    rows
}

/// How many rows the composer takes on this screen: as many as the line needs,
/// up to the cap, and never so many that the list it was opened from is gone.
///
/// `chrome` is every other band already spoken for — the header, the keys, the
/// closer look — and one row over that is the list's, which the composer may
/// not have.
fn composer_height(composer: &Composer, area: Rect, chrome: u16) -> u16 {
    let room = area.height.saturating_sub(chrome + 1) as usize;
    let cap = COMPOSER_CAP.min(area.height as usize / 3).min(room).max(1);
    composer_lines(&composer.text, composer_room(composer, area.width))
        .len()
        .clamp(1, cap) as u16
}

/// The line somebody is typing, with the terminal's own cursor at the end of
/// it: something being typed into should look like it.
///
/// Past the cap it is the end of the line that is drawn, because the end is
/// where somebody is typing — but the prompt stays on the top row however far
/// the rest has scrolled. It is what says where enter will send this, and that
/// is worth a gutter wherever the text has got to.
fn composing_line(frame: &mut Frame, composer: &Composer, area: Rect, theme: Theme) {
    let prompt = gutter(composer);
    let width = area.width as usize;
    let rows = composer_lines(&composer.text, composer_room(composer, area.width));
    let from = rows.len().saturating_sub(area.height as usize);
    let shown = &rows[from..];

    let indent = " ".repeat(prompt.chars().count());
    let lines: Vec<Line> = shown
        .iter()
        .enumerate()
        .map(|(at, text)| {
            let head = match at {
                0 => Span::styled(prompt.clone(), Style::new().fg(theme.waiting)),
                _ => Span::raw(indent.clone()),
            };
            let mut spans = vec![head, Span::raw(text.clone())];
            // An empty line holds its prefixes as ghost text, cut where the
            // screen ends; the cursor set below sits over the front of it.
            if at == 0
                && let Some(hint) = placeholder(composer)
            {
                let room = width.saturating_sub(prompt.chars().count());
                spans.push(Span::styled(fit(hint, room), dim()));
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);

    let at = prompt.chars().count() + shown.last().map_or(0, |row| row.chars().count());
    frame.set_cursor_position((
        area.x + at.min(width.saturating_sub(1)) as u16,
        area.y + shown.len().saturating_sub(1) as u16,
    ));
}

/// What the next agent will be allowed to do without asking, under the line it
/// would be started from — where claude's own screens put it, and where
/// somebody about to press enter is already looking.
///
/// The row belongs to a line that will start an agent: not to a reply, which
/// goes to one already running under whatever it was started with, and not to
/// a line that narrows the list. At the sentinel it names the layer rather
/// than a mode, because amx does not know which mode the vendor is configured
/// for and a guess at it is the same lie the model dial refuses. A vendor
/// whose entry declares no permission dial has nothing to say here and nothing
/// to turn, so the row is absent rather than empty.
fn permission(screen: &Screen) -> Option<Line<'static>> {
    let Mode::Typing(composer) = &screen.mode else {
        return None;
    };
    if !matches!(composer.asking, Asking::Task) || composer.narrows() {
        return None;
    }
    screen.profile.permission_dial()?;

    let said = match screen.profile.permission.as_str() {
        DEFAULT => "permission: vendor default".to_string(),
        mode => format!("⏵⏵ {mode}"),
    };
    Some(Line::styled(
        format!("{said} (shift+tab to cycle)"),
        prospective(screen.theme),
    ))
}

/// The words the task line reads at its front, said on the line itself while
/// there is nothing on it.
///
/// The prefixes are amx's own grammar and nothing else on the screen teaches
/// them: a dial turned by `m:` looks exactly like a task that happens to open
/// with one. So the empty line holds them the way a form field holds its
/// ghost text — dim, after the prompt, and gone at the first character typed,
/// because whoever is typing has stopped reading it. A reply and a rename
/// read no prefixes, so their lines teach none.
fn placeholder(composer: &Composer) -> Option<&'static str> {
    if !matches!(composer.asking, Asking::Task) || !composer.text.is_empty() {
        return None;
    }
    Some(
        "m:model · p:permission · w:on|off · d:directory · agent:command \
         · s:state · a:name",
    )
}

/// The keys with nowhere else to be said, as the line under the cursor makes
/// them true.
///
/// Enter brings a window forward on a row, shuts a group on a heading and
/// gives back the fold's rows on the fold; a row of hints that named one of
/// those over the other two would be teaching somebody to press the wrong key.
/// So what the cursor is standing on decides the front of the row, and the
/// keys that mean the same thing wherever it is standing follow.
fn hints(screen: &Screen) -> Vec<&'static str> {
    let list = &screen.list;
    let mut said = match list.items().get(list.cursor()) {
        Some(Item::Heading(_, tally)) => vec![
            match tally.shut {
                true => "enter opens it",
                false => "enter shuts it",
            },
            "ctrl+x clears the group",
        ],
        Some(Item::Fold(_)) => vec!["enter shows them"],
        // The cursor never rests on a blank; the arm is for the compiler.
        Some(Item::Blank) => Vec::new(),
        // An agent whose command has ended has no window to bring forward and
        // nothing left to stop, and the same key that would have stopped it
        // forgets it instead.
        Some(Item::Agent(_)) => {
            let card = match screen.card.is_some() {
                true => "space closes it",
                false => "space card",
            };
            match list
                .selected()
                .is_some_and(|view| view.phase().is_terminal())
            {
                true => vec![card, "ctrl+x forget"],
                false => vec![card, "enter attach", "ctrl+x stop"],
            }
        }
        // A wall with nothing on it has no line under the cursor, and the one
        // key that changes that is the one worth the room.
        None => vec!["n starts one"],
    };
    said.extend(["ctrl+s axis", "q quit"]);
    said
}

/// Those keys on one row, cut to what a screen this wide can hold.
///
/// What goes is what is furthest from `?`, and `?` itself never does: a hint
/// clipped by the terminal reads as a key that ends where the screen does, and
/// the last one a narrow row has room for should be the one that leads to all
/// the others.
fn fitted(said: &[&str], width: usize) -> String {
    let taken = |kept: &[&str]| {
        kept.iter()
            .map(|hint| hint.chars().count() + SEPARATOR.chars().count())
            .sum::<usize>()
            + MORE.chars().count()
    };

    let mut kept = said.to_vec();
    while !kept.is_empty() && taken(&kept) > width {
        kept.pop();
    }
    kept.push(MORE);
    kept.join(SEPARATOR)
}

/// The keys, or whatever the view has to say for itself instead.
///
/// The row under the card is the card's while it is holding a line, because
/// what enter does there is not what it does anywhere else in the view.
fn footer(screen: &Screen, width: u16) -> Line<'static> {
    if let Some(notice) = &screen.notice {
        return match notice {
            Notice::Failed(said) => {
                Line::styled(said.clone(), Style::new().fg(screen.theme.failed))
            }
            Notice::Advice(said) => Line::styled(said.clone(), dim()),
        };
    }
    if screen.answering().is_some() {
        return Line::styled(ANSWERS.to_string(), dim());
    }
    // A question of the view's own is not advice and not a key: it is the one
    // thing on the screen, in the colour of something waiting on a person.
    if let Mode::Confirming(asked) = &screen.mode {
        return Line::styled(asked.question(), Style::new().fg(screen.theme.waiting));
    }
    Line::styled(
        match &screen.mode {
            Mode::List => fitted(&hints(screen), width as usize),
            Mode::Keys => "any key goes back · q quits".to_string(),
            // A question up is the whole of this row, and is drawn above.
            Mode::Confirming(_) => fitted(&hints(screen), width as usize),
            Mode::Typing(composer) if composer.narrows() => {
                "enter narrows it · s: or a: alone clears · esc cancels".to_string()
            }
            Mode::Typing(composer) => match composer.asking {
                Asking::Task => "enter starts it · alt+enter newline · esc cancels".to_string(),
                Asking::Reply { .. } => {
                    "enter sends it · alt+enter newline · esc cancels".to_string()
                }
                Asking::Name { .. } => "enter renames it · esc leaves it alone".to_string(),
            },
        },
        dim(),
    )
}

/// How wide the pull request column has to be, which is the one column of a
/// row the design does not fix: the widest label anybody on the screen is
/// wearing, and no column at all where nobody is wearing one — which is every
/// list on a machine with no forge on it.
fn request_column(list: &List) -> usize {
    list.items()
        .iter()
        .filter_map(|item| list.agent(*item))
        .filter_map(|view| list.requests(view).first())
        .map(|pr| pr.label().chars().count())
        .max()
        .unwrap_or(0)
}

/// How many rows text takes when it is wrapped to a width.
fn wrapped(text: &str, width: u16) -> u16 {
    let width = width.max(1) as usize;
    let rows = text.chars().count().div_ceil(width);
    rows.clamp(1, u16::MAX as usize) as u16
}

/// Which rows of a screen the card shows: the last of the `end` rows the body
/// kept, which is where the newest of a pane is.
///
/// A window rather than the rows themselves, because a body carries the words
/// its rows say and the paint they say them in, and a reading that cut one
/// without the other would have them disagree.
fn tail(end: usize, wanted: usize, back: usize) -> Range<usize> {
    // A paged card stands that many rows above the bottom it is read from.
    let end = end.saturating_sub(back);
    end.saturating_sub(wanted)..end
}

/// And which rows of a patch or a recorded answer: the first of them, because
/// both read forward from their top. A paged card starts that many rows below
/// it.
fn head(end: usize, wanted: usize, away: usize) -> Range<usize> {
    let start = away.min(end);
    start..end.min(start.saturating_add(wanted))
}

/// What stands between two columns of the list, whether that is a name and a
/// summary or a heading's rule and its count.
const GAP: usize = 2;

/// One line of it, so a paragraph of an answer cannot take over a row.
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim()
}

/// `text`, cut to `width` with an ellipsis for what was cut.
/// The columns `text` takes on a screen, which is not its characters: an
/// emoji is one char and two columns, and a row measured in characters
/// pushes its last column off the terminal's edge. ratatui's own measure,
/// so a row is cut by the same arithmetic it is drawn with.
fn width_of(text: &str) -> usize {
    Span::raw(text).width()
}

fn fit(text: &str, width: usize) -> String {
    if width_of(text) <= width {
        return text.to_string();
    }
    match width {
        0 => String::new(),
        1 => "…".to_string(),
        _ => {
            let mut kept = String::new();
            let mut used = 0;
            for one in text.chars() {
                let wide = width_of(one.encode_utf8(&mut [0; 4]));
                if used + wide > width - 1 {
                    break;
                }
                used += wide;
                kept.push(one);
            }
            kept.push('…');
            kept
        }
    }
}

/// What a row is indented by, so an agent reads as sitting under the heading
/// it belongs to rather than beside it. One column for each mark a row can
/// carry, which is what lets the marks cost the list no width at all.
const GUTTER: &str = "  ";
const _: () = assert!(GUTTER.len() == MARKS);

/// The vendor's glyph set for a terminal. Ghostty draws the eight-spoked
/// asterisk where everything else gets a plain one, and that is the only thing
/// `$TERM` decides. Measured from the 2.1.237 bundle.
fn set_for(term: &str) -> [&'static str; 6] {
    match term {
        "xterm-ghostty" => ["·", "✢", "✳", "✶", "✻", "✻"],
        _ => ["·", "✢", "*", "✶", "✻", "✽"],
    }
}

/// That set for this terminal, read once: `$TERM` does not change under a
/// running view, and the vendor memoizes it for the same reason.
fn set() -> [&'static str; 6] {
    static SET: OnceLock<[&'static str; 6]> = OnceLock::new();
    *SET.get_or_init(|| set_for(std::env::var("TERM").unwrap_or_default().as_str()))
}

/// Which of the six a working row rests on, and the frame the pulse is
/// largest at either side of.
const LIVE: usize = 4;

/// The six ping-ponged into twelve frames, which is the vendor's own working
/// mark ported rather than approximated: the set forwards and then backwards,
/// one frame every 120ms. It grows from a dot to the largest asterisk and
/// shrinks back, so a working row breathes rather than spins.
fn pulse(beat: usize) -> &'static str {
    let set = set();
    let frames = set.len() * 2;
    let at = beat % frames;
    // The back half is the front half read the other way.
    set[at.min(frames - 1 - at)]
}

/// The mark a state rests on: eight states and eight marks, so a row says
/// which one it is in with the colour turned off.
///
/// The circle is drawn three ways — dotted while it is coming up, hollow while
/// it is alive and quiet, filled once it is finished — and nothing at rest may
/// borrow a frame of the pulse, because every one of those is in motion. The
/// exception is working itself, which rests on the vendor's live glyph and is
/// the thing the pulse moves off and back to.
fn resting(phase: Phase) -> &'static str {
    match phase {
        Phase::Waiting => "?",
        Phase::Starting => "◌",
        Phase::Working => set()[LIVE],
        Phase::Idle => "○",
        Phase::Done => "●",
        Phase::Failed => "✗",
        Phase::Stopped => "⏹",
        // amx does not know what this agent is doing, and says so.
        Phase::Unknown => "~",
    }
}

/// The mark on a row now: a working agent is drawn a frame at a time, and
/// every other state rests.
fn icon(phase: Phase, beat: usize) -> &'static str {
    match phase {
        Phase::Working => pulse(beat),
        phase => resting(phase),
    }
}

/// What a state is worth saying in colour.
///
/// Whether anything is running is the mark's job, which leaves the colour to
/// carry how it went: an agent still at work has nothing to say about that
/// yet, so it takes the terminal's own colour and earns one by ending.
fn colour(theme: Theme, phase: Phase) -> Style {
    match phase {
        // What amx cannot account for wants a person as much as a question
        // does, and the mark is what says which of the two it is.
        Phase::Waiting | Phase::Unknown => Style::new().fg(theme.waiting),
        Phase::Starting | Phase::Working => Style::new(),
        Phase::Idle => dim(),
        Phase::Done => Style::new().fg(theme.done),
        Phase::Failed => Style::new().fg(theme.failed),
        Phase::Stopped => Style::new().fg(theme.stopped),
    }
}

/// What a pull request's standing is worth saying in colour.
///
/// The same five roles the rest of the view is painted in, asked the same
/// question: how did it go. A merged request and an approved one went the way
/// they were meant to; a failing check was attempted and failed; a reviewer
/// asking for changes is a thing waiting on a person; a request that was shut
/// was ended by hand. Two of them take the terminal's own colour, because a
/// request whose checks are still running and one nobody has read yet have the
/// same answer to that question — nothing yet. Which of the two it is, is what
/// the card says in words.
fn request_colour(theme: Theme, standing: Standing) -> Style {
    match standing {
        Standing::Merged | Standing::Ready => Style::new().fg(theme.done),
        Standing::Failing => Style::new().fg(theme.failed),
        Standing::Changes => Style::new().fg(theme.waiting),
        Standing::Closed => Style::new().fg(theme.stopped),
        Standing::Draft => dim(),
        Standing::Running | Standing::Open => Style::new(),
    }
}

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

/// What the next agent may do without asking, under the line that would start
/// it: the same accent every dial above the list wears, because it is one of
/// them, promoted to where somebody is about to press enter past it.
///
/// Weight as well as colour, which is what sets it apart from the dials it came
/// from and holds it apart on a terminal with the colour turned off: the row
/// has to read as amx's own answer for a spawn rather than as another line of
/// the composer it is under.
fn prospective(theme: Theme) -> Style {
    Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests {
    use super::super::Arm;
    use super::super::rows::Narrow;
    use super::*;
    use crate::derive::{Evidence, Verdict};
    use crate::store::{Meta, State};
    use crate::tmux::{PaneId, Socket};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;
    use std::path::PathBuf;
    use std::time::Instant;

    /// The palette a screen nobody handed a theme is painted in, which is the
    /// one every screen built here has and the one these colours are read out
    /// of: what the tests are about is which role a thing is painted in, and
    /// the values are the theme's business.
    fn theme() -> Theme {
        Theme::default()
    }

    fn view(id: &str, phase: Phase, said: Option<&str>, age: u64) -> View {
        View {
            meta: Meta {
                id: id.to_string(),
                task: "fix the login bug".to_string(),
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
                // The rows print the worked seconds; most of these tests only
                // care that a number is where the column is, so the helper
                // hands both clocks the same one.
                worked: age,
            },
        }
    }

    /// Every state there is, so a table of marks cannot quietly miss one.
    const EVERY: [Phase; 8] = [
        Phase::Starting,
        Phase::Working,
        Phase::Waiting,
        Phase::Idle,
        Phase::Done,
        Phase::Failed,
        Phase::Stopped,
        Phase::Unknown,
    ];

    /// The view, with a reading in it. The card is read as it is planted,
    /// the way the view itself builds one.
    fn showing(views: Vec<View>, card: Option<Card>) -> Screen {
        let mut screen = Screen::default();
        screen.list.show(views);
        screen.card = card.map(Card::read);
        screen
    }

    /// The card a waiting agent's row opens: what it is asking, the choices it
    /// offers, and the screen it is asking on.
    fn asking(options: &[&str], kind: Option<Kind>) -> Card {
        Card {
            id: "ask-a1b".to_string(),
            phase: Phase::Waiting,
            age: 29,
            question: Some("Which fixture should the port keep?".to_string()),
            options: options.iter().map(|label| (*label).to_string()).collect(),
            kind,
            body: "$ cargo test\nDo you want to proceed?".to_string(),
            changes: false,
            answer: false,
        }
    }

    /// The same reading, running somewhere else.
    fn at(mut view: View, dir: &str) -> View {
        view.meta.dir = PathBuf::from(dir);
        view
    }

    /// The same reading, on a branch of its own.
    fn on_a_branch(mut view: View, branch: &str) -> View {
        view.meta.branch = Some(branch.to_string());
        view
    }

    /// A forge holding one failing request for the agent that is asking, and
    /// two for the one beside it — the second attempt and the first.
    fn a_forge(meta: &crate::store::Meta) -> Vec<Pr> {
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

    /// The view over that forge.
    fn over_the_forge(views: Vec<View>, card: Option<Card>) -> Screen {
        let mut screen = Screen::default();
        screen.list.asking(a_forge);
        screen.list.show(views);
        screen.card = card.map(Card::read);
        screen
    }

    /// The view with the agents gathered by where they are running.
    fn by_project(views: Vec<View>) -> Screen {
        let mut screen = Screen::default();
        screen.list.turn();
        screen.list.show(views);
        screen
    }

    /// What a view of this size draws, cell by cell.
    fn cells(screen: &Screen, size: (u16, u16)) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
        terminal.draw(|frame| draw(frame, screen)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// The mark on a row, and how the view painted it: a mark carries its
    /// colour, and a test that read the text alone could not see it.
    fn mark(screen: &Screen, size: (u16, u16), row: u16) -> (String, Color, Modifier) {
        let cell = cells(screen, size)[(2, row)].clone();
        (cell.symbol().to_string(), cell.fg, cell.modifier)
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

    /// What the view puts on a screen of this size, line by line.
    fn drawn(views: Vec<View>, card: Option<Card>, size: (u16, u16)) -> Vec<String> {
        painted(&showing(views, card), size)
    }

    /// What a heading line says in front of the rule that carries it out to
    /// the edge: the label, and how many failed under it where any did.
    fn heading_of(line: &str) -> &str {
        line.split('─').next().unwrap_or_default().trim()
    }

    /// And the count it ends in, which is the last thing on the line.
    fn counted(line: &str) -> &str {
        line.split_whitespace().next_back().unwrap_or_default()
    }

    /// The same, once the list has learned the screen's size: the first
    /// frame writes the room back the way the loop's draw does, the refit
    /// lays the rows out for it, and the second frame is the one a person
    /// reads.
    fn settled(views: Vec<View>, size: (u16, u16)) -> Vec<String> {
        let mut screen = showing(views, None);
        let _ = painted(&screen, size);
        screen.list.refit();
        painted(&screen, size)
    }

    /// The two agents a card is opened over, so there is a list to still be
    /// drawn behind it.
    fn a_fleet() -> Vec<View> {
        vec![
            view("ask-a1b", Phase::Waiting, None, 29),
            view("busy-b2c", Phase::Working, Some("Running Bash"), 3),
        ]
    }

    #[test]
    fn card_floats_a_bordered_box_over_the_still_drawn_list() {
        let screen = drawn(
            a_fleet(),
            Some(asking(
                &["the sqlite one", "the docker one"],
                Some(Kind::Question),
            )),
            (60, 14),
        );

        assert_eq!(heading_of(&screen[3]), "NEEDS INPUT", "{screen:?}");
        assert!(
            screen[4].contains("ask-a1b"),
            "the row the card was opened from is still on the screen: {screen:?}"
        );

        let top = screen
            .iter()
            .position(|line| line.starts_with('╭'))
            .expect("the top of the card");
        assert!(
            screen[top].contains("ask-a1b · waiting 29s"),
            "which agent, what it is doing, and how long since: {:?}",
            screen[top]
        );
        assert!(screen[top].ends_with('╮'), "{:?}", screen[top]);
        assert!(
            screen[top + 1].contains("Which fixture should the port keep?"),
            "{:?}",
            screen[top + 1]
        );
        assert!(
            !screen.iter().any(|line| line.contains("Do you want to")),
            "and the pane it is asking on is not echoed under it: {screen:?}"
        );

        let bottom = screen
            .iter()
            .rposition(|line| line.starts_with('╰'))
            .expect("the foot of the card");
        assert!(screen[bottom].ends_with('╯'), "{:?}", screen[bottom]);
        assert_eq!(
            bottom + 2,
            screen.len(),
            "and the hint row is the one beneath it: {screen:?}"
        );
    }

    #[test]
    fn card_numbers_the_choices_the_question_offers() {
        let screen = drawn(
            a_fleet(),
            Some(asking(
                &["the sqlite one", "the docker one"],
                Some(Kind::Question),
            )),
            (60, 14),
        );
        assert!(
            screen
                .iter()
                .any(|line| line.contains("1. the sqlite one   2. the docker one")),
            "numbered the way every surface numbers them: {screen:?}"
        );
    }

    /// The same card, with somebody part way through typing the answer to it.
    fn answering(card: Card, typed: &str) -> Screen {
        let mut screen = showing(a_fleet(), Some(card));
        let mut composer = Composer::new(Asking::Reply {
            id: "ask-a1b".to_string(),
            question: true,
        });
        composer.text = typed.to_string();
        screen.mode = Mode::Typing(composer);
        screen
    }

    /// The row of the card the answer is typed on.
    fn answer_row(screen: &[String]) -> String {
        screen
            .iter()
            .find(|line| line.contains('❯'))
            .unwrap_or_else(|| panic!("no row to answer on in: {screen:?}"))
            .clone()
    }

    #[test]
    fn card_takes_the_answer_on_a_row_of_the_card_itself() {
        let question = || asking(&["the sqlite one", "the docker one"], Some(Kind::Question));

        let empty = painted(&answering(question(), ""), (60, 14));
        assert!(
            answer_row(&empty).contains("❯ press 1-2, or type an answer"),
            "an empty row says what the question will take: {:?}",
            answer_row(&empty)
        );
        assert_eq!(
            empty[13], ANSWERS,
            "and the row under the card says what its own keys do"
        );

        let typed = painted(&answering(question(), "the docker one"), (60, 14));
        assert!(
            answer_row(&typed).contains("❯ the docker one"),
            "{:?}",
            answer_row(&typed)
        );
        assert!(
            !typed.iter().any(|line| line.contains("type an answer")),
            "what was typed takes the row the invitation had: {typed:?}"
        );
        assert!(
            !typed.iter().any(|line| line.starts_with("answer ask-a1b")),
            "and the line is on the card rather than on a band of its own \
             under it: {typed:?}"
        );
        assert_eq!(
            caret(&answering(question(), "the docker one"), (60, 14)),
            (18, 11),
            "with the terminal's own cursor at the end of what was typed, on \
             a card that is the question block's own size"
        );
    }

    #[test]
    fn card_is_no_taller_than_what_it_has_to_show() {
        // An agent whose answer is one line does not want seven rows of box to
        // say it in, and every row the card leaves is a row of the wall.
        let brief = Card {
            phase: Phase::Done,
            question: None,
            options: Vec::new(),
            body: "did what it was asked".to_string(),
            ..asking(&[], None)
        };
        let screen = drawn(a_fleet(), Some(brief), (60, 20));
        let top = screen
            .iter()
            .position(|line| line.starts_with('╭'))
            .expect("the top of the card");
        let bottom = screen
            .iter()
            .rposition(|line| line.starts_with('╰'))
            .expect("the foot of the card");

        assert_eq!(
            bottom - top,
            2,
            "two borders and the one line it has: {screen:?}"
        );
        assert!(
            screen[top + 1].contains("did what it was asked"),
            "{screen:?}"
        );
        assert_eq!(
            screen[top - 1],
            "",
            "with the rows it is not covering behind it: {screen:?}"
        );
    }

    #[test]
    fn card_keeps_the_row_being_typed_on_when_there_is_room_for_little_else() {
        // A card with one row inside its borders. What somebody is typing is
        // what that row is for: the question is on the agent's row behind the
        // card, and the line is nowhere else at all.
        let screen = painted(
            &answering(asking(&["the sqlite one"], Some(Kind::Question)), "the sq"),
            (60, 6),
        );
        assert!(answer_row(&screen).contains("❯ the sq"), "{screen:?}");
        assert_eq!(screen[5], ANSWERS, "with the card's own keys under it");
    }

    #[test]
    fn card_invites_only_the_answers_the_question_will_take() {
        // A permission box has no field for words: they would land on whatever
        // is highlighted, which is an answer nobody chose.
        let box_office = Card {
            kind: Some(Kind::Permission),
            question: Some("Claude needs your permission to use Bash".to_string()),
            ..asking(&["Yes", "No"], None)
        };
        let asked = answer_row(&painted(&answering(box_office, ""), (60, 14)));
        assert!(asked.contains("❯ press 1-2, y or n"), "{asked:?}");
        assert!(
            !asked.contains("type"),
            "a hint that offers what the prompt will refuse is a hint that \
             lies: {asked:?}"
        );

        // And a card nobody is answering has the list's own keys under it.
        let looking = painted(&showing(a_fleet(), Some(asking(&[], None))), (60, 14));
        assert_eq!(
            looking[13],
            "space closes it · enter attach · ctrl+x stop · ? keys"
        );
        assert!(
            !looking.iter().any(|line| line.contains('❯')),
            "with no row to answer on: {looking:?}"
        );
    }

    #[test]
    fn card_packs_the_choices_onto_as_few_rows_as_it_is_wide() {
        let two = ["the sqlite one".to_string(), "the docker one".to_string()];
        assert_eq!(
            choices(&two, 40, false),
            ["1. the sqlite one   2. the docker one"]
        );
        assert_eq!(
            choices(&two, 20, false),
            ["1. the sqlite one", "2. the docker one"],
            "and one to a row where they will not sit together"
        );
        assert_eq!(
            choices(&two, 10, false),
            ["1. the sq…", "2. the do…"],
            "a choice wider than the card is cut, and says it was"
        );
        assert!(choices(&[], 40, false).is_empty());
    }

    #[test]
    fn card_gives_a_question_that_takes_several_a_box_beside_every_choice() {
        let two = ["the sqlite one".to_string(), "the docker one".to_string()];
        assert_eq!(
            choices(&two, 50, true),
            ["1. [ ] the sqlite one   2. [ ] the docker one"],
            "the vendor's own box, between the number and the label"
        );
        assert_eq!(
            choices(&two, 20, true),
            ["1. [ ] the sqlite o…", "2. [ ] the docker o…"],
            "and a narrow card cuts the label rather than the box, because the \
             box is what says the row is one"
        );
    }

    #[test]
    fn glyphs_give_every_state_a_mark_of_its_own() {
        let marks: Vec<&str> = EVERY.iter().map(|phase| resting(*phase)).collect();
        assert_eq!(
            marks
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            EVERY.len(),
            "eight states, eight marks: {marks:?}"
        );
        assert_eq!(resting(Phase::Waiting), "?");
        assert_eq!(resting(Phase::Starting), "◌");
        assert_eq!(resting(Phase::Idle), "○");
        assert_eq!(resting(Phase::Done), "●");
        assert_eq!(resting(Phase::Failed), "✗");
        assert_eq!(resting(Phase::Stopped), "⏹");
        assert_eq!(resting(Phase::Unknown), "~");

        for phase in EVERY.iter().filter(|phase| **phase != Phase::Working) {
            assert!(
                !set().contains(&resting(*phase)),
                "{phase} rests on a mark the pulse passes through"
            );
        }
    }

    #[test]
    fn glyphs_pulse_a_working_row_through_twelve_frames() {
        let set = set();
        let want: Vec<&str> = set.iter().chain(set.iter().rev()).copied().collect();
        let frames: Vec<&str> = (0..12).map(pulse).collect();

        assert_eq!(frames, want, "the set, and then the set backwards");
        assert_eq!(pulse(12), pulse(0), "and round again");
        assert_eq!(
            resting(Phase::Working),
            set[LIVE],
            "and it rests on the vendor's own live glyph"
        );
    }

    #[test]
    fn glyphs_take_the_set_the_terminal_asks_for() {
        assert_eq!(set_for("xterm-ghostty"), ["·", "✢", "✳", "✶", "✻", "✻"]);
        assert_eq!(set_for("tmux-256color"), ["·", "✢", "*", "✶", "✻", "✽"]);
        assert_eq!(
            set_for(""),
            set_for("xterm"),
            "and anything else is the same"
        );
    }

    #[test]
    fn glyphs_leave_the_colour_to_say_how_it_went() {
        // The mark on the one row a view of one agent draws.
        let painted = |phase| {
            let screen = showing(vec![view("agent-a1b", phase, Some("said"), 5)], None);
            mark(&screen, (60, 8), 2)
        };
        let plain = Modifier::empty();

        // The one glyph with weight on it, which is the one the view is
        // opened to find.
        assert_eq!(
            painted(Phase::Waiting),
            ("?".into(), theme().waiting, Modifier::BOLD)
        );
        assert_eq!(
            painted(Phase::Unknown),
            ("~".into(), theme().waiting, plain)
        );
        assert_eq!(painted(Phase::Done), ("●".into(), theme().done, plain));
        assert_eq!(painted(Phase::Failed), ("✗".into(), theme().failed, plain));
        assert_eq!(
            painted(Phase::Stopped),
            ("⏹".into(), theme().stopped, plain)
        );

        // An agent still at work has nothing to say about how it went, so it
        // takes the terminal's own colour and the pulse does the talking. An
        // agent that has finished its turn and is sitting there is quiet.
        assert_eq!(painted(Phase::Starting), ("◌".into(), Color::Reset, plain));
        assert_eq!(
            painted(Phase::Working),
            (pulse(0).into(), Color::Reset, plain)
        );
        assert_eq!(
            painted(Phase::Idle),
            ("○".into(), Color::Reset, Modifier::DIM)
        );
    }

    #[test]
    fn glyphs_draw_a_working_row_a_frame_at_a_time() {
        let at = |beat| {
            let mut screen = showing(
                vec![view("port-import-b2c", Phase::Working, Some("Running"), 3)],
                None,
            );
            screen.beat = beat;
            painted(&screen, (60, 8))[2].clone()
        };

        assert!(
            at(0).starts_with(&format!("  {} port-import-b2c", pulse(0))),
            "{:?}",
            at(0)
        );
        assert_ne!(at(0), at(LIVE), "a working row moves");
    }

    #[test]
    fn view_draws_a_row_for_every_agent_under_a_heading_for_its_group() {
        let screen = drawn(
            vec![
                view("ask-a1b", Phase::Waiting, None, 90),
                view("fix-login-b2c", Phase::Working, Some("Running Bash"), 3),
            ],
            None,
            (60, 10),
        );

        assert!(
            screen[0].ends_with("1 working   2/5 running    1 WAITING"),
            "{:?}",
            screen[0]
        );
        assert_eq!(heading_of(&screen[2]), "NEEDS INPUT");
        assert!(
            screen[3].starts_with("• ? ask-a1b"),
            "a question nobody has been to read carries the mark that says so: \
             {:?}",
            screen[3]
        );
        assert!(screen[3].ends_with("1m"), "{:?}", screen[3]);
        assert_eq!(screen[4], "", "the next group stands off from this one");
        assert_eq!(heading_of(&screen[5]), "WORKING");
        assert!(
            screen[6].starts_with(&format!("  {} fix-login-b2c", pulse(0))),
            "{:?}",
            screen[6]
        );
        assert!(screen[6].contains("Running Bash"), "{:?}", screen[6]);
        assert!(screen[6].ends_with("3s"), "{:?}", screen[6]);
        assert_eq!(
            screen[9], "space card · enter attach · ctrl+x stop · ? keys",
            "and the keys, where they can be read"
        );
    }

    #[test]
    fn view_keeps_a_row_to_one_line_however_much_the_agent_said() {
        let screen = drawn(
            vec![view(
                "fix-login-a1b",
                Phase::Idle,
                Some("I fixed it.\n\nHere is what I changed:\n- the parser"),
                1,
            )],
            None,
            (60, 8),
        );
        assert!(screen[2].contains("I fixed it."), "{:?}", screen[2]);
        assert!(
            !screen.iter().any(|line| line.contains("the parser")),
            "{screen:?}"
        );
    }

    #[test]
    fn view_cuts_what_will_not_fit_rather_than_losing_the_age() {
        let screen = drawn(
            vec![view(
                "fix-login-a1b",
                Phase::Working,
                Some("Editing a file with a very long name indeed, and then some"),
                45,
            )],
            None,
            (40, 8),
        );
        assert!(screen[2].contains('…'), "{:?}", screen[2]);
        assert!(screen[2].ends_with("45s"), "{:?}", screen[2]);
        assert!(screen[2].chars().count() <= 40, "{:?}", screen[2]);
    }

    #[test]
    fn view_says_on_an_armed_row_what_the_next_press_would_do_to_it() {
        let size = (60, 8);
        let mut screen = showing(
            vec![
                view("fix-login-a1b", Phase::Done, Some("wrote the parser"), 60),
                view(
                    "port-importer-b2c",
                    Phase::Done,
                    Some("wrote the tests"),
                    90,
                ),
            ],
            None,
        );
        assert!(painted(&screen, size)[2].contains("wrote the parser"));

        screen.arm = Some(Arm {
            ids: vec!["fix-login-a1b".to_string()],
            swept: false,
            at: Instant::now(),
        });
        let drawn = painted(&screen, size);
        assert!(
            drawn[2].contains("ctrl+x again forgets"),
            "the row says it where it was saying what the agent did: {:?}",
            drawn[2]
        );
        assert!(
            !drawn[2].contains("wrote the parser"),
            "in place of the summary rather than beside it: {:?}",
            drawn[2]
        );
        assert!(
            drawn[2].ends_with("1m"),
            "and the columns either side of it are where they were: {:?}",
            drawn[2]
        );
        assert_eq!(
            word_colour(&screen, size, 2, "ctrl+x again forgets"),
            theme().waiting,
            "in the colour of a thing waiting on a person"
        );
        assert!(
            drawn[3].contains("wrote the tests"),
            "and the rows nobody armed say what they always said: {:?}",
            drawn[3]
        );
    }

    #[test]
    fn axis_heads_the_rows_with_the_project_and_gives_each_one_its_state() {
        let screen = painted(
            &by_project(vec![
                at(view("ask-a1b", Phase::Waiting, None, 30), "/src/api"),
                at(
                    view("fix-login-b2c", Phase::Done, Some("fixed it"), 30),
                    "/src/api",
                ),
                at(view("busy-c3d", Phase::Working, None, 3), "/src/web"),
            ]),
            (60, 10),
        );

        assert_eq!(screen[2], "/src/api");
        assert!(screen[3].contains("ask-a1b"), "{:?}", screen[3]);
        assert!(
            screen[3].contains("waiting"),
            "the heading is a place, so the row says the state: {:?}",
            screen[3]
        );
        assert!(screen[4].contains("done"), "{:?}", screen[4]);
        assert_eq!(screen[5], "", "the next project stands off from this one");
        assert_eq!(screen[6], "/src/web");

        // One column, so the states read down the screen rather than wandering
        // with the length of the name above them. Counted in characters: the
        // marks are not all one byte, and a column is what a person sees.
        let column = |line: &str, word: &str| {
            let at = line.find(word).expect("the state on the row");
            line[..at].chars().count()
        };
        assert_eq!(column(&screen[3], "waiting"), column(&screen[4], "done"));
    }

    #[test]
    fn axis_leaves_the_state_off_a_row_the_heading_over_it_already_says() {
        let screen = painted(
            &showing(vec![view("busy-a1b", Phase::Working, None, 3)], None),
            (60, 8),
        );
        assert_eq!(heading_of(&screen[1]), "WORKING");
        assert!(
            !screen[2].contains("working"),
            "twice on one screen is a column of noise: {:?}",
            screen[2]
        );
    }

    #[test]
    fn axis_says_at_the_top_what_the_list_was_narrowed_to() {
        let mut screen = showing(
            vec![
                view("busy-a1b", Phase::Working, None, 3),
                view("done-b2c", Phase::Done, None, 60),
            ],
            None,
        );
        screen
            .list
            .narrow(vec![Narrow::State(Some("working".to_string()))]);

        let painted = painted(&screen, (60, 8));
        assert!(
            painted[0].ends_with("1 working   1/5 running   s:working   nothing waiting"),
            "{:?}",
            painted[0]
        );
        assert!(painted[2].contains("busy-a1b"), "{:?}", painted[2]);
        assert!(
            !painted.iter().any(|line| line.contains("done-b2c")),
            "a hidden agent is not counted, not drawn and not headed: {painted:?}"
        );
    }

    #[test]
    fn axis_says_nothing_matches_rather_than_claiming_there_are_no_agents() {
        let mut screen = showing(vec![view("busy-a1b", Phase::Working, None, 3)], None);
        screen
            .list
            .narrow(vec![Narrow::Name(Some("nobody".to_string()))]);

        assert_eq!(painted(&screen, (60, 8))[1], "nothing matches a:nobody");
    }

    #[test]
    fn axis_says_a_line_that_narrows_will_narrow_rather_than_start_anything() {
        let mut screen = showing(Vec::new(), None);
        let mut composer = Composer::new(Asking::Task);
        composer.text = "s:waiting".to_string();
        screen.mode = Mode::Typing(composer);

        let painted = painted(&screen, (60, 6));
        assert_eq!(painted[4], "narrow ▸ s:waiting");
        assert!(painted[5].contains("enter narrows it"), "{:?}", painted[5]);
        assert!(
            !painted[5].contains("starts it"),
            "a hint that says the other thing is a hint that lies: {:?}",
            painted[5]
        );
    }

    /// The background of every cell across one row of the list.
    fn behind(screen: &Screen, size: (u16, u16), row: u16) -> Vec<Color> {
        let buffer = cells(screen, size);
        (0..size.0).map(|at| buffer[(at, row)].bg).collect()
    }

    #[test]
    fn a_cursor_on_a_headings_line_is_marked_the_way_a_cursor_on_a_row_is() {
        let mut screen = showing(
            vec![
                view("busy-a1b", Phase::Working, None, 3),
                view("busy-b2c", Phase::Working, None, 5),
            ],
            None,
        );
        let bar = vec![theme().cursor; 60];
        let plain = vec![Color::Reset; 60];

        // The view opens on the first agent, with the heading over it bare.
        assert_eq!(behind(&screen, (60, 8), 2), bar, "the row the cursor is on");
        assert_eq!(behind(&screen, (60, 8), 1), plain, "and not the heading");

        screen.list.up();
        assert_eq!(
            behind(&screen, (60, 8), 1),
            bar,
            "a heading is a line like any other, so the cursor looks the same \
             on it: column zero to the last column, over a label that is a \
             third of that"
        );
        assert_eq!(behind(&screen, (60, 8), 2), plain);
    }

    #[test]
    fn a_headings_bar_is_the_only_thing_that_says_where_the_cursor_is() {
        let painted = painted(
            &showing(
                vec![view("busy-a1b", Phase::Working, Some("Running"), 3)],
                None,
            ),
            (60, 8),
        );
        assert!(
            painted[2].starts_with(&format!("  {} busy-a1b", pulse(0))),
            "a row reads the same whether or not the cursor is on it: {:?}",
            painted[2]
        );
    }

    #[test]
    fn headings_count_their_agents_whether_or_not_the_rows_are_under_them() {
        let mut screen = showing(
            vec![
                view("busy-a1b", Phase::Working, None, 3),
                view("busy-b2c", Phase::Working, None, 5),
            ],
            None,
        );

        assert_eq!(
            counted(&painted(&screen, (60, 8))[1]),
            "2",
            "the margin of a screen is a line of numbers, open or shut"
        );

        screen.list.up();
        screen.list.shut_or_open();
        let painted = painted(&screen, (60, 8));
        assert_eq!(counted(&painted[1]), "2");
        assert!(
            !painted.iter().any(|line| line.contains("busy-a1b")),
            "and shut, the count is all that is standing in for them: {painted:?}"
        );
    }

    #[test]
    fn headings_say_how_many_failed_whether_or_not_the_rows_are_under_them() {
        let mut screen = showing(
            vec![
                view("done-a1b", Phase::Done, Some("did it"), 60),
                view("broke-b2c", Phase::Failed, Some("could not"), 60),
            ],
            None,
        );

        assert_eq!(
            heading_of(&painted(&screen, (60, 8))[1]),
            "COMPLETED · 1 failed",
            "a screenful of headings says how it went without being opened"
        );

        screen.list.up();
        screen.list.shut_or_open();
        assert_eq!(
            heading_of(&painted(&screen, (60, 8))[1]),
            "COMPLETED · 1 failed",
            "shutting a group hides the detail of a failure, never the fact"
        );
    }

    #[test]
    fn card_neutralises_the_question_and_the_choices_it_quotes() {
        // The question is the agent's own words, and a bidirectional override
        // written into them can visually reorder the choices underneath —
        // which are the keys a person is about to press. ratatui drops the
        // control characters on its own; the invisible format characters it
        // keeps have to be neutralised before anything draws them.
        let mut card = asking(&["yes\u{200b}really", "no\u{ad}pe"], Some(Kind::Question));
        card.question = Some("pro\u{ad}ceed\u{202e}?".to_string());
        let screen = drawn(a_fleet(), Some(card), (60, 14)).join("\n");

        for (invisible, name) in [
            ('\u{202e}', "a bidi override"),
            ('\u{200b}', "a zero-width space"),
            ('\u{ad}', "a soft hyphen"),
        ] {
            assert!(
                !screen.contains(invisible),
                "{name} reached the terminal: {screen:?}"
            );
        }
        assert!(screen.contains("pro ceed"), "{screen:?}");
    }

    #[test]
    fn headings_stand_off_from_whatever_is_above_them() {
        // A blank line above every heading, so the groups read as groups
        // instead of one run of rows — and the first of them is stood off from
        // the header the same way, so the list starts where the chrome ends
        // rather than against it.
        let screen = drawn(a_fleet(), None, (60, 12));
        assert!(screen[0].contains("running"), "the header: {screen:?}");
        assert_eq!(screen[2], "", "the space over the list");
        assert_eq!(heading_of(&screen[3]), "NEEDS INPUT", "the first heading");
        assert!(screen[4].contains("ask-a1b"), "{screen:?}");
        assert_eq!(screen[5], "", "a blank line stands the next group off");
        assert_eq!(heading_of(&screen[6]), "WORKING");
        assert!(screen[7].contains("busy-b2c"), "{screen:?}");
    }

    #[test]
    fn the_space_over_the_list_is_the_first_row_a_short_screen_takes_back() {
        // Air is worth a row where there are rows to spare and not where there
        // are none: the header has already given its second row up by then,
        // and this one goes the same way.
        let tall = drawn(a_fleet(), None, (60, SPACED as u16));
        assert_eq!(tall[2], "", "{tall:?}");
        assert_eq!(heading_of(&tall[3]), "NEEDS INPUT", "{tall:?}");

        let short = drawn(a_fleet(), None, (60, SPACED as u16 - 1));
        assert_eq!(heading_of(&short[2]), "NEEDS INPUT", "{short:?}");
        assert!(short[3].contains("ask-a1b"), "{short:?}");
    }

    #[test]
    fn headings_carry_the_weight_on_the_label_and_none_of_it_on_the_rule() {
        // Case and weight are what make a heading here, with no second type
        // size to make it with, and every heading wears them: where the cursor
        // is standing is said by the bar under one line, not by the headings
        // around it putting weight down and picking it up.
        let screen = showing(a_fleet(), None);
        let cells = cells(&screen, (60, 10));
        for row in [2, 5] {
            let label = cells[(1, row)].clone();
            assert!(
                label.modifier.contains(Modifier::BOLD),
                "the heading on row {row} is bold: {:?}",
                label.modifier
            );
        }
        assert_eq!(
            cells[(1, 2)].fg,
            theme().waiting,
            "the group that wants a person is the one carrying colour up here"
        );
        assert_eq!(
            cells[(1, 5)].fg,
            Color::Reset,
            "and the rest of them do not"
        );

        // The rule that carries the label out to its count carries none of the
        // weight, which is what leaves the label the loud thing on the line.
        let rule = cells[(30, 2)].clone();
        assert_eq!(rule.symbol(), "─", "the rule runs out to the count");
        assert!(
            rule.modifier.contains(Modifier::DIM) && !rule.modifier.contains(Modifier::BOLD),
            "{:?}",
            rule.modifier
        );
    }

    #[test]
    fn view_says_when_there_is_nothing_to_show() {
        let screen = drawn(Vec::new(), None, (40, 6));
        assert!(screen[0].starts_with("AMX"), "{:?}", screen[0]);
        assert!(
            screen[0].ends_with("0/5 running   nothing waiting"),
            "{:?}",
            screen[0]
        );
        assert_eq!(screen[1], "no agents");
    }

    /// A screen with room for the whole header, at the width the mockup was
    /// drawn at.
    const WIDE: (u16, u16) = (100, 12);

    /// The view with a launch profile that says where it is running: the
    /// directory is read from the disk when a real view opens, and a test says
    /// what the disk would have answered.
    fn launching(views: Vec<View>) -> Screen {
        let mut screen = showing(views, None);
        screen.profile.dir = "~/code/amx".to_string();
        screen
    }

    /// One line of what a view of this size draws.
    fn screen_line(screen: &Screen, size: (u16, u16), row: usize) -> String {
        painted(screen, size)[row].clone()
    }

    /// Which column of a drawn line a word starts at, for the tests that ask
    /// what the view painted it in. Columns, not bytes: the separator between
    /// two things said on one row is two bytes wide and one column.
    fn column_of(line: &str, word: &str) -> u16 {
        let at = line.find(word).expect("the word is on the line");
        line[..at].chars().count() as u16
    }

    #[test]
    fn header_says_where_it_is_and_what_the_fleet_is_over_the_dials() {
        let screen = painted(
            &launching(vec![
                view("ask-a1b", Phase::Waiting, None, 30),
                view("busy-b2c", Phase::Working, Some("Running Bash"), 3),
            ]),
            WIDE,
        );

        assert!(
            screen[0].starts_with("AMX  ~/code/amx"),
            "whose screen this is and where it was opened: {:?}",
            screen[0]
        );
        assert!(
            !screen[0].contains(env!("CARGO_PKG_VERSION")),
            "which version this is says nothing about the fleet: {:?}",
            screen[0]
        );
        assert!(
            screen[0].ends_with("1 working   2/5 running    1 WAITING"),
            "what the fleet is, the gate the next one meets, and the one count \
             that wants somebody at the end of the row: {:?}",
            screen[0]
        );
        assert_eq!(
            screen[1], "└ next  claude   model  default   permission  default   worktree  new",
            "and under it every dial the next agent will be started with"
        );
        assert_eq!(screen[2], "", "a blank row stands the list off from it");
        assert_eq!(
            heading_of(&screen[3]),
            "NEEDS INPUT",
            "and the list starts under that"
        );
    }

    #[test]
    fn header_spends_its_one_colour_on_the_count_that_wants_a_person() {
        let screen = launching(vec![
            view("ask-a1b", Phase::Waiting, None, 30),
            view("ask-b2c", Phase::Waiting, None, 10),
            view("busy-c3d", Phase::Working, Some("Running Bash"), 3),
        ]);
        let drawn = painted(&screen, WIDE);

        assert!(
            drawn[0].ends_with(" 2 WAITING"),
            "the one number the view was opened for, at the end of the row: {:?}",
            drawn[0]
        );
        assert!(
            drawn[0].contains("1 working   3/5 running"),
            "the counts beside it say the rest of the fleet, and say the \
             waiting one nowhere else: {:?}",
            drawn[0]
        );

        // A block rather than a phrase: reverse video in the waiting colour,
        // out to the edge of the row, the space either side of the words
        // included.
        let buffer = cells(&screen, WIDE);
        for column in column_of(&drawn[0], " 2 WAITING")..WIDE.0 {
            let cell = buffer[(column, 0)].clone();
            assert_eq!(cell.fg, theme().waiting, "column {column}: {:?}", drawn[0]);
            assert!(
                cell.modifier.contains(Modifier::REVERSED | Modifier::BOLD),
                "column {column}: {:?}",
                cell.modifier
            );
        }
    }

    #[test]
    fn header_says_nothing_waiting_in_words_where_nobody_is() {
        let screen = launching(vec![view(
            "busy-a1b",
            Phase::Working,
            Some("Running Bash"),
            3,
        )]);
        let drawn = painted(&screen, WIDE);

        assert!(
            drawn[0].ends_with("nothing waiting"),
            "the answer stands where the answer always stands: {:?}",
            drawn[0]
        );

        let buffer = cells(&screen, WIDE);
        let cell = buffer[(column_of(&drawn[0], "nothing waiting"), 0)].clone();
        assert_eq!(
            cell.fg,
            Color::Reset,
            "nothing is asking, so nothing shouts"
        );
        assert!(cell.modifier.contains(Modifier::DIM), "{:?}", cell.modifier);
        assert!(
            !cell.modifier.contains(Modifier::REVERSED),
            "{:?}",
            cell.modifier
        );
    }

    #[test]
    fn header_hangs_the_dials_off_the_row_they_are_under() {
        let screen = launching(Vec::new());
        let drawn = painted(&screen, WIDE);
        assert_eq!(
            drawn[1], "└ next  claude   model  default   permission  default   worktree  new",
            "one glyph in the first column says the row is subordinate to the \
             one above it, without a word of explanation"
        );

        let buffer = cells(&screen, WIDE);
        for label in ["└", "next", "model", "permission", "worktree"] {
            let cell = buffer[(column_of(&drawn[1], label), 1)].clone();
            assert_eq!(cell.fg, Color::Reset, "{label}: {:?}", drawn[1]);
            assert!(
                cell.modifier.contains(Modifier::DIM),
                "{label}: {:?}",
                cell.modifier
            );
        }
        // The values are what somebody reads the row for, so they are the
        // thing on it wearing a colour.
        for value in ["claude", "new"] {
            let cell = buffer[(column_of(&drawn[1], value), 1)].clone();
            assert_eq!(cell.fg, theme().accent, "{value}: {:?}", drawn[1]);
            assert!(
                !cell.modifier.contains(Modifier::DIM),
                "{value}: {:?}",
                cell.modifier
            );
        }
    }

    #[test]
    fn header_drops_the_dial_labels_before_it_cuts_what_they_are_set_to() {
        let screen = launching(Vec::new());
        assert_eq!(
            screen_line(&screen, (60, 12), 1),
            "└ next  claude  ·  default  ·  default  ·  new",
            "the value is the reading; the label is what a person already knows \
             the order of. Only `next` keeps its own, because it is what says \
             which half of the screen the row is about"
        );
    }

    #[test]
    fn header_names_a_dial_that_rests_where_the_vendor_left_it() {
        let mut screen = launching(Vec::new());
        assert_eq!(
            screen_line(&screen, WIDE, 1),
            "└ next  claude   model  default   permission  default   worktree  new",
            "the vendor's own answer said as a value, not a guess at which \
             model claude would have picked"
        );

        // Turned, the value is what it was turned to. The label does not move,
        // so the row a person has learned to read stays the row they read.
        screen.profile.model = "opus".to_string();
        screen.profile.permission = "plan".to_string();
        screen.profile.worktree = false;
        assert_eq!(
            screen_line(&screen, WIDE, 1),
            "└ next  claude   model  opus   permission  plan   worktree  none"
        );

        // An agent the registry never heard of declares no dials, so the row
        // holds the vendor and the one dial that is amx's own.
        screen.profile.agent = "mock-claude".to_string();
        assert_eq!(
            screen_line(&screen, WIDE, 1),
            "└ next  mock-claude   worktree  none"
        );
    }

    #[test]
    fn header_counts_the_fleet_in_the_words_a_filter_takes() {
        let mut screen = launching(vec![
            view("ask-a1b", Phase::Waiting, None, 30),
            view("done-b2c", Phase::Done, Some("did it"), 60),
        ]);
        assert!(
            screen_line(&screen, WIDE, 0).ends_with("1 done   1/5 running    1 WAITING"),
            "the heading over the rows says `needs input`; the counter says \
             the word the list can be narrowed by, and says the waiting one \
             once, in the badge: {:?}",
            screen_line(&screen, WIDE, 0)
        );

        // A narrowing is still read back where it was typed, so a short list
        // says why it is short.
        screen
            .list
            .narrow(vec![Narrow::State(Some("waiting".to_string()))]);
        assert!(
            screen_line(&screen, WIDE, 0).ends_with("1/5 running   s:waiting    1 WAITING"),
            "{:?}",
            screen_line(&screen, WIDE, 0)
        );
    }

    #[test]
    fn header_says_the_gate_the_next_agent_meets_before_it_refuses() {
        let mut screen = launching(vec![
            view("busy-a1b", Phase::Working, None, 3),
            view("busy-b2c", Phase::Working, None, 3),
            view("busy-c3d", Phase::Working, None, 3),
            view("done-d4e", Phase::Done, Some("did it"), 60),
        ]);
        screen.profile.max = 5;
        assert!(
            screen_line(&screen, WIDE, 0).contains("3/5 running"),
            "an agent whose command has ended holds no slot: {:?}",
            screen_line(&screen, WIDE, 0)
        );
    }

    #[test]
    fn header_sheds_the_dir_before_the_name_and_the_vendor_before_a_dial() {
        // Decided here rather than discovered at the edge of a terminal.
        let mut screen = launching(vec![view("busy-a1b", Phase::Working, None, 3)]);

        let cramped = painted(&screen, (28, 12));
        assert!(
            cramped[0].starts_with("AMX"),
            "the name says what the screen is, and it is three columns: {:?}",
            cramped[0]
        );
        assert!(
            !cramped[0].contains("code/amx"),
            "a path cut to nothing is not a path: {:?}",
            cramped[0]
        );

        // A vendor is a command line, and a command is routinely a long one.
        // It gives way to the dials beside it: a dial cut off the end of the
        // row is a dial nobody can see they have turned.
        screen.profile.agent = "claude --settings /etc/amx/every-hook.json".to_string();
        let long = painted(&screen, (80, 12));
        assert!(long[1].starts_with("└ next  claude --set"), "{:?}", long[1]);
        assert!(
            long[1].contains('…'),
            "and it says it was cut: {:?}",
            long[1]
        );
        assert!(
            long[1].ends_with("permission  default   worktree  new"),
            "{:?}",
            long[1]
        );

        // Narrower still and the labels go first, which buys the vendor ten
        // columns before a dial gives up a character of its value.
        assert_eq!(
            screen_line(&screen, (50, 12), 1),
            "└ next  claude --…  ·  default  ·  default  ·  new"
        );

        // Narrower again and there is no room for all of it either way. What
        // the vendor keeps is a floor: a row that had fitted every dial on
        // the screen by leaving off what runs would be a row about nothing.
        let narrow = screen_line(&screen, (36, 12), 1);
        assert!(narrow.starts_with("└ next  claude …"), "{narrow:?}");
        assert!(
            narrow.ends_with('…'),
            "and the end of the row is what says it was cut: {narrow:?}"
        );
    }

    #[test]
    fn header_sheds_the_counts_before_the_one_that_wants_a_person() {
        // Every group at once, which is more counting than a narrow terminal
        // has room for beside the name. What goes is the counting: the badge
        // is the answer the view was opened to read.
        let screen = launching(vec![
            view("ask-a1b", Phase::Waiting, None, 30),
            view("busy-b2c", Phase::Working, None, 3),
            view("idle-c3d", Phase::Idle, None, 30),
            view("done-d4e", Phase::Done, Some("did it"), 60),
        ]);

        assert!(
            screen_line(&screen, (60, 12), 0).contains("1 working   1 idle   1 done   3/5 running"),
            "{:?}",
            screen_line(&screen, (60, 12), 0)
        );

        let cramped = screen_line(&screen, (40, 12), 0);
        assert!(cramped.starts_with("AMX  ~/code/amx"), "{cramped:?}");
        assert!(cramped.ends_with(" 1 WAITING"), "{cramped:?}");
        assert!(
            !cramped.contains("running"),
            "and the counting is what gave the room up: {cramped:?}"
        );
    }

    #[test]
    fn header_gives_the_row_back_to_the_list_on_a_short_screen() {
        let screen = launching(vec![view("busy-a1b", Phase::Working, None, 3)]);
        let short = painted(&screen, (60, SHORT as u16 - 1));

        assert!(
            short[0].starts_with("AMX  ~/code/amx"),
            "the row that says what there is stays; the dials are one \
             keypress from being read under the composer: {:?}",
            short[0]
        );
        assert!(
            short[0].ends_with("1 working   1/5 running   nothing waiting"),
            "{:?}",
            short[0]
        );
        assert!(!short.iter().any(|line| line.starts_with('└')), "{short:?}");
        assert_eq!(
            heading_of(&short[1]),
            "WORKING",
            "and the list starts a row sooner"
        );
    }

    /// A screen with room for the bands above and below the list, the space
    /// between the header and it, and a group or two under that.
    const WALL: (u16, u16) = (80, 12);

    #[test]
    fn a_wall_nobody_has_put_anything_on_says_so_in_one_line_of_its_own() {
        let screen = drawn(Vec::new(), None, WALL);

        assert_eq!(screen[3], WELCOME, "{screen:?}");
        // Everything under it down to the keys is the empty wall itself: one
        // line where the four groups used to have a sentence each.
        assert!(
            screen[4..screen.len() - 1].iter().all(String::is_empty),
            "one line, and no more: {screen:?}"
        );
        for group in Group::ALL {
            assert!(
                !screen.iter().any(|line| line.contains(group.title())),
                "{} stands over rows, and there are none: {screen:?}",
                group.title()
            );
        }
    }

    #[test]
    fn the_wall_says_it_plainly_where_the_line_of_its_own_will_not_fit() {
        // Said whole or not at all: a sentence cut by the terminal reads as a
        // sentence that ends where the screen does, and this one is a joke as
        // well, which is worse to be handed two thirds of.
        let narrow = drawn(
            Vec::new(),
            None,
            (WELCOME.chars().count() as u16 - 1, WALL.1),
        );
        assert_eq!(narrow[3], "no agents");
        let wide = drawn(Vec::new(), None, (WELCOME.chars().count() as u16, WALL.1));
        assert_eq!(wide[3], WELCOME);
    }

    #[test]
    fn the_wall_has_its_line_to_itself_and_gives_it_up_to_the_first_row() {
        let one = drawn(
            vec![view("done-a1b", Phase::Done, Some("did it"), 60)],
            None,
            WALL,
        );
        assert_eq!(heading_of(&one[3]), "COMPLETED");
        assert!(
            !one.iter().any(|line| line.contains("nobody asking")),
            "one agent and there is something to read off the rows: {one:?}"
        );

        // A fleet somebody narrowed to nothing is not a fleet nobody started,
        // and the view owes them the words they typed rather than a joke.
        let mut screen = showing(Vec::new(), None);
        screen
            .list
            .narrow(vec![Narrow::Name(Some("nobody".to_string()))]);
        assert_eq!(painted(&screen, WALL)[3], "nothing matches a:nobody");

        // And the project axis is a list of places, which nobody arrives at
        // with nothing to arrange.
        let mut screen = showing(Vec::new(), None);
        screen.list.turn();
        assert_eq!(painted(&screen, WALL)[3], "no agents");
    }

    #[test]
    fn view_shows_the_fold_and_what_it_is_holding_back() {
        // A working agent and five finished. On a tall screen every row is
        // drawn and there is no fold at all; on a short one the finished
        // group takes the rows the live group left, and the fold stands on
        // the band's last row saying exactly what did not fit.
        let fleet = || {
            let mut views = vec![view("busy-b2c", Phase::Working, Some("Running Bash"), 3)];
            views.extend(
                (0..5).map(|n| view(&format!("done-{n}"), Phase::Done, Some("did it"), 60)),
            );
            views
        };

        let tall = settled(fleet(), (40, 24));
        assert_eq!(tall.iter().filter(|l| l.contains("done-")).count(), 5);
        assert!(!tall.iter().any(|l| l.contains("more")), "{tall:?}");

        let short = settled(fleet(), (40, 10));
        assert_eq!(heading_of(&short[5]), "COMPLETED");
        assert_eq!(short.iter().filter(|l| l.contains("done-")).count(), 2);
        assert!(
            short[8].contains("… 3 more"),
            "the fold stands on the last row the band has: {short:?}"
        );
    }

    #[test]
    fn card_shows_the_question_alone_and_none_of_the_pane_it_is_asked_on() {
        // The pane under a question is the vendor's drawing of the same box
        // the card already says in rows of its own, behind an echo of the
        // prompt: everything on it is noise below the answer line.
        let screen = drawn(
            vec![view("ask-a1b", Phase::Waiting, None, 30)],
            Some(Card {
                question: Some("Claude needs your permission to use Bash".to_string()),
                body: "$ rm -rf build\nDo you want to proceed?\n\n\n".to_string(),
                options: Vec::new(),
                kind: Some(Kind::Permission),
                ..asking(&[], None)
            }),
            (60, 12),
        );

        let all = screen.join("\n");
        assert!(all.contains("ask-a1b · waiting"), "{all}");
        assert!(all.contains("Claude needs your permission"), "{all}");
        assert!(
            !all.contains("Do you want to proceed?"),
            "the question block is the whole of the card: {all}"
        );
        assert_eq!(
            screen[11], "space closes it · enter attach · ctrl+x stop · ? keys",
            "the keys stay on the screen under the card, saying what they do \
             while it is up"
        );
        assert!(
            screen.iter().any(|line| line.contains("ask-a1b")),
            "and the list is still there above it: {all}"
        );

        let top = screen
            .iter()
            .position(|line| line.starts_with('╭'))
            .expect("the top of the card");
        let bottom = screen
            .iter()
            .rposition(|line| line.starts_with('╰'))
            .expect("the foot of the card");
        assert_eq!(
            bottom - top,
            2,
            "and the card is the question's own size, with no window kept \
             for a pane it will not draw: {screen:?}"
        );
    }

    /// The row the keys are drawn on, which is the last one on the screen.
    fn hint_row(screen: &Screen, size: (u16, u16)) -> String {
        painted(screen, size).pop().expect("a row for the keys")
    }

    /// A fleet with nothing left to finish, so there is a fold to walk onto.
    fn all_done() -> Vec<View> {
        (0..5)
            .map(|n| view(&format!("done-{n}"), Phase::Done, Some("did it"), 60))
            .collect()
    }

    #[test]
    fn keymap_hints_are_the_keys_the_line_under_the_cursor_answers_to() {
        let wide = (80, 12);
        let mut screen = showing(a_fleet(), None);

        // The view opens on an agent's row, where those keys reach the agent.
        assert_eq!(
            hint_row(&screen, wide),
            "space card · enter attach · ctrl+x stop · ctrl+s axis · q quit · ? keys"
        );

        // One line up is the heading over it, where the same two keys do
        // something else entirely.
        screen.list.up();
        assert_eq!(
            hint_row(&screen, wide),
            "enter shuts it · ctrl+x clears the group · ctrl+s axis · q quit · ? keys"
        );

        // And a group somebody has shut is opened by the key that shut it.
        screen.list.shut_or_open();
        assert!(
            hint_row(&screen, wide).starts_with("enter opens it"),
            "{:?}",
            hint_row(&screen, wide)
        );
    }

    #[test]
    fn keymap_hints_offer_nothing_the_line_under_the_cursor_cannot_do() {
        let wide = (80, 12);

        // A card is put away by the key that opened it.
        let mut screen = showing(a_fleet(), None);
        screen.card = Some(asking(&[], None).read());
        assert!(
            hint_row(&screen, wide).starts_with("space closes it · enter attach"),
            "{:?}",
            hint_row(&screen, wide)
        );

        // An agent whose command has ended has no window to bring forward and
        // nothing left to stop.
        let mut screen = showing(all_done(), None);
        screen.list.fit(5);
        screen.list.refit();
        let row = hint_row(&screen, wide);
        assert!(row.starts_with("space card · ctrl+x forget"), "{row:?}");
        assert!(!row.contains("attach"), "{row:?}");

        // The fold is not an agent either: what enter does there is give back
        // the rows it is holding.
        for _ in 0..3 {
            screen.list.down();
        }
        assert!(
            hint_row(&screen, wide).starts_with("enter shows them"),
            "{:?}",
            hint_row(&screen, wide)
        );

        // And a wall with nothing on it has no line under the cursor at all.
        let screen = showing(Vec::new(), None);
        assert!(
            hint_row(&screen, wide).starts_with("n starts one"),
            "{:?}",
            hint_row(&screen, wide)
        );
    }

    #[test]
    fn keymap_hints_shed_from_the_far_end_and_never_shed_the_overlay() {
        let screen = showing(a_fleet(), None);
        for width in 12..=80 {
            let row = hint_row(&screen, (width, 12));
            assert!(
                row.chars().count() <= width as usize,
                "a hint cut in half is a key that reads as another one: {row:?}"
            );
            assert!(
                row.ends_with("? keys"),
                "the row that has all of them is the last thing to go: {row:?}"
            );
        }

        // What is shed is what is furthest from it, and what is kept is what
        // the line under the cursor answers to.
        assert_eq!(
            hint_row(&screen, (60, 12)),
            "space card · enter attach · ctrl+x stop · ? keys"
        );
    }

    #[test]
    fn view_says_what_it_could_not_do_where_the_keys_are() {
        let mut screen = showing(Vec::new(), None);
        screen.notice = Some(Notice::Advice(
            "fix-login-a1b has no pane any more".to_string(),
        ));

        let painted = painted(&screen, (60, 6));
        assert_eq!(painted[5], "fix-login-a1b has no pane any more");
    }

    #[test]
    fn glyphs_and_notices_tell_a_failure_from_advice() {
        // The first cell of the row the two of them share.
        let said = |notice| {
            let mut screen = showing(Vec::new(), None);
            screen.notice = Some(notice);
            let cell = cells(&screen, (60, 6))[(0, 5)].clone();
            (cell.fg, cell.modifier)
        };

        assert_eq!(
            said(Notice::Failed("could not stop fix-login-a1b".to_string())),
            (theme().failed, Modifier::empty())
        );
        assert_eq!(
            said(Notice::Advice(
                "fix-login-a1b is done; nothing is listening".to_string()
            )),
            (Color::Reset, Modifier::DIM),
            "a thing that did not happen is not a thing that went wrong"
        );
    }

    #[test]
    fn view_shows_what_an_agent_changed_from_the_top_of_the_patch() {
        let patch = (0..40)
            .map(|n| format!("+ line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let screen = drawn(
            vec![view("fix-login-a1b", Phase::Working, None, 3)],
            Some(Card {
                id: "fix-login-a1b".to_string(),
                phase: Phase::Working,
                age: 3,
                question: None,
                options: Vec::new(),
                kind: None,
                body: patch,
                changes: true,
                answer: false,
            }),
            (60, 14),
        );

        let all = screen.join("\n");
        assert!(all.contains("fix-login-a1b · what it has changed"), "{all}");
        assert!(
            all.contains("+ line 0"),
            "the first of it, not the last: {all}"
        );
        assert!(!all.contains("+ line 39"), "{all}");
    }

    /// The card over a patch of this many lines, which can be more than any
    /// card has rows for.
    fn a_long_patch(lines: usize) -> Card {
        Card {
            id: "fix-login-a1b".to_string(),
            phase: Phase::Working,
            age: 3,
            question: None,
            options: Vec::new(),
            kind: None,
            body: (0..lines)
                .map(|n| format!("+ line {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
            changes: true,
            answer: false,
        }
    }

    #[test]
    fn card_pages_a_patch_from_its_offset_and_says_how_far() {
        let screen = showing(
            vec![view("fix-login-a1b", Phase::Working, None, 3)],
            Some(a_long_patch(40)),
        );
        screen.scroll.away.set(20);

        let all = painted(&screen, (60, 14)).join("\n");
        assert!(all.contains("+ line 20"), "the page it was sent to: {all}");
        assert!(!all.contains("+ line 0"), "{all}");
        assert!(!all.contains("+ line 39"), "{all}");
        assert!(all.contains("↑ 20 more"), "how far from the top: {all}");
        assert_eq!(screen.scroll.away.get(), 20);
    }

    #[test]
    fn card_stops_a_page_at_the_end_of_the_patch() {
        let screen = showing(
            vec![view("fix-login-a1b", Phase::Working, None, 3)],
            Some(a_long_patch(40)),
        );
        screen.scroll.away.set(1000);

        let all = painted(&screen, (60, 14)).join("\n");
        assert!(all.contains("+ line 39"), "the last of it: {all}");
        assert_eq!(
            screen.scroll.away.get(),
            40 - screen.scroll.page.get(),
            "written back as the last page there is: {all}"
        );
    }

    #[test]
    fn card_holds_a_fitting_body_at_its_edge() {
        let screen = showing(
            vec![view("fix-login-a1b", Phase::Working, None, 3)],
            Some(a_long_patch(3)),
        );
        screen.scroll.away.set(5);

        let all = painted(&screen, (60, 14)).join("\n");
        assert!(all.contains("+ line 0"), "{all}");
        assert!(all.contains("+ line 2"), "{all}");
        assert!(!all.contains("more"), "nothing is hidden: {all}");
        assert_eq!(screen.scroll.away.get(), 0, "nothing to page over");
    }

    #[test]
    fn card_pages_a_recorded_answer_down_from_its_top() {
        let answered = || {
            showing(
                vec![view("fix-login-a1b", Phase::Done, None, 3)],
                Some(Card {
                    id: "fix-login-a1b".to_string(),
                    phase: Phase::Done,
                    age: 3,
                    question: None,
                    options: Vec::new(),
                    kind: None,
                    body: (0..40)
                        .map(|n| format!("said {n}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    changes: false,
                    answer: true,
                }),
            )
        };

        // An answer reads forward, so the card opens on its first words.
        let opened = painted(&answered(), (60, 14)).join("\n");
        assert!(opened.contains("said 0"), "{opened}");
        assert!(!opened.contains("said 39"), "{opened}");

        // And paged, it stands that many rows below the top.
        let screen = answered();
        screen.scroll.away.set(7);
        let all = painted(&screen, (60, 14)).join("\n");
        assert!(all.contains("said 7"), "seven rows below the top: {all}");
        assert!(
            !all.contains("said 0"),
            "the first words are behind it: {all}"
        );
        assert!(all.contains("↑ 7 more"), "how far from the top: {all}");
        assert_eq!(screen.scroll.away.get(), 7);
    }

    #[test]
    fn card_gives_a_long_answer_its_whole_allowance() {
        // Forty rows of answer on a twenty-row screen: the card grows to
        // everything the height allows rather than the few lines a capture
        // used to fill, and the rest is there to page onto.
        let long: String = (0..40).map(|n| format!("said {n}\n")).collect();
        let card = Card {
            phase: Phase::Done,
            question: None,
            options: Vec::new(),
            body: long,
            answer: true,
            ..asking(&[], None)
        };
        let screen = drawn(a_fleet(), Some(card), (60, 20));

        let top = screen
            .iter()
            .position(|line| line.starts_with('╭'))
            .expect("the top of the card");
        let bottom = screen
            .iter()
            .rposition(|line| line.starts_with('╰'))
            .expect("the foot of the card");
        assert_eq!(
            bottom - top,
            9,
            "half the screen, the card's cap: {screen:?}"
        );
        assert!(
            screen[top + 1].contains("said 0"),
            "opened at the answer's first words: {screen:?}"
        );
    }

    #[test]
    fn card_holding_a_question_never_leaves_its_edge() {
        let screen = showing(a_fleet(), Some(asking(&["1. Yes", "2. No"], None)));
        screen.scroll.away.set(5);

        let all = painted(&screen, (60, 14)).join("\n");
        assert!(!all.contains("more"), "{all}");
        assert_eq!(
            screen.scroll.away.get(),
            0,
            "a question block does not page"
        );
    }

    /// A capture with the vendor's paint on it, which is what costs something
    /// to read: the escapes are what the walk is for.
    const PAINTED: &str = "\u{1b}[1mwrote the parser\u{1b}[0m\n\u{1b}[32m+ done\u{1b}[0m";

    #[test]
    fn card_walks_its_body_when_it_is_built_and_never_again_on_a_frame() {
        let mut card = asking(&[], None);
        card.phase = Phase::Working;
        card.question = None;
        card.body = PAINTED.to_string();

        let walked = walks();
        let screen = showing(a_fleet(), Some(card));
        assert_eq!(
            walks(),
            walked + 1,
            "the body is walked out of its escapes where the card is built"
        );

        // A view redraws on every key, every tick and every mouse move. None
        // of them is a reason to read the same capture again.
        for _ in 0..3 {
            let drawn = painted(&screen, (60, 14)).join("\n");
            assert!(drawn.contains("wrote the parser"), "{drawn}");
        }
        assert_eq!(
            walks(),
            walked + 1,
            "and every frame after it draws from that walk"
        );
    }

    /// A screen with room for the composer to reach its cap and a list above
    /// it: ten rows is a third of thirty.
    const TALL: (u16, u16) = (60, 30);

    /// The view with somebody part way through typing this line.
    fn typing(text: &str) -> Screen {
        let mut screen = showing(Vec::new(), None);
        let mut composer = Composer::new(Asking::Task);
        composer.text = text.to_string();
        screen.mode = Mode::Typing(composer);
        screen
    }

    /// Where the terminal's own cursor was left, which is where the next
    /// character somebody types will land.
    fn caret(screen: &Screen, size: (u16, u16)) -> (u16, u16) {
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
        terminal.draw(|frame| draw(frame, screen)).unwrap();
        let at = terminal.get_cursor_position().unwrap();
        (at.x, at.y)
    }

    #[test]
    fn composer_an_empty_task_line_names_its_own_prefixes() {
        // Wide enough for the whole sentence; a narrow screen clips it with
        // the ellipsis every other row wears.
        let empty = painted(&typing(""), (110, 30));
        let hint = empty
            .iter()
            .find(|row| row.contains("m:model"))
            .expect("the empty line teaches its prefixes");
        assert!(
            hint.starts_with("task ▸ m:model"),
            "the hint is a placeholder on the line itself, not a row of its \
             own: {hint}"
        );
        for named in [
            "m:model",
            "p:permission",
            "w:on|off",
            "d:directory",
            "agent:command",
            "s:state",
            "a:name",
        ] {
            assert!(hint.contains(named), "{named} is not taught: {hint}");
        }
        assert_eq!(
            empty.iter().filter(|row| row.contains("m:model")).count(),
            1,
            "and only there: the band under the composer is gone"
        );

        let narrow = painted(&typing(""), TALL);
        let clipped = narrow
            .iter()
            .find(|row| row.contains("m:model"))
            .expect("a narrow screen still teaches what fits");
        assert!(clipped.starts_with("task ▸ m:model"), "{clipped}");
        assert!(clipped.trim_end().ends_with('…'), "{clipped}");

        // The next keystroke lands where the prompt ends, over the
        // placeholder, the way a browser draws a field's ghost text.
        assert_eq!(caret(&typing(""), TALL), (7, 27));

        // The first character typed takes the placeholder away: whoever is
        // typing has stopped reading it.
        let typed = painted(&typing("p"), TALL);
        assert!(
            !typed.iter().any(|row| row.contains("m:model")),
            "{typed:?}"
        );

        // A reply goes to an agent already running, where a dial means
        // nothing, so the line would be teaching keys it does not read.
        let mut replying = showing(Vec::new(), None);
        replying.mode = Mode::Typing(Composer::new(Asking::Reply {
            id: "fix-a1b".to_string(),
            question: false,
        }));
        let reply = painted(&replying, TALL);
        assert!(
            !reply.iter().any(|row| row.contains("m:model")),
            "{reply:?}"
        );
    }

    #[test]
    fn composer_wraps_what_will_not_fit_and_starts_a_row_at_every_newline() {
        assert_eq!(composer_lines("abcdef", 3), ["abc", "def"]);
        assert_eq!(
            composer_lines("port the importer\nand its tests", 40),
            ["port the importer", "and its tests"]
        );
        assert_eq!(
            composer_lines("a\n\nb", 8),
            ["a", "", "b"],
            "a paragraph with nothing in it is a row, because the cursor sits \
             on it"
        );
        assert_eq!(composer_lines("", 8), [""]);
    }

    #[test]
    fn composer_grows_a_row_at_a_time_as_the_line_it_holds_does() {
        let one = painted(&typing("port the importer"), TALL);
        assert_eq!(one[27], "task ▸ port the importer");
        assert_eq!(one[26], "", "one line takes one row, at the foot of it all");

        let three = painted(
            &typing("port the importer\nand its tests\nand the docs"),
            TALL,
        );
        assert_eq!(three[25], "task ▸ port the importer");
        assert_eq!(
            three[26], "       and its tests",
            "a row under the first is indented to it, so a task reads as one \
             thing"
        );
        assert_eq!(three[27], "       and the docs");
        assert_eq!(
            caret(&typing("port it\nand test it"), TALL),
            (18, 27),
            "and the cursor is at the end of the last of them"
        );
    }

    #[test]
    fn composer_wrapping_past_the_width_grows_it_the_same_way_a_newline_does() {
        // Twice the room a sixty-column screen leaves beside the prompt.
        let painted = painted(&typing(&"x".repeat(106)), TALL);
        assert_eq!(painted[26], format!("task ▸ {}", "x".repeat(53)));
        assert_eq!(painted[27], format!("       {}", "x".repeat(53)));
    }

    /// A line long enough to need more rows than any screen will give it.
    fn twenty_rows() -> String {
        (1..=20)
            .map(|n| format!("row-{n:02}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn composer_stops_growing_at_its_cap_and_scrolls_the_line_inside_it() {
        let screen = typing(&twenty_rows());
        let painted = painted(&screen, TALL);

        assert_eq!(
            painted[18], "task ▸ row-11",
            "the prompt is on the top row however far the rest has scrolled: \
             {painted:?}"
        );
        assert_eq!(painted[27], "       row-20", "{painted:?}");
        assert!(
            !painted.iter().any(|line| line.contains("row-10")),
            "and what scrolled past is off the screen: {painted:?}"
        );
        assert_eq!(caret(&screen, TALL), (13, 27));
    }

    #[test]
    fn composer_leaves_the_list_it_was_opened_from_on_the_screen() {
        // A third of eight rows is two, whatever the line is holding, and the
        // agents are what the view is for.
        let painted = painted(&typing(&twenty_rows()), (60, 8));
        assert_eq!(painted[4], "task ▸ row-19");
        assert_eq!(painted[5], "       row-20");
        assert_eq!(
            painted[1], WELCOME,
            "the list is still there above it: {painted:?}"
        );
    }

    #[test]
    fn view_shows_the_line_being_typed_and_what_entering_it_will_do() {
        let mut screen = showing(Vec::new(), None);
        let mut composer = Composer::new(Asking::Task);
        composer.text = "port the importer".to_string();
        screen.mode = Mode::Typing(composer);

        let painted = painted(&screen, (60, 6));
        assert_eq!(painted[3], "task ▸ port the importer");
        assert!(painted[5].contains("enter starts it"), "{:?}", painted[5]);
        assert!(painted[5].contains("alt+enter newline"), "{:?}", painted[5]);
    }

    #[test]
    fn header_says_what_the_next_agent_may_do_without_asking() {
        let mut screen = launching(Vec::new());
        screen.mode = Mode::Typing(Composer::new(Asking::Task));

        let drawn = painted(&screen, (60, 8));
        assert!(
            drawn[5].starts_with("task ▸ m:model"),
            "the empty line carries its placeholder above the dial: {:?}",
            drawn[5]
        );
        assert_eq!(
            drawn[6], "permission: vendor default (shift+tab to cycle)",
            "the layer, not a guess at which mode claude would have picked"
        );
        assert!(drawn[7].contains("enter starts it"), "{:?}", drawn[7]);

        screen.profile.permission = "acceptEdits".to_string();
        assert_eq!(
            painted(&screen, (60, 8))[6],
            "⏵⏵ acceptEdits (shift+tab to cycle)",
            "and a mode in the vendor's own word for it"
        );
    }

    #[test]
    fn header_keeps_the_permission_row_to_the_lines_that_start_an_agent() {
        let row = |screen: &Screen| {
            painted(screen, (60, 8))
                .iter()
                .any(|line| line.contains("shift+tab"))
        };

        // A reply goes to an agent that is already running under whatever it
        // was started with, so the dial has nothing to say about it.
        let mut screen = launching(Vec::new());
        screen.mode = Mode::Typing(Composer::new(Asking::Reply {
            id: "ask-a1b".to_string(),
            question: true,
        }));
        assert!(!row(&screen), "a reply is not a spawn");

        // Nor has it anything to say about a line that narrows the list.
        let mut composer = Composer::new(Asking::Task);
        composer.text = "s:waiting".to_string();
        screen.mode = Mode::Typing(composer);
        assert!(!row(&screen));

        // A vendor amx has no entry for declares no permission dial: there is
        // nothing to say and nothing to turn, so the row is absent rather than
        // empty.
        screen.mode = Mode::Typing(Composer::new(Asking::Task));
        screen.profile.agent = "mock-claude".to_string();
        assert!(!row(&screen));

        // And nothing is being typed at all, which is most of the time.
        let screen = launching(Vec::new());
        assert!(!row(&screen));
    }

    #[test]
    fn header_leaves_the_list_a_row_with_every_other_band_open() {
        // Four bands of chrome at once: the header, a closer look, a line
        // being typed and the row under it. The list is what the view is for,
        // so the closer look gives way rather than the rows it was opened
        // from.
        let mut screen = launching(vec![view("ask-a1b", Phase::Waiting, None, 30)]);
        screen.card = Some(asking(&["the sqlite one"], Some(Kind::Question)).read());
        screen.mode = Mode::Typing(Composer::new(Asking::Task));

        let painted = painted(&screen, (60, 10));
        assert!(
            painted.iter().any(|line| line.contains("ask-a1b")),
            "{painted:?}"
        );
        assert!(
            painted.iter().any(|line| line.contains("shift+tab")),
            "{painted:?}"
        );
        assert!(painted[9].contains("enter starts it"), "{:?}", painted[9]);
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

    /// The overlay on a screen this size, and the rows it was drawn on.
    fn overlay(size: (u16, u16)) -> Vec<String> {
        let mut screen = showing(Vec::new(), None);
        screen.mode = Mode::Keys;
        painted(&screen, size)
    }

    #[test]
    fn keymap_stands_the_keys_under_headings_that_say_what_they_are_for() {
        // A screen with room for the groups in two columns, which is the
        // shape they are laid out in wherever the width will take it.
        let painted = overlay((140, 38));

        // Down before across: the second key is under the first rather than
        // beside it, and the second column starts where the first one's share
        // of the width ends.
        assert!(painted[3].starts_with("walk"), "{:?}", painted[3]);
        assert!(painted[4].starts_with(HELP[0].0), "{:?}", painted[4]);
        assert_eq!(
            column_of(&painted[3], "arrange"),
            70,
            "and the next column stands beside the first: {:?}",
            painted[3]
        );

        // A heading over every run of keys, a blank row between two groups,
        // and the groups themselves whole rather than split down the fold.
        assert!(
            painted[9].chars().take(70).all(char::is_whitespace),
            "one group stands off from the next: {:?}",
            painted[9]
        );
        assert!(painted[10].starts_with("look"), "{:?}", painted[10]);
        assert!(painted[17].starts_with("start"), "{:?}", painted[17]);
        assert_eq!(column_of(&painted[12], "dials"), 70, "{:?}", painted[12]);

        let all = painted.join("\n");
        for (key, does) in HELP {
            assert!(key.len() < 12, "{key} is wider than a band's key column");
            assert!(all.contains(key), "{key} is missing:\n{all}");
            assert!(all.contains(does), "{does} is missing:\n{all}");
        }
    }

    #[test]
    fn keymap_headings_are_the_quietest_thing_on_the_screen_of_keys() {
        let mut screen = showing(Vec::new(), None);
        screen.mode = Mode::Keys;
        let buffer = cells(&screen, (140, 38));

        let heading = buffer[(0, 3)].clone();
        assert!(
            heading.modifier.contains(Modifier::DIM),
            "a heading stands over the keys and is not one of them: {:?}",
            heading.modifier
        );
        let key = buffer[(0, 4)].clone();
        assert!(
            key.modifier.contains(Modifier::BOLD),
            "the key itself is what somebody came here to find: {:?}",
            key.modifier
        );
    }

    #[test]
    fn keymap_takes_another_column_when_the_rows_will_not_hold_a_group() {
        // Two rows of header, one of space and one of keys leave eleven for
        // the overlay, which is fewer rows than two columns of groups need:
        // rather than cut a group in half or run one off the bottom, the
        // groups deal into as many columns as the height asks for.
        let painted = overlay((140, 15));
        let all = painted.join("\n");
        for (key, _) in HELP {
            assert!(all.contains(key), "{key} is missing:\n{all}");
        }
        assert!(painted[3].starts_with("walk"), "{:?}", painted[3]);
        assert_eq!(
            column_of(&painted[3], "dials"),
            4 * (140 / GROUPS.len() as u16),
            "a column each, in the order the table has them: {:?}",
            painted[3]
        );
    }

    #[test]
    fn keymap_the_keys_give_up_what_they_say_before_they_give_up_a_key() {
        // The same screen with no room for two whole bands. Every key is
        // still on it, because a key nobody can find is worse than one whose
        // line was cut short.
        let painted = overlay((60, 15));
        let all = painted.join("\n");
        for (key, _) in HELP {
            assert!(all.contains(key), "{key} is missing:\n{all}");
        }
        for line in &painted {
            assert!(line.chars().count() <= 60, "{line:?}");
        }
        assert!(
            all.contains('…'),
            "and what was cut says it was cut:\n{all}"
        );
    }

    #[test]
    fn view_reads_the_bottom_of_a_screen_and_drops_what_is_blank() {
        let shown = |text: &'static str, wanted: usize, back: usize| {
            // The blank rows at the bottom are dropped where the body is
            // built, so what `tail` is handed is already the last row anybody
            // wrote on.
            let rows: Vec<&str> = text.lines().collect();
            let mut kept = rows.len();
            while kept > 0 && rows[kept - 1].trim().is_empty() {
                kept -= 1;
            }
            rows[tail(kept, wanted, back)].to_vec()
        };
        assert_eq!(shown("a\nb\nc\n\n\n", 2, 0), ["b", "c"]);
        assert_eq!(shown("a\nb", 5, 0), ["a", "b"]);
        assert!(shown("", 3, 0).is_empty());
        // Paged back, the window stands above the bottom it is read from.
        assert_eq!(shown("a\nb\nc\nd\n\n", 2, 1), ["b", "c"]);
        assert!(shown("a\nb", 2, 5).is_empty());
    }

    /// The five rows claude draws at the bottom of every pane it has the room
    /// for, in the vendor's own order: the composer's top border with its
    /// right-anchored label, whatever is staged in the box, the composer's
    /// bottom border, the statusline, and the mode footer. Transcribed from a
    /// live 2.1.237 at 100 columns on 2026-08-21.
    const CHROME: [&str; 5] = [
        "───────────────────────────── execute amx-v2 tail ─",
        "❯ ",
        "───────────────────────────────────────────────────",
        "  Opus 5 │ ◈ 0% │ amx-main (main) │ ◖ xhigh",
        "  ⏵⏵ accept edits on (shift+tab to cycle) · ← 3 agents",
    ];

    /// A row of the agent's own work, which is the one thing no step may take.
    const SAID: &str = "what the agent said";

    /// That screen with `typed` staged in the composer, under a row of work.
    fn staged(typed: &[&'static str]) -> Vec<&'static str> {
        let mut screen = vec![SAID, CHROME[0]];
        screen.extend_from_slice(typed);
        screen.extend_from_slice(&CHROME[2..]);
        screen
    }

    #[test]
    fn view_tail_cuts_the_chrome_claude_draws_under_every_pane() {
        let mut screen = vec![SAID, "", "✻ Nesting… (15s · thinking)", ""];
        screen.extend_from_slice(&CHROME);
        assert_eq!(
            cut(&screen),
            [SAID, ""].as_slice(),
            "the spinner goes with the box it sits over"
        );
    }

    #[test]
    fn view_tail_cuts_a_composer_whatever_is_staged_in_it() {
        // A composer with one row of text in it is the state that let a walk
        // cutting exactly one input row pass for a working rule, so neither
        // fixture here has one: a task wrapped over three rows, and a message
        // typed over four lines.
        let wrapped = staged(&[
            "❯ port the importer and then check every",
            "  call site that used to take the old",
            "  shape",
        ]);
        assert_eq!(cut(&wrapped), [SAID].as_slice());

        let lines = staged(&["❯ first", "  second", "  third", "  fourth"]);
        assert_eq!(cut(&lines), [SAID].as_slice());
    }

    #[test]
    fn view_tail_leaves_a_screen_the_vendor_drew_no_footer_under_alone() {
        // A permission prompt, which ends at its own confirm row: cutting
        // upward from there would take the question the card was opened for.
        let prompt = [
            "───────────────────────────────────",
            " Bash command",
            "   rm -rf build",
            " Do you want to proceed?",
            " ❯ 1. Yes",
            "   2. No",
            " Esc to cancel · Tab to amend",
        ];
        assert_eq!(cut(&prompt), prompt.as_slice());

        // And a pane too short for the vendor to draw its chrome in, whose
        // last row is the composer's own bottom border.
        let short = [SAID, CHROME[0], CHROME[1], CHROME[2]];
        assert_eq!(cut(&short), short.as_slice());
    }

    #[test]
    fn view_tail_gives_back_by_position_what_it_cannot_place() {
        // Three rows between the footer and the nearest rule: not the shape
        // this was measured against, so the statusline step abandons and only
        // the footer — matched by its own opener — stays cut.
        let odd = [SAID, CHROME[2], "one", "two", "three", CHROME[4]];
        assert_eq!(cut(&odd), &odd[..odd.len() - 1]);

        // A composer whose staged text is taller than half the capture: the
        // scan runs past its cap without meeting a top border, so it gives
        // back every row it took and the box survives on screen.
        let mut runaway = vec![SAID];
        runaway.extend((0..8).map(|_| "  typed"));
        runaway.extend_from_slice(&CHROME[2..]);
        assert_eq!(
            cut(&runaway),
            &runaway[..runaway.len() - 3],
            "the footer, the statusline and the bottom border keep their anchors"
        );
    }

    /// `capture-pane -p -J` of a live claude 2.1.237 at 72 columns on
    /// 2026-08-21, with a task typed into the composer and wrapped over three
    /// rows. Verbatim, trailing spaces and the no-break space after the
    /// chevron included: the rows above are transcriptions, and what a
    /// transcription cannot carry is exactly what these predicates walk over.
    const CAPTURED: [&str; 9] = [
        "what the agent said",
        "  tmux detected · scroll with PgUp/PgDn · or add 'set -g mouse on' to…",
        "────────────────────────────────────────────────── execute amx-v2 tail ─",
        "❯\u{a0}check every call site 1 check every call site 2 check every call      ",
        "  site 3 check every call site 4 check every call site 5 check every    ",
        "  call site 6                                             ",
        "────────────────────────────────────────────────────────────────────────",
        "  Opus 5 (1M context) (1M context) │ ◈ 0% │ amx-main (main) │ ◖ xhigh",
        "  ⏵⏵ accept edits on (shift+tab to cycle)               ",
    ];

    #[test]
    fn view_tail_cuts_what_a_live_vendor_actually_drew() {
        // The pane's own padding under the last row the vendor drew on.
        let mut screen = CAPTURED.to_vec();
        screen.push("");

        // The warning claude renders flush against the composer's top border
        // with no blank row between them stays: it is above the box, and a
        // walk that ran upward until a blank row would have eaten it.
        assert_eq!(cut(&screen), &CAPTURED[..2]);
    }

    #[test]
    fn view_tail_keeps_the_capture_the_card_has_no_question_to_draw() {
        let asked = |question: Option<&str>| {
            let mut card = asking(&[], Some(Kind::Question));
            card.body = format!("{SAID}\n\nWhich features should be enabled?\n");
            card.question = question.map(str::to_string);
            card
        };

        // The one asking card that still shows its pane: amx missed the call
        // that drew the menu, so the pane is the only place the question is
        // written at all.
        let kept = said(asked(None), 24);
        assert!(
            kept.contains(&"Which features should be enabled?".to_string()),
            "{kept:?}"
        );

        // And with the question on it, the card is the question block alone:
        // the pane under it is the same box behind an echo of the prompt.
        let block = said(asked(Some("Which features should be enabled?")), 24);
        assert!(block.is_empty(), "{block:?}");
    }

    /// What a card's body says, with the paint it says it in set aside.
    fn said(card: Card, rows: usize) -> Vec<String> {
        body(&card.read(), rows, 0)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    /// Every standing there is, so a table over them cannot quietly miss one.
    const EVERY_STANDING: [Standing; 8] = [
        Standing::Merged,
        Standing::Closed,
        Standing::Draft,
        Standing::Failing,
        Standing::Changes,
        Standing::Running,
        Standing::Ready,
        Standing::Open,
    ];

    /// The colour a word on a row was painted in.
    fn word_colour(screen: &Screen, size: (u16, u16), row: u16, word: &str) -> Color {
        let buffer = cells(screen, size);
        let line: String = (0..size.0)
            .map(|column| buffer[(column, row)].symbol())
            .collect();
        let at = line
            .find(word)
            .unwrap_or_else(|| panic!("{word:?} is not on {line:?}"));
        buffer[(line[..at].chars().count() as u16, row)].fg
    }

    /// And the weight it was painted at, for the tests about the muted rows.
    fn word_modifier(screen: &Screen, size: (u16, u16), row: u16, word: &str) -> Modifier {
        let buffer = cells(screen, size);
        let line: String = (0..size.0)
            .map(|column| buffer[(column, row)].symbol())
            .collect();
        let at = line
            .find(word)
            .unwrap_or_else(|| panic!("{word:?} is not on {line:?}"));
        buffer[(line[..at].chars().count() as u16, row)].modifier
    }

    #[test]
    fn rows_keep_the_name_bright_and_dim_what_the_agent_said() {
        let size = (60, 10);
        let screen = showing(
            vec![
                view("fix-login-a1b", Phase::Done, Some("wrote the parser"), 60),
                view("port-import-b2c", Phase::Done, Some("wrote the tests"), 300),
            ],
            None,
        );

        // The cursor opens on the first agent, and the two rows read the same:
        // the name in the terminal's own, what the agent said and how long it
        // worked dim beside it. Which line the cursor is on is the bar's to
        // say, and a row does not change its tones to say it again.
        for (row, name, said, age) in [
            (3, "fix-login-a1b", "wrote the parser", "1m"),
            (4, "port-import-b2c", "wrote the tests", "5m"),
        ] {
            let named = word_modifier(&screen, size, row, name);
            assert!(
                !named.contains(Modifier::DIM) && !named.contains(Modifier::BOLD),
                "{name} is neither dimmed nor weighted: {named:?}"
            );
            for word in [said, age] {
                assert!(
                    word_modifier(&screen, size, row, word).contains(Modifier::DIM),
                    "{word} is the quiet half of the row"
                );
            }
        }

        // The state is carried by the glyph's colour alone.
        let (glyph, painted, _) = mark(&screen, size, 4);
        assert_eq!((glyph.as_str(), painted), ("●", theme().done));
    }

    #[test]
    fn rows_hovered_name_takes_the_weight_and_nothing_else_does() {
        let size = (60, 10);
        let mut screen = showing(
            vec![
                view("fix-login-a1b", Phase::Done, Some("wrote the parser"), 60),
                view("port-import-b2c", Phase::Done, Some("wrote the tests"), 300),
            ],
            None,
        );
        // The pointer resting on the second agent's line, which is the third
        // item under the heading.
        screen.hover = Some(2);

        let hovered = word_modifier(&screen, size, 4, "port-import-b2c");
        assert!(hovered.contains(Modifier::BOLD), "{hovered:?}");
        assert!(!hovered.contains(Modifier::DIM), "{hovered:?}");
        assert!(
            word_modifier(&screen, size, 4, "wrote the tests").contains(Modifier::DIM),
            "the tint is the name's alone: what the agent said stays quiet"
        );
        assert_eq!(
            behind(&screen, size, 4),
            vec![Color::Reset; 60],
            "and a hover is not the bar"
        );
    }

    #[test]
    fn rows_on_the_project_axis_keep_the_phase_colour_on_the_state_word() {
        // The state word replaces the icon's job under a project heading, so
        // it keeps the phase colour while the words beside it stay muted.
        let size = (60, 10);
        let screen = by_project(vec![
            at(
                view("busy-c3d", Phase::Working, Some("Running Bash"), 3),
                "/src/api",
            ),
            at(
                view("fix-login-a1b", Phase::Done, Some("fixed it"), 60),
                "/src/api",
            ),
        ]);

        assert_eq!(word_colour(&screen, size, 4, "done"), theme().done);
        assert!(word_modifier(&screen, size, 4, "fixed it").contains(Modifier::DIM));
    }

    #[test]
    fn pr_the_row_says_what_the_branchs_request_is_doing() {
        let screen = over_the_forge(
            vec![
                on_a_branch(view("ask-a1b", Phase::Waiting, None, 30), "amx/ask-a1b"),
                on_a_branch(
                    view("busy-b2c", Phase::Working, Some("Running Bash"), 3),
                    "amx/busy-b2c",
                ),
            ],
            None,
        );
        let size = (60, 10);
        let lines = painted(&screen, size);
        let row = |word: &str| {
            lines
                .iter()
                .position(|line| line.contains(word))
                .unwrap_or_else(|| panic!("no row says {word:?}: {lines:?}"))
        };

        let asking = row("ask-a1b");
        assert!(lines[asking].contains("#12"), "{:?}", lines[asking]);
        assert_eq!(
            word_colour(&screen, size, asking as u16, "#12"),
            theme().failed,
            "a failing check is a thing that was attempted and failed"
        );

        // One column, so the numbers read down the screen rather than
        // wandering with the length of the name beside them.
        let busy = row("busy-b2c");
        let column = |line: &str, word: &str| {
            let at = line.find(word).expect("the number on the row");
            line[..at].chars().count()
        };
        assert_eq!(
            column(&lines[asking], "#12"),
            column(&lines[busy], "#40"),
            "{lines:?}"
        );
        assert!(
            lines[busy].contains("Running Bash"),
            "and what the agent is doing is still on it: {:?}",
            lines[busy]
        );
        assert!(
            !lines[busy].contains("#7"),
            "the row is read for the attempt that is still going, and the \
             one before it is on the card: {:?}",
            lines[busy]
        );
    }

    #[test]
    fn pr_costs_the_list_nothing_where_no_branch_has_one() {
        // Which is every list on a machine with no forge on it, and the whole
        // of what such a machine loses.
        let fleet = || {
            vec![
                view("ask-a1b", Phase::Waiting, None, 30),
                view("busy-b2c", Phase::Working, Some("Running Bash"), 3),
            ]
        };
        assert_eq!(
            painted(&over_the_forge(fleet(), None), (60, 10)),
            painted(&showing(fleet(), None), (60, 10)),
            "a fleet with no requests draws the rows amx always drew"
        );
    }

    #[test]
    fn pr_the_card_lists_every_request_the_branch_has() {
        let mut card = asking(&[], None);
        card.id = "busy-b2c".to_string();
        card.phase = Phase::Working;
        card.question = None;
        let screen = over_the_forge(
            vec![on_a_branch(
                view("busy-b2c", Phase::Working, Some("Running Bash"), 3),
                "amx/busy-b2c",
            )],
            Some(card),
        );
        let size = (60, 14);
        let lines = painted(&screen, size);

        let row = lines
            .iter()
            .position(|line| line.contains("#40 open"))
            .unwrap_or_else(|| panic!("nothing on the card lists them: {lines:?}"));
        assert!(
            lines[row].contains("#7 merged"),
            "every request the branch has, each with the question its colour \
             came from: {:?}",
            lines[row]
        );
        assert!(
            lines[..row].iter().any(|line| line.starts_with('╭')),
            "on the card rather than on the row behind it: {lines:?}"
        );
        assert_eq!(word_colour(&screen, size, row as u16, "#7"), theme().done);
    }

    #[test]
    fn pr_every_standing_has_a_word_and_a_colour() {
        // Eight standings and eight words, so a card never says one thing for
        // two of them. The colours are five and are meant to be shared: they
        // answer how it is going, and two standings can have the same answer.
        let said: Vec<&str> = EVERY_STANDING.into_iter().map(Standing::says).collect();
        assert_eq!(
            said.iter().collect::<std::collections::BTreeSet<_>>().len(),
            EVERY_STANDING.len(),
            "{said:?}"
        );
        for standing in EVERY_STANDING {
            assert_eq!(
                request_colour(theme(), standing).bg,
                None,
                "{standing:?} is a word on a row, not a bar under one"
            );
        }
        assert_eq!(
            request_colour(theme(), Standing::Merged).fg,
            Some(theme().done)
        );
        assert_eq!(
            request_colour(theme(), Standing::Failing).fg,
            Some(theme().failed)
        );
        assert_eq!(
            request_colour(theme(), Standing::Changes).fg,
            Some(theme().waiting)
        );
        assert_eq!(
            request_colour(theme(), Standing::Closed).fg,
            Some(theme().stopped)
        );
        assert_eq!(
            request_colour(theme(), Standing::Open).fg,
            None,
            "a request nobody has read yet has nothing to say about how it went"
        );
    }

    #[test]
    fn view_tail_says_so_when_a_capture_is_nothing_but_chrome() {
        let captured = |text: String| {
            let mut card = asking(&[], None);
            card.phase = Phase::Working;
            card.body = text;
            card
        };
        assert_eq!(said(captured(CHROME.join("\n")), 8), [ALL_CHROME]);

        // Which is not what an agent with nothing to say gets: no capture was
        // cut there, and "the pane held only furniture" is a different fact.
        assert!(said(captured(String::new()), 8).is_empty());
    }

    #[test]
    fn view_tail_is_cut_before_the_card_measures_what_it_has() {
        let mut card = asking(&[], None);
        card.phase = Phase::Working;
        card.question = None;
        let mut screen = vec!["what the agent said"];
        screen.extend_from_slice(&CHROME);
        card.body = screen.join("\n");

        // Two borders and the one row left under them, not the six rows the
        // capture has: a card that measured before it cut would spend its
        // height on the vendor's furniture.
        assert_eq!(card_rows(&card.read(), None, &[], false, 60), 3);
    }

    #[test]
    fn view_ages_are_the_readings_own_number_in_the_readings_own_words() {
        // Both the number and the units come from the reading, and the row
        // only asks for them. A row that worked the words out for itself would
        // agree with the table until the next hand touched one of the two, and
        // the person with both open is who finds out.
        for age in [0, 59, 60, 3_599, 3_600, 86_400] {
            let row = drawn(
                vec![view("busy-a1b", Phase::Working, None, age)],
                None,
                WALL,
            )
            .into_iter()
            .find(|line| line.contains("busy-a1b"))
            .expect("the agent's row");
            assert!(
                row.ends_with(&derive::in_words(age)),
                "{age} seconds is drawn as {row:?}"
            );
        }
    }

    #[test]
    fn view_rows_carry_the_worked_seconds_and_not_the_age() {
        // An idle agent's age climbs with every quiet second; what it worked
        // does not, and the column is about the work. The wait and the age
        // stay the card's.
        let mut idle = view("rests-a1b", Phase::Idle, Some("done for now"), 500);
        idle.verdict.worked = 60;
        let row = drawn(vec![idle], None, WALL)
            .into_iter()
            .find(|line| line.contains("rests-a1b"))
            .expect("the agent's row");
        assert!(row.ends_with("1m"), "{row:?}");
    }

    #[test]
    fn view_cuts_text_without_losing_the_last_character_to_the_ellipsis() {
        assert_eq!(fit("short", 10), "short");
        assert_eq!(fit("exactly", 7), "exactly");
        assert_eq!(fit("too long by far", 8), "too lon…");
        assert_eq!(fit("anything", 1), "…");
        assert_eq!(fit("anything", 0), "");
    }

    #[test]
    fn view_a_wide_glyph_in_the_summary_does_not_push_the_age_off_the_edge() {
        // Measured on the wall 2026-08-25: `Hello! 👋` — one char, two
        // columns — shifted everything after it right by one, and the row's
        // age lost its unit to the terminal's edge, reading `5` where every
        // other row read `5m`. A row is measured in columns, not characters.
        let row = drawn(
            vec![view(
                "waves-a1b",
                Phase::Done,
                Some("Hello! 👋 done and dusted"),
                345,
            )],
            None,
            WALL,
        )
        .into_iter()
        .find(|line| line.contains("waves-a1b"))
        .expect("the agent's row");
        assert!(
            row.trim_end().ends_with("5m"),
            "the unit survives the emoji: {row:?}"
        );

        // And the clip itself counts columns: four emoji are eight columns,
        // whole at eight and one emoji plus the ellipsis at four.
        assert_eq!(fit("👋👋👋👋", 8), "👋👋👋👋");
        assert_eq!(fit("👋👋👋👋", 4), "👋…");
        assert_eq!(fit("ab👋cd", 5), "ab👋…");
    }
}
