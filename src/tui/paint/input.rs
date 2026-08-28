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
