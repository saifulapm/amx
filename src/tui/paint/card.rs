//! The card: a detailed look at one agent, drawn over the foot of the list.
//!
//! A card is a rule naming the agent, then the pull requests, question,
//! choices, body and queued messages, then the answer line. It covers the
//! last rows of the list band instead of taking rows from it, so opening,
//! closing or walking a card never moves a list row.
//!
//! - A card is built from text (a pane capture, a recorded answer, a patch)
//!   and drawn from a [`Body`]: the text is parsed once when the card is
//!   built, and frames only window the prepared rows.
//! - Scroll offsets are clamped by the paint, which is the only place that
//!   knows how many rows the body gets; see [`Scroll`].

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ops::Range;

use super::input::{COMPOSER_CAP, behind, composer_lines, rows_of, typed_rows};
use super::prose;
use super::style::{bold, colour, dim, request_colour};
use super::text::{RULE, SEPARATOR, fit, inert, width_of};
use super::wall::icon;
use crate::ansi::{self, Colour, Painted};
use crate::conversation::Said;
use crate::derive::{Evidence, View};
use crate::furniture::Furniture;
use crate::pr::Pr;
use crate::store::{Ask, Kind, Phase};
use crate::theme::Theme;
use crate::tui::act::{self, Composer};
use crate::tui::rows::Showing;
use crate::verbs::send::numbered;

/// A card: one agent's question, choices and body.
///
/// `B` is the body's form: text when built, [`Body`] when drawn.
pub struct Card<B = String> {
    pub id: String,
    pub phase: Phase,
    /// The pending question, if any.
    pub question: Option<String>,
    /// The question's choices, in screen order.
    pub options: Vec<String>,
    /// Whether the choices are amx's numbering of an unnumbered vendor list
    /// (see [`crate::store::State::walked`]); answering sends a walk.
    pub walked: bool,
    /// The question's kind, which decides what answers it takes.
    pub kind: Option<Kind>,
    /// The pane capture, recorded answer or conversation, or the patch.
    pub body: B,
    /// Whether the body is a patch.
    pub changes: bool,
    /// Whether the body is the agent's own words read from the top (a
    /// recorded answer, or a finished agent's conversation). Pane captures
    /// and live conversations are read up from the bottom.
    pub answer: bool,
    /// Whether a typed line reaches the agent: sent to its pane, or used to
    /// resume it when the pane is gone.
    pub listening: bool,
    /// Messages sent but not yet taken, oldest first (see
    /// [`crate::verbs::send::queued`]). Only a working agent has any; the
    /// vendor draws them in its composer, which the card cuts off, so they are
    /// shown here.
    pub queued: Vec<String>,
}

impl<B> Card<B> {
    /// Whether the card is a question that can be answered.
    pub fn asks(&self) -> bool {
        !self.changes && self.phase == Phase::Waiting
    }

    /// Whether the body reads from its top (a patch or a recorded answer).
    pub fn forward(&self) -> bool {
        self.changes || self.answer
    }
}

impl Card<Body> {
    /// The initial scroll offset from the natural edge: the body's anchor for
    /// a forward card, else 0.
    ///
    /// The anchor may lie past the last row; the paint clamps it.
    pub fn opens_at(&self) -> usize {
        match self.forward() {
            true => self.body.anchor(),
            false => 0,
        }
    }
}

impl Card<String> {
    /// Parse the body text into a [`Body`].
    ///
    /// For cards built from text already in hand, such as a patch. Cards built
    /// from a record or a pane construct their [`Body`] directly.
    pub fn read(self) -> Card<Body> {
        Card {
            // A live pane here has no known vendor, so it is cut with the
            // fallback vendor's furniture. The view builds live cards with the
            // right vendor elsewhere.
            body: match (self.changes, self.answer || self.phase.is_terminal()) {
                (true, _) => Body::patch(&self.body),
                (_, true) => Body::said(&self.body),
                _ => Body::screen(crate::rules::of("").furniture(), &self.body),
            },
            id: self.id,
            phase: self.phase,
            question: self.question,
            options: self.options,
            walked: self.walked,
            kind: self.kind,
            changes: self.changes,
            answer: self.answer,
            listening: self.listening,
            queued: self.queued,
        }
    }
}

/// A card's body, parsed once when the card is built.
///
/// The rows are ready to draw (made inert, styled), so a frame only takes a
/// window of them, whatever the body's length.
pub struct Body {
    rows: Vec<Line<'static>>,
    /// How many rows the card shows: trailing vendor furniture and blank rows
    /// are excluded.
    kept: usize,
    /// Whether vendor furniture was cut off. A pane of nothing but furniture
    /// is shown as [`ALL_CHROME`], not as an empty body.
    chrome: bool,
    /// The row a forward card opens on: past the last row for a conversation
    /// (the paint clamps it, see [`Scroll::kept`]), 0 otherwise.
    anchor: usize,
    /// A patch's hunks in patch order; empty for any other body.
    hunks: Vec<Hunk>,
}

/// One hunk of a patch body, found in the same pass that builds the rows so
/// the two agree on where it starts.
pub struct Hunk {
    /// The file on the new side, or the old side for a deleted file.
    pub path: String,
    /// The start line the header names on that side.
    pub line: usize,
    /// The body row holding the `@@` header.
    pub row: usize,
    /// The hunk's text from its header on, as git wrote it.
    pub text: String,
}

/// The glyph before a prompt, as in the composer.
const PROMPT: &str = "❯ ";
/// The glyph before a tool call. Not U+2692 (hammer): with a colour-emoji
/// font fallback it drew two cells wide over the following space.
const TOOL: &str = "› ";
/// Rows of the live stream kept under the conversation. Small enough that
/// some of the record stays visible on the tallest card; a fixed count because
/// the body is built before the card's height is known.
const TAIL: usize = 8;

impl Body {
    /// An empty body, for a card showing a question.
    pub(in crate::tui) fn none() -> Body {
        Body {
            rows: Vec::new(),
            kept: 0,
            chrome: false,
            anchor: 0,
            hunks: Vec::new(),
        }
    }

    /// A conversation from the transcript, with the vendor's live stream
    /// (`live`, the last [`TAIL`] rows of it) underneath while a turn runs.
    ///
    /// A vendor that streams nothing shows the record alone; the pane is not
    /// shown under a record (it duplicated the turn with the vendor's chrome).
    ///
    /// Prompts get the [`PROMPT`] glyph, text is rendered as markdown, and a
    /// tool call is one row: the name, then its argument dim. Items are
    /// separated by a blank row, except consecutive tool calls. Rows are
    /// wrapped to `width` here since a card never reflows. The anchor is the
    /// end, so a finished conversation opens on the end of its last answer.
    pub(in crate::tui) fn conversation(
        said: &[Said],
        live: Option<&str>,
        width: u16,
        theme: Theme,
    ) -> Body {
        let width = width.max(1);
        let mut rows: Vec<Line<'static>> = Vec::new();
        let mut after_call = false;
        for one in said {
            let drawn: Vec<Line<'static>> = match one {
                Said::Prompt(text) => {
                    let lead = bold().fg(theme.accent);
                    prose::render(text, width.saturating_sub(2), theme)
                        .into_iter()
                        .enumerate()
                        .map(|(at, line)| {
                            let glyph = if at == 0 { PROMPT } else { "  " };
                            let mut spans = vec![Span::styled(glyph, lead)];
                            spans.extend(line.spans);
                            Line::from(spans)
                        })
                        .collect()
                }
                Said::Text(text) => prose::render(text, width, theme),
                Said::Tool { name, detail } => {
                    let width = width as usize;
                    let name = fit(&inert(name), width.saturating_sub(width_of(TOOL)));
                    let mut spans = vec![Span::styled(TOOL, dim()), Span::raw(name.clone())];
                    if let Some(detail) = detail {
                        let room = width.saturating_sub(width_of(TOOL) + width_of(&name) + 1);
                        if room > 0 {
                            let detail = fit(&inert(detail), room);
                            spans.push(Span::styled(format!(" {detail}"), dim()));
                        }
                    }
                    vec![Line::from(spans)]
                }
            };
            if drawn.is_empty() {
                continue;
            }
            let call = matches!(one, Said::Tool { .. });
            if !rows.is_empty() && !(after_call && call) {
                rows.push(Line::raw(String::new()));
            }
            rows.extend(drawn);
            after_call = call;
        }

        // `prose::render` never ends on a blank row, so neither does anything
        // pushed here.
        if let Some(live) = live {
            let tail = prose::render(live, width, theme);
            let skipped = tail.len().saturating_sub(TAIL);
            // No separator row over an empty stream.
            let tail: Vec<Line<'static>> = tail.into_iter().skip(skipped).collect();
            if !tail.is_empty() {
                if !rows.is_empty() {
                    rows.push(Line::raw(String::new()));
                }
                rows.extend(tail);
            }
        }

