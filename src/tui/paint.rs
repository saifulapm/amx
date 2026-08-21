//! Drawing the view.
//!
//! Five bands, top to bottom: what there is, the agents themselves, a closer
//! look at one of them when somebody asked for one, the line somebody is
//! typing when they are typing one, and the keys. Everything here is a
//! function of what it is handed, so what the screen says can be read back in
//! a test without a terminal anywhere near it.
//!
//! A row is one line, always: an agent's answer is a paragraph, and a
//! paragraph in a list is how a list stops being one.
//!
//! Two kinds of thing are on the screen at once and they are drawn apart:
//! what is happening — the rows, the counters — and what the *next* agent will
//! be started with, which has not happened at all. Everything of the second
//! kind wears one treatment of its own, so nobody reads a dial as a fact about
//! the fleet.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::sync::OnceLock;

use super::act::{Asking, Composer};
use super::rows::{Axis, Group, Item, List, Tally, Under};
use super::{Mode, Profile, Screen};
use crate::derive::View;
use crate::store::Phase;

/// The keys with nowhere else to be said, on the screen where somebody can see
/// them. The rest are one keypress away, which is what `?` is for.
const KEYS: &str = "space peek · enter attach · ctrl+s axis · ? keys · q quit";

/// Every key, for whoever asked what they are.
const HELP: [(&str, &str); 17] = [
    ("↑ ↓", "walk the agents"),
    ("space", "look closer at one"),
    ("enter", "bring its window forward · shut a group"),
    ("n", "start an agent · tab starts it out of sight"),
    ("r", "reply: a message, or the key a question wants"),
    ("d", "what it has changed"),
    ("ctrl+x", "stop it · again to forget it"),
    ("ctrl+s", "gather them by state or by project"),
    ("alt+enter", "a newline in the line, without sending it"),
    ("alt+m", "which model the next agent is given"),
    ("alt+w", "whether it gets a worktree of its own"),
    ("shift+tab", "what it may do without asking"),
    ("s: a:", "narrow by state or name, on the task line"),
    ("m: p: w:", "model, permission and worktree, for one spawn"),
    ("agent:", "which vendor runs it, for one spawn"),
    ("?", "these keys"),
    ("q", "close the view"),
];

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

/// A closer look at one agent.
pub struct Peek {
    pub id: String,
    pub phase: Phase,
    /// What it is waiting to be told, when it is waiting to be told anything.
    pub question: Option<String>,
    /// The screen it is sitting on, the answer it left behind, or what it has
    /// changed.
    pub body: String,
    /// Whether the body is that diff, which is read from the top down rather
    /// than from the bottom up.
    pub changes: bool,
}

/// Draw everything.
pub fn draw(frame: &mut Frame, screen: &Screen) {
    let area = frame.area();
    let helping = matches!(screen.mode, Mode::Keys);
    let head = header_rows(area.height);
    let typing = matches!(screen.mode, Mode::Typing(_));

    // Every band that is neither the list nor the closer look: the header, the
    // keys, and — while somebody is typing — the line and the row under it,
    // counted at the one row the line never goes below.
    let chrome = head + 1 + u16::from(typing);
    let panel = match (helping, &screen.peek) {
        (false, Some(_)) => peek_height(area.height, chrome),
        _ => 0,
    };
    let composing = match &screen.mode {
        Mode::Typing(composer) => composer_height(composer, area, head + 1 + panel),
        _ => 0,
    };

    let [top, middle, bottom, line, keys] = Layout::vertical([
        Constraint::Length(head),
        Constraint::Min(1),
        Constraint::Length(panel),
        Constraint::Length(composing),
        Constraint::Length(1),
    ])
    .areas(area);

    frame.render_widget(Paragraph::new(header(screen, top)), top);
    match &screen.mode {
        Mode::Keys => help(frame, middle),
        _ => agents(frame, &screen.list, middle, screen.beat),
    }
    if panel > 0
        && let Some(peek) = &screen.peek
    {
        look(frame, peek, bottom);
    }
    if let Mode::Typing(composer) = &screen.mode {
        composing_line(frame, composer, line);
    }
    frame.render_widget(Paragraph::new(footer(screen)), keys);
}

/// How much of the screen a peek takes: about half, and never so much that
/// the list it was opened from is gone — which is what `chrome` is counted
/// for, because a peek can be open while somebody is typing and the rows it
/// was opened from would be what paid for the line.
fn peek_height(total: u16, chrome: u16) -> u16 {
    let room = total.saturating_sub(chrome + 1);
    (total / 2).clamp(3, 14).min(room)
}

/// Below this the worktree dial says what it is without saying what it will
/// do: the dial is the thing somebody turns, the path is what it means.
const NARROW: usize = 90;

/// Below this many rows the header is the launch profile alone. Two rows of
/// chrome over a screen that short is a third of it, and the list is what the
/// view is for.
const SHORT: usize = 10;

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

