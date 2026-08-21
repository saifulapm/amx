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

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::sync::OnceLock;

use super::act::{Asking, Composer};
use super::rows::{Group, Item, List};
use super::{Mode, Screen};
use crate::derive::View;
use crate::store::Phase;

/// The keys with nowhere else to be said, on the screen where somebody can see
/// them. The rest are one keypress away, which is what `?` is for.
const KEYS: &str = "space peek · enter attach · ? keys · q quit";

/// Every key, for whoever asked what they are.
const HELP: [(&str, &str); 9] = [
    ("↑ ↓", "walk the agents"),
    ("space", "look closer at one"),
    ("enter", "bring its window forward"),
    ("n", "start an agent · tab starts it out of sight"),
    ("r", "reply: a message, or the key a question wants"),
    ("d", "what it has changed"),
    ("ctrl+x", "stop it · again to forget it"),
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
    let panel = match (helping, &screen.peek) {
        (false, Some(_)) => peek_height(area.height),
        _ => 0,
    };
    let composing = matches!(screen.mode, Mode::Typing(_));

    let [top, middle, bottom, line, keys] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(panel),
        Constraint::Length(u16::from(composing)),
        Constraint::Length(1),
    ])
    .areas(area);

    frame.render_widget(Paragraph::new(header(&screen.list)), top);
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
/// the list it was opened from is gone.
fn peek_height(total: u16) -> u16 {
    let room = total.saturating_sub(3);
    (total / 2).clamp(3, 14).min(room)
}

/// What there is, in one line.
fn header(list: &List) -> Line<'static> {
    let mut spans = vec![Span::styled(
        "amx",
        Style::new().add_modifier(Modifier::BOLD),
    )];
    for (group, count) in list.counts() {
        spans.push(Span::raw(" · "));
        spans.push(Span::styled(
            format!("{count} {}", group.title()),
            group_colour(group),
        ));
    }
    Line::from(spans)
}

/// The agents themselves.
fn agents(frame: &mut Frame, list: &List, area: Rect, beat: usize) {
    if list.is_empty() {
        frame.render_widget(Paragraph::new(Line::styled("no agents", dim())), area);
        return;
    }

    let height = area.height as usize;
    // Enough of the top scrolled away to keep the cursor on the screen.
    let offset = list.cursor().saturating_sub(height.saturating_sub(1));
    let names = name_width(list);

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
                names,
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
    names: usize,
    width: usize,
    beat: usize,
) -> Line<'static> {
    match item {
        Item::Heading(group, _) => Line::styled(
            group.title().to_string(),
            group_colour(group).add_modifier(Modifier::BOLD),
        ),
        Item::Fold(hidden) => Line::styled(format!("{}… {hidden} more", marker(selected)), dim()),
        Item::Agent(_) => match list.agent(item) {
            Some(view) => row(view, selected, names, width, beat),
            None => Line::raw(""),
        },
    }
}

/// An agent's row: what state it is in, what it is called, what it is up to,
/// and how long since anybody heard from it.
fn row(view: &View, selected: bool, names: usize, width: usize, beat: usize) -> Line<'static> {
    let phase = view.phase();
    let age = age(view.verdict.age);
    let name = fit(view.id(), names);
    // The marker, the icon and its space, the name and its gap, the age and
    // the space before it.
    let spent = 2 + 2 + names + 2 + AGE + 1;
    let room = width.saturating_sub(spent);
    let said = fit(first_line(view.line().unwrap_or("")), room);

    Line::from(vec![
        Span::raw(marker(selected)),
        Span::styled(format!("{} ", icon(phase, beat)), colour(phase)),
        Span::styled(
            format!("{name:<names$}  "),
            if selected {
                Style::new().add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            },
        ),
        Span::styled(format!("{said:<room$} "), dim()),
        Span::styled(format!("{age:>AGE$}"), dim()),
    ])
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
                    format!("{key:<7}"),
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                Span::styled((*does).to_string(), dim()),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// The line somebody is typing, with the terminal's own cursor at the end of
/// it: something being typed into should look like it.
fn composing_line(frame: &mut Frame, composer: &Composer, area: Rect) {
    let prompt = format!("{} ▸ ", composer.prompt());
    let width = area.width as usize;
    let room = width.saturating_sub(prompt.chars().count() + 1);
    // The end of the line, because the end is where somebody is typing.
    let typed = end_of(&composer.text, room);

    let at = prompt.chars().count() + typed.chars().count();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(prompt, Style::new().fg(role::WARNING)),
            Span::raw(typed),
        ])),
        area,
    );
    frame.set_cursor_position((area.x + at.min(width.saturating_sub(1)) as u16, area.y));
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
            Mode::Typing(composer) => match composer.asking {
                Asking::Task => "enter starts it · tab out of sight · esc cancels".to_string(),
                Asking::Reply { .. } => "enter sends it · esc cancels".to_string(),
            },
        },
        dim(),
    )
}

