//! The agents themselves, which is what the view is for.
//!
//! A row is one line, always: an agent's answer is a paragraph, and a
//! paragraph in a list is how a list stops being one. What a row says stands
//! on the widths the grid fixes rather than on what this fleet happens to
//! hold, so the columns are where they were when the last agent ended.
//!
//! The weight goes where the work is. A row that is asking carries the one
//! colour and the one bold name on the wall, and every other row says its
//! state on the glyph alone.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::sync::OnceLock;

use super::empty;
use super::style::{bold, colour, dim, name_colour, request_colour};
use super::text::{inert, width_of};
use crate::derive::{self, View};
use crate::pr::Pr;
use crate::store::Phase;
use crate::theme::Theme;
use crate::tui::grid::{self, Widths};
use crate::tui::rows::{self, Group, Item, List, Tally, Under};

/// The agents themselves.
///
/// `visible` is how many of the rows are not behind the card, and it is what
/// the cursor is kept inside. The rest are drawn anyway: a card is in front of
/// a list, not instead of one, and the rows it covers are the ones somebody
/// gets back by closing it.
pub(super) fn agents(
    frame: &mut Frame,
    list: &List,
    area: Rect,
    moment: Moment,
    visible: u16,
    theme: Theme,
) {
    if list.is_empty() {
        let nothing = empty::nothing(list, area.width as usize);
        frame.render_widget(Paragraph::new(nothing), area);
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
pub(super) fn first_drawn(list: &List, visible: u16) -> usize {
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
pub(super) struct Moment<'a> {
    pub(super) beat: usize,
    pub(super) armed: &'a [String],
    /// The line the pointer is resting on, if it is resting on an agent's.
    pub(super) hover: Option<usize>,
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

/// What stands between two columns of the list, whether that is a name and a
/// summary or a heading's rule and its count.
const GAP: usize = 2;

/// One line of it, so a paragraph of an answer cannot take over a row.
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim()
}

/// What a row is indented by, so an agent reads as sitting under the heading
/// it belongs to rather than beside it. One column for each mark a row can
/// carry, which is what lets the marks cost the list no width at all.
const GUTTER: &str = "  ";
const _: () = assert!(GUTTER.len() == MARKS);

/// The vendor's glyph set for a terminal. Ghostty draws the eight-spoked
/// asterisk where everything else gets a plain one, and that is the only thing
/// `$TERM` decides. Measured from the 2.1.237 bundle.
pub(super) fn set_for(term: &str) -> [&'static str; 6] {
    match term {
        "xterm-ghostty" => ["·", "✢", "✳", "✶", "✻", "✻"],
        _ => ["·", "✢", "*", "✶", "✻", "✽"],
    }
}

/// That set for this terminal, read once: `$TERM` does not change under a
/// running view, and the vendor memoizes it for the same reason.
pub(super) fn set() -> [&'static str; 6] {
    static SET: OnceLock<[&'static str; 6]> = OnceLock::new();
    *SET.get_or_init(|| set_for(std::env::var("TERM").unwrap_or_default().as_str()))
}

/// Which of the six a working row rests on, and the frame the pulse is
/// largest at either side of.
pub(super) const LIVE: usize = 4;

/// The six ping-ponged into twelve frames, which is the vendor's own working
/// mark ported rather than approximated: the set forwards and then backwards,
/// one frame every 120ms. It grows from a dot to the largest asterisk and
/// shrinks back, so a working row breathes rather than spins.
pub(super) fn pulse(beat: usize) -> &'static str {
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
pub(super) fn resting(phase: Phase) -> &'static str {
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