/// What is above the list: what the next agent will be started with, and what
/// the fleet is doing now.
///
/// Two rows where there is room for two. The build's own version and the
/// worktree dial share the first, and the launch profile and the counters
/// share the second — one prospective half and one current half on each, which
/// is that law made visible. The row that goes on a short screen is the first:
/// which version this is says nothing about the fleet, and the dial is one
/// keypress from being read in the row under the composer.
fn header(screen: &Screen, area: Rect) -> Vec<Line<'static>> {
    let width = area.width as usize;
    let mut lines = Vec::new();
    if area.height >= 2 {
        lines.push(spread(
            vec![Span::styled(
                format!("amx v{}", env!("CARGO_PKG_VERSION")),
                dim(),
            )],
            vec![Span::styled(
                worktree_dial(screen.profile.worktree, width),
                prospective(),
            )],
            width,
        ));
    }

    // The fleet's half is worked out first: it is what there is, and the
    // profile is what fits beside it.
    let counters = counters(&screen.list, screen.profile.max);
    let room = width.saturating_sub(said(&counters) + 1);
    lines.push(spread(profile(&screen.profile, room), counters, width));
    lines
}

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

/// What the next agent will be started with: the vendor, the model it will be
/// given, and where it will run.
///
/// The model is named as it stands, `default` and all — amx does not know
/// which model claude would pick for itself and will not print a guess at it —
/// and a vendor whose entry declares no model dial has nothing to put in
/// parentheses, so none are drawn.
///
/// An `agent` is a command, and a command is routinely an absolute path. Left
/// to the terminal such a row is clipped, which reads as a name that ends
/// where the screen does; so the directory gives way first, and whatever is
/// still too long is cut with an ellipsis that says it was cut.
fn profile(profile: &Profile, room: usize) -> Vec<Span<'static>> {
    let agent = match profile.model_dial() {
        Some(_) => format!("{} ({})", profile.agent, profile.model),
        None => profile.agent.clone(),
    };
    let left = room.saturating_sub(agent.chars().count() + SEPARATOR.len());
    let said = match !profile.dir.is_empty() && left >= SHORTEST_DIR {
        true => format!("{agent}{SEPARATOR}{}", fit(&profile.dir, left)),
        false => fit(&agent, room),
    };
    vec![Span::styled(said, prospective())]
}

/// What stands between two things said on one row.
const SEPARATOR: &str = " · ";

/// What the fleet is: a count per group, in the word the list can be narrowed
/// by, and the gate the next agent will meet.
fn counters(list: &List, max: usize) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (group, count) in list.counts() {
        if !spans.is_empty() {
            spans.push(Span::styled(SEPARATOR, dim()));
        }
        spans.push(Span::styled(
            format!("{count} {}", group.state()),
            group_colour(group),
        ));
    }
    if !spans.is_empty() {
        spans.push(Span::styled(SEPARATOR, dim()));
    }
    // The limit that refuses a spawn, said before it refuses one.
    spans.push(Span::styled(
        format!("{}/{max} running", list.live()),
        dim(),
    ));

    // What the list was narrowed to, in the words it was narrowed with, so
    // somebody who has forgotten why it is short can read why.
    if let Some(narrowing) = list.narrowing() {
        spans.push(Span::styled(format!("{SEPARATOR}{narrowing}"), dim()));
    }
    spans
}

/// The worktree dial, and what it will do — named rather than implied. Under
/// [`NARROW`] the consequence goes and the dial stays.
fn worktree_dial(on: bool, width: usize) -> String {
    match (on, width < NARROW) {
        (true, true) => "worktree: on".to_string(),
        (false, true) => "worktree: off".to_string(),
        (true, false) => "worktree: on → .amx/worktrees/<id>".to_string(),
        (false, false) => "worktree: off → runs in the launch dir".to_string(),
    }
}

/// How wide each column is, worked out over the whole list so that every row
/// lines up with the ones above it.
#[derive(Clone, Copy)]
struct Columns {
    names: usize,
    /// The state a row carries, which is a column only where the heading does
    /// not say it. Zero is no column at all.
    status: usize,
}

/// How wide the list has to be for the empty state to say what lands in each
/// group. The longest of the four takes 64 columns at the gutter the rows are
/// indented by, so 72 is that with six columns to spare rather than a bound
/// worked out to the character: the copy is copy, and the next edit to it
/// should not quietly cross a cliff. A test measures the copy against this
/// number, so an edit that outgrows it says so rather than being cut.
const BLURBS_WIDE: usize = 72;

/// And how tall: four headings with a line each under them.
const BLURBS_TALL: usize = 2 * Group::ALL.len();

