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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::{Evidence, Verdict, View};
    use crate::store::{Meta, Phase, State};
    use crate::tmux::{PaneId, Socket};
    use crate::tui::paint::{Card, draw};
    use crate::tui::rows::Narrow;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;
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

    /// What a heading line says in front of the rule that carries it out to
    /// the edge: the label, and how many failed under it where any did.
    fn heading_of(line: &str) -> &str {
        line.split('┈').next().unwrap_or_default().trim()
    }

    /// The two agents a card is opened over, so there is a list to still be
    /// drawn behind it.
    fn a_fleet() -> Vec<View> {
        vec![
            view("ask-a1b", Phase::Waiting, None, 29),
            view("busy-b2c", Phase::Working, Some("Running Bash"), 3),
        ]
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
}
