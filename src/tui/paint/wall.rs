//! The list of agents: headings, rows and folds.
//!
//! - Each row is exactly one line, laid out on the fixed column widths from
//!   [`grid`], so columns do not shift as the fleet changes.
//! - A row's state is its glyph: the shape says whether a process is still
//!   there, the colour which state it is in, and a pulse that a turn is
//!   running.
//! - The wall uses no bold. Names are dim except under the cursor or the
//!   pointer; the row the terminal was last lent to wears the accent.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::cell::Cell;
use std::sync::OnceLock;

use super::empty;
use super::style::{colour, dim, name_colour, request_colour};
use super::text::{inert, width_of};
use crate::derive::{self, Evidence, View};
use crate::pr::Pr;
use crate::store::Phase;
use crate::theme::Theme;
use crate::tui::grid::{self, Widths};
use crate::tui::rows::{self, Group, Item, List, Tally, Under};

/// Draw the list into `area`, starting at item `offset`.
///
/// Always the whole band; a card is drawn over its foot afterwards.
pub(super) fn agents(
    frame: &mut Frame,
    list: &List,
    area: Rect,
    offset: usize,
    moment: Moment,
    theme: Theme,
) {
    if list.is_empty() {
        let nothing = empty::nothing(list, area.width as usize);
        frame.render_widget(Paragraph::new(nothing), area);
        return;
    }

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

/// The list's scroll position.
///
/// The mouse wheel moves `top` without moving the cursor; a cursor move sets
/// `follow` so the next frame scrolls just enough to show it. `Cell`s because
/// only the paint knows the band's height and so can clamp.
#[derive(Default)]
pub struct WallScroll {
    /// The item on the band's first row, clamped every frame.
    pub top: Cell<usize>,
    /// Set when the cursor moved; cleared by the frame that scrolls to it.
    pub follow: Cell<bool>,
}

/// The first item drawn in a band `visible` rows tall, after clamping `top`
/// to the last page and scrolling the least needed to show the cursor when
/// [`WallScroll::follow`] is set.
pub(super) fn first_drawn(list: &List, visible: u16, scroll: &WallScroll) -> usize {
    let visible = visible.max(1) as usize;
    let last = list.items().len().saturating_sub(visible);
    let mut top = scroll.top.get().min(last);
    if scroll.follow.replace(false) {
        let cursor = list.cursor();
        top = top.clamp(cursor.saturating_sub(visible - 1), cursor.min(last));
    }
    scroll.top.set(top);
    top
}

/// Per-frame view state the rows need that is not part of any agent's record.
#[derive(Clone, Copy)]
pub(super) struct Moment<'a> {
    /// The frame of the working pulse.
    pub(super) beat: usize,
    /// Ids of the rows a press has armed.
    pub(super) armed: &'a [String],
    /// Why each armed row was armed, parallel to `armed`; empty if the press
    /// gave no reasons.
    pub(super) why: &'a [String],
    /// Armed rows the second press will keep because their worktree holds
    /// uncommitted work.
    pub(super) held: &'a [String],
    /// Whether a heading armed the rows (so the next press stops and forgets
    /// the whole group).
    pub(super) swept: bool,
    /// The item under the pointer, if it is an agent or a heading.
    pub(super) hover: Option<usize>,
    /// The agent the terminal was last lent to.
    pub(super) lent: Option<&'a str>,
    /// Whether the vendor column is shown.
    pub(super) vendor: bool,
}

/// How the cursor, the pointer and the last lend relate to one line.
#[derive(Clone, Copy, Default)]
struct At {
    selected: bool,
    hovered: bool,
    lent: bool,
}

/// One line of the list: a heading, fold, agent row or blank.
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

