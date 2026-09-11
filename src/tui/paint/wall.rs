//! The agents themselves, which is what the view is for.
//!
//! A row is one line, always: an agent's answer is a paragraph, and a
//! paragraph in a list is how a list stops being one. What a row says stands
//! on the widths the grid fixes rather than on what this fleet happens to
//! hold, so the columns are where they were when the last agent ended.
//!
//! A row says its state on one glyph: the shape is whether there is still a
//! process to go back to, the colour is which state that process is in, and
//! the pulse is a turn running. It says nothing at all with weight, because the
//! wall spends none: a screenful of names half of which are shouting is a
//! screenful nobody reads down.
//!
//! What marks the row somebody is working with is strength instead. Every name
//! is as quiet as the summary beside it and the heading over it, but the one
//! under the cursor and the one under the pointer, and those come up to the
//! terminal's own.
//!
//! One colour is not about the agent either: the row the terminal was lent to
//! wears the accent on its name. Detaching from a pane lands on a wall of rows
//! that all look alike, and the one somebody was just inside is the one they
//! are about to look for.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::sync::OnceLock;

use super::empty;
use super::style::{colour, dim, name_colour, request_colour};
use super::text::inert;
use crate::derive::{self, Evidence, View};
use crate::pr::Pr;
use crate::store::Phase;
use crate::theme::Theme;
use crate::tui::grid::{self, Widths};
use crate::tui::rows::{self, Group, Item, List, Tally, Under};

