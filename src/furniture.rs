//! The vendor's chrome (composer box, statusline, mode footer, spinner line),
//! told apart from the agent's output above it.
//!
//! The card and `amx logs` both cut the chrome off a captured pane and share
//! this walk. Every anchor it steps on is one vendor's own glyph, read from
//! the `[furniture]` table of that vendor's screens document. A vendor with no
//! measured chrome keeps every row.

use serde::Deserialize;

/// Anchors that find one vendor's chrome, and caps that keep the walk off the
/// transcript above it.
///
/// The `[furniture]` table of a screens document. A document without one has
/// nothing measured, and the whole screen is kept.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct Furniture {
    /// Openings of the mode footer, any one of them. The walk hangs off this
    /// row.
    pub mode: Vec<String>,
    /// Fragments the spinner line of a running turn always carries, all on one
    /// row. They tell it from the line left behind once the turn is over.
    pub spinner: Vec<String>,
    /// Frames the vendor pulses at the head of its spinner line, any one of
    /// them.
    ///
    /// For a vendor whose message varies: pi swaps between four messages on
    /// one status line, one of them written by an extension, so the frame is
    /// all they share. A vendor with a fixed message names `spinner` instead.
    #[serde(default)]
    pub frames: Vec<String>,
    /// Whole, trimmed rows the vendor draws while hidden reasoning runs, any
    /// one of them. pi 0.85.1 with `hideThinkingBlock` draws `Thinking...`
    /// until text or a tool row follows it.
    #[serde(default)]
    pub thinking: Vec<String>,
    /// The character the composer box is drawn with.
    pub rule: char,
    /// Most statusline rows the walk steps over to reach the composer.
    pub statusline: usize,
    /// Most rows the composer's bottom border can take.
    pub bottom: usize,
    /// Openings, after indent, of rows the vendor draws under its mode footer.
    /// claude 2.1.270 lists the agents a turn started there.
    #[serde(default)]
    pub beneath: Vec<String>,
    /// Most rows under the footer, blank ones included, the walk steps over.
    #[serde(default)]
    pub panel: usize,
    /// Fragments of the optional row directly above the composer's top border.
    #[serde(default)]
    pub hint: Vec<String>,
    /// Openings, after indent, of the optional row between the spinner line
    /// and the composer.
    #[serde(default)]
    pub tip: Vec<String>,
    /// Openings, after indent, of the row the vendor leaves when a turn is cut
    /// short in its own pane.
    ///
    /// No hook fires on esc, so this row is the only record of that turn
    /// ending. See [`crate::derive`].
    #[serde(default)]
    pub interrupted: Vec<String>,
    /// Fragments of the mode footer while the vendor still runs a shell of its
    /// own, any one of them.
    ///
    /// Not part of the walk. This is what says the background count a hook
    /// wrote is still true. Empty means the vendor never says, and
    /// [`Furniture::shells_running`] answers `None`.
    #[serde(default)]
    pub shells: Vec<String>,
    /// Footer tails that are key hints rather than counts, any one of them.
    /// Removed before `shells` is matched, so a hint's separator is not read
    /// as a shell.
    #[serde(default)]
    pub footer_hints: Vec<String>,
}