/// The cursor line: `line` padded to `width` on the theme's cursor background.
///
/// A full-width background rather than reverse video, so a short heading gets
/// the same bar as a row. The default colour is claude's own selection colour
/// (from the 2.1.237 bundle).
fn barred(line: Line<'static>, width: usize, theme: Theme) -> Line<'static> {
    let said = line.width();
    let mut line = line;
    if said < width {
        line.spans.push(Span::raw(" ".repeat(width - said)));
    }
    line.style(Style::new().bg(theme.cursor))
}

/// A state group's heading: its title, the member count when the group is
/// shut, and the failure count when there are failures.
fn heading(group: Group, tally: Tally, hovered: bool, theme: Theme) -> Line<'static> {
    // Dim, except the waiting group, which wears the waiting colour, and a
    // hovered heading, which comes up to full strength.
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

/// A project heading: the path, laid out like a group heading, with the
/// per-state counts of its rows right-aligned.
///
/// A long path loses its middle ([`grid::elide`]), since the last segments
/// tell worktrees of one project apart. The counts use the header's words.
fn path_heading(
    title: String,
    tally: Tally,
    hovered: bool,
    width: usize,
    theme: Theme,
) -> Line<'static> {
    let failed = failures(tally);
    let doing = doing(tally);
    let spent = match doing.is_empty() {
        true => 0,
        false => width_of(&doing) + APART,
    };
    let path = grid::elide(
        &title,
        grid::path_room(width, failed.trim()).saturating_sub(spent),
    );
    let label = match hovered {
        true => Style::new(),
        false => dim(),
    };
    let said = format!("{path}{}", count(tally));
    let stood = width_of(&said) + width_of(&failed) + width_of(&doing);
    Line::from(vec![
        Span::styled(said, label),
        Span::styled(failed, Style::new().fg(theme.failed)),
        Span::raw(" ".repeat(width.saturating_sub(stood))),
        Span::styled(doing, dim()),
    ])
}

/// Per-state counts of a heading's rows, in the header's words (which are also
/// the narrowing words). Empty groups are left out.
fn doing(tally: Tally) -> String {
    tally
        .doing()
        .into_iter()
        .map(|(group, count)| format!("{count} {}", group.state()))
        .collect::<Vec<String>>()
        .join(&" ".repeat(APART))
}

/// Gap between counts, as in the header.
const APART: usize = 3;

/// The member count, shown only when the group is shut.
fn count(tally: Tally) -> String {
    match tally.shut {
        true => format!(" {}", tally.members),
        false => String::new(),
    }
}

/// The failure count, shown whether the group is open or shut.
fn failures(tally: Tally) -> String {
    match tally.failures {
        0 => String::new(),
        failures => format!(" · {failures} failed"),
    }
}

/// An agent's row: indent, glyph, name, then the optional vendor, state and
/// pull request columns, the summary, and the worked time right-aligned.
///
/// Columns come from the [`grid`] widths for the screen, not from the fleet.
/// Everything is dim except: the name under the cursor or pointer (full
/// strength), a waiting agent's question (full strength), waiting and failed
/// names (their colour, see [`name_colour`]), the pull request (its standing's
/// colour), and the state word (see [`state_colour`]).
///
/// An armed row replaces its summary with what the next press will do, in
/// the waiting colour, so no column moves.
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
    // Worked time, not age: an idle agent's age keeps climbing. Formatted by
    // `derive` so `ls` and the view agree.
    let worked = derive::in_words(view.verdict.worked);
    // A user can rename an agent, so the name is made inert. `name_room` is
    // already short by a child's connector indent.
    let name = grid::pad(&inert(rows::called(view)), name_room);
    // The pull request column comes out of the summary, so no other column
    // moves when there is a forge.
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
            // Full strength under the cursor or the pointer; a hover has no
            // other mark.
            name_colour(theme, phase, at.selected || at.hovered, at.lent),
        ),
    ];
    if widths.vendor > 0 {
        // Always dim, whatever the state.
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
        // Only the branch's first (live) request; the card lists them all.
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

/// The state word's style (project axis only): the phase's colour once it has
/// ended, dim while it is starting or working.
fn state_colour(theme: Theme, phase: Phase) -> Style {
    match phase {
        Phase::Starting | Phase::Working => dim(),
        phase => colour(theme, phase),
    }
}

/// An armed row's summary after `ctrl+x`. Same words as claude's agent view.
const AGAIN: &str = "ctrl+x again forgets";

/// An armed row's summary when `ctrl+x` was pressed on its heading: the next
/// press stops every live agent in the group and forgets them all.
const AGAIN_ALL: &str = "ctrl+x again stops and forgets";

/// An armed row's summary after `c`, before the reason. The instruction comes
/// first so a narrow summary column cuts the reason, not the instruction.
const CLEARS: &str = "c again clears";

/// The summary of a row `c` found with uncommitted work in its worktree. The
/// second press keeps such a row, so it must not say `c again clears`.
const HOLDS: &str = "holds work no commit has";

/// Width of the pull request column: the widest label on the list, or zero
/// when no agent has a request.
fn request_column(list: &List) -> usize {
    list.items()
        .iter()
        .filter_map(|item| list.agent(*item))
        .filter_map(|view| list.requests(view).first())
        .map(|pr| pr.label().chars().count())
        .max()
        .unwrap_or(0)
}

/// Gap between two columns of a row.
const GAP: usize = 2;

/// The first line of `text`, trimmed.
pub(super) fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim()
}

