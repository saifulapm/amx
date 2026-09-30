//! The typed line and the keys row at the foot of the screen.
//!
//! The typed line is a rule (the mode's name, a reminder that letters are
//! text until esc, and the permission dial in reverse video) over a composer
//! that grows a row at a time up to a cap. The same composer rows are drawn
//! at the foot of a card. Everything above the rule is dimmed while a line is
//! open. The keys row shows hints, a notice, the find line or a confirmation.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::card::{notes, pages};
use super::style::{bold, dim, prospective};
use super::text::{RULE, SEPARATOR, char_width, fit, width_of};
use crate::registry::DEFAULT;
use crate::theme::Theme;
use crate::tui::act::{Asking, Composer};
use crate::tui::rows::Item;
use crate::tui::{Mode, Screen};

/// A key hint: the key (bold) and what it does (dim).
///
/// The description is borrowed because some are built at draw time, such as
/// one naming the hunk under the cursor.
pub(super) type Hint<'a> = (&'static str, &'a str);

/// The hint the list's keys row always keeps.
const MORE: Hint<'static> = ("?", "keys");

/// The hint the keys row under a card always keeps.
const CLOSES: Hint<'static> = ("esc", "closes it");

/// The keys row while a `g` waits for its second press.
const WAITING_ON_A_G: [Hint<'static>; 2] = [
    ("g again", "the top of the list"),
    ("any other key", "carries on"),
];

/// A message in the keys row, by severity: a failure (red), a refusal
/// (amber), or advice (dim).
pub enum Notice {
    /// An action was attempted and failed.
    Failed(String),
    /// A request was deliberately not carried out.
    Refused(String),
    /// Advice, or confirmation that something worked.
    Advice(String),
}

/// The most rows the composer grows to before scrolling (also capped at a
/// third of the screen).
pub(super) const COMPOSER_CAP: usize = 10;

/// The composer's first-row prefix; later rows are indented to match. The
/// completion band uses the same indent.
pub(super) const GUTTER: &str = "❯ ";

/// The composer's text width at this band width.
pub(super) fn composer_room(width: u16) -> usize {
    (width as usize)
        .saturating_sub(GUTTER.chars().count())
        .max(1)
}

/// `text` word-wrapped into rows `room` cells wide.
///
/// Every `\n` starts a row, and an empty paragraph is an empty row (where the
/// cursor sits after a newline).
pub(super) fn composer_lines(text: &str, room: usize) -> Vec<String> {
    text.split('\n')
        .flat_map(|paragraph| cut(paragraph, room))
        .collect()
}

/// One paragraph of the line in the rows it takes, measured in cells (a wide
/// character is two).
///
/// Rows break after a space, so a word that does not fit moves to the next row
/// whole; a word longer than a row is broken where the row ends. Spaces may
/// hang past the edge. Every character lands on exactly one row, which is what
/// [`cursor_cell`] counts on.
fn cut(paragraph: &str, room: usize) -> Vec<String> {
    let mut rows = vec![String::new()];
    let mut used = 0;
    // Byte offset in the current row just past its last space.
    let mut after_space = None;
    for one in paragraph.chars() {
        let wide = char_width(one);
        let row = rows.last_mut().expect("there is always a row");
        if used > 0 && used + wide > room && one != ' ' {
            let carried = after_space.map(|at| row.split_off(at)).unwrap_or_default();
            used = width_of(&carried);
            rows.push(carried);
            after_space = None;
        }
        let row = rows.last_mut().expect("there is always a row");
        row.push(one);
        used += wide;
        if one == ' ' {
            after_space = Some(row.len());
        }
    }
    rows
}

/// The cursor's (row, char offset) in the rows [`composer_lines`] produces.
///
/// At the end of a full row the offset is one past the last char, which is
/// off screen; [`last_cell`] moves the block back onto the row.
pub(super) fn cursor_cell(composer: &Composer, room: usize) -> (u16, u16) {
    let mut left = composer.at.min(composer.text.chars().count());
    let mut row = 0;
    for paragraph in composer.text.split('\n') {
        let rows = cut(paragraph, room);
        let last = rows.len() - 1;
        for (down, text) in rows.iter().enumerate() {
            let length = text.chars().count();
            if left < length || (down == last && left == length) {
                return ((row + down) as u16, left as u16);
            }
            left -= length;
        }
        // Step over the newline between paragraphs.
        left -= 1;
        row += rows.len();
    }
    (row.saturating_sub(1) as u16, 0)
}

/// The rule's row above the composer.
const RULE_ROW: usize = 1;

/// Rows the typed-line band takes: the rule plus the composer's rows, capped,
/// leaving the list at least one row.
///
/// `chrome` is the rows every other band already takes.
pub(super) fn composer_height(composer: &Composer, area: Rect, chrome: u16) -> u16 {
    let room = (area.height.saturating_sub(chrome + 1) as usize).saturating_sub(RULE_ROW);
    let cap = COMPOSER_CAP.min(area.height as usize / 3).min(room).max(1);
    let rows = rows_of(composer, area.width).min(cap);
    (rows + RULE_ROW) as u16
}

/// Rows the composer's text needs at this width, uncapped, at least one.
pub(super) fn rows_of(composer: &Composer, width: u16) -> usize {
    composer_lines(&composer.text, composer_room(width))
        .len()
        .max(1)
}

/// The rule over the composer.
///
/// The mode label (and its target, see [`Composer::about`]) styled as
/// [`prospective`], then [`GLOSS`], then dashes to the edge with the
/// permission dial in reverse video near the end. When space runs out the
/// gloss goes first, then the dial; the label is only cut.
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
        width_of(&label)
            + width_of(gloss)
            + match dial.is_empty() {
                // No dial, no tail.
                true => 0,
                false => width_of(dial) + TAIL,
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
    let edge = edge_colour(composer, theme);
    let mut spans = vec![
        Span::styled(fit(&label, width), drawn),
        Span::styled(gloss, dim()),
        Span::styled(RULE.repeat(width.saturating_sub(taken(gloss, &dial))), edge),
    ];
    if !dial.is_empty() {
        spans.push(Span::styled(dial, drawn.add_modifier(Modifier::REVERSED)));
        spans.push(Span::styled(RULE.repeat(TAIL), edge));
    }
    Line::from(spans)
}

