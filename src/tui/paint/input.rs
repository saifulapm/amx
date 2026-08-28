//! The line somebody is typing when they are typing one, and the row of keys
//! under it.
//!
//! Two bands and the slot between them. The composer grows a row at a time as
//! the line does and stops before it takes the list; under it goes the one
//! thing the next agent will be started with that is not on the header's dial
//! row — what it may do without asking, said where somebody is about to press
//! enter and wearing weight as well as the accent for it; and under that, the
//! keys, or whatever the view has to say for itself instead.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::style::{dim, prospective};
use super::text::{SEPARATOR, fit};
use crate::registry::DEFAULT;
use crate::theme::Theme;
use crate::tui::act::{Asking, Composer};
use crate::tui::rows::Item;
use crate::tui::{Mode, Screen};

/// The key the hint row keeps whatever else it has to shed, because the
/// overlay behind it is where every key is.
const MORE: &str = "? keys";

/// What the card's own keys do, under the card, while it is holding a line.
///
/// What may be typed *into* that line is the question's business and is said
/// on the line itself. Only these two are offered: alt+enter puts a newline in
/// the line like anywhere else in the view, and a prompt that reads one key
/// would refuse whatever a newline was typed into, so a row that named it
/// would be naming a key that cannot work where it was read.
pub(super) const ANSWERS: &str = "enter answers it · esc closes it";

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
pub(super) fn composer_lines(text: &str, room: usize) -> Vec<String> {
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
pub(super) fn composer_height(composer: &Composer, area: Rect, chrome: u16) -> u16 {
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
pub(super) fn composing_line(frame: &mut Frame, composer: &Composer, area: Rect, theme: Theme) {
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
                0 => Span::styled(prompt.clone(), Style::new().fg(theme.waiting)),
                _ => Span::raw(indent.clone()),
            };
            let mut spans = vec![head, Span::raw(text.clone())];
            // An empty line holds its prefixes as ghost text, cut where the
            // screen ends; the cursor set below sits over the front of it.
            if at == 0
                && let Some(hint) = placeholder(composer)
            {
                let room = width.saturating_sub(prompt.chars().count());
                spans.push(Span::styled(fit(hint, room), dim()));
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);

    let at = prompt.chars().count() + shown.last().map_or(0, |row| row.chars().count());
    frame.set_cursor_position((
        area.x + at.min(width.saturating_sub(1)) as u16,
        area.y + shown.len().saturating_sub(1) as u16,
    ));
}

/// What the next agent will be allowed to do without asking, under the line it
/// would be started from — where claude's own screens put it, and where
/// somebody about to press enter is already looking.
///
/// The row belongs to a line that will start an agent: not to a reply, which
/// goes to one already running under whatever it was started with, and not to
/// a line that narrows the list. At the sentinel it names the layer rather
/// than a mode, because amx does not know which mode the vendor is configured
/// for and a guess at it is the same lie the model dial refuses. A vendor
/// whose entry declares no permission dial has nothing to say here and nothing
/// to turn, so the row is absent rather than empty.
pub(super) fn permission(screen: &Screen) -> Option<Line<'static>> {
    let Mode::Typing(composer) = &screen.mode else {
        return None;
    };
    if !matches!(composer.asking, Asking::Task) || composer.narrows() {
        return None;
    }
    screen.profile.permission_dial()?;

    let said = match screen.profile.permission.as_str() {
        DEFAULT => "permission: vendor default".to_string(),
        mode => format!("⏵⏵ {mode}"),
    };
    Some(Line::styled(
        format!("{said} (shift+tab to cycle)"),
        prospective(screen.theme),
    ))
}

/// The words the task line reads at its front, said on the line itself while
/// there is nothing on it.
///
/// The prefixes are amx's own grammar and nothing else on the screen teaches
/// them: a dial turned by `m:` looks exactly like a task that happens to open
/// with one. So the empty line holds them the way a form field holds its
/// ghost text — dim, after the prompt, and gone at the first character typed,
/// because whoever is typing has stopped reading it. A reply and a rename
/// read no prefixes, so their lines teach none.
fn placeholder(composer: &Composer) -> Option<&'static str> {
    if !matches!(composer.asking, Asking::Task) || !composer.text.is_empty() {
        return None;
    }
    Some(
        "m:model · p:permission · w:on|off · d:directory · agent:command \
         · s:state · a:name",
    )
}

