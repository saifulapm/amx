//! The two bands above the list.
//!
//! Two kinds of thing are said up here and they are drawn apart: what is
//! happening — the counts, the badge, where the view was opened — and what the
//! *next* agent will be started with, which has not happened at all. Each has a
//! row of its own, and the second hangs off the first on a branch glyph and
//! carries the accent on every value, so nobody reads a dial as a fact about
//! the fleet.
//!
//! What the terminal is called is here too. It is not a band, but it answers
//! the same question the badge does — how many are waiting on somebody — and a
//! title counting one thing while the header counted another would be two
//! answers to one question.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::style::dim;
use super::text::{SEPARATOR, fit, said};
use crate::theme::Theme;
use crate::tui::rows::{Group, List};
use crate::tui::{Profile, Screen};

/// Below this many rows the header is what there is and nothing else. Two rows
/// of chrome over a screen that short is a third of it, and the list is what
/// the view is for.
pub(super) const SHORT: usize = 10;

/// From this many rows up there is a blank one between the header and the
/// list. The groups stand off from each other that way, and the first of them
/// has the chrome above it rather than nothing at all.
///
/// It is the first row to go on a screen running out of them, for the reason
/// [`SHORT`] is a rule: four rows of chrome over ten of terminal is most of
/// what a person opened the view to read, and a row of air is worth less than
/// a row of agents.
pub(super) const SPACED: usize = 12;

/// Fewer columns than this left for a directory and it is not on the row at
/// all: a path cut to three characters is not a path.
const SHORTEST_DIR: usize = 8;

/// How many rows the header takes at this height.
pub(super) fn header_rows(height: u16) -> u16 {
    match (height as usize) < SHORT {
        true => 1,
        false => 2,
    }
}

/// And how many stand between it and the list.
pub(super) fn space_rows(height: u16) -> u16 {
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
pub(super) fn header(screen: &Screen, area: Rect) -> Vec<Line<'static>> {
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
