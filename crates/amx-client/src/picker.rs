//! The one fuzzy-list primitive (04 §7).
//!
//! "One fuzzy-list primitive for every choose-one interaction (workspaces,
//! panes, agents, commands, worktrees, move targets). No other dialog type
//! exists." [`Picker`] is that primitive, and its deliberate poverty is the
//! design: it holds labels and answers with an index. It knows nothing about
//! workspaces, panes or commands — every source flattens itself to labels on
//! the way in and maps the chosen index back on the way out, which is what
//! keeps the choose-one interaction *one* code path instead of one per
//! domain.
//!
//! Keys: printable ASCII edits the query, backspace deletes, `ctrl+n`/
//! `ctrl+p` move the selection, `Enter` chooses, `Esc` cancels. The matcher
//! is a case-insensitive subsequence match scored by compactness — earlier
//! and tighter matches rank first, ties keep source order — because a picker
//! driven by a few typed characters needs predictability more than
//! cleverness.
//!
//! # The one extension: a second column
//!
//! `docs/10-attention-surfaces.md` §D15 licenses exactly one addition to this
//! primitive — "entries may carry a live detail line" — and that sentence is
//! the whole of it. An entry may carry a *detail*: a second string beside its
//! label, replaceable without disturbing the query, the match set or anything
//! the caller has selected. Two rules keep it from becoming a second dialog
//! type:
//!
//! - **The detail is never matched.** [`score`] reads the label and nothing
//!   else, so typing filters on what the row *is* and never on what it is
//!   currently saying. A detail that participated in the match would be a
//!   filter syntax nobody wrote down, which D15 rejects by name.
//! - **The detail is not the picker's business to draw.** It is held here so
//!   one surface can refresh every entry's second column in one call; where
//!   the column goes, how wide it is and what it says are the caller's.
//!
//! Its reader is the agents view (`crate::app::agents`), whose detail is the
//! agent's last screen line as `agent.list` reports it, refreshed at up to
//! 4 Hz while the list itself sits still.

/// `Esc`.
const ESC: u8 = 0x1b;
/// `ctrl+n`: selection down.
const CTRL_N: u8 = 0x0e;
/// `ctrl+p`: selection up.
const CTRL_P: u8 = 0x10;

/// What one picker key did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickerEvent {
    /// The picker is still open.
    Continue,
    /// `Enter` on a match: the index into the item list the picker was built
    /// with. The caller maps it back to whatever the label stood for.
    Chosen(usize),
    /// `Esc`: the interaction ended with nothing chosen.
    Cancelled,
}

/// The fuzzy list: every choose-one interaction in amx is one of these.
#[derive(Clone, Debug)]
pub struct Picker {
    /// The labels, in the order the source produced them.
    items: Vec<String>,
    /// One optional second column per item, same order and same length.
    ///
    /// Empty until a caller sets one, which is what makes the extension free
    /// for the sources that do not want it: every picker in the tree but the
    /// agents view holds an empty `Vec` here and nothing about it changes.
    details: Vec<String>,
    /// The typed filter.
    query: String,
    /// Indices into `items` that match `query`, best first.
    matches: Vec<usize>,
    /// Position in `matches` the selection is on.
    selected: usize,
}

/// A picker over nothing, which is what a surface holds before its source has
/// answered.
impl Default for Picker {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl Picker {
    /// A picker over `items`, with an empty query matching all of them in
    /// source order.
    #[must_use]
    pub fn new(items: Vec<String>) -> Self {
        let matches = (0..items.len()).collect();
        Self {
            items,
            details: Vec::new(),
            query: String::new(),
            matches,
            selected: 0,
        }
    }

    /// The labels the picker was built with.
    #[must_use]
    pub fn items(&self) -> &[String] {
        &self.items
    }

    /// Replace every entry's second column, leaving the query, the match set
    /// and the selection exactly where they were.
    ///
    /// The whole point of the extension: a live detail line is refreshed on a
    /// cadence of its own while the user is typing into the list it belongs to,
    /// and a refresh that rebuilt the picker would throw the query away four
    /// times a second. Extra details are dropped and missing ones read as
    /// absent, so a caller whose row count changed under it cannot desynchronise
    /// the two lists — it gets a stale-but-aligned column until it rebuilds.
    pub fn set_details(&mut self, details: Vec<String>) {
        self.details = details;
        self.details.truncate(self.items.len());
    }

    /// The second column of the item at `index`, if it has one.
    #[must_use]
    pub fn detail(&self, index: usize) -> Option<&str> {
        self.details.get(index).map(String::as_str)
    }

    /// The current query.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The matching item indices, best first.
    #[must_use]
    pub fn matches(&self) -> &[usize] {
        &self.matches
    }

    /// The item index the selection is on, if anything matches.
    #[must_use]
    pub fn selected(&self) -> Option<usize> {
        self.matches.get(self.selected).copied()
    }

    /// Apply one key.
    pub fn key(&mut self, b: u8) -> PickerEvent {
        match b {
            b'\r' | b'\n' => {
                if let Some(item) = self.selected() {
                    return PickerEvent::Chosen(item);
                }
            }
            ESC => return PickerEvent::Cancelled,
            0x08 | 0x7f => {
                self.query.pop();
                self.refilter();
            }
            CTRL_N => {
                if self.selected + 1 < self.matches.len() {
                    self.selected += 1;
                }
            }
            CTRL_P => self.selected = self.selected.saturating_sub(1),
            0x20..=0x7e => {
                self.query.push(char::from(b));
                self.refilter();
            }
            _ => {}
        }
        PickerEvent::Continue
    }

    /// Recompute `matches` for the current query and reset the selection to
    /// the best match.
    fn refilter(&mut self) {
        let mut scored: Vec<(u32, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(at, label)| score(&self.query, label).map(|s| (s, at)))
            .collect();
        scored.sort_unstable();
        self.matches.clear();
        self.matches.extend(scored.into_iter().map(|(_, at)| at));
        self.selected = 0;
    }
}

/// Score `label` against `query`: `None` if `query` is not a case-insensitive
/// subsequence of `label`, otherwise a cost — the match's start position plus
/// every gap between matched characters — where lower is better. An empty
/// query matches everything at cost zero.
fn score(query: &str, label: &str) -> Option<u32> {
    let mut cost = 0_u32;
    let mut at = 0_u32;
    let mut previous = None;
    let mut wanted = query.chars().flat_map(char::to_lowercase);
    let mut want = wanted.next();
    for ch in label.chars().flat_map(char::to_lowercase) {
        let Some(expect) = want else { break };
        if ch == expect {
            cost = cost.saturating_add(previous.map_or(at, |p: u32| at - p - 1));
            previous = Some(at);
            want = wanted.next();
        }
        at = at.saturating_add(1);
    }
    want.is_none().then_some(cost)
}