/// How wide the column of names has to be. Capped, because one long name must
/// not push what every agent is doing off the side of the screen.
fn name_width(list: &List) -> usize {
    list.items()
        .iter()
        .filter_map(|item| list.agent(*item))
        .map(|view| view.id().chars().count())
        .max()
        .unwrap_or(0)
        .clamp(6, 24)
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

/// The last `width` characters of `text`, which is the part of a line
/// somebody typing it is looking at.
fn end_of(text: &str, width: usize) -> String {
    let over = text.chars().count().saturating_sub(width);
    text.chars().skip(over).collect()
}

/// Where the cursor is.
fn marker(selected: bool) -> &'static str {
    if selected { "▸ " } else { "  " }
}

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

/// The colours, by what they mean rather than by what they are. Four of them,
/// and the values are the vendor's own dark theme measured from the 2.1.237
/// binary: a view beside claude's should not be a different shade of the same
/// idea.
mod role {
    use ratatui::style::Color;

    /// It went the way it was meant to.
    pub const SUCCESS: Color = Color::Rgb(78, 186, 101);
    /// Something is waiting on a person.
    pub const WARNING: Color = Color::Rgb(255, 193, 7);
    /// It was attempted and it failed.
    pub const ERROR: Color = Color::Rgb(255, 107, 128);
    /// It was ended by hand, and nothing more is coming.
    pub const INACTIVE: Color = Color::Rgb(153, 153, 153);
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

#[cfg(test)]
mod tests {
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
            at(0).starts_with(&format!("▸ {} port-importer-b2c", pulse(0))),
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

        assert_eq!(screen[0], "amx · 1 needs input · 1 working");
        assert_eq!(screen[1], "needs input");
        assert!(screen[2].starts_with("▸ ? ask-a1b"), "{:?}", screen[2]);
        assert!(screen[2].ends_with("1m"), "{:?}", screen[2]);
        assert_eq!(screen[3], "working");
        assert!(
            screen[4].starts_with(&format!("  {} fix-login-b2c", pulse(0))),
            "{:?}",
            screen[4]
        );
        assert!(screen[4].contains("Running Bash"), "{:?}", screen[4]);
        assert!(screen[4].ends_with("3s"), "{:?}", screen[4]);
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
    fn view_says_when_there_is_nothing_to_show() {
        let screen = drawn(Vec::new(), None, (40, 6));
        assert_eq!(screen[0], "amx");
        assert_eq!(screen[1], "no agents");
    }

    #[test]
    fn view_shows_the_fold_and_what_it_is_holding_back() {
        let views = (0..5)
            .map(|n| view(&format!("done-{n}"), Phase::Done, Some("did it"), 60))
            .collect();
        let screen = drawn(views, None, (40, 10));

        assert_eq!(screen[1], "completed");
        assert_eq!(screen.iter().filter(|l| l.contains("done-")).count(), 3);
        assert!(screen[5].contains("… 2 more"), "{:?}", screen[5]);
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

        let painted = painted(&screen, (60, 12)).join("\n");
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