/// The reminder after the mode label.
const GLOSS: &str = "· letters are text until esc ";

/// Dashes after the dial, so it sits inside the rule.
const TAIL: usize = 2;

/// The style of the rule's dashes and the chevron: dim, or the accent (not
/// bold) on a `!` command line, which runs a shell instead of an agent.
fn edge_colour(composer: &Composer, theme: Theme) -> Style {
    match composer.commanding() {
        true => Style::new().fg(theme.accent),
        false => dim(),
    }
}

/// The typed-line band: dims everything above, then draws the rule and the
/// composer rows.
///
/// The terminal's cursor stays hidden; the block drawn by [`typed_rows`] is
/// the only cursor.
pub(super) fn composing_line(frame: &mut Frame, composer: &Composer, area: Rect, theme: Theme) {
    behind(frame, area.y);
    let [edge, band] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    frame.render_widget(
        Paragraph::new(rule(composer, area.width as usize, theme)),
        edge,
    );
    typed_rows(
        frame,
        composer,
        band,
        edge_colour(composer, theme),
        placeholder(composer),
        theme,
    );
}

/// The composer rows, in their own band or at the foot of a card.
///
/// The first row starts with [`GUTTER`] in `chevron`, later rows with its
/// width of spaces. The cursor is a reversed cell. On an empty line `hint` is
/// shown dim, with the block on its first cell. When the rows overflow the
/// band, the rows ending at the cursor are shown; the chevron stays on the top
/// shown row.
pub(super) fn typed_rows(
    frame: &mut Frame,
    composer: &Composer,
    band: Rect,
    chevron: Style,
    hint: Option<&str>,
    theme: Theme,
) {
    let prompt = GUTTER.to_string();
    let room = composer_room(band.width);
    let rows = composer_lines(&composer.text, room);
    let (row, column) = cursor_cell(composer, room);
    let (row, column) = (row as usize, column as usize);
    // Show the last rows, unless the cursor is above them.
    let from = rows.len().saturating_sub(band.height as usize).min(row);
    let shown = &rows[from..];

    let indent = " ".repeat(prompt.chars().count());
    let hint = hint.filter(|_| composer.text.is_empty());
    let lines: Vec<Line> = shown
        .iter()
        .enumerate()
        .map(|(down, text)| {
            let head = match down {
                0 => Span::styled(prompt.clone(), chevron),
                _ => Span::raw(indent.clone()),
            };
            let mut spans = vec![head];
            match hint.filter(|_| down == 0) {
                Some(hint) => {
                    spans.extend(under_the_block(&fit(hint, room), 0, dim(), Style::new()));
                }
                None if from + down == row => spans.extend(under_the_block(
                    text,
                    last_cell(text, column, room),
                    Style::new(),
                    Style::new().fg(theme.accent),
                )),
                None => spans.push(Span::styled(text.clone(), Style::new())),
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), band);
}

/// Which character of a row the block stands on for a cursor `column` chars
/// along it: that one, unless its cell is past the edge (the end of a full
/// row, or a space hanging off it), where it stands on the last character
/// still on screen.
fn last_cell(text: &str, column: usize, room: usize) -> usize {
    // Cells before the character at `column`, and the last one before it that
    // starts on screen.
    let mut start = 0;
    let mut last = 0;
    for (at, one) in text.chars().enumerate() {
        if at == column {
            break;
        }
        if start < room {
            last = at;
        }
        start += char_width(one);
    }
    match start >= room {
        true => last,
        false => column,
    }
}

/// `text` in `paint`, with the char at `column` reversed in `block` as the
/// cursor. Past the end, a reversed space.
///
/// Reversing keeps the char under the cursor readable.
pub(super) fn under_the_block(
    text: &str,
    column: usize,
    paint: Style,
    block: Style,
) -> Vec<Span<'static>> {
    let line: Vec<char> = text.chars().collect();
    let before: String = line.iter().take(column).collect();
    let on = line
        .get(column)
        .map_or_else(|| PAST_THE_END.to_string(), char::to_string);
    vec![
        Span::styled(before, paint),
        Span::styled(on, block.add_modifier(Modifier::REVERSED)),
        Span::styled(line.iter().skip(column + 1).collect::<String>(), paint),
    ]
}

/// The cursor cell past the end of the text.
const PAST_THE_END: &str = " ";

/// Dim every row above `until` and strip its bold and reverse video, to show
/// the keyboard now belongs to the line (or card) below.
///
/// Applied to the drawn buffer, so the other bands need no mode flag.
pub(super) fn behind(frame: &mut Frame, until: u16) {
    let wall = Rect {
        height: until,
        ..frame.area()
    };
    frame.buffer_mut().set_style(
        wall,
        dim().remove_modifier(Modifier::BOLD | Modifier::REVERSED),
    );
}

/// Store the permission dial's text on the composer for [`rule`] to draw.
///
/// Only a task line gets one, not a `!` command, and only when the vendor
/// declares a permission dial. At the default it reads "vendor default",
/// since amx does not know the vendor's configured mode.
pub(super) fn permission(screen: &Screen) {
    let Mode::Typing(composer) = &screen.mode else {
        return;
    };
    composer.allowed.set(
        (matches!(composer.asking, Asking::Task)
            && !composer.commanding()
            && screen.profile.permission_dial().is_some())
        .then(|| match screen.profile.permission.as_str() {
            DEFAULT => "vendor default".to_string(),
            mode => mode.to_string(),
        }),
    );
}

/// Placeholder text for an empty task line, listing the prefixes it accepts.
/// Other lines accept no prefixes and get none.
fn placeholder(composer: &Composer) -> Option<&'static str> {
    if !matches!(composer.asking, Asking::Task) || !composer.text.is_empty() {
        return None;
    }
    Some("!command · m:model · p:permission · w:on|off|changes · d:directory · agent:command")
}

