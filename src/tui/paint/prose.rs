//! An agent's words, drawn the way the agent meant them.
//!
//! What an agent says is markdown, and a card that showed the marks would be
//! showing the agent's typing rather than its answer. So the text is parsed
//! the way the vendor's own screen parses it and drawn into rows a card can
//! hold: headings in weight, emphasis in its two slants, code set apart and
//! dim, lists with a bullet in the gutter, a quote behind a bar, a rule a rule.
//! Every row is already wrapped to the width it was asked for, because the
//! rows of a card are windowed and not reflowed — see [`super::card::Body`].
//!
//! The words are the agent's, and an agent's words go through [`inert`] before
//! a terminal sees them, the same as every other byte amx did not write.

// Nothing draws through here until the card does, one commit on.
#![allow(dead_code)]

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::style::{bold, dim};
use super::text::{RULE, inert, width_of};
use crate::theme::Theme;

/// The marker a list item wears in the gutter, and the bar a quote stands
/// behind.
const BULLET: &str = "• ";
const QUOTE: &str = "│ ";
/// What code is set in from the margin, block and fence alike.
const CODE_INDENT: &str = "  ";

/// `text` as markdown, drawn into rows no wider than `width`.
pub(super) fn render(text: &str, width: u16, theme: Theme) -> Vec<Line<'static>> {
    let mut drawing = Drawing::new(width.max(1) as usize, theme);
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    for event in Parser::new_ext(text, options) {
        drawing.take(event);
    }
    drawing.finish()
}

/// One run of words in one style, inside the block being gathered.
#[derive(Debug, Clone)]
struct Run {
    text: String,
    style: Style,
}

/// A list being drawn: what the next item is numbered, or bulleted.
#[derive(Debug, Clone, Copy)]
enum Listing {
    Bulleted,
    Numbered(u64),
}

/// The drawing as it is gathered: finished rows above, and the block still
/// being filled below them.
struct Drawing {
    width: usize,
    theme: Theme,
    rows: Vec<Line<'static>>,
    /// The runs of the block being gathered, styled as they arrived.
    runs: Vec<Run>,
    /// The styles open around the words arriving now, innermost last.
    open: Vec<Style>,
    /// Every list the block is inside, outermost first.
    lists: Vec<Listing>,
    /// The marker the next row of this block wears in its gutter, if the block
    /// opened one: an item's bullet or number, drawn once on its first row.
    marker: Option<String>,
    /// How many quote bars stand in front of the block.
    quoted: usize,
    /// Inside a code block, where lines are rows and nothing is wrapped.
    coding: bool,
    /// Inside a table row, where cells are joined rather than stacked.
    cell: Vec<String>,
    /// Whether the last block drawn wants a blank row before the next one.
    spaced: bool,
}

impl Drawing {
    fn new(width: usize, theme: Theme) -> Self {
        Drawing {
            width,
            theme,
            rows: Vec::new(),
            runs: Vec::new(),
            open: Vec::new(),
            lists: Vec::new(),
            marker: None,
            quoted: 0,
            coding: false,
            cell: Vec::new(),
            spaced: false,
        }
    }

    /// The style the words arriving now are drawn in.
    fn current(&self) -> Style {
        self.open
            .iter()
            .fold(Style::new(), |style, open| style.patch(*open))
    }