/// The agents themselves.
fn agents(frame: &mut Frame, list: &List, area: Rect, beat: usize) {
    if list.is_empty() {
        let room = area.width as usize >= BLURBS_WIDE && area.height as usize >= BLURBS_TALL;
        if list.unstarted() && room {
            frame.render_widget(Paragraph::new(blurbs()), area);
            return;
        }
        // Nothing to show is one thing while there are no agents, and another
        // while a narrowing is holding every one of them back.
        let said = match list.narrowing() {
            Some(narrowing) => format!("nothing matches {narrowing}"),
            None => "no agents".to_string(),
        };
        frame.render_widget(Paragraph::new(Line::styled(said, dim())), area);
        return;
    }

    let height = area.height as usize;
    // Enough of the top scrolled away to keep the cursor on the screen.
    let offset = list.cursor().saturating_sub(height.saturating_sub(1));
    let columns = columns(list);

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
                at == list.cursor(),
                columns,
                area.width as usize,
                beat,
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// One line of the list, whatever kind of line it is.
fn line(
    list: &List,
    item: Item,
    selected: bool,
    columns: Columns,
    width: usize,
    beat: usize,
) -> Line<'static> {
    let line = match item {
        Item::Heading(under, tally) => heading(list.title(under), under, tally),
        Item::Fold(hidden) => Line::styled(format!("{GUTTER}… {hidden} more"), dim()),
        Item::Agent(_) => match list.agent(item) {
            Some(view) => row(view, selected, columns, width, beat),
            None => Line::raw(""),
        },
    };
    match selected {
        true => barred(line, width),
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
/// The colour is the vendor's own for a selected line, measured from the
/// 2.1.237 bundle, for the reason the rest of them are.
fn barred(line: Line<'static>, width: usize) -> Line<'static> {
    let said: usize = line
        .spans
        .iter()
        .map(|span| span.content.chars().count())
        .sum();
    let mut line = line;
    if said < width {
        line.spans.push(Span::raw(" ".repeat(width - said)));
    }
    line.style(Style::new().bg(role::SELECTED))
}

/// Every group over a fleet nobody has started, with what lands in it under
/// each one.
///
/// The one screen where a heading with nobody under it is worth drawing: there
/// is nothing to read off the rows, so the groups say in a sentence what the
/// rest of the time they say by what is standing beneath them. A heading with
/// no line under it would be the fault this is for, so they come as pairs or
/// not at all — which is why the room for all eight is asked for before any of
/// them is drawn, and why they are never cut to fit.
fn blurbs() -> Vec<Line<'static>> {
    Group::ALL
        .into_iter()
        .flat_map(|group| {
            [
                Line::styled(group.title(), heading_colour(Under::Group(group))),
                Line::styled(format!("{GUTTER}{}", group.blurb()), dim()),
            ]
        })
        .collect()
}

/// A heading: what it stands for, and what it is answerable for.
///
/// A group that is open has its rows on the screen, so counting them there
/// would be a number beside the thing it counts. Shut, the count is the only
/// thing standing in for them. The failures are said either way — that is the
/// one number a heading is worth reading without opening it, because an agent
/// that failed is the reason somebody came to the screen.
fn heading(title: String, under: Under, tally: Tally) -> Line<'static> {
    let counted = match (tally.shut, tally.failures) {
        (false, 0) => String::new(),
        (false, failures) => format!(" · {failures} failed"),
        (true, 0) => format!(" {}", tally.members),
        (true, failures) => format!(" {} · {failures} failed", tally.members),
    };
    let mut spans = vec![Span::styled(title, heading_colour(under))];
    if !counted.is_empty() {
        spans.push(Span::styled(counted, dim()));
    }
    Line::from(spans)
}

/// An agent's row: what state it is in, what it is called, what it is up to,
/// and how long since anybody heard from it.
///
/// The state is on the row twice where it is on it at all — as the mark, and
/// as the word beside the name. The mark is worth reading at a glance across a
/// whole screen and the word is worth reading on one row, and under a project
/// heading nothing else says which state a row is in.
fn row(view: &View, selected: bool, columns: Columns, width: usize, beat: usize) -> Line<'static> {
    let Columns { names, status } = columns;
    let phase = view.phase();
    let age = age(view.verdict.age);
    let name = fit(view.id(), names);
    // The gutter, the icon and its space, the name and its gap, the status
    // column and its gap where there is one, the age and the space before it.
    let spent = GUTTER.len() + 2 + names + 2 + status + 2 * usize::from(status > 0) + AGE + 1;
    let room = width.saturating_sub(spent);
    let said = fit(first_line(view.line().unwrap_or("")), room);

    let mut spans = vec![
        Span::raw(GUTTER),
        Span::styled(format!("{} ", icon(phase, beat)), colour(phase)),
        Span::styled(
            format!("{name:<names$}  "),
            if selected {
                Style::new().add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            },
        ),
    ];
    if status > 0 {
        spans.push(Span::styled(
            format!("{:<status$}  ", fit(phase.as_str(), status)),
            colour(phase),
        ));
    }
    spans.push(Span::styled(format!("{said:<room$} "), dim()));
    spans.push(Span::styled(format!("{age:>AGE$}"), dim()));
    Line::from(spans)
}

