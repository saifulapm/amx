//! A closer look at one agent, as the card hung under its row.
//!
//! Not a band, and not a box. It is a spine: one column of `│` standing where
//! the row drew its own state glyph, closed with `╰`, and everything it says
//! written from the name column beside it. What the card belongs to is said by
//! where it stands, which costs no cells to say and cannot be mistaken for
//! another row's — a box says the same thing in four borders and takes two
//! rows and two columns of the wall to say it.
//!
//! How tall it is and where it stands are worked out here as well, because
//! both are answers about the list underneath: never so much of the screen
//! that the wall it was opened from is gone.
//!
//! A card carries its body in one of two states. It is *built* from text — a
//! pane capture, a recorded answer, a patch — and it is *drawn* from [`Body`],
//! that text already walked out of its escapes. Everything the paint takes is
//! the second: the walk happens once, where the card is made, and no frame
//! pays for it again.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use std::cell::Cell;
use std::ops::Range;

use super::input::composer_lines;
use super::style::{colour, dim, request_colour};
use super::text::{SEPARATOR, fit, inert, width_of};
use crate::ansi::{self, Colour, Painted};
use crate::furniture::cut;
use crate::pr::Pr;
use crate::store::{Kind, Phase};
use crate::theme::Theme;
use crate::tui::act::{self, Composer};
use crate::tui::rows::Showing;
use crate::verbs::send::numbered;

/// A closer look at one agent, as the card hung under its row.
///
/// A card carries its body in one of two states, which is what `B` says. A
/// card is *built* from text — a pane capture, a recorded answer, a patch —
/// and it is *drawn* from [`Body`], that text already walked out of its
/// escapes. Everything the paint takes is the second: the walk happens once,
/// where the card is made, and no frame pays for it again.
pub struct Card<B = String> {
    pub id: String,
    pub phase: Phase,
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
    pub(in crate::tui) fn none() -> Body {
        Body {
            rows: Vec::new(),
            kept: 0,
            chrome: false,
        }
    }