/// A row's indent under its heading, as in claude's own agent view.
const GUTTER: &str = " ";

/// claude's pulse glyphs for this `$TERM` (from the 2.1.237 bundle). Ghostty
/// gets an eight-spoked asterisk in place of the plain one.
pub(super) fn set_for(term: &str) -> [&'static str; 6] {
    match term {
        "xterm-ghostty" => ["·", "✢", "✳", "✶", "✻", "✻"],
        _ => ["·", "✢", "*", "✶", "✻", "✽"],
    }
}

/// [`set_for`] this terminal, read once.
pub(super) fn set() -> [&'static str; 6] {
    static SET: OnceLock<[&'static str; 6]> = OnceLock::new();
    *SET.get_or_init(|| set_for(std::env::var("TERM").unwrap_or_default().as_str()))
}

/// The glyph of [`set`] a live row rests on, and the pulse's largest frame.
pub(super) const LIVE: usize = 4;

/// The working glyph for this frame: [`set`] played forwards then backwards,
/// twelve frames, as claude's own working mark does.
pub(super) fn pulse(beat: usize) -> &'static str {
    let set = set();
    let frames = set.len() * 2;
    let at = beat % frames;
    // The second half mirrors the first.
    set[at.min(frames - 1 - at)]
}

/// The glyph of a row whose process has gone.
const ENDED: &str = "∙";

/// The glyph of a row running a shell command.
pub(super) const COMMAND_GLYPH: &str = "$";

/// The still glyph for a state: [`set`]'s [`LIVE`] glyph while a process is
/// there to attach to, [`ENDED`] once it is gone.
///
/// Only two shapes: the colour carries the state. The live glyph is taken from
/// the pulse, so a row that stops working settles on a glyph it already
/// showed.
pub(super) fn resting(phase: Phase) -> &'static str {
    match phase {
        Phase::Waiting | Phase::Starting | Phase::Working | Phase::Idle | Phase::Unknown => {
            set()[LIVE]
        }
        Phase::Done | Phase::Failed | Phase::Stopped => ENDED,
    }
}