/// The list's key hints, led by what the keys do on the item under the
/// cursor.
fn hints(screen: &Screen) -> Vec<Hint<'static>> {
    let list = &screen.list;
    let mut said = match list.items().get(list.cursor()) {
        Some(Item::Heading(..)) => vec![enters(screen), ("ctrl+x", "clears the group")],
        Some(Item::Fold(..) | Item::Sub(..)) => vec![enters(screen)],
        // The cursor never rests on a blank.
        Some(Item::Blank) => Vec::new(),
        // An ended agent cannot be attached or stopped; ctrl+x forgets it.
        Some(Item::Agent(_)) => {
            let card = match screen.card.is_some() {
                true => ("space", "closes it"),
                false => ("space", "card"),
            };
            let pin = match list.selected().is_some_and(|view| list.holding(view)) {
                true => ("ctrl+t", "unpin"),
                false => ("ctrl+t", "pin"),
            };
            match list
                .selected()
                .is_some_and(|view| view.phase().is_terminal())
            {
                true => vec![card, ("ctrl+x", "forget"), pin],
                false => vec![card, enters(screen), ("ctrl+x", "stop"), pin],
            }
        }
        None => vec![("n", "starts one")],
    };
    said.extend([("ctrl+s", "axis"), ("q", "quit")]);
    said
}

/// What enter does on the item under the cursor: open or shut a heading's
/// group, unfold a fold, or attach to an agent. Used under the list and under
/// a card's empty line.
fn enters(screen: &Screen) -> Hint<'static> {
    match screen.list.items().get(screen.list.cursor()) {
        Some(Item::Heading(_, tally)) => match tally.shut {
            true => ("enter", "opens it"),
            false => ("enter", "shuts it"),
        },
        Some(Item::Fold(..) | Item::Sub(..)) => ("enter", "shows them"),
        _ => ("enter", "attach"),
    }
}

/// The keys row under a card.
///
/// With the line empty, space and enter act on the list, plus `pgup` when the
/// body has more than one page. Once something is typed the row says what
/// enter will send, and adds alt+enter. While a review is being written,
/// enter counts the notes it would send and esc says how many it would drop.
fn card_keys(screen: &Screen, composer: &Composer, width: usize) -> Line<'static> {
    let going = screen.noted(&composer.text);
    let kept = screen.scroll.noted().len();
    let drops = format!("drops {}", notes(kept));
    let closes = match kept {
        0 => CLOSES,
        _ => ("esc", drops.as_str()),
    };

    if !composer.text.is_empty() {
        // Enter answers a question, resumes an ended agent, sends a message,
        // or sends the review (naming the hunk when it is the only note).
        let resumes = screen
            .card
            .as_ref()
            .is_some_and(|card| card.listening && card.phase.is_terminal());
        let does = match screen.card.as_ref().is_some_and(|card| card.asks()) {
            true => "answers it".to_string(),
            false => match (going.as_slice(), screen.at_hunk()) {
                ([], _) if resumes => "resumes it".to_string(),
                ([], _) => "sends it".to_string(),
                ([one], Some((at, _))) if *one == at => format!("sends it with hunk {}", at + 1),
                (going, _) => format!("sends {}", notes(going.len())),
            },
        };
        let mut said = vec![("enter", does.as_str()), ("alt+enter", "newline")];
        // Keeps the words as a note on this hunk. Last, so it is shed first.
        if screen.card.as_ref().is_some_and(|card| card.changes) {
            said.push(("ctrl+n", "keeps it"));
        }
        return fitted(&said, closes, width);
    }

    // With notes kept, enter on an empty line sends them.
    let sends = format!("sends {}", notes(going.len()));
    let mut said = match going.is_empty() {
        true => vec![enters(screen), ("space", "closes it")],
        false => vec![("enter", sends.as_str()), ("space", "closes it")],
    };
    if screen
        .card
        .as_ref()
        .is_some_and(|card| pages(card, &screen.scroll))
    {
        said.push(("pgup", "pages it"));
    }
    fitted(&said, closes, width)
}

