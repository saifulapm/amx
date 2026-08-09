//! The table itself: five columns, dropped from the least useful end until the
//! whole row fits the terminal it is going to.
//!
//! The one-shot form and `--watch` call this with the same arguments and print
//! the same lines; the only thing `--watch` adds is a footer of its own.

use amx_core::agent::EpochMillis;
use amx_proto::control::agent::{AgentEntry, ListReply};

use super::{age, ordered};

/// What separates two columns.
const GAP: usize = 2;

/// The narrowest last-line column worth keeping.
///
/// Below this the column says nothing a reader can use, and the space is worth
/// more given back to the columns before it — which is how the table survives
/// the 45-column SSH window D14 exists for.
const MIN_LAST: usize = 8;

/// The header row, which is also the floor under every column's width.
const HEAD: Columns<&str> = Columns {
    agent: "AGENT",
    status: "STATUS",
    reason: "REASON",
    age: "AGE",
    last: "LAST LINE",
};

/// One row's five cells.
///
/// Generic only so [`HEAD`] can be a `const` of borrowed strings beside the
/// owned ones a reply produces.
struct Columns<S> {
    agent: S,
    status: S,
    reason: S,
    age: S,
    last: S,
}

/// Render `reply` as the lines a terminal `width` columns wide can hold.
///
/// `width` is `None` when nothing is going to a terminal — a pipe, a file, a
/// capture — and then no line is truncated at all: a consumer redirecting this
/// wants the last line it asked for, not the part of it that would have fitted
/// a window that is not there.
///
/// `now` is the server's own clock, from the same reply (D-M4-4). Passing this
/// rather than reading a clock here is what makes an age identical on both ends
/// of an SSH link, and it is why `--watch` advances the value it passes by
/// monotonic elapsed time instead of asking the local machine what time it is.
#[must_use]
pub fn render(reply: &ListReply, now: EpochMillis, width: Option<usize>) -> Vec<String> {
    let mut lines = vec![summary(reply)];
    if reply.agents.is_empty() {
        return lines;
    }

    let rows: Vec<Columns<String>> = ordered(reply)
        .into_iter()
        .map(|entry| cells(entry, now))
        .collect();
    let layout = Layout::fit(&rows, width);
    lines.push(layout.line(&HEAD.owned()));
    lines.extend(rows.iter().map(|row| layout.line(row)));
    // Only after the layout has done what it can: a column dropped is a column
    // a reader can tell is missing, and a line clipped here is the last resort
    // for a terminal narrower than one agent's name.
    if let Some(width) = width {
        for line in &mut lines {
            *line = clip(line, width);
        }
    }
    lines
}

/// The count line: how many agents, and how many of them are waiting.
///
/// The blocked count is the *queue's* length and not a tally of rows in the
/// `blocked` state, for the reason X11 pinned on the status line: the number a
/// person reads and the queue `agent.next` walks have to be one number.
fn summary(reply: &ListReply) -> String {
    let agents = reply.agents.len();
    if agents == 0 {
        return "no agents".to_owned();
    }
    let noun = if agents == 1 { "agent" } else { "agents" };
    if reply.attention.is_empty() {
        return format!("{agents} {noun}");
    }
    format!("{agents} {noun} · {} blocked", reply.attention.len())
}

/// One entry's cells, before anything knows how wide they may be.
fn cells(entry: &AgentEntry, now: EpochMillis) -> Columns<String> {
    let workspace = entry.workspace.name.as_deref().unwrap_or("-");
    let name = entry.name.as_deref().unwrap_or("-");
    Columns {
        agent: format!("{workspace}/{name}"),
        status: entry.status.to_string(),
        // Blank, not a placeholder. A probe-derived status and both tier-3
        // states carry no reason at all, and that is the ordinary case rather
        // than a missing value: a column of dashes would be noise on every row
        // of a quiet session (X00's wave-2 boundary).
        reason: plain(entry.reason.as_deref().unwrap_or_default()),
        age: age(now, entry.since),
        last: plain(&entry.last_line),
    }
}

/// Which columns survive this width, and how wide each one is.
struct Layout {
    agent: usize,
    status: usize,
    reason: Option<usize>,
    age: Option<usize>,
    last: Option<usize>,
}