/// The keys with nowhere else to be said, as the line under the cursor makes
/// them true.
///
/// Enter brings a window forward on a row, shuts a group on a heading and
/// gives back the fold's rows on the fold; a row of hints that named one of
/// those over the other two would be teaching somebody to press the wrong key.
/// So what the cursor is standing on decides the front of the row, and the
/// keys that mean the same thing wherever it is standing follow.
fn hints(screen: &Screen) -> Vec<&'static str> {
    let list = &screen.list;
    let mut said = match list.items().get(list.cursor()) {
        Some(Item::Heading(_, tally)) => vec![
            match tally.shut {
                true => "enter opens it",
                false => "enter shuts it",
            },
            "ctrl+x clears the group",
        ],
        Some(Item::Fold(_)) => vec!["enter shows them"],
        // The cursor never rests on a blank; the arm is for the compiler.
        Some(Item::Blank) => Vec::new(),
        // An agent whose command has ended has no window to bring forward and
        // nothing left to stop, and the same key that would have stopped it
        // forgets it instead.
        Some(Item::Agent(_)) => {
            let card = match screen.card.is_some() {
                true => "space closes it",
                false => "space card",
            };
            match list
                .selected()
                .is_some_and(|view| view.phase().is_terminal())
            {
                true => vec![card, "ctrl+x forget"],
                false => vec![card, "enter attach", "ctrl+x stop"],
            }
        }
        // A wall with nothing on it has no line under the cursor, and the one
        // key that changes that is the one worth the room.
        None => vec!["n starts one"],
    };
    said.extend(["ctrl+s axis", "q quit"]);
    said
}

/// Those keys on one row, cut to what a screen this wide can hold.
///
/// What goes is what is furthest from `?`, and `?` itself never does: a hint
/// clipped by the terminal reads as a key that ends where the screen does, and
/// the last one a narrow row has room for should be the one that leads to all
/// the others.
fn fitted(said: &[&str], width: usize) -> String {
    let taken = |kept: &[&str]| {
        kept.iter()
            .map(|hint| hint.chars().count() + SEPARATOR.chars().count())
            .sum::<usize>()
            + MORE.chars().count()
    };

    let mut kept = said.to_vec();
    while !kept.is_empty() && taken(&kept) > width {
        kept.pop();
    }
    kept.push(MORE);
    kept.join(SEPARATOR)
}