        Body {
            kept: rows.len(),
            anchor: rows.len(),
            rows,
            chrome: false,
            hunks: Vec::new(),
        }
    }

    /// The row a forward card opens on.
    pub(in crate::tui) fn anchor(&self) -> usize {
        self.anchor
    }

    /// A `git diff` patch.
    ///
    /// Each file's header block (`diff --git`, `index`, `---`/`+++`, mode and
    /// rename lines) collapses into one heading row: the path and its added
    /// and removed counts. Other rows keep their `+`/`-` marks and are coloured
    /// as git colours them. Hunks are recorded in the same pass.
    pub(in crate::tui) fn patch(text: &str) -> Body {
        let mut rows: Vec<Line<'static>> = Vec::new();
        let mut hunks: Vec<Hunk> = Vec::new();
        // The current file's header block. Its heading row is pushed blank and
        // filled in once the file's counts are known.
        let mut headers: Vec<&str> = Vec::new();
        let mut file: Option<Reading> = None;
        // The current hunk's rows, from its `@@` header on.
        let mut held: Vec<&str> = Vec::new();

        for row in text.lines() {
            if row.starts_with(FILE) {
                shut(&mut hunks, &mut held);
                // A file with headers only (mode change, rename, binary) is
                // just its heading.
                file = open(&mut rows, &mut headers).or(file);
                close(&mut rows, file.take());
                headers.push(row);
                continue;
            }
            // The first non-header row ends the block.
            if !headers.is_empty() {
                if header(row) {
                    headers.push(row);
                    continue;
                }
                file = open(&mut rows, &mut headers);
            }

            let style = if row.starts_with(HUNK) {
                shut(&mut hunks, &mut held);
                hunks.push(Hunk {
                    path: file
                        .as_ref()
                        .map_or(String::new(), |file| file.path.clone()),
                    line: starts(row),
                    row: rows.len(),
                    text: String::new(),
                });
                Style::default().fg(Color::Cyan)
            } else if row.starts_with('+') {
                if let Some(file) = file.as_mut() {
                    file.added += 1;
                }
                Style::default().fg(Color::Green)
            } else if row.starts_with('-') {
                if let Some(file) = file.as_mut() {
                    file.removed += 1;
                }
                Style::default().fg(Color::Red)
            } else {
                dim()
            };
            if !held.is_empty() || row.starts_with(HUNK) {
                held.push(row);
            }
            rows.push(Line::styled(inert(row), style));
        }
        shut(&mut hunks, &mut held);
        file = open(&mut rows, &mut headers).or(file);
        close(&mut rows, file);

        Body {
            kept: rows.len(),
            rows,
            chrome: false,
            anchor: 0,
            hunks,
        }
    }

    /// The patch's hunks, in patch order.
    pub(in crate::tui) fn hunks(&self) -> &[Hunk] {
        &self.hunks
    }

    /// A live pane capture with its ANSI styling kept and the vendor's
    /// furniture cut off the bottom.
    ///
    /// `chrome` must be the pane's own vendor's: another vendor's anchors
    /// match nothing and leave the furniture in.
    pub(in crate::tui) fn screen(chrome: &Furniture, text: &str) -> Body {
        Body::walk(text, Some(chrome))
    }

    /// Text that came from no live pane (a recorded answer, or an ended
    /// agent's output), with nothing cut.
    pub(in crate::tui) fn said(text: &str) -> Body {
        Body::walk(text, None)
    }

    /// Parse ANSI `text` into rows, cutting `chrome` furniture if given.
    fn walk(text: &str, chrome: Option<&Furniture>) -> Body {
        #[cfg(test)]
        WALKS.with(|walks| walks.set(walks.get() + 1));
        // The only place escape sequences are parsed; nothing downstream holds
        // a control sequence.
        let read = ansi::painted(text);
        let said: Vec<String> = read.iter().map(|row| words(row)).collect();
        let plain: Vec<&str> = said.iter().map(String::as_str).collect();
        let drawn = match chrome {
            Some(chrome) => chrome.cut(&plain).len(),
            None => plain.len(),
        };
        // Trailing blank rows (pane padding) are dropped too.
        let mut kept = drawn;
        while kept > 0 && plain[kept - 1].trim().is_empty() {
            kept -= 1;
        }
        Body {
            rows: read.iter().map(|row| as_painted(row)).collect(),
            kept,
            chrome: drawn < plain.len(),
            anchor: 0,
            hunks: Vec::new(),
        }
    }

    /// Rows the body shows, counting the [`ALL_CHROME`] row as one.
    fn length(&self) -> usize {
        self.kept.max(usize::from(self.chrome))
    }

    /// The body's text, one line per row.
    #[cfg(test)]
    pub(in crate::tui) fn says(&self) -> String {
        self.rows
            .iter()
            .map(|row| {
                row.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The prefix of the row that starts a file in a patch.
const FILE: &str = "diff --git ";

/// The prefix of a hunk header.
const HUNK: &str = "@@";

/// The missing side of an added or deleted file.
const NOWHERE: &str = "/dev/null";

/// A patch file being read: its heading row, path, and counts so far.
struct Reading {
    at: usize,
    path: String,
    added: usize,
    removed: usize,
}

/// Replace the header block with a blank heading row, to be filled by
/// [`close`] once the counts are known.
fn open(rows: &mut Vec<Line<'static>>, headers: &mut Vec<&str>) -> Option<Reading> {
    if headers.is_empty() {
        return None;
    }
    let file = Reading {
        at: rows.len(),
        path: named(headers),
        added: 0,
        removed: 0,
    };
    rows.push(Line::default());
    headers.clear();
    Some(file)
}

/// Fill in a finished file's heading: path and counts.
fn close(rows: &mut [Line<'static>], file: Option<Reading>) {
    let Some(file) = file else {
        return;
    };
    let said = format!("{}  +{} -{}", inert(&file.path), file.added, file.removed);
    rows[file.at] = Line::styled(said, Style::default());
}

/// Store the gathered rows as the last hunk's text.
fn shut(hunks: &mut [Hunk], held: &mut Vec<&str>) {
    if held.is_empty() {
        return;
    }
    if let Some(hunk) = hunks.last_mut() {
        hunk.text = held.join("\n");
    }
    held.clear();
}

/// Whether a row is a git file header between `diff --git` and the first hunk.
fn header(row: &str) -> bool {
    const HEADERS: [&str; 13] = [
        "index ",
        "--- ",
        "+++ ",
        "old mode ",
        "new mode ",
        "new file mode ",
        "deleted file mode ",
        "similarity index ",
        "dissimilarity index ",
        "rename ",
        "copy ",
        "Binary files ",
        "GIT binary patch",
    ];
    HEADERS.iter().any(|header| row.starts_with(header))
}

/// The file's path from its headers: the `+++` side, else the `---` side (a
/// deleted file), else the `b/` path on the `diff --git` row (mode changes,
/// pure renames and binary files have no `---`/`+++`).
fn named(headers: &[&str]) -> String {
    let side = |mark: &str| {
        headers
            .iter()
            .find_map(|row| row.strip_prefix(mark))
            .filter(|path| *path != NOWHERE)
            .map(|path| {
                path.strip_prefix("a/")
                    .or_else(|| path.strip_prefix("b/"))
                    .unwrap_or(path)
                    .to_string()
            })
    };
    side("+++ ").or_else(|| side("--- ")).unwrap_or_else(|| {
        let opened = headers.first().copied().unwrap_or_default();
        let opened = opened.strip_prefix(FILE).unwrap_or(opened);
        opened
            .split_once(" b/")
            .map_or(opened.to_string(), |(_, new)| new.to_string())
    })
}

/// A hunk header's start line: the new side's, or the old side's when the new
/// side is empty (a pure deletion).
fn starts(row: &str) -> usize {
    let mut sides = row.trim_start_matches('@').trim_start().split(' ');
    let old = counted(sides.next().unwrap_or_default());
    let new = counted(sides.next().unwrap_or_default());
    match new.1 {
        0 => old.0,
        _ => new.0,
    }
}

/// One side of a hunk header as (start, count). A missing count means 1.
fn counted(side: &str) -> (usize, usize) {
    let side = side.trim_start_matches(['-', '+']);
    let (start, rows) = side.split_once(',').unwrap_or((side, "1"));
    (start.parse().unwrap_or(0), rows.parse().unwrap_or(1))
}

/// The card's scroll position, hunk cursor and review notes.
///
/// Offsets count rows from the body's natural edge (the bottom of a pane or
/// live conversation, the top of a forward body). Keys only add and subtract;
/// the paint clamps in [`Scroll::kept`], since only it knows the window
/// height. `Cell`s so the draw can write the clamp back.
#[derive(Default)]
pub struct Scroll {
    /// Rows between the window and the natural edge.
    pub away: Cell<usize>,
    /// Window height last frame; one page key moves by this.
    pub page: Cell<usize>,
    /// Where the card opened, clamped. `away != opened` means the user paged.
    pub opened: Cell<usize>,
    /// Where the card was asked to open, unclamped (past the end of a
    /// conversation). An unpaged card is re-clamped from this every frame so
    /// it stays on its last page when the window shrinks.
    anchor: Cell<usize>,
    /// The patch hunk under the cursor, if the user stepped to one.
    hunk: Cell<Option<usize>>,
    /// Review notes so far: keyed by hunk index, with `None` for text at the
    /// top of the patch. A `BTreeMap` so iteration is in patch order, which is
    /// the order the review message is built in.
    remarks: RefCell<BTreeMap<Option<usize>, String>>,
}

impl Scroll {
    /// Open a card `away` rows from its natural edge.
    ///
    /// Every card opens through here, so this also resets the hunk cursor and
    /// drops the notes, which belonged to the previous card's patch.
    pub fn open_at(&self, away: usize) {
        self.away.set(away);
        self.opened.set(away);
        self.anchor.set(away);
        self.hunk.set(None);
        self.remarks.borrow_mut().clear();
    }

    /// Store `words` as the note on hunk `at` (`None` for the top of the
    /// patch), or remove the note when `words` is blank, so the map's length is
    /// the note count.
    pub fn remark(&self, at: Option<usize>, words: &str) {
        match words.trim().is_empty() {
            true => self.remarks.borrow_mut().remove(&at),
            false => self.remarks.borrow_mut().insert(at, words.to_string()),
        };
    }

    /// The note on `at`, or an empty string.
    pub fn remarked(&self, at: Option<usize>) -> String {
        self.remarks.borrow().get(&at).cloned().unwrap_or_default()
    }

    /// Hunks with a note, in patch order. Text at the top of the patch is not
    /// a note on any hunk and is not counted.
    pub fn noted(&self) -> Vec<usize> {
        self.remarks.borrow().keys().copied().flatten().collect()
    }

    /// Every note: the top of the patch first, then hunks in patch order.
    pub fn remarks(&self) -> Vec<(Option<usize>, String)> {
        self.remarks
            .borrow()
            .iter()
            .map(|(at, words)| (*at, words.clone()))
            .collect()
    }

    /// Step the hunk cursor forward or back and scroll to that hunk's header.
    ///
    /// Before the first hunk is the top of the patch (no hunk), where a
    /// review's opening words go. Stepping stops at both ends rather than
    /// wrapping. The offsets are set as if opened (not paged) so the header
    /// stays at the top of the window, but unlike [`Scroll::open_at`] the notes
    /// are kept.
    pub fn to_hunk(&self, hunks: &[Hunk], forward: bool) {
        let Some(last) = hunks.len().checked_sub(1) else {
            return;
        };
        let at = match (self.hunk.get(), forward) {
            (None, true) => Some(0),
            (None, false) => return,
            (Some(at), true) => Some(at.saturating_add(1).min(last)),
            (Some(0), false) => None,
            (Some(at), false) => Some(at - 1),
        };
        let row = at.map_or(0, |at| hunks[at].row);
        self.away.set(row);
        self.opened.set(row);
        self.anchor.set(row);
        self.hunk.set(at);
    }

    /// The hunk under the cursor, if any.
    pub fn at_hunk(&self) -> Option<usize> {
        self.hunk.get()
    }

    /// Whether the user has paged away from where the card opened.
    pub fn paged(&self) -> bool {
        self.away.get() != self.opened.get()
    }

    /// Clamp the offset for a body of `length` rows in a `window`-row window,
    /// record the page size, and return the offset.
    ///
    /// `opened` is clamped too: a conversation opens past its end, and an
    /// unclamped `opened` would never equal `away` again, making
    /// [`Scroll::paged`] true forever. An unpaged card is re-clamped from
    /// `anchor` so it stays on its last page as the window changes; a paged
    /// card is clamped where it is.
    fn kept(&self, length: usize, window: usize) -> usize {
        let last = length.saturating_sub(window);
        let away = match self.paged() {
            true => {
                self.opened.set(self.opened.get().min(last));
                self.away.get().min(last)
            }
            false => {
                let edge = self.anchor.get().min(last);
                self.opened.set(edge);
                edge
            }
        };
        self.away.set(away);
        self.page.set(window.max(1));
        away
    }
}

/// The card's height: the rows it `wanted`, capped at half the terminal
/// (within [`CARD_SHORT`]..=[`CARD_TALL`]) and always leaving the list band a
/// row. Zero when not even [`CARD_SHORT`] fits.
pub(super) fn card_height(total: u16, band: u16, wanted: u16) -> u16 {
    let room = (total / 2)
        .clamp(CARD_SHORT, CARD_TALL)
        .min(wanted.max(CARD_SHORT))
        .min(band.saturating_sub(1));
    match room >= CARD_SHORT {
        true => room,
        false => 0,
    }
}

/// The rows the card would need to show everything: rule, pull requests,
/// tab strip, question, choices, the vendor's extra row, queued messages,
/// body, and the answer line with the blank row above it.
pub(super) fn card_rows(
    card: &Card<Body>,
    showing: Option<Showing>,
    prs: &[Pr],
    answering: Option<&Composer>,
    width: u16,
) -> u16 {
    let asked = card.question.as_deref().map_or(0, |question| {
        asked_rows(question, width).len().min(ASKED_TALL)
    });
    let listed = choices(&card.options, width as usize, boxed(showing)).len();
    // Capped: runs every frame, and a patch body can be thousands of rows.
    let shown = length(card).min(CARD_TALL as usize);

    let rows = RULE_ROW
        + usize::from(!prs.is_empty())
        + asked
        + usize::from(tab(showing).is_some())
        + listed
        + usize::from(added(card, showing).is_some())
        + answering.map_or(0, |line| line_rows(line, width) + GAP_ROW)
        + queued_rows(card)
        + shown;
    rows.min(u16::MAX as usize) as u16
}

/// Rows for queued messages: one each, at most [`QUEUED_TALL`].
fn queued_rows(card: &Card<Body>) -> usize {
    card.queued.len().min(QUEUED_TALL)
}

/// The most queued messages the card lists.
const QUEUED_TALL: usize = 3;

/// The dim suffix on a queued message's row.
const QUEUED: &str = " · queued";

/// The newest queued messages, one row each: the [`PROMPT`] glyph and the
/// message's first line in the waiting colour, then [`QUEUED`].
fn queued(card: &Card<Body>, width: usize, theme: Theme) -> Vec<Line<'static>> {
    let newest = card.queued.len().saturating_sub(QUEUED_TALL);
    card.queued[newest..]
        .iter()
        .map(|message| {
            let first = inert(message.lines().next().unwrap_or_default());
            let room = width.saturating_sub(width_of(PROMPT) + width_of(QUEUED));
            Line::from(vec![
                Span::styled(PROMPT, Style::new().fg(theme.waiting)),
                Span::styled(fit(&first, room), Style::new().fg(theme.waiting)),
                Span::styled(QUEUED, dim()),
            ])
        })
        .collect()
}

/// Rows of the answer line, grown like the task line and capped the same.
fn line_rows(line: &Composer, width: u16) -> usize {
    rows_of(line, width).min(COMPOSER_CAP)
}

/// The smallest card: just the rule.
const CARD_SHORT: u16 = 1;

/// The tallest card, whatever the terminal height.
const CARD_TALL: u16 = 14;

/// The rule's row.
const RULE_ROW: usize = 1;

/// The blank row above the answer line; the first row given up when short.
const GAP_ROW: usize = 1;

/// What the rule says of a patch card.
const CHANGED: &str = "what it has changed";

/// What the rule says of a running turn that has reported nothing yet, so
/// the rule never looks like the agent stopped.
const THINKING: &str = "thinking…";

/// What the rule says of a turn the vendor ended with background shells still
/// running.
fn shells_running(n: u32) -> String {
    match n {
        1 => "1 shell running".to_string(),
        n => format!("{n} shells running"),
    }
}

/// The most rows a wrapped question gets.
const ASKED_TALL: usize = 3;

/// Draw the card into `area`, over the foot of the list.
///
/// Top to bottom: the rule, pull requests, tab strip, question, choices, the
/// vendor's extra row, the body, queued messages, a blank row and the answer
/// line. Full width, since the body may be a pane capture.
///
/// `called` is the agent's name as the list shows it and `runs` its vendor
/// words, both for the rule. `on` (the list's reading of the agent) and
/// `beat` give the rule its state glyph and activity text.
#[allow(clippy::too_many_arguments)]
pub(super) fn float(
    frame: &mut Frame,
    card: &Card<Body>,
    on: Option<&View>,
    beat: usize,
    called: &str,
    runs: &str,
    showing: Option<Showing>,
    prs: &[Pr],
    answering: Option<&Composer>,
    scroll: &Scroll,
    area: Rect,
    theme: Theme,
) {
    // Clear the list rows the card covers.
    frame.render_widget(Clear, area);
    // The card is modal like a typed line, so the list above dims.
    behind(frame, area.y);
    // The answer line takes its rows first, but leaves at least one row for
    // the card's content, unless the rule leaves only one row. The blank row
    // above the line is only kept when content still fits above it.
    let spare = area.height.saturating_sub(RULE_ROW as u16);
    let wanted = answering.map_or(0, |line| line_rows(line, area.width) as u16);
    let typing = wanted
        .min(spare.saturating_sub(1))
        .max(wanted.min(spare).min(1));
    let gap = u16::from(typing > 0 && spare > typing + 1) * GAP_ROW as u16;
    let [ruled, between, _, typed] = Layout::vertical([
        Constraint::Length(RULE_ROW as u16),
        Constraint::Min(0),
        Constraint::Length(gap),
        Constraint::Length(typing),
    ])
    .areas(area);
    // Not indented: an indented pane capture puts the vendor's own chevrons
    // two columns off the answer line's chevron.
    let said = between;

    // Everything but the body takes its rows first; the body gets the rest.
    let mut room = said.height;
    let mut take = |wanted: u16| {
        let taken = wanted.min(room);
        room -= taken;
        taken
    };
    let open = requests(prs, theme);
    let opened = take(u16::from(!open.is_empty()));
    // The question and choices are made inert first. ratatui on its own would
    // delete invisible format characters, which lets one choice spoof
    // another's spelling; replacing them with spaces does not.
    let question = card
        .question
        .as_deref()
        .map(|question| asked_rows(question, said.width));
    let options: Vec<String> = card.options.iter().map(|option| inert(option)).collect();
    let asked = take(
        question
            .as_ref()
            .map_or(0, |rows| rows.len().min(ASKED_TALL) as u16),
    );
    // Which tab of a multi-question call this is, above its choices.
    let strip = tab(showing);
    let tabbed = take(u16::from(strip.is_some()));
    let choices = choices(&options, said.width as usize, boxed(showing));
    let listed = take(choices.len() as u16);
    let added = added(card, showing);
    let adding = take(u16::from(added.is_some()));
    // Queued messages sit just above the answer line.
    let waiting = take(queued_rows(card) as u16);

    // What is left is the body's window.
    let held = scroll.kept(length(card), room as usize);
    // Ignore a hunk cursor left over from a longer patch.
    let at = scroll.at_hunk().filter(|at| *at < card.body.hunks().len());
    let notes = scroll.noted();

    frame.render_widget(
        Paragraph::new(rule(
            card,
            on,
            beat,
            called,
            runs,
            held,
            at,
            notes.len(),
            area.width as usize,
            theme,
        )),
        ruled,
    );

    let [requesting, tabbing, asking, listing, adds, screen, sent] = Layout::vertical([
        Constraint::Length(opened),
        Constraint::Length(tabbed),
        Constraint::Length(asked),
        Constraint::Length(listed),
        Constraint::Length(adding),
        Constraint::Min(0),
        Constraint::Length(waiting),
    ])
    .areas(said);

    if opened > 0 {
        frame.render_widget(Paragraph::new(Line::from(open)), requesting);
    }
    if let Some(strip) = strip.filter(|_| tabbed > 0) {
        frame.render_widget(Paragraph::new(Line::styled(strip, dim())), tabbing);
    }
    if let Some(rows) = question {
        let lines: Vec<Line> = rows.into_iter().map(Line::raw).collect();
        frame.render_widget(
            Paragraph::new(lines).style(Style::new().fg(theme.waiting)),
            asking,
        );
    }
    if listed > 0 {
        let lines: Vec<Line> = choices
            .into_iter()
            .take(listed as usize)
            .map(Line::raw)
            .collect();
        frame.render_widget(Paragraph::new(lines), listing);
    }
    if let Some(added) = added.filter(|_| adding > 0) {
        frame.render_widget(Paragraph::new(Line::styled(added, dim())), adds);
    }
    if let Some(composer) = answering.filter(|_| typing > 0) {
        answer_row(frame, card, showing, composer, typed, theme);
    }

    frame.render_widget(
        Paragraph::new(body(card, screen.height as usize, held, at, &notes, theme)),
        screen,
    );
    if waiting > 0 {
        let rows = queued(card, sent.width as usize, theme);
        let newest = rows.len().saturating_sub(waiting as usize);
        frame.render_widget(Paragraph::new(rows[newest..].to_vec()), sent);
    }
}

/// The card's top row.
///
/// The agent's glyph (pulsing like its row) and name in its state colour;
/// then, dim: its vendor words, for a patch [`CHANGED`] with the hunk
/// position and note count, a dashed [`RULE`], what a running turn is doing,
/// and how many rows a paged body has beyond the window.
#[allow(clippy::too_many_arguments)]
fn rule(
    card: &Card<Body>,
    on: Option<&View>,
    beat: usize,
    called: &str,
    runs: &str,
    held: usize,
    at: Option<usize>,
    kept: usize,
    width: usize,
    theme: Theme,
) -> Line<'static> {
    // The card's phase; evidence and kind come from the list's reading, with
    // fallbacks when the list has lost the agent.
    let mark = format!(
        "{} ",
        icon(
            card.phase,
            on.map_or(&Evidence::Unknown, |view| &view.verdict.evidence),
            beat,
            on.is_some_and(|view| view.meta.agent.is_none()),
        )
    );
    let named = fit(&inert(called), width.saturating_sub(width_of(&mark)));
    let hunk = match at {
        Some(at) => format!("{SEPARATOR}hunk {} of {}", at + 1, card.body.hunks().len()),
        None => String::new(),
    };
    let noted = match kept {
        0 => String::new(),
        kept => format!("{SEPARATOR}{}", notes(kept)),
    };
    let changed = match card.changes {
        true => fit(
            &format!("{SEPARATOR}{CHANGED}{hunk}{noted}"),
            width.saturating_sub(width_of(&mark) + width_of(&named)),
        ),
        false => String::new(),
    };
    let more = match held {
        0 => String::new(),
        held => {
            let edge = match card.forward() {
                true => '↑',
                false => '↓',
            };
            format!(" {edge} {held} more")
        }
    };
    // Not the row's summary, which the card body already shows at length.
    // The vendor's spinner line if one was read, else the background shells
    // count, else `THINKING`.
    let doing = match card.phase {
        Phase::Starting | Phase::Working => format!(
            " {}",
            match on.map_or(0, |view| view.state.background) {
                0 => on
                    .and_then(|view| view.doing.as_deref())
                    .map(inert)
                    .filter(|said| !said.is_empty())
                    .unwrap_or_else(|| THINKING.to_string()),
                n => shells_running(n),
            }
        ),
        _ => String::new(),
    };
    // Reserve one dash so the rule never disappears entirely.
    let edge = width_of(&mark)
        + width_of(&named)
        + width_of(&changed)
        + 1
        + width_of(&more)
        + width_of(RULE);
    let doing = fit(&doing, width.saturating_sub(edge));
    // The vendor words get only the room left after everything else: a long
    // launch command must not crowd out what the turn is doing.
    let runs = match runs.is_empty() {
        true => String::new(),
        false => fit(
            &format!("{SEPARATOR}{runs}"),
            width.saturating_sub(edge + width_of(&doing)),
        ),
    };
    // Includes the space between the label and the dashes.
    let said = width_of(&mark)
        + width_of(&named)
        + width_of(&runs)
        + width_of(&changed)
        + 1
        + width_of(&doing)
        + width_of(&more);
    Line::from(vec![
        Span::styled(format!("{mark}{named}"), colour(theme, card.phase)),
        Span::styled(runs, dim()),
        Span::styled(changed, dim()),
        Span::raw(" "),
        Span::styled(RULE.repeat(width.saturating_sub(said)), dim()),
        Span::styled(doing, dim()),
        Span::styled(more, dim()),
    ])
}

/// "1 note" or "N notes", shared by the rule and the keys row.
pub(super) fn notes(kept: usize) -> String {
    match kept {
        1 => "1 note".to_string(),
        kept => format!("{kept} notes"),
    }
}

/// Whether the body is longer than the window, so the page key is worth
/// naming. Uses this frame's window: the keys row is drawn after the card.
pub(super) fn pages(card: &Card<Body>, scroll: &Scroll) -> bool {
    length(card) > scroll.page.get()
}

/// Rows the card's body has; zero for a card showing a question.
fn length(card: &Card<Body>) -> usize {
    match card.asks() && card.question.is_some() {
        true => 0,
        false => card.body.length(),
    }
}

/// Every pull request on the agent's branch, on one row: the number and its
/// standing in words, in the standing's colour (two standings share a colour,
/// hence the words).
///
/// Not made inert: the text is amx's own.
fn requests(prs: &[Pr], theme: Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for pr in prs {
        if !spans.is_empty() {
            spans.push(Span::styled(SEPARATOR, dim()));
        }
        spans.push(Span::styled(
            format!("{} {}", pr.label(), pr.standing.says()),
            request_colour(theme, pr.standing),
        ));
    }
    spans
}

/// The body's visible window, `rows` tall and `away` rows from its natural
/// edge: the bottom for a pane, the top for a forward body.
///
/// Empty for a card showing a recorded question: the pane under it only
/// repeats the question in the vendor's chrome. A waiting card whose question
/// amx has not read keeps its capture, the only place the question appears.
///
/// The header row of hunk `at` gets the cursor background; headers of hunks
/// in `notes` get the waiting colour.
pub(super) fn body(
    card: &Card<Body>,
    rows: usize,
    away: usize,
    at: Option<usize>,
    notes: &[usize],
    theme: Theme,
) -> Vec<Line<'static>> {
    if card.asks() && card.question.is_some() {
        return Vec::new();
    }

    let window = match card.forward() {
        true => head(card.body.kept, rows, away),
        false => tail(card.body.kept, rows, away),
    };
    let start = window.start;
    let mut shown = card.body.rows[window].to_vec();

    // Notes first, so a hunk with a note under the cursor gets both styles.
    for at in notes {
        if let Some(row) = card
            .body
            .hunks
            .get(*at)
            .and_then(|hunk| hunk.row.checked_sub(start))
            && let Some(line) = shown.get_mut(row)
        {
            line.style = line.style.fg(theme.waiting);
        }
    }

    // Usually the top row, but lower when the clamp pulled the window up at
    // the end of the patch.
    if let Some(row) = at
        .and_then(|at| card.body.hunks.get(at))
        .and_then(|hunk| hunk.row.checked_sub(start))
        && let Some(line) = shown.get_mut(row)
    {
        line.style = line.style.bg(theme.cursor);
    }

    // Only when furniture was cut: an agent that has printed nothing yet is
    // a different case.
    match shown.is_empty() && card.body.chrome {
        true => vec![Line::styled(ALL_CHROME, dim())],
        false => shown,
    }
}

#[cfg(test)]
thread_local! {
    /// How many bodies this thread has parsed from ANSI, the expensive part of
    /// building a card, so tests can assert when parsing happens. Per thread
    /// because tests run in parallel.
    static WALKS: Cell<usize> = const { Cell::new(0) };
}

/// How many ANSI bodies this thread has parsed.
#[cfg(test)]
pub(in crate::tui) fn walks() -> usize {
    WALKS.with(Cell::get)
}

/// A captured row's plain text, for the furniture cut. Built from the same
/// runs as the styled row, so the two cannot disagree.
fn words(row: &[Painted]) -> String {
    row.iter().map(|run| run.text.as_str()).collect()
}

/// A captured row with the vendor's styling.
fn as_painted(row: &[Painted]) -> Line<'static> {
    let spans: Vec<Span<'static>> = row
        .iter()
        .map(|run| Span::styled(inert(&run.text), paint(run)))
        .collect();
    Line::from(spans)
}