/// A row's glyph this frame.
///
/// In order: a shell command (`!cmd`, `--exec`) is always [`COMMAND_GLYPH`];
/// an agent amx let go is [`ENDED`], since its record keeps the old phase
/// (see [`crate::derive::Evidence::LetGo`]); starting and working pulse;
/// everything else is [`resting`]. The colour stays the state's either way.
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
    use crate::tui::paint::fixtures::{
        a_fleet, cells, command, drawn, heading_of, on_a_branch, over_the_forge, painted, showing,
        theme, view,
    };
    use crate::tui::paint::text::fit;
    use crate::tui::rows::{FOLD_AT, Narrow};
    use crate::tui::{Arm, Screen};
    use ratatui::style::{Color, Modifier};
    use std::path::PathBuf;
    use std::time::Instant;

    /// Every state, so a table over them cannot miss one.
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

    /// The same reading, marked as seen. This only changes where the row sorts
    /// against the completed fold ([`rows`]).
    fn read(mut view: View) -> View {
        view.state.seen = view.state.last_event.max(view.state.since);
        view
    }

    /// The same reading, in another directory.
    fn at(mut view: View, dir: &str) -> View {
        view.meta.dir = PathBuf::from(dir);
        view
    }

    /// The same reading, as a child of `parent`.
    fn child_of(mut view: View, parent: &str) -> View {
        view.meta.parent = Some(parent.to_string());
        view.meta.depth = 1;
        view
    }

    /// The view on the project axis.
    fn by_project(views: Vec<View>) -> Screen {
        let mut screen = Screen::default();
        screen.list.turn();
        screen.list.show(views);
        screen
    }

    /// The glyph cell of a row: symbol, colour and modifiers.
    fn mark(screen: &Screen, size: (u16, u16), row: u16) -> (String, Color, Modifier) {
        let cell = cells(screen, size)[(1, row)].clone();
        (cell.symbol().to_string(), cell.fg, cell.modifier)
    }

    /// The rows drawn for these readings, with no card.
    fn settled(views: Vec<View>, size: (u16, u16)) -> Vec<String> {
        painted(&showing(views, None), size)
    }

    /// The background colour of every cell in a row.
    fn behind(screen: &Screen, size: (u16, u16), row: u16) -> Vec<Color> {
        let buffer = cells(screen, size);
        (0..size.0).map(|at| buffer[(at, row)].bg).collect()
    }

    /// The foreground colour of `word`'s first cell on a row.
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

    /// The modifiers of `word`'s first cell on a row.
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

    /// Room for the header, the spacing rows and a group or two.
    const WALL: (u16, u16) = (80, 12);

    #[test]
    fn rows_draw_a_parent_and_its_children_as_one_family() {
        // A working parent with a finished child and a working one: children
        // hang on connectors, newest first, and the summaries line up.
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
            family[0].starts_with(" · parent-a1b"),
            "a root keeps the wall's own column, whatever the family: {:?}",
            family[0]
        );
        assert!(
            family[1].starts_with(" ├─· review-c3d"),
            "the newest child opens the pair, its connector in the column of \
             the glyph it hangs from: {:?}",
            family[1]
        );
        assert!(
            family[2].starts_with(" └─∙ scout-b2c"),
            "the oldest closes the pair under the same column: {:?}",
            family[2]
        );
        let column = |line: &str, word: &str| {
            let at = line.find(word).expect("the word on the row");
            // Chars, not bytes: the connectors are multi-byte.
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
    fn rows_stand_a_root_at_the_glyph_column_however_deep_the_family_goes() {
        // Two levels deep. Roots stay at the glyph column, so a sub-agent
        // spawning never shifts the other rows.
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
        let alone = view("other-d4e", Phase::Working, Some("nesting"), 1);
        let lines = drawn(
            vec![
                view("parent-a1b", Phase::Working, Some("running tests"), 2),
                helper,
                scout,
                alone.clone(),
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
            family[0].starts_with(" · parent-a1b"),
            "the root stands at the glyph column: {:?}",
            family[0]
        );
        assert!(
            family[1].starts_with(" └─∙ helper-f6g"),
            "its child a level under it: {:?}",
            family[1]
        );
        assert!(
            family[2].starts_with("   └─· scout-b2c"),
            "and the grandchild a level under that: {:?}",
            family[2]
        );
        let other = |lines: &[String]| {
            lines
                .iter()
                .find(|line| line.contains("other-d4e"))
                .unwrap_or_else(|| panic!("the other root: {lines:#?}"))
                .clone()
        };
        assert_eq!(
            other(&lines),
            other(&drawn(vec![alone], None, (100, 12))),
            "and the root with nothing under it is drawn the same row whether \
             or not the family is on the wall"
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
        // The connector's two cells come out of the child's name column, so
        // a long name is cut and the other columns stay aligned.
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
        // Two shapes over eight states; the colour tells the states apart.
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

        // Starting pulses too: it is the start of a turn.
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
        // In every state and for every kind of evidence.
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
        let painted = |phase| {
            let screen = showing(vec![command("build-a1b", phase)], None);
            mark(&screen, (60, 8), 2)
        };
        let plain = Modifier::empty();

        // The shape says it is a command; the colour still says the state.
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
        let painted = |phase| {
            let screen = showing(vec![view("agent-a1b", phase, Some("said"), 5)], None);
            mark(&screen, (60, 8), 2)
        };
        let plain = Modifier::empty();

        // Colour only, never weight.
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

        // A live turn keeps the terminal's colour and pulses. Idle wears the
        // done colour. Unknown stands still in the terminal's colour, unlike a
        // waiting row.
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
        // The same idle agent at its prompt and after amx let its pane go.
        // The record is the same for both; only the glyph differs.
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
            heading: None,
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
            heading: None,
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
            heading: None,
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
            heading: None,
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

        assert!(screen[2].starts_with("/src/api"), "{screen:?}");
        assert!(screen[3].contains("ask-a1b"), "{:?}", screen[3]);
        assert!(
            screen[3].contains("waiting"),
            "the heading is a place, so the row says the state: {:?}",
            screen[3]
        );
        assert!(screen[4].contains("done"), "{:?}", screen[4]);
        assert_eq!(screen[5], "", "the next project stands off from this one");
        assert!(screen[6].starts_with("/src/web"), "{screen:?}");

        // State words line up in one column (counted in chars, not bytes).
        let column = |line: &str, word: &str| {
            let at = line.find(word).expect("the state on the row");
            line[..at].chars().count()
        };
        assert_eq!(column(&screen[3], "waiting"), column(&screen[4], "done"));
    }

    #[test]
    fn a_path_heading_says_at_its_far_edge_what_the_rows_under_it_are_doing() {
        let wide = (70, 12);
        let screen = painted(
            &by_project(vec![
                at(view("ask-a1b", Phase::Waiting, None, 30), "/src/api"),
                at(view("busy-b2c", Phase::Working, None, 3), "/src/api"),
                at(
                    view("fix-login-c3d", Phase::Done, Some("fixed it"), 30),
                    "/src/api",
                ),
                at(
                    view("done-d4e", Phase::Done, Some("shipped it"), 30),
                    "/src/api",
                ),
            ]),
            wide,
        );

        // In the header's words, which `s:` also takes.
        let heading = |screen: &[String]| {
            screen
                .iter()
                .find(|line| line.starts_with("/src/api"))
                .unwrap_or_else(|| panic!("no heading in: {screen:?}"))
                .clone()
        };
        let said = heading(&screen);
        assert!(
            said.ends_with("1 waiting   1 working   2 done"),
            "what is under it stands at the far edge, loudest first: {said:?}"
        );
        assert_eq!(
            said.chars().count(),
            wide.0 as usize,
            "run out to the edge of the wall, where the header's own counts \
             stand: {said:?}"
        );

        // Empty groups are left out, not shown as zero.
        for word in ["pinned", "review", "asleep"] {
            assert!(!said.contains(word), "{word}: {said:?}");
        }

        // Children are counted, not only top-level agents.
        let family = painted(
            &by_project(vec![
                at(
                    view("parent-a1b", Phase::Done, Some("done"), 30),
                    "/src/api",
                ),
                at(
                    child_of(view("scout-b2c", Phase::Working, None, 3), "parent-a1b"),
                    "/src/api",
                ),
            ]),
            wide,
        );
        let said = heading(&family);
        assert!(said.ends_with("1 working   1 done"), "{said:?}");
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

        // The cursor opens on the first agent.
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
        // No rule after the title.
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
        // A blank row above every heading, the first one included.
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
        // Headings are dim with no bold; only the waiting group has a colour.
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
        // Dim throughout, with the count only when shut.
        let size = (60, 10);
        let mut screen = by_project(vec![
            at(view("ask-a1b", Phase::Waiting, None, 30), "/src/api"),
            at(
                view("broke-b2c", Phase::Failed, Some("could not"), 60),
                "/src/api",
            ),
        ]);

        assert_eq!(
            painted(&screen, size)[2],
            "/src/api · 1 failed                       1 waiting   1 done"
        );
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
        assert_eq!(
            painted(&screen, size)[2],
            "/src/api 2 · 1 failed                     1 waiting   1 done"
        );
    }

    #[test]
    fn view_shows_the_fold_and_what_it_is_holding_back() {
        // Two more finished agents than the fold shows.
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

        // The fold depends on the group's length, not the screen's height.
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

        // Only the name under the cursor is at full strength.
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

        // The glyph's colour carries the state.
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

        // Seen and unseen rows are drawn alike; being seen only affects the
        // fold's ordering.
        for (row, name) in [(6, "fix-login-a1b"), (7, "port-import-b2c")] {
            let painted = word_modifier(&screen, size, row, name);
            assert!(
                painted.contains(Modifier::DIM) && !painted.contains(Modifier::BOLD),
                "{name} is as quiet as the other: {painted:?}"
            );
        }

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
        // Item 2 is the second agent (item 0 is the heading).
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
        // Tall enough for three groups.
        let size = (60, 13);
        let mut screen = showing(
            vec![
                read(view("ask-a1b", Phase::Waiting, Some("Proceed?"), 30)),
                read(view("busy-b2c", Phase::Working, Some("Running Bash"), 3)),
                view("fix-login-c3d", Phase::Done, Some("wrote the parser"), 60),
            ],
            None,
        );
        // Row positions, taken once: the accent moves no row.
        let lines = painted(&screen, size);
        let at = |name: &str| {
            lines
                .iter()
                .position(|line| line.contains(name))
                .unwrap_or_else(|| panic!("{name} is not on {lines:?}")) as u16
        };
        let (asking, busy, done) = (at("ask-a1b"), at("busy-b2c"), at("fix-login-c3d"));

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

        // A name that already has a state colour keeps it.
        screen.lent = Some("ask-a1b".to_string());
        assert_eq!(
            word_colour(&screen, size, asking, "ask-a1b"),
            theme().waiting,
            "a row that is asking is still asking"
        );

        // Off the cursor, the accented name is still dim.
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

    /// An agent with model and effort set, one with neither, and a shell
    /// command.
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
            // The age is right-aligned, so it ends the trimmed line.
            let age = |line: &str| line.chars().rev().take(2).collect::<String>();
            assert_eq!(
                age(before),
                age(after),
                "nor does the age for {id}:\n{before:?}\n{after:?}"
            );
        }
        // Off, no row shows it. Checked per row because the header's dials
        // row always says `claude`.
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

        // The numbers line up in one column.
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
        // Formatted by `derive::in_words`, the same as `ls`.
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
        // An idle agent's age keeps climbing; its worked time does not.
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
        // Control and zero-width characters become spaces, as in the name and
        // the card's question, so the row keeps its width.
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
        // An emoji is one char and two columns. Measured in chars, it pushed
        // the row one column right and cut the age's unit off the edge.
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

        // `fit` counts columns too.
        assert_eq!(fit("👋👋👋👋", 8), "👋👋👋👋");
        assert_eq!(fit("👋👋👋👋", 4), "👋…");
        assert_eq!(fit("ab👋cd", 5), "ab👋…");
    }

    /// More agents than any band here is tall, under one heading and unfolded.
    fn twenty() -> Vec<View> {
        (0..20)
            .map(|n| view(&format!("row-{n:02}"), Phase::Done, Some("did it"), 60))
            .collect()
    }

    #[test]
    fn the_window_holds_where_it_was_left_and_never_past_the_last_page() {
        let screen = showing(twenty(), None);
        let items = screen.list.items().len();

        assert_eq!(first_drawn(&screen.list, 6, &screen.wall), 0);

        // The wheel scrolls without following the cursor.
        screen.wall.top.set(3);
        assert_eq!(first_drawn(&screen.list, 6, &screen.wall), 3);

        // Past the end clamps to the last page, and the clamp is stored.
        screen.wall.top.set(900);
        assert_eq!(first_drawn(&screen.list, 6, &screen.wall), items - 6);
        assert_eq!(screen.wall.top.get(), items - 6);

        // A band taller than the list shows it from the top.
        screen.wall.top.set(4);
        assert_eq!(first_drawn(&screen.list, 60, &screen.wall), 0);
    }

    #[test]
    fn the_window_follows_the_cursor_by_the_least_it_can() {
        let mut screen = showing(twenty(), None);
        let items = screen.list.items().len();

        // Cursor below the window: scroll until it is on the last line.
        screen.list.bottom();
        screen.wall.follow.set(true);
        assert_eq!(first_drawn(&screen.list, 6, &screen.wall), items - 6);
        assert!(!screen.wall.follow.get(), "the frame answered it");

        // Cursor inside the window: no scroll.
        screen.list.up();
        screen.wall.follow.set(true);
        assert_eq!(first_drawn(&screen.list, 6, &screen.wall), items - 6);

        // Cursor above the window: scroll up to it.
        screen.list.top();
        screen.wall.follow.set(true);
        assert_eq!(first_drawn(&screen.list, 6, &screen.wall), 0);

        // Without `follow`, a wheel scroll away from the cursor holds.
        screen.wall.top.set(items - 6);
        assert_eq!(first_drawn(&screen.list, 6, &screen.wall), items - 6);
    }
}
