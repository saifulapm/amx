//! A closer look at one agent, as the band at the foot of the list.
//!
//! Not a box, and not a thing hung among the rows. It is drawn the way the
//! band a line is typed in is drawn, because it stands where that band stands:
//! a rule, and rows under it. The rule carries what the card is a look at —
//! the agent's own name, in the colour its row says its state in — and the
//! rows stand two cells in, under the chevron the card's line begins with.
//!
//! At the foot rather than under the row it came off, because the list is what
//! somebody with a card open is walking: a card among the rows moves every row
//! below it down, and walking the cursor with one open shakes the wall it is
//! being read against. It covers the last rows of the list rather than taking
//! them, and the wall is laid out as if no card were up, so opening one,
//! closing it or walking the cursor with it up moves nothing. A cursor row the
//! card is standing over is said by the card's own rule.
//!
//! How tall it is is worked out here as well, because that is an answer about
//! the list above: never so much of the band that the wall it was opened from
//! is gone.
//!
//! A card carries its body in one of two states. It is *built* from text — a
//! pane capture, a recorded answer, a patch — and it is *drawn* from [`Body`],
//! that text already walked out of its escapes. Everything the paint takes is
//! the second: the walk happens once, where the card is made, and no frame
//! pays for it again.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ops::Range;

use super::input::{COMPOSER_CAP, behind, rows_of, typed_rows};
use super::prose;
use super::style::{bold, colour, dim, request_colour};
use super::text::{RULE, SEPARATOR, fit, inert, width_of};
use super::wall::icon;
use crate::ansi::{self, Colour, Painted};
use crate::conversation::Said;
use crate::derive::{Evidence, View};
use crate::furniture::{Furniture, cut};
use crate::pr::Pr;
use crate::store::{Ask, Kind, Phase};
use crate::theme::Theme;
use crate::tui::act::{self, Composer};
use crate::tui::rows::Showing;
use crate::verbs::send::numbered;

/// A closer look at one agent, as the band at the foot of the list.
///
/// A card carries its body in one of two states, which is what `B` says. A
/// card is *built* from text — a pane capture, a recorded answer, a patch —
/// and it is *drawn* from [`Body`], that text already walked out of its
/// escapes. Everything the paint takes is the second: the walk happens once,
/// where the card is made, and no frame pays for it again.
pub struct Card<B = String> {
    pub id: String,
    pub phase: Phase,
    /// What it is waiting to be told, when it is waiting to be told anything.
    pub question: Option<String>,
    /// The choices that question offers, in the order the screen lists them.
    pub options: Vec<String>,
    /// Whether those choices are amx's own numbering of a list the vendor put
    /// no numbers on — see [`crate::store::State::walked`]. The numbers are
    /// still what the card offers, but a walk is what the verb sends to reach
    /// the one somebody names.
    pub walked: bool,
    /// What kind of question it is, which is what decides the answers it will
    /// take.
    pub kind: Option<Kind>,
    /// The screen it is sitting on, the answer it left behind, or what it has
    /// changed.
    pub body: B,
    /// Whether the body is that diff, which is read from the top down rather
    /// than from the bottom up.
    pub changes: bool,
    /// Whether the body is the agent's own words read forward — the answer
    /// the record holds, or the whole conversation of an agent whose turn is
    /// over — rather than a picture of a pane, or a conversation still being
    /// added to, both of which are read up from their bottom.
    pub answer: bool,
    /// Whether a line typed here would reach the agent at all: an agent in a
    /// pane takes what is typed, and one whose pane has gone takes it by being
    /// brought back on it. Only the agent that can be neither sent to nor
    /// started again is past listening.
    pub listening: bool,
}

impl<B> Card<B> {
    /// Whether this card is one somebody can answer. A patch is not a
    /// question, and neither is a look at an agent that is getting on with it.
    pub fn asks(&self) -> bool {
        !self.changes && self.phase == Phase::Waiting
    }

    /// Whether the body reads forward, from its top: a patch does, and so
    /// does a recorded answer. Only a live screen is read up from its
    /// bottom, where the newest of it is.
    pub fn forward(&self) -> bool {
        self.changes || self.answer
    }
}

impl Card<Body> {
    /// Where the card opens, as rows from its natural edge: a card read
    /// forward opens on its anchor — the end of a conversation — and
    /// everything else at its edge.
    ///
    /// That anchor is past the body's last row, and the paint clamps both it
    /// and the offset to the last page the card had room for; where it opened
    /// is where it is held from.
    pub fn opens_at(&self) -> usize {
        match self.forward() {
            true => self.body.anchor(),
            false => 0,
        }
    }
}

impl Card<String> {
    /// The same card with its body read, which is the form the paint draws.
    ///
    /// For a card built out of text somebody already holds, which is what a
    /// patch is. A card built from a record or a pane walks the words where it
    /// takes them, and never makes the copy this one is handed.
    pub fn read(self) -> Card<Body> {
        Card {
            // A patch is amx's own reading of a repository, not a pane; a
            // recorded answer and a finished agent's last words are whole,
            // with no vendor furniture under them; and what is left is a
            // picture of a pane somebody is still working in — one whose
            // vendor nothing here names, so the walk is handed the document
            // amx falls back to. The card the view opens on a live agent is
            // built where the record says whose pane it is.
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
        }
    }
}

/// A card's body, walked out of its escapes once — when the card was built.
///
/// The rows are ready to draw: neutralised, in the paint the vendor drew them
/// in, with amx's own text dimmed. A frame windows them and nothing else, so
/// an open card costs a redraw the same whether it is holding four rows of
/// answer or four thousand of patch.
pub struct Body {
    /// Every row of it, in order.
    rows: Vec<Line<'static>>,
    /// How many of them the card reads from its natural edge: the vendor's
    /// own furniture is off the end of a live capture, and the blank rows a
    /// pane is padded out with are off the end of everything.
    kept: usize,
    /// Whether the cut took furniture off. A pane holding nothing but the
    /// vendor's own chrome is a different fact from an agent that has said
    /// nothing yet, and the card says the first out loud.
    chrome: bool,
    /// The row a card read forward opens on: the end of a conversation, past
    /// its last row, and the top of everything else. Past the end because the
    /// paint owns the clamp — see [`Scroll::kept`] — and only the paint knows
    /// how many rows the card had to give.
    anchor: usize,
    /// The hunks of it, where it is a patch, in the order the patch writes
    /// them. Every other body has none: a pane and an answer are prose, and
    /// nothing in them is a change to a file.
    hunks: Vec<Hunk>,
}

/// One hunk of a patch, as the card holds it: where it stands on the card, and
/// what a comment on it would have to name.
///
/// Read off the same pass that makes the rows, because both are readings of one
/// text and a second walk could disagree with the first about where a hunk
/// begins.
///
/// The cursor steps by the row, and what it is standing on is what a comment
/// off the card names.
pub struct Hunk {
    /// The file it changes, on the new side — the old side where the file is
    /// being deleted and there is no new one.
    pub path: String,
    /// The line its header names on that side.
    pub line: usize,
    /// Which of the body's rows its `@@` header is.
    pub row: usize,
    /// Its rows, from that header to the last of them, as git wrote them.
    pub text: String,
}

/// The glyph a prompt wears in the conversation, which is the composer's own.
const PROMPT: &str = "❯ ";
/// And the one a tool call wears: a smaller mark of the same family, from a
/// block no font maps to an emoji. The hammer this used to be (U+2692) is in
/// the emoji set, and a terminal with a colour-emoji fallback drew it in
/// orange, two cells wide, over the space after it.
const TOOL: &str = "› ";
/// How much of the tail the card keeps: the last rows of what is streaming,
/// and few enough that a row of the record stays above it on the card — at
/// its tallest, and on a card half a small screen tall. A body is built before
/// the frame that draws it says how tall the card is, which is why this is a
/// number rather than a share of the card. Eight since 2026-09-06, when a tail
/// that was a whole chrome-cut pane pushed the record off the top of the
/// card's window and the card read as the pane it came off.
const TAIL: usize = 8;

impl Body {
    /// Nothing under everything else, which is what a card holding a question
    /// has.
    pub(in crate::tui) fn none() -> Body {
        Body {
            rows: Vec::new(),
            kept: 0,
            chrome: false,
            anchor: 0,
            hunks: Vec::new(),
        }
    }

    /// The whole conversation, drawn the way the agent meant it, with what
    /// the agent is saying now under it where a turn is still running.
    ///
    /// What it is saying now is what its vendor streams, and nothing else. A
    /// vendor that streams nothing has a card that is the record alone until
    /// its next message lands — its calls as they are issued, its answers as
    /// each message ends — with the row over the card saying what it is doing
    /// meanwhile. The pane used to stand under the record where nothing
    /// streamed, cut of its furniture, and was a second copy of the same turn
    /// in the vendor's dress: boxes with rows of nothing between them, a
    /// banner, a spinner line, a hint about a key. Saiful took it off on
    /// 2026-09-11, and a pane is read for a card only where there is no
    /// record to draw — see [`Body::screen`].
    ///
    /// A prompt stands behind the composer's own glyph, an answer is its
    /// markdown drawn into rows, and a tool call is one row: the tool at the
    /// terminal's own weight and the argument worth a row dim behind it. A
    /// blank row stands between one thing said and the next, except between
    /// one call and the call after it: a run of calls is one block, read as
    /// a column of names. Every row is wrapped to `width` here, because a
    /// card windows its rows and does not reflow them.
    ///
    /// The anchor is the end of it. A card read forward opens on the last
    /// rows of the last answer, where the conclusion of it is, with the rest
    /// of the turn and every turn before it a page up: an answer of any
    /// length runs off the bottom of a card, and its first rows are the ones
    /// a reader can guess.
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

