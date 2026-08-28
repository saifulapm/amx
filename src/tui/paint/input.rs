//! The line somebody is typing when they are typing one, and the row of keys
//! under it.
//!
//! A rule, a composer and the slot under them. The rule is the edge the whole
//! mode hangs off: it names which of the five lines this is, says the one thing
//! that is true of all of them — every letter is text until esc — and carries
//! at its far end the one dial that is not on the header's row, what the next
//! agent may do without asking, in reverse video where somebody about to press
//! enter cannot miss it. Under the rule the composer grows a row at a time as
//! the line does and stops before it takes the list, and under that go the
//! keys, or whatever the view has to say for itself instead.
//!
//! The wall the rule was drawn over goes dim for as long as the mode is on.
//! Every row, heading, count and dial behind gives up its colour's weight in
//! one pass, so the band below the rule — the line being typed, and the keys
//! that are still keys under it — is the only thing on the screen carrying
//! any, which is what says the wall has stopped answering to the keyboard
//! without anybody reading a word of it.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::style::{bold, dim, prospective};
use super::text::{SEPARATOR, fit};
use crate::registry::DEFAULT;
use crate::theme::Theme;
use crate::tui::act::{Asking, Composer};
use crate::tui::rows::Item;
use crate::tui::{Mode, Screen};

/// A key and what pressing it does, which is the shape every hint has.
///
/// Two pieces rather than one sentence because they are read differently: the
/// key carries the weight and the words after it go dim, so a row of them
/// reads as a keyboard at a glance and only as prose on a second look. That is
/// also what stands between one hint and the next — the weight changing is a
/// clearer edge than any character amx could put there, and it costs no cells.
pub(super) type Hint = (&'static str, &'static str);

/// The key the hint row keeps whatever else it has to shed, because the
/// overlay behind it is where every key is.
const MORE: Hint = ("?", "keys");

/// What the card's own keys do, under the card, while it is holding a line.
///
/// What may be typed *into* that line is the question's business and is said
/// on the line itself. Only these two are offered: alt+enter puts a newline in
/// the line like anywhere else in the view, and a prompt that reads one key
/// would refuse whatever a newline was typed into, so a row that named it
/// would be naming a key that cannot work where it was read.
pub(super) const ANSWERS: [Hint; 2] = [("enter", "answers it"), ("esc", "closes it")];

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

/// What the composer's rows begin with: the chevron on the first of them, and
/// the same width of nothing under it, so a line that wrapped reads as one
/// line.
///
/// The same two cells whichever of the five lines this is. Which one it is, and
/// which agent it is aimed at, are on the rule above — so the line starts in
/// the column the rule's own label starts in, and moving between lines does not
/// move the words somebody is reading.
const GUTTER: &str = "❯ ";

