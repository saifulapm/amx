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
use std::iter::repeat_n;
use std::sync::OnceLock;

use super::empty;
use super::style::{bold, colour, dim, name_colour, request_colour};
use super::text::{RULE, inert, width_of};
use crate::derive::{self, View};
use crate::pr::Pr;
use crate::store::Phase;
use crate::theme::Theme;
use crate::tui::grid::{self, Widths};
use crate::tui::rows::{self, Group, Item, List, Tally, Under};

/// The agents themselves.
///
/// `floated` is the card's own box, where one is up. The rows above it are
/// drawn where they stood and the rows under it are moved down by its height,
/// so the card stands between the line it hangs off and the rest of the list
/// rather than over any of them. What that pushes off the bottom of the band
/// is what somebody gets back by closing it, and what is left above the card
/// is what the cursor is kept inside.
pub(super) fn agents(
    frame: &mut Frame,
    list: &List,
    area: Rect,
    moment: Moment,
    floated: Option<Rect>,
    theme: Theme,
) {
    if list.is_empty() {
        let nothing = empty::nothing(list, area.width as usize);
        frame.render_widget(Paragraph::new(nothing), area);
        return;
    }

    let visible = area.height - floated.map_or(0, |card| card.height);
    let offset = first_drawn(list, visible);
    let width = area.width as usize;
    let widths = grid::widths(width, list.axis());
    let requests = request_column(list);

    let mut lines: Vec<Line> = list
        .items()
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible as usize)
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
    // The room the card takes, given up by the rows under the line it hangs
    // off: blank here, because the card draws over them itself.
    if let Some(card) = floated {
        let at = (card.y - area.y) as usize;
        lines.splice(at..at, repeat_n(Line::raw(""), card.height as usize));
    }
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