/// A closer look at one agent: what it is asking, over the screen it is
/// asking it on — or, when that is what was asked for, what it has changed.
fn look(frame: &mut Frame, peek: &Peek, area: Rect) {
    let title = match peek.changes {
        true => format!(" {} · what it has changed ", peek.id),
        false => format!(" {} · {} ", peek.id, peek.phase.as_str()),
    };
    let block = Block::new().borders(Borders::TOP).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let asked = peek
        .question
        .as_deref()
        .map_or(0, |question| wrapped(question, inner.width).min(3));
    let [asking, screen] =
        Layout::vertical([Constraint::Length(asked), Constraint::Min(0)]).areas(inner);

    if let Some(question) = &peek.question {
        frame.render_widget(
            Paragraph::new(question.clone())
                .wrap(Wrap { trim: true })
                .style(Style::new().fg(role::WARNING)),
            asking,
        );
    }
    // A screen is read from the bottom, where the newest of it is; a diff is
    // read from the top, where the first file it touched is.
    let rows = screen.height as usize;
    let body = match peek.changes {
        true => peek.body.lines().take(rows).collect(),
        false => tail(&peek.body, rows),
    };
    let lines: Vec<Line> = body
        .into_iter()
        .map(|text| Line::styled(text.to_string(), dim()))
        .collect();
    frame.render_widget(Paragraph::new(lines), screen);
}