impl Layout {
    /// The widest layout `width` can hold, dropping columns from the least
    /// useful end.
    ///
    /// The order is reason, then last line, then age, and it is a claim about
    /// what a narrow reader still needs: `status` already says *that* an agent
    /// is blocked, so the detector's name is the first thing worth giving up;
    /// the line on its screen says *what about*, which is the question D15
    /// exists to answer, so it goes second; the age goes last, because "who has
    /// waited longest" is the whole of the queue's order.
    fn fit(rows: &[Columns<String>], width: Option<usize>) -> Self {
        let widest = |cell: fn(&Columns<String>) -> &str, head: &str| {
            rows.iter()
                .map(|row| chars(cell(row)))
                .chain(std::iter::once(chars(head)))
                .max()
                .unwrap_or(0)
        };
        // A column no row has anything to put in is dropped before width is
        // even considered: a `REASON` heading over a session of probe-derived
        // statuses spends eight columns saying that eight columns were
        // available. The two on the left are never dropped — a table with no
        // agent and no status is not a table.
        let filled =
            |cell: fn(&Columns<String>) -> &str| rows.iter().any(|row| !cell(row).is_empty());
        let agent = widest(|row| &row.agent, HEAD.agent);
        let status = widest(|row| &row.status, HEAD.status);
        let reason = filled(|row| &row.reason).then(|| widest(|row| &row.reason, HEAD.reason));
        let last = filled(|row| &row.last).then(|| widest(|row| &row.last, HEAD.last));
        let age = filled(|row| &row.age).then(|| widest(|row| &row.age, HEAD.age));

        let Some(width) = width else {
            return Self {
                agent,
                status,
                reason,
                age,
                last,
            };
        };

        // Everything but the last line is fixed-width, so the question at each
        // step is only whether what remains leaves the last column room to say
        // anything.
        let fixed = |reason: Option<usize>, age: Option<usize>| {
            agent + GAP + status + reason.map_or(0, |w| GAP + w) + age.map_or(0, |w| GAP + w)
        };
        if last.is_some() {
            for candidate in [reason, None] {
                let spent = fixed(candidate, age);
                if spent + GAP + MIN_LAST <= width {
                    return Self {
                        agent,
                        status,
                        reason: candidate,
                        age,
                        last: Some(width - spent - GAP),
                    };
                }
            }
        }
        // No room for a last line at all. Keep whatever else fits, and let
        // `clip` finish the job for a terminal too narrow even for that.
        for (reason, age) in [(reason, age), (None, age), (None, None)] {
            if fixed(reason, age) <= width {
                return Self {
                    agent,
                    status,
                    reason,
                    age,
                    last: None,
                };
            }
        }
        Self {
            agent,
            status,
            reason: None,
            age: None,
            last: None,
        }
    }

    /// One row, padded to this layout.
    fn line(&self, row: &Columns<String>) -> String {
        let mut out = String::new();
        pad(&mut out, &row.agent, self.agent);
        out.push_str(&" ".repeat(GAP));
        // The last surviving column is never padded: trailing blanks on every
        // line of a table somebody is about to `grep` are litter.
        let mut trailing = String::new();
        pad(&mut trailing, &row.status, self.status);
        if let Some(width) = self.reason {
            trailing.push_str(&" ".repeat(GAP));
            pad(&mut trailing, &row.reason, width);
        }
        if let Some(width) = self.age {
            trailing.push_str(&" ".repeat(GAP));
            // Right-aligned, so `4m` and `11m` line up on their unit and a
            // column of ages reads as a column of numbers.
            let cell = clip(&row.age, width);
            trailing.push_str(&" ".repeat(width.saturating_sub(chars(&cell))));
            trailing.push_str(&cell);
        }
        if let Some(width) = self.last {
            trailing.push_str(&" ".repeat(GAP));
            trailing.push_str(&clip(&row.last, width));
        }
        out.push_str(trailing.trim_end());
        out
    }
}

impl Columns<&'static str> {
    /// The header row as owned cells, so it renders through the same code every
    /// other row does.
    fn owned(&self) -> Columns<String> {
        Columns {
            agent: self.agent.to_owned(),
            status: self.status.to_owned(),
            reason: self.reason.to_owned(),
            age: self.age.to_owned(),
            last: self.last.to_owned(),
        }
    }
}

/// Append `cell` to `out`, padded to `width` and clipped if it is wider.
fn pad(out: &mut String, cell: &str, width: usize) {
    let cell = clip(cell, width);
    out.push_str(&cell);
    out.push_str(&" ".repeat(width.saturating_sub(chars(&cell))));
}

/// `text` in at most `width` columns, ending in an ellipsis when it did not
/// fit.
///
/// Counted in `char`s, which is the same approximation `amx session report`
/// makes: a wide glyph or a combining mark can still push a line one cell over.
/// It costs a ragged column and never a broken frame, because `--watch` clears
/// each row before it writes one.
fn clip(text: &str, width: usize) -> String {
    if chars(text) <= width {
        return text.to_owned();
    }
    match width {
        0 => String::new(),
        1 => "…".to_owned(),
        _ => {
            let mut out: String = text.chars().take(width - 1).collect();
            out.push('…');
            out
        }
    }
}

/// `text` with anything a terminal would obey replaced by a space, and both
/// ends trimmed.
///
/// `last_line` is read off a cell grid and a `reason` is a detector's own
/// identifier, so neither should carry a control character — but this table is
/// written straight to a terminal by `--watch`, and "should" is not a property
/// a monitor can rely on when the string ultimately came from whatever an agent
/// printed.
///
/// The leading trim is a rendering decision and not a change to the fact: the
/// server sends the row with its own indentation intact and `--json` prints it
/// that way, but a dialog's `  2. No` indented inside a column that is already
/// aligned costs two cells of a 45-column window to say nothing.
fn plain(text: &str) -> String {
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// How many columns `text` claims.
fn chars(text: &str) -> usize {
    text.chars().count()
}