/// Hints on one row within `width`, dropping from the end of `said` until
/// they fit. `last` is always kept at the end: `?` on the list, the way out
/// elsewhere.
fn fitted<'a>(said: &[Hint<'a>], last: Hint<'a>, width: usize) -> Line<'static> {
    let with = |kept: &[Hint<'a>]| -> Vec<Hint<'a>> {
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

/// Hints drawn on one row, separated by [`GAP`].
pub(super) fn row(hints: &[Hint<'_>]) -> Line<'static> {
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

/// Columns [`row`] would take for these hints.
fn spent(hints: &[Hint<'_>]) -> usize {
    let said: usize = hints
        .iter()
        .map(|(key, does)| key.chars().count() + 1 + does.chars().count())
        .sum();
    said + GAP.len() * hints.len().saturating_sub(1)
}

/// Gap between hints.
const GAP: &str = "   ";

/// The find line, if one is open.
pub(super) fn finding(screen: &Screen) -> Option<&Composer> {
    match &screen.mode {
        Mode::Typing(composer) if matches!(composer.asking, Asking::Find) => Some(composer),
        _ => None,
    }
}

/// The find line's prefix, the key that opens it.
const FIND: &str = "/";

/// Placeholder for an empty find line. It also matches ids and pull request
/// numbers; the task is named because it is not shown on the row. Must fit a
/// 60-column terminal whole.
const FINDING: &str = "a name or task, or s:state · enter keeps · esc clears";

/// The find line, drawn in the keys row with a cursor block.
///
/// No rule and no dimming: the list narrows as it is typed and must stay
/// readable.
fn find_row(line: &Composer, width: usize) -> Line<'static> {
    let mut spans = vec![Span::styled(FIND, dim())];
    match line.text.is_empty() {
        true => spans.push(Span::styled(
            fit(FINDING, width.saturating_sub(FIND.len())),
            dim(),
        )),
        false => spans.extend(under_the_block(
            &fit(&line.text, width.saturating_sub(FIND.len() + 1)),
            line.at,
            Style::new(),
            bold(),
        )),
    }
    Line::from(spans)
}

/// The keys row. In priority order: a pending `g`, a notice, the card's keys
/// (see [`card_keys`]), the find line, a confirmation question, then the
/// mode's hints.
pub(super) fn footer(screen: &Screen, width: u16) -> Line<'static> {
    // A pending `g` changes nothing else on screen, so it must show here.
    if screen.going {
        return row(&WAITING_ON_A_G);
    }
    if let Some(notice) = &screen.notice {
        return match notice {
            Notice::Failed(said) => {
                Line::styled(said.clone(), Style::new().fg(screen.theme.failed))
            }
            // The waiting colour, as on an armed row.
            Notice::Refused(said) => {
                Line::styled(said.clone(), Style::new().fg(screen.theme.waiting))
            }
            Notice::Advice(said) => Line::styled(said.clone(), dim()),
        };
    }
    if let Some(composer) = screen.answering() {
        return card_keys(screen, composer, width as usize);
    }
    // The find line takes no band, so the list keeps all its rows.
    if let Some(line) = finding(screen) {
        return find_row(line, width as usize);
    }
    if let Mode::Confirming(asked) = &screen.mode {
        return Line::styled(asked.question(), Style::new().fg(screen.theme.waiting));
    }
    let width = width as usize;
    match &screen.mode {
        Mode::List => fitted(&hints(screen), MORE, width),
        Mode::Keys => match screen.keymap.finding() {
            // Letters go to the search; only enter and esc do anything else.
            true => row(&[("enter", "keeps it"), ("esc", "drops it")]),
            false => fitted(
                &[("j k", "scroll"), ("/", "finds one"), ("q", "quits")],
                ("any key", "goes back"),
                width,
            ),
        },
        // Unreachable: a confirmation is drawn above.
        Mode::Confirming(_) => fitted(&hints(screen), MORE, width),
        Mode::Typing(composer) => match composer.asking {
            Asking::Task => {
                let enter = match composer.commanding() {
                    true => ("enter", "runs it"),
                    false => ("enter", "starts it"),
                };
                let mut said = vec![enter, ("alt+enter", "newline")];
                // The dial on the rule has no label, so name its key here.
                if !composer.commanding() && screen.profile.permission_dial().is_some() {
                    said.push(("shift+tab", "permission"));
                }
                said.push(("ctrl+g", "$EDITOR"));
                fitted(&said, ("esc", "cancels"), width)
            }
            Asking::Reply => fitted(
                &[("enter", "sends it"), ("alt+enter", "newline")],
                ("esc", "cancels"),
                width,
            ),
            Asking::Name { .. } => fitted(
                &[("enter", "renames it")],
                ("esc", "leaves it alone"),
                width,
            ),
            // Enter on an empty fork line starts a copy with no first turn.
            Asking::Fork { .. } => fitted(
                &[("enter", "starts the copy"), ("empty", "no first turn")],
                ("esc", "cancels"),
                width,
            ),
            // Unreachable: the find line is drawn above.
            Asking::Find => find_row(composer, width),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::View;
    use crate::store::{Kind, Phase};
    use crate::tui::paint::Card;
    use crate::tui::paint::empty::WELCOME;
    use crate::tui::paint::fixtures::{
        a_fleet, asking, block, cells, launching, painted, showing, theme, view,
    };
    use ratatui::style::{Color, Modifier};

    /// The modifiers of `word`'s first cell, on the first row that has it.
    fn word_modifier(screen: &Screen, size: (u16, u16), word: &str) -> Modifier {
        let lines = painted(screen, size);
        let (row, line) = lines
            .iter()
            .enumerate()
            .find(|(_, line)| line.contains(word))
            .unwrap_or_else(|| panic!("{word:?} is on none of {lines:?}"));
        let at = line.find(word).expect("the word on the row");
        cells(screen, size)[(line[..at].chars().count() as u16, row as u16)].modifier
    }

    /// The keys row, the screen's last.
    fn hint_row(screen: &Screen, size: (u16, u16)) -> String {
        painted(screen, size).pop().expect("a row for the keys")
    }

    /// The view with `text` typed on the find line.
    fn seeking(text: &str) -> Screen {
        let mut screen = showing(a_fleet(), None);
        let mut composer = Composer::new(Asking::Find);
        composer.insert(text);
        screen.mode = Mode::Typing(composer);
        screen
    }

    #[test]
    fn find_stands_on_the_keys_row_and_leaves_the_wall_alone() {
        let empty = painted(&seeking(""), TALL);
        assert_eq!(
            empty[29], "/a name or task, or s:state · enter keeps · esc clears",
            "whole on the sixty columns a narrow terminal has: ghost text cut \
             off is a lesson half taught"
        );

        let typed = painted(&seeking("port"), TALL);
        assert_eq!(typed[29], "/port");
        assert_eq!(
            block(&seeking("port"), TALL, 29),
            Some(5),
            "with the block turning over the cell the next character lands in"
        );

        // No rule and no band of its own.
        assert!(
            !typed.iter().any(|row| row.starts_with("FIND")),
            "no rule over it: {typed:?}"
        );
        assert!(
            typed.iter().any(|row| row.contains("ask-a1b")),
            "and the agents are still on the wall: {typed:?}"
        );

        // And no dimming, unlike every other typed line.
        assert!(
            !word_modifier(&seeking("port"), TALL, "ask-a1b").contains(Modifier::DIM),
            "the wall keeps its strength while a find is open"
        );
    }

    #[test]
    fn keymap_a_g_waiting_for_its_second_says_so_where_the_keys_are() {
        let mut screen = showing(a_fleet(), None);
        let wide = (80, 12);
        assert!(
            !hint_row(&screen, wide).contains("g again"),
            "nothing is waiting yet"
        );

        screen.going = true;
        let row = hint_row(&screen, wide);
        assert!(row.starts_with("g again the top of the list"), "{row:?}");
        assert!(
            row.contains("any other key carries on"),
            "and the way out of it: {row:?}"
        );
    }

    /// Finished agents, two more than the fold shows.
    fn all_done() -> Vec<View> {
        (0..crate::tui::rows::FOLD_AT + 2)
            .map(|n| view(&format!("done-{n:02}"), Phase::Done, Some("did it"), 60))
            .collect()
    }

    /// Tall enough for the composer to reach its cap (a third of 30 rows).
    const TALL: (u16, u16) = (60, 30);

    /// An empty view with `text` typed on a task line.
    fn typing(text: &str) -> Screen {
        let mut screen = showing(Vec::new(), None);
        let mut composer = Composer::new(Asking::Task);
        composer.insert(text);
        screen.mode = Mode::Typing(composer);
        screen
    }

    /// Twenty paragraphs, more rows than the cap.
    fn twenty_rows() -> String {
        (1..=20)
            .map(|n| format!("row-{n:02}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The rule over the task line.
    fn edge(screen: &Screen, size: (u16, u16)) -> String {
        painted(screen, size)
            .into_iter()
            .find(|row| row.starts_with("TASK"))
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

        // Wide enough for label, gloss and dial.
        let whole = edge(&typing("port it"), (80, 30));
        assert!(
            whole.starts_with("TASK · letters are text until esc "),
            "{whole:?}"
        );
        assert!(whole.ends_with(" vendor default ┈┈"), "{whole:?}");

        // The gloss goes first, then the dial; the label stays.
        let tight = edge(&typing("port it"), (40, 30));
        assert!(tight.starts_with("TASK ┈"), "{tight:?}");
        assert!(tight.ends_with(" vendor default ┈┈"), "{tight:?}");

        let narrow = edge(&typing("port it"), (20, 30));
        assert!(narrow.starts_with("TASK ┈"), "{narrow:?}");
        assert!(!narrow.contains("vendor"), "{narrow:?}");
    }

    #[test]
    fn input_mode_measures_a_wide_directory_on_the_rule_in_columns() {
        let mut screen = typing("port it");
        if let Mode::Typing(composer) = &mut screen.mode {
            composer.under = Some(std::path::PathBuf::from("/srv/日本語"));
        }
        let drawn = edge(&screen, (80, 30));
        assert!(drawn.ends_with(" vendor default ┈┈"), "{drawn:?}");
    }

    #[test]
    fn input_mode_takes_the_strength_off_the_wall_it_is_drawn_over() {
        // Every row above the band: all but the rule, the line, the blank row
        // and the keys.
        let weighty = |screen: &Screen| {
            let cells = cells(screen, TALL);
            (0..26).any(|row| {
                (0..TALL.0).any(|column| cells[(column, row)].modifier.contains(Modifier::BOLD))
            })
        };

        let mut screen = showing(a_fleet(), None);
        assert!(
            weighty(&screen),
            "the band above the wall carries weight while the keys are still keys"
        );
        assert!(
            !word_modifier(&screen, TALL, "ask-a1b").contains(Modifier::DIM),
            "and the wall spends none of it, so what it has to give up is the \
             strength on the name under the cursor"
        );

        let mut composer = Composer::new(Asking::Task);
        composer.insert("port it");
        screen.mode = Mode::Typing(composer);
        assert!(
            !weighty(&screen),
            "the moment a line is being typed every bit of the weight goes"
        );
        assert!(
            word_modifier(&screen, TALL, "ask-a1b").contains(Modifier::DIM),
            "and the row somebody was working with goes quiet with the rest"
        );

        // Dimmed, not hidden.
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
    fn axis_says_a_line_of_state_tokens_will_start_an_agent_like_any_other() {
        let mut screen = showing(Vec::new(), None);
        let mut composer = Composer::new(Asking::Task);
        composer.insert("s:waiting");
        screen.mode = Mode::Typing(composer);

        // `s:` only narrows on the find line; here it is part of a task.
        let painted = painted(&screen, (60, 6));
        assert!(
            painted[3].starts_with("TASK ·"),
            "the rule over it says the same thing its edge does: {:?}",
            painted[3]
        );
        assert_eq!(painted[4], "❯ s:waiting");
        assert!(painted[5].contains("enter starts it"), "{:?}", painted[5]);
        assert!(
            !painted[5].contains("narrows it"),
            "a hint that says the other thing is a hint that lies: {:?}",
            painted[5]
        );
    }

    #[test]
    fn keymap_hints_are_the_keys_the_line_under_the_cursor_answers_to() {
        let wide = (80, 12);
        let mut screen = showing(a_fleet(), None);

        // On an agent's row.
        assert_eq!(
            hint_row(&screen, wide),
            "space card   enter attach   ctrl+x stop   ctrl+t pin   ctrl+s axis   ? keys"
        );

        // On its heading.
        screen.list.up();
        assert_eq!(
            hint_row(&screen, wide),
            "enter shuts it   ctrl+x clears the group   ctrl+s axis   q quit   ? keys"
        );

        // On a shut heading.
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

        // With a card up, space closes it.
        let mut screen = showing(a_fleet(), None);
        screen.card = Some(asking(&[], None).read());
        assert!(
            hint_row(&screen, wide).starts_with("space closes it   enter attach"),
            "{:?}",
            hint_row(&screen, wide)
        );

        // An ended agent cannot be attached or stopped.
        let mut screen = showing(all_done(), None);
        let row = hint_row(&screen, wide);
        assert!(row.starts_with("space card   ctrl+x forget"), "{row:?}");
        assert!(!row.contains("attach"), "{row:?}");

        // On the fold, enter unfolds.
        for _ in 0..crate::tui::rows::FOLD_AT {
            screen.list.down();
        }
        assert!(
            hint_row(&screen, wide).starts_with("enter shows them"),
            "{:?}",
            hint_row(&screen, wide)
        );

        // On a pinned row, ctrl+t unpins.
        let mut screen = showing(a_fleet(), None);
        assert!(screen.list.hold_or_let_go());
        let row = hint_row(&screen, wide);
        assert!(row.contains("ctrl+t unpin"), "{row:?}");

        // An empty list.
        let screen = showing(Vec::new(), None);
        assert!(
            hint_row(&screen, wide).starts_with("n starts one"),
            "{:?}",
            hint_row(&screen, wide)
        );
    }

    /// The view with `card` up and `typed` on its line.
    fn carded(card: Card, typed: &str) -> Screen {
        let mut screen = showing(a_fleet(), Some(card));
        let mut composer = Composer::new(Asking::Reply);
        composer.insert(typed);
        screen.mode = Mode::Typing(composer);
        screen
    }

    /// A card whose body is longer than any card, so it pages.
    fn a_long_answer() -> Card {
        Card {
            phase: Phase::Done,
            question: None,
            options: Vec::new(),
            body: (1..=40)
                .map(|n| format!("row {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
            answer: true,
            ..asking(&[], None)
        }
    }

    #[test]
    fn keymap_the_keys_under_a_card_are_the_cards_while_its_line_is_empty() {
        let wide = (80, 14);

        // Empty line: enter and space act on the list; esc stays pinned last.
        let mut screen = carded(asking(&[], None), "");
        assert_eq!(
            hint_row(&screen, wide),
            "enter attach   space closes it   esc closes it"
        );

        // `pgup` only when the body has more than a page.
        assert_eq!(
            hint_row(&carded(a_long_answer(), ""), wide),
            "enter attach   space closes it   pgup pages it   esc closes it"
        );

        // On a heading, enter shuts it.
        screen.list.up();
        assert!(
            hint_row(&screen, wide).starts_with("enter shuts it   space closes it"),
            "{:?}",
            hint_row(&screen, wide)
        );
    }

    #[test]
    fn keymap_the_keys_under_a_card_are_the_lines_the_moment_it_holds_a_word() {
        let wide = (80, 14);

        // Enter answers a question.
        assert_eq!(
            hint_row(
                &carded(asking(&["the sqlite one"], Some(Kind::Question)), "keep it"),
                wide
            ),
            "enter answers it   alt+enter newline   esc closes it"
        );
        // It sends to a live agent and resumes an ended one.
        let between_turns = Card {
            phase: Phase::Idle,
            ..a_long_answer()
        };
        assert_eq!(
            hint_row(&carded(between_turns, "keep it"), wide),
            "enter sends it   alt+enter newline   esc closes it"
        );
        assert_eq!(
            hint_row(&carded(a_long_answer(), "keep it"), wide),
            "enter resumes it   alt+enter newline   esc closes it"
        );
    }

    /// A patch card with two files, one hunk each.
    fn a_patch() -> Card {
        Card {
            phase: Phase::Working,
            question: None,
            options: Vec::new(),
            body: "diff --git a/src/foo.rs b/src/foo.rs\n\
                   --- a/src/foo.rs\n\
                   +++ b/src/foo.rs\n\
                   @@ -1,2 +1,3 @@\n \
                   context\n\
                   +added\n\
                   diff --git a/src/bar.rs b/src/bar.rs\n\
                   --- a/src/bar.rs\n\
                   +++ b/src/bar.rs\n\
                   @@ -8,1 +8,2 @@\n \
                   done\n\
                   +and more\n"
                .to_string(),
            changes: true,
            ..asking(&[], None)
        }
    }

    #[test]
    fn keymap_the_line_under_a_patch_says_the_hunk_the_words_will_go_with() {
        let wide = (80, 14);

        // No hunk selected: the words are sent as typed. ctrl+n only appears
        // on a patch.
        let screen = carded(a_patch(), "why this row?");
        assert_eq!(
            hint_row(&screen, wide),
            "enter sends it   alt+enter newline   ctrl+n keeps it   esc closes it"
        );

        // On a hunk, enter names it.
        let card = screen.card.as_ref().expect("the card");
        screen.scroll.to_hunk(card.body.hunks(), true);
        assert_eq!(
            hint_row(&screen, wide),
            "enter sends it with hunk 1   alt+enter newline   ctrl+n keeps it   esc closes it"
        );

        // With a kept note, enter counts the notes and esc says what it drops.
        screen.scroll.remark(Some(1), "this file can go");
        assert_eq!(
            hint_row(&screen, wide),
            "enter sends 2 notes   alt+enter newline   ctrl+n keeps it   esc drops 1 note"
        );
    }

    #[test]
    fn keymap_an_empty_line_over_a_review_says_enter_sends_it() {
        let wide = (80, 14);

        // No notes: the same row as under any card.
        let screen = carded(a_patch(), "");
        assert_eq!(
            hint_row(&screen, wide),
            "enter attach   space closes it   pgup pages it   esc closes it"
        );

        // With notes kept, enter sends them and esc drops them.
        screen.scroll.remark(Some(0), "why this row?");
        screen.scroll.remark(Some(1), "this file can go");
        assert_eq!(
            hint_row(&screen, wide),
            "enter sends 2 notes   space closes it   pgup pages it   esc drops 2 notes"
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

        // Hints go from the far end.
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

        // The editor hint goes before the permission hint.
        let tight = hint_row(&typing("port it"), (80, 12));
        assert!(tight.contains("shift+tab permission"), "{tight:?}");
        assert!(!tight.contains("ctrl+g"), "{tight:?}");
    }

    #[test]
    fn view_says_what_it_could_not_do_where_the_keys_are() {
        let mut screen = showing(Vec::new(), None);
        screen.notice = Some(Notice::Advice(
            "fix-login-a1b no longer has a pane".to_string(),
        ));

        let painted = painted(&screen, (60, 6));
        assert_eq!(painted[5], "fix-login-a1b no longer has a pane");
    }

    #[test]
    fn glyphs_and_notices_tell_a_failure_from_advice() {
        // The first cell of the keys row.
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
        assert_eq!(
            said(Notice::Refused(
                "keeping fix-login-a1b: it has uncommitted changes".to_string()
            )),
            (theme().waiting, Modifier::empty()),
            "and a thing somebody asked for that did not happen is neither: \
             it is the colour of something standing between them and it"
        );
    }

    #[test]
    fn composer_an_empty_task_line_names_its_own_prefixes() {
        // Wide enough for the whole placeholder.
        let empty = painted(&typing(""), (110, 30));
        let hint = empty
            .iter()
            .find(|row| row.contains("m:model"))
            .expect("the empty line teaches its prefixes");
        assert!(
            hint.starts_with("❯ !command"),
            "the hint is a placeholder on the line itself, not a row of its \
             own, and the mark that leads the line leads it: {hint}"
        );
        for named in [
            "!command",
            "m:model",
            "p:permission",
            "w:on|off|changes",
            "d:directory",
            "agent:command",
        ] {
            assert!(hint.contains(named), "{named} is not taught: {hint}");
        }
        assert!(
            !hint.contains("s:state"),
            "and not the one token this line no longer reads: {hint}"
        );
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
        assert!(clipped.starts_with("❯ !command"), "{clipped}");
        assert!(clipped.trim_end().ends_with('…'), "{clipped}");

        // The block reverses the placeholder's first cell.
        assert_eq!(block(&typing(""), TALL, 27), Some(2));
        assert!(!clipped.contains('█'), "{clipped}");

        // The first character typed removes it.
        let typed = painted(&typing("p"), TALL);
        assert!(
            !typed.iter().any(|row| row.contains("m:model")),
            "{typed:?}"
        );

        // A reply takes no prefixes.
        let mut replying = showing(Vec::new(), Some(asking(&[], None)));
        replying.mode = Mode::Typing(Composer::new(Asking::Reply));
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
    fn composer_wraps_at_word_boundaries_and_breaks_only_an_overlong_word() {
        assert_eq!(
            composer_lines("you need to attach it", 10),
            ["you need ", "to attach ", "it"],
            "a word that does not fit moves down whole"
        );
        assert_eq!(
            composer_lines("see abcdefghijkl", 6),
            ["see ", "abcdef", "ghijkl"],
            "a word longer than a row breaks where the row ends"
        );
        assert_eq!(
            composer_lines("ab    cd", 3),
            ["ab    ", "cd"],
            "spaces hang past the edge rather than start a row"
        );
        let text = "Let me know if you have any question";
        assert_eq!(
            composer_lines(text, 12).concat(),
            text,
            "no character is lost"
        );
    }

    #[test]
    fn the_block_never_stands_past_the_edge_of_a_row() {
        assert_eq!(last_cell("ab    ", 4, 3), 2, "on a hanging space");
        assert_eq!(last_cell("abc", 3, 3), 2, "at the end of a full row");
        assert_eq!(last_cell("abc", 1, 3), 1);
        assert_eq!(last_cell("ab", 2, 3), 2, "with room left after it");
    }

    #[test]
    fn composer_grows_a_row_at_a_time_as_the_line_it_holds_does() {
        let one = painted(&typing("port the importer"), TALL);
        assert_eq!(one[27], "❯ port the importer");
        assert_eq!(
            one[25], "",
            "one line takes one row, at the foot of it all, with the rule over it"
        );

        let three = painted(
            &typing("port the importer\nand its tests\nand the docs"),
            TALL,
        );
        assert_eq!(three[25], "❯ port the importer");
        assert_eq!(
            three[26], "  and its tests",
            "a row under the first is indented to it, so a task reads as one \
             thing"
        );
        assert_eq!(three[27], "  and the docs");
        assert_eq!(
            block(&typing("port it\nand test it"), TALL, 27),
            Some(13),
            "and the block is at the end of the last of them"
        );
    }

    #[test]
    fn wide_text_wraps_where_its_cells_run_out_with_the_block_after_it() {
        // 48 wide chars are 96 cells; a 60-column line has 58 for text, so
        // 29 chars per row.
        let line = "日本語の文章".repeat(8);
        let painted = painted(&typing(&line), TALL);
        // A wide char's second cell reads back as a space.
        let cells = |chars: Vec<char>| {
            let row: String = chars.iter().map(|one| format!("{one} ")).collect();
            row.trim_end().to_string()
        };
        let first = cells(line.chars().take(29).collect());
        let second = cells(line.chars().skip(29).collect());
        assert_eq!(painted[26], format!("❯ {first}"), "{painted:?}");
        assert_eq!(painted[27], format!("  {second}"), "{painted:?}");
        assert_eq!(
            block(&typing(&line), TALL, 27),
            Some(2 + 19 * 2),
            "the block stands in the cell after the last character"
        );
    }

    /// [`typing`] with the cursor moved `back` chars left.
    fn typing_at(text: &str, back: usize) -> Screen {
        let mut screen = typing(text);
        if let Mode::Typing(composer) = &mut screen.mode {
            for _ in 0..back {
                composer.left();
            }
        }
        screen
    }

    #[test]
    fn composer_stands_the_block_on_the_character_the_cursor_is_on() {
        // On the "r" of "rter".
        let screen = typing_at("port the importer", 4);
        let painted = painted(&screen, TALL);
        assert_eq!(
            painted[27], "❯ port the importer",
            "the character keeps its cell, so nothing is hidden by the cursor \
             standing on it: {painted:?}"
        );

        let cell = cells(&screen, TALL)[(15, 27)].clone();
        assert_eq!(cell.symbol(), "r");
        assert!(
            cell.modifier.contains(Modifier::REVERSED),
            "the block is set behind it: {:?}",
            cell.modifier
        );
        assert_eq!(
            cell.fg,
            theme().accent,
            "in the colour the block has at the end of a line"
        );

        // Across a wrap the block moves to the row above.
        assert_eq!(
            block(&typing_at(&"x".repeat(116), 58), TALL, 27),
            Some(2),
            "the first character of the second row"
        );
        assert_eq!(
            block(&typing_at(&"x".repeat(116), 59), TALL, 26),
            Some(59),
            "and the one before it is the last of the first row"
        );
    }

    #[test]
    fn composer_draws_the_line_at_the_weight_the_rest_of_the_view_is_typed_at() {
        // Three rows, cursor inside the last, so both kinds of row are drawn.
        let screen = typing_at("port the importer\nand its tests\nand the docs", 4);
        let cells = cells(&screen, TALL);
        for row in 25..=27 {
            assert!(
                (0..TALL.0).all(|column| !cells[(column, row)].modifier.contains(Modifier::BOLD)),
                "no cell of the line being typed carries weight: row {row}"
            );
        }

        // The block is reversed, in the accent.
        let cell = cells[(10, 27)].clone();
        assert_eq!(cell.symbol(), "d");
        assert!(
            cell.modifier.contains(Modifier::REVERSED),
            "{:?}",
            cell.modifier
        );
        assert_eq!(cell.fg, theme().accent);

        // The keys row keeps its bold keys.
        assert!(
            (0..TALL.0).any(|column| cells[(column, 29)].modifier.contains(Modifier::BOLD)),
            "the keys under the line are still keys"
        );
    }

    #[test]
    fn composer_wrapping_past_the_width_grows_it_the_same_way_a_newline_does() {
        // Two rows' worth of text at 60 columns.
        let painted = painted(&typing(&"x".repeat(116)), TALL);
        assert_eq!(painted[26], format!("❯ {}", "x".repeat(58)));
        assert_eq!(painted[27], format!("  {}", "x".repeat(58)));

        // At the end of a full row the block sits on its last cell.
        assert_eq!(block(&typing(&"x".repeat(58)), TALL, 27), Some(59));
    }

    #[test]
    fn composer_stops_growing_at_its_cap_and_scrolls_the_line_inside_it() {
        let screen = typing(&twenty_rows());
        let painted = painted(&screen, TALL);

        assert_eq!(
            painted[18], "❯ row-11",
            "the prompt is on the top row however far the rest has scrolled: \
             {painted:?}"
        );
        assert_eq!(painted[27], "  row-20", "{painted:?}");
        assert!(
            !painted.iter().any(|line| line.contains("row-10")),
            "and what scrolled past is off the screen: {painted:?}"
        );
        assert_eq!(block(&screen, TALL, 27), Some(8));
    }

    #[test]
    fn composer_leaves_the_list_it_was_opened_from_on_the_screen() {
        // A third of eight rows is two.
        let painted = painted(&typing(&twenty_rows()), (60, 8));
        assert_eq!(painted[5], "❯ row-19");
        assert_eq!(painted[6], "  row-20");
        assert_eq!(
            painted[1], WELCOME,
            "the list is still there above it: {painted:?}"
        );
    }

    #[test]
    fn view_shows_the_line_being_typed_and_what_entering_it_will_do() {
        let mut screen = showing(Vec::new(), None);
        let mut composer = Composer::new(Asking::Task);
        composer.insert("port the importer");
        screen.mode = Mode::Typing(composer);

        let painted = painted(&screen, (60, 6));
        assert_eq!(painted[4], "❯ port the importer");
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
            drawn[5].ends_with(" vendor default ┈┈"),
            "and carries at its far end the layer, not a guess at which mode \
             claude would have picked: {:?}",
            drawn[5]
        );
        assert!(
            drawn[6].starts_with("❯ !command"),
            "the empty line under it carries its placeholder: {:?}",
            drawn[6]
        );
        assert!(drawn[7].contains("enter starts it"), "{:?}", drawn[7]);

        screen.profile.permission = "acceptEdits".to_string();
        assert!(
            painted(&screen, (60, 8))[5].ends_with(" acceptEdits ┈┈"),
            "and a mode in the vendor's own word for it: {:?}",
            painted(&screen, (60, 8))[5]
        );
    }

    #[test]
    fn composer_a_task_line_says_on_its_rule_which_project_it_will_run_in() {
        // A line opened under a project heading starts its agent there.
        let mut screen = launching(Vec::new());
        let mut composer = Composer::new(Asking::Task);
        composer.under = Some(std::path::PathBuf::from("/src/api"));
        composer.insert("port the importer");
        screen.mode = Mode::Typing(composer);

        let drawn = painted(&screen, (80, 8));
        assert!(
            drawn[5].starts_with("TASK · in /src/api · letters are text until esc"),
            "the rule says where the line will run: {:?}",
            drawn[5]
        );
    }

    #[test]
    fn composer_a_command_row_says_so_on_its_rule_and_in_the_keys_under_it() {
        let mut screen = launching(Vec::new());
        let mut composer = Composer::new(Asking::Task);
        composer.insert("!cargo test");
        screen.mode = Mode::Typing(composer);

        let drawn = painted(&screen, (60, 8));
        assert!(
            drawn[5].starts_with("COMMAND · letters are text until esc"),
            "the rule names what enter is about to do: {:?}",
            drawn[5]
        );
        assert_eq!(drawn[6], "❯ !cargo test");
        assert!(
            drawn[7].contains("enter runs it"),
            "and so does the row under it: {:?}",
            drawn[7]
        );
        assert!(
            !drawn[7].contains("shift+tab"),
            "which names no key for a dial this row has nothing to turn: {:?}",
            drawn[7]
        );
    }

    /// The colours of the rule's first dash and of the chevron, with `text`
    /// typed.
    fn edges(asking: Asking, text: &str) -> (Color, Color) {
        let mut screen = launching(Vec::new());
        let mut composer = Composer::new(asking);
        composer.insert(text);
        screen.mode = Mode::Typing(composer);

        let cells = cells(&screen, (60, 8));
        let dash = (0..60)
            .find(|column| cells[(*column, 5)].symbol() == RULE)
            .expect("a rule with an edge on it");
        (cells[(dash, 5)].fg, cells[(0, 6)].fg)
    }

    #[test]
    fn composer_a_command_row_lights_its_rule_and_the_chevron_under_it() {
        assert_eq!(
            edges(Asking::Task, "!cargo test"),
            (theme().accent, theme().accent),
            "the dashes and the chevron take the accent while the bang stands"
        );

        // Without the `!` they are dim again.
        assert_eq!(
            edges(Asking::Task, "cargo test"),
            (Color::Reset, Color::Reset),
            "and go back to dim the keystroke the bang comes off"
        );
    }

    #[test]
    fn header_keeps_the_permission_dial_to_the_lines_that_start_an_agent() {
        // Whether the dial or its key is shown anywhere.
        let turned = |screen: &Screen| {
            painted(screen, (60, 8))
                .iter()
                .any(|line| line.contains("default ┈┈") || line.contains("shift+tab"))
        };

        // Not on a reply, which goes to an agent already running.
        let mut screen = launching(Vec::new());
        screen.card = Some(asking(&[], None).read());
        screen.mode = Mode::Typing(Composer::new(Asking::Reply));
        assert!(!turned(&screen), "a reply is not a spawn");

        // Not on the find line.
        screen.mode = Mode::Typing(Composer::new(Asking::Find));
        assert!(!turned(&screen));

        // Not on a `!` command.
        let mut commanding = Composer::new(Asking::Task);
        commanding.insert("!cargo test");
        screen.mode = Mode::Typing(commanding);
        assert!(!turned(&screen));

        // Not for a vendor that declares no permission dial.
        screen.mode = Mode::Typing(Composer::new(Asking::Task));
        screen.profile.agent = "mock-claude".to_string();
        assert!(!turned(&screen));

        // Not when nothing is being typed.
        let screen = launching(Vec::new());
        assert!(!turned(&screen));
    }

    #[test]
    fn header_leaves_the_list_a_row_with_every_other_band_open() {
        // Header, card, typed line and keys all open at once.
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

        // Top to bottom: the list, the card over its foot, the typed line.
        let at = |front: &str| {
            painted
                .iter()
                .position(|line| line.starts_with(front))
                .unwrap_or_else(|| panic!("nothing begins {front:?} in: {painted:?}"))
        };
        assert!(
            at("Needs input") < at("✻ ask-a1b · claude ┈"),
            "{painted:?}"
        );
        assert!(at("✻ ask-a1b · claude ┈") < at("TASK"), "{painted:?}");
        // The card covers the agent's own row; its rule names the agent.
        assert!(
            !painted.iter().any(|line| line.starts_with(" ✻ ask-a1b")),
            "{painted:?}"
        );
    }
}