        let blank =
            |row: &Line<'static>| row.spans.iter().all(|span| span.content.trim().is_empty());
        if let Some(live) = live {
            let mut tail = prose::render(live, width, theme);
            // The end of it, where what is landing is — see [`TAIL`].
            while tail.last().is_some_and(&blank) {
                tail.pop();
            }
            let skipped = tail.len().saturating_sub(TAIL);
            // The blank row that stands the tail off the record above it, only
            // where there are rows under it: a stream the vendor has opened
            // and said nothing into yet is no tail, and a blank row over it
            // would stand the record off nothing.
            let tail: Vec<Line<'static>> = tail.into_iter().skip(skipped).collect();
            if !tail.is_empty() {
                if !rows.is_empty() {
                    rows.push(Line::raw(String::new()));
                }
                rows.extend(tail);
            }
        }

        while rows.last().is_some_and(&blank) {
            rows.pop();
        }
        Body {
            kept: rows.len(),
            anchor: rows.len(),
            rows,
            chrome: false,
            hunks: Vec::new(),
        }
    }

    /// The row a card read forward opens on.
    pub(in crate::tui) fn anchor(&self) -> usize {
        self.anchor
    }

    /// A patch: amx's own reading of a repository rather than a pane, so there
    /// is no paint on it to keep and no furniture under it to cut.
    ///
    /// Read the way git writes it. The block of headers over each file — the
    /// `diff --git` row, the blob it was, the two sides, a mode, a rename, a
    /// binary row — is six or seven rows saying one thing, and on a card
    /// fourteen rows tall that is a file whose changes are off the bottom
    /// before they begin. All of it becomes one heading: what the file is
    /// called and what it gained and lost. What is under the heading is
    /// coloured by what it is, the way git colours a patch at a terminal, and
    /// kept whole: `+after` is a row that says `+after`, and a card that
    /// dropped the mark would have it read as the line that was already there.
    ///
    /// The hunks come off the same pass, because both are readings of one text
    /// and a second walk could disagree with the first about where a hunk
    /// begins.
    pub(in crate::tui) fn patch(text: &str) -> Body {
        let mut rows: Vec<Line<'static>> = Vec::new();
        let mut hunks: Vec<Hunk> = Vec::new();
        // The headers of the file being opened, held until the first row
        // under them: the counts the heading carries are not known until that
        // file's hunks have been read, so the row goes down blank and is
        // written when the file closes.
        let mut headers: Vec<&str> = Vec::new();
        let mut file: Option<Reading> = None;
        // And the rows of the hunk being read, from its header down.
        let mut held: Vec<&str> = Vec::new();

        for row in text.lines() {
            if row.starts_with(FILE) {
                shut(&mut hunks, &mut held);
                // A file whose headers had nothing under them at all — a mode
                // change, a rename, a binary file — is its heading and no
                // more.
                file = open(&mut rows, &mut headers).or(file);
                close(&mut rows, file.take());
                headers.push(row);
                continue;
            }
            // The first row that is not one of those headers closes the block:
            // the heading stands where the whole of it stood.
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

    /// The hunks of the patch it is holding, in the order the patch writes
    /// them.
    pub(in crate::tui) fn hunks(&self) -> &[Hunk] {
        &self.hunks
    }

    /// A live pane, in the paint the vendor drew it in, with that vendor's own
    /// furniture cut off the bottom.
    ///
    /// Whose furniture is the caller's to say, because every anchor the walk
    /// steps on is one vendor's own: the anchors that find claude's composer
    /// are absent from a pi pane, and a walk given the wrong ones leaves the
    /// chrome where it is.
    pub(in crate::tui) fn screen(chrome: &Furniture, text: &str) -> Body {
        Body::walk(text, Some(chrome))
    }

    /// What an agent said: a recorded answer, or whatever an agent whose
    /// command has ended left behind. Nothing is cut off it — there is no
    /// pane under it to hold furniture.
    pub(in crate::tui) fn said(text: &str) -> Body {
        Body::walk(text, None)
    }

    /// The walk itself. The furniture is the vendor's whose pane this came
    /// off, and `None` is text that came off no pane at all — the only body
    /// the cut is not taken off.
    fn walk(text: &str, chrome: Option<&Furniture>) -> Body {
        #[cfg(test)]
        WALKS.with(|walks| walks.set(walks.get() + 1));
        // The escapes are walked into styling here and nowhere else, so
        // nothing downstream of this line is holding a control sequence.
        let read = ansi::painted(text);
        let said: Vec<String> = read.iter().map(|row| words(row)).collect();
        let plain: Vec<&str> = said.iter().map(String::as_str).collect();
        // What the vendor drew on, with its own furniture off the bottom.
        let drawn = match chrome {
            Some(chrome) => cut(chrome, &plain).len(),
            None => plain.len(),
        };
        // The blank rows a pane is padded out with go the same way, so what
        // is left ends on the last row anybody wrote on: the edge both ends
        // of the body are measured from.
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

    /// How many rows it has to give a card, which is what the last page is
    /// measured against. The one row the card says it found nothing but
    /// furniture on counts: it is a row, and a card of one row does not page.
    fn length(&self) -> usize {
        self.kept.max(usize::from(self.chrome))
    }

    /// What the body says, for the tests that ask a card what it is holding.
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

/// The row git opens a file with, which is where one file's part of a patch
/// begins.
const FILE: &str = "diff --git ";

/// And the row it opens a hunk with.
const HUNK: &str = "@@";

/// The side of a hunk that is not there at all, on a file being added or
/// deleted.
const NOWHERE: &str = "/dev/null";

/// A file of the patch as it is being read: which row its heading is, what it
/// is called, and what its hunks have added and taken away so far.
struct Reading {
    at: usize,
    path: String,
    added: usize,
    removed: usize,
}

/// Put a heading where the block of headers stood, blank for now: what the
/// file gained and lost is not known until its hunks have been read.
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

/// And write it, now that the file is over: what it is called and what it
/// gained and lost.
fn close(rows: &mut [Line<'static>], file: Option<Reading>) {
    let Some(file) = file else {
        return;
    };
    let said = format!("{}  +{} -{}", inert(&file.path), file.added, file.removed);
    rows[file.at] = Line::styled(said, Style::default());
}

/// Hand the hunk that was being read the rows it was given.
fn shut(hunks: &mut [Hunk], held: &mut Vec<&str>) {
    if held.is_empty() {
        return;
    }
    if let Some(hunk) = hunks.last_mut() {
        hunk.text = held.join("\n");
    }
    held.clear();
}

/// Whether a row is one of the headers git writes between `diff --git` and the
/// first hunk of a file.
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

/// What the file those headers open is called: the path on the new side, and
/// the old one's where there is no new side — a file being deleted is named by
/// what it was.
///
/// Off the two sides where they are there at all. A mode change, a rename with
/// nothing in it and a binary file have neither, and the row the block opens
/// with names both.
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

/// What a hunk's header says it starts at: the line on the new side, and the
/// old side's where the new side holds no rows at all — a hunk that only takes
/// rows away names no line of the file as it now stands.
fn starts(row: &str) -> usize {
    let mut sides = row.trim_start_matches('@').trim_start().split(' ');
    let old = counted(sides.next().unwrap_or_default());
    let new = counted(sides.next().unwrap_or_default());
    match new.1 {
        0 => old.0,
        _ => new.0,
    }
}

/// One side of that header, as the line it starts at and the rows it holds. A
/// side with no count holds one row, which is how git writes it.
fn counted(side: &str) -> (usize, usize) {
    let side = side.trim_start_matches(['-', '+']);
    let (start, rows) = side.split_once(',').unwrap_or((side, "1"));
    (start.parse().unwrap_or(0), rows.parse().unwrap_or(1))
}

/// Where the card's body stands against its natural edge — the bottom of a
/// screen or an answer, the top of a patch — and how far one page is.
///
/// The keys add and subtract; the paint owns the clamp, because only the
/// paint knows how many rows the body was given. Cells, so a draw that is
/// otherwise a pure reading of the view can write back what it kept: a body
/// that fits is pinned to its edge, and a press past the end lands on the
/// last page rather than beyond it.
#[derive(Default)]
pub struct Scroll {
    /// Rows between what the card shows and the body's natural edge.
    pub away: Cell<usize>,
    /// The rows the body had last frame, which is what one press moves by.
    pub page: Cell<usize>,
    /// Where the card opened, in the same rows: its edge for a conversation
    /// that opens on its end rather than at its top. A card standing anywhere
    /// else has been paged by hand, and holds.
    pub opened: Cell<usize>,
    /// Where the card was asked to open, in rows nobody has clamped: past the
    /// end of a conversation. Kept whole so that a window that shrinks under
    /// a card nobody has paged — the line at its foot growing a row — puts
    /// the card back on the page it opened on, which is the end, rather than
    /// leaving it standing a row short of the end it was opened to show.
    anchor: Cell<usize>,
    /// Which hunk of the patch the card is standing on, where somebody has
    /// stepped to one. A patch is read a hunk at a time, and this is the one
    /// the rule counts, the paint marks, and a message off the card is about.
    hunk: Cell<Option<usize>>,
    /// What has been written about the patch so far, a hunk at a time: the
    /// words the line was holding when the cursor stepped off each one, under
    /// that hunk's own index, and the words typed at the top of the patch
    /// under no hunk at all.
    ///
    /// Here rather than on the line because the line holds one hunk's words at
    /// a time and a review is about several: the line is what is being written
    /// now, and this is what has been written already. In patch order, because
    /// that is the order the message is built in — a review reads the way the
    /// diff does.
    remarks: RefCell<BTreeMap<Option<usize>, String>>,
}

impl Scroll {
    /// Open a card `away` rows from its natural edge, and remember that this
    /// is where it opened.
    ///
    /// The one door a card comes up through, which is why the hunk cursor is
    /// put back to none here: taking the diff again and walking the list onto
    /// another agent both open a card, and a card that was closed comes back
    /// this way too. A hunk of the patch before it is no place to open on.
    ///
    /// The remarks go with it, for the same reason and one more: they are a
    /// review of the patch this card was showing, and the next card through
    /// this door is showing something else.
    pub fn open_at(&self, away: usize) {
        self.away.set(away);
        self.opened.set(away);
        self.anchor.set(away);
        self.hunk.set(None);
        self.remarks.borrow_mut().clear();
    }

    /// Keep `words` as the remark on `at` — a hunk, or the top of the patch —
    /// or drop what was there when they are blank.
    ///
    /// Blank is dropped rather than kept as an empty string so that everything
    /// counting remarks can count entries: a line stepped off with nothing on
    /// it is not a note on that hunk, and neither is one cleared by hand.
    pub fn remark(&self, at: Option<usize>, words: &str) {
        match words.trim().is_empty() {
            true => self.remarks.borrow_mut().remove(&at),
            false => self.remarks.borrow_mut().insert(at, words.to_string()),
        };
    }

    /// What was written about `at`, which is empty where nothing was.
    pub fn remarked(&self, at: Option<usize>) -> String {
        self.remarks.borrow().get(&at).cloned().unwrap_or_default()
    }

    /// The hunks something has been written about, in patch order, which is
    /// what the rule counts, the body marks and the row under the line offers
    /// to send. The words at the top of the patch are on no hunk, and are no
    /// note.
    pub fn noted(&self) -> Vec<usize> {
        self.remarks.borrow().keys().copied().flatten().collect()
    }

    /// Everything written so far, the opening first and the hunks after it in
    /// the order the patch writes them, which is the order a review reads in.
    pub fn remarks(&self) -> Vec<(Option<usize>, String)> {
        self.remarks
            .borrow()
            .iter()
            .map(|(at, words)| (*at, words.clone()))
            .collect()
    }

    /// Step to the next hunk of the patch the card is holding, or the one
    /// before it, or the top of the patch above them all, and stand the card
    /// on that row.
    ///
    /// Before the first hunk is the top of the patch itself, where no hunk is
    /// under the cursor: a review opens there, and that is where its opening
    /// words are written. Forward from there is the first hunk; back from
    /// there stays, because there is nothing above the top. The last hunk
    /// holds the same way, because a key that came back round to the other end
    /// would read as a key that had lost its place.
    ///
    /// Opened rather than paged, so the frame that draws it puts the header
    /// row at the top of the window and every frame after it holds the card
    /// there — clamped, on a hunk near the end of a patch, to the last page
    /// the card has rows for. The offsets are moved here rather than through
    /// [`Scroll::open_at`]: stepping through a patch is reading the card that
    /// is open, not opening another one, and the remarks written so far are
    /// what the step is carrying.
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

    /// Which hunk the card is standing on, where somebody has stepped to one.
    pub fn at_hunk(&self) -> Option<usize> {
        self.hunk.get()
    }

    /// Whether somebody has paged the card away from where it opened.
    pub fn paged(&self) -> bool {
        self.away.get() != self.opened.get()
    }

    /// Clamp the offset to the last page this body and window allow, remember
    /// what a page is, and say where the card now stands.
    ///
    /// `window` is the rows the body has this frame, which is what one press
    /// moves by as well: the card spends the same rows on its rule and its
    /// line whether or not it has been paged, so the way out and the way home
    /// are the same distance.
    ///
    /// Where it opened is clamped to that same last page, because a card is
    /// opened past its end — a conversation is anchored on its end and no
    /// body knows how tall a card is. Left where it was asked for, it would
    /// never equal the offset again, [`Scroll::paged`] would read true on
    /// every frame, and a card nobody touched would hold still forever
    /// instead of following its agent back to work.
    ///
    /// A card nobody has paged is put back on the page it was asked to open
    /// on every frame, clamped afresh: the last page moves when the window
    /// does, and a card opened on its end stays on its end when the line
    /// under it grows a row rather than standing one row short of it. A card
    /// somebody has paged is clamped where it stands, and holds.
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

/// How much of the screen the card takes: what it has to show, up to about
/// half, and never so much that the list it was opened from is gone.
///
/// What it has to show comes into it because a card is over a wall somebody is
/// reading: an agent whose answer is one line does not need seven rows to say
/// it in, and every row the card does not take is a row of the list still on
/// the screen. Below one row there is no card at all.
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

/// How many rows the card would take to say everything it has: its own rule,
/// what its branch has open, which question of the call this is, what the
/// agent is asking, the choices under that, the row the vendor adds under
/// them, the line the answer goes on, and the screen it is all happening on.
///
/// The rule, the line and the blank row the line stands off the rest by are
/// rows of the card like any other, so a card that says one thing in one row
/// asks for four; and the line asks for as many rows as it has grown to, up to
/// the cap the task line grows to.
pub(super) fn card_rows(
    card: &Card<Body>,
    showing: Option<Showing>,
    prs: &[Pr],
    answering: Option<&Composer>,
    width: u16,
) -> u16 {
    let inner = body_width(width);
    let asked = card
        .question
        .as_deref()
        .map_or(0, |question| wrapped(question, inner).min(ASKED_TALL));
    let listed = choices(&card.options, inner as usize, boxed(showing)).len();
    // Counted no further than the card could ever grow: the body can be a
    // patch of thousands of rows, and this runs on every frame.
    let shown = length(card).min(CARD_TALL as usize);

    let rows = RULE_ROW
        + usize::from(!prs.is_empty())
        + asked as usize
        + usize::from(tab(showing).is_some())
        + listed
        + usize::from(added(card, showing).is_some())
        + answering.map_or(0, |line| line_rows(line, width) + GAP_ROW)
        + shown;
    rows.min(u16::MAX as usize) as u16
}

/// How many rows the line at the foot of the card takes: as many as it has
/// grown to, the way the task line grows, and no more than that line may.
fn line_rows(line: &Composer, width: u16) -> usize {
    rows_of(line, width).min(COMPOSER_CAP)
}

/// One row, which is the least a card is: the rule, which names the agent the
/// card is a look at and says how far a paged body has been read.
const CARD_SHORT: u16 = 1;

/// And the most of a screen it will take, however tall the terminal is.
const CARD_TALL: u16 = 14;

/// The card's own row, which it holds whatever it is a look at: the rule.
const RULE_ROW: usize = 1;

/// And the blank row its line stands off the rest of it by, so what the card
/// says and what somebody is saying back to it do not run together. The one
/// row the card gives up first when the band is short.
const GAP_ROW: usize = 1;

/// How far in everything the card says stands: the two cells the line under it
/// spends on its own chevron, so a row of the card and the words being typed
/// about it start in one column.
const INDENT: u16 = 2;

/// How wide a card's body is on a band this wide: everything the card says
/// stands in under the chevron.
pub fn body_width(band: u16) -> u16 {
    band.saturating_sub(INDENT)
}

/// What a card holding a patch says it is, which is the one thing the row it
/// came off cannot: the row says what the agent is doing, and this card is
/// not a look at that at all.
const CHANGED: &str = "what it has changed";

/// What the rule says a turn nothing has been heard from yet is doing. A turn
/// under way is doing something whether or not anything has said what, and the
/// far end of the rule going bare would read as an agent that had stopped.
const THINKING: &str = "thinking…";

/// What the rule says of a turn the vendor ended with shells still running:
/// the record's own count, in the record's own words.
fn shells_running(n: u32) -> String {
    match n {
        1 => "1 shell running".to_string(),
        n => format!("{n} shells running"),
    }
}

/// How many rows of a wrapped question the card gives before it stops: the
/// words of it a person needs to decide, with the pane underneath for the rest.
const ASKED_TALL: u16 = 3;

/// The card: its rule, what its branch has open, which question of the call
/// this is, what one agent is asking, the choices it offers, the row the vendor
/// adds under them, the screen it is all happening on — or, when that is what
/// was asked for, what it has changed — and the line at its foot.
///
/// Full width, because the bottom of it is a picture of a terminal and a
/// terminal cut down the middle is a picture of nothing. The rule and the line
/// stand in the band's own columns and everything between them is indented
/// under the chevron, so the card reads as one block rather than as rows of a
/// second list.
///
/// `called` is what the list calls the agent, which is what the rule says: the
/// card is no longer touching that row, so its name is the one thing it has to
/// carry for itself. `runs` is what that row runs, in the same case: already
/// spelled the way the rule says it, because the words are the wall's and the
/// card is only reading them out.
///
/// `on` is the reading the row was drawn from and `beat` the frame the wall is
/// pulsing on, for the mark at the front of the rule and the words at the end
/// of it: both are about the agent rather than about the card, so both are read
/// off the list the same way the row above them is.
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
    // The band is the last rows of the list, so the rows already drawn there
    // come off before anything of the card goes down: what the card says
    // covers the wall rather than showing through it.
    frame.render_widget(Clear, area);
    // A card is a modal the way a line being typed is: for as long as it is
    // up every letter is its line's, and the wall above says so the way it
    // does under a task line, by going quiet to its last cell.
    behind(frame, area.y);
    // The rule opens the band and the line closes it; what the card says
    // stands between them, in under the line's own chevron, with one blank
    // row between it and the line. The line takes the rows it has grown to
    // off the band before anything else — it is what somebody is typing into,
    // and it is nowhere else at all — and the blank row stands only where a
    // row of what the card says is still left under it. A band with room for
    // nothing but the rule draws the rule.
    let spare = area.height.saturating_sub(RULE_ROW as u16);
    // Never the last row of what the card says, though: a line that had
    // grown over the whole card would be a line nobody could see what they
    // were answering from. Only a band with one row under its rule gives that
    // row to the line, because there the line is what the row is for.
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
    let said = Rect {
        x: between.x + INDENT,
        width: between.width.saturating_sub(INDENT),
        ..between
    }
    .intersection(between);

    // What the card is for comes first and the pane takes what is left.
    let mut room = said.height;
    let mut take = |wanted: u16| {
        let taken = wanted.min(room);
        room -= taken;
        taken
    };
    // Every request this branch has, above everything the card says about the
    // turn: what happened to the work after the turn ended is the question
    // somebody opening a finished agent's card came with.
    let open = requests(prs, theme);
    let opened = take(u16::from(!open.is_empty()));
    // The question and the choices are the agent's own words, and the choices
    // are the keys a person is about to press: both go through `inert` before
    // anything draws them. ratatui would *delete* the invisible format
    // characters on its own, which is exactly the wrong treatment — deleting
    // a zero-width lets one choice wear another's spelling.
    let question = card.question.as_deref().map(inert);
    let options: Vec<String> = card.options.iter().map(|option| inert(option)).collect();
    let asked = take(
        question
            .as_deref()
            .map_or(0, |question| wrapped(question, said.width).min(ASKED_TALL)),
    );
    // Which question of the call this is comes before the choices, because it
    // decides what the choices mean: the tab behind this one asks something
    // else and offers somebody else's answers.
    let strip = tab(showing);
    let tabbed = take(u16::from(strip.is_some()));
    let choices = choices(&options, said.width as usize, boxed(showing));
    let listed = take(choices.len() as u16);
    let added = added(card, showing);
    let adding = take(u16::from(added.is_some()));

    // What is left is the body's window, which is what the offset is clamped
    // against and what one press moves by.
    let held = scroll.kept(length(card), room as usize);
    // And where the hunk cursor is standing, which the rule counts and the
    // body marks. A patch shorter than the one the cursor was stepped through
    // has no such hunk, and nothing is marked.
    let at = scroll.at_hunk().filter(|at| *at < card.body.hunks().len());
    // And which hunks a note has been written on, which the rule counts and
    // the body marks.
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

    let [requesting, tabbing, asking, listing, adds, screen] = Layout::vertical([
        Constraint::Length(opened),
        Constraint::Length(tabbed),
        Constraint::Length(asked),
        Constraint::Length(listed),
        Constraint::Length(adding),
        Constraint::Min(0),
    ])
    .areas(said);

    if opened > 0 {
        frame.render_widget(Paragraph::new(Line::from(open)), requesting);
    }
    if let Some(strip) = strip.filter(|_| tabbed > 0) {
        frame.render_widget(Paragraph::new(Line::styled(strip, dim())), tabbing);
    }
    if let Some(question) = question {
        frame.render_widget(
            Paragraph::new(question)
                .wrap(Wrap { trim: true })
                .style(Style::new().fg(theme.waiting)),
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
}

/// The card's rule: the edge the band hangs off, and the things said on it.
///
/// At its front, the mark the agent's own row wears and what the list calls the
/// agent, in the colour that row says its state in — the card stands away from
/// its row now, so the mark and the name are what say which agent this is a
/// look at and how it is going. The mark pulses on the wall's own beat, so a
/// card over a turn under way breathes with the row it came off.
///
/// After the name, what that agent runs, in the words the wall's own column
/// says them in. After that, on a card that is a reading of a patch, that it is
/// one: the row says what the agent is doing, and this is not that, and which
/// hunk of it is under the cursor where somebody has stepped to one. And at the
/// far end, what the agent is doing while it is doing anything, and how far a
/// paged body stands from its natural edge. All of those are dim, because they
/// are facts about what the card is showing rather than about the agent.
///
/// The same rule the band a line is typed in draws, in the same character and
/// the same dim, because the card is that band with something else in it.
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
    // What the row this card was opened from is marked with. The card's own
    // phase, because that is what the name beside it is painted for; the rest
    // is the record's, and a list that has lost the row has neither — so the
    // mark falls back to the shape a state rests on.
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
    // After the hunk, because the hunk is where the next note is about to be
    // written: what is on the line, and then what is behind it.
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
    // How the turn is going, before the count, and never the row's summary:
    // the card under this rule is what the agent is doing, at length, and the
    // summary said again at the far end of the rule was a second row (Saiful,
    // 2026-09-18). The vendor's own spinner line where a reader found one on
    // the pane, the shells the vendor is holding the turn open for where it
    // is, and thinking where neither has anything to say.
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
    // One cell of the rule itself is kept back from both of the things that
    // give way, because the rule is what says this line is the card's edge and
    // words long enough to reach the last cell would take that away with the
    // dashes.
    let edge = width_of(&mark)
        + width_of(&named)
        + width_of(&changed)
        + 1
        + width_of(&more)
        + width_of(RULE);
    let doing = fit(&doing, width.saturating_sub(edge));
    // What the agent runs stands between its name and what the card is
    // showing, and takes the room the rest of the rule has left: a launch
    // command is a path as often as a word, and a rule that let one of those
    // crowd out what the card is showing would be saying the least useful
    // thing on it at the cost of the most. What the agent is doing is served
    // before it for the same reason — how a turn is going is what somebody
    // opened the card to read, and how it was started is the quietest fact on
    // the line.
    let runs = match runs.is_empty() {
        true => String::new(),
        false => fit(
            &format!("{SEPARATOR}{runs}"),
            width.saturating_sub(edge + width_of(&doing)),
        ),
    };
    // A cell of wall between the label and the rule, so the words are not
    // running into the dashes.
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

/// How many notes, in the words the rule over the card and the row under the
/// line both say them in: a review is counted the same wherever it is counted.
pub(super) fn notes(kept: usize) -> String {
    match kept {
        1 => "1 note".to_string(),
        kept => format!("{kept} notes"),
    }
}

/// Whether the body holds more than the card is showing, which is what makes
/// the page keys worth naming in the row under it.
///
/// Measured against what the last frame gave the body, which is the number one
/// press of those keys moves by: the row under the card is drawn after the
/// card itself, so within a frame this is what that frame left behind.
pub(super) fn pages(card: &Card<Body>, scroll: &Scroll) -> bool {
    length(card) > scroll.page.get()
}

/// How many rows the body could give a card, which is what the last page is
/// measured against. Asked of the body itself rather than of a window of it,
/// so measuring a patch of thousands of rows does not build them.
fn length(card: &Card<Body>) -> usize {
    match card.asks() && card.question.is_some() {
        true => 0,
        false => card.body.length(),
    }
}

/// Every pull request the agent's branch has, as the one row the card gives
/// them.
///
/// The row says the number in its own colour and then, in words, which of the
/// four questions that colour came from — a row has only the colour, and two
/// standings share one. All of them and not the first: a branch that has been
/// through this twice is a branch where the second attempt is the news and the
/// first is the reason there was a second.
///
/// Nothing here comes off a pane, so nothing here is neutralised: the numbers
/// are amx's own formatting of an integer, and the words are this file's.
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

/// What the card has under everything else, in the paint it was drawn in and
/// cut to the rows the card has for it.
///
/// A screen is read from the bottom, where the newest of it is; a diff from
/// the top, where the first file it touched is; and a recorded answer from
/// its top too, because an answer reads forward.
///
/// A card holding a question has nothing under everything else at all. The
/// question block — the tab strip, the question, the choices and the rows
/// under them — is the whole of what that card is for, and the pane beneath
/// it is the vendor's drawing of the same box behind an echo of the prompt:
/// every row of it is noise below the answer line. Only the waiting card
/// whose question amx has not read keeps its capture, because the pane is
/// the one place that question is written at all.
///
/// The vendor's own furniture came off the screen before it was ever counted,
/// in [`Body::screen`]. After would be worse than not at all: the card would
/// spend its window on the vendor's composer and then have nothing left for
/// the work.
///
/// `at` is the hunk the cursor is standing on, whose header row is drawn on
/// the cursor's own background — the same mark the list puts under the row a
/// person is on, because it is the same fact: this is where they are.
///
/// `notes` are the hunks something has been written about, whose header rows
/// are drawn in the colour a waiting agent's row wears: the review is held off
/// the card until enter sends it, and this is the only place paging the patch
/// shows where it has been.
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

    // A patch and a recorded answer both read forward, so both are windowed
    // from their top; a screen from its bottom, where the newest of it is.
    let window = match card.forward() {
        true => head(card.body.kept, rows, away),
        false => tail(card.body.kept, rows, away),
    };
    let start = window.start;
    let mut shown = card.body.rows[window].to_vec();

    // The header row of every hunk a note is on, where the window has it.
    // Before the cursor's own mark, so a hunk that is both wears both: the
    // colour says a note is kept there, the background says this is where the
    // reader is.
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

    // The header row of that hunk, where the window has it: a hunk stepped to
    // is at the top of the window, and one the clamp pulled up from the end of
    // a patch is further down it.
    if let Some(row) = at
        .and_then(|at| card.body.hunks.get(at))
        .and_then(|hunk| hunk.row.checked_sub(start))
        && let Some(line) = shown.get_mut(row)
    {
        line.style = line.style.bg(theme.cursor);
    }

    // Said only where the walk actually cut. An agent that has said nothing
    // yet is a different fact from a pane holding nothing but furniture, and
    // a card that answered both with the same sentence would be lying about
    // one of them.
    match shown.is_empty() && card.body.chrome {
        true => vec![Line::styled(ALL_CHROME, dim())],
        false => shown,
    }
}

#[cfg(test)]
thread_local! {
    /// How many bodies this thread has walked out of ANSI, which is the whole
    /// cost of a card: a pane capture is a few thousand bytes of escape
    /// sequences, and walking them is the one piece of work a card does that
    /// grows with what the agent wrote. Counted so a test can say where the
    /// walk happens and not only what it produces.
    ///
    /// Per thread, because the tests run side by side in one process and a
    /// count they shared would be a count none of them could assert on.
    static WALKS: Cell<usize> = const { Cell::new(0) };
}

/// How many walks this thread has paid for so far.
#[cfg(test)]
pub(in crate::tui) fn walks() -> usize {
    WALKS.with(Cell::get)
}

/// What a captured row says, which is what the cut reads it for. The runs of
/// one row joined, so the words and the paint can never disagree about where a
/// row begins or what is on it.
fn words(row: &[Painted]) -> String {
    row.iter().map(|run| run.text.as_str()).collect()
}

/// One captured row, drawn the way the vendor drew it.
fn as_painted(row: &[Painted]) -> Line<'static> {
    let spans: Vec<Span<'static>> = row
        .iter()
        .map(|run| Span::styled(inert(&run.text), paint(run)))
        .collect();
    Line::from(spans)
}

/// The paint one run was written in, as the renderer's own styling.
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

/// A colour the vendor named, as the renderer names it. The first sixteen are
/// named rather than numbered, so a person's own palette decides what red
/// looks like on their terminal, the way it does in the pane itself.
fn shade(colour: Colour) -> Color {
    match colour {
        Colour::Ansi(n) => ANSI[usize::from(n) & 0x0f],
        Colour::Indexed(n) => Color::Indexed(n),
        Colour::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// The sixteen SGR names them, in the order ANSI numbers them.
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

/// What the card says where the walk finds nothing underneath the chrome.
///
/// Whichever vendor drew it: the walk holds that agent's own anchors, so the
/// row this stands in for is the composer of whatever is running in the pane.
pub(super) const ALL_CHROME: &str = "amx captured nothing but the vendor's own chrome";

/// Which question of the call the card is showing, and how many there are.
///
/// The one thing on the card that is nowhere on the pane under it. Measured
/// against claude 2.1.240, the vendor's tab strip elides its own headers as
/// the pane narrows and at 24 columns draws the showing tab's name as an
/// ellipsis and nothing else, so no reader can count or name the tabs from a
/// screen. A call of one question is not a strip and says nothing here.
fn tab(showing: Option<Showing>) -> Option<String> {
    let showing = showing.filter(|showing| showing.of > 1)?;
    let counted = format!("{} of {}", showing.at, showing.of);
    Some(match showing.header() {
        Some(header) => format!("{header}{SEPARATOR}{counted}"),
        None => counted,
    })
}

/// Whether the choices are boxes to check rather than a choice to make.
fn boxed(showing: Option<Showing>) -> bool {
    showing.is_some_and(|showing| showing.ask.multi)
}

/// The vendor's own empty box, drawn between the number and the label the way
/// 2.1.240 draws it, so the row on the card reads as the row on the pane.
///
/// Empty, always. What amx holds is the payload, and the payload names the
/// choices and never says which of them are checked — the boxes themselves are
/// on the pane at the bottom of the card, where they are being checked.
const BOX: &str = "[ ]";

/// The row the vendor draws under the choices that no payload accounts for.
///
/// Every menu the tool draws carries one free-text row as its last choice, and
/// a question whose choices carry a preview draws a notes field in its place
/// and no free-text row at all — neither is in the payload, and both are what
/// somebody about to answer needs to know is there. A permission box and the
/// trust screen have neither, and choices amx has not read yet have nothing
/// for this to stand under.
fn added(card: &Card<Body>, showing: Option<Showing>) -> Option<&'static str> {
    if card.options.is_empty() || card.kind != Some(Kind::Question) {
        return None;
    }
    match showing.is_some_and(|showing| showing.ask.takes_notes()) {
        true => Some(NOTES),
        false => Some(OTHER),
    }
}

/// The free-text row, named as the vendor's rather than the agent's: the
/// payload does not carry it, so the pane below the card has a numbered row
/// the choices above it do not.
const OTHER: &str = "and under them, the vendor's row for words of your own";

/// And the field the vendor draws where a choice carries a preview, which is
/// the one layout that has no free-text row at all.
const NOTES: &str = "and beside them, the vendor's field for a note";

/// The choices under the question, numbered the way every surface numbers them
/// and packed onto as few rows as the card is wide.
///
/// From [`numbered`] like the rest of them, so the number a person presses on
/// the card is the number `amx answer` takes and the number `ls` printed. One
/// too wide for the card is cut with the ellipsis that says it was: a choice
/// nobody can read is still a choice they can press, and its number is at the
/// front where the cut cannot reach it.
///
/// `boxed` puts the vendor's box between the number and the label, on the
/// question that takes more than one choice. A number pressed there checks a
/// box and submits nothing, and a row that looked the same either way would be
/// a screen telling somebody they had answered.
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
        let room = width.saturating_sub(choice.chars().count() + BETWEEN.len());
        match rows.last_mut() {
            Some(row) if row.chars().count() <= room => {
                row.push_str(BETWEEN);
                row.push_str(&choice);
            }
            _ => rows.push(fit(&choice, width)),
        }
    }
    rows
}

/// What stands between two choices sitting on one row.
const BETWEEN: &str = "   ";

/// The line at the foot of the card, with the block on the cell the cursor is
/// standing in.
///
/// The composer's own line, drawn by the composer's own hand: the chevron,
/// what has been typed on as many rows as it has grown to, and the one cell
/// the cursor stands in turned over. Past the cap it is the rows around the
/// cursor that are drawn, exactly as under the task line.
///
/// Every card has one, because every agent can be said something to. Empty, it
/// says what this one will take, and the block stands on the first cell of
/// that, where what is typed will begin.
///
/// The chevron carries the waiting colour at a question and nothing but the
/// dim elsewhere: a prompt in front of somebody is the one thing on this
/// screen that is waiting on them, and a line they may type at if they feel
/// like it is not.
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

/// What the empty line says it will take.
///
/// At a question, what that question will take — which is the one thing
/// somebody looking at a prompt they did not draw cannot work out for
/// themselves, and it is said from the same place the refusal is written. On
/// an agent something will come of it on, the word for what the line is:
/// whatever is typed there reaches the agent, by being sent to its pane or by
/// bringing it back on those words. And on one nothing will come of it on,
/// that — in the words [`act::reply`] refuses it in, because a
/// line that invited a reply nobody would receive would be the card telling
/// somebody to type into the dark.
fn invites(card: &Card<Body>, asked: Option<&Ask>) -> String {
    match (card.asks(), card.listening) {
        (true, _) => act::invitation(card.kind, &card.options, asked, card.walked),
        (_, false) => NOBODY.to_string(),
        _ if card.phase.is_terminal() => RESUME.to_string(),
        _ => REPLY.to_string(),
    }
}

/// What the line says on an agent that will do something with it, which is
/// what it is.
const REPLY: &str = "reply";

/// And on one whose turn is over: a line typed here starts the agent again
/// on those words, which is a vendor brought back and a turn paid for, so the
/// line says so before the keystroke rather than after it (Saiful,
/// 2026-09-15, off a stopped agent woken by a line typed at its card).
const RESUME: &str = "resume";

/// And on one past listening, which is the whole of what would come of it.
const NOBODY: &str = "nothing is listening";

/// How many rows text takes when it is wrapped to a width.
fn wrapped(text: &str, width: u16) -> u16 {
    let width = width.max(1) as usize;
    let rows = text.chars().count().div_ceil(width);
    rows.clamp(1, u16::MAX as usize) as u16
}

/// Which rows of a screen the card shows: the last of the `end` rows the body
/// kept, which is where the newest of a pane is.
///
/// A window rather than the rows themselves, because a body carries the words
/// its rows say and the paint they say them in, and a reading that cut one
/// without the other would have them disagree.
pub(super) fn tail(end: usize, wanted: usize, back: usize) -> Range<usize> {
    // A paged card stands that many rows above the bottom it is read from.
    let end = end.saturating_sub(back);
    end.saturating_sub(wanted)..end
}

/// And which rows of a patch or a recorded answer: the first of them, because
/// both read forward from their top. A paged card starts that many rows below
/// it.
fn head(end: usize, wanted: usize, away: usize) -> Range<usize> {
    let start = away.min(end);
    start..end.min(start.saturating_add(wanted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::{Evidence, Verdict, View};
    use crate::pr::Standing;
    use crate::store::{Meta, State};
    use crate::tmux::{PaneId, Socket};
    use crate::tui::act::Asking;
    use crate::tui::paint::draw;
    use crate::tui::paint::header::space_rows;
    use crate::tui::paint::wall::{LIVE, pulse, set};
    use crate::tui::rows::FOLD_AT;
    use crate::tui::{Mode, Screen};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
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
                role: None,
                parent: None,
                depth: 0,
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                agent: Some("claude".to_string()),
                model: None,
                effort: None,
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
            doing: None,
        }
    }

    /// The same row run by a shell command rather than a vendor: the record a
    /// `!cmd` or an `--exec` spawn writes, which is one with no agent on it.
    fn command(id: &str, phase: Phase) -> View {
        let mut view = view(id, phase, Some("cargo build"), 5);
        view.meta.agent = None;
        view
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
            walked: false,
            kind,
            body: "$ cargo test\nDo you want to proceed?".to_string(),
            changes: false,
            answer: false,
            listening: true,
        }
    }

    /// The same reading, on a branch of its own.
    fn on_a_branch(mut view: View, branch: &str) -> View {
        view.meta.branch = Some(branch.to_string());
        view
    }

    /// A forge holding one failing request for the agent that is asking, and
    /// two for the one beside it — the second attempt and the first.
    fn a_forge(meta: &crate::store::Meta) -> Vec<Pr> {
        match meta.branch.as_deref() {
            Some("amx/ask-a1b") => vec![Pr {
                number: 12,
                standing: Standing::Failing,
            }],
            Some("amx/busy-b2c") => vec![
                Pr {
                    number: 40,
                    standing: Standing::Open,
                },
                Pr {
                    number: 7,
                    standing: Standing::Merged,
                },
            ],
            _ => Vec::new(),
        }
    }

    /// The view over that forge.
    fn over_the_forge(views: Vec<View>, card: Option<Card>) -> Screen {
        let mut screen = Screen::default();
        screen.list.asking(a_forge);
        screen.list.show(views);
        screen.card = card.map(Card::read);
        screen
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

    /// What the view puts on a screen of this size, line by line.
    fn drawn(views: Vec<View>, card: Option<Card>, size: (u16, u16)) -> Vec<String> {
        painted(&showing(views, card), size)
    }

    /// The card as it stands on the screen, top to bottom: the band at the
    /// foot of the list, which opens on its rule and runs to the keys.
    ///
    /// The keys are never the card's, and on a screen tall enough for it
    /// neither is the blank row standing them off what is above them.
    fn card_lines(screen: &[String]) -> Vec<&str> {
        let Some(top) = screen.iter().position(|line| line.contains(RULE)) else {
            return Vec::new();
        };
        let foot = screen.len() - 1 - space_rows(screen.len() as u16) as usize;
        screen[top..foot].iter().map(String::as_str).collect()
    }

    /// What a heading line says: the group's own words, the count where the
    /// group is shut, and how many failed under it where any did.
    fn heading_of(line: &str) -> &str {
        line.trim()
    }

    /// Which cell of this row the block is standing in: the one drawn in
    /// reverse video, which is where the next character somebody types will
    /// land and the only thing on the screen that says so.
    fn block(screen: &Screen, size: (u16, u16), row: u16) -> Option<u16> {
        let cells = cells(screen, size);
        (0..size.0).find(|column| cells[(*column, row)].modifier.contains(Modifier::REVERSED))
    }

    /// Which column of a drawn line a word starts in, counted in cells rather
    /// than bytes: the glyph a row wears is not one byte.
    fn column_of(line: &str, word: &str) -> usize {
        let at = line
            .find(word)
            .unwrap_or_else(|| panic!("{word:?} is not on {line:?}"));
        line[..at].chars().count()
    }

    /// The colour a word on a row was painted in.
    fn word_colour(screen: &Screen, size: (u16, u16), row: u16, word: &str) -> Color {
        let buffer = cells(screen, size);
        let line: String = (0..size.0)
            .map(|column| buffer[(column, row)].symbol())
            .collect();
        buffer[(column_of(&line, word) as u16, row)].fg
    }

    /// The two agents a card is opened over, so there is a list to still be
    /// drawn behind it.
    fn a_fleet() -> Vec<View> {
        vec![
            view("ask-a1b", Phase::Waiting, None, 29),
            view("busy-b2c", Phase::Working, Some("Running Bash"), 3),
        ]
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

        // The glyph wears the accent; on a call the glyph and the argument
        // are dim and the tool's name is not; the words are not.
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

        // Nothing said is no rows and no anchor.
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

        // A row is one row: the argument is cut to what is left beside the
        // name, and a name that fills the row leaves it no room at all.
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
        // And nothing where the vendor streams nothing: the record is the
        // whole of the card, with no pane under it.
        let quiet = Body::conversation(&told, None, 30, theme());
        assert_eq!(quiet.says(), "❯ port it\n\n› Bash cargo test\n\non it");
        assert_eq!(quiet.anchor(), quiet.kept, "and reads up from its end");
    }

    #[test]
    fn card_keeps_the_last_rows_of_a_long_live_tail() {
        let told = a_talk("port it", "on it");
        // Everything under the record's last row and the blank row that stands
        // the tail off it.
        let after_the_record = |body: &Body| -> Vec<String> {
            let said = body.says();
            let (_, tail) = said
                .split_once("on it\n\n")
                .expect("a tail under the record");
            tail.lines().map(str::to_string).collect()
        };

        // A stream longer than the card: the last rows of it.
        let streamed = (1..=20)
            .map(|n| format!("{n}. reason {n}\n"))
            .collect::<String>();
        let long = Body::conversation(&told, Some(&streamed), 30, theme());
        let tail = after_the_record(&long);
        assert_eq!(tail.len(), TAIL, "{tail:?}");
        assert!(tail[TAIL - 1].ends_with("reason 20"), "{tail:?}");
        assert!(tail[0].ends_with("reason 13"), "{tail:?}");

        // A short one is whole.
        let short = Body::conversation(&told, Some("one\n\ntwo"), 30, theme());
        assert_eq!(after_the_record(&short), ["one", "", "two"]);
    }

    #[test]
    fn card_stands_no_blank_row_over_a_stream_with_nothing_in_it() {
        // The seconds between a turn starting and its first word landing: a
        // stream the vendor has opened and written nothing to, or nothing but
        // blank rows. The record is the whole of what the card has, so a blank
        // row over the tail would be a row spent standing the record off
        // nothing.
        let told = a_talk("port it", "on it");
        let said = "❯ port it\n\n› Bash cargo test\n\non it";
        for streamed in ["", "\n\n\n"] {
            let body = Body::conversation(&told, Some(streamed), 30, theme());
            assert_eq!(body.says(), said, "{streamed:?}");
            assert_eq!(body.kept, 5);
        }

        // One row streamed is a tail, and stands off the record.
        let landing = Body::conversation(&told, Some("reading the importer\n"), 30, theme());
        assert_eq!(
            landing.says(),
            format!("{said}\n\nreading the importer"),
            "one blank row between the record and the tail, and nothing else"
        );
    }

    /// A patch as git writes one: a file changed and a file deleted, with the
    /// whole block of headers over each of them.
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

        // The colours are git's own at a terminal, and the heading is in no
        // colour at all: it is amx's own row, and the one row of a file a
        // reader walking the patch is looking for.
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

        // A body that is not a patch has no hunks to step through.
        assert!(Body::said("nothing to review here").hunks().is_empty());
    }

    #[test]
    fn card_steps_the_hunk_cursor_through_the_patch_and_back() {
        let body = Body::patch(A_PATCH);
        let scroll = Scroll::default();
        assert_eq!(scroll.at_hunk(), None, "a card opens on no hunk at all");

        // The first hunk from none, with the card opened on its header row:
        // opened rather than paged, so the frame that draws it puts it back
        // there rather than holding it wherever it stood.
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

        // Before the first hunk is the top of the patch, which is a place of
        // its own: no hunk under the cursor, and the window on the first row.
        scroll.to_hunk(body.hunks(), false);
        assert_eq!(scroll.at_hunk(), None, "the top of the patch itself");
        assert_eq!(scroll.away.get(), 0);
        assert!(!scroll.paged(), "which is where the card now opens");
        scroll.to_hunk(body.hunks(), false);
        assert_eq!(scroll.at_hunk(), None, "and the top stays");
        assert_eq!(scroll.away.get(), 0);

        // Whatever puts a card where it opens puts the cursor back to none:
        // taking the diff again, and the cursor landing on another agent.
        scroll.to_hunk(body.hunks(), true);
        scroll.open_at(0);
        assert_eq!(scroll.at_hunk(), None);

        // A card that is not a patch has no hunk to step to.
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

        // A line with nothing on it keeps nothing: stepping off a hunk nobody
        // wrote on is the common way through here, and clearing what was
        // written on one is how a remark is taken back.
        scroll.remark(Some(0), " \n ");
        assert_eq!(scroll.remarked(Some(0)), "");
        assert_eq!(scroll.remarks().len(), 2);

        // Stepping through the patch leaves every one of them where it is:
        // that is the whole of what they are for.
        scroll.to_hunk(body.hunks(), true);
        scroll.to_hunk(body.hunks(), false);
        assert_eq!(scroll.remarks().len(), 2, "the step kept them");

        // And whatever opens a card drops them along with the cursor, because
        // a review is about the patch the card was showing.
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
            }),
        );
        let size = (60, 24);

        // Nothing is counted until somebody steps to a hunk.
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
        };
        let screen = showing(
            vec![view("fix-login-a1b", Phase::Working, None, 3)],
            Some(patch()),
        );
        // Wide enough for the whole rule: the name, what the agent runs, and
        // the count after the hunk, which is what is being read here.
        let size = (76, 24);

        // The words a review opens with are not a note on anything, so the
        // rule has nothing to count yet.
        screen.scroll.remark(None, "the whole of it reads well");
        let all = painted(&screen, size).join("\n");
        assert!(!all.contains("note"), "the opening is no note: {all}");

        screen.scroll.remark(Some(1), "this file can go");
        let all = painted(&screen, size).join("\n");
        assert!(
            all.contains("what it has changed · 1 note"),
            "one note kept: {all}"
        );

        // And the count stands after the hunk the cursor is on, because the
        // hunk is where the next one is about to be written.
        screen.scroll.remark(Some(0), "the name reads backwards");
        let hunks = screen.card.as_ref().expect("the card").body.hunks();
        screen.scroll.to_hunk(hunks, true);
        let all = painted(&screen, size).join("\n");
        assert!(
            all.contains("· hunk 1 of 2 · 2 notes"),
            "both of them, after the hunk: {all}"
        );

        // Every hunk a note is on says so on its header row, so paging the
        // patch shows where the review has been. The hunk under the cursor
        // wears the cursor's own background as well.
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

        // A hunk nobody has written on keeps git's own colour.
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
        };
        // Wide enough for the whole rule: the mark, the name, what the agent
        // runs, what the card is showing, and what the agent is doing at the
        // far end, which is the one of them that gives way last.
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

        // Only the parts the record holds: a dial nobody turned is the
        // vendor's own and amx never saw it.
        let plain = view("fix-login-a1b", Phase::Working, None, 3);
        let (_, rule) = ruled(&showing(vec![plain], Some(patch())));
        assert!(
            rule.starts_with(&format!(
                "{} fix-login-a1b · claude · what it has changed",
                pulse(0)
            )),
            "{rule:?}"
        );

        // And a row no vendor runs says what it is instead, in the words its
        // column on the wall says them in.
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
            };
            let mut screen = showing(vec![agent], Some(card));
            // The frame the pulse is largest at, so the glyph on the rule is
            // the one a reader would see on the row beside it.
            screen.beat = LIVE;
            painted(&screen, size)
                .into_iter()
                .find(|line| line.contains(RULE))
                .expect("the card's rule")
        };

        // A turn under way: the row's own mark in front of the name, and what
        // the wall says the agent is doing at the far end of the rule.
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

        // The vendor's own line about the turn, where a reader found one
        // spinning on the pane, whether or not the record names a tool.
        let mut spinning = view("fix-login-a1b", Phase::Working, Some("Running Bash"), 3);
        spinning.doing = Some("Nesting… (15s · ↓ 1.3k tokens)".to_string());
        let spinning = looked(spinning);
        assert!(
            spinning.ends_with("Nesting… (15s · ↓ 1.3k tokens)"),
            "the spinner line whole, at the far end: {spinning:?}"
        );

        // A turn the vendor ended with shells still running says so, since
        // that is the one thing about it the card cannot show.
        let mut held = view("fix-login-a1b", Phase::Working, Some("2 shells running"), 3);
        held.state.background = 2;
        let held = looked(held);
        assert!(
            held.ends_with("2 shells running"),
            "the shells the turn is held open for: {held:?}"
        );

        // A turn that has said nothing yet is still a turn, and the rule says
        // so rather than leaving the far end of it bare.
        let quiet = looked(view("fix-login-a1b", Phase::Working, None, 3));
        assert!(
            quiet.ends_with("thinking…"),
            "a working agent with nothing to say is thinking: {quiet:?}"
        );

        // And a turn that is over has the ended mark and nothing at that end:
        // what it said is on the card, and the rule is not a second row.
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
        // A launch command is a path as often as a word, and a path can be
        // longer than the rule. The rule is what says the line is the card's
        // edge, so the path gives way before the last dash does.
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
        // A card is a modal the way a line being typed is, and the wall says
        // so the same way under both: everything above the rule goes dim to
        // its last cell, the row under the cursor with the rest, and what is
        // left undimmed is the card.
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
        // What the card says and what somebody is saying back to it do not
        // run together: one blank row stands between the card's last row and
        // the chevron, and it is the first row to go where the band is short.
        let question = || asking(&["the sqlite one", "the docker one"], Some(Kind::Question));
        let roomy = painted(&answering(question(), ""), (60, 20));
        let line = roomy
            .iter()
            .position(|row| row.starts_with('❯'))
            .expect("the line");
        assert_eq!(roomy[line - 1], "", "a blank row over the line: {roomy:?}");
        assert!(
            roomy[line - 2].contains("vendor's row"),
            "and the card's last row over that: {roomy:?}"
        );

        // On a band with room for the rule, one row of the card and the line,
        // the gap is what goes.
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
        // A newline in the line is a row of the card's line, drawn the way
        // the task line draws it: the chevron on the first row, the indent
        // under it on the next, and the block on the cell the cursor stands
        // in, which is the end of the second row.
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

        // A line taller than the card has room for takes rows off what the
        // card says, up to the task line's own cap, and never the rule or the
        // card's last row.
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
        // The words the line under the cursor could be stand in a band under
        // the card and over the keys, exactly where the task line's stand:
        // the card's line offers the same words, and the band is the same
        // band.
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
            asked.starts_with("  Which fixture should the port keep?"),
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
        // The wall is what somebody with a card open is walking, so opening
        // one leaves every row of it where it stood: the card takes its rows
        // off the foot of the screen rather than out of the middle of the list.
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

        // On a screen with nearly no room the card is cut to what half of it
        // allows, because the rows it would take next are the last rows of the
        // list.
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

    /// The view drawn at a size a test picks: what a person sees there.
    fn settled(views: Vec<View>, card: Option<Card>, size: (u16, u16)) -> Vec<String> {
        painted(&showing(views, card), size)
    }

    /// The card a finished row opens: the answer it left behind, long enough
    /// that the card asks for every row it is allowed.
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
        }
    }

    /// Six waiting agents and six finished ones, which with a heading over
    /// each group and the blank row between them is fifteen rows of list —
    /// exactly the band a twenty-row screen has for it.
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
        // Fifteen rows on a twenty-row screen: the header takes two, the rows
        // of air at either end of the list one each, and the keys the last.
        // The cursor is on the last of the fifteen, which is the case the card
        // used to scroll the wall for.
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

        // Walking the list with the card open moves no row either: the wall is
        // laid out as if no card were up, so the offset cannot change under it.
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
        // The same fifteen rows with the cursor on the last of them, read the
        // way the mouse reads them: a point above the card names the row it
        // named with no card up, and a point on the card names nothing at all.
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
        // Ten finished agents more than the fold holds, so the completed
        // group folds behind a count. The card covers the foot of the band
        // rather than taking it, so the wall above the card is laid out
        // exactly as it was with no card up — the fold is not cut again for
        // the rows the card left, and no row that stood above the card's rule
        // is a different row afterwards.
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
    fn card_stands_its_rows_in_under_the_chevron_its_line_begins_with() {
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
        // Bar the blank row the line stands off the rest by, which says
        // nothing and stands nowhere.
        for row in said.iter().filter(|row| !row.is_empty()) {
            assert!(
                row.starts_with("  ") && !row.starts_with("   "),
                "and what the card says stands two cells in, under that \
                 chevron: {row:?}"
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

    /// The same card, with somebody part way through typing the answer to it.
    fn answering(card: Card, typed: &str) -> Screen {
        let mut screen = showing(a_fleet(), Some(card));
        let mut composer = Composer::new(Asking::Reply);
        composer.text = typed.to_string();
        // Where somebody typing it would have left the cursor, which is what
        // the block on the line stands on.
        composer.at = composer.text.chars().count();
        screen.mode = Mode::Typing(composer);
        screen
    }

    /// The row of the card the answer is typed on.
    fn answer_row(screen: &[String]) -> String {
        screen
            .iter()
            .find(|line| line.contains('❯'))
            .unwrap_or_else(|| panic!("no row to answer on in: {screen:?}"))
            .clone()
    }

    /// Which row of the screen the card's line is standing on.
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

        // Where the cursor has been walked back into what was typed, the block
        // is on the cell it is standing in: that is where the next character
        // lands, and the end of the line is not.
        let mut walked = answering(question(), "the docker one");
        if let Mode::Typing(composer) = &mut walked.mode {
            composer.at = 4;
        }
        assert_eq!(block(&walked, size, line_row(&typed)), Some(6));
    }

    /// The weight the chevron on the card's line was drawn at, which is how
    /// the dim is told from the colour.
    fn chevron(screen: &Screen, size: (u16, u16)) -> (Color, Modifier) {
        let row = line_row(&painted(screen, size));
        let cell = cells(screen, size);
        (cell[(0, row)].fg, cell[(0, row)].modifier)
    }

    #[test]
    fn card_line_says_what_it_will_take_on_every_kind_of_card() {
        let size = (60, 14);

        // At a question, what that question will take, with the chevron in
        // the colour of a thing waiting on a person.
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

        // On an agent still at work, the word for what the line is: what is
        // typed there goes to it as it stands.
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

        // An agent whose command has ended still takes a line, because a line
        // typed there brings it back on those words — and the line says so,
        // since a resume is a vendor started again on the words, which is
        // more than a working agent's line costs.
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

        // And on the one there is nothing to bring back, what would come of
        // it — in the words the reply itself is refused in, because it is the
        // same fact said before rather than after the keystroke.
        let over = answering(ended(false), "");
        let past = answer_row(&painted(&over, size));
        assert!(past.contains("❯ nothing is listening"), "{past:?}");
        assert_eq!(chevron(&over, size), (Color::Reset, Modifier::DIM));
    }

    #[test]
    fn card_is_no_taller_than_what_it_has_to_show() {
        // An agent whose answer is one line does not want seven rows of box to
        // say it in, and every row the card leaves is a row of the wall.
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
        assert!(card[1].starts_with("  did what it was asked"), "{screen:?}");

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
        // A card with room for one row under its rule. What somebody is typing
        // is what that row is for: the question is on the agent's row above,
        // and the line is nowhere else at all.
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
        // A permission box has no field for words: they would land on whatever
        // is highlighted, which is an answer nobody chose.
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

        // And a card nobody is answering has the list's own keys under it.
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
        // The question is the agent's own words, and a bidirectional override
        // written into them can visually reorder the choices underneath —
        // which are the keys a person is about to press. ratatui drops the
        // control characters on its own; the invisible format characters it
        // keeps have to be neutralised before anything draws them.
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
        // The pane under a question is the vendor's drawing of the same box
        // the card already says in rows of its own, behind an echo of the
        // prompt: everything on it is noise below the answer line.
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

    /// The card over a patch of this many lines, which can be more than any
    /// card has rows for.
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
                }),
            )
        };

        // An answer reads forward, so the card opens on its first words.
        let opened = painted(&answered(), (60, 14)).join("\n");
        assert!(opened.contains("said 0"), "{opened}");
        assert!(!opened.contains("said 39"), "{opened}");

        // And paged, it stands that many rows below the top.
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
        // Forty rows of answer on a twenty-row screen: the card grows to
        // everything the height allows rather than the few lines a capture
        // used to fill, and the rest is there to page onto.
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

    /// A capture with the vendor's paint on it, which is what costs something
    /// to read: the escapes are what the walk is for.
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

        // A view redraws on every key, every tick and every mouse move. None
        // of them is a reason to read the same capture again.
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
            // The blank rows at the bottom are dropped where the body is
            // built, so what `tail` is handed is already the last row anybody
            // wrote on.
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
        // Paged back, the window stands above the bottom it is read from.
        assert_eq!(shown("a\nb\nc\nd\n\n", 2, 1), ["b", "c"]);
        assert!(shown("a\nb", 2, 5).is_empty());
    }

    /// The five rows claude draws at the bottom of every pane it has the room
    /// for, in the vendor's own order: the composer's top border with its
    /// right-anchored label, whatever is staged in the box, the composer's
    /// bottom border, the statusline, and the mode footer. Transcribed from a
    /// live 2.1.237 at 100 columns on 2026-08-21.
    const CHROME: [&str; 5] = [
        "───────────────────────────── execute amx-v2 tail ─",
        "❯ ",
        "───────────────────────────────────────────────────",
        "  Opus 5 │ ◈ 0% │ amx-main (main) │ ◖ xhigh",
        "  ⏵⏵ accept edits on (shift+tab to cycle) · ← 3 agents",
    ];

    /// A row of the agent's own work, which is the one thing no step may take.
    const SAID: &str = "what the agent said";

    /// claude's own anchors, which the rows above were measured off. The walk
    /// is handed the furniture of the vendor whose pane it is reading, and
    /// none of this chrome is findable without them.
    fn chrome() -> &'static Furniture {
        crate::rules::of("claude").furniture()
    }

    /// That screen with `typed` staged in the composer, under a row of work.
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
            cut(chrome(), &screen),
            [SAID, ""].as_slice(),
            "the spinner goes with the box it sits over"
        );
    }

    #[test]
    fn view_tail_cuts_a_composer_whatever_is_staged_in_it() {
        // A composer with one row of text in it is the state that let a walk
        // cutting exactly one input row pass for a working rule, so neither
        // fixture here has one: a task wrapped over three rows, and a message
        // typed over four lines.
        let wrapped = staged(&[
            "❯ port the importer and then check every",
            "  call site that used to take the old",
            "  shape",
        ]);
        assert_eq!(cut(chrome(), &wrapped), [SAID].as_slice());

        let lines = staged(&["❯ first", "  second", "  third", "  fourth"]);
        assert_eq!(cut(chrome(), &lines), [SAID].as_slice());
    }

    #[test]
    fn view_tail_leaves_a_screen_the_vendor_drew_no_footer_under_alone() {
        // A permission prompt, which ends at its own confirm row: cutting
        // upward from there would take the question the card was opened for.
        let prompt = [
            "───────────────────────────────────",
            " Bash command",
            "   rm -rf build",
            " Do you want to proceed?",
            " ❯ 1. Yes",
            "   2. No",
            " Esc to cancel · Tab to amend",
        ];
        assert_eq!(cut(chrome(), &prompt), prompt.as_slice());

        // And a pane too short for the vendor to draw its chrome in, whose
        // last row is the composer's own bottom border.
        let short = [SAID, CHROME[0], CHROME[1], CHROME[2]];
        assert_eq!(cut(chrome(), &short), short.as_slice());
    }

    #[test]
    fn view_tail_gives_back_by_position_what_it_cannot_place() {
        // A statusline is whatever somebody's command prints, and claude
        // 2.1.263 draws up to eight rows of it: four of four and eight of ten,
        // measured 2026-09-11. Every row within that is stepped over.
        for rows in [4, 8] {
            let mut tall = vec![SAID, CHROME[2]];
            tall.extend((0..rows).map(|_| "  status"));
            tall.push(CHROME[4]);
            assert_eq!(cut(chrome(), &tall), &tall[..1], "{rows} rows");
        }

        // Nine rows between the footer and the nearest rule: not a shape the
        // vendor draws, so the statusline step abandons and only the footer —
        // matched by its own opener — stays cut.
        let mut odd = vec![SAID, CHROME[2]];
        odd.extend((0..9).map(|_| "  status"));
        odd.push(CHROME[4]);
        assert_eq!(cut(chrome(), &odd), &odd[..odd.len() - 1]);

        // A composer whose staged text is taller than half the capture: the
        // scan runs past its cap without meeting a top border, so it gives
        // back every row it took and the box survives on screen.
        let mut runaway = vec![SAID];
        runaway.extend((0..8).map(|_| "  typed"));
        runaway.extend_from_slice(&CHROME[2..]);
        assert_eq!(
            cut(chrome(), &runaway),
            &runaway[..runaway.len() - 3],
            "the footer, the statusline and the bottom border keep their anchors"
        );
    }

    /// `capture-pane -p -J` of a live claude 2.1.237 at 72 columns on
    /// 2026-08-21, with a task typed into the composer and wrapped over three
    /// rows. Verbatim, trailing spaces and the no-break space after the
    /// chevron included: the rows above are transcriptions, and what a
    /// transcription cannot carry is exactly what these predicates walk over.
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
        // The pane's own padding under the last row the vendor drew on.
        let mut screen = CAPTURED.to_vec();
        screen.push("");

        // The warning claude renders flush against the composer's top border
        // with no blank row between them stays: it is above the box, and a
        // walk that ran upward until a blank row would have eaten it.
        assert_eq!(cut(chrome(), &screen), &CAPTURED[..2]);
    }

    #[test]
    fn view_tail_cuts_the_spinner_however_much_of_it_the_vendor_drew() {
        // The row claude spins while a turn runs, as it read for the 65
        // seconds before the first token at `--effort low` on 2026-09-06:
        // glyph, gerund, ellipsis and nothing after them. And as it reads on
        // a wide pane mid-turn, with the elapsed time and a detail behind.
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
            // The blank row over the spinner is left, as the blank rows a
            // pane is padded out with are: the walk trims both.
            assert_eq!(cut(chrome(), &screen), &screen[..2], "{spinner}");
        }

        // The line a finished turn leaves behind is the agent's, and stays.
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
        assert_eq!(cut(chrome(), &screen), &screen[..4]);
    }

    /// What a card's body says, with the paint it says it in set aside.
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

        // The one asking card that still shows its pane: amx missed the call
        // that drew the menu, so the pane is the only place the question is
        // written at all.
        let kept = said(asked(None), 24);
        assert!(
            kept.contains(&"Which features should be enabled?".to_string()),
            "{kept:?}"
        );

        // And with the question on it, the card is the question block alone:
        // the pane under it is the same box behind an echo of the prompt.
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
            lines[row].starts_with("  #40"),
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

        // Which is not what an agent with nothing to say gets: no capture was
        // cut there, and "the pane held only furniture" is a different fact.
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

        // The one row left after the cut, not the six rows the capture has: a
        // card that measured before it cut would spend its height on the
        // vendor's furniture. Two with the rule over it.
        assert_eq!(card_rows(&card.read(), None, &[], None, 60), 2);
    }
}