/// Every key and what it does.
fn help(frame: &mut Frame, area: Rect) {
    let lines: Vec<Line> = HELP
        .iter()
        .map(|(key, does)| {
            Line::from(vec![
                Span::styled(
                    format!("{key:<10}"),
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                Span::styled((*does).to_string(), dim()),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
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
fn composing_line(frame: &mut Frame, composer: &Composer, area: Rect) {
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
                0 => Span::styled(prompt.clone(), Style::new().fg(role::WARNING)),
                _ => Span::raw(indent.clone()),
            };
            Line::from(vec![head, Span::raw(text.clone())])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);

    let at = prompt.chars().count() + shown.last().map_or(0, |row| row.chars().count());
    frame.set_cursor_position((
        area.x + at.min(width.saturating_sub(1)) as u16,
        area.y + shown.len().saturating_sub(1) as u16,
    ));
}

/// The keys, or whatever the view has to say for itself instead.
fn footer(screen: &Screen) -> Line<'static> {
    if let Some(notice) = &screen.notice {
        return match notice {
            Notice::Failed(said) => Line::styled(said.clone(), Style::new().fg(role::ERROR)),
            Notice::Advice(said) => Line::styled(said.clone(), dim()),
        };
    }
    Line::styled(
        match &screen.mode {
            Mode::List => KEYS.to_string(),
            Mode::Keys => "any key goes back · q quits".to_string(),
            Mode::Typing(composer) if composer.narrows() => {
                "enter narrows it · s: or a: alone clears · esc cancels".to_string()
            }
            Mode::Typing(composer) => match composer.asking {
                Asking::Task => {
                    "enter starts it · alt+enter newline · tab out of sight · esc cancels"
                        .to_string()
                }
                Asking::Reply { .. } => {
                    "enter sends it · alt+enter newline · esc cancels".to_string()
                }
            },
        },
        dim(),
    )
}

/// How wide each column has to be. The names are capped, because one long name
/// must not push what every agent is doing off the side of the screen.
fn columns(list: &List) -> Columns {
    let shown = || list.items().iter().filter_map(|item| list.agent(*item));
    Columns {
        names: shown()
            .map(|view| view.id().chars().count())
            .max()
            .unwrap_or(0)
            .clamp(6, 24),
        // On the state axis the heading over the row already says the state,
        // and saying it twice would be a column of noise.
        status: match list.axis() {
            Axis::State => 0,
            Axis::Project => shown()
                .map(|view| view.phase().as_str().len())
                .max()
                .unwrap_or(0),
        },
    }
}

/// How many rows text takes when it is wrapped to a width.
fn wrapped(text: &str, width: u16) -> u16 {
    let width = width.max(1) as usize;
    let rows = text.chars().count().div_ceil(width);
    rows.clamp(1, u16::MAX as usize) as u16
}

/// The last rows of a screen, with the blank ones at the bottom dropped: a
/// pane is as tall as its window and its content rarely is.
fn tail(text: &str, rows: usize) -> Vec<&str> {
    let mut lines: Vec<&str> = text.lines().collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let from = lines.len().saturating_sub(rows);
    lines.split_off(from)
}

/// The width the age is given, which fits everything up to `365d`.
const AGE: usize = 4;

/// How long since anything was heard, in the shortest form that says it.
fn age(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// One line of it, so a paragraph of an answer cannot take over a row.
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim()
}

/// `text`, cut to `width` with an ellipsis for what was cut.
fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    match width {
        0 => String::new(),
        1 => "…".to_string(),
        _ => text.chars().take(width - 1).chain(['…']).collect(),
    }
}

/// What a row is indented by, so an agent reads as sitting under the heading
/// it belongs to rather than beside it.
const GUTTER: &str = "  ";

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

/// The colours, by what they mean rather than by what they are. Five of them
/// carry the vendor's own dark theme measured from the 2.1.237 binary: a view
/// beside claude's should not be a different shade of the same idea.
mod role {
    use ratatui::style::Color;

    /// What the next agent will be started with. The terminal's own cyan
    /// rather than a value out of the binary: what colour claude paints the
    /// prospective half of its own header is not something amx has measured,
    /// and an RGB nobody measured would read as one that was.
    pub const PROSPECTIVE: Color = Color::Cyan;

    /// It went the way it was meant to.
    pub const SUCCESS: Color = Color::Rgb(78, 186, 101);
    /// Something is waiting on a person.
    pub const WARNING: Color = Color::Rgb(255, 193, 7);
    /// It was attempted and it failed.
    pub const ERROR: Color = Color::Rgb(255, 107, 128);
    /// It was ended by hand, and nothing more is coming.
    pub const INACTIVE: Color = Color::Rgb(153, 153, 153);
    /// The line the cursor is on. A background rather than a foreground: it
    /// says where the cursor is without taking a colour away from what the
    /// line was already saying.
    pub const SELECTED: Color = Color::Rgb(55, 55, 55);
}

/// What a state is worth saying in colour.
///
/// Whether anything is running is the mark's job, which leaves the colour to
/// carry how it went: an agent still at work has nothing to say about that
/// yet, so it takes the terminal's own colour and earns one by ending.
fn colour(phase: Phase) -> Style {
    match phase {
        // What amx cannot account for wants a person as much as a question
        // does, and the mark is what says which of the two it is.
        Phase::Waiting | Phase::Unknown => Style::new().fg(role::WARNING),
        Phase::Starting | Phase::Working => Style::new(),
        Phase::Idle => dim(),
        Phase::Done => Style::new().fg(role::SUCCESS),
        Phase::Failed => Style::new().fg(role::ERROR),
        Phase::Stopped => Style::new().fg(role::INACTIVE),
    }
}

/// A heading, in whatever it stands for. A state heading is coloured by what
/// that state means; a project is a place, and a place means nothing about how
/// anything is going.
fn heading_colour(under: Under) -> Style {
    match under {
        Under::Group(group) => group_colour(group),
        Under::Project(_) => Style::new(),
    }
    .add_modifier(Modifier::BOLD)
}

/// The same roles over a group, so the count at the top and the rows under the
/// heading say the same thing in the same colour.
fn group_colour(group: Group) -> Style {
    match group {
        Group::NeedsInput => Style::new().fg(role::WARNING),
        Group::Working => Style::new(),
        Group::Idle => dim(),
        Group::Completed => Style::new().fg(role::INACTIVE),
    }
}

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

/// The one treatment everything prospective wears — the profile, the dials,
/// the permission the next agent will run under — so the eye tells what the
/// next spawn will use from what is running now without reading either.
///
/// Weight as well as colour: the counters beside it are already coloured by
/// what they mean, and a terminal with the colour turned off still has to be
/// able to tell a dial from a count.
fn prospective() -> Style {
    Style::new()
        .fg(role::PROSPECTIVE)
        .add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests {
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

    /// The view, with a reading in it.
    fn showing(views: Vec<View>, peek: Option<Peek>) -> Screen {
        let mut screen = Screen::default();
        screen.list.show(views);
        screen.peek = peek;
        screen
    }

    /// The same reading, running somewhere else.
    fn at(mut view: View, dir: &str) -> View {
        view.meta.dir = PathBuf::from(dir);
        view
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
    fn drawn(views: Vec<View>, peek: Option<Peek>, size: (u16, u16)) -> Vec<String> {
        painted(&showing(views, peek), size)
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

        assert_eq!(painted(Phase::Waiting), ("?".into(), role::WARNING, plain));
        assert_eq!(painted(Phase::Unknown), ("~".into(), role::WARNING, plain));
        assert_eq!(painted(Phase::Done), ("●".into(), role::SUCCESS, plain));
        assert_eq!(painted(Phase::Failed), ("✗".into(), role::ERROR, plain));
        assert_eq!(painted(Phase::Stopped), ("⏹".into(), role::INACTIVE, plain));

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
                vec![view(
                    "port-importer-b2c",
                    Phase::Working,
                    Some("Running"),
                    3,
                )],
                None,
            );
            screen.beat = beat;
            painted(&screen, (60, 8))[2].clone()
        };

        assert!(
            at(0).starts_with(&format!("  {} port-importer-b2c", pulse(0))),
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
            screen[1].ends_with("1 waiting · 1 working · 2/5 running"),
            "{:?}",
            screen[1]
        );
        assert_eq!(screen[2], "needs input");
        assert!(screen[3].starts_with("  ? ask-a1b"), "{:?}", screen[3]);
        assert!(screen[3].ends_with("1m"), "{:?}", screen[3]);
        assert_eq!(screen[4], "working");
        assert!(
            screen[5].starts_with(&format!("  {} fix-login-b2c", pulse(0))),
            "{:?}",
            screen[5]
        );
        assert!(screen[5].contains("Running Bash"), "{:?}", screen[5]);
        assert!(screen[5].ends_with("3s"), "{:?}", screen[5]);
        assert_eq!(screen[9], KEYS, "and the keys, where they can be read");
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
        assert_eq!(screen[5], "/src/web");

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
        assert_eq!(screen[1], "working");
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
            painted[0].ends_with("1 working · 1/5 running · s:working"),
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
        let bar = vec![role::SELECTED; 60];
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
    fn headings_count_their_agents_only_while_they_are_holding_them_back() {
        let mut screen = showing(
            vec![
                view("busy-a1b", Phase::Working, None, 3),
                view("busy-b2c", Phase::Working, None, 5),
            ],
            None,
        );

        assert_eq!(
            painted(&screen, (60, 8))[1],
            "working",
            "expanded, the rows are on the screen and counting them is noise"
        );

        screen.list.up();
        screen.list.shut_or_open();
        let painted = painted(&screen, (60, 8));
        assert_eq!(painted[1], "working 2");
        assert!(
            !painted.iter().any(|line| line.contains("busy-a1b")),
            "and the count is standing in for the rows: {painted:?}"
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
            painted(&screen, (60, 8))[1],
            "completed · 1 failed",
            "a screenful of headings says how it went without being opened"
        );

        screen.list.up();
        screen.list.shut_or_open();
        assert_eq!(
            painted(&screen, (60, 8))[1],
            "completed 2 · 1 failed",
            "shutting a group hides the detail of a failure, never the fact"
        );
    }

    #[test]
    fn view_says_when_there_is_nothing_to_show() {
        let screen = drawn(Vec::new(), None, (40, 6));
        assert!(screen[0].starts_with("claude (default)"), "{:?}", screen[0]);
        assert!(screen[0].ends_with("0/5 running"), "{:?}", screen[0]);
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

    #[test]
    fn header_says_the_version_the_profile_the_fleet_and_the_worktree_dial() {
        let screen = painted(
            &launching(vec![
                view("ask-a1b", Phase::Waiting, None, 30),
                view("busy-b2c", Phase::Working, Some("Running Bash"), 3),
            ]),
            WIDE,
        );

        assert!(
            screen[0].starts_with(&format!("amx v{}", env!("CARGO_PKG_VERSION"))),
            "the build's own version, never a literal: {:?}",
            screen[0]
        );
        assert!(
            screen[0].ends_with("worktree: on → .amx/worktrees/<id>"),
            "the dial says what it will do, not merely that it is on: {:?}",
            screen[0]
        );
        assert!(
            screen[1].starts_with("claude (default) · ~/code/amx"),
            "{:?}",
            screen[1]
        );
        assert!(
            screen[1].ends_with("1 waiting · 1 working · 2/5 running"),
            "what the fleet is, and the gate the next one meets: {:?}",
            screen[1]
        );
        assert_eq!(screen[2], "needs input", "and the list starts under it");
    }

    #[test]
    fn header_counts_the_fleet_in_the_words_a_filter_takes() {
        let mut screen = launching(vec![
            view("ask-a1b", Phase::Waiting, None, 30),
            view("done-b2c", Phase::Done, Some("did it"), 60),
        ]);
        assert!(
            screen_line(&screen, WIDE, 1).ends_with("1 waiting · 1 done · 1/5 running"),
            "the heading over the rows says `needs input`; the counter says \
             the word the list can be narrowed by: {:?}",
            screen_line(&screen, WIDE, 1)
        );

        // A narrowing is still read back where it was typed, so a short list
        // says why it is short.
        screen
            .list
            .narrow(vec![Narrow::State(Some("waiting".to_string()))]);
        assert!(
            screen_line(&screen, WIDE, 1).ends_with("1 waiting · 1/5 running · s:waiting"),
            "{:?}",
            screen_line(&screen, WIDE, 1)
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
            screen_line(&screen, WIDE, 1).ends_with("3/5 running"),
            "an agent whose command has ended holds no slot: {:?}",
            screen_line(&screen, WIDE, 1)
        );
    }

    #[test]
    fn header_says_the_model_rather_than_guessing_the_vendors_own() {
        let mut screen = launching(Vec::new());
        assert!(
            screen_line(&screen, WIDE, 1).starts_with("claude (default) · ~/code/amx"),
            "the word, not a guess at what claude would have picked: {:?}",
            screen_line(&screen, WIDE, 1)
        );

        screen.profile.model = "opus".to_string();
        assert!(
            screen_line(&screen, WIDE, 1).starts_with("claude (opus) · ~/code/amx"),
            "{:?}",
            screen_line(&screen, WIDE, 1)
        );

        // An agent the registry never heard of declares no model dial, so
        // there is nothing to put in the parentheses and none are drawn.
        screen.profile.agent = "mock-claude".to_string();
        assert!(
            screen_line(&screen, WIDE, 1).starts_with("mock-claude · ~/code/amx"),
            "{:?}",
            screen_line(&screen, WIDE, 1)
        );
    }

    #[test]
    fn header_sheds_the_path_before_the_dial_and_the_dir_before_the_agent() {
        // Decided here rather than discovered at the edge of a terminal.
        let screen = launching(vec![view("busy-a1b", Phase::Working, None, 3)]);

        let narrow = painted(&screen, (NARROW as u16 - 1, 12));
        assert!(
            narrow[0].ends_with("worktree: on"),
            "the dial is what somebody turns; the path is what it means: {:?}",
            narrow[0]
        );

        let cramped = painted(&screen, (40, 12));
        assert!(
            cramped[1].starts_with("claude (default)"),
            "the dir is the losable half of the profile: {:?}",
            cramped[1]
        );
        assert!(
            cramped[1].ends_with("1 working · 1/5 running"),
            "what the fleet is stays: {:?}",
            cramped[1]
        );
        assert!(
            !cramped[1].contains("code/amx"),
            "a path cut to nothing is not a path: {:?}",
            cramped[1]
        );
    }

    #[test]
    fn header_keeps_the_fleet_on_the_row_that_has_no_room_for_the_profile() {
        // Every group at once on a narrow terminal: the counters are wider
        // than the screen on their own, so nothing is left for the profile.
        // What the fleet is doing is what the row is for, and a row drawn
        // blank says less than one the terminal cut.
        let screen = launching(vec![
            view("ask-a1b", Phase::Waiting, None, 30),
            view("busy-b2c", Phase::Working, None, 3),
            view("idle-c3d", Phase::Idle, None, 30),
            view("done-d4e", Phase::Done, Some("did it"), 60),
        ]);

        let cramped = painted(&screen, (40, 12));
        assert!(
            cramped[1].starts_with("1 waiting · 1 working"),
            "{:?}",
            cramped[1]
        );
    }

    #[test]
    fn header_gives_the_row_back_to_the_list_on_a_short_screen() {
        let screen = launching(vec![view("busy-a1b", Phase::Working, None, 3)]);
        let short = painted(&screen, (60, SHORT as u16 - 1));

        assert!(
            short[0].starts_with("claude (default)"),
            "the line that says what the next agent will be is the one that \
             stays: {:?}",
            short[0]
        );
        assert!(
            short[0].ends_with("1 working · 1/5 running"),
            "{:?}",
            short[0]
        );
        assert_eq!(short[1], "working", "and the list starts a row sooner");
    }

    /// Room for the four headings, their four lines, and the bands above and
    /// below the list.
    const WALL: (u16, u16) = (80, 12);

    #[test]
    fn headings_that_explain_themselves_stand_over_a_fleet_nobody_has_started() {
        let screen = drawn(Vec::new(), None, WALL);

        let mut at = 2;
        for group in Group::ALL {
            assert_eq!(screen[at], group.title(), "{screen:?}");
            assert_eq!(screen[at + 1], format!("{GUTTER}{}", group.blurb()));
            at += 2;
        }
        assert!(
            !screen.iter().any(|line| line.contains("no agents")),
            "the four of them are said instead of the one sentence, not \
             beside it: {screen:?}"
        );
    }

    #[test]
    fn headings_that_explain_themselves_go_the_moment_there_is_anything_to_read() {
        let one = drawn(
            vec![view("done-a1b", Phase::Done, Some("did it"), 60)],
            None,
            WALL,
        );
        assert_eq!(one[2], "completed");
        assert!(
            !one.iter().any(|line| line.contains("here to read")),
            "one agent and there is something to read off the rows: {one:?}"
        );

        // A fleet somebody narrowed to nothing is not a fleet nobody started,
        // and the view already has a sentence for that one.
        let mut screen = showing(Vec::new(), None);
        screen
            .list
            .narrow(vec![Narrow::Name(Some("nobody".to_string()))]);
        assert_eq!(painted(&screen, WALL)[2], "nothing matches a:nobody");

        // On the project axis a heading is a place, and a place nobody is
        // running anything in has nothing to explain.
        let mut screen = showing(Vec::new(), None);
        screen.list.turn();
        assert_eq!(painted(&screen, WALL)[2], "no agents");
    }

    #[test]
    fn headings_that_explain_themselves_fit_the_width_they_ask_for() {
        let widest = Group::ALL
            .into_iter()
            .map(|group| GUTTER.len() + group.blurb().chars().count())
            .max()
            .expect("four groups");
        assert!(
            widest <= BLURBS_WIDE,
            "{widest} columns of copy in a list {BLURBS_WIDE} wide: the floor \
             is a chosen number, so an edit that outgrows it moves the copy \
             back rather than the floor"
        );

        // Under either floor the pair is dropped whole: half an explanation
        // reads worse than the one sentence that was there before it. One row
        // short is a screen whose list band is one short of the eight, which
        // is the header's two rows and the keys' one over that.
        let narrow = drawn(Vec::new(), None, (BLURBS_WIDE as u16 - 1, WALL.1));
        assert_eq!(narrow[2], "no agents");
        let short = drawn(Vec::new(), None, (WALL.0, BLURBS_TALL as u16 + 2));
        assert_eq!(short[2], "no agents");
    }

    #[test]
    fn view_shows_the_fold_and_what_it_is_holding_back() {
        let views = (0..5)
            .map(|n| view(&format!("done-{n}"), Phase::Done, Some("did it"), 60))
            .collect();
        let screen = drawn(views, None, (40, 10));

        assert_eq!(screen[2], "completed");
        assert_eq!(screen.iter().filter(|l| l.contains("done-")).count(), 3);
        assert!(screen[6].contains("… 2 more"), "{:?}", screen[6]);
    }

    #[test]
    fn view_peeks_at_the_question_over_the_screen_it_is_asked_on() {
        let screen = drawn(
            vec![view("ask-a1b", Phase::Waiting, None, 30)],
            Some(Peek {
                id: "ask-a1b".to_string(),
                phase: Phase::Waiting,
                question: Some("Claude needs your permission to use Bash".to_string()),
                body: "$ rm -rf build\nDo you want to proceed?\n\n\n".to_string(),
                changes: false,
            }),
            (60, 12),
        );

        let all = screen.join("\n");
        assert!(all.contains("ask-a1b · waiting"), "{all}");
        assert!(all.contains("Claude needs your permission"), "{all}");
        assert!(all.contains("Do you want to proceed?"), "{all}");
        assert_eq!(
            screen[11], KEYS,
            "the keys stay on the screen under the peek"
        );
        assert!(
            screen.iter().any(|line| line.contains("ask-a1b")),
            "and the list is still there above it: {all}"
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
            (role::ERROR, Modifier::empty())
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
            Some(Peek {
                id: "fix-login-a1b".to_string(),
                phase: Phase::Working,
                question: None,
                body: patch,
                changes: true,
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
        assert_eq!(one[28], "task ▸ port the importer");
        assert_eq!(one[27], "", "one line takes one row, at the foot of it all");

        let three = painted(
            &typing("port the importer\nand its tests\nand the docs"),
            TALL,
        );
        assert_eq!(three[26], "task ▸ port the importer");
        assert_eq!(
            three[27], "       and its tests",
            "a row under the first is indented to it, so a task reads as one \
             thing"
        );
        assert_eq!(three[28], "       and the docs");
        assert_eq!(
            caret(&typing("port it\nand test it"), TALL),
            (18, 28),
            "and the cursor is at the end of the last of them"
        );
    }

    #[test]
    fn composer_wrapping_past_the_width_grows_it_the_same_way_a_newline_does() {
        // Twice the room a sixty-column screen leaves beside the prompt.
        let painted = painted(&typing(&"x".repeat(106)), TALL);
        assert_eq!(painted[27], format!("task ▸ {}", "x".repeat(53)));
        assert_eq!(painted[28], format!("       {}", "x".repeat(53)));
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
            painted[19], "task ▸ row-11",
            "the prompt is on the top row however far the rest has scrolled: \
             {painted:?}"
        );
        assert_eq!(painted[28], "       row-20", "{painted:?}");
        assert!(
            !painted.iter().any(|line| line.contains("row-10")),
            "and what scrolled past is off the screen: {painted:?}"
        );
        assert_eq!(caret(&screen, TALL), (13, 28));
    }

    #[test]
    fn composer_leaves_the_list_it_was_opened_from_on_the_screen() {
        // A third of eight rows is two, whatever the line is holding, and the
        // agents are what the view is for.
        let painted = painted(&typing(&twenty_rows()), (60, 8));
        assert_eq!(painted[5], "task ▸ row-19");
        assert_eq!(painted[6], "       row-20");
        assert_eq!(
            painted[1], "no agents",
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
        assert_eq!(painted[4], "task ▸ port the importer");
        assert!(painted[5].contains("enter starts it"), "{:?}", painted[5]);
        assert!(painted[5].contains("tab out of sight"), "{:?}", painted[5]);
    }

    #[test]
    fn view_lists_every_key_when_somebody_asks_for_them() {
        let mut screen = showing(Vec::new(), None);
        screen.mode = Mode::Keys;

        // Tall enough for every key: the overlay is one column, and a screen
        // shorter than the list cuts the end off it.
        let painted = painted(&screen, (60, 22)).join("\n");
        for (key, does) in HELP {
            assert!(painted.contains(key), "{key} is missing:\n{painted}");
            assert!(painted.contains(does), "{does} is missing:\n{painted}");
        }
    }

    #[test]
    fn view_reads_the_bottom_of_a_screen_and_drops_what_is_blank() {
        assert_eq!(tail("a\nb\nc\n\n\n", 2), ["b", "c"]);
        assert_eq!(tail("a\nb", 5), ["a", "b"]);
        assert!(tail("", 3).is_empty());
    }

    #[test]
    fn view_ages_read_as_a_person_would_say_them() {
        assert_eq!(age(0), "0s");
        assert_eq!(age(59), "59s");
        assert_eq!(age(60), "1m");
        assert_eq!(age(3_600), "1h");
        assert_eq!(age(86_400), "1d");
    }

    #[test]
    fn view_cuts_text_without_losing_the_last_character_to_the_ellipsis() {
        assert_eq!(fit("short", 10), "short");
        assert_eq!(fit("exactly", 7), "exactly");
        assert_eq!(fit("too long by far", 8), "too lon…");
        assert_eq!(fit("anything", 1), "…");
        assert_eq!(fit("anything", 0), "");
    }
}