    fn take(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => match self.coding {
                true => self.code_lines(&text),
                false => self.push(&text, self.current()),
            },
            Event::Code(code) => self.push(&code, self.current().patch(dim())),
            Event::SoftBreak => self.push(" ", self.current()),
            Event::HardBreak => self.push("\n", self.current()),
            Event::Rule => {
                self.flush();
                self.space();
                self.rows
                    .push(Line::from(Span::styled(RULE.repeat(self.width), dim())));
                self.spaced = true;
            }
            Event::TaskListMarker(done) => {
                let mark = if done { "[x] " } else { "[ ] " };
                self.push(mark, self.current().patch(dim()));
            }
            Event::InlineMath(math) | Event::DisplayMath(math) => {
                self.push(&math, self.current().patch(dim()))
            }
            Event::FootnoteReference(name) => {
                self.push(&format!("[{name}]"), self.current().patch(dim()))
            }
            Event::Html(_) | Event::InlineHtml(_) => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                // A paragraph inside an item stands on the item's own row;
                // one after another in the same item is a block of its own.
                if self.marker.is_none() {
                    self.space();
                }
            }
            Tag::Heading { level, .. } => {
                self.space();
                let style = match level {
                    HeadingLevel::H1 | HeadingLevel::H2 => bold().fg(self.theme.accent),
                    _ => bold(),
                };
                self.open.push(style);
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.space();
                self.quoted += 1;
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                self.space();
                self.coding = true;
                if let CodeBlockKind::Fenced(language) = kind
                    && !language.is_empty()
                {
                    let label = format!("{CODE_INDENT}{}", inert(&language));
                    self.rows.push(Line::from(Span::styled(label, dim())));
                }
            }
            Tag::List(first) => {
                self.flush();
                if self.lists.is_empty() {
                    self.space();
                }
                self.lists.push(match first {
                    Some(number) => Listing::Numbered(number),
                    None => Listing::Bulleted,
                });
            }
            Tag::Item => {
                self.flush();
                let marker = match self.lists.last_mut() {
                    Some(Listing::Numbered(number)) => {
                        let mark = format!("{number}. ");
                        *number += 1;
                        mark
                    }
                    _ => BULLET.to_string(),
                };
                self.marker = Some(marker);
            }
            Tag::Emphasis => self.open.push(Style::new().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.open.push(bold()),
            Tag::Strikethrough => self
                .open
                .push(Style::new().add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { .. } => self
                .open
                .push(Style::new().add_modifier(Modifier::UNDERLINED)),
            Tag::Image { .. } => self.open.push(dim()),
            Tag::Table(_) => {
                self.flush();
                self.space();
            }
            Tag::TableHead => self.open.push(bold()),
            Tag::TableRow | Tag::TableCell => {}
            Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_)
            | Tag::Superscript
            | Tag::Subscript => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                self.spaced = true;
            }
            TagEnd::Heading(_) => {
                self.flush();
                self.open.pop();
                self.spaced = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quoted = self.quoted.saturating_sub(1);
                self.spaced = true;
            }
            TagEnd::CodeBlock => {
                self.coding = false;
                self.spaced = true;
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.spaced = true;
                }
            }
            TagEnd::Item => {
                self.flush();
                self.marker = None;
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Link
            | TagEnd::Image => {
                self.open.pop();
            }
            TagEnd::TableHead => {
                // Drawn in the weight the head opened, before it closes.
                self.table_row();
                self.open.pop();
            }
            TagEnd::TableRow => self.table_row(),
            TagEnd::TableCell => {
                let cell: String = self.runs.drain(..).map(|run| run.text).collect();
                self.cell.push(cell.trim().to_string());
            }
            TagEnd::Table => {
                self.spaced = true;
            }
            TagEnd::HtmlBlock
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::MetadataBlock(_)
            | TagEnd::Superscript
            | TagEnd::Subscript => {}
        }
    }

    /// Words arriving for the block being gathered.
    fn push(&mut self, text: &str, style: Style) {
        let text = inert(text);
        match self.runs.last_mut() {
            Some(run) if run.style == style => run.text.push_str(&text),
            _ => self.runs.push(Run { text, style }),
        }
    }

    /// Lines of a code block, a row apiece, set in and dim, never wrapped: a
    /// row too wide for the card is cut by the card, the way code is.
    fn code_lines(&mut self, text: &str) {
        for line in text.lines() {
            let row = format!("{}{CODE_INDENT}{}", self.gutter(), inert(line));
            self.rows.push(Line::from(Span::styled(row, dim())));
        }
    }

    /// The cells gathered for one table row, joined on one row.
    fn table_row(&mut self) {
        if self.cell.is_empty() {
            return;
        }
        let style = self.current();
        let row = std::mem::take(&mut self.cell).join("  ");
        self.runs.push(Run { text: row, style });
        self.flush();
    }

    /// A blank row between one block and the next, where the last block asked
    /// for one and there is something above it to stand apart from.
    fn space(&mut self) {
        if self.spaced && !self.rows.is_empty() {
            self.rows.push(Line::raw(String::new()));
        }
        self.spaced = false;
    }

    /// What stands in front of every row of the block: the quote bars and
    /// the indent of the lists it is inside.
    fn gutter(&self) -> String {
        let quotes = QUOTE.repeat(self.quoted);
        let depth = self.lists.len().saturating_sub(1);
        format!("{quotes}{}", " ".repeat(depth * 2))
    }

    /// The block gathered so far, wrapped into rows and drawn.
    fn flush(&mut self) {
        if self.runs.is_empty() {
            return;
        }
        let runs = std::mem::take(&mut self.runs);
        let gutter = self.gutter();
        let marker = self.marker.take().unwrap_or_default();
        // A block inside a list item hangs under its marker: the first row
        // wears it, and every row after stands in the room it took.
        let first = format!("{gutter}{marker}");
        let rest = format!("{gutter}{}", " ".repeat(width_of(&marker)));
        let room = self.width.saturating_sub(width_of(&first)).max(1);
        let quote = (self.quoted > 0).then(dim);
        for (at, row) in wrap(&runs, room).into_iter().enumerate() {
            let lead = if at == 0 { &first } else { &rest };
            let mut spans = Vec::with_capacity(row.len() + 1);
            if !lead.is_empty() {
                spans.push(Span::styled(lead.clone(), quote.unwrap_or_else(dim)));
            }
            spans.extend(row);
            self.rows.push(Line::from(spans));
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush();
        while self
            .rows
            .last()
            .is_some_and(|row| row.spans.iter().all(|span| span.content.trim().is_empty()))
        {
            self.rows.pop();
        }
        self.rows
    }
}

