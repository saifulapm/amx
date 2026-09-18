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
/// The whole band, whatever else is on the screen: a card is drawn over the
/// last rows of it rather than taking rows off it, so the rows are drawn where
/// they were drawn before it opened and none of them moves while somebody walks
/// the list with it up.
pub(super) fn agents(frame: &mut Frame, list: &List, area: Rect, moment: Moment, theme: Theme) {
    if list.is_empty() {
        let nothing = empty::nothing(list, area.width as usize);
        frame.render_widget(Paragraph::new(nothing), area);
        return;
    }

    let offset = first_drawn(list, area.height);
    let width = area.width as usize;
    let widths = grid::widths(width, list.axis(), moment.vendor, list.root_pad());
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
    /// Why each of those rows was armed, in the order `armed` is in, where the
    /// press had a reason to give. Empty where it had none.
    pub(super) why: &'a [String],
    /// Which of those rows the second press will leave where they are, because
    /// the tree behind them holds work no commit has. A row asks whether it is
    /// among them, since most presses find none.
    pub(super) held: &'a [String],
    /// Whether a heading armed them, which is what the armed rows say the
    /// press after this one would do. One arm at a time, so it is a fact about
    /// the frame rather than about each row.
    pub(super) swept: bool,
    /// The line the pointer is resting on, if it is resting on an agent's or
    /// a heading.
    pub(super) hover: Option<usize>,
    /// The agent the terminal was last lent to, where it has been lent to one.
    pub(super) lent: Option<&'a str>,
    /// Whether the rows are saying what runs them. Not a fact about the clock
    /// like the rest of these, but the same kind of thing to a row: something
    /// the person at the screen is doing to the whole list at once, handed
    /// down rather than asked for row by row.
    pub(super) vendor: bool,
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
            Under::Group(group) => heading(group, tally, at.hovered, theme),
            Under::Project(_) => path_heading(list.title(under), tally, at.hovered, width, theme),
        },
        Item::Fold(_, hidden) => Line::styled(format!("{GUTTER}… {hidden} more"), dim()),
        Item::Sub(n, hidden) => Line::styled(
            format!(
                "{GUTTER}{}… {hidden} sub",
                list.gutter(Item::Sub(n, hidden))
            ),
            dim(),
        ),
        Item::Agent(_) => match list.agent(item) {
            Some(view) => row(
                view,
                list.requests(view),
                list.gutter(item),
                widths.name.saturating_sub(grid::NEST * list.depth(item)),
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
fn heading(group: Group, tally: Tally, hovered: bool, theme: Theme) -> Line<'static> {
    // Dim like the rows under it, with the one exception the wall makes up
    // here: the group that wants a person says so in colour, which is what the
    // weight used to be spent on and reads louder than it did. Under the
    // pointer it comes up the way a hovered name does.
    let label = match group {
        Group::NeedsInput => Style::new().fg(theme.waiting),
        _ if hovered => Style::new(),
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
fn path_heading(
    title: String,
    tally: Tally,
    hovered: bool,
    width: usize,
    theme: Theme,
) -> Line<'static> {
    let failed = failures(tally);
    let path = grid::elide(&title, grid::path_room(width, failed.trim()));
    let label = match hovered {
        true => Style::new(),
        false => dim(),
    };
    Line::from(vec![
        Span::styled(format!("{path}{}", count(tally)), label),
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
#[allow(clippy::too_many_arguments)]
fn row(
    view: &View,
    prs: &[Pr],
    gutter: String,
    name_room: usize,
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
    // The name column a child gives up to the connector that indents it, so
    // the state, the age and the summary stay under its parent's.
    let name = grid::pad(&inert(rows::called(view)), name_room);
    // The pull request is not one of the design's columns, so it is paid for
    // the way the state word is: out of the summary, which is the column that
    // gives way. Name, age and count stay where they are whether or not there
    // is a forge on the machine.
    let room = widths.summary.saturating_sub(match requests {
        0 => 0,
        column => column + GAP,
    });
    let armed = moment.armed.iter().position(|id| id == view.id());
    let said = match armed {
        Some(at) => match moment.why.get(at) {
            Some(why) if moment.held.iter().any(|id| id == view.id()) => {
                format!("{HOLDS} · {why}")
            }
            Some(why) => format!("{CLEARS} · {why}"),
            None if moment.swept => AGAIN_ALL.to_string(),
            None => AGAIN.to_string(),
        },
        None => inert(first_line(view.line().unwrap_or(""))),
    };

    let asking = phase == Phase::Waiting;
    let mut spans = vec![
        Span::styled(format!("{GUTTER}{gutter}"), dim()),
        Span::styled(
            format!(
                "{} ",
                icon(
                    phase,
                    &view.verdict.evidence,
                    moment.beat,
                    view.meta.agent.is_none()
                )
            ),
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
    if widths.vendor > 0 {
        // Dim like the name beside it. What runs a row is a fact about how it
        // was started rather than about how it is going, so it is the quietest
        // thing on the line whatever state the row is in.
        spans.push(Span::styled(
            format!(
                "{}{}",
                grid::pad(&rows::vendor_words(&view.meta), widths.vendor),
                " ".repeat(GAP)
            ),
            dim(),
        ));
    }
    if widths.state > 0 {
        spans.push(Span::styled(
            format!(
                "{}{}",
                grid::pad(phase.word(), widths.state),
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
    let summary = match (armed.is_some(), asking) {
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

/// And what a row `c` armed says before the reason it was found by: the same
/// two-press sentence in the key that is actually armed.
///
/// The instruction comes first because the summary column is the one that
/// gives way: at eighty columns it holds forty-eight cells, and a branch
/// merged into main is more than that on its own. What the cut takes is the
/// end of the sentence, and the end can be the reason, which the card still
/// holds; it cannot be the half that says what the next press does.
const CLEARS: &str = "c again clears";

/// And what a row `c` found holding work no commit has says instead, which is
/// why rather than what next: the press after this one goes past it.
///
/// The verb keeps such a tree and the record that names it, so the row would
/// still be on the wall after the second press. Saying `c again clears` over
/// it promises something that will not happen — and the reason it will not is
/// the one thing worth reading, since it is work somebody has not committed.
///
/// The row is where it is said. The notice the second press leaves counts them
/// — `kept 2 holding work no commit has` — because a press that covers a wall
/// can keep more rows than one line of a footer holds.
const HOLDS: &str = "holds work no commit has";

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
pub(super) fn first_line(text: &str) -> &str {
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

/// What a row running a shell command is marked with: the prompt a person
/// types a command at.
pub(super) const COMMAND_GLYPH: &str = "$";

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
///
/// A command is asked before either of them, and in every state, because the
/// two shapes above are an agent's: they say whether there is a pane left to
/// attach to, answer or stop, and a row running `!cmd` or an `--exec` spawn
/// is none of those things. So the shape says which kind of row it is — the
/// one thing a wall mixing the two could not say at all — and the colour goes
/// on saying how it is going.
pub(super) fn icon(phase: Phase, evidence: &Evidence, beat: usize, command: bool) -> &'static str {
    match (command, evidence, phase) {
        (true, _, _) => COMMAND_GLYPH,
        (_, Evidence::LetGo, _) => ENDED,
        (_, _, Phase::Starting | Phase::Working) => pulse(beat),
        (_, _, phase) => resting(phase),
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
    use crate::tui::rows::{FOLD_AT, Narrow};
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
                role: None,
                parent: None,
                depth: 0,
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                agent: Some("claude".to_string()),
                model: None,
                effort: None,
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

    /// The same row run by a shell command rather than a vendor: the record a
    /// `!cmd` or an `--exec` spawn writes, which is one with no agent on it.
    fn command(id: &str, phase: Phase) -> View {
        let mut view = view(id, phase, Some("cargo build"), 5);
        view.meta.agent = None;
        view
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

    /// The same reading, a child of the agent with this id.
    fn child_of(mut view: View, parent: &str) -> View {
        view.meta.parent = Some(parent.to_string());
        view.meta.depth = 1;
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

    /// The same, drawn through a screen of its own: what a person reads at
    /// this size.
    fn settled(views: Vec<View>, size: (u16, u16)) -> Vec<String> {
        painted(&showing(views, None), size)
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
    fn rows_draw_a_parent_and_its_children_as_one_family() {
        // The mockup in the plan: a working parent, a finished child and one
        // still reading, the children hung under the parent on connectors,
        // newest first, and every name and summary standing at the same
        // column.
        let mut scout = child_of(
            view("scout-b2c", Phase::Done, Some("find auth"), 12),
            "parent-a1b",
        );
        scout.meta.created = 10;
        let mut review = child_of(
            view("review-c3d", Phase::Working, Some("reading store.rs"), 5),
            "parent-a1b",
        );
        review.meta.created = 20;
        let lines = drawn(
            vec![
                view("parent-a1b", Phase::Working, Some("running tests"), 2),
                scout,
                review,
            ],
            None,
            (100, 12),
        );
        let family: Vec<&String> = lines
            .iter()
            .filter(|line| {
                ["parent-a1b", "scout-b2c", "review-c3d"]
                    .iter()
                    .any(|id| line.contains(id))
            })
            .collect();
        assert_eq!(family.len(), 3, "one row each: {lines:#?}");
        assert!(
            family[0].starts_with("   · parent-a1b"),
            "a root keeps its column, whatever the family's depth: {:?}",
            family[0]
        );
        assert!(
            family[1].starts_with("   ├─· review-c3d"),
            "the newest child opens the pair, its connector in the column of \
             the glyph it hangs from: {:?}",
            family[1]
        );
        assert!(
            family[2].starts_with("   └─∙ scout-b2c"),
            "the oldest closes the pair under the same column: {:?}",
            family[2]
        );
        let column = |line: &str, word: &str| {
            let at = line.find(word).expect("the word on the row");
            // The cells before the word, not the bytes: the connectors are
            // multi-byte and a byte offset would say the columns parted.
            line[..at].chars().count()
        };
        assert_eq!(
            column(family[0], "running"),
            column(family[1], "reading"),
            "a child pays for its connector out of its own name, so the \
             summaries still stand at one column: {family:#?}"
        );
        assert_eq!(
            column(family[1], "reading"),
            column(family[2], "find"),
            "including the last child's"
        );
        assert_eq!(
            column(family[0], "parent-a1b"),
            column(family[1], "review-c3d") - 2,
            "and the names indent with the connector"
        );
    }

    #[test]
    fn rows_stand_a_root_one_level_in_however_deep_the_family_goes() {
        // A grandchild takes the family to two levels. A root still stands one
        // level in: the wall spends two cells on there being a family at all,
        // not two per level, so one deep family does not push every root on
        // the screen across.
        let mut helper = child_of(
            view("helper-f6g", Phase::Done, Some("ran the suite"), 12),
            "parent-a1b",
        );
        helper.meta.created = 20;
        let mut scout = child_of(
            view("scout-b2c", Phase::Working, Some("reading store.rs"), 5),
            "helper-f6g",
        );
        scout.meta.created = 30;
        scout.meta.depth = 2;
        let lines = drawn(
            vec![
                view("parent-a1b", Phase::Working, Some("running tests"), 2),
                helper,
                scout,
            ],
            None,
            (100, 12),
        );
        let family: Vec<&String> = lines
            .iter()
            .filter(|line| {
                ["parent-a1b", "helper-f6g", "scout-b2c"]
                    .iter()
                    .any(|id| line.contains(id))
            })
            .collect();
        assert_eq!(family.len(), 3, "one row each: {lines:#?}");
        assert!(
            family[0].starts_with("   · parent-a1b"),
            "the root stands one level in: {:?}",
            family[0]
        );
        assert!(
            family[1].starts_with("   └─∙ helper-f6g"),
            "its child a level under it: {:?}",
            family[1]
        );
        assert!(
            family[2].starts_with("     └─· scout-b2c"),
            "and the grandchild a level under that: {:?}",
            family[2]
        );
        let column = |line: &str, word: &str| {
            let at = line.find(word).expect("the word on the row");
            line[..at].chars().count()
        };
        assert_eq!(
            column(family[0], "running"),
            column(family[2], "reading"),
            "and every level is still paid for by the name, so the summaries \
             stand at one column: {family:#?}"
        );
    }

    #[test]
    fn a_child_pays_for_its_connector_out_of_its_own_name() {
        // The connector indents a child, and the two cells it takes come out
        // of the child's name rather than out of the wall: a name too long for
        // what is left is cut, so the state, the age and the summary still
        // stand under the parent's.
        let mut loud = child_of(
            view(
                "a-child-name-far-too-long-1a2b",
                Phase::Done,
                Some("find auth"),
                12,
            ),
            "parent-a1b",
        );
        loud.meta.created = 10;
        let lines = drawn(
            vec![
                view("parent-a1b", Phase::Working, Some("running tests"), 2),
                loud,
            ],
            None,
            (100, 12),
        );
        let root = lines
            .iter()
            .find(|line| line.contains("parent-a1b"))
            .unwrap_or_else(|| panic!("the parent's row: {lines:#?}"));
        let child = lines
            .iter()
            .find(|line| line.contains("└─"))
            .unwrap_or_else(|| panic!("the child's row: {lines:#?}"));
        let column = |line: &str, word: &str| {
            line[..line
                .find(word)
                .unwrap_or_else(|| panic!("{word} on {line:?}"))]
                .chars()
                .count()
        };
        assert_eq!(
            column(root, "running"),
            column(child, "find"),
            "the summaries stand at one column however long the child's name"
        );
        assert!(
            !child.contains("a-child-name-far-too-long-1a2b"),
            "the name was cut to pay for the connector: {child:?}"
        );
    }

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
                icon(phase, &Evidence::Hooks, 1, false),
                pulse(1),
                "{phase} is a turn under way"
            );
        }
        for phase in EVERY
            .iter()
            .filter(|phase| !matches!(phase, Phase::Starting | Phase::Working))
        {
            assert_eq!(
                icon(*phase, &Evidence::Hooks, 1, false),
                resting(*phase),
                "and {phase} stands still"
            );
        }
    }

    #[test]
    fn glyphs_wear_a_dollar_on_a_row_running_a_command() {
        // Every state and both evidences, because a command's row says what
        // it is and not how far along it is: the pulse and the dot are an
        // agent's, and what they carry — whether there is still a pane to
        // attach to, answer or stop — is not a question anybody asks of a
        // shell command.
        for phase in EVERY {
            for evidence in [Evidence::Hooks, Evidence::Screen, Evidence::LetGo] {
                assert_eq!(
                    icon(phase, &evidence, 1, true),
                    COMMAND_GLYPH,
                    "{phase} on {evidence:?} is still a command"
                );
            }
        }
    }

    #[test]
    fn glyphs_leave_a_command_row_the_colour_too() {
        // The mark on the one row a view of one command draws.
        let painted = |phase| {
            let screen = showing(vec![command("build-a1b", phase)], None);
            mark(&screen, (60, 8), 2)
        };
        let plain = Modifier::empty();

        // The shape is the kind of row and the colour is how it went, which is
        // the division the wall already draws the glyph by.
        assert_eq!(
            painted(Phase::Working),
            (COMMAND_GLYPH.into(), Color::Reset, plain),
            "a command still running has nothing to say about how it went"
        );
        assert_eq!(
            painted(Phase::Done),
            (COMMAND_GLYPH.into(), theme().done, plain)
        );
        assert_eq!(
            painted(Phase::Failed),
            (COMMAND_GLYPH.into(), theme().failed, plain)
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
        // agent whose turn is over — still at its prompt or gone — says how it
        // went in green, and the shape says whether there is still a process
        // to reach. One amx cannot account for is neither: it stands still in
        // the terminal's own, which is the one thing left to tell it from a
        // row that is asking.
        assert_eq!(
            painted(Phase::Starting),
            (pulse(0).into(), Color::Reset, plain)
        );
        assert_eq!(
            painted(Phase::Working),
            (pulse(0).into(), Color::Reset, plain)
        );
        assert_eq!(painted(Phase::Idle), ("✻".into(), theme().done, plain));
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
            cleared: false,
            why: Vec::new(),
            held: Vec::new(),
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
    fn view_says_on_a_row_c_armed_why_its_work_has_landed() {
        let size = (72, 8);
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
            ids: vec!["fix-login-a1b".to_string()],
            swept: false,
            cleared: true,
            why: vec!["#12 merged".to_string()],
            held: Vec::new(),
            at: Instant::now(),
        });
        let drawn = painted(&screen, size);
        assert!(
            drawn[2].contains("c again clears · #12 merged"),
            "the row says why it is on the list and what the next press does: {:?}",
            drawn[2]
        );
        assert!(
            !drawn[2].contains("wrote the parser"),
            "in place of the summary, like every other armed row: {:?}",
            drawn[2]
        );
        assert!(
            !drawn[2].contains("ctrl+x"),
            "and it names the key that armed it, not the other one: {:?}",
            drawn[2]
        );
        assert_eq!(
            word_colour(&screen, size, 2, "#12 merged"),
            theme().waiting,
            "in the colour an armed row already takes"
        );
        assert!(
            drawn[3].contains("wrote the tests"),
            "the rows the sweep did not find say what they always said: {:?}",
            drawn[3]
        );
    }

    #[test]
    fn view_says_on_a_row_c_found_holding_work_that_the_press_after_keeps_it() {
        let size = (72, 8);
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
            swept: false,
            cleared: true,
            why: vec!["#12 merged".to_string(), "#13 merged".to_string()],
            held: vec!["fix-login-a1b".to_string()],
            at: Instant::now(),
        });
        let drawn = painted(&screen, size);
        assert!(
            drawn[2].contains("holds work no commit has · #12 merged"),
            "the row says what the press after this one will not do to it: {:?}",
            drawn[2]
        );
        assert!(
            !drawn[2].contains("c again clears"),
            "and does not promise the press that clears, which will pass it by: {:?}",
            drawn[2]
        );
        assert_eq!(
            word_colour(&screen, size, 2, "holds work no commit has"),
            theme().waiting,
            "in the colour every armed row wears"
        );
        assert!(
            drawn[3].contains("c again clears · #13 merged"),
            "and the rows with nothing uncommitted in them say what they said: {:?}",
            drawn[3]
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
            cleared: false,
            why: Vec::new(),
            held: Vec::new(),
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
        // A working agent and two endings more than the fold holds: the fold's
        // worth are drawn and the fold stands under them saying what it is
        // holding back.
        let fleet = || {
            let mut views = vec![view("busy-b2c", Phase::Working, Some("Running Bash"), 3)];
            views.extend(
                (0..FOLD_AT + 2)
                    .map(|n| view(&format!("done-{n:02}"), Phase::Done, Some("did it"), 60)),
            );
            views
        };

        let height = (FOLD_AT + 14) as u16;
        let tall = settled(fleet(), (40, height));
        assert_eq!(heading_of(&tall[6]), "Completed");
        assert_eq!(tall.iter().filter(|l| l.contains("done-")).count(), FOLD_AT);
        assert!(
            tall[FOLD_AT + 7].contains("… 2 more"),
            "the fold stands on the row under them: {tall:?}"
        );

        // Twice the screen, the same rows and the same count. What folds is
        // the length of the group, and the window has no say in it.
        let taller = settled(fleet(), (40, height * 2));
        assert_eq!(
            taller.iter().filter(|l| l.contains("done-")).count(),
            FOLD_AT
        );
        assert!(taller[FOLD_AT + 7].contains("… 2 more"), "{taller:?}");
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
    fn rows_a_hovered_heading_comes_up_too() {
        let size = (60, 10);
        let mut screen = showing(
            vec![read(view(
                "fix-login-a1b",
                Phase::Done,
                Some("wrote the parser"),
                60,
            ))],
            None,
        );
        assert!(
            word_modifier(&screen, size, 2, "Completed").contains(Modifier::DIM),
            "a heading is dim until the pointer rests on it"
        );
        screen.hover = Some(0);
        let hovered = word_modifier(&screen, size, 2, "Completed");
        assert!(!hovered.contains(Modifier::DIM), "{hovered:?}");
        assert_eq!(
            behind(&screen, size, 2),
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
        //
        // A row taller than the rest of these: the three rows and the two
        // headings between them want every row the list has once the keys
        // have taken their own and the blank one over them.
        let size = (60, 13);
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

    /// The three kinds of row the vendor column has anything to say about: a
    /// spawn that turned both dials, one that turned neither, and a shell
    /// command, which runs no vendor at all.
    fn three_kinds() -> Vec<View> {
        let mut dialled = view("fix-login-a1b", Phase::Working, Some("Running Bash"), 3);
        dialled.meta.model = Some("opus".to_string());
        dialled.meta.effort = Some("high".to_string());
        vec![
            dialled,
            view(
                "port-import-b2c",
                Phase::Working,
                Some("Read src/lib.rs"),
                5,
            ),
            command("build-c3d", Phase::Working),
        ]
    }

    #[test]
    fn vendor_column_says_what_each_kind_of_row_runs() {
        let mut screen = showing(three_kinds(), None);
        screen.vendor = true;
        let drawn = painted(&screen, (100, 10));
        let row = |id: &str| {
            drawn
                .iter()
                .find(|line| line.contains(id))
                .unwrap_or_else(|| panic!("no row for {id}:\n{drawn:?}"))
                .clone()
        };

        assert!(
            row("fix-login-a1b").contains("claude opus high"),
            "the vendor and both dials the spawn turned: {:?}",
            row("fix-login-a1b")
        );
        assert!(
            row("port-import-b2c").contains("claude"),
            "{:?}",
            row("port-import-b2c")
        );
        assert!(
            !row("port-import-b2c").contains("opus"),
            "a dial nobody turned is the vendor's own, and amx does not guess it: {:?}",
            row("port-import-b2c")
        );
        assert!(
            row("build-c3d").contains("sh"),
            "a command row runs no vendor: {:?}",
            row("build-c3d")
        );
    }

    #[test]
    fn vendor_column_is_paid_for_by_the_summary_so_the_name_and_the_age_stay() {
        let size = (100, 10);
        let quiet = painted(&showing(three_kinds(), None), size);
        let mut screen = showing(three_kinds(), None);
        screen.vendor = true;
        let loud = painted(&screen, size);

        for id in ["fix-login-a1b", "port-import-b2c", "build-c3d"] {
            let (before, after) = (
                quiet
                    .iter()
                    .find(|line| line.contains(id))
                    .expect("the row"),
                loud.iter().find(|line| line.contains(id)).expect("the row"),
            );
            assert_eq!(
                before.find(id),
                after.find(id),
                "the name column does not move for {id}:\n{before:?}\n{after:?}"
            );
            // The age is right-aligned at the edge, which is where the line
            // ends once the trailing spaces are off it.
            let age = |line: &str| line.chars().rev().take(2).collect::<String>();
            assert_eq!(
                age(before),
                age(after),
                "nor does the age for {id}:\n{before:?}\n{after:?}"
            );
        }
        // And off, no row says any of it. The header's dial row says `claude`
        // about the next agent whatever the wall is doing, so this is asked of
        // the rows rather than of the screen.
        for id in ["fix-login-a1b", "port-import-b2c", "build-c3d"] {
            let row = quiet
                .iter()
                .find(|line| line.contains(id))
                .expect("the row");
            assert!(
                !row.contains("claude") && !row.contains(" sh "),
                "the column is not there until somebody asks: {row:?}"
            );
        }
    }

    #[test]
    fn vendor_column_stands_between_the_name_and_the_state_word() {
        let mut screen = by_project(vec![at(
            {
                let mut view = view("fix-login-a1b", Phase::Working, Some("Running Bash"), 3);
                view.meta.model = Some("opus".to_string());
                view
            },
            "/src/api",
        )]);
        screen.vendor = true;
        let drawn = painted(&screen, (120, 10));
        let row = drawn
            .iter()
            .find(|line| line.contains("fix-login-a1b"))
            .expect("the row");

        let at_of = |word: &str| {
            row.find(word)
                .unwrap_or_else(|| panic!("{word} in {row:?}"))
        };
        assert!(
            at_of("fix-login-a1b") < at_of("claude opus"),
            "what runs it comes after what it is called: {row:?}"
        );
        assert!(
            at_of("claude opus") < at_of("working"),
            "and before the state word the dir axis adds: {row:?}"
        );
        assert!(
            at_of("working") < at_of("Running Bash"),
            "which still stands in front of the summary: {row:?}"
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