impl Furniture {
    /// The rows above the vendor's chrome: the hint row over the composer, the
    /// composer box and whatever is staged in it, the statusline, the mode
    /// footer, any panel under it, and the spinner line of a running turn.
    ///
    /// The walk goes up from the bottom and every step is capped. From the
    /// bottom, a quoted footer higher up (a capture delivered by `amx send`,
    /// say) is never taken for the anchor. A step that meets a shape it was not
    /// measured against gives back what it took by position and keeps what it
    /// took by anchor, so a wrong cap leaves chrome on screen and never cuts a
    /// row of work.
    pub fn cut<'a, 'b>(&self, rows: &'a [&'b str]) -> &'a [&'b str] {
        let at = match self.composer(rows) {
            Ok(at) => at,
            Err(kept) => return &rows[..kept],
        };

        // Then the spinner line above the box. A vendor that spins in the top
        // border (pi since 0.85.1) lost it with the border; on pi this finds
        // the row it keeps there for a compaction.
        match self.above_composer(rows, at) {
            Some(above) if self.spinning(rows[above]) => &rows[..above],
            _ => &rows[..at],
        }
    }

    /// The first row with content above the composer, past blanks and the
    /// hint row: where a vendor that spins a line above its box spins it.
    /// `None` when the walk finds no composer or nothing above it.
    pub fn spinner_row(&self, rows: &[&str]) -> Option<usize> {
        self.above_composer(rows, self.composer(rows).ok()?)
    }

    /// The first non-blank row above `at`, past a tip row under the spinner.
    fn above_composer(&self, rows: &[&str], at: usize) -> Option<usize> {
        let row = rows[..at].iter().rposition(|row| !blank(row))?;
        match self.tip_row(rows[row]) {
            true => rows[..row].iter().rposition(|row| !blank(row)),
            false => Some(row),
        }
    }

    /// Walks up from the bottom to the composer's top border. `Ok` is the
    /// index where the chrome starts; `Err` is how many rows to keep when a
    /// step gave up first.
    fn composer(&self, rows: &[&str]) -> Result<usize, usize> {
        // Skip the blank rows the pane is padded with.
        let mut at = rows.len();
        while at > 0 && blank(rows[at - 1]) {
            at -= 1;
        }

        // The panel under the footer (claude 2.1.270 lists a turn's agents
        // there), stepped over by position and capped. A taller panel leaves
        // the walk on a row that is no footer, and the screen is kept whole.
        let mut under = 0;
        while at > 0
            && !self.mode_footer(rows[at - 1])
            && under < self.panel
            && (blank(rows[at - 1]) || self.beneath_row(rows[at - 1]))
        {
            at -= 1;
            under += 1;
        }

        // No footer, no cut: blocking prompts, full-screen dialogs, panes too
        // small for chrome, the moment after a paste and unmeasured vendors all
        // keep the whole screen.
        if at == 0 || !self.mode_footer(rows[at - 1]) {
            return Err(rows.len());
        }
        at -= 1;
        let footer = at;

        // The statusline is configurable and may be absent, so it is stepped
        // over by position. The cap keeps the walk off the transcript: claude
        // draws transient warnings flush against the composer's top border.
        let mut stepped = 0;
        while at > 0 && !self.rule_row(rows[at - 1]) {
            if stepped == self.statusline {
                return Err(footer);
            }
            at -= 1;
            stepped += 1;
        }
        if at == 0 {
            return Err(footer);
        }

        // The composer's bottom border.
        let mut borders = 0;
        while at > 0 && borders < self.bottom && self.rule_row(rows[at - 1]) {
            at -= 1;
            borders += 1;
        }
        let bottom = at;

        // Whatever is staged in the composer, taken by position until a row
        // ending in the rule, which is the top border. Hitting the cap means
        // the border was never found, so give back what was taken.
        let mut typed = 0;
        while at > 0 && !self.ends_in_rule(rows[at - 1]) {
            if typed == rows.len() / 2 {
                return Err(bottom);
            }
            at -= 1;
            typed += 1;
        }
        if at == 0 {
            return Err(bottom);
        }

        // The top border.
        at -= 1;

        // The hint row hung off the top border with no gap (claude 2.1.270
        // right-aligns its effort level there).
        if at > 0 && self.hint_row(rows[at - 1]) {
            at -= 1;
        }
        Ok(at)
    }

    /// A row of nothing but the rule: the composer's bottom border. A blank
    /// row is not a border.
    fn rule_row(&self, row: &str) -> bool {
        let drawn = row.trim();
        !drawn.is_empty() && drawn.chars().all(|glyph| glyph == self.rule)
    }

    /// A row ending in the rule. The composer's top border carries a label,
    /// but its last character is the rule wherever the label breaks.
    fn ends_in_rule(&self, row: &str) -> bool {
        row.trim_end().ends_with(self.rule)
    }

    /// Whether the row opens, after indent, like the mode footer.
    fn mode_footer(&self, row: &str) -> bool {
        let drawn = row.trim_start();
        self.mode.iter().any(|opening| drawn.starts_with(opening))
    }

    /// Whether the row opens, after indent, like a panel row under the footer.
    fn beneath_row(&self, row: &str) -> bool {
        let drawn = row.trim_start();
        self.beneath
            .iter()
            .any(|opening| drawn.starts_with(opening))
    }

    /// Whether the row carries a hint fragment. Matched anywhere, because the
    /// row is right-aligned and where it starts depends on the pane width.
    fn hint_row(&self, row: &str) -> bool {
        self.hint.iter().any(|fragment| row.contains(fragment))
    }

    /// Whether the row opens, after indent, like a tip row.
    fn tip_row(&self, row: &str) -> bool {
        let drawn = spaced(row.trim_start());
        self.tip.iter().any(|opening| drawn.starts_with(opening))
    }

    /// Whether the row is the spinner line of a running turn: it carries every
    /// `spinner` fragment, or opens with one of the `frames`.
    ///
    /// An empty `spinner` list matches no row rather than every row.
    pub fn spinning(&self, row: &str) -> bool {
        let fragments =
            !self.spinner.is_empty() && self.spinner.iter().all(|fragment| row.contains(fragment));
        fragments || self.framed(row)
    }

    /// Whether this vendor has a spinner line to find at all.
    pub fn spins(&self) -> bool {
        !self.spinner.is_empty() || !self.frames.is_empty()
    }

    /// Whether one of the frames opens the row, past indent and any run of the
    /// rule. pi 0.85.1 draws its spinner into the composer's top border:
    /// `── ⠙ Working ───`.
    fn framed(&self, row: &str) -> bool {
        let drawn = self.unruled(row);
        self.frames.iter().any(|frame| drawn.starts_with(frame))
    }

    /// The row with the rule and the spaces beside it trimmed off both ends.
    pub fn unruled<'a>(&self, row: &'a str) -> &'a str {
        row.trim().trim_matches(self.rule).trim()
    }

    /// Whether the row is one of the vendor's hidden-reasoning rows.
    pub fn thinking(&self, row: &str) -> bool {
        let drawn = row.trim();
        self.thinking.iter().any(|label| label == drawn)
    }

    /// Whether the last row of work, chrome cut, is the vendor's marker for a
    /// turn interrupted in its own pane.
    ///
    /// The marker stays in the transcript for the rest of the session, so only
    /// its being last makes it current. Anything after it means the screen is
    /// about something else.
    pub fn cut_by_hand(&self, rows: &[&str]) -> bool {
        if self.interrupted.is_empty() {
            return false;
        }
        let said = self.cut(rows);
        let Some(last) = said.iter().rev().find(|row| !blank(row)) else {
            return false;
        };
        let drawn = spaced(last.trim_start());
        self.interrupted
            .iter()
            .any(|opening| drawn.starts_with(opening))
    }

    /// Whether the mode footer says the vendor still runs a shell, or `None`
    /// for a vendor that never says.
    ///
    /// The footer is redrawn every frame, so it is current where the count on
    /// the record dates from the end of the turn. The lowest footer is read,
    /// because a quoted footer sits above the real one.
    pub fn shells_running(&self, rows: &[&str]) -> Option<bool> {
        if self.shells.is_empty() {
            return None;
        }
        let footer = rows.iter().rposition(|row| self.mode_footer(row))?;
        let footer = self
            .footer_hints
            .iter()
            .fold(rows[footer].to_string(), |row, hint| row.replace(hint, ""));
        Some(self.shells.iter().any(|fragment| footer.contains(fragment)))
    }
}