/// The runs of one block, wrapped at whitespace into rows no wider than
/// `width`, each row the styled spans that fell on it.
///
/// A word wider than the whole row is broken where the row ends rather than
/// pushed off the edge of the terminal, and a hard break inside a run ends
/// the row where it stands.
fn wrap(runs: &[Run], width: usize) -> Vec<Vec<Span<'static>>> {
    // Flattened to characters so a word can span two styles and still be one
    // word to the wrap.
    let mut chars: Vec<(char, Style)> = Vec::new();
    for run in runs {
        chars.extend(run.text.chars().map(|c| (c, run.style)));
    }

    let mut rows: Vec<Vec<(char, Style)>> = vec![Vec::new()];
    let mut used = 0;
    let mut at = 0;
    while at < chars.len() {
        let (c, _) = chars[at];
        if c == '\n' {
            rows.push(Vec::new());
            used = 0;
            at += 1;
            continue;
        }
        // The next word, which is the run of characters up to the next
        // whitespace, and its width on a screen.
        let end = chars[at..]
            .iter()
            .position(|(c, _)| c.is_whitespace())
            .map_or(chars.len(), |found| at + found);
        let word = &chars[at..end.max(at + 1)];
        let wide: usize = word.iter().map(|(c, _)| glyph_width(*c)).sum();

        if c.is_whitespace() {
            // A space at the head of a row is the wrap's own and is dropped.
            if used > 0 && used < width {
                rows.last_mut().expect("a row").push((' ', chars[at].1));
                used += 1;
            }
            at += 1;
            continue;
        }
        if used > 0 && used + wide > width {
            // Off the end of this row: the word starts the next one, and the
            // space that led to it goes.
            let row = rows.last_mut().expect("a row");
            while row.last().is_some_and(|(c, _)| *c == ' ') {
                row.pop();
            }
            rows.push(Vec::new());
            used = 0;
        }
        if wide > width {
            // Wider than a whole row: broken where the row ends.
            for (c, style) in word {
                let w = glyph_width(*c);
                if used + w > width && used > 0 {
                    rows.push(Vec::new());
                    used = 0;
                }
                rows.last_mut().expect("a row").push((*c, *style));
                used += w;
            }
        } else {
            rows.last_mut().expect("a row").extend_from_slice(word);
            used += wide;
        }
        at = end.max(at + 1);
    }

    rows.into_iter().map(|row| spans_of(&row)).collect()
}

/// Consecutive characters in one style, as one span.
fn spans_of(row: &[(char, Style)]) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current: Option<(String, Style)> = None;
    for (c, style) in row {
        match &mut current {
            Some((text, open)) if *open == *style => text.push(*c),
            _ => {
                if let Some((text, open)) = current.take() {
                    spans.push(Span::styled(text, open));
                }
                current = Some((c.to_string(), *style));
            }
        }
    }
    if let Some((text, open)) = current {
        spans.push(Span::styled(text, open));
    }
    spans
}

