//! Drawing the view.
//!
//! Four bands, top to bottom: what there is, the agents themselves, a closer
//! look at one of them when somebody asked for one, and the keys. Everything
//! here is a function of what it is handed, so what the screen says can be
//! read back in a test without a terminal anywhere near it.
//!
//! A row is one line, always: an agent's answer is a paragraph, and a
//! paragraph in a list is how a list stops being one.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::rows::{Group, Item, List};
use crate::derive::View;
use crate::store::Phase;

/// What the keys do, on the screen where somebody can see them.
const KEYS: &str = "space peek · enter attach · q quit";

/// A closer look at one agent.
pub struct Peek {
    pub id: String,
    pub phase: Phase,
    /// What it is waiting to be told, when it is waiting to be told anything.
    pub question: Option<String>,
    /// The screen it is sitting on, or the answer it left behind.
    pub body: String,
}

/// Draw everything.
pub fn draw(frame: &mut Frame, list: &List, peek: Option<&Peek>, notice: Option<&str>) {
    let area = frame.area();
    let panel = peek.map_or(0, |_| peek_height(area.height));
    let [top, middle, bottom, keys] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(panel),
        Constraint::Length(1),
    ])
    .areas(area);

    frame.render_widget(Paragraph::new(header(list)), top);
    agents(frame, list, middle);
    if let Some(peek) = peek {
        look(frame, peek, bottom);
    }
    frame.render_widget(Paragraph::new(footer(notice)), keys);
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
            Style::new().fg(group_colour(group)),
        ));
    }
    Line::from(spans)
}

/// The agents themselves.
fn agents(frame: &mut Frame, list: &List, area: Rect) {
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
        .map(|(at, item)| line(list, *item, at == list.cursor(), names, area.width as usize))
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// One line of the list, whatever kind of line it is.
fn line(list: &List, item: Item, selected: bool, names: usize, width: usize) -> Line<'static> {
    match item {
        Item::Heading(group, _) => Line::styled(
            group.title().to_string(),
            Style::new()
                .fg(group_colour(group))
                .add_modifier(Modifier::BOLD),
        ),
        Item::Fold(hidden) => Line::styled(format!("{}… {hidden} more", marker(selected)), dim()),
        Item::Agent(_) => match list.agent(item) {
            Some(view) => row(view, selected, names, width),
            None => Line::raw(""),
        },
    }
}

/// An agent's row: what state it is in, what it is called, what it is up to,
/// and how long since anybody heard from it.
fn row(view: &View, selected: bool, names: usize, width: usize) -> Line<'static> {
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
        Span::styled(format!("{} ", icon(phase)), Style::new().fg(colour(phase))),
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
/// asking it on.
fn look(frame: &mut Frame, peek: &Peek, area: Rect) {
    let block = Block::new().borders(Borders::TOP).title(format!(
        " {} · {} ",
        peek.id,
        peek.phase.as_str()
    ));
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
                .style(Style::new().fg(Color::Yellow)),
            asking,
        );
    }
    let lines: Vec<Line> = tail(&peek.body, screen.height as usize)
        .into_iter()
        .map(|text| Line::styled(text.to_string(), dim()))
        .collect();
    frame.render_widget(Paragraph::new(lines), screen);
}

/// The keys, or whatever the view has to say for itself instead.
fn footer(notice: Option<&str>) -> Line<'static> {
    match notice {
        Some(said) => Line::styled(said.to_string(), Style::new().fg(Color::Yellow)),
        None => Line::styled(KEYS, dim()),
    }
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

/// Where the cursor is.
fn marker(selected: bool) -> &'static str {
    if selected { "▸ " } else { "  " }
}

/// The state, as one character.
fn icon(phase: Phase) -> char {
    match phase {
        Phase::Waiting => '?',
        Phase::Starting | Phase::Working => '●',
        Phase::Idle => '○',
        Phase::Done => '✓',
        Phase::Failed => '✗',
        Phase::Stopped => '■',
        Phase::Unknown => '~',
    }
}

fn colour(phase: Phase) -> Color {
    match phase {
        Phase::Waiting => Color::Yellow,
        Phase::Starting | Phase::Working => Color::Cyan,
        Phase::Idle => Color::Green,
        Phase::Done => Color::Green,
        Phase::Failed => Color::Red,
        Phase::Stopped => Color::DarkGray,
        Phase::Unknown => Color::Magenta,
    }
}

fn group_colour(group: Group) -> Color {
    match group {
        Group::NeedsInput => Color::Yellow,
        Group::Working => Color::Cyan,
        Group::Idle => Color::Green,
        Group::Completed => Color::DarkGray,
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

    /// What the view puts on a screen of this size, line by line.
    fn drawn(views: Vec<View>, peek: Option<Peek>, size: (u16, u16)) -> Vec<String> {
        let mut list = List::default();
        list.show(views);
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
        terminal
            .draw(|frame| draw(frame, &list, peek.as_ref(), None))
            .unwrap();

        let buffer = terminal.backend().buffer().clone();
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
            screen[4].starts_with("  ● fix-login-b2c"),
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
        let mut list = List::default();
        list.show(Vec::new());
        let mut terminal = Terminal::new(TestBackend::new(60, 6)).unwrap();
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &list,
                    None,
                    Some("fix-login-a1b has no pane any more"),
                )
            })
            .unwrap();

        let buffer = terminal.backend().buffer().clone();
        let last: String = (0..60).map(|c| buffer[(c, 5)].symbol()).collect();
        assert_eq!(last.trim_end(), "fix-login-a1b has no pane any more");
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