/// Which line of the band the card hangs off: the line the agent it is a look
/// at stands on.
///
/// The cursor's own line where that agent has none, which is where the card was
/// opened from. Never below the last line drawn in front of the card, so the
/// line it hangs off is one somebody can still see.
pub(super) fn hangs_off(list: &List, id: &str, visible: u16) -> u16 {
    let at = list
        .items()
        .iter()
        .position(|item| list.agent(*item).is_some_and(|view| view.id() == id))
        .unwrap_or_else(|| list.cursor());
    at.saturating_sub(first_drawn(list, visible))
        .min(visible.saturating_sub(1) as usize) as u16
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
            Under::Project(_) => path_heading(list.title(under), tally, widths, width, theme),
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
    let rule = RULE.repeat(width.saturating_sub(spent).max(1));
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

/// The heading over a project, which is a path rather than a word.
///
/// The same rule and the same right-aligned count as the heading over a group,
/// so the two axes read as one document and the right margin is one line of
/// numbers on either. What changes is the label: a path is not a word, so it is
/// not uppercased, and the weight goes on the last segment with the parents it
/// hangs off dim behind it — which gives a left-heavy string of no fixed length
/// a bright end to find it by.
///
/// A path too long for the heading loses its middle rather than its end, which
/// is [`grid::elide`]'s business: the end is the segment that says which
/// worktree of a project this is, and cutting there would leave every one of
/// them reading the same.
fn path_heading(
    title: String,
    tally: Tally,
    widths: Widths,
    width: usize,
    theme: Theme,
) -> Line<'static> {
    let failures = match tally.failures {
        0 => String::new(),
        failures => format!("· {failures} failed "),
    };
    let path = grid::elide(&title, grid::path_room(width, failures.trim_end()));
    // Everything up to the last separator is where the directory is; what
    // comes after it is which directory it is.
    let cut = path.rfind('/').map(|at| at + 1).unwrap_or(0);
    // The same arithmetic the group heading's rule is left over from.
    let spent = 1 + width_of(&path) + 1 + width_of(&failures) + GAP + widths.age;
    let rule = RULE.repeat(width.saturating_sub(spent).max(1));
    Line::from(vec![
        Span::styled(format!(" {}", &path[..cut]), dim()),
        Span::styled(path[cut..].to_string(), bold()),
        Span::raw(" "),
        Span::styled(failures, Style::new().fg(theme.failed)),
        Span::styled(rule, dim()),
        Span::raw(" ".repeat(GAP)),
        Span::styled(grid::padl(&tally.members.to_string(), widths.age), dim()),
    ])
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
/// the state word keeps what the phase has to say because it replaces the
/// glyph's job there — see [`state_colour`]. What the cursor is on is said by
/// the bar under it, not by the row changing its tones.
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
        false => inert(first_line(view.line().unwrap_or(""))),
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
            state_colour(theme, phase),
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

/// What the state word is painted in under a project heading: the phase's own
/// colour where the phase has one, and dim where it has not.
///
/// The word is the glyph's job moved down a level rather than a second summary.
/// A row that has ended says how it went in the colour that says so, and a row
/// still at work has nothing to say about that yet — so it stays out of the way
/// of the line beside it, which is the part somebody is reading.
fn state_colour(theme: Theme, phase: Phase) -> Style {
    match phase {
        Phase::Starting | Phase::Working => dim(),
        phase => colour(theme, phase),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::{Evidence, Verdict};
    use crate::pr::Standing;
    use crate::store::{Meta, State};
    use crate::tmux::{PaneId, Socket};
    use crate::tui::paint::text::fit;
    use crate::tui::paint::{Card, draw};
    use crate::tui::rows::Narrow;
    use crate::tui::{Arm, Screen};
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
        line.split('┈').next().unwrap_or_default().trim()
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

    /// The background of every cell across one row of the list.
    fn behind(screen: &Screen, size: (u16, u16), row: u16) -> Vec<Color> {
        let buffer = cells(screen, size);
        (0..size.0).map(|at| buffer[(at, row)].bg).collect()
    }

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

    /// A screen with room for the bands above and below the list, the space
    /// between the header and it, and a group or two under that.
    const WALL: (u16, u16) = (80, 12);

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
            screen[9], "space card   enter attach   ctrl+x stop   ? keys",
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

        assert!(screen[2].starts_with(" /src/api "), "{:?}", screen[2]);
        assert!(screen[3].contains("ask-a1b"), "{:?}", screen[3]);
        assert!(
            screen[3].contains("waiting"),
            "the heading is a place, so the row says the state: {:?}",
            screen[3]
        );
        assert!(screen[4].contains("done"), "{:?}", screen[4]);
        assert_eq!(screen[5], "", "the next project stands off from this one");
        assert!(screen[6].starts_with(" /src/web "), "{:?}", screen[6]);

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
        assert_eq!(rule.symbol(), "┈", "the rule runs out to the count");
        assert!(
            rule.modifier.contains(Modifier::DIM) && !rule.modifier.contains(Modifier::BOLD),
            "{:?}",
            rule.modifier
        );
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
    fn rows_neutralise_what_an_agent_said_the_way_the_name_and_the_question_are() {
        // An escape byte and a zero-width character in what an agent said are
        // neutralised the way the name at row 338 and the card's question at
        // card.rs:402 are: replaced with a space rather than dropped, so the
        // row stays exactly as wide as the record spells it.
        let said = "pro\u{1b}ceed\u{200b}now";
        let row = drawn(
            vec![view("fix-login-a1b", Phase::Done, Some(said), 60)],
            None,
            (60, 8),
        )
        .into_iter()
        .find(|line| line.contains("fix-login-a1b"))
        .expect("the agent's row");
        assert!(!row.contains('\u{1b}'), "{row:?}");
        assert!(!row.contains('\u{200b}'), "{row:?}");
        assert!(row.contains("pro ceed now"), "{row:?}");
        assert!(row.ends_with("1m"), "{row:?}");
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