/// A run's SGR attributes as a ratatui style.
fn paint(run: &Painted) -> Style {
    let mut style = Style::new();
    for (on, modifier) in [
        (run.bold, Modifier::BOLD),
        (run.dim, Modifier::DIM),
        (run.italic, Modifier::ITALIC),
        (run.underline, Modifier::UNDERLINED),
        (run.reverse, Modifier::REVERSED),
    ] {
        if on {
            style = style.add_modifier(modifier);
        }
    }
    if let Some(fg) = run.fg {
        style = style.fg(shade(fg));
    }
    if let Some(bg) = run.bg {
        style = style.bg(shade(bg));
    }
    style
}

/// An SGR colour as a ratatui colour. The 16 basic colours map to named
/// colours so the terminal's own palette applies, as it does in the pane.
fn shade(colour: Colour) -> Color {
    match colour {
        Colour::Ansi(n) => ANSI[usize::from(n) & 0x0f],
        Colour::Indexed(n) => Color::Indexed(n),
        Colour::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// The 16 basic colours in ANSI order.
const ANSI: [Color; 16] = [
    Color::Black,
    Color::Red,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::Magenta,
    Color::Cyan,
    Color::Gray,
    Color::DarkGray,
    Color::LightRed,
    Color::LightGreen,
    Color::LightYellow,
    Color::LightBlue,
    Color::LightMagenta,
    Color::LightCyan,
    Color::White,
];

/// The body shown when the capture was nothing but vendor furniture.
pub(super) const ALL_CHROME: &str = "the pane shows nothing but the vendor's interface";

/// The tab strip for a multi-question call: the current tab's header and
/// "N of M". `None` for a single question.
///
/// claude 2.1.240 elides its own tab headers as the pane narrows (at 24
/// columns the current tab is just an ellipsis), so the pane cannot be relied
/// on for this.
fn tab(showing: Option<Showing>) -> Option<String> {
    let showing = showing.filter(|showing| showing.of > 1)?;
    let counted = format!("{} of {}", showing.at, showing.of);
    Some(match showing.header() {
        Some(header) => format!("{header}{SEPARATOR}{counted}"),
        None => counted,
    })
}

/// Whether the question takes several choices (checkboxes).
fn boxed(showing: Option<Showing>) -> bool {
    showing.is_some_and(|showing| showing.ask.multi)
}

/// The checkbox claude 2.1.240 draws between number and label. Always empty:
/// the payload does not say which boxes are checked.
const BOX: &str = "[ ]";

/// A note on the extra row the vendor draws under a question's choices, which
/// the payload does not include: a free-text row, or a notes field when the
/// choices carry previews. `None` for permission prompts, the trust screen,
/// and unread choices.
fn added(card: &Card<Body>, showing: Option<Showing>) -> Option<&'static str> {
    if card.options.is_empty() || card.kind != Some(Kind::Question) {
        return None;
    }
    match showing.is_some_and(|showing| showing.ask.takes_notes()) {
        true => Some(NOTES),
        false => Some(OTHER),
    }
}

/// The note for the vendor's free-text row.
const OTHER: &str = "plus a row for words of your own";

/// The note for the vendor's notes field (choices with previews).
const NOTES: &str = "plus a field for a note";

/// The choices, numbered by [`numbered`] (as `amx answer` and `ls` number
/// them) and packed onto as few rows as fit in `width`. A choice too wide
/// for a row is cut; its number stays visible.
///
/// `boxed` adds [`BOX`] after the number, so a multi-select question does not
/// look like one that submits on a number.
pub(super) fn choices(options: &[String], width: usize, boxed: bool) -> Vec<String> {
    let labels: Vec<String> = match boxed {
        true => options
            .iter()
            .map(|label| format!("{BOX} {label}"))
            .collect(),
        false => options.to_vec(),
    };

    let mut rows: Vec<String> = Vec::new();
    for choice in numbered(&labels) {
        let room = width.saturating_sub(width_of(&choice) + BETWEEN.len());
        match rows.last_mut() {
            Some(row) if width_of(row) <= room => {
                row.push_str(BETWEEN);
                row.push_str(&choice);
            }
            _ => rows.push(fit(&choice, width)),
        }
    }
    rows
}

/// Gap between choices on one row.
const BETWEEN: &str = "   ";

/// The answer line at the card's foot, drawn by [`typed_rows`] with
/// [`invites`] as its placeholder. The chevron wears the waiting colour at a
/// question, dim otherwise.
fn answer_row(
    frame: &mut Frame,
    card: &Card<Body>,
    showing: Option<Showing>,
    composer: &Composer,
    area: Rect,
    theme: Theme,
) {
    let asked = showing.map(|showing| showing.ask);
    let chevron = match card.asks() {
        true => Style::new().fg(theme.waiting),
        false => dim(),
    };
    typed_rows(
        frame,
        composer,
        area,
        chevron,
        Some(&invites(card, asked)),
        theme,
    );
}

/// The answer line's placeholder: what a question accepts (from
/// [`act::invitation`], which also words the refusal), else [`REPLY`],
/// [`RESUME`] for an ended agent, or [`NOBODY`] when nothing would receive it
/// (as [`act::reply`] refuses).
fn invites(card: &Card<Body>, asked: Option<&Ask>) -> String {
    match (card.asks(), card.listening) {
        (true, _) => act::invitation(card.kind, &card.options, asked, card.walked),
        (_, false) => NOBODY.to_string(),
        _ if card.phase.is_terminal() => RESUME.to_string(),
        _ => REPLY.to_string(),
    }
}

/// Placeholder on a live agent's card.
const REPLY: &str = "reply";

/// Placeholder on an ended agent's card: sending resumes the vendor, which
/// costs a turn, so the line says so before anything is typed.
const RESUME: &str = "resume";

/// Placeholder when no one would receive the line.
const NOBODY: &str = "nothing is listening";

/// The question made inert and word-wrapped to `width`. Used for both the
/// row count and the drawing, so they agree.
fn asked_rows(question: &str, width: u16) -> Vec<String> {
    composer_lines(&inert(question), width.max(1) as usize)
}

/// The window over a bottom-anchored body: `wanted` rows ending `back` rows
/// above row `end`.
pub(super) fn tail(end: usize, wanted: usize, back: usize) -> Range<usize> {
    let end = end.saturating_sub(back);
    end.saturating_sub(wanted)..end
}

/// The window over a top-anchored body: `wanted` rows starting `away` rows
/// down, within `end`.
fn head(end: usize, wanted: usize, away: usize) -> Range<usize> {
    let start = away.min(end);
    start..end.min(start.saturating_add(wanted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::View;
    use crate::tui::act::Asking;
    use crate::tui::paint::fixtures::{
        a_fleet, asking, block, cells, command, drawn, heading_of, on_a_branch, over_the_forge,
        painted, showing, theme, view,
    };
    use crate::tui::paint::header::space_rows;
    use crate::tui::paint::wall::{LIVE, pulse, set};
    use crate::tui::rows::FOLD_AT;
    use crate::tui::{Mode, Screen};

    /// The card's rows: from its rule down to (not including) the blank row
    /// and the keys row.
    fn card_lines(screen: &[String]) -> Vec<&str> {
        let Some(top) = screen.iter().position(|line| line.contains(RULE)) else {
            return Vec::new();
        };
        let foot = screen.len() - 1 - space_rows(screen.len() as u16) as usize;
        screen[top..foot].iter().map(String::as_str).collect()
    }

    /// The column `word` starts at in a drawn row (in chars, not bytes).
    fn column_of(line: &str, word: &str) -> usize {
        let at = line
            .find(word)
            .unwrap_or_else(|| panic!("{word:?} is not on {line:?}"));
        line[..at].chars().count()
    }

    /// The foreground colour of `word`'s first cell on a row.
    fn word_colour(screen: &Screen, size: (u16, u16), row: u16, word: &str) -> Color {
        let buffer = cells(screen, size);
        let line: String = (0..size.0)
            .map(|column| buffer[(column, row)].symbol())
            .collect();
        buffer[(column_of(&line, word) as u16, row)].fg
    }

    fn a_talk(prompt: &str, answer: &str) -> Vec<Said> {
        vec![
            Said::Prompt(prompt.to_string()),
            Said::Tool {
                name: "Bash".to_string(),
                detail: Some("cargo test".to_string()),
            },
            Said::Text(answer.to_string()),
        ]
    }

    #[test]
    fn card_draws_a_conversation_a_voice_a_glyph_and_anchors_on_its_end() {
        let mut told = a_talk("first ask", "first answer");
        told.extend(a_talk("second ask", "**second** answer"));
        let body = Body::conversation(&told, None, 40, theme());

        assert_eq!(
            body.says(),
            "❯ first ask\n\n› Bash cargo test\n\nfirst answer\n\n\
             ❯ second ask\n\n› Bash cargo test\n\nsecond answer",
            "the composer's glyph on a prompt, a tool's on a call, the words \
             drawn rather than their marks"
        );
        assert_eq!(body.kept, 11);
        assert_eq!(
            body.anchor(),
            11,
            "the end of it, past the last row: the paint clamps that down to \
             the last page the card has room for"
        );
        assert!(!body.chrome);

        // The prompt glyph is in the accent; a call's glyph and argument are
        // dim, its name is not.
        let prompt = &body.rows[6].spans[0];
        assert_eq!(prompt.content.as_ref(), PROMPT);
        assert_eq!(prompt.style.fg, Some(theme().accent));
        let call: Vec<(&str, bool)> = body.rows[8]
            .spans
            .iter()
            .map(|span| {
                (
                    span.content.as_ref(),
                    span.style.add_modifier.contains(Modifier::DIM),
                )
            })
            .collect();
        assert_eq!(
            call,
            vec![(TOOL, true), ("Bash", false), (" cargo test", true)]
        );
        assert!(
            body.rows[10]
                .spans
                .iter()
                .any(|span| span.content.contains("second")
                    && span.style.add_modifier.contains(Modifier::BOLD))
        );

        let empty = Body::conversation(&[], None, 40, theme());
        assert_eq!(empty.kept, 0);
        assert_eq!(empty.anchor(), 0);
    }

    #[test]
    fn card_stands_a_run_of_tool_calls_as_one_block_and_cuts_a_long_one() {
        let call = |name: &str, detail: Option<&str>| Said::Tool {
            name: name.to_string(),
            detail: detail.map(str::to_string),
        };
        let told = vec![
            Said::Prompt("look".to_string()),
            call("Read", Some("src/main.rs")),
            call("Bash", Some("cargo test")),
            call("ls", None),
            Said::Text("seen".to_string()),
        ];
        let body = Body::conversation(&told, None, 40, theme());
        assert_eq!(
            body.says(),
            "❯ look\n\n› Read src/main.rs\n› Bash cargo test\n› ls\n\nseen",
            "no blank row inside the run, one on either side of it"
        );

        // One row per call: the argument is cut to fit, or dropped.
        let body = Body::conversation(&[call("Bash", Some("cargo test --all"))], None, 14, theme());
        assert_eq!(body.says(), "› Bash cargo …");
        let body = Body::conversation(&[call("Bash", Some("cargo test"))], None, 6, theme());
        assert_eq!(body.says(), "› Bash");
    }

    #[test]
    fn card_ends_a_running_conversation_on_what_its_vendor_streams() {
        let told = a_talk("port it", "on it");
        let streamed = Body::conversation(&told, Some("still **going**"), 30, theme());
        assert_eq!(
            streamed.says(),
            "❯ port it\n\n› Bash cargo test\n\non it\n\nstill going",
            "the vendor's own stream one blank row under the record"
        );
        // No stream: the record alone, with no pane under it.
        let quiet = Body::conversation(&told, None, 30, theme());
        assert_eq!(quiet.says(), "❯ port it\n\n› Bash cargo test\n\non it");
        assert_eq!(quiet.anchor(), quiet.kept, "and reads up from its end");
    }

    #[test]
    fn card_keeps_the_last_rows_of_a_long_live_tail() {
        let told = a_talk("port it", "on it");
        // The rows after the record and its separator.
        let after_the_record = |body: &Body| -> Vec<String> {
            let said = body.says();
            let (_, tail) = said
                .split_once("on it\n\n")
                .expect("a tail under the record");
            tail.lines().map(str::to_string).collect()
        };

        // A long stream keeps its last rows.
        let streamed = (1..=20)
            .map(|n| format!("{n}. reason {n}\n"))
            .collect::<String>();
        let long = Body::conversation(&told, Some(&streamed), 30, theme());
        let tail = after_the_record(&long);
        assert_eq!(tail.len(), TAIL, "{tail:?}");
        assert!(tail[TAIL - 1].ends_with("reason 20"), "{tail:?}");
        assert!(tail[0].ends_with("reason 13"), "{tail:?}");

        let short = Body::conversation(&told, Some("one\n\ntwo"), 30, theme());
        assert_eq!(after_the_record(&short), ["one", "", "two"]);
    }

    #[test]
    fn card_stands_no_blank_row_over_a_stream_with_nothing_in_it() {
        // A stream that is open but empty, or only blank rows.
        let told = a_talk("port it", "on it");
        let said = "❯ port it\n\n› Bash cargo test\n\non it";
        for streamed in ["", "\n\n\n"] {
            let body = Body::conversation(&told, Some(streamed), 30, theme());
            assert_eq!(body.says(), said, "{streamed:?}");
            assert_eq!(body.kept, 5);
        }

        // One streamed row gets the separator.
        let landing = Body::conversation(&told, Some("reading the importer\n"), 30, theme());
        assert_eq!(
            landing.says(),
            format!("{said}\n\nreading the importer"),
            "one blank row between the record and the tail, and nothing else"
        );
    }

    /// A `git diff` with one changed file and one deleted file.
    const A_PATCH: &str = "\
diff --git a/src/foo.rs b/src/foo.rs
index 1234567..89abcde 100644
--- a/src/foo.rs
+++ b/src/foo.rs
@@ -1,4 +1,5 @@
 fn main() {
-    let old = 1;
+    let new = 2;
+    let more = 3;
 }
diff --git a/old.txt b/old.txt
deleted file mode 100644
index e69de29..0000000
--- a/old.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-gone
-and gone
\\ No newline at end of file";

    #[test]
    fn card_reads_a_patch_into_rows_by_kind_and_hunks() {
        let body = Body::patch(A_PATCH);

        assert_eq!(
            body.says(),
            "src/foo.rs  +2 -1\n\
             @@ -1,4 +1,5 @@\n\
             \x20fn main() {\n\
             -    let old = 1;\n\
             +    let new = 2;\n\
             +    let more = 3;\n\
             \x20}\n\
             old.txt  +0 -2\n\
             @@ -1,2 +0,0 @@\n\
             -gone\n\
             -and gone\n\
             \\ No newline at end of file",
            "each file's headers as one heading row with its counts, and every \
             other row whole"
        );
        assert_eq!(
            body.kept, 12,
            "counted as they are drawn, so the far end follows the collapsed \
             headers"
        );

        // git's colours; the heading row is unstyled.
        let paint: Vec<Style> = body.rows.iter().map(|row| row.style).collect();
        assert_eq!(paint[0], Style::default());
        assert_eq!(paint[7], Style::default());
        assert_eq!(paint[1].fg, Some(Color::Cyan), "the hunk header");
        assert_eq!(paint[3].fg, Some(Color::Red), "what it took away");
        assert_eq!(paint[4].fg, Some(Color::Green), "and what it added");
        assert_eq!(paint[2], dim(), "the context around them");
        assert_eq!(paint[11], dim(), "and the row about the missing newline");

        let hunks = body.hunks();
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].path, "src/foo.rs");
        assert_eq!(hunks[0].line, 1, "the new side's start");
        assert_eq!(hunks[0].row, 1, "which row of the body its header is");
        assert_eq!(
            hunks[0].text,
            "@@ -1,4 +1,5 @@\n fn main() {\n-    let old = 1;\n\
             +    let new = 2;\n+    let more = 3;\n }",
            "its own rows, as git wrote them"
        );
        assert_eq!(
            hunks[1].path, "old.txt",
            "the old side, where the new one is gone"
        );
        assert_eq!(
            hunks[1].line, 1,
            "and the old side's start, where the new side holds nothing"
        );
        assert_eq!(hunks[1].row, 8);
        assert!(
            hunks[1]
                .text
                .ends_with("-and gone\n\\ No newline at end of file"),
            "down to the last row before the next file: {:?}",
            hunks[1].text
        );

        assert!(Body::said("nothing to review here").hunks().is_empty());
    }

    #[test]
    fn card_steps_the_hunk_cursor_through_the_patch_and_back() {
        let body = Body::patch(A_PATCH);
        let scroll = Scroll::default();
        assert_eq!(scroll.at_hunk(), None, "a card opens on no hunk at all");

        // Stepping sets the offsets as opened, not paged.
        scroll.to_hunk(body.hunks(), true);
        assert_eq!(scroll.at_hunk(), Some(0));
        assert_eq!(scroll.away.get(), body.hunks()[0].row);
        assert!(!scroll.paged(), "which is where the card now opens");

        scroll.to_hunk(body.hunks(), true);
        assert_eq!(scroll.at_hunk(), Some(1), "and the next one after it");
        assert_eq!(scroll.away.get(), body.hunks()[1].row);
        scroll.to_hunk(body.hunks(), true);
        assert_eq!(scroll.at_hunk(), Some(1), "the last of them stays");

        scroll.to_hunk(body.hunks(), false);
        assert_eq!(scroll.at_hunk(), Some(0), "and back the way it came");
        assert_eq!(scroll.away.get(), body.hunks()[0].row);

        // Before the first hunk: the top of the patch, no hunk selected.
        scroll.to_hunk(body.hunks(), false);
        assert_eq!(scroll.at_hunk(), None, "the top of the patch itself");
        assert_eq!(scroll.away.get(), 0);
        assert!(!scroll.paged(), "which is where the card now opens");
        scroll.to_hunk(body.hunks(), false);
        assert_eq!(scroll.at_hunk(), None, "and the top stays");
        assert_eq!(scroll.away.get(), 0);

        // Opening a card resets the hunk cursor.
        scroll.to_hunk(body.hunks(), true);
        scroll.open_at(0);
        assert_eq!(scroll.at_hunk(), None);

        // Not a patch: nothing to step to.
        scroll.to_hunk(Body::said("nothing to review here").hunks(), true);
        assert_eq!(scroll.at_hunk(), None);
        assert_eq!(scroll.away.get(), 0, "and the card was left where it was");
    }

    #[test]
    fn card_keeps_a_remark_for_every_hunk_and_one_for_the_top() {
        let body = Body::patch(A_PATCH);
        let scroll = Scroll::default();
        assert_eq!(scroll.remarked(None), "", "nothing kept, nothing to read");
        assert!(scroll.remarks().is_empty());

        scroll.remark(Some(1), "this file can go");
        scroll.remark(None, "the whole of it reads well");
        scroll.remark(Some(0), "the name reads backwards");
        assert_eq!(scroll.remarked(Some(1)), "this file can go");
        assert_eq!(
            scroll.remarks(),
            vec![
                (None, "the whole of it reads well".to_string()),
                (Some(0), "the name reads backwards".to_string()),
                (Some(1), "this file can go".to_string()),
            ],
            "the opening first and the hunks in the order the patch writes them"
        );

        // A blank remark removes the note.
        scroll.remark(Some(0), " \n ");
        assert_eq!(scroll.remarked(Some(0)), "");
        assert_eq!(scroll.remarks().len(), 2);

        // Stepping keeps the notes.
        scroll.to_hunk(body.hunks(), true);
        scroll.to_hunk(body.hunks(), false);
        assert_eq!(scroll.remarks().len(), 2, "the step kept them");

        // Opening a card drops them.
        scroll.open_at(0);
        assert!(scroll.remarks().is_empty());
        assert_eq!(scroll.at_hunk(), None);
    }

    #[test]
    fn card_stands_the_hunk_under_the_cursor_at_the_top_and_counts_it_on_the_rule() {
        let screen = showing(
            vec![view("fix-login-a1b", Phase::Working, None, 3)],
            Some(Card {
                id: "fix-login-a1b".to_string(),
                phase: Phase::Working,
                question: None,
                options: Vec::new(),
                walked: false,
                kind: None,
                body: A_PATCH.to_string(),
                changes: true,
                answer: false,
                listening: true,
                queued: Vec::new(),
            }),
        );
        let size = (60, 24);

        // No hunk count until a hunk is selected.
        let all = painted(&screen, size).join("\n");
        assert!(all.contains("what it has changed"), "{all}");
        assert!(!all.contains("hunk"), "no hunk is under the cursor: {all}");

        let hunks = screen.card.as_ref().expect("the card").body.hunks();
        screen.scroll.to_hunk(hunks, true);

        let buffer = cells(&screen, size);
        let rows: Vec<String> = (0..size.1)
            .map(|row| {
                (0..size.0)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect()
            })
            .collect();
        let at = rows
            .iter()
            .position(|row| row.contains("@@ -1,4 +1,5 @@"))
            .expect("the hunk's header on the card");
        assert!(
            rows[at - 1].contains("what it has changed · hunk 1 of 2"),
            "the rule counts the hunks and says which one this is: {:?}",
            rows[at - 1]
        );
        assert_eq!(
            buffer[(2, at as u16)].bg,
            theme().cursor,
            "the header row is the row the cursor is standing on"
        );
        assert_ne!(
            buffer[(2, at as u16 + 1)].bg,
            theme().cursor,
            "and the rows of the hunk under it are not"
        );
        assert!(
            rows[at + 1].contains("fn main() {"),
            "the hunk stands at the top of the window: {:?}",
            &rows[at..]
        );
    }

    #[test]
    fn card_counts_the_notes_on_its_rule_and_marks_the_hunks_they_are_on() {
        let patch = || Card {
            id: "fix-login-a1b".to_string(),
            phase: Phase::Working,
            question: None,
            options: Vec::new(),
            walked: false,
            kind: None,
            body: A_PATCH.to_string(),
            changes: true,
            answer: false,
            listening: true,
            queued: Vec::new(),
        };
        let screen = showing(
            vec![view("fix-login-a1b", Phase::Working, None, 3)],
            Some(patch()),
        );
        // Wide enough for the whole rule.
        let size = (76, 24);

        // Text at the top of the patch is not counted as a note.
        screen.scroll.remark(None, "the whole of it reads well");
        let all = painted(&screen, size).join("\n");
        assert!(!all.contains("note"), "the opening is no note: {all}");

        screen.scroll.remark(Some(1), "this file can go");
        let all = painted(&screen, size).join("\n");
        assert!(
            all.contains("what it has changed · 1 note"),
            "one note kept: {all}"
        );

        // The note count follows the hunk position.
        screen.scroll.remark(Some(0), "the name reads backwards");
        let hunks = screen.card.as_ref().expect("the card").body.hunks();
        screen.scroll.to_hunk(hunks, true);
        let all = painted(&screen, size).join("\n");
        assert!(
            all.contains("· hunk 1 of 2 · 2 notes"),
            "both of them, after the hunk: {all}"
        );

        // Noted hunk headers wear the waiting colour; the selected one also
        // gets the cursor background.
        let rows = body(&patch().read(), 12, 0, Some(0), &[0, 1], theme());
        assert_eq!(
            rows[1].style.fg,
            Some(theme().waiting),
            "the header of the hunk under the cursor, noted"
        );
        assert_eq!(
            rows[1].style.bg,
            Some(theme().cursor),
            "and still the cursor"
        );
        assert_eq!(
            rows[8].style.fg,
            Some(theme().waiting),
            "and the other noted hunk, further down the patch"
        );
        assert_ne!(rows[8].style.bg, Some(theme().cursor));

        // An unnoted hunk header keeps git's colour.
        let rows = body(&patch().read(), 12, 0, None, &[1], theme());
        assert_eq!(rows[1].style.fg, Some(Color::Cyan));
        assert_eq!(rows[8].style.fg, Some(theme().waiting));
    }

    #[test]
    fn card_rule_says_what_the_agent_it_is_a_look_at_runs() {
        let patch = || Card {
            id: "fix-login-a1b".to_string(),
            phase: Phase::Working,
            question: None,
            options: Vec::new(),
            walked: false,
            kind: None,
            body: A_PATCH.to_string(),
            changes: true,
            answer: false,
            listening: true,
            queued: Vec::new(),
        };
        // Wide enough for the whole rule.
        let size = (80, 24);
        let ruled = |screen: &Screen| {
            let drawn = painted(screen, size);
            let at = drawn
                .iter()
                .position(|line| line.contains(RULE))
                .expect("the card's rule");
            (at as u16, drawn[at].clone())
        };

        let mut agent = view("fix-login-a1b", Phase::Working, None, 3);
        agent.meta.model = Some("opus".to_string());
        agent.meta.effort = Some("high".to_string());
        let screen = showing(vec![agent], Some(patch()));
        let (at, rule) = ruled(&screen);
        assert!(
            rule.starts_with(&format!(
                "{} fix-login-a1b · claude · opus · high · what it has changed",
                pulse(0)
            )),
            "the rule marks the agent and names it, then what it runs, and \
             what the card is showing after both: {rule:?}"
        );
        assert!(
            cells(&screen, size)[(column_of(&rule, "claude") as u16, at)]
                .modifier
                .contains(Modifier::DIM),
            "and what it runs is a fact about what the card is showing rather \
             than about the agent, so it is dim like the rest of them: {rule:?}"
        );

        // Unset dials are not shown.
        let plain = view("fix-login-a1b", Phase::Working, None, 3);
        let (_, rule) = ruled(&showing(vec![plain], Some(patch())));
        assert!(
            rule.starts_with(&format!(
                "{} fix-login-a1b · claude · what it has changed",
                pulse(0)
            )),
            "{rule:?}"
        );

        // A shell command shows `$` and `sh`, as on the wall.
        let (_, rule) = ruled(&showing(
            vec![command("fix-login-a1b", Phase::Working)],
            Some(patch()),
        ));
        assert!(
            rule.starts_with("$ fix-login-a1b · sh · what it has changed"),
            "marked for the kind of row it is, as the wall marks it: {rule:?}"
        );
    }

    #[test]
    fn card_rule_marks_the_state_and_says_what_the_agent_is_doing() {
        let size = (80, 20);
        let looked = |agent: View| {
            let card = Card {
                id: agent.meta.id.clone(),
                phase: agent.phase(),
                question: None,
                options: Vec::new(),
                walked: false,
                kind: None,
                body: "$ cargo test".to_string(),
                changes: false,
                answer: false,
                listening: true,
                queued: Vec::new(),
            };
            let mut screen = showing(vec![agent], Some(card));
            screen.beat = LIVE;
            painted(&screen, size)
                .into_iter()
                .find(|line| line.contains(RULE))
                .expect("the card's rule")
        };

        // A running turn: the row's pulse glyph, and activity at the far end.
        let working = looked(view(
            "fix-login-a1b",
            Phase::Working,
            Some("Running Bash"),
            3,
        ));
        assert!(
            working.starts_with(&format!("{} fix-login-a1b · claude ┈", pulse(LIVE))),
            "the rule opens on the glyph the row pulses: {working:?}"
        );
        assert!(
            working.ends_with("thinking…"),
            "and the far end says the turn is running, never the summary — the \
             card under the rule is what the agent is doing: {working:?}"
        );

        // The vendor's spinner line, when one was read off the pane.
        let mut spinning = view("fix-login-a1b", Phase::Working, Some("Running Bash"), 3);
        spinning.doing = Some("Nesting… (15s · ↓ 1.3k tokens)".to_string());
        let spinning = looked(spinning);
        assert!(
            spinning.ends_with("Nesting… (15s · ↓ 1.3k tokens)"),
            "the spinner line whole, at the far end: {spinning:?}"
        );

        // Background shells still running.
        let mut held = view("fix-login-a1b", Phase::Working, Some("2 shells running"), 3);
        held.state.background = 2;
        let held = looked(held);
        assert!(
            held.ends_with("2 shells running"),
            "the shells the turn is held open for: {held:?}"
        );

        let quiet = looked(view("fix-login-a1b", Phase::Working, None, 3));
        assert!(
            quiet.ends_with("thinking…"),
            "a working agent with nothing to say is thinking: {quiet:?}"
        );

        // An ended turn: the ended glyph and nothing at the far end.
        let done = looked(view(
            "old-job-b2c",
            Phase::Done,
            Some("did what it was asked"),
            60,
        ));
        assert!(
            done.starts_with("∙ old-job-b2c · claude ┈"),
            "the rule of an agent whose pane has gone: {done:?}"
        );
        assert!(
            done.ends_with(RULE),
            "run out to the far end, with nothing said at it: {done:?}"
        );
    }

    #[test]
    fn card_rule_keeps_a_dash_however_long_the_launch_command_is() {
        // A launch command path longer than the rule is cut before the last
        // dash is.
        let size = (60, 20);
        let mut agent = view("fix-login-a1b", Phase::Working, None, 3);
        agent.meta.agent = Some(
            "/home/somebody/.local/state/workflow/worktrees/amx/a-long-run-name/_integration/tests/mock_claude/claude"
                .to_string(),
        );
        let card = Card {
            id: "fix-login-a1b".to_string(),
            phase: Phase::Working,
            question: None,
            options: Vec::new(),
            walked: false,
            kind: None,
            body: "$ cargo test".to_string(),
            changes: false,
            answer: false,
            listening: true,
            queued: Vec::new(),
        };
        let screen = showing(vec![agent], Some(card));
        let drawn = painted(&screen, size);
        let rule = drawn
            .iter()
            .find(|line| line.starts_with(&format!("{} fix-login-a1b · ", pulse(0))))
            .expect("the card's rule");
        assert!(
            rule.contains(RULE),
            "the rule keeps at least one dash past the launch command: {rule:?}"
        );
        assert!(
            rule.contains('…'),
            "and it is the command that was cut: {rule:?}"
        );
    }

    #[test]
    fn card_takes_the_strength_off_the_wall_it_is_drawn_over() {
        // Like a typed line, a card dims everything above its rule.
        let size = (60, 20);
        let modifier = |screen: &Screen, word: &str| {
            let lines = painted(screen, size);
            let row = lines
                .iter()
                .position(|line| line.contains(word))
                .unwrap_or_else(|| panic!("{word} is not on {lines:?}"));
            let column = column_of(&lines[row], word) as u16;
            cells(screen, size)[(column, row as u16)].modifier
        };

        let quiet = showing(a_fleet(), None);
        assert!(
            !modifier(&quiet, "ask-a1b").contains(Modifier::DIM),
            "the name under the cursor comes up out of the dim while the keys \
             are the list's"
        );

        let carded = showing(
            a_fleet(),
            Some(asking(&["the sqlite one"], Some(Kind::Question))),
        );
        assert!(
            modifier(&carded, "ask-a1b").contains(Modifier::DIM),
            "and goes quiet with the rest the moment a card is up"
        );
        let buffer = cells(&carded, size);
        assert!(
            (0..size.0).all(|column| buffer[(column, 0)].modifier.contains(Modifier::DIM)),
            "the header behind goes dim to its last cell"
        );
        let rule = painted(&carded, size)
            .iter()
            .position(|line| line.contains(RULE))
            .expect("the card's rule");
        assert!(
            !buffer[(0, rule as u16)].modifier.contains(Modifier::DIM),
            "and the name on the card's own rule is not"
        );
    }

    #[test]
    fn card_stands_a_blank_row_between_what_it_says_and_its_line() {
        let question = || asking(&["the sqlite one", "the docker one"], Some(Kind::Question));
        let roomy = painted(&answering(question(), ""), (60, 20));
        let line = roomy
            .iter()
            .position(|row| row.starts_with('❯'))
            .expect("the line");
        assert_eq!(roomy[line - 1], "", "a blank row over the line: {roomy:?}");
        assert!(
            roomy[line - 2].contains("words of your own"),
            "and the card's last row over that: {roomy:?}"
        );

        // With room for only the rule, one content row and the line, the gap
        // goes.
        let tight = painted(
            &answering(asking(&["the sqlite one"], Some(Kind::Question)), ""),
            (60, 7),
        );
        let line = tight
            .iter()
            .position(|row| row.starts_with('❯'))
            .expect("the line");
        assert!(
            tight[line - 1].contains("sqlite") || tight[line - 1].contains("Which"),
            "the card's row stands against the line: {tight:?}"
        );
    }

    #[test]
    fn card_line_grows_a_row_at_a_time_as_the_task_line_does() {
        // Two rows, drawn like the task line, with the block at the end.
        let question = || asking(&["the sqlite one", "the docker one"], Some(Kind::Question));
        let screen = answering(question(), "one\ntwo");
        let drawn = painted(&screen, (60, 20));
        let first = drawn
            .iter()
            .position(|row| row.starts_with("❯ one"))
            .expect("the first row of the line");
        assert_eq!(drawn[first + 1], "  two", "{drawn:?}");
        assert_eq!(
            block(&screen, (60, 20), (first + 1) as u16),
            Some(5),
            "with the block at the end of the row the cursor is on"
        );
        assert_eq!(
            drawn[first - 1],
            "",
            "and the blank row still over it: {drawn:?}"
        );

        // A tall line is capped like the task line and never takes the rule
        // or the card's last content row.
        let tall = (1..=12)
            .map(|n| format!("row {n}"))
            .collect::<Vec<String>>()
            .join("\n");
        let screen = answering(question(), &tall);
        let drawn = painted(&screen, (60, 40));
        let rows = drawn
            .iter()
            .filter(|row| row.starts_with('❯') || row.starts_with("  row "))
            .count();
        assert_eq!(
            rows, COMPOSER_CAP,
            "capped where the task line is: {drawn:?}"
        );
        assert!(
            drawn
                .iter()
                .any(|row| row.starts_with(&format!("{} ask-a1b · claude ┈", set()[LIVE]))),
            "the rule stands: {drawn:?}"
        );
        assert!(
            drawn.iter().any(|row| row.contains("Which fixture")),
            "and a row of what the card says: {drawn:?}"
        );
    }

    #[test]
    fn card_line_offers_its_words_under_the_card() {
        // Completions go under the card and over the keys, as for the task
        // line.
        let question = || asking(&["the sqlite one"], Some(Kind::Question));
        let mut screen = answering(question(), "/rev");
        if let Mode::Typing(line) = &mut screen.mode {
            line.suggest = Some(act::Suggest {
                word: 0..4,
                entries: ["/review", "/revise"]
                    .iter()
                    .map(|spelled| crate::catalog::Entry {
                        spelled: spelled.to_string(),
                        kind: crate::catalog::Kind::Skill,
                        about: String::new(),
                    })
                    .collect(),
                chosen: 0,
            });
        }
        let drawn = painted(&screen, (60, 20));
        let line = drawn
            .iter()
            .position(|row| row.starts_with("❯ /rev"))
            .expect("the line");
        assert!(drawn[line + 1].contains("/review"), "{drawn:?}");
        assert!(drawn[line + 2].contains("/revise"), "{drawn:?}");
        assert!(
            drawn[19].contains("esc closes it"),
            "with the keys still the last row: {drawn:?}"
        );
    }

    #[test]
    fn card_stands_at_the_foot_under_a_rule_that_says_whose_it_is() {
        let screen = drawn(
            a_fleet(),
            Some(asking(
                &["the sqlite one", "the docker one"],
                Some(Kind::Question),
            )),
            (60, 14),
        );

        assert_eq!(heading_of(&screen[3]), "Needs input", "{screen:?}");
        assert!(
            screen[4].contains("ask-a1b"),
            "the row the card was opened from is still on the screen: {screen:?}"
        );

        let card = card_lines(&screen);
        let [ruled, asked, ..] = card.as_slice() else {
            panic!("no card in: {screen:?}")
        };
        assert!(
            ruled.starts_with(&format!("{} ask-a1b · claude ┈", set()[LIVE]))
                && ruled.ends_with('┈'),
            "the card opens on a rule carrying the mark of the agent it is a \
             look at, its name and what it runs, run out to the far end: \
             {ruled:?}"
        );
        assert!(
            asked.starts_with("Which fixture should the port keep?"),
            "and what the agent is asking stands under it: {asked:?}"
        );
        assert!(
            !screen.iter().any(|line| line.contains("Do you want to")),
            "and the pane it is asking on is not echoed under it: {screen:?}"
        );

        let top = screen
            .iter()
            .position(|line| line.contains(RULE))
            .expect("the card's rule");
        assert!(
            screen[..top].iter().any(|line| line.contains("busy-b2c")),
            "the whole list is above it rather than around it: {screen:?}"
        );
        assert_eq!(
            top + card.len(),
            screen.len() - 2,
            "and the keys are what is under it, standing off it by a row: \
             {screen:?}"
        );
    }

    #[test]
    fn card_moves_no_row_of_the_list_when_it_opens() {
        let group = || {
            vec![
                view("ask-a1b", Phase::Waiting, None, 29),
                view("ask-c3d", Phase::Waiting, None, 12),
            ]
        };
        let question = || asking(&["the sqlite one"], Some(Kind::Question));
        let bare = drawn(group(), None, (60, 20));
        let screen = drawn(group(), Some(question()), (60, 20));

        let top = screen
            .iter()
            .position(|line| line.contains(RULE))
            .expect("the card's rule");
        assert_eq!(
            screen[..top],
            bare[..top],
            "every row above the card is the row that was there without it"
        );
        assert!(
            screen[..top].iter().any(|line| line.contains("ask-c3d")),
            "the row under the one the card came off included: {screen:?}"
        );

        // A very short screen cuts the card and keeps a list row.
        let tight = drawn(group(), Some(question()), (60, 5));
        assert_eq!(
            card_lines(&tight).len(),
            2,
            "the rule and the one row it has left for what it says: {tight:?}"
        );
        assert!(
            tight.iter().any(|line| line.contains("ask-a1b")),
            "with a row of the list still standing: {tight:?}"
        );
    }

    /// The rows drawn for these readings and card.
    fn settled(views: Vec<View>, card: Option<Card>, size: (u16, u16)) -> Vec<String> {
        painted(&showing(views, card), size)
    }

    /// A finished agent's card with an answer long enough to fill the card.
    fn ending(id: &str) -> Card {
        Card {
            id: id.to_string(),
            phase: Phase::Done,
            question: None,
            options: Vec::new(),
            walked: false,
            kind: None,
            body: (0..40).map(|n| format!("said {n}\n")).collect(),
            changes: false,
            answer: true,
            listening: false,
            queued: Vec::new(),
        }
    }

    /// Six waiting and six finished agents: with two headings and a blank
    /// row, fifteen list rows, exactly the band of a 20-row screen.
    fn fifteen_rows() -> Vec<View> {
        (0..6)
            .map(|n| view(&format!("ask-{n:02}"), Phase::Waiting, None, 29))
            .chain(
                (0..6).map(|n| view(&format!("done-{n:02}"), Phase::Done, Some("did it"), 60 + n)),
            )
            .collect()
    }

    #[test]
    fn card_draws_over_the_foot_and_moves_no_row_under_a_walked_cursor() {
        // The cursor on the last of fifteen rows, under where the card goes.
        let size = (60, 20);
        let mut screen = showing(fifteen_rows(), None);
        let last = screen.list.items().len() - 1;
        assert_eq!(last + 1, 15, "fifteen rows: {:?}", screen.list.items());
        assert!(screen.list.land(last));
        let on = screen
            .list
            .selected()
            .expect("the last row")
            .id()
            .to_string();
        let bare = painted(&screen, size);
        let foot = bare
            .iter()
            .position(|line| line.contains(&on))
            .expect("the cursor's row");

        screen.card = Some(ending(&on).read());
        let carded = painted(&screen, size);
        let top = carded
            .iter()
            .position(|line| line.contains(RULE))
            .expect("the card's rule");
        assert!(
            top < foot,
            "the card is drawn over the foot of the list rather than under it: \
             {carded:?}"
        );
        assert_eq!(
            carded[..top],
            bare[..top],
            "and every row above it is the row that stood there without it"
        );
        assert!(
            !carded[..top].iter().any(|line| line.contains(&on)),
            "the row under the card is said by the card's rule alone: {carded:?}"
        );

        // Walking the cursor with the card up moves no row either.
        for at in (0..last).rev() {
            if !screen.list.land(at) {
                continue;
            }
            let walked = painted(&screen, size);
            assert_eq!(
                walked[..top],
                bare[..top],
                "the cursor walked to {at} and moved a row"
            );
        }
    }

    #[test]
    fn a_click_reads_the_row_the_card_left_where_it_was_and_none_under_the_card() {
        // Above the card, a point maps to the same item as without a card; on
        // the card it maps to none.
        let size = (60, 20);
        let mut screen = showing(fifteen_rows(), None);
        let last = screen.list.items().len() - 1;
        assert!(screen.list.land(last));
        let on = screen
            .list
            .selected()
            .expect("the last row")
            .id()
            .to_string();
        let _ = painted(&screen, size);
        let bare: Vec<Option<usize>> = (0..size.1)
            .map(|row| screen.map.line_under(5, row))
            .collect();

        screen.card = Some(ending(&on).read());
        let carded = painted(&screen, size);
        let top = carded
            .iter()
            .position(|line| line.contains(RULE))
            .expect("the card's rule") as u16;
        for row in 0..top {
            assert_eq!(
                screen.map.line_under(5, row),
                bare[row as usize],
                "row {row} names what it named with no card up: {carded:?}"
            );
        }
        for row in top..size.1 {
            assert_eq!(
                screen.map.line_under(5, row),
                None,
                "row {row} is the card's or the chrome's, and names no line"
            );
        }
        assert!(
            bare[(size.1 - 3) as usize].is_some(),
            "the card's band is rows the list itself was drawn in: {carded:?}"
        );
    }

    #[test]
    fn card_stands_over_the_list_and_folds_nothing_when_it_opens() {
        // A folded completed group: opening a card does not refold it.
        let fleet = || {
            (0..FOLD_AT + 10)
                .map(|n| view(&format!("done-{n:02}"), Phase::Done, Some("did it"), 60))
                .collect::<Vec<View>>()
        };
        let size = (60, (FOLD_AT + 10) as u16);
        let bare = settled(fleet(), None, size);
        assert!(
            bare.iter().any(|line| line.contains("more")),
            "a wall this long folds on a screen this short: {bare:?}"
        );

        let carded = settled(
            fleet(),
            Some(asking(&["the sqlite one"], Some(Kind::Question))),
            size,
        );
        let top = carded
            .iter()
            .position(|line| line.contains(RULE))
            .expect("the card's rule");
        assert_eq!(
            carded[..top],
            bare[..top],
            "every row above the card is the row that was there without it, \
             the fold uncut"
        );
    }

    #[test]
    fn card_stands_its_rows_in_the_band_column_the_rule_and_the_line_stand_in() {
        let screen = painted(
            &answering(
                asking(&["the sqlite one", "the docker one"], Some(Kind::Question)),
                "",
            ),
            (60, 14),
        );

        let card = card_lines(&screen);
        let [ruled, said @ .., line] = card.as_slice() else {
            panic!("no card in: {screen:?}")
        };
        assert_eq!(
            column_of(ruled, &format!("{} ask-a1b", set()[LIVE])),
            0,
            "the rule stands in the band's own column, marked the way the row \
             it came off is: {ruled:?}"
        );
        assert_eq!(
            column_of(line, "❯"),
            0,
            "and so does the line at its foot: {line:?}"
        );
        // So does every non-blank row between them.
        for row in said.iter().filter(|row| !row.is_empty()) {
            assert!(
                !row.starts_with(' '),
                "and what the card says hangs off the same edge, so a \
                 photograph of a terminal keeps its own left margin: {row:?}"
            );
        }
        assert!(
            !card
                .iter()
                .any(|row| row.contains('│') || row.contains('╰')),
            "with no spine down it and no corner under it: {card:?}"
        );
    }

    #[test]
    fn card_numbers_the_choices_the_question_offers() {
        let screen = drawn(
            a_fleet(),
            Some(asking(
                &["the sqlite one", "the docker one"],
                Some(Kind::Question),
            )),
            (60, 14),
        );
        assert!(
            screen
                .iter()
                .any(|line| line.contains("1. the sqlite one   2. the docker one")),
            "numbered the way every surface numbers them: {screen:?}"
        );
    }

    /// The view with `card` up and `typed` on its answer line.
    fn answering(card: Card, typed: &str) -> Screen {
        let mut screen = showing(a_fleet(), Some(card));
        let mut composer = Composer::new(Asking::Reply);
        // Cursor at the end, as after typing.
        composer.set_text(typed.to_string());
        screen.mode = Mode::Typing(composer);
        screen
    }

    /// The card's answer line.
    fn answer_row(screen: &[String]) -> String {
        screen
            .iter()
            .find(|line| line.contains('❯'))
            .unwrap_or_else(|| panic!("no row to answer on in: {screen:?}"))
            .clone()
    }

    /// The screen row of the card's answer line.
    fn line_row(screen: &[String]) -> u16 {
        screen
            .iter()
            .position(|line| line.contains('❯'))
            .unwrap_or_else(|| panic!("no row to answer on in: {screen:?}")) as u16
    }

    #[test]
    fn card_takes_the_answer_on_the_line_at_the_foot_of_the_card() {
        let question = || asking(&["the sqlite one", "the docker one"], Some(Kind::Question));
        let size = (60, 14);

        let empty = painted(&answering(question(), ""), size);
        assert!(
            answer_row(&empty).contains("❯ 1-2 picks, or type an answer"),
            "an empty row says what the question will take: {:?}",
            answer_row(&empty)
        );
        assert_eq!(
            card_lines(&empty).last().copied(),
            Some(answer_row(&empty).as_str()),
            "on the last row the card has: {empty:?}"
        );
        assert_eq!(
            empty[13], "enter attach   space closes it   esc closes it",
            "and the row under the card says what its own keys do, which while \
             the line is empty is what the two the line has no use for do"
        );

        let typed = painted(&answering(question(), "the docker one"), size);
        assert!(
            answer_row(&typed).contains("❯ the docker one"),
            "{:?}",
            answer_row(&typed)
        );
        assert!(
            !typed.iter().any(|line| line.contains("type an answer")),
            "what was typed takes the row the invitation had: {typed:?}"
        );
        assert!(
            !typed.iter().any(|line| line.starts_with("answer ask-a1b")),
            "and the line is on the card rather than on a band of its own \
             under it: {typed:?}"
        );
        assert_eq!(
            block(
                &answering(question(), "the docker one"),
                size,
                line_row(&typed)
            ),
            Some(16),
            "with the block at the end of what was typed, two cells in under \
             the chevron"
        );
        assert_eq!(
            block(&answering(question(), ""), size, line_row(&empty)),
            Some(2),
            "and on the first cell of the invitation while the line is empty, \
             which is where the answer will begin"
        );

        // With the cursor moved back, the block follows it.
        let mut walked = answering(question(), "the docker one");
        if let Mode::Typing(composer) = &mut walked.mode {
            composer.at = 4;
        }
        assert_eq!(block(&walked, size, line_row(&typed)), Some(6));
    }

    /// The colour and modifiers of the answer line's chevron.
    fn chevron(screen: &Screen, size: (u16, u16)) -> (Color, Modifier) {
        let row = line_row(&painted(screen, size));
        let cell = cells(screen, size);
        (cell[(0, row)].fg, cell[(0, row)].modifier)
    }

    #[test]
    fn card_line_says_what_it_will_take_on_every_kind_of_card() {
        let size = (60, 14);

        // A question: what it accepts, chevron in the waiting colour.
        let question = answering(
            asking(&["the sqlite one", "the docker one"], Some(Kind::Question)),
            "",
        );
        let asked = answer_row(&painted(&question, size));
        assert!(
            asked.contains("❯ 1-2 picks, or type an answer"),
            "{asked:?}"
        );
        assert_eq!(chevron(&question, size).0, theme().waiting);

        // A working agent: "reply".
        let busy = answering(
            Card {
                phase: Phase::Working,
                question: None,
                options: Vec::new(),
                body: "$ cargo test".to_string(),
                ..asking(&[], None)
            },
            "",
        );
        let working = answer_row(&painted(&busy, size));
        assert!(working.contains("❯ reply"), "{working:?}");
        assert_eq!(
            chevron(&busy, size),
            (Color::Reset, Modifier::DIM),
            "with the chevron dim: a line somebody may type at is not one \
             waiting on them"
        );

        // An ended agent that can be resumed: "resume".
        let ended = |listening| Card {
            phase: Phase::Done,
            question: None,
            options: Vec::new(),
            body: "did what it was asked".to_string(),
            answer: true,
            listening,
            ..asking(&[], None)
        };
        let back = answering(ended(true), "");
        let comes_back = answer_row(&painted(&back, size));
        assert!(comes_back.contains("❯ resume"), "{comes_back:?}");

        // One that cannot: the words the reply would be refused in.
        let over = answering(ended(false), "");
        let past = answer_row(&painted(&over, size));
        assert!(past.contains("❯ nothing is listening"), "{past:?}");
        assert_eq!(chevron(&over, size), (Color::Reset, Modifier::DIM));
    }

    #[test]
    fn card_is_no_taller_than_what_it_has_to_show() {
        let brief = Card {
            phase: Phase::Done,
            question: None,
            options: Vec::new(),
            body: "did what it was asked".to_string(),
            ..asking(&[], None)
        };
        let screen = drawn(a_fleet(), Some(brief), (60, 20));
        let card = card_lines(&screen);
        assert_eq!(
            card.len(),
            2,
            "the rule and the one line it has: {screen:?}"
        );
        assert!(card[1].starts_with("did what it was asked"), "{screen:?}");

        let top = screen
            .iter()
            .position(|line| line.contains(RULE))
            .expect("the card's rule");
        for name in ["ask-a1b", "busy-b2c"] {
            assert!(
                screen[..top].iter().any(|line| line.contains(name)),
                "with the rows it is not taking still on the wall above it: \
                 {screen:?}"
            );
        }
    }

    #[test]
    fn card_keeps_the_row_being_typed_on_when_there_is_room_for_little_else() {
        // One row under the rule goes to the answer line.
        let screen = painted(
            &answering(asking(&["the sqlite one"], Some(Kind::Question)), "the sq"),
            (60, 6),
        );
        assert!(answer_row(&screen).contains("❯ the sq"), "{screen:?}");
        assert_eq!(
            screen[5], "enter answers it   alt+enter newline   esc closes it",
            "with the card's own keys under it, which a line holding a word \
             are the line's"
        );
    }

    #[test]
    fn card_invites_only_the_answers_the_question_will_take() {
        // A permission prompt takes no free text: typed words would land on
        // the highlighted choice.
        let box_office = Card {
            kind: Some(Kind::Permission),
            question: Some("Claude needs your permission to use Bash".to_string()),
            ..asking(&["Yes", "No"], None)
        };
        let asked = answer_row(&painted(&answering(box_office, ""), (60, 14)));
        assert!(asked.contains("❯ press 1-2, y or n"), "{asked:?}");
        assert!(
            !asked.contains("type"),
            "a hint that offers what the prompt will refuse is a hint that \
             lies: {asked:?}"
        );

        // With no answer line open, the list's keys show.
        let looking = painted(&showing(a_fleet(), Some(asking(&[], None))), (60, 14));
        assert_eq!(
            looking[13],
            "space closes it   enter attach   ctrl+x stop   ? keys"
        );
        assert!(
            !looking.iter().any(|line| line.contains('❯')),
            "with no row to answer on: {looking:?}"
        );
    }

    #[test]
    fn card_packs_the_choices_onto_as_few_rows_as_it_is_wide() {
        let two = ["the sqlite one".to_string(), "the docker one".to_string()];
        assert_eq!(
            choices(&two, 40, false),
            ["1. the sqlite one   2. the docker one"]
        );
        assert_eq!(
            choices(&two, 20, false),
            ["1. the sqlite one", "2. the docker one"],
            "and one to a row where they will not sit together"
        );
        assert_eq!(
            choices(&two, 10, false),
            ["1. the sq…", "2. the do…"],
            "a choice wider than the card is cut, and says it was"
        );
        assert!(choices(&[], 40, false).is_empty());
    }

    #[test]
    fn card_gives_the_question_every_row_its_words_wrap_to() {
        // 37 chars: two rows of 20 when cut anywhere, three at the spaces.
        let mut card = asking(&[], None);
        card.question = Some("reconciliation authentication tokens?".to_string());
        let screen = drawn(a_fleet(), Some(card), (20, 24));
        assert!(
            screen.iter().any(|line| line.trim() == "tokens?"),
            "{screen:?}"
        );

        // A newline in the question starts a row of its own.
        let mut card = asking(&[], None);
        card.question = Some("Keep it?\nThe port\nneeds one".to_string());
        let screen = drawn(a_fleet(), Some(card), (40, 24));
        assert!(
            screen.iter().any(|line| line.trim() == "needs one"),
            "{screen:?}"
        );
    }

    #[test]
    fn card_packs_wide_choices_by_the_columns_they_take() {
        let wide = ["日本語日本語".to_string(), "b".to_string()];
        assert_eq!(choices(&wide, 20, false), ["1. 日本語日本語", "2. b"]);
    }

    #[test]
    fn card_gives_a_question_that_takes_several_a_box_beside_every_choice() {
        let two = ["the sqlite one".to_string(), "the docker one".to_string()];
        assert_eq!(
            choices(&two, 50, true),
            ["1. [ ] the sqlite one   2. [ ] the docker one"],
            "the vendor's own box, between the number and the label"
        );
        assert_eq!(
            choices(&two, 20, true),
            ["1. [ ] the sqlite o…", "2. [ ] the docker o…"],
            "and a narrow card cuts the label rather than the box, because the \
             box is what says the row is one"
        );
    }

    #[test]
    fn card_neutralises_the_question_and_the_choices_it_quotes() {
        // A bidi override in the question could visually reorder the choices.
        // ratatui drops control characters itself but keeps format characters,
        // so those must be made inert first.
        let mut card = asking(&["yes\u{200b}really", "no\u{ad}pe"], Some(Kind::Question));
        card.question = Some("pro\u{ad}ceed\u{202e}?".to_string());
        let screen = drawn(a_fleet(), Some(card), (60, 14)).join("\n");

        for (invisible, name) in [
            ('\u{202e}', "a bidi override"),
            ('\u{200b}', "a zero-width space"),
            ('\u{ad}', "a soft hyphen"),
        ] {
            assert!(
                !screen.contains(invisible),
                "{name} reached the terminal: {screen:?}"
            );
        }
        assert!(screen.contains("pro ceed"), "{screen:?}");
    }

    #[test]
    fn card_shows_the_question_alone_and_none_of_the_pane_it_is_asked_on() {
        // The pane only repeats the question in the vendor's chrome.
        let screen = drawn(
            vec![view("ask-a1b", Phase::Waiting, None, 30)],
            Some(Card {
                question: Some("Claude needs your permission to use Bash".to_string()),
                body: "$ rm -rf build\nDo you want to proceed?\n\n\n".to_string(),
                options: Vec::new(),
                kind: Some(Kind::Permission),
                ..asking(&[], None)
            }),
            (60, 12),
        );

        let all = screen.join("\n");
        assert!(all.contains("Claude needs your permission"), "{all}");
        assert!(
            !all.contains("Do you want to proceed?"),
            "the question block is the whole of the card: {all}"
        );
        assert_eq!(
            screen[11], "space closes it   enter attach   ctrl+x stop   ? keys",
            "the keys stay on the screen under the card, saying what they do \
             while it is up"
        );
        assert!(
            screen.iter().any(|line| line.contains("ask-a1b")),
            "and the list is still there above it: {all}"
        );

        assert_eq!(
            card_lines(&screen).len(),
            2,
            "and the card is its rule and the question, with no window kept \
             for a pane it will not draw: {screen:?}"
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
            Some(Card {
                id: "fix-login-a1b".to_string(),
                phase: Phase::Working,
                question: None,
                options: Vec::new(),
                walked: false,
                kind: None,
                body: patch,
                changes: true,
                answer: false,
                listening: true,
                queued: Vec::new(),
            }),
            (60, 14),
        );

        let all = screen.join("\n");
        assert!(
            all.contains("what it has changed"),
            "a card holding a patch says so on its rule, because the row it \
             came off says what the agent is doing and this is not that: \
             {all}"
        );
        assert!(
            all.contains("+ line 0"),
            "the first of it, not the last: {all}"
        );
        assert!(!all.contains("+ line 39"), "{all}");
    }

    /// A patch card with `lines` added lines.
    fn a_long_patch(lines: usize) -> Card {
        Card {
            id: "fix-login-a1b".to_string(),
            phase: Phase::Working,
            question: None,
            options: Vec::new(),
            walked: false,
            kind: None,
            body: (0..lines)
                .map(|n| format!("+ line {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
            changes: true,
            answer: false,
            listening: true,
            queued: Vec::new(),
        }
    }

    #[test]
    fn card_pages_a_patch_from_its_offset_and_says_how_far() {
        let screen = showing(
            vec![view("fix-login-a1b", Phase::Working, None, 3)],
            Some(a_long_patch(40)),
        );
        screen.scroll.away.set(20);

        let all = painted(&screen, (60, 14)).join("\n");
        assert!(all.contains("+ line 20"), "the page it was sent to: {all}");
        assert!(!all.contains("+ line 0"), "{all}");
        assert!(!all.contains("+ line 39"), "{all}");
        assert!(all.contains("↑ 20 more"), "how far from the top: {all}");
        assert_eq!(screen.scroll.away.get(), 20);
    }

    #[test]
    fn card_stops_a_page_at_the_end_of_the_patch() {
        let screen = showing(
            vec![view("fix-login-a1b", Phase::Working, None, 3)],
            Some(a_long_patch(40)),
        );
        screen.scroll.away.set(1000);

        let all = painted(&screen, (60, 14)).join("\n");
        assert!(all.contains("+ line 39"), "the last of it: {all}");
        assert_eq!(
            screen.scroll.away.get(),
            40 - screen.scroll.page.get(),
            "written back as the last page there is: {all}"
        );
    }

    #[test]
    fn card_holds_a_fitting_body_at_its_edge() {
        let screen = showing(
            vec![view("fix-login-a1b", Phase::Working, None, 3)],
            Some(a_long_patch(3)),
        );
        screen.scroll.away.set(5);

        let all = painted(&screen, (60, 14)).join("\n");
        assert!(all.contains("+ line 0"), "{all}");
        assert!(all.contains("+ line 2"), "{all}");
        assert!(!all.contains("more"), "nothing is hidden: {all}");
        assert_eq!(screen.scroll.away.get(), 0, "nothing to page over");
    }

    #[test]
    fn card_pages_a_recorded_answer_down_from_its_top() {
        let answered = || {
            showing(
                vec![view("fix-login-a1b", Phase::Done, None, 3)],
                Some(Card {
                    id: "fix-login-a1b".to_string(),
                    phase: Phase::Done,
                    question: None,
                    options: Vec::new(),
                    walked: false,
                    kind: None,
                    body: (0..40)
                        .map(|n| format!("said {n}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    changes: false,
                    answer: true,
                    listening: true,
                    queued: Vec::new(),
                }),
            )
        };

        // Opens at the top.
        let opened = painted(&answered(), (60, 14)).join("\n");
        assert!(opened.contains("said 0"), "{opened}");
        assert!(!opened.contains("said 39"), "{opened}");

        // Paged, it is `away` rows below the top.
        let screen = answered();
        screen.scroll.away.set(7);
        let all = painted(&screen, (60, 14)).join("\n");
        assert!(all.contains("said 7"), "seven rows below the top: {all}");
        assert!(
            !all.contains("said 0"),
            "the first words are behind it: {all}"
        );
        assert!(all.contains("↑ 7 more"), "how far from the top: {all}");
        assert_eq!(screen.scroll.away.get(), 7);
    }

    #[test]
    fn card_gives_a_long_answer_its_whole_allowance() {
        // 40 rows of answer on a 20-row screen: the card takes its full
        // allowance and pages the rest.
        let long: String = (0..40).map(|n| format!("said {n}\n")).collect();
        let card = Card {
            phase: Phase::Done,
            question: None,
            options: Vec::new(),
            body: long,
            answer: true,
            ..asking(&[], None)
        };
        let screen = drawn(a_fleet(), Some(card), (60, 20));

        let card = card_lines(&screen);
        assert_eq!(
            card.len(),
            10,
            "half the screen, the card's cap: {screen:?}"
        );
        assert!(
            card[1].contains("said 0"),
            "opened under its rule at the answer's first words: {screen:?}"
        );
    }

    #[test]
    fn card_holding_a_question_never_leaves_its_edge() {
        let screen = showing(a_fleet(), Some(asking(&["1. Yes", "2. No"], None)));
        screen.scroll.away.set(5);

        let all = painted(&screen, (60, 14)).join("\n");
        assert!(!all.contains("more"), "{all}");
        assert_eq!(
            screen.scroll.away.get(),
            0,
            "a question block does not page"
        );
    }

    #[test]
    fn wide_text_in_a_question_gets_every_row_it_needs() {
        // 42 wide chars: fewer chars than the card's width, more cells than
        // one row.
        let question = format!("{}終わり", "日本語".repeat(13));
        let mut card = asking(&["1. Yes", "2. No"], None);
        card.question = Some(question.clone());

        let screen = painted(&showing(a_fleet(), Some(card)), (60, 30));
        // A wide char's second cell reads back as a space.
        let all: String = screen.concat().replace(' ', "");
        assert!(all.contains(&question), "{screen:#?}");
    }

    /// A capture with ANSI styling, so building a body has to parse it.
    const PAINTED: &str = "\u{1b}[1mwrote the parser\u{1b}[0m\n\u{1b}[32m+ done\u{1b}[0m";

    #[test]
    fn card_walks_its_body_when_it_is_built_and_never_again_on_a_frame() {
        let mut card = asking(&[], None);
        card.phase = Phase::Working;
        card.question = None;
        card.body = PAINTED.to_string();

        let walked = walks();
        let screen = showing(a_fleet(), Some(card));
        assert_eq!(
            walks(),
            walked + 1,
            "the body is walked out of its escapes where the card is built"
        );

        // Redraws do not parse again.
        for _ in 0..3 {
            let drawn = painted(&screen, (60, 14)).join("\n");
            assert!(drawn.contains("wrote the parser"), "{drawn}");
        }
        assert_eq!(
            walks(),
            walked + 1,
            "and every frame after it draws from that walk"
        );
    }

    #[test]
    fn view_reads_the_bottom_of_a_screen_and_drops_what_is_blank() {
        let shown = |text: &'static str, wanted: usize, back: usize| {
            // Trailing blank rows are dropped when the body is built, before
            // `tail` sees it.
            let rows: Vec<&str> = text.lines().collect();
            let mut kept = rows.len();
            while kept > 0 && rows[kept - 1].trim().is_empty() {
                kept -= 1;
            }
            rows[tail(kept, wanted, back)].to_vec()
        };
        assert_eq!(shown("a\nb\nc\n\n\n", 2, 0), ["b", "c"]);
        assert_eq!(shown("a\nb", 5, 0), ["a", "b"]);
        assert!(shown("", 3, 0).is_empty());
        // Paged back, the window moves up from the bottom.
        assert_eq!(shown("a\nb\nc\nd\n\n", 2, 1), ["b", "c"]);
        assert!(shown("a\nb", 2, 5).is_empty());
    }

    /// The five rows claude draws at the bottom of a pane: the composer's top
    /// border with its right-anchored label, the staged text, the bottom
    /// border, the statusline, and the mode footer. Transcribed from claude
    /// 2.1.237 at 100 columns.
    const CHROME: [&str; 5] = [
        "───────────────────────────── execute amx-v2 tail ─",
        "❯ ",
        "───────────────────────────────────────────────────",
        "  Opus 5 │ ◈ 0% │ amx-main (main) │ ◖ xhigh",
        "  ⏵⏵ accept edits on (shift+tab to cycle) · ← 3 agents",
    ];

    /// A row of the agent's output, which the cut must never take.
    const SAID: &str = "what the agent said";

    /// claude's furniture anchors, which [`CHROME`] matches.
    fn chrome() -> &'static Furniture {
        crate::rules::of("claude").furniture()
    }

    /// [`CHROME`] with `typed` staged in the composer, under a row of output.
    fn staged(typed: &[&'static str]) -> Vec<&'static str> {
        let mut screen = vec![SAID, CHROME[0]];
        screen.extend_from_slice(typed);
        screen.extend_from_slice(&CHROME[2..]);
        screen
    }

    #[test]
    fn view_tail_cuts_the_chrome_claude_draws_under_every_pane() {
        let mut screen = vec![SAID, "", "✻ Nesting… (15s · thinking)", ""];
        screen.extend_from_slice(&CHROME);
        assert_eq!(
            chrome().cut(&screen),
            [SAID, ""].as_slice(),
            "the spinner goes with the box it sits over"
        );
    }

    #[test]
    fn view_tail_cuts_a_composer_whatever_is_staged_in_it() {
        // Multi-row staged text only: a one-row composer would also pass a cut
        // that removes exactly one input row.
        let wrapped = staged(&[
            "❯ port the importer and then check every",
            "  call site that used to take the old",
            "  shape",
        ]);
        assert_eq!(chrome().cut(&wrapped), [SAID].as_slice());

        let lines = staged(&["❯ first", "  second", "  third", "  fourth"]);
        assert_eq!(chrome().cut(&lines), [SAID].as_slice());
    }

    #[test]
    fn view_tail_leaves_a_screen_the_vendor_drew_no_footer_under_alone() {
        // A permission prompt ends at its confirm row; cutting it would take
        // the question.
        let prompt = [
            "───────────────────────────────────",
            " Bash command",
            "   rm -rf build",
            " Do you want to proceed?",
            " ❯ 1. Yes",
            "   2. No",
            " Esc to cancel · Tab to amend",
        ];
        assert_eq!(chrome().cut(&prompt), prompt.as_slice());

        // A pane too short for the footer, ending on the composer's border.
        let short = [SAID, CHROME[0], CHROME[1], CHROME[2]];
        assert_eq!(chrome().cut(&short), short.as_slice());
    }

    #[test]
    fn view_tail_gives_back_by_position_what_it_cannot_place() {
        // claude 2.1.263 draws up to eight statusline rows (all four of four,
        // eight of ten). Any count within that is stepped over.
        for rows in [4, 8] {
            let mut tall = vec![SAID, CHROME[2]];
            tall.extend((0..rows).map(|_| "  status"));
            tall.push(CHROME[4]);
            assert_eq!(chrome().cut(&tall), &tall[..1], "{rows} rows");
        }

        // Nine rows is not a shape claude draws: the statusline step gives up
        // and only the footer, matched by its own opener, is cut.
        let mut odd = vec![SAID, CHROME[2]];
        odd.extend((0..9).map(|_| "  status"));
        odd.push(CHROME[4]);
        assert_eq!(chrome().cut(&odd), &odd[..odd.len() - 1]);

        // Staged text taller than half the capture: the scan hits its cap
        // before a top border and gives the composer rows back.
        let mut runaway = vec![SAID];
        runaway.extend((0..8).map(|_| "  typed"));
        runaway.extend_from_slice(&CHROME[2..]);
        assert_eq!(
            chrome().cut(&runaway),
            &runaway[..runaway.len() - 3],
            "the footer, the statusline and the bottom border keep their anchors"
        );
    }

    /// `capture-pane -p -J` of claude 2.1.237 at 72 columns with a task
    /// wrapped over three composer rows. Verbatim, including trailing spaces
    /// and the no-break space after the chevron, which a transcription would
    /// lose.
    const CAPTURED: [&str; 9] = [
        "what the agent said",
        "  tmux detected · scroll with PgUp/PgDn · or add 'set -g mouse on' to…",
        "────────────────────────────────────────────────── execute amx-v2 tail ─",
        "❯\u{a0}check every call site 1 check every call site 2 check every call      ",
        "  site 3 check every call site 4 check every call site 5 check every    ",
        "  call site 6                                             ",
        "────────────────────────────────────────────────────────────────────────",
        "  Opus 5 (1M context) (1M context) │ ◈ 0% │ amx-main (main) │ ◖ xhigh",
        "  ⏵⏵ accept edits on (shift+tab to cycle)               ",
    ];

    #[test]
    fn view_tail_cuts_what_a_live_vendor_actually_drew() {
        // Pane padding under the last drawn row.
        let mut screen = CAPTURED.to_vec();
        screen.push("");

        // The warning flush against the composer's top border stays; a cut
        // that ran up to the nearest blank row would have taken it.
        assert_eq!(chrome().cut(&screen), &CAPTURED[..2]);
    }

    #[test]
    fn view_tail_cuts_the_spinner_however_much_of_it_the_vendor_drew() {
        // claude's spinner row: bare before the first token (seen at
        // `--effort low`), and with elapsed time and detail mid-turn.
        for spinner in [
            "● Actioning…",
            "✶ Forging… (9s · thinking with xhigh effort)",
        ] {
            let screen = [
                "what the agent said",
                "",
                spinner,
                "",
                "────────────────────────────────",
                "❯\u{a0}",
                "────────────────────────────────",
                "  Opus 5 (1M context) │ ◖ low",
                "  ⏵⏵ auto mode on (shift+tab to cycle)",
            ];
            // The blank row above the spinner is left; the body trims it.
            assert_eq!(chrome().cut(&screen), &screen[..2], "{spinner}");
        }

        // A finished turn's summary line is output, and stays.
        let screen = [
            "what the agent said",
            "",
            "✻ Cogitated for 1m 5s · done 9:33 AM",
            "",
            "────────────────────────────────",
            "❯\u{a0}",
            "────────────────────────────────",
            "  Opus 5 (1M context) │ ◖ low",
            "  ⏵⏵ auto mode on (shift+tab to cycle)",
        ];
        assert_eq!(chrome().cut(&screen), &screen[..4]);
    }

    /// The text of a card body's first `rows` rows.
    fn said(card: Card, rows: usize) -> Vec<String> {
        body(&card.read(), rows, 0, None, &[], theme())
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn view_tail_keeps_the_capture_the_card_has_no_question_to_draw() {
        let asked = |question: Option<&str>| {
            let mut card = asking(&[], Some(Kind::Question));
            card.body = format!("{SAID}\n\nWhich features should be enabled?\n");
            card.question = question.map(str::to_string);
            card
        };

        // No recorded question: the pane is the only place it appears.
        let kept = said(asked(None), 24);
        assert!(
            kept.contains(&"Which features should be enabled?".to_string()),
            "{kept:?}"
        );

        // With the question recorded, no pane.
        let block = said(asked(Some("Which features should be enabled?")), 24);
        assert!(block.is_empty(), "{block:?}");
    }

    #[test]
    fn pr_the_card_lists_every_request_the_branch_has() {
        let mut card = asking(&[], None);
        card.id = "busy-b2c".to_string();
        card.phase = Phase::Working;
        card.question = None;
        let screen = over_the_forge(
            vec![on_a_branch(
                view("busy-b2c", Phase::Working, Some("Running Bash"), 3),
                "amx/busy-b2c",
            )],
            Some(card),
        );
        let size = (60, 14);
        let lines = painted(&screen, size);

        let row = lines
            .iter()
            .position(|line| line.contains("#40 open"))
            .unwrap_or_else(|| panic!("nothing on the card lists them: {lines:?}"));
        assert!(
            lines[row].contains("#7 merged"),
            "every request the branch has, each with the question its colour \
             came from: {:?}",
            lines[row]
        );
        assert!(
            lines[row].starts_with("#40"),
            "on the card rather than on the row above it: {lines:?}"
        );
        assert_eq!(word_colour(&screen, size, row as u16, "#7"), theme().done);
    }

    #[test]
    fn view_tail_says_so_when_a_capture_is_nothing_but_chrome() {
        let captured = |text: String| {
            let mut card = asking(&[], None);
            card.phase = Phase::Working;
            card.body = text;
            card
        };
        assert_eq!(said(captured(CHROME.join("\n")), 8), [ALL_CHROME]);

        // An empty capture is not the same case.
        assert!(said(captured(String::new()), 8).is_empty());
    }

    #[test]
    fn view_tail_is_cut_before_the_card_measures_what_it_has() {
        let mut card = asking(&[], None);
        card.phase = Phase::Working;
        card.question = None;
        let mut screen = vec!["what the agent said"];
        screen.extend_from_slice(&CHROME);
        card.body = screen.join("\n");

        // One body row after the cut, plus the rule; not the six captured.
        assert_eq!(card_rows(&card.read(), None, &[], None, 60), 2);
    }

    #[test]
    fn view_card_counts_a_row_for_each_message_not_yet_taken_up_to_three() {
        let with = |queued: Vec<String>| {
            let mut card = asking(&[], None);
            card.phase = Phase::Working;
            card.question = None;
            card.body = "what the agent said".to_string();
            card.queued = queued;
            card.read()
        };
        // The rule, one body row, and one per message.
        let two = with(vec![
            "and the linter".to_string(),
            "then the docs".to_string(),
        ]);
        assert_eq!(card_rows(&two, None, &[], None, 60), 4);

        let five = with((1..=5).map(|n| format!("message {n}")).collect());
        assert_eq!(card_rows(&five, None, &[], None, 60), 5);
        // The newest three.
        let rows: Vec<String> = queued(&five, 60, Theme::default())
            .iter()
            .map(|line| line.to_string())
            .collect();
        assert_eq!(
            rows,
            [
                "❯ message 3 · queued",
                "❯ message 4 · queued",
                "❯ message 5 · queued"
            ]
        );
    }
}