/// How wide the text itself is drawn, which is the same on every row of the
/// composer whether the chevron or the indent is in front of it.
fn composer_room(width: u16) -> usize {
    (width as usize)
        .saturating_sub(GUTTER.chars().count())
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

/// The rule's own row, which the band holds whatever the line is holding: an
/// edge that came and went with the length of what somebody was typing would
/// not read as an edge.
const RULE: usize = 1;

/// How many rows the band takes on this screen: the rule, and under it as many
/// rows as the line needs, up to the cap, and never so many that the list it
/// was opened from is gone.
///
/// `chrome` is every other band already spoken for — the header, the keys, the
/// closer look — and one row over that is the list's, which the composer may
/// not have.
pub(super) fn composer_height(composer: &Composer, area: Rect, chrome: u16) -> u16 {
    let room = (area.height.saturating_sub(chrome + 1) as usize).saturating_sub(RULE);
    let cap = COMPOSER_CAP.min(area.height as usize / 3).min(room).max(1);
    let rows = composer_lines(&composer.text, composer_room(area.width))
        .len()
        .clamp(1, cap);
    (rows + RULE) as u16
}

/// The rule the mode hangs off, and everything said on it.
///
/// Its front is what this line is — the mode's own word, in the accent and
/// carrying weight, and after it which agent the line is aimed at where it is
/// aimed at one. Then the one thing true of every one of the five: while the
/// mode is on, a letter is a letter and not the key it is bound to, and esc is
/// the way out. Then the rule itself to the far end, where what the next agent
/// may do without asking is set in reverse video: the one dial that is not on
/// the header's row, promoted to the border somebody about to press enter is
/// looking straight at, costing no row of its own.
///
/// A screen with no room sheds the sentence first and the dial after it. The
/// label and the edge are what a rule cannot be without: one says which mode
/// this is and the other is the whole of why the rule is drawn. The agent goes
/// with the label, because a line aimed at the wrong agent is worse than a
/// line whose mode nobody can read.
fn rule(composer: &Composer, width: usize, theme: Theme) -> Line<'static> {
    let label = match composer.about() {
        Some(about) => format!("{}{SEPARATOR}{about} ", composer.label()),
        None => format!("{} ", composer.label()),
    };
    let dial = composer
        .allowed
        .take()
        .map_or_else(String::new, |said| format!(" {said} "));
    let taken = |gloss: &str, dial: &str| {
        label.chars().count()
            + gloss.chars().count()
            + match dial.is_empty() {
                // The tail is what closes the dial into the edge, so it costs
                // nothing on a rule that is not carrying one.
                true => 0,
                false => dial.chars().count() + TAIL.chars().count(),
            }
    };
    let (gloss, dial) = if taken(GLOSS, &dial) < width {
        (GLOSS, dial)
    } else if taken("", &dial) < width {
        ("", dial)
    } else {
        ("", String::new())
    };

    let drawn = prospective(theme);
    let mut spans = vec![
        Span::styled(fit(&label, width), drawn),
        Span::styled(gloss, dim()),
        Span::styled("─".repeat(width.saturating_sub(taken(gloss, &dial))), dim()),
    ];
    if !dial.is_empty() {
        spans.push(Span::styled(dial, drawn.add_modifier(Modifier::REVERSED)));
        spans.push(Span::styled(TAIL, dim()));
    }
    Line::from(spans)
}

/// What the rule says after the mode's own word.
const GLOSS: &str = "· letters are text until esc ";

/// The run of rule that closes it past the dial, so the dial reads as set into
/// the edge rather than as the end of it.
const TAIL: &str = "──";