/// The agents themselves.
///
/// The whole band, whatever else is on the screen: a card stands in a band of
/// its own at the foot, so the rows are drawn where they were drawn before it
/// opened and none of them moves while somebody walks the list with it up.
pub(super) fn agents(frame: &mut Frame, list: &List, area: Rect, moment: Moment, theme: Theme) {
    if list.is_empty() {
        let nothing = empty::nothing(list, area.width as usize);
        frame.render_widget(Paragraph::new(nothing), area);
        return;
    }

    let offset = first_drawn(list, area.height);
    let width = area.width as usize;
    let widths = grid::widths(width, list.axis());
    let requests = request_column(list);

    let lines: Vec<Line> = list
        .items()
        .iter()
        .enumerate()
        .skip(offset)
        .take(area.height as usize)
        .map(|(at, item)| {
            line(
                list,
                *item,
                At {
                    selected: at == list.cursor(),
                    hovered: moment.hover == Some(at),
                    lent: moment
                        .lent
                        .is_some_and(|id| list.agent(*item).is_some_and(|view| view.id() == id)),
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
/// keep the cursor on the screen. Shared with the map the mouse reads, so a
/// click lands on the row the frame actually drew there.
pub(super) fn first_drawn(list: &List, visible: u16) -> usize {
    list.cursor()
        .saturating_sub((visible.max(1) as usize).saturating_sub(1))
}

/// What the clock has made of the list at the moment it is drawn: which frame
/// of the working pulse the rows are on, and which of them a press has armed —
/// one row, or every row under the heading the press was on.
///
/// Neither is a fact about an agent, and neither is worth writing down: they
/// are what the view is doing while somebody watches it, so they are handed to
/// the rows and forgotten with the frame.
#[derive(Clone, Copy)]
pub(super) struct Moment<'a> {
    pub(super) beat: usize,
    pub(super) armed: &'a [String],
    /// Whether a heading armed them, which is what the armed rows say the
    /// press after this one would do. One arm at a time, so it is a fact about
    /// the frame rather than about each row.
    pub(super) swept: bool,
    /// The line the pointer is resting on, if it is resting on an agent's.
    pub(super) hover: Option<usize>,
    /// The agent the terminal was last lent to, where it has been lent to one.
    pub(super) lent: Option<&'a str>,
}

/// How the cursor and the pointer stand to one line: on it, over it, or come
/// back from it.
#[derive(Clone, Copy, Default)]
struct At {
    selected: bool,
    hovered: bool,
    lent: bool,
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
            Under::Group(group) => heading(group, tally, theme),
            Under::Project(_) => path_heading(list.title(under), tally, width, theme),
        },
        Item::Fold(hidden) => Line::styled(format!("{GUTTER}… {hidden} more"), dim()),
        Item::Agent(_) => match list.agent(item) {
            Some(view) => row(
                view,
                list.requests(view),
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
/// The group's own words, and the line ends there. What makes it a heading is
/// the blank row over it and the rows indented under it, so it needs neither
/// case nor weight to be read as one — and with no number waiting at the far
/// edge there is nothing for a rule to carry the eye out to. That leaves the
/// right margin of the wall the ages alone.
///
/// The count is there only while the rows are not: an open group is counted by
/// the rows a person is looking at, and saying it again in a number is the same
/// fact twice. Shut, the number is all that stands in for them, so it follows
/// the label rather than the edge.
///
/// The failures come after it either way, because that is the one thing a
/// heading is worth reading without opening it — an agent that failed is the
/// reason somebody came to the screen.
fn heading(group: Group, tally: Tally, theme: Theme) -> Line<'static> {
    // Dim like the rows under it, with the one exception the wall makes up
    // here: the group that wants a person says so in colour, which is what the
    // weight used to be spent on and reads louder than it did.
    let label = match group {
        Group::NeedsInput => Style::new().fg(theme.waiting),
        _ => dim(),
    };
    Line::from(vec![
        Span::styled(format!("{}{}", group.title(), count(tally)), label),
        Span::styled(failures(tally), Style::new().fg(theme.failed)),
    ])
}

/// The heading over a project, which is a path rather than a word.
///
/// The same words in the same places as the heading over a group, so the two
/// axes read as one document: dim end to end, no weight on the last segment,
/// and the count only where the rows are shut.
///
/// A path too long for the heading loses its middle rather than its end, which
/// is [`grid::elide`]'s business: the end is the segment that says which
/// worktree of a project this is, and cutting there would leave every one of
/// them reading the same.
fn path_heading(title: String, tally: Tally, width: usize, theme: Theme) -> Line<'static> {
    let failed = failures(tally);
    let path = grid::elide(&title, grid::path_room(width, failed.trim()));
    Line::from(vec![
        Span::styled(format!("{path}{}", count(tally)), dim()),
        Span::styled(failed, Style::new().fg(theme.failed)),
    ])
}

/// How many agents a heading answers for, said only where the rows it stands
/// over are not on the screen to be counted.
fn count(tally: Tally) -> String {
    match tally.shut {
        true => format!(" {}", tally.members),
        false => String::new(),
    }
}

/// And how many of them failed, said whether the group is open or shut.
fn failures(tally: Tally) -> String {
    match tally.failures {
        0 => String::new(),
        failures => format!(" · {failures} failed"),
    }
}

/// An agent's row: what state it is in, what it is called, what its work is
/// waiting on out in the world, what it is up to, and how long it has worked.
///
/// Three cells before the name — one of indent, the state glyph and the space
/// after it — then the name, the summary, and the age right-aligned at the
/// edge, all on the widths the grid fixes for the screen. Fixed rather than
/// measured off the fleet, so the columns stand where they stood when the last
/// agent ended and the row a person learned wide is the row they get narrow.
///
/// The wall spends no weight, so a row is drawn as quietly as the heading over
/// it: the name dim, what the agent said dim beside it, and the state on the
/// glyph alone. The name under the cursor and the name under the pointer are
/// what come up to the terminal's own, which is the row somebody is working
/// with saying so. A row that is asking puts its question at full strength,
/// because that is the sentence somebody opened the view to read. The
/// exceptions earn their colour: a waiting name and a failed one say so without
/// their glyph being read, the pull request's number answers how the work went,
/// and under a project heading the state word keeps what the phase has to say
/// because it replaces the glyph's job there — see [`state_colour`].
///
/// The row the terminal was lent to takes the accent on its name: about the
/// person at the screen rather than the agent, and given up wherever the state
/// has already coloured the name.
///
/// A row a press has armed says that instead of what the agent said, in the
/// colour of a thing waiting on a person. The summary is the one part of a row
/// amx is free to speak over: the state, the name and the age are what the row
/// is for, and a warning that took a column of its own would move every row
/// under it for as long as it was up.
fn row(
    view: &View,
    prs: &[Pr],
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
    let said = match (armed, moment.swept) {
        (true, true) => AGAIN_ALL.to_string(),
        (true, false) => AGAIN.to_string(),
        (false, _) => inert(first_line(view.line().unwrap_or(""))),
    };

    let asking = phase == Phase::Waiting;
    let mut spans = vec![
        Span::raw(GUTTER),
        Span::styled(
            format!("{} ", icon(phase, &view.verdict.evidence, moment.beat)),
            colour(theme, phase),
        ),
        Span::styled(
            format!("{name}{}", " ".repeat(GAP)),
            // The two rows a person is working with, brought up out of the
            // quiet the rest of the wall is drawn at: the one the cursor is on,
            // and the one the pointer is resting on — which, without the bar,
            // is the whole of what a hover is.
            name_colour(theme, phase, at.selected || at.hovered, at.lent),
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

/// And what a row a heading armed says, which is more: the press after it
/// stops every live agent under that heading before it forgets them all.
///
/// The whole group wears it, whatever each row is doing, because the press is
/// about the group and a row cannot say what the press will cost by speaking
/// only for itself.
const AGAIN_ALL: &str = "ctrl+x again stops and forgets";

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

/// What stands between two columns of a row, whether that is a name and a
/// summary or a summary and the seconds at the edge.
const GAP: usize = 2;

/// One line of it, so a paragraph of an answer cannot take over a row.
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim()
}

/// What a row is indented by, so an agent reads as sitting under the heading
/// it belongs to rather than beside it. One blank cell, which is what the
/// vendor's own view spends there: a wall that put a mark in it would be a
/// column a person has to learn before the one they came to read.
const GUTTER: &str = " ";

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

/// What a row whose process has gone is marked with: the dot it left behind,
/// which is the one shape here the vendor's set does not hand out.
const ENDED: &str = "∙";

/// The mark a state rests on: the vendor's own asterisk while there is still a
/// process to go back to, and that dot once there is not.
///
/// Two shapes over eight states, because the shape is not where a state is
/// said — the colour is, and a wall of eight shapes is a wall somebody reads a
/// legend for. What the shape carries is the one thing the colour cannot: an
/// agent still there is one somebody can attach to, answer or stop, and an
/// agent that has gone is a record to read. That is what a person walking the
/// list is deciding on, and it survives a terminal with the colour turned off.
///
/// The live shape is read out of the pulse rather than spelled a second time
/// here, so a row that stops working settles onto the glyph it was already
/// breathing through rather than changing under the reader.
pub(super) fn resting(phase: Phase) -> &'static str {
    match phase {
        Phase::Waiting | Phase::Starting | Phase::Working | Phase::Idle | Phase::Unknown => {
            set()[LIVE]
        }
        Phase::Done | Phase::Failed | Phase::Stopped => ENDED,
    }
}

/// The mark on a row now: an agent whose turn is running is drawn a frame at a
/// time, and every other state stands still.
///
/// Starting as well as working, because coming up is the first part of a turn
/// and the pulse is what says a turn is under way. Which of the two it is, is
/// on the row in words under a project heading and in the heading itself under
/// a state one.
///
/// An agent amx let go is the dot whatever its record says, and the evidence
/// is asked before the phase for it: parking takes the pane and leaves the
/// record standing, so the phase is the one the agent was in and the shape is
/// the only thing on the row that can say the pane has gone. The colour stays
/// the state's own, because the state is still true — see
/// [`crate::derive::Evidence::LetGo`].
fn icon(phase: Phase, evidence: &Evidence, beat: usize) -> &'static str {
    match (evidence, phase) {
        (Evidence::LetGo, _) => ENDED,
        (_, Phase::Starting | Phase::Working) => pulse(beat),
        (_, phase) => resting(phase),
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
    use ratatui::style::{Color, Modifier};
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

    /// The same reading, with somebody having been to look at what it is
    /// holding. The wall paints it neither way; what it moves is where the row
    /// sorts against the completed fold, which is [`rows`]'s business.
    fn read(mut view: View) -> View {
        view.state.seen = view.state.last_event.max(view.state.since);
        view
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
        let cell = cells(screen, size)[(1, row)].clone();
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

    /// What a heading line says: the group's own words, the count where the
    /// group is shut, and how many failed under it where any did.
    fn heading_of(line: &str) -> &str {
        line.trim()
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

    /// And the strength it was painted at, for the tests about which of the
    /// wall's rows is brought up out of the quiet.
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
    fn glyphs_say_a_process_that_is_there_from_one_that_has_gone() {
        // Two shapes over eight states, and the colour says which of the eight
        // it is: a wall of eight shapes is a wall somebody reads a legend for.
        let live = [
            Phase::Waiting,
            Phase::Starting,
            Phase::Working,
            Phase::Idle,
            Phase::Unknown,
        ];
        let gone = [Phase::Done, Phase::Failed, Phase::Stopped];
        assert_eq!(
            live.len() + gone.len(),
            EVERY.len(),
            "every state is one or the other"
        );
        for phase in live {
            assert_eq!(resting(phase), "✻", "{phase} is still there");
        }
        for phase in gone {
            assert_eq!(resting(phase), "∙", "{phase} has ended");
        }
        assert_eq!(
            resting(Phase::Working),
            set()[LIVE],
            "and the live shape is the vendor's own, which is the frame the \
             pulse grows out of and falls back to"
        );
    }

    #[test]
    fn glyphs_pulse_a_working_row_through_twelve_frames() {
        let set = set();
        let want: Vec<&str> = set.iter().chain(set.iter().rev()).copied().collect();
        let frames: Vec<&str> = (0..12).map(pulse).collect();

        assert_eq!(frames, want, "the set, and then the set backwards");
        assert_eq!(pulse(12), pulse(0), "and round again");

        // The pulse is a turn running, which starting is the first part of.
        for phase in [Phase::Starting, Phase::Working] {
            assert_eq!(
                icon(phase, &Evidence::Hooks, 1),
                pulse(1),
                "{phase} is a turn under way"
            );
        }
        for phase in EVERY
            .iter()
            .filter(|phase| !matches!(phase, Phase::Starting | Phase::Working))
        {
            assert_eq!(
                icon(*phase, &Evidence::Hooks, 1),
                resting(*phase),
                "and {phase} stands still"
            );
        }
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

        // The colour is the whole of what the glyph says, weight and all: the
        // wall spends no weight on anything, the glyph included.
        assert_eq!(
            painted(Phase::Waiting),
            ("✻".into(), theme().waiting, plain)
        );
        assert_eq!(painted(Phase::Done), ("∙".into(), theme().done, plain));
        assert_eq!(painted(Phase::Failed), ("∙".into(), theme().failed, plain));
        assert_eq!(
            painted(Phase::Stopped),
            ("∙".into(), theme().stopped, plain)
        );

        // An agent still at work has nothing to say about how it went, so it
        // takes the terminal's own colour and the pulse does the talking. An
        // agent that has finished its turn and is sitting there is quiet, and
        // one amx cannot account for is neither: it stands still in the
        // terminal's own, which is the one thing left to tell it from a row
        // that is asking.
        assert_eq!(
            painted(Phase::Starting),
            (pulse(0).into(), Color::Reset, plain)
        );
        assert_eq!(
            painted(Phase::Working),
            (pulse(0).into(), Color::Reset, plain)
        );
        assert_eq!(
            painted(Phase::Idle),
            ("✻".into(), Color::Reset, Modifier::DIM)
        );
        assert_eq!(painted(Phase::Unknown), ("✻".into(), Color::Reset, plain));
    }

    #[test]
    fn glyphs_draw_the_dot_on_an_agent_amx_let_go() {
        // The same idle agent twice: one sitting at its prompt, and one whose
        // pane amx took while nobody was watching. The record says the same
        // thing about both — nothing about the agent ended — so the shape is
        // the only thing left to say there is nothing there to attach to.
        let row = |evidence| {
            let mut idle = view(
                "fix-login-a1b",
                Phase::Idle,
                Some("the login bug is fixed"),
                240,
            );
            idle.verdict.evidence = evidence;
            let screen = showing(vec![idle], None);
            (
                mark(&screen, (60, 8), 2),
                painted(&screen, (60, 8))[2].clone(),
            )
        };

        let (there, at_its_prompt) = row(Evidence::Hooks);
        let (gone, let_go) = row(Evidence::LetGo);

        assert_eq!(there.0, set()[LIVE], "a pane to attach to, answer or stop");
        assert_eq!(
            gone.0, ENDED,
            "and a record to read, which enter brings back"
        );
        assert_eq!(
            (gone.1, gone.2),
            (there.1, there.2),
            "painted in the state's own colour, because the state has not \
             changed"
        );
        assert_eq!(
            let_go.replace(ENDED, set()[LIVE]),
            at_its_prompt,
            "and the name, what it said and the seconds stand where they stood"
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
            at(0).starts_with(&format!(" {} port-import-b2c", pulse(0))),
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
        assert_eq!(heading_of(&screen[2]), "Needs input");
        assert!(
            screen[3].starts_with(" ✻ ask-a1b"),
            "a cell of indent, the glyph and a space, and then the name: \
             {:?}",
            screen[3]
        );
        assert!(screen[3].ends_with("1m"), "{:?}", screen[3]);
        assert_eq!(screen[4], "", "the next group stands off from this one");
        assert_eq!(heading_of(&screen[5]), "Working");
        assert!(
            screen[6].starts_with(&format!(" {} fix-login-b2c", pulse(0))),
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
        assert!(
            !drawn[2].contains("stops and forgets"),
            "a row that armed itself says what its own second press does, and no more: {:?}",
            drawn[2]
        );
    }

    #[test]
    fn view_says_on_a_row_a_heading_armed_that_the_press_after_stops_it_too() {
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
        screen.arm = Some(Arm {
            ids: vec!["fix-login-a1b".to_string(), "port-importer-b2c".to_string()],
            swept: true,
            at: Instant::now(),
        });
        let drawn = painted(&screen, size);
        for row in [2, 3] {
            assert!(
                drawn[row].contains("ctrl+x again stops and forgets"),
                "the press over a group stops the live rows under it as well as forgetting them all: {:?}",
                drawn[row]
            );
        }
        assert_eq!(
            word_colour(&screen, size, 2, "ctrl+x again stops and forgets"),
            theme().waiting,
            "in the colour a row armed on its own wears"
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

        assert_eq!(screen[2], "/src/api", "{screen:?}");
        assert!(screen[3].contains("ask-a1b"), "{:?}", screen[3]);
        assert!(
            screen[3].contains("waiting"),
            "the heading is a place, so the row says the state: {:?}",
            screen[3]
        );
        assert!(screen[4].contains("done"), "{:?}", screen[4]);
        assert_eq!(screen[5], "", "the next project stands off from this one");
        assert_eq!(screen[6], "/src/web", "{screen:?}");

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
        assert_eq!(heading_of(&screen[1]), "Working");
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
            painted[2].starts_with(&format!(" {} busy-a1b", pulse(0))),
            "a row reads the same whether or not the cursor is on it: {:?}",
            painted[2]
        );
    }

    #[test]
    fn headings_read_the_groups_own_words_and_stop_there() {
        // The words somebody would say out loud, and nothing after them: no
        // rule, because there is no number at the far end to carry the eye out
        // to, and the right margin is the ages alone.
        let screen = drawn(a_fleet(), None, (60, 12));

        assert_eq!(screen[3], "Needs input");
        assert_eq!(screen[6], "Working");
        assert!(
            !screen.iter().any(|line| line.contains('┈')),
            "nothing is run out to the edge of the wall: {screen:?}"
        );
    }

    #[test]
    fn headings_count_their_agents_only_once_the_rows_are_shut() {
        let size = (60, 8);
        let mut screen = showing(
            vec![
                view("busy-a1b", Phase::Working, None, 3),
                view("busy-b2c", Phase::Working, None, 5),
            ],
            None,
        );

        assert_eq!(
            painted(&screen, size)[1],
            "Working",
            "the rows under an open heading are the count, and a number beside \
             them is the same fact drawn twice"
        );

        screen.list.up();
        screen.list.shut_or_open();
        let drawn = painted(&screen, size);
        assert_eq!(
            drawn[1], "Working 2",
            "shut, the count is all that stands in for them, so it follows the \
             label rather than the far edge"
        );
        assert!(
            !drawn.iter().any(|line| line.contains("busy-a1b")),
            "{drawn:?}"
        );
        assert_eq!(
            word_colour(&screen, size, 1, "2"),
            Color::Reset,
            "and it is the label's own paint: a count is not a second state"
        );
        assert!(word_modifier(&screen, size, 1, "2").contains(Modifier::DIM));
    }

    #[test]
    fn headings_say_how_many_failed_whether_or_not_the_rows_are_under_them() {
        let size = (60, 8);
        let mut screen = showing(
            vec![
                view("done-a1b", Phase::Done, Some("did it"), 60),
                view("broke-b2c", Phase::Failed, Some("could not"), 60),
            ],
            None,
        );

        assert_eq!(
            painted(&screen, size)[1],
            "Completed · 1 failed",
            "a screenful of headings says how it went without being opened"
        );
        assert_eq!(
            word_colour(&screen, size, 1, "· 1 failed"),
            theme().failed,
            "the one thing up here worth a colour besides the group that wants \
             a person"
        );

        screen.list.up();
        screen.list.shut_or_open();
        assert_eq!(
            painted(&screen, size)[1],
            "Completed 2 · 1 failed",
            "shutting a group hides the detail of a failure, never the fact, \
             and the failures keep their place after the count"
        );
        assert_eq!(
            word_colour(&screen, size, 1, "2 ·"),
            Color::Reset,
            "a group that has ended does not paint its count the colour of a \
             stopped agent: the margin that number stood in is gone"
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
        assert_eq!(heading_of(&screen[3]), "Needs input", "the first heading");
        assert!(screen[4].contains("ask-a1b"), "{screen:?}");
        assert_eq!(screen[5], "", "a blank line stands the next group off");
        assert_eq!(heading_of(&screen[6]), "Working");
        assert!(screen[7].contains("busy-b2c"), "{screen:?}");
    }

    #[test]
    fn headings_carry_no_weight_and_one_colour() {
        // What makes a heading here is the blank row over it and the rows
        // indented under it, not weight: the wall spends none. So a heading
        // reads as quiet as the summaries beside the rows it heads, and the
        // one thing that breaks the quiet is a group waiting on a person.
        let screen = showing(a_fleet(), None);
        let cells = cells(&screen, (60, 10));

        let asking = cells[(1, 2)].clone();
        assert_eq!(
            asking.fg,
            theme().waiting,
            "the group that wants a person is the one carrying colour up here"
        );
        assert!(
            !asking.modifier.contains(Modifier::BOLD),
            "and it carries it instead of weight: {:?}",
            asking.modifier
        );

        let working = cells[(1, 5)].clone();
        assert_eq!(working.fg, Color::Reset, "and the rest of them do not");
        assert!(
            working.modifier.contains(Modifier::DIM) && !working.modifier.contains(Modifier::BOLD),
            "{:?}",
            working.modifier
        );
    }

    #[test]
    fn path_headings_read_the_way_a_group_heading_does() {
        // One document on either axis: the same words in the same places, dim
        // end to end, with the count only where the rows are not.
        let size = (60, 10);
        let mut screen = by_project(vec![
            at(view("ask-a1b", Phase::Waiting, None, 30), "/src/api"),
            at(
                view("broke-b2c", Phase::Failed, Some("could not"), 60),
                "/src/api",
            ),
        ]);

        assert_eq!(painted(&screen, size)[2], "/src/api · 1 failed");
        assert_eq!(word_colour(&screen, size, 2, "· 1 failed"), theme().failed);
        for word in ["/src/api", "api"] {
            let painted = word_modifier(&screen, size, 2, word);
            assert!(
                painted.contains(Modifier::DIM) && !painted.contains(Modifier::BOLD),
                "the last segment of a path carries no more weight than its \
                 parents do: {painted:?}"
            );
        }

        screen.list.up();
        screen.list.shut_or_open();
        assert_eq!(painted(&screen, size)[2], "/src/api 2 · 1 failed");
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
        assert_eq!(heading_of(&short[5]), "Completed");
        assert_eq!(short.iter().filter(|l| l.contains("done-")).count(), 2);
        assert!(
            short[8].contains("… 3 more"),
            "the fold stands on the last row the band has: {short:?}"
        );
    }

    #[test]
    fn rows_bring_up_the_name_under_the_cursor_and_leave_the_wall_quiet() {
        let size = (60, 10);
        let screen = showing(
            vec![
                read(view(
                    "fix-login-a1b",
                    Phase::Done,
                    Some("wrote the parser"),
                    60,
                )),
                read(view(
                    "port-import-b2c",
                    Phase::Done,
                    Some("wrote the tests"),
                    300,
                )),
            ],
            None,
        );

        // The cursor opens on the first agent, and its name is the one thing on
        // the wall at the terminal's own strength. Everything else is dim: what
        // the agent said, how long it worked, and the whole of the row under it.
        let named = word_modifier(&screen, size, 3, "fix-login-a1b");
        assert!(
            !named.contains(Modifier::DIM) && !named.contains(Modifier::BOLD),
            "the name under the cursor comes up without weight: {named:?}"
        );
        for (row, word) in [
            (3, "wrote the parser"),
            (3, "1m"),
            (4, "port-import-b2c"),
            (4, "wrote the tests"),
            (4, "5m"),
        ] {
            let painted = word_modifier(&screen, size, row, word);
            assert!(
                painted.contains(Modifier::DIM) && !painted.contains(Modifier::BOLD),
                "{word} is drawn at the quiet the rest of the wall is: {painted:?}"
            );
        }

        // The state is carried by the glyph's colour alone.
        let (glyph, painted, _) = mark(&screen, size, 4);
        assert_eq!((glyph.as_str(), painted), ("∙", theme().done));
    }

    #[test]
    fn rows_say_nothing_about_who_has_been_to_read_them() {
        let size = (60, 10);
        let screen = showing(
            vec![
                view("fix-login-a1b", Phase::Done, Some("wrote the parser"), 60),
                read(view(
                    "port-import-b2c",
                    Phase::Done,
                    Some("wrote the tests"),
                    300,
                )),
                read(view("ask-c3d", Phase::Waiting, Some("Proceed?"), 30)),
            ],
            None,
        );

        // An ending nobody has been to read and one somebody has been through
        // are the same row: whether a person has caught up is what keeps the
        // unread one in front of the fold, and the paint says none of it.
        for (row, name) in [(6, "fix-login-a1b"), (7, "port-import-b2c")] {
            let painted = word_modifier(&screen, size, row, name);
            assert!(
                painted.contains(Modifier::DIM) && !painted.contains(Modifier::BOLD),
                "{name} is as quiet as the other: {painted:?}"
            );
        }

        // The colour a state earned stays on the name off the cursor's row, at
        // the strength the rest of the wall is drawn at.
        assert_eq!(word_colour(&screen, size, 3, "ask-c3d"), theme().waiting);
        assert!(
            !word_modifier(&screen, size, 3, "ask-c3d").contains(Modifier::DIM),
            "and the cursor opens on it, which is what brings it up"
        );
    }

    #[test]
    fn rows_a_hovered_name_comes_up_the_way_the_cursors_does() {
        let size = (60, 10);
        let mut screen = showing(
            vec![
                read(view(
                    "fix-login-a1b",
                    Phase::Done,
                    Some("wrote the parser"),
                    60,
                )),
                read(view(
                    "port-import-b2c",
                    Phase::Done,
                    Some("wrote the tests"),
                    300,
                )),
            ],
            None,
        );
        // The pointer resting on the second agent's line, which is the third
        // item under the heading.
        screen.hover = Some(2);

        let hovered = word_modifier(&screen, size, 4, "port-import-b2c");
        assert!(!hovered.contains(Modifier::DIM), "{hovered:?}");
        assert!(!hovered.contains(Modifier::BOLD), "{hovered:?}");
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
    fn rows_the_one_the_terminal_came_back_from_wears_the_accent() {
        // Somebody attaches to an agent, reads what it is doing, and detaches
        // onto a wall of rows that all look alike. The one they were just in
        // is the row they are about to look for, so the wall says which it
        // was rather than leaving them to remember.
        let size = (60, 12);
        let mut screen = showing(
            vec![
                read(view("ask-a1b", Phase::Waiting, Some("Proceed?"), 30)),
                read(view("busy-b2c", Phase::Working, Some("Running Bash"), 3)),
                view("fix-login-c3d", Phase::Done, Some("wrote the parser"), 60),
            ],
            None,
        );
        // Which line each of them is drawn on, taken once: the mark is a
        // colour on a name and moves no row.
        let lines = painted(&screen, size);
        let at = |name: &str| {
            lines
                .iter()
                .position(|line| line.contains(name))
                .unwrap_or_else(|| panic!("{name} is not on {lines:?}")) as u16
        };
        let (asking, busy, done) = (at("ask-a1b"), at("busy-b2c"), at("fix-login-c3d"));

        // A view nobody has lent the terminal out of yet marks nothing: the
        // accent says where somebody has been, and they have been nowhere.
        assert_eq!(
            word_colour(&screen, size, busy, "busy-b2c"),
            Color::Reset,
            "nothing is marked before the first lend"
        );

        screen.lent = Some("busy-b2c".to_string());
        assert_eq!(
            word_colour(&screen, size, busy, "busy-b2c"),
            theme().accent,
            "the row the terminal came back from"
        );
        assert_eq!(
            word_colour(&screen, size, done, "fix-login-c3d"),
            Color::Reset,
            "and every other name is the terminal's own"
        );

        // A name that already has a colour keeps it. What an agent wants is
        // worth more than where the terminal has been, and a wall that said
        // both on one name would be saying neither.
        screen.lent = Some("ask-a1b".to_string());
        assert_eq!(
            word_colour(&screen, size, asking, "ask-a1b"),
            theme().waiting,
            "a row that is asking is still asking"
        );

        // And the mark is a colour rather than a second cursor, so it is drawn
        // at the strength every row off the cursor's is drawn at.
        screen.lent = Some("fix-login-c3d".to_string());
        assert_eq!(
            word_colour(&screen, size, done, "fix-login-c3d"),
            theme().accent
        );
        let marked = word_modifier(&screen, size, done, "fix-login-c3d");
        assert!(
            marked.contains(Modifier::DIM) && !marked.contains(Modifier::BOLD),
            "the row somebody came back from is still a quiet row: {marked:?}"
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