/// The columns one character takes.
fn glyph_width(c: char) -> usize {
    width_of(c.encode_utf8(&mut [0; 4]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::default()
    }

    /// What the rows say, one string a row.
    fn words(rows: &[Line<'static>]) -> Vec<String> {
        rows.iter()
            .map(|row| row.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// The row holding `text`, or a panic naming what was drawn.
    fn row_with<'a>(rows: &'a [Line<'static>], text: &str) -> &'a Line<'static> {
        rows.iter()
            .find(|row| row.spans.iter().any(|span| span.content.contains(text)))
            .unwrap_or_else(|| panic!("no row holds {text:?} in {:?}", words(rows)))
    }

    fn span_with<'a>(row: &'a Line<'static>, text: &str) -> &'a Span<'static> {
        row.spans
            .iter()
            .find(|span| span.content.contains(text))
            .unwrap_or_else(|| panic!("no span holds {text:?} in {row:?}"))
    }

    const EVERYTHING: &str = "\
# The plan

Fix the **login** bug, which is *small* and in `auth.rs`.

```rust
fn check(token: &str) -> bool {
    token.len() > 8
}
```

- first thing
- second thing, which is rather longer than one row of this card holds
  1. a step
  2. another

> a quote

---

Done.";

    #[test]
    fn prose_draws_every_kind_of_block_into_rows() {
        let rows = render(EVERYTHING, 40, theme());
        assert_eq!(
            words(&rows),
            vec![
                "The plan",
                "",
                "Fix the login bug, which is small and in",
                "auth.rs.",
                "",
                "  rust",
                "  fn check(token: &str) -> bool {",
                "      token.len() > 8",
                "  }",
                "",
                "• first thing",
                "• second thing, which is rather longer",
                "  than one row of this card holds",
                "  1. a step",
                "  2. another",
                "",
                "│ a quote",
                "",
                &RULE.repeat(40),
                "",
                "Done.",
            ]
        );
    }

    #[test]
    fn prose_paints_weight_slant_and_code() {
        let rows = render(EVERYTHING, 40, theme());
        let heading = span_with(row_with(&rows, "The plan"), "The plan");
        assert!(heading.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(heading.style.fg, Some(theme().accent));

        let line = row_with(&rows, "login");
        assert!(
            span_with(line, "login")
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            span_with(line, "small")
                .style
                .add_modifier
                .contains(Modifier::ITALIC)
        );
        assert!(
            span_with(row_with(&rows, "auth.rs"), "auth.rs")
                .style
                .add_modifier
                .contains(Modifier::DIM)
        );
        assert!(
            span_with(row_with(&rows, "token.len"), "token.len")
                .style
                .add_modifier
                .contains(Modifier::DIM),
            "a fenced block is dim, and the fence's own marks are gone"
        );
        assert!(
            !words(&rows).iter().any(|row| row.contains("```")),
            "{:?}",
            words(&rows)
        );
    }

    #[test]
    fn prose_wraps_at_words_and_breaks_a_word_wider_than_the_row() {
        let rows = render("one two three four five", 9, theme());
        assert_eq!(words(&rows), vec!["one two", "three", "four five"]);

        let rows = render("abcdefghijkl", 5, theme());
        assert_eq!(words(&rows), vec!["abcde", "fghij", "kl"]);

        // A hard break ends the row where it stands.
        let rows = render("first  \nsecond", 40, theme());
        assert_eq!(words(&rows), vec!["first", "second"]);

        // Width is measured in columns, so a wide glyph is two of them.
        let rows = render("日本 語", 4, theme());
        assert_eq!(words(&rows), vec!["日本", "語"]);
    }

    #[test]
    fn prose_makes_the_agents_words_inert() {
        let rows = render("done\u{1b}]0;PWNED\u{7} ad\u{200b}min", 80, theme());
        let said = words(&rows).join("\n");
        assert!(said.contains("]0;PWNED"), "{said:?}");
        assert!(said.contains("ad min"), "{said:?}");
        assert_eq!(said.chars().filter(|c| c.is_control()).count(), 0);
    }

    #[test]
    fn prose_of_nothing_is_no_rows() {
        assert!(render("", 40, theme()).is_empty());
        assert!(render("\n\n  \n", 40, theme()).is_empty());
    }

    #[test]
    fn prose_joins_a_tables_cells_on_one_row() {
        let rows = render("| a | b |\n|---|---|\n| 1 | 2 |", 40, theme());
        assert_eq!(words(&rows), vec!["a  b", "1  2"]);
        assert!(
            span_with(&rows[0], "a")
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }
}