/// The rule and, under it, the line somebody is typing.
///
/// The line is drawn bold with a block for the cursor, because it is the one
/// thing on the screen that has not happened yet and the only thing left
/// carrying weight — everything behind is dimmed by the same call. The
/// terminal's own cursor is put where the block is as well: a screen being
/// typed into should be one a terminal agrees is being typed into.
///
/// Past the cap it is the end of the line that is drawn, because the end is
/// where somebody is typing — but the chevron stays on the top row however far
/// the rest has scrolled. It is what says a line is being typed at all, and
/// that is worth a gutter wherever the text has got to.
pub(super) fn composing_line(frame: &mut Frame, composer: &Composer, area: Rect, theme: Theme) {
    behind(frame, area.y);
    let [edge, band] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    frame.render_widget(
        Paragraph::new(rule(composer, area.width as usize, theme)),
        edge,
    );

    let prompt = GUTTER.to_string();
    let width = band.width as usize;
    let rows = composer_lines(&composer.text, composer_room(band.width));
    let from = rows.len().saturating_sub(band.height as usize);
    let shown = &rows[from..];

    let indent = " ".repeat(prompt.chars().count());
    let lines: Vec<Line> = shown
        .iter()
        .enumerate()
        .map(|(at, text)| {
            let head = match at {
                0 => Span::styled(prompt.clone(), dim()),
                _ => Span::raw(indent.clone()),
            };
            let mut spans = vec![head, Span::styled(text.clone(), bold())];
            match placeholder(composer).filter(|_| at == 0) {
                // An empty line holds its prefixes as ghost text, cut where the
                // screen ends. It has the cell the block would have taken: a
                // cursor drawn over the first letter of what the line is
                // teaching would cost the lesson to say nothing the terminal's
                // own cursor is not already saying there.
                Some(hint) => {
                    let room = width.saturating_sub(prompt.chars().count());
                    spans.push(Span::styled(fit(hint, room), dim()));
                }
                None if at == shown.len() - 1 => {
                    spans.push(Span::styled(CURSOR, Style::new().fg(theme.accent)));
                }
                None => {}
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), band);

    let at = prompt.chars().count() + shown.last().map_or(0, |row| row.chars().count());
    frame.set_cursor_position((
        band.x + at.min(width.saturating_sub(1)) as u16,
        band.y + shown.len().saturating_sub(1) as u16,
    ));
}

/// The block that stands where the next character will land.
const CURSOR: &str = "█";

/// Everything above the rule, dimmed for as long as the mode is on.
///
/// One pass over what has already been drawn rather than a flag every surface
/// carries: the rows, the headings, the counts and the dials are each painted
/// for what they mean, and a mode is not one of the things they mean. What
/// this takes is the weight and the reverse video — the badge included, which
/// is the loudest thing up there — and what it leaves is the colours, dimmed,
/// so the wall is still readable as the wall it was a keystroke ago.
fn behind(frame: &mut Frame, until: u16) {
    let wall = Rect {
        height: until,
        ..frame.area()
    };
    frame.buffer_mut().set_style(
        wall,
        dim().remove_modifier(Modifier::BOLD | Modifier::REVERSED),
    );
}

/// What the next agent may do without asking, left on the line for the rule
/// over it to carry.
///
/// It belongs to a line that will start an agent: not to a reply, which goes to
/// one already running under whatever it was started with, and not to a line
/// that narrows the list. At the sentinel it names the layer rather than a
/// mode, because amx does not know which mode the vendor is configured for and
/// a guess at it is the same lie the model dial refuses. A vendor whose entry
/// declares no permission dial has nothing to say and nothing to turn, so the
/// rule ends bare.
///
/// The reading is taken here and left on the line because this is where the
/// view is in hand: [`rule`] is handed the line and the theme, which is how
/// every other band in this file is drawn, and which permission the dial is
/// resting on is a fact about neither. Nothing comes back for the band under
/// the composer — the dial had a row of its own there and has the far end of
/// the rule instead, and the row it gave up goes back to the list.
pub(super) fn permission(screen: &Screen) -> Option<Line<'static>> {
    let Mode::Typing(composer) = &screen.mode else {
        return None;
    };
    composer.allowed.set(
        (matches!(composer.asking, Asking::Task)
            && !composer.narrows()
            && screen.profile.permission_dial().is_some())
        .then(|| match screen.profile.permission.as_str() {
            DEFAULT => "vendor default".to_string(),
            mode => mode.to_string(),
        }),
    );
    None
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
fn hints(screen: &Screen) -> Vec<Hint> {
    let list = &screen.list;
    let mut said = match list.items().get(list.cursor()) {
        Some(Item::Heading(_, tally)) => vec![
            match tally.shut {
                true => ("enter", "opens it"),
                false => ("enter", "shuts it"),
            },
            ("ctrl+x", "clears the group"),
        ],
        Some(Item::Fold(_)) => vec![("enter", "shows them")],
        // The cursor never rests on a blank; the arm is for the compiler.
        Some(Item::Blank) => Vec::new(),
        // An agent whose command has ended has no window to bring forward and
        // nothing left to stop, and the same key that would have stopped it
        // forgets it instead.
        Some(Item::Agent(_)) => {
            let card = match screen.card.is_some() {
                true => ("space", "closes it"),
                false => ("space", "card"),
            };
            match list
                .selected()
                .is_some_and(|view| view.phase().is_terminal())
            {
                true => vec![card, ("ctrl+x", "forget")],
                false => vec![card, ("enter", "attach"), ("ctrl+x", "stop")],
            }
        }
        // A wall with nothing on it has no line under the cursor, and the one
        // key that changes that is the one worth the room.
        None => vec![("n", "starts one")],
    };
    said.extend([("ctrl+s", "axis"), ("q", "quit")]);
    said
}

/// Those keys on one row, cut to what a screen this wide can hold, with
/// `last` pinned to the end of it.
///
/// What goes is what is furthest from the pinned one, and the pinned one never
/// does: a hint clipped by the terminal reads as a key that ends where the
/// screen does. Walking the list, the one worth that place is `?`, which leads
/// to all the others; on a line being typed `?` is a character like any other
/// and there is no overlay to shed into, so the place goes to esc, because a
/// mode nobody can see the way out of is a mode they are stuck in.
fn fitted(said: &[Hint], last: Hint, width: usize) -> Line<'static> {
    let with = |kept: &[Hint]| {
        let mut all = kept.to_vec();
        all.push(last);
        all
    };

    let mut kept = said.to_vec();
    while !kept.is_empty() && spent(&with(&kept)) > width {
        kept.pop();
    }
    row(&with(&kept))
}