/// The keys, or whatever the view has to say for itself instead.
///
/// The row under the card is the card's while it is holding a line, because
/// what enter does there is not what it does anywhere else in the view.
pub(super) fn footer(screen: &Screen, width: u16) -> Line<'static> {
    if let Some(notice) = &screen.notice {
        return match notice {
            Notice::Failed(said) => {
                Line::styled(said.clone(), Style::new().fg(screen.theme.failed))
            }
            Notice::Advice(said) => Line::styled(said.clone(), dim()),
        };
    }
    if screen.answering().is_some() {
        return Line::styled(ANSWERS.to_string(), dim());
    }
    // A question of the view's own is not advice and not a key: it is the one
    // thing on the screen, in the colour of something waiting on a person.
    if let Mode::Confirming(asked) = &screen.mode {
        return Line::styled(asked.question(), Style::new().fg(screen.theme.waiting));
    }
    Line::styled(
        match &screen.mode {
            Mode::List => fitted(&hints(screen), width as usize),
            Mode::Keys => "any key goes back · q quits".to_string(),
            // A question up is the whole of this row, and is drawn above.
            Mode::Confirming(_) => fitted(&hints(screen), width as usize),
            Mode::Typing(composer) if composer.narrows() => {
                "enter narrows it · s: or a: alone clears · esc cancels".to_string()
            }
            Mode::Typing(composer) => match composer.asking {
                Asking::Task => "enter starts it · alt+enter newline · esc cancels".to_string(),
                Asking::Reply { .. } => {
                    "enter sends it · alt+enter newline · esc cancels".to_string()
                }
                Asking::Name { .. } => "enter renames it · esc leaves it alone".to_string(),
            },
        },
        dim(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::{Evidence, Verdict, View};
    use crate::store::{Kind, Meta, Phase, State};
    use crate::tmux::{PaneId, Socket};
    use crate::tui::paint::empty::WELCOME;
    use crate::tui::paint::{Card, draw};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::{Color, Modifier};
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

    /// The card a waiting agent's row opens: what it is asking, the choices it
    /// offers, and the screen it is asking on.
    fn asking(options: &[&str], kind: Option<Kind>) -> Card {
        Card {
            id: "ask-a1b".to_string(),
            phase: Phase::Waiting,
            age: 29,
            question: Some("Which fixture should the port keep?".to_string()),
            options: options.iter().map(|label| (*label).to_string()).collect(),
            kind,
            body: "$ cargo test\nDo you want to proceed?".to_string(),
            changes: false,
            answer: false,
        }
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

    /// The two agents a card is opened over, so there is a list to still be
    /// drawn behind it.
    fn a_fleet() -> Vec<View> {
        vec![
            view("ask-a1b", Phase::Waiting, None, 29),
            view("busy-b2c", Phase::Working, Some("Running Bash"), 3),
        ]
    }

    /// The view with a launch profile that says where it is running: the
    /// directory is read from the disk when a real view opens, and a test says
    /// what the disk would have answered.
    fn launching(views: Vec<View>) -> Screen {
        let mut screen = showing(views, None);
        screen.profile.dir = "~/code/amx".to_string();
        screen
    }

    /// The row the keys are drawn on, which is the last one on the screen.
    fn hint_row(screen: &Screen, size: (u16, u16)) -> String {
        painted(screen, size).pop().expect("a row for the keys")
    }

    /// A fleet with nothing left to finish, so there is a fold to walk onto.
    fn all_done() -> Vec<View> {
        (0..5)
            .map(|n| view(&format!("done-{n}"), Phase::Done, Some("did it"), 60))
            .collect()
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

    /// A line long enough to need more rows than any screen will give it.
    fn twenty_rows() -> String {
        (1..=20)
            .map(|n| format!("row-{n:02}"))
            .collect::<Vec<_>>()
            .join("\n")
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

    #[test]
    fn keymap_hints_are_the_keys_the_line_under_the_cursor_answers_to() {
        let wide = (80, 12);
        let mut screen = showing(a_fleet(), None);

        // The view opens on an agent's row, where those keys reach the agent.
        assert_eq!(
            hint_row(&screen, wide),
            "space card · enter attach · ctrl+x stop · ctrl+s axis · q quit · ? keys"
        );

        // One line up is the heading over it, where the same two keys do
        // something else entirely.
        screen.list.up();
        assert_eq!(
            hint_row(&screen, wide),
            "enter shuts it · ctrl+x clears the group · ctrl+s axis · q quit · ? keys"
        );

        // And a group somebody has shut is opened by the key that shut it.
        screen.list.shut_or_open();
        assert!(
            hint_row(&screen, wide).starts_with("enter opens it"),
            "{:?}",
            hint_row(&screen, wide)
        );
    }

    #[test]
    fn keymap_hints_offer_nothing_the_line_under_the_cursor_cannot_do() {
        let wide = (80, 12);

        // A card is put away by the key that opened it.
        let mut screen = showing(a_fleet(), None);
        screen.card = Some(asking(&[], None).read());
        assert!(
            hint_row(&screen, wide).starts_with("space closes it · enter attach"),
            "{:?}",
            hint_row(&screen, wide)
        );

        // An agent whose command has ended has no window to bring forward and
        // nothing left to stop.
        let mut screen = showing(all_done(), None);
        screen.list.fit(5);
        screen.list.refit();
        let row = hint_row(&screen, wide);
        assert!(row.starts_with("space card · ctrl+x forget"), "{row:?}");
        assert!(!row.contains("attach"), "{row:?}");

        // The fold is not an agent either: what enter does there is give back
        // the rows it is holding.
        for _ in 0..3 {
            screen.list.down();
        }
        assert!(
            hint_row(&screen, wide).starts_with("enter shows them"),
            "{:?}",
            hint_row(&screen, wide)
        );

        // And a wall with nothing on it has no line under the cursor at all.
        let screen = showing(Vec::new(), None);
        assert!(
            hint_row(&screen, wide).starts_with("n starts one"),
            "{:?}",
            hint_row(&screen, wide)
        );
    }

    #[test]
    fn keymap_hints_shed_from_the_far_end_and_never_shed_the_overlay() {
        let screen = showing(a_fleet(), None);
        for width in 12..=80 {
            let row = hint_row(&screen, (width, 12));
            assert!(
                row.chars().count() <= width as usize,
                "a hint cut in half is a key that reads as another one: {row:?}"
            );
            assert!(
                row.ends_with("? keys"),
                "the row that has all of them is the last thing to go: {row:?}"
            );
        }

        // What is shed is what is furthest from it, and what is kept is what
        // the line under the cursor answers to.
        assert_eq!(
            hint_row(&screen, (60, 12)),
            "space card · enter attach · ctrl+x stop · ? keys"
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
            (theme().failed, Modifier::empty())
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
    fn composer_an_empty_task_line_names_its_own_prefixes() {
        // Wide enough for the whole sentence; a narrow screen clips it with
        // the ellipsis every other row wears.
        let empty = painted(&typing(""), (110, 30));
        let hint = empty
            .iter()
            .find(|row| row.contains("m:model"))
            .expect("the empty line teaches its prefixes");
        assert!(
            hint.starts_with("task ▸ m:model"),
            "the hint is a placeholder on the line itself, not a row of its \
             own: {hint}"
        );
        for named in [
            "m:model",
            "p:permission",
            "w:on|off",
            "d:directory",
            "agent:command",
            "s:state",
            "a:name",
        ] {
            assert!(hint.contains(named), "{named} is not taught: {hint}");
        }
        assert_eq!(
            empty.iter().filter(|row| row.contains("m:model")).count(),
            1,
            "and only there: the band under the composer is gone"
        );

        let narrow = painted(&typing(""), TALL);
        let clipped = narrow
            .iter()
            .find(|row| row.contains("m:model"))
            .expect("a narrow screen still teaches what fits");
        assert!(clipped.starts_with("task ▸ m:model"), "{clipped}");
        assert!(clipped.trim_end().ends_with('…'), "{clipped}");

        // The next keystroke lands where the prompt ends, over the
        // placeholder, the way a browser draws a field's ghost text.
        assert_eq!(caret(&typing(""), TALL), (7, 27));

        // The first character typed takes the placeholder away: whoever is
        // typing has stopped reading it.
        let typed = painted(&typing("p"), TALL);
        assert!(
            !typed.iter().any(|row| row.contains("m:model")),
            "{typed:?}"
        );

        // A reply goes to an agent already running, where a dial means
        // nothing, so the line would be teaching keys it does not read.
        let mut replying = showing(Vec::new(), None);
        replying.mode = Mode::Typing(Composer::new(Asking::Reply {
            id: "fix-a1b".to_string(),
            question: false,
        }));
        let reply = painted(&replying, TALL);
        assert!(
            !reply.iter().any(|row| row.contains("m:model")),
            "{reply:?}"
        );
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
        assert_eq!(one[27], "task ▸ port the importer");
        assert_eq!(one[26], "", "one line takes one row, at the foot of it all");

        let three = painted(
            &typing("port the importer\nand its tests\nand the docs"),
            TALL,
        );
        assert_eq!(three[25], "task ▸ port the importer");
        assert_eq!(
            three[26], "       and its tests",
            "a row under the first is indented to it, so a task reads as one \
             thing"
        );
        assert_eq!(three[27], "       and the docs");
        assert_eq!(
            caret(&typing("port it\nand test it"), TALL),
            (18, 27),
            "and the cursor is at the end of the last of them"
        );
    }

    #[test]
    fn composer_wrapping_past_the_width_grows_it_the_same_way_a_newline_does() {
        // Twice the room a sixty-column screen leaves beside the prompt.
        let painted = painted(&typing(&"x".repeat(106)), TALL);
        assert_eq!(painted[26], format!("task ▸ {}", "x".repeat(53)));
        assert_eq!(painted[27], format!("       {}", "x".repeat(53)));
    }

    #[test]
    fn composer_stops_growing_at_its_cap_and_scrolls_the_line_inside_it() {
        let screen = typing(&twenty_rows());
        let painted = painted(&screen, TALL);

        assert_eq!(
            painted[18], "task ▸ row-11",
            "the prompt is on the top row however far the rest has scrolled: \
             {painted:?}"
        );
        assert_eq!(painted[27], "       row-20", "{painted:?}");
        assert!(
            !painted.iter().any(|line| line.contains("row-10")),
            "and what scrolled past is off the screen: {painted:?}"
        );
        assert_eq!(caret(&screen, TALL), (13, 27));
    }

    #[test]
    fn composer_leaves_the_list_it_was_opened_from_on_the_screen() {
        // A third of eight rows is two, whatever the line is holding, and the
        // agents are what the view is for.
        let painted = painted(&typing(&twenty_rows()), (60, 8));
        assert_eq!(painted[4], "task ▸ row-19");
        assert_eq!(painted[5], "       row-20");
        assert_eq!(
            painted[1], WELCOME,
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
        assert_eq!(painted[3], "task ▸ port the importer");
        assert!(painted[5].contains("enter starts it"), "{:?}", painted[5]);
        assert!(painted[5].contains("alt+enter newline"), "{:?}", painted[5]);
    }

    #[test]
    fn header_says_what_the_next_agent_may_do_without_asking() {
        let mut screen = launching(Vec::new());
        screen.mode = Mode::Typing(Composer::new(Asking::Task));

        let drawn = painted(&screen, (60, 8));
        assert!(
            drawn[5].starts_with("task ▸ m:model"),
            "the empty line carries its placeholder above the dial: {:?}",
            drawn[5]
        );
        assert_eq!(
            drawn[6], "permission: vendor default (shift+tab to cycle)",
            "the layer, not a guess at which mode claude would have picked"
        );
        assert!(drawn[7].contains("enter starts it"), "{:?}", drawn[7]);

        screen.profile.permission = "acceptEdits".to_string();
        assert_eq!(
            painted(&screen, (60, 8))[6],
            "⏵⏵ acceptEdits (shift+tab to cycle)",
            "and a mode in the vendor's own word for it"
        );
    }

    #[test]
    fn header_keeps_the_permission_row_to_the_lines_that_start_an_agent() {
        let row = |screen: &Screen| {
            painted(screen, (60, 8))
                .iter()
                .any(|line| line.contains("shift+tab"))
        };

        // A reply goes to an agent that is already running under whatever it
        // was started with, so the dial has nothing to say about it.
        let mut screen = launching(Vec::new());
        screen.mode = Mode::Typing(Composer::new(Asking::Reply {
            id: "ask-a1b".to_string(),
            question: true,
        }));
        assert!(!row(&screen), "a reply is not a spawn");

        // Nor has it anything to say about a line that narrows the list.
        let mut composer = Composer::new(Asking::Task);
        composer.text = "s:waiting".to_string();
        screen.mode = Mode::Typing(composer);
        assert!(!row(&screen));

        // A vendor amx has no entry for declares no permission dial: there is
        // nothing to say and nothing to turn, so the row is absent rather than
        // empty.
        screen.mode = Mode::Typing(Composer::new(Asking::Task));
        screen.profile.agent = "mock-claude".to_string();
        assert!(!row(&screen));

        // And nothing is being typed at all, which is most of the time.
        let screen = launching(Vec::new());
        assert!(!row(&screen));
    }

    #[test]
    fn header_leaves_the_list_a_row_with_every_other_band_open() {
        // Four bands of chrome at once: the header, a closer look, a line
        // being typed and the row under it. The list is what the view is for,
        // so the closer look gives way rather than the rows it was opened
        // from.
        let mut screen = launching(vec![view("ask-a1b", Phase::Waiting, None, 30)]);
        screen.card = Some(asking(&["the sqlite one"], Some(Kind::Question)).read());
        screen.mode = Mode::Typing(Composer::new(Asking::Task));

        let painted = painted(&screen, (60, 10));
        assert!(
            painted.iter().any(|line| line.contains("ask-a1b")),
            "{painted:?}"
        );
        assert!(
            painted.iter().any(|line| line.contains("shift+tab")),
            "{painted:?}"
        );
        assert!(painted[9].contains("enter starts it"), "{:?}", painted[9]);
    }
}
