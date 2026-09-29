//! Rendering an agent's markdown into styled rows.
//!
//! Headings are bold, emphasis italic, code blocks indented and dim, inline
//! code in the accent, list items behind a bullet or number, quotes behind a
//! bar. Rows come out wrapped to the requested width, because a card windows
//! its rows without reflowing them (see [`super::card::Body`]). All text goes
//! through [`inert`].

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::style::{bold, dim};
use super::text::{RULE, char_width, inert, width_of};
use crate::theme::Theme;

const BULLET: &str = "• ";
const QUOTE: &str = "│ ";
/// Indent of a code block.
const CODE_INDENT: &str = "  ";
/// Tab stop width in code blocks. The sanitiser would turn a tab into one
/// space and flatten tab-indented code.
const TAB: usize = 4;

/// `text` as markdown, drawn into rows no wider than `width`, never ending on
/// a blank row.
///
/// A soft line break ends the row, as a hard break does, rather than becoming
/// a space. This matches pi-tui's `Markdown`, which keeps the newline.
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

/// A run of text in one style within the current block.
#[derive(Debug, Clone)]
struct Run {
    text: String,
    style: Style,
}

/// An open list: bulleted, or numbered with the next item's number.
#[derive(Debug, Clone, Copy)]
enum Listing {
    Bulleted,
    Numbered(u64),
}