/// Those hints drawn: each key carrying the weight, what it does dim behind
/// it, and a gap of plain wall between one and the next.
pub(super) fn row(hints: &[Hint]) -> Line<'static> {
    let mut spans = Vec::new();
    for (key, does) in hints {
        if !spans.is_empty() {
            spans.push(Span::raw(GAP));
        }
        spans.push(Span::styled(*key, bold()));
        spans.push(Span::styled(format!(" {does}"), dim()));
    }
    Line::from(spans)
}

/// Those hints as the row draws them, text alone: a test reads one string back
/// off the screen, and how a pair is spelled and what stands between two of
/// them are this file's business rather than the caller's.
#[cfg(test)]
pub(in crate::tui) fn spelled(hints: &[Hint]) -> String {
    row(hints)
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

/// The cells that row takes, which is what the shedding is measured against.
fn spent(hints: &[Hint]) -> usize {
    let said: usize = hints
        .iter()
        .map(|(key, does)| key.chars().count() + 1 + does.chars().count())
        .sum();
    said + GAP.len() * hints.len().saturating_sub(1)
}

/// What stands between one hint and the next: wall, because the weight on the
/// key is already the edge and a character there would be a third thing to
/// read on a row that is meant to be glanced at.
const GAP: &str = "   ";

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
        return row(&ANSWERS);
    }
    // A question of the view's own is not advice and not a key: it is the one
    // thing on the screen, in the colour of something waiting on a person.
    if let Mode::Confirming(asked) = &screen.mode {
        return Line::styled(asked.question(), Style::new().fg(screen.theme.waiting));
    }
    let width = width as usize;
    match &screen.mode {
        Mode::List => fitted(&hints(screen), MORE, width),
        Mode::Keys => row(&[("any key", "goes back"), ("q", "quits")]),
        // A question up is the whole of this row, and is drawn above.
        Mode::Confirming(_) => fitted(&hints(screen), MORE, width),
        Mode::Typing(composer) if composer.narrows() => fitted(
            &[("enter", "narrows it"), ("s: or a:", "alone clears")],
            ("esc", "cancels"),
            width,
        ),
        Mode::Typing(composer) => match composer.asking {
            Asking::Task => {
                let mut said = vec![("enter", "starts it"), ("alt+enter", "newline")];
                // The dial on the rule above wears no label and says nothing
                // about the key that turns it, so this row does: a setting
                // nobody can find the key for is a setting nobody can change.
                // A vendor that declares no dial has none to name.
                if screen.profile.permission_dial().is_some() {
                    said.push(("shift+tab", "permission"));
                }
                // And the way out of the line for anybody whose task wants
                // more room than a row, which is nowhere else on the screen.
                said.push(("ctrl+g", "$EDITOR"));
                fitted(&said, ("esc", "cancels"), width)
            }
            Asking::Reply { .. } => fitted(
                &[("enter", "sends it"), ("alt+enter", "newline")],
                ("esc", "cancels"),
                width,
            ),
            Asking::Name { .. } => fitted(
                &[("enter", "renames it")],
                ("esc", "leaves it alone"),
                width,
            ),
        },
    }
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

    /// The rule over the line, wherever the band it heads has ended up.
    fn edge(screen: &Screen, size: (u16, u16)) -> String {
        painted(screen, size)
            .into_iter()
            .find(|row| row.starts_with("TASK") || row.starts_with("NARROW"))
            .expect("a rule over the line")
    }

    #[test]
    fn input_mode_sheds_the_rule_from_the_far_end_and_keeps_the_word_it_names() {
        for width in 20..=110 {
            let drawn = edge(&typing("port it"), (width, 30));
            assert_eq!(
                drawn.chars().count(),
                width as usize,
                "a rule that stops short of the edge is not a rule: {drawn:?}"
            );
        }

        // Room for all of it: which mode this is, the one law of it, and what
        // the next agent may do without asking.
        let whole = edge(&typing("port it"), (80, 30));
        assert!(
            whole.starts_with("TASK · letters are text until esc "),
            "{whole:?}"
        );
        assert!(whole.ends_with(" vendor default ──"), "{whole:?}");

        // The sentence is what goes first as the room runs out, and the dial
        // after it; the word the rule is named for never does.
        let tight = edge(&typing("port it"), (40, 30));
        assert!(tight.starts_with("TASK ─"), "{tight:?}");
        assert!(tight.ends_with(" vendor default ──"), "{tight:?}");

        let narrow = edge(&typing("port it"), (20, 30));
        assert!(narrow.starts_with("TASK ─"), "{narrow:?}");
        assert!(!narrow.contains("vendor"), "{narrow:?}");
    }

    #[test]
    fn input_mode_takes_the_weight_off_the_wall_it_is_drawn_over() {
        // Everything above the band the line is drawn in, which on a screen
        // this tall holding one line is every row but the last three.
        let weighty = |screen: &Screen| {
            let cells = cells(screen, TALL);
            (0..27).any(|row| {
                (0..TALL.0).any(|column| cells[(column, row)].modifier.contains(Modifier::BOLD))
            })
        };

        let mut screen = showing(a_fleet(), None);
        assert!(
            weighty(&screen),
            "the wall carries weight while the keys are still keys"
        );

        let mut composer = Composer::new(Asking::Task);
        composer.text = "port it".to_string();
        screen.mode = Mode::Typing(composer);
        assert!(
            !weighty(&screen),
            "and gives up every bit of it the moment a line is being typed"
        );

        // Dimmed rather than taken away: the wall is still the wall it was a
        // keystroke ago, and the rows are still on it to be read.
        let cells = cells(&screen, TALL);
        assert!(
            (0..TALL.0).all(|column| cells[(column, 0)].modifier.contains(Modifier::DIM)),
            "the header behind goes dim to its last cell"
        );
        assert!(
            painted(&screen, TALL)
                .iter()
                .any(|row| row.contains("ask-a1b")),
            "and the agents are still named on it"
        );
    }

    #[test]
    fn axis_says_a_line_that_narrows_will_narrow_rather_than_start_anything() {
        let mut screen = showing(Vec::new(), None);
        let mut composer = Composer::new(Asking::Task);
        composer.text = "s:waiting".to_string();
        screen.mode = Mode::Typing(composer);

        let painted = painted(&screen, (60, 6));
        assert!(
            painted[3].starts_with("NARROW ·"),
            "the rule over it says the same thing its edge does: {:?}",
            painted[3]
        );
        assert_eq!(painted[4], "❯ s:waiting█");
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
            "space card   enter attach   ctrl+x stop   ctrl+s axis   q quit   ? keys"
        );

        // One line up is the heading over it, where the same two keys do
        // something else entirely.
        screen.list.up();
        assert_eq!(
            hint_row(&screen, wide),
            "enter shuts it   ctrl+x clears the group   ctrl+s axis   q quit   ? keys"
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
            hint_row(&screen, wide).starts_with("space closes it   enter attach"),
            "{:?}",
            hint_row(&screen, wide)
        );

        // An agent whose command has ended has no window to bring forward and
        // nothing left to stop.
        let mut screen = showing(all_done(), None);
        screen.list.fit(5);
        screen.list.refit();
        let row = hint_row(&screen, wide);
        assert!(row.starts_with("space card   ctrl+x forget"), "{row:?}");
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
            "space card   enter attach   ctrl+x stop   ? keys"
        );
    }

    #[test]
    fn keymap_hints_on_a_line_being_typed_keep_the_way_out_of_it() {
        for width in 12..=80 {
            let row = hint_row(&typing("port it"), (width, 12));
            assert!(
                row.chars().count() <= width as usize,
                "a hint cut in half is a key that reads as another one: {row:?}"
            );
            assert!(
                row.ends_with("esc cancels"),
                "and the way out of the mode is the last thing to go: {row:?}"
            );
        }

        assert_eq!(
            hint_row(&typing("port it"), (100, 12)),
            "enter starts it   alt+enter newline   shift+tab permission   ctrl+g $EDITOR   esc cancels",
            "the key that turns the dial on the rule is named among them, and \
             the one that takes the line somewhere with room to write it"
        );

        // Where they will not all fit, the editor goes before the dial does:
        // a line can be typed without ever leaving for one, and the dial is
        // the only thing on the rule that a key changes.
        let tight = hint_row(&typing("port it"), (80, 12));
        assert!(tight.contains("shift+tab permission"), "{tight:?}");
        assert!(!tight.contains("ctrl+g"), "{tight:?}");
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
            hint.starts_with("❯ m:model"),
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
        assert!(clipped.starts_with("❯ m:model"), "{clipped}");
        assert!(clipped.trim_end().ends_with('…'), "{clipped}");

        // The next keystroke lands where the prompt ends, over the
        // placeholder, the way a browser draws a field's ghost text. The block
        // that stands there on a line with something on it gives way to what
        // the line is teaching.
        assert_eq!(caret(&typing(""), TALL), (2, 28));
        assert!(!clipped.contains('█'), "{clipped}");

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
        assert_eq!(one[28], "❯ port the importer█");
        assert_eq!(
            one[26], "",
            "one line takes one row, at the foot of it all, with the rule over it"
        );

        let three = painted(
            &typing("port the importer\nand its tests\nand the docs"),
            TALL,
        );
        assert_eq!(three[26], "❯ port the importer");
        assert_eq!(
            three[27], "  and its tests",
            "a row under the first is indented to it, so a task reads as one \
             thing"
        );
        assert_eq!(three[28], "  and the docs█");
        assert_eq!(
            caret(&typing("port it\nand test it"), TALL),
            (13, 28),
            "and the cursor is at the end of the last of them"
        );
    }

    #[test]
    fn composer_wrapping_past_the_width_grows_it_the_same_way_a_newline_does() {
        // Twice the room a sixty-column screen leaves beside the chevron.
        let painted = painted(&typing(&"x".repeat(116)), TALL);
        assert_eq!(painted[27], format!("❯ {}", "x".repeat(58)));
        assert_eq!(painted[28], format!("  {}", "x".repeat(58)));
    }

    #[test]
    fn composer_stops_growing_at_its_cap_and_scrolls_the_line_inside_it() {
        let screen = typing(&twenty_rows());
        let painted = painted(&screen, TALL);

        assert_eq!(
            painted[19], "❯ row-11",
            "the prompt is on the top row however far the rest has scrolled: \
             {painted:?}"
        );
        assert_eq!(painted[28], "  row-20█", "{painted:?}");
        assert!(
            !painted.iter().any(|line| line.contains("row-10")),
            "and what scrolled past is off the screen: {painted:?}"
        );
        assert_eq!(caret(&screen, TALL), (8, 28));
    }

    #[test]
    fn composer_leaves_the_list_it_was_opened_from_on_the_screen() {
        // A third of eight rows is two, whatever the line is holding, and the
        // agents are what the view is for.
        let painted = painted(&typing(&twenty_rows()), (60, 8));
        assert_eq!(painted[5], "❯ row-19");
        assert_eq!(painted[6], "  row-20█");
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
        assert_eq!(painted[4], "❯ port the importer█");
        assert!(painted[5].contains("enter starts it"), "{:?}", painted[5]);
        assert!(painted[5].contains("alt+enter newline"), "{:?}", painted[5]);
    }

    #[test]
    fn header_says_what_the_next_agent_may_do_without_asking() {
        let mut screen = launching(Vec::new());
        screen.mode = Mode::Typing(Composer::new(Asking::Task));

        let drawn = painted(&screen, (60, 8));
        assert!(
            drawn[5].starts_with("TASK · letters are text until esc"),
            "the rule names the mode and the one law of it: {:?}",
            drawn[5]
        );
        assert!(
            drawn[5].ends_with(" vendor default ──"),
            "and carries at its far end the layer, not a guess at which mode \
             claude would have picked: {:?}",
            drawn[5]
        );
        assert!(
            drawn[6].starts_with("❯ m:model"),
            "the empty line under it carries its placeholder: {:?}",
            drawn[6]
        );
        assert!(drawn[7].contains("enter starts it"), "{:?}", drawn[7]);

        screen.profile.permission = "acceptEdits".to_string();
        assert!(
            painted(&screen, (60, 8))[5].ends_with(" acceptEdits ──"),
            "and a mode in the vendor's own word for it: {:?}",
            painted(&screen, (60, 8))[5]
        );
    }

    #[test]
    fn header_keeps_the_permission_dial_to_the_lines_that_start_an_agent() {
        // The dial, and the key that turns it: the rule carries the one and
        // the row under the line names the other, and neither is said about a
        // line that will not start anything.
        let turned = |screen: &Screen| {
            painted(screen, (60, 8))
                .iter()
                .any(|line| line.contains("default ──") || line.contains("shift+tab"))
        };

        // A reply goes to an agent that is already running under whatever it
        // was started with, so the dial has nothing to say about it.
        let mut screen = launching(Vec::new());
        screen.mode = Mode::Typing(Composer::new(Asking::Reply {
            id: "ask-a1b".to_string(),
            question: true,
        }));
        assert!(!turned(&screen), "a reply is not a spawn");

        // Nor has it anything to say about a line that narrows the list.
        let mut composer = Composer::new(Asking::Task);
        composer.text = "s:waiting".to_string();
        screen.mode = Mode::Typing(composer);
        assert!(!turned(&screen));

        // A vendor amx has no entry for declares no permission dial: there is
        // nothing to say and nothing to turn, so the rule ends bare.
        screen.mode = Mode::Typing(Composer::new(Asking::Task));
        screen.profile.agent = "mock-claude".to_string();
        assert!(!turned(&screen));

        // And nothing is being typed at all, which is most of the time.
        let screen = launching(Vec::new());
        assert!(!turned(&screen));
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
            painted.iter().any(|line| line.contains("vendor default")),
            "{painted:?}"
        );
        assert!(painted[9].contains("enter starts it"), "{:?}", painted[9]);
    }
}