fn blank(row: &str) -> bool {
    row.trim().is_empty()
}

/// The row with non-breaking spaces turned into spaces.
///
/// claude pads tool-result rows with U+00A0 (`⎿ \u{a0}`) so a wrap cannot
/// split the glyph from its words. Anchors are written with a plain space,
/// so the row is folded before an anchor that spans such a gap is tried.
/// Only there: folding every row of every capture is wasted work.
fn spaced(row: &str) -> String {
    row.replace('\u{a0}', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The second vendor's pane: a composer drawn with `=`, a statusline, and
    /// a mode footer that opens with a word. Nothing on it is claude's.
    const A_SECOND_VENDOR_PANE: &[&str] = &[
        "  It did the thing.",
        "",
        " = compose =",
        " >",
        " ===========",
        "  model: small",
        "  mode: careful",
    ];

    /// The same pane mid-turn, with the spinner line above the box.
    const A_SECOND_VENDOR_MID_TURN: &[&str] = &[
        "  It did the thing.",
        "",
        " thinking for 12s",
        " = compose =",
        " >",
        " ===========",
        "  model: small",
        "  mode: careful",
    ];

    /// claude 2.1.237 chrome.
    const A_CLAUDE_PANE: &[&str] = &[
        "  Ran the migration.",
        "",
        "──────────────────────────── execute amx-v2 ─",
        "❯ ",
        "─────────────────────────────────────────────",
        "  Opus 5 │ amx-main (main) │ xhigh",
        "  ⏵⏵ accept edits on (shift+tab to cycle)",
    ];

    /// claude 2.1.270 at 100 columns with a subagent running. The agents are
    /// listed under the mode footer, and a right-aligned `● high · /effort`
    /// hint sits on the composer's top border with no blank row between.
    const A_BACKGROUND_LINE_PANE: &[&str] = &[
        "",
        " ▐▛███▛█   Claude Code v2.1.270 ",
        "▝▜██████▀  Opus 5 (1M context) with high effort · Claude Max",
        "  ▝▝ ▝▝    /tmp/measure-270                                                                         ",
        "",
        "",
        "❯ /btw ",
        "  ⎿  Usage: /btw <your question>   ",
        "",
        "❯ Use the Task tool to start exactly one subagent in the background whose whole job is to run the   ",
        "  shell command sleep 45 and then say finished. Do not wait for it. Reply with the single word      ",
        "  started as soon as it is launched.                                                                ",
        "",
        "● I'll launch it in the background.",
        "",
        "● Agent(Sleep 45 then report)",
        "  ⎿  Backgrounded agent (↓ to manage · ctrl+o to expand)                                            ",
        "                                              ",
        "● started",
        "           ",
        "✻ Waiting for 1 background agent to finish",
        "                                                                                  ● high · /effort",
        "────────────────────────────────────────────────────────────────────────────────────────────────────",
        "❯                                    ",
        "────────────────────────────────────────────────────────────────────────────────────────────────────",
        "  Opus 5 (1M context) (1M context) │ ◈ 2% │ measure-270 │ ◖ high",
        "  ⏵⏵ auto mode on (shift+tab to cycle) · ← 2 agents",
        "        ",
        "  ● main",
        "  ◯ general-purpose  Preparing to run `sleep 45`                              48s · ↓ 10.2k tokens",
    ];

    /// claude 2.1.270 a turn later: the footer is last again, and the spinner,
    /// the hint row and the composer touch with no blank row between them.
    const A_PLAN_PANE_AFTER_A_SUBAGENT: &[&str] = &[
        "     │                                                                                             │",
        "     │ Terminal captures of the Claude Code v2.1.270 UI — startup box, transcript, status line and │",
        "     │ background-task line — recorded in numbered series and, for the `c*` files, at several      │",
        "     │ terminal                                                                                    │",
        "     │ widths.                                                                                     │",
        "     │                                                                                             │",
        "     │ Assumption worth flagging: I inferred the folder's purpose from the filenames and from      │",
        "     │ reading                                                                                     │",
        "     │ c0-fresh.txt and c1-transcript-w40.txt. If these captures were taken for a specific         │",
        "     │ investigation (e.g. checking how the status line wraps at narrow widths), say so and the    │",
        "     │ sentence                                                                                    │",
        "     │ should name that instead — it is more useful than describing the file format.               │",
        "     │                                                                                             │",
        "     │ Verification                                                                                │",
        "     │                                                                                             │",
        "     │ - cat /tmp/measure-270/README.md — file exists and holds the sentence above.                │",
        "     │ - ls /tmp/measure-270 — README.md present, all pre-existing .txt files unchanged.           │",
        "     ╰─────────────────────────────────────────────────────────────────────────────────────────────╯",
        "                                                                                    ",
        "✻ Churned for 20s · done 3:11 PM",
        "",
        r#"● Agent "Sleep 45 then report" finished · 1m 34s                                                  "#,
        "                                                                                   ",
        "● Wibbling… (20s)",
        "                                                                                  ● high · /effort",
        "────────────────────────────────────────────────────────────────────────────────────────────────────",
        "❯                                 ",
        "────────────────────────────────────────────────────────────────────────────────────────────────────",
        "  Opus 5 (1M context) (1M context) │ ◈ 3% │ measure-270 │ ◖ high",
        "  ⏸ plan mode on (shift+tab to cycle) · /tasks to see subagents · ← 2 agents",
    ];

    /// A fresh claude 2.1.270 pane: the welcome box, the hint row and chrome.
    const A_FRESH_PANE: &[&str] = &[
        "",
        " ▐▛███▛█   Claude Code v2.1.270",
        "▝▜██████▀  Opus 5 (1M context) with high effort · Claude Max",
        "  ▝▝ ▝▝    /tmp/measure-270",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "                                                                                  ● high · /effort",
        "────────────────────────────────────────────────────────────────────────────────────────────────────",
        "❯  ",
        "────────────────────────────────────────────────────────────────────────────────────────────────────",
        "  Opus 5 (1M context) (1M context) │ ◈ 0% │ measure-270 │ ◖ high",
        "  ⏵⏵ auto mode on (shift+tab to cycle) · ← 2 agents  ",
    ];

    /// An idle claude 2.1.270 pane with no hint row.
    const AN_IDLE_PANE: &[&str] = &[
        "",
        " ▐▛███▛█   Claude Code v2.1.270",
        "▝▜██████▀  Opus 5 (1M context) with high effort · Claude Max",
        "  ▝▝ ▝▝    /tmp/measure-270",
        "",
        "",
        "❯ Use the Bash tool to run exactly: sleep 15. Then reply with the single word done.                 ",
        "",
        "● I'll run that now.      ",
        "",
        "  Ran 1 shell command          ",
        "",
        "● done                                           ",
        "",
        "✻ Cooked for 18s · done 2:17 PM",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "                                                                                                  ",
        "────────────────────────────────────────────────────────────────────────────────────────────────────",
        "❯                                                                                   ",
        "────────────────────────────────────────────────────────────────────────────────────────────────────",
        "  Opus 5 (1M context) (1M context) │ ◈ 2% │ measure-270 │ ◖ high",
        "  ⏵⏵ auto mode on (shift+tab to cycle) · ← 2 agents  ",
    ];

    /// `A_BACKGROUND_LINE_PANE` at 54 columns. The panel and hint row are the
    /// same; only the wrapping differs.
    const A_BACKGROUND_LINE_AT_54: &[&str] = &[
        "▝▜██████▀  Claude Max",
        "  ▝▝ ▝▝    /tmp/measure-270",
        "",
        "",
        "❯ /btw ",
        "  ⎿  Usage: /btw <your question>",
        "",
        "❯ Use the Task tool to start exactly one subagent in  ",
        "  the background whose whole job is to run the shell  ",
        "  command sleep 45 and then say finished. Do not wait ",
        "  for it. Reply with the single word started as soon  ",
        "  as it is launched.                                  ",
        "",
        "● I'll launch it in the background.",
        "",
        "● Agent(Sleep 45 then report)",
        "  ⎿  Backgrounded agent (↓ to manage · ctrl+o to ",
        "",
        "● started",
        "",
        "✻ Waiting for 1 background agent to finish",
        "                                    ● high · /effort",
        "──────────────────────────────────────────────────────",
        "❯  ",
        "──────────────────────────────────────────────────────",
        "  Opus 5 (1M context) (1M context) │ ◈ 2% │ measure…",
        "  ⏵⏵ auto mode on (shift+tab to cycle) · ← 2 agents",
        "",
        "  ● main",
        "  ◯ general-purpose  Preparing… 49s · ↓ 10.2k tokens",
    ];

    /// At 40 columns: the waiting line wraps and the subagent row loses its
    /// label.
    const A_BACKGROUND_LINE_AT_40: &[&str] = &[
        "",
        "❯ /btw ",
        "  ⎿  Usage: /btw <your question>",
        "",
        "❯ Use the Task tool to start exactly    ",
        "  one subagent in the background whose  ",
        "  whole job is to run the shell command ",
        "  sleep 45 and then say finished. Do    ",
        "  not wait for it. Reply with the       ",
        "  single word started as soon as it is  ",
        "  launched.                             ",
        "",
        "● I'll launch it in the background.",
        "",
        "● Agent(Sleep 45 then report)",
        "  ⎿  Backgrounded agent (↓ to ",
        "",
        "● started",
        "",
        "✻ Waiting for 1 background agent to ",
        "  finish",
        "                      ● high · /effort",
        "────────────────────────────────────────",
        "❯  ",
        "────────────────────────────────────────",
        "  Opus 5 (1M context) (1M context) │ …",
        "  ⏵⏵ auto mode on (shift+tab to cycle)",
        "",
        "  ● main",
        "  ◯ general-purpose 50s · ↓ 10.2k tokens",
    ];

    /// At 30 columns: the statusline is elided mid-word.
    const A_BACKGROUND_LINE_AT_30: &[&str] = &[
        "",
        "❯ Use the Task tool to start  ",
        "  exactly one subagent in the ",
        "  background whose whole job  ",
        "  is to run the shell         ",
        "  command sleep 45 and then   ",
        "  say finished. Do not wait   ",
        "  for it. Reply with the      ",
        "  single word started as soon ",
        "  as it is launched.          ",
        "",
        "● I'll launch it in the",
        "  background.",
        "",
        "● Agent(Sleep 45 then report)",
        "  ⎿  Backgrounded agent ",
        "",
        "● started",
        "",
        "✻ Waiting for 1 background ",
        "  agent to finish",
        "            ● high · /effort",
        "──────────────────────────────",
        "❯  ",
        "──────────────────────────────",
        "  Opus 5 (1M context) (1M c…",
        "  ⏵⏵ auto mode on (shift+tab",
        "",
        "  ● main",
        "  ◯ general-purpose 51s · ↓ ",
    ];

    /// At 24 columns: the waiting line takes three rows.
    const A_BACKGROUND_LINE_AT_24: &[&str] = &[
        "  whole job is to run   ",
        "  the shell command     ",
        "  sleep 45 and then say ",
        "  finished. Do not      ",
        "  wait for it. Reply    ",
        "  with the single word  ",
        "  started as soon as it ",
        "  is launched.          ",
        "",
        "● I'll launch it in the",
        "  background.",
        "",
        "● Agent(Sleep 45 then",
        "       report)",
        "  ⎿  Backgrounded",
        "",
        "● started",
        "",
        "✻ Waiting for 1 ",
        "  background agent to ",
        "  finish",
        "      ● high · /effort",
        "────────────────────────",
        "❯  ",
        "────────────────────────",
        "  Opus 5 (1M context)…",
        "  ⏵⏵ auto mode on ",
        "",
        "  ● main",
        "  ◯ general-purpose 51s ",
    ];

    /// An idle claude 2.1.270 pane that does draw the hint row.
    const AN_IDLE_PANE_WITH_A_HINT_ROW: &[&str] = &[
        "",
        " ▐▛███▛█   Claude Code v2.1.270",
        "▝▜██████▀  Opus 5 (1M context) with high effort · Claude Max",
        "  ▝▝ ▝▝    /tmp/measure-270",
        "",
        "",
        "❯ First use the AskUserQuestion tool to ask me one question, \"Proceed?\", with the two options Yes   ",
        "  and No. After I answer, run exactly: sleep 5 with the Bash tool, then reply with the single word  ",
        "  done.                                                                                             ",
        "",
        "● I'll ask the question first.     ",
        "",
        "● User answered Claude's questions:",
        "  ⎿  · Proceed? → Yes     ",
        "",
        "  Ran 1 shell command          ",
        "",
        "● done                                           ",
        "       ",
        "✻ Cogitated for 10s · done 2:19 PM",
        "                      ",
        "",
        "                                                                                                  ",
        "",
        "                                                                                  ● high · /effort",
        "────────────────────────────────────────────────────────────────────────────────────────────────────",
        "❯        ",
        "────────────────────────────────────────────────────────────────────────────────────────────────────",
        "  Opus 5 (1M context) (1M context) │ ◈ 2% │ measure-270 │ ◖ high",
        "  ⏸ manual mode on · ← 2 agents  ",
    ];

    fn claude() -> &'static Furniture {
        crate::rules::of("claude").furniture()
    }

    #[test]
    fn furniture_reads_an_interrupt_across_the_gap_the_vendor_holds_open() {
        // Captured off a live claude pane. The gap after the glyph is U+00A0,
        // so an anchor typed with plain spaces matched nothing and the row
        // stayed `working` until a quiescent rule timed out. Written as an
        // escape because the two characters look alike.
        let interrupted = [
            "❯ Tell me about this project",
            "",
            "● Skill(mem)",
            "  ⎿ \u{a0}Initializing…",
            "  ⎿ \u{a0}Error: Unknown skill: mem. Did you mean new?",
            "  ⎿ \u{a0}Interrupted · What should Claude do instead?",
            "",
            "────────────────────────────────────────",
            "❯\u{a0}",
            "────────────────────────────────────────",
            "  Haiku 4.5 │ ◈ 9% │ amx (main) │ ◖ thinking",
            "  ⏵⏵ bypass permissions on (shift+tab to cycle)",
        ];
        assert!(
            interrupted[5].contains('\u{a0}'),
            "the row this is about carries the gap it is about"
        );
        assert!(claude().cut_by_hand(&interrupted));

        // Tool results above it carry the same gap and are not interrupts.
        let finished = [
            "● Skill(mem)",
            "  ⎿ \u{a0}Error: Unknown skill: mem. Did you mean new?",
            "",
            "────────────────────────────────────────",
            "❯\u{a0}",
            "────────────────────────────────────────",
            "  ⏵⏵ bypass permissions on (shift+tab to cycle)",
        ];
        assert!(!claude().cut_by_hand(&finished));
    }

    fn second() -> Furniture {
        let screens = crate::vendor::second::SECOND
            .screens
            .expect("the second vendor draws screens of its own");
        crate::rules::Ruleset::parse(screens)
            .expect("and they parse")
            .furniture()
            .clone()
    }

    #[test]
    fn furniture_is_cut_by_the_anchors_its_own_vendor_named() {
        // The same walk, with every glyph the second vendor's own.
        assert_eq!(
            second().cut(A_SECOND_VENDOR_PANE),
            ["  It did the thing.", ""]
        );
        assert_eq!(
            second().cut(A_SECOND_VENDOR_MID_TURN),
            ["  It did the thing.", ""],
            "the line a turn spins is the vendor's too"
        );

        let claude = crate::rules::of("claude").furniture();
        assert_eq!(claude.cut(A_CLAUDE_PANE), ["  Ran the migration.", ""]);
    }

    #[test]
    fn furniture_one_vendors_anchors_cut_nothing_off_anothers_pane() {
        // Another vendor's anchors are absent from this chrome: no footer is
        // found and nothing is cut.
        assert_eq!(second().cut(A_CLAUDE_PANE), A_CLAUDE_PANE);
        assert_eq!(
            crate::rules::of("claude")
                .furniture()
                .cut(A_SECOND_VENDOR_PANE),
            A_SECOND_VENDOR_PANE
        );
    }

    #[test]
    fn furniture_a_vendor_nobody_has_measured_keeps_its_whole_screen() {
        // Nothing measured: cut nothing rather than guess at a border.
        let unmeasured = Furniture::default();
        assert_eq!(unmeasured.cut(A_CLAUDE_PANE), A_CLAUDE_PANE);
        assert_eq!(unmeasured.cut(A_SECOND_VENDOR_PANE), A_SECOND_VENDOR_PANE);
        assert!(
            !unmeasured.spinning(" thinking for 12s"),
            "no fragments to find is not every row found"
        );
        assert!(
            !unmeasured.spinning(" ⠼ Working..."),
            "and neither is no frames to find"
        );
    }

    #[test]
    fn furniture_the_walk_steps_under_the_footer_to_reach_it() {
        // claude 2.1.270 lists the agents a turn started under its footer, so
        // the footer is no longer the last row.
        let kept = claude().cut(A_BACKGROUND_LINE_PANE);
        assert_eq!(kept, &A_BACKGROUND_LINE_PANE[..21]);
        assert_eq!(
            kept.last(),
            Some(&"✻ Waiting for 1 background agent to finish"),
            "the line the vendor draws about its own subagents is not a row of work"
        );
    }

    #[test]
    fn furniture_the_walk_cuts_the_hint_row_and_what_is_over_it() {
        // The hint row sits on the top border with no gap. The spinner line
        // above it goes too.
        let kept = claude().cut(A_PLAN_PANE_AFTER_A_SUBAGENT);
        assert_eq!(kept, &A_PLAN_PANE_AFTER_A_SUBAGENT[..23]);
        assert!(
            kept[21].starts_with("● Agent \"Sleep 45 then report\" finished"),
            "the turn's own last word is kept, and the blank row under it: {:?}",
            kept[21]
        );
    }

    #[test]
    fn furniture_the_hint_row_is_cut_off_a_pane_with_nothing_above_it() {
        // A fresh pane: the welcome box is all the content.
        assert_eq!(claude().cut(A_FRESH_PANE), &A_FRESH_PANE[..24]);
    }

    #[test]
    fn furniture_a_pane_with_no_hint_row_is_cut_where_it_always_was() {
        // No hint row here. The walk looks for one and does not require it.
        assert_eq!(claude().cut(AN_IDLE_PANE), &AN_IDLE_PANE[..25]);
    }

    #[test]
    fn furniture_the_narrow_widths_are_cut_where_the_wide_one_is() {
        // The panel and the hint row are drawn at every width, so the cut falls
        // in the same place and the waiting line is kept whole.
        for (width, pane) in [
            (54, A_BACKGROUND_LINE_AT_54),
            (40, A_BACKGROUND_LINE_AT_40),
            (30, A_BACKGROUND_LINE_AT_30),
            (24, A_BACKGROUND_LINE_AT_24),
        ] {
            let kept = claude().cut(pane);
            assert_eq!(kept, &pane[..21], "at {width} columns");
            assert!(
                kept.last().is_some_and(|row| row.ends_with("finish")),
                "at {width} columns the waiting line is kept whole: {:?}",
                kept.last()
            );
        }
    }

    #[test]
    fn furniture_an_idle_pane_that_does_draw_the_hint_row_loses_it() {
        // AN_IDLE_PANE with the hint row drawn: it comes off too.
        let kept = claude().cut(AN_IDLE_PANE_WITH_A_HINT_ROW);
        assert_eq!(kept, &AN_IDLE_PANE_WITH_A_HINT_ROW[..24]);
        assert!(
            !kept.iter().any(|row| row.contains("/effort")),
            "the row the walk used to leave on the card is off it"
        );
    }

    #[test]
    fn furniture_a_panel_taller_than_the_measurement_keeps_the_whole_screen() {
        // The panel is stepped over by position, so one taller than the cap
        // gives every row back.
        let mut pane = A_CLAUDE_PANE.to_vec();
        pane.extend(std::iter::repeat_n(
            "  ◯ general-purpose  Preparing…  3s",
            9,
        ));
        assert_eq!(claude().cut(&pane), pane);
    }

    #[test]
    fn furniture_a_footer_hint_is_no_shell_and_a_count_of_agents_is_one() {
        // claude 2.1.278 ends its footer with `· ← for agents` in every mode,
        // which is a key hint. `← 5 agents` in the same place is a count.
        for footer in [
            "⏵⏵ auto mode on (shift+tab to cycle) · ← for agents",
            "⏵⏵ accept edits on (shift+tab to cycle) · ← for agents",
            "⏸ plan mode on (shift+tab to cycle) · ← for agents",
            "⏸ manual mode on · ← for agents",
            "⏵⏵ bypass permissions on (shift+tab to cycle) · ← for agents",
            "⏵⏵ don't ask on (shift+tab to cycle) · ← for agents",
        ] {
            assert_eq!(claude().shells_running(&[footer]), Some(false), "{footer}");
        }
        assert_eq!(
            claude().shells_running(&["⏸ manual mode on · ← 5 agents"]),
            Some(true)
        );
        assert_eq!(
            claude().shells_running(&["⏵⏵ bypass permissions on · 1 shell · ← for agents"]),
            Some(true)
        );
    }
}