/// Render state: finished rows, and the block still being gathered.
struct Drawing {
    width: usize,
    theme: Theme,
    rows: Vec<Line<'static>>,
    /// The current block's runs.
    runs: Vec<Run>,
    /// Open inline styles, innermost last.
    open: Vec<Style>,
    /// Open lists, outermost first.
    lists: Vec<Listing>,
    /// The bullet or number for the current block's first row.
    marker: Option<String>,
    /// Quote nesting depth.
    quoted: usize,
    /// Inside a code block: one row per line, no wrapping.
    coding: bool,
    /// Cells of the table row being gathered, and the finished rows with
    /// their style. The table is drawn when it closes, once column widths are
    /// known.
    cell: Vec<String>,
    table: Vec<(Vec<String>, Style)>,
    /// The open link's URL and where its text starts in the block.
    link: Option<(String, usize)>,
    /// Whether the next block needs a blank row before it.
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
            table: Vec::new(),
            link: None,
            spaced: false,
        }
    }

    /// The style of text arriving now: every open style patched together.
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
            // The accent, since dim is already used by tool rows and gutters.
            Event::Code(code) => self.push(&code, self.current().fg(self.theme.accent)),
            // Both breaks end the row; see [`render`].
            Event::SoftBreak | Event::HardBreak => self.push("\n", self.current()),
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
                // An item's first paragraph starts on the marker's row; later
                // ones are spaced like any block.
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
            Tag::Link { dest_url, .. } => {
                self.open
                    .push(Style::new().add_modifier(Modifier::UNDERLINED));
                self.link = Some((dest_url.to_string(), self.gathered().len()));
            }
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
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Image => {
                self.open.pop();
            }
            TagEnd::Link => {
                self.open.pop();
                // The URL, dim after the link text, unless the text already is
                // the URL (an autolink).
                if let Some((url, from)) = self.link.take() {
                    let address = inert(url.strip_prefix("mailto:").unwrap_or(&url));
                    if self.gathered().get(from..).map(str::trim) != Some(address.as_str()) {
                        self.push(&format!(" ({url})"), self.current().patch(dim()));
                    }
                }
            }
            TagEnd::TableHead => {
                // Gather the head row while its bold style is still open.
                self.table_row();
                self.open.pop();
            }
            TagEnd::TableRow => self.table_row(),
            TagEnd::TableCell => {
                let cell: String = self.runs.drain(..).map(|run| run.text).collect();
                self.cell.push(cell.trim().to_string());
            }
            TagEnd::Table => {
                self.table();
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

    /// The current block's text so far.
    fn gathered(&self) -> String {
        self.runs.iter().map(|run| run.text.as_str()).collect()
    }

    /// Append text to the current block, merging with the last run when the
    /// style matches.
    fn push(&mut self, text: &str, style: Style) {
        let text = inert(text);
        match self.runs.last_mut() {
            Some(run) if run.style == style => run.text.push_str(&text),
            _ => self.runs.push(Run { text, style }),
        }
    }

    /// Code block lines, one dim indented row each. Never wrapped; the card
    /// clips a row that is too wide.
    fn code_lines(&mut self, text: &str) {
        for line in text.lines() {
            let row = format!("{}{CODE_INDENT}{}", self.gutter(), inert(&untabbed(line)));
            self.rows.push(Line::from(Span::styled(row, dim())));
        }
    }

    /// Keep the gathered cells as a table row.
    fn table_row(&mut self) {
        if self.cell.is_empty() {
            return;
        }
        let style = self.current();
        self.table.push((std::mem::take(&mut self.cell), style));
    }

    /// Draw the gathered table, one row per table row. Cells are padded to
    /// their column's widest, two spaces apart; the last cell is not padded.
    fn table(&mut self) {
        let rows = std::mem::take(&mut self.table);
        let columns = rows.iter().map(|(cells, _)| cells.len()).max().unwrap_or(0);
        let widths: Vec<usize> = (0..columns)
            .map(|at| {
                rows.iter()
                    .filter_map(|(cells, _)| cells.get(at))
                    .map(|cell| width_of(cell))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        for (cells, style) in rows {
            let last = cells.len().saturating_sub(1);
            let text = cells
                .iter()
                .enumerate()
                .map(|(at, cell)| match at < last {
                    true => format!("{cell}{}", " ".repeat(widths[at] - width_of(cell))),
                    false => cell.clone(),
                })
                .collect::<Vec<_>>()
                .join("  ")
                .trim_end()
                .to_string();
            self.runs.push(Run { text, style });
            self.flush();
        }
    }

    /// Push a blank row if the last block asked for one and is not the first.
    fn space(&mut self) {
        if self.spaced && !self.rows.is_empty() {
            self.rows.push(Line::raw(String::new()));
        }
        self.spaced = false;
    }

    /// The prefix of every row in the block: quote bars and list indent.
    fn gutter(&self) -> String {
        let quotes = QUOTE.repeat(self.quoted);
        let depth = self.lists.len().saturating_sub(1);
        format!("{quotes}{}", " ".repeat(depth * 2))
    }

    /// Wrap the current block into rows and push them.
    fn flush(&mut self) {
        if self.runs.is_empty() {
            return;
        }
        let runs = std::mem::take(&mut self.runs);
        let gutter = self.gutter();
        let marker = self.marker.take().unwrap_or_default();
        // Hanging indent: continuation rows line up after the marker.
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

/// A block's runs wrapped at whitespace into rows of styled spans no wider
/// than `width`.
///
/// A word longer than a row is broken at the row's end. A `\n` ends the row.
/// Spaces at a wrap point are dropped.
fn wrap(runs: &[Run], width: usize) -> Vec<Vec<Span<'static>>> {
    // Flattened to chars so one word can span two styles.
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
        // The word starting here, up to the next whitespace.
        let end = chars[at..]
            .iter()
            .position(|(c, _)| c.is_whitespace())
            .map_or(chars.len(), |found| at + found);
        let word = &chars[at..end.max(at + 1)];
        let wide: usize = word.iter().map(|(c, _)| char_width(*c)).sum();

        if c.is_whitespace() {
            // Each whitespace char becomes a space, dropped at a row's edges.
            if used > 0 && used < width {
                rows.last_mut().expect("a row").push((' ', chars[at].1));
                used += 1;
            }
            at += 1;
            continue;
        }
        if used > 0 && used + wide > width {
            // Move the word to a new row and drop the trailing spaces.
            let row = rows.last_mut().expect("a row");
            while row.last().is_some_and(|(c, _)| *c == ' ') {
                row.pop();
            }
            rows.push(Vec::new());
            used = 0;
        }
        if wide > width {
            for (c, style) in word {
                let w = char_width(*c);
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

/// A row of styled chars as spans, one per run of equal style.
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

/// `line` with each tab expanded to the next [`TAB`] stop.
fn untabbed(line: &str) -> String {
    let mut out = String::new();
    let mut column = 0;
    for c in line.chars() {
        if c == '\t' {
            let stop = TAB - column % TAB;
            out.push_str(&" ".repeat(stop));
            column += stop;
        } else {
            out.push(c);
            column += char_width(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::default()
    }

    /// Each row's text.
    fn words(rows: &[Line<'static>]) -> Vec<String> {
        rows.iter()
            .map(|row| row.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// The first row containing `text`.
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
        let code = span_with(row_with(&rows, "auth.rs"), "auth.rs").style;
        assert_eq!(code.fg, Some(theme().accent));
        assert!(
            !code.add_modifier.contains(Modifier::DIM),
            "code in a line is in the accent and not dim, which a tool row is"
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

        // A hard break ends the row.
        let rows = render("first  \nsecond", 40, theme());
        assert_eq!(words(&rows), vec!["first", "second"]);

        // Width is in columns; a wide glyph takes two.
        let rows = render("日本 語", 4, theme());
        assert_eq!(words(&rows), vec!["日本", "語"]);
    }

    #[test]
    fn prose_ends_a_row_at_a_single_newline() {
        let rows = render("first line\nsecond line", 40, theme());
        assert_eq!(
            words(&rows),
            vec!["first line", "second line"],
            "one newline is a row break, not the space markdown calls it"
        );

        // Code blocks, list items and long paragraphs wrap as before.
        let rows = render(
            "```rust\nfn check() {}\n```\n\n\
             - first thing\n\
             - second thing, which is rather longer than one row of this card holds\n\n\
             plain words wrapped because they are longer than the row is wide",
            40,
            theme(),
        );
        assert_eq!(
            words(&rows),
            vec![
                "  rust",
                "  fn check() {}",
                "",
                "• first thing",
                "• second thing, which is rather longer",
                "  than one row of this card holds",
                "",
                "plain words wrapped because they are",
                "longer than the row is wide",
            ]
        );

        // A line break inside an item keeps the hanging indent.
        let rows = render("- first line\n  second line", 40, theme());
        assert_eq!(words(&rows), vec!["• first line", "  second line"]);
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
    fn prose_keeps_a_tab_indented_block_indented() {
        let rows = render(
            "```go\nfunc main() {\n\tgo()\n\t\tdeep\nab\tc\n}\n```",
            40,
            theme(),
        );
        assert_eq!(
            words(&rows),
            vec![
                "  go",
                "  func main() {",
                "      go()",
                "          deep",
                "  ab  c",
                "  }",
            ],
            "a tab is the spaces to the next stop, not the one space a control becomes"
        );
    }

    #[test]
    fn prose_keeps_a_links_address_beside_its_words() {
        let rows = render(
            "see [the docs](https://x.dev/d), <https://y.dev> or <me@x.dev>",
            80,
            theme(),
        );
        assert_eq!(
            words(&rows),
            vec!["see the docs (https://x.dev/d), https://y.dev or me@x.dev"],
            "the address behind the words, and once only where the words are it"
        );
        let row = &rows[0];
        assert!(
            span_with(row, "the docs")
                .style
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
        let address = span_with(row, "(https://x.dev/d)").style;
        assert!(address.add_modifier.contains(Modifier::DIM));
        assert!(!address.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn prose_of_nothing_is_no_rows() {
        assert!(render("", 40, theme()).is_empty());
        assert!(render("\n\n  \n", 40, theme()).is_empty());
    }

    #[test]
    fn prose_stands_a_tables_columns_under_one_another() {
        let rows = render(
            "| a | bee | c |\n|---|---|---|\n| one | 2 | three |\n| 日本 | x | |",
            40,
            theme(),
        );
        assert_eq!(
            words(&rows),
            vec!["a     bee  c", "one   2    three", "日本  x"],
            "each column as wide as its widest cell, measured in columns"
        );
        assert!(
            span_with(&rows[0], "a")
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }
}