    /// A patch: amx's own reading of a repository rather than a pane, so there
    /// is no paint on it to keep and no furniture under it to cut.
    pub(in crate::tui) fn patch(text: &str) -> Body {
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
    pub(in crate::tui) fn screen(text: &str) -> Body {
        Body::walk(text, true)
    }

    /// What an agent said: a recorded answer, or whatever an agent whose
    /// command has ended left behind. Nothing is cut off it — there is no
    /// pane under it to hold furniture.
    pub(in crate::tui) fn said(text: &str) -> Body {
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
    pub(in crate::tui) fn says(&self) -> String {
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
    ///
    /// `window` is the rows the body has this frame, and `step` is what one
    /// press moves by, which is the rows a *paged* body has. The two part
    /// company on a card that only grows its marker row once it has left its
    /// edge: a step measured on the taller window would step over a row on
    /// the way out and never land back on the edge on the way home.
    fn kept(&self, length: usize, window: usize, step: usize) -> usize {
        let away = self.away.get().min(length.saturating_sub(window));
        self.away.set(away);
        self.page.set(step.max(1));
        away
    }
}

/// How much of the screen the card takes: what it has to show, up to about
/// half, and never so much that the list it was opened from is gone.
///
/// What it has to show comes into it because a card is over a wall somebody is
/// reading: an agent whose answer is one line does not need seven rows to say
/// it in, and every row the card does not take is a row of the list still on
/// the screen. Below one row there is no card at all.
pub(super) fn card_height(total: u16, band: u16, wanted: u16) -> u16 {
    let room = (total / 2)
        .clamp(CARD_SHORT, CARD_TALL)
        .min(wanted.max(CARD_SHORT))
        .min(band.saturating_sub(1));
    match room >= CARD_SHORT {
        true => room,
        false => 0,
    }
}

/// How many rows the card would take to say everything it has: the heading a
/// patch is labelled with, what its branch has open, which question of the
/// call this is, what the agent is asking, the choices under that, the row the
/// vendor adds under them, the line the answer goes on, and the screen it is
/// all happening on.
///
/// The row a paged body takes for its marker is not counted, and does not need
/// to be: a body that pages is a body too tall for the card, so the height
/// this asks for is already more than the screen will give.
pub(super) fn card_rows(
    card: &Card<Body>,
    showing: Option<Showing>,
    prs: &[Pr],
    answering: bool,
    width: u16,
) -> u16 {
    let inner = width.saturating_sub(NAME);
    let asked = card
        .question
        .as_deref()
        .map_or(0, |question| wrapped(question, inner).min(ASKED_TALL));
    let listed = choices(&card.options, inner as usize, boxed(showing)).len();
    // Counted no further than the card could ever grow: the body can be a
    // patch of thousands of rows, and this runs on every frame.
    let shown = length(card).min(CARD_TALL as usize);

    let rows = usize::from(card.changes)
        + usize::from(!prs.is_empty())
        + asked as usize
        + usize::from(tab(showing).is_some())
        + listed
        + usize::from(added(card, showing).is_some())
        + usize::from(answering)
        + shown;
    rows.min(u16::MAX as usize) as u16
}

/// The `height` rows under the line the card hangs off, which is where it
/// floats.
///
/// Under that line because the card is a thing said about it: the rows above
/// stay where they are and the rows below give up the room, so what the card
/// belongs to is the line it is touching rather than one somewhere up the wall.
pub(super) fn under(band: Rect, line: u16, height: u16) -> Rect {
    Rect {
        y: band.y + line + 1,
        height,
        ..band
    }
}

/// One row, which is the least a card is: with nothing repeated off the row it
/// hangs from, one row is a card that says something.
const CARD_SHORT: u16 = 1;

/// And the most of a screen it will take, however tall the terminal is.
const CARD_TALL: u16 = 14;

/// The column the spine stands in, which is the column the row it hangs from
/// drew its state glyph in.
const GLYPH: u16 = 2;

/// And the column everything the card says starts in, which is the column that
/// row's name starts in: the same four cells, so the card reads as a thing
/// said under one row rather than as a table of its own.
const NAME: u16 = 4;

/// The spine, and the corner that closes it on the card's last row.
const SPINE: &str = "│";
const FOOT: &str = "╰";

/// What a card holding a patch says it is, which is the one thing the row it
/// hangs off cannot: the row says what the agent is doing, and this card is
/// not a look at that at all.
const CHANGED: &str = "what it has changed";

/// How many rows of a wrapped question the card gives before it stops: the
/// words of it a person needs to decide, with the pane underneath for the rest.
const ASKED_TALL: u16 = 3;

/// The card: what its branch has open, which question of the call this is,
/// what one agent is asking, the choices it offers, the row the vendor adds
/// under them, the line the answer is typed on, and the screen it is all
/// happening on — or, when that is what was asked for, what it has changed.
///
/// Full width, because the bottom of it is a picture of a terminal and a
/// terminal cut down the middle is a picture of nothing. Every row of it
/// carries the spine, so there is no row of the card a reader has to work out
/// the owner of.
#[allow(clippy::too_many_arguments)]
pub(super) fn float(
    frame: &mut Frame,
    card: &Card<Body>,
    showing: Option<Showing>,
    prs: &[Pr],
    answering: Option<&Composer>,
    scroll: &Scroll,
    area: Rect,
    theme: Theme,
) {
    // Everything the card says stands in the name column, and the spine stands
    // in the column between.
    let said = Rect {
        x: area.x + NAME,
        width: area.width.saturating_sub(NAME),
        ..area
    }
    .intersection(area);

    // What the card is for comes first and the pane takes what is left. The
    // row being typed on comes before even the question: the question is on
    // the agent's own row behind the card, and a line somebody is typing into
    // is nowhere else at all.
    let mut room = said.height;
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
            .map_or(0, |question| wrapped(question, said.width).min(ASKED_TALL)),
    );
    // Which question of the call this is comes before the choices, because it
    // decides what the choices mean: the tab behind this one asks something
    // else and offers somebody else's answers.
    let strip = tab(showing);
    let tabbed = take(u16::from(strip.is_some()));
    let choices = choices(&options, said.width as usize, boxed(showing));
    let listed = take(choices.len() as u16);
    let added = added(card, showing);
    let adding = take(u16::from(added.is_some()));

    // And a row for the heading only where there is something to put on it:
    // that the card is a reading of a patch, or how far a paged body stands
    // from its edge. Measured against the window the body would have had with
    // no heading taken, which can only understate the distance — the row this
    // decides to take leaves a shorter window, and a shorter window stands
    // further from the edge rather than nearer.
    let length = length(card);
    let paged = scroll.away.get().min(length.saturating_sub(room as usize));
    let titled = u16::from(card.changes || paged > 0).min(room);

    // What is left is the body's window, which is what the offset is clamped
    // against. A press moves by the window a paged body has, heading row and
    // all, whether or not this frame is showing one.
    let held = scroll.kept(
        length,
        (room - titled) as usize,
        room.saturating_sub(1) as usize,
    );

    // Whatever the list drew here, the card is in front of it.
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(spine(card.phase, area.height, theme)),
        Rect {
            x: area.x + GLYPH,
            width: 1,
            ..area
        }
        .intersection(area),
    );

    let [
        heading_row,
        requesting,
        tabbing,
        asking,
        listing,
        adds,
        answer,
        screen,
    ] = Layout::vertical([
        Constraint::Length(titled),
        Constraint::Length(opened),
        Constraint::Length(tabbed),
        Constraint::Length(asked),
        Constraint::Length(listed),
        Constraint::Length(adding),
        Constraint::Length(typing),
        Constraint::Min(0),
    ])
    .areas(said);

    if titled > 0 {
        frame.render_widget(
            Paragraph::new(heading(card, held, said.width as usize, theme)),
            heading_row,
        );
    }
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

/// The spine: a column of `│` under the row's own state glyph, closed with
/// `╰` on the card's last row.
///
/// In the colour that glyph is painted in, because it is hanging off it. That
/// is the whole of what a card wears to say which row it belongs to: it is
/// under the row, in the row's own column, in the row's own colour, and none
/// of those cost it a cell of the wall.
fn spine(phase: Phase, height: u16, theme: Theme) -> Vec<Line<'static>> {
    (1..=height)
        .map(|row| {
            Line::styled(
                match row == height {
                    true => FOOT,
                    false => SPINE,
                },
                colour(theme, phase),
            )
        })
        .collect()
}

/// The card's own heading, which says only what the row it hangs off cannot.
///
/// Which agent this is and what it is doing are on that row, two cells above,
/// and a card that repeated them spent a row of the wall saying what the
/// reader was already looking at. What is left is the two facts the row has no
/// way to hold: that the card is a reading of what the agent changed rather
/// than of what it said, and how far a paged body stands from its natural
/// edge — dim, at the far end, because it is a fact about what the card is
/// showing rather than about the agent.
///
/// Which is why there is no heading at all on the ordinary card: the row said
/// it, the body has not moved, and the row goes to the body instead.
fn heading(card: &Card<Body>, held: usize, width: usize, theme: Theme) -> Line<'static> {
    let mut spans = Vec::new();
    let titled = match card.changes {
        true => {
            spans.push(Span::styled(CHANGED, colour(theme, card.phase)));
            width_of(CHANGED)
        }
        false => 0,
    };
    if held > 0 {
        let edge = match card.forward() {
            true => '↑',
            false => '↓',
        };
        let more = format!("{edge} {held} more");
        let said = titled + width_of(&more);
        if said < width {
            spans.push(Span::raw(" ".repeat(width - said)));
        }
        spans.push(Span::styled(more, dim()));
    }
    Line::from(spans)
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
pub(super) fn body(card: &Card<Body>, rows: usize, away: usize) -> Vec<Line<'static>> {
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
pub(in crate::tui) fn walks() -> usize {
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

/// What the card says where the walk finds nothing underneath the chrome.
pub(super) const ALL_CHROME: &str = "amx captured nothing but claude's own chrome";

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
pub(super) fn choices(options: &[String], width: usize, boxed: bool) -> Vec<String> {
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
pub(super) fn tail(end: usize, wanted: usize, back: usize) -> Range<usize> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::{Evidence, Verdict, View};
    use crate::pr::Standing;
    use crate::store::{Meta, State};
    use crate::tmux::{PaneId, Socket};
    use crate::tui::act::Asking;
    use crate::tui::paint::draw;
    use crate::tui::paint::input::{ANSWERS, spelled};
    use crate::tui::{Mode, Screen};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use std::path::PathBuf;

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
                agent: None,
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
            question: Some("Which fixture should the port keep?".to_string()),
            options: options.iter().map(|label| (*label).to_string()).collect(),
            kind,
            body: "$ cargo test\nDo you want to proceed?".to_string(),
            changes: false,
            answer: false,
        }
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

    /// What the view puts on a screen of this size, line by line.
    fn drawn(views: Vec<View>, card: Option<Card>, size: (u16, u16)) -> Vec<String> {
        painted(&showing(views, card), size)
    }

    /// The card as it stands on the screen, top to bottom: every row carrying
    /// the spine, which is every row a card has.
    fn card_lines(screen: &[String]) -> Vec<&str> {
        screen
            .iter()
            .filter(|line| line.starts_with("  │") || line.starts_with("  ╰"))
            .map(String::as_str)
            .collect()
    }

    /// What a heading line says in front of the rule that carries it out to
    /// the edge: the label, and how many failed under it where any did.
    fn heading_of(line: &str) -> &str {
        line.split('┈').next().unwrap_or_default().trim()
    }

    /// Where the terminal's own cursor was left, which is where the next
    /// character somebody types will land.
    fn caret(screen: &Screen, size: (u16, u16)) -> (u16, u16) {
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
        terminal.draw(|frame| draw(frame, screen)).unwrap();
        let at = terminal.get_cursor_position().unwrap();
        (at.x, at.y)
    }

    /// Which column of a drawn line a word starts in, counted in cells rather
    /// than bytes: the marks and the glyph a row wears are not one byte each.
    fn column_of(line: &str, word: &str) -> usize {
        let at = line
            .find(word)
            .unwrap_or_else(|| panic!("{word:?} is not on {line:?}"));
        line[..at].chars().count()
    }

    /// The colour a word on a row was painted in.
    fn word_colour(screen: &Screen, size: (u16, u16), row: u16, word: &str) -> Color {
        let buffer = cells(screen, size);
        let line: String = (0..size.0)
            .map(|column| buffer[(column, row)].symbol())
            .collect();
        buffer[(column_of(&line, word) as u16, row)].fg
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
    fn card_hangs_a_spine_off_the_row_it_was_opened_from() {
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

        let card = card_lines(&screen);
        let [asked, ..] = card.as_slice() else {
            panic!("no card in: {screen:?}")
        };
        assert!(
            asked.starts_with("  │ Which fixture should the port keep?"),
            "the card opens on what the agent is asking: {asked:?}"
        );
        assert!(
            !card
                .iter()
                .any(|line| line.contains("ask-a1b") || line.contains("waiting")),
            "which agent this is and what it is doing are on the row two \
             cells above, and the card does not spend a row repeating them: \
             {card:?}"
        );
        assert!(
            !screen.iter().any(|line| line.contains("Do you want to")),
            "and the pane it is asking on is not echoed under it: {screen:?}"
        );
        assert!(
            card.last().is_some_and(|line| line.starts_with("  ╰ ")),
            "closed on its last row: {card:?}"
        );
        assert!(
            !card.iter().any(|line| line.contains('┈')),
            "and drawn with no border cells at all: {card:?}"
        );

        let row = screen
            .iter()
            .position(|line| line.contains("ask-a1b"))
            .expect("the row the card was opened from");
        let top = screen
            .iter()
            .position(|line| line.starts_with("  │"))
            .expect("the top of the card");
        assert_eq!(
            top,
            row + 1,
            "and it starts on the line under that row rather than at the foot \
             of the list: {screen:?}"
        );
        let bottom = screen
            .iter()
            .rposition(|line| line.starts_with("  ╰"))
            .expect("the foot of the card");
        assert!(
            screen[bottom + 1..]
                .iter()
                .any(|line| line.contains("busy-b2c")),
            "with the rows that were under it moved down: {screen:?}"
        );
    }

    #[test]
    fn card_stands_in_the_columns_the_rows_behind_it_stand_in() {
        let screen = drawn(
            a_fleet(),
            Some(asking(
                &["the sqlite one", "the docker one"],
                Some(Kind::Question),
            )),
            (60, 14),
        );

        // Read off a row rather than written down here as a number: the card
        // hangs off the list, so the day the list moves its gutter the card
        // is standing in a column of its own and this says so.
        let row = screen
            .iter()
            .find(|line| line.contains("busy-b2c"))
            .expect("a row of the list the card is not over");
        let named = column_of(row, "busy-b2c");
        let glyph = named - 2;

        let card = card_lines(&screen);
        assert_eq!(
            column_of(card[0], "Which fixture"),
            named,
            "the card says what it has to say in the column the wall names \
             its rows in: {screen:?}"
        );
        for line in &card {
            assert!(
                matches!(line.chars().nth(glyph), Some('│' | '╰')),
                "and every row of it carries the spine in the column the wall \
                 draws a row's own state glyph in: {line:?}"
            );
        }
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
            empty[13],
            spelled(&ANSWERS),
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
            (20, 8),
            "with the terminal's own cursor at the end of what was typed, on \
             the answer row of a card that is the question block's own size"
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
        let card = card_lines(&screen);
        assert_eq!(card.len(), 1, "the one line it has: {screen:?}");
        assert!(
            card[0].starts_with("  ╰ did what it was asked"),
            "{screen:?}"
        );

        let top = screen
            .iter()
            .position(|line| line.starts_with("  ╰"))
            .expect("the top of the card");
        assert!(
            screen[top - 1].contains("ask-a1b"),
            "hung off its own row, with the rows it is not taking still on the \
             wall around it: {screen:?}"
        );
    }

    #[test]
    fn card_keeps_the_row_being_typed_on_when_there_is_room_for_little_else() {
        // A card with one row under its heading. What somebody is typing is
        // what that row is for: the question is on the agent's row behind the
        // card, and the line is nowhere else at all.
        let screen = painted(
            &answering(asking(&["the sqlite one"], Some(Kind::Question)), "the sq"),
            (60, 6),
        );
        assert!(answer_row(&screen).contains("❯ the sq"), "{screen:?}");
        assert_eq!(
            screen[5],
            spelled(&ANSWERS),
            "with the card's own keys under it"
        );
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
            "space closes it   enter attach   ctrl+x stop   ? keys"
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
        assert!(all.contains("Claude needs your permission"), "{all}");
        assert!(
            !all.contains("Do you want to proceed?"),
            "the question block is the whole of the card: {all}"
        );
        assert_eq!(
            screen[11], "space closes it   enter attach   ctrl+x stop   ? keys",
            "the keys stay on the screen under the card, saying what they do \
             while it is up"
        );
        assert!(
            screen.iter().any(|line| line.contains("ask-a1b")),
            "and the list is still there above it: {all}"
        );

        assert_eq!(
            card_lines(&screen).len(),
            1,
            "and the card is the question's own size, with no window kept \
             for a pane it will not draw: {screen:?}"
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
        assert!(
            all.contains("what it has changed"),
            "a card holding a patch says so, because the row it hangs off \
             says what the agent is doing and this is not that: {all}"
        );
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

        let card = card_lines(&screen);
        assert_eq!(
            card.len(),
            10,
            "half the screen, the card's cap: {screen:?}"
        );
        assert!(
            card[0].contains("said 0"),
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
            lines[row].starts_with("  │"),
            "on the card rather than on the row behind it: {lines:?}"
        );
        assert_eq!(word_colour(&screen, size, row as u16, "#7"), theme().done);
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

        // The one row left after the cut, not the six rows the capture has: a
        // card that measured before it cut would spend its height on the
        // vendor's furniture.
        assert_eq!(card_rows(&card.read(), None, &[], false, 60), 1);
    }
}
