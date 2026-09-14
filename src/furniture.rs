//! The vendor's own furniture, told apart from an agent's work.
//!
//! One pane, two authors: the transcript rows an agent earned, and the chrome
//! the vendor draws under them — composer box, statusline, mode footer, the
//! spinner of a running turn. Two surfaces read a captured pane and neither
//! wants the furniture: the card the view floats over an agent, and `amx
//! logs` printing a screen into somebody's terminal. The walk lives here so
//! the two cannot drift apart.
//!
//! The walk is here; what it walks over is not. Every anchor it steps on is
//! one vendor's own glyph, measured off that vendor and written down in its
//! screens document beside the rules that follow the same law — see
//! `assets/screen-rules.toml`. A pane whose vendor amx has measured no chrome
//! for keeps every row it has.

use serde::Deserialize;

/// The anchors that find one vendor's chrome, and the caps that keep the walk
/// off the transcript above it.
///
/// The `[furniture]` table of a vendor's screens document. A document that
/// leaves it out has none of this measured, which is not the same as a vendor
/// that draws nothing: it is amx not knowing, and the whole screen is then the
/// right answer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct Furniture {
    /// What the vendor's mode footer opens with, any one of them. The last row
    /// of every pane it has the room to draw one in, and the anchor the whole
    /// walk hangs off.
    pub mode: Vec<String>,
    /// The fragments the vendor's turn spinner always carries, all of them on
    /// the one row. What tells the line a running turn spins from the line it
    /// leaves behind when the turn is over.
    pub spinner: Vec<String>,
    /// The frames the vendor pulses in front of that line's message, any one
    /// of them and at the head of the row.
    ///
    /// The other way to find the same row, for a vendor that has more than one
    /// thing to say on it: pi draws a single status line and swaps out which
    /// of four messages is on it — one of them an extension's to write — so
    /// there is no fragment all four carry and the frame is the whole of what
    /// they share. A vendor whose message is fixed names fragments instead and
    /// leaves this out.
    #[serde(default)]
    pub frames: Vec<String>,
    /// The rule the vendor draws its composer's box with.
    pub rule: char,
    /// How many rows of statusline the walk will step over to reach the
    /// composer's bottom border.
    pub statusline: usize,
    /// How many rows the composer's bottom border can take.
    pub bottom: usize,
    /// What a row the vendor draws UNDER its mode footer opens with, after
    /// whatever it indents by, any one of them. The footer is the last row of
    /// most panes and not of all of them: a vendor that reports on the agents
    /// a turn started puts them below it.
    #[serde(default)]
    pub beneath: Vec<String>,
    /// How many such rows, blank ones counted, the walk will step over from
    /// the bottom before it must meet the footer. The cap on a step taken by
    /// position, the same as `statusline` is on its own.
    #[serde(default)]
    pub panel: usize,
    /// The fragments the row directly above the composer's top border carries,
    /// where the vendor draws one there. Not every screen has it, so it is a
    /// row the walk steps over when it is there rather than one it requires.
    #[serde(default)]
    pub hint: Vec<String>,
}

/// The vendor's own furniture, cut off the bottom of a capture.
///
/// The door the surfaces that print a pane come through, and the anchors come
/// with them: the record says which vendor was started in the pane, so the
/// walk is handed that vendor's own glyphs rather than whichever document the
/// binary happens to bundle first.
pub fn cut<'a, 'b>(furniture: &Furniture, rows: &'a [&'b str]) -> &'a [&'b str] {
    furniture.cut(rows)
}

impl Furniture {
    /// The chrome this vendor draws under every pane it has the room for: the
    /// row it hangs off the composer's top border, that border, whatever is
    /// staged in the box, the composer's bottom border, the statusline, the
    /// mode footer, and whatever the vendor draws below that footer. None of
    /// it is the agent's work, and all of it stands between a person and the
    /// rows they opened the card to read.
    ///
    /// **Read from the bottom, and every step capped.** A rule that found the
    /// last footer row and cut everything below it reads the same and is not:
    /// an agent that quotes a mode footer — `amx send` delivers captures of
    /// other panes — and then stops on a permission prompt would have the
    /// quotation found as the anchor and the prompt cut out from under it.
    /// From the bottom a quotation is unreachable, because a screen with a
    /// real prompt on it does not end in a footer. Where a step meets a shape
    /// it was not measured against it gives back what it cut by position and
    /// keeps what it cut by an anchor, so what a wrong number costs is
    /// furniture left on the screen and never a row of work taken off it.
    ///
    /// claude's numbers were measured against a live 2.1.237 on 2026-08-21 at
    /// 100, 30, 24, 23, 22, 21 and 20 columns and at pane heights 30, 12, 10,
    /// 9 and 8, with the composer empty and with three and ten rows staged in
    /// it — see the document they are written in.
    pub fn cut<'a, 'b>(&self, rows: &'a [&'b str]) -> &'a [&'b str] {
        // Past the blank rows a pane is padded out with, to the last row the
        // vendor actually drew on.
        let mut at = rows.len();
        while at > 0 && blank(rows[at - 1]) {
            at -= 1;
        }

        // The panel a vendor draws under its own footer, which claude 2.1.270
        // fills with the agents a turn started: a blank row, the main agent,
        // and one row each for the subagents. Stepped over by position and
        // capped like every other step of that kind — a panel taller than the
        // measurement leaves the walk on a row that is no footer, and the
        // screen is kept whole.
        let mut under = 0;
        while at > 0
            && !self.mode_footer(rows[at - 1])
            && under < self.panel
            && (blank(rows[at - 1]) || self.beneath_row(rows[at - 1]))
        {
            at -= 1;
            under += 1;
        }

        // The anchor. No footer, no cut: the screens carrying none are the
        // blocking prompts, the full-screen dialogs, a pane too small for the
        // vendor to draw its chrome in, and the seconds after a paste — and on
        // every one of them the whole screen is the right answer. A vendor amx
        // has measured no footer for carries none anywhere, and keeps every
        // screen whole for the same reason.
        if at == 0 || !self.mode_footer(rows[at - 1]) {
            return rows;
        }
        at -= 1;
        let footer = at;

        // The statusline, which is whatever somebody configured and is not
        // always there at all, so it is stepped over by position. The cap is
        // what keeps the walk off the transcript: claude renders a transient
        // warning flush against the composer's top border with no blank row
        // between them, and a walk that ran upward until a blank row would
        // have eaten it.
        let mut stepped = 0;
        while at > 0 && !self.rule_row(rows[at - 1]) {
            if stepped == self.statusline {
                return &rows[..footer];
            }
            at -= 1;
            stepped += 1;
        }
        if at == 0 {
            return &rows[..footer];
        }

        // The composer's bottom border.
        let mut borders = 0;
        while at > 0 && borders < self.bottom && self.rule_row(rows[at - 1]) {
            at -= 1;
            borders += 1;
        }
        let bottom = at;

        // Everything staged in the composer, however many rows of it there
        // are. The walk is between the box's two borders now, so these rows
        // are taken by position and never because one was recognised; what
        // stops it is the top border, which ends in its rule wherever the
        // label breaks. Reaching the cap means that border was never found,
        // and a step that cannot find its border gives back what it took.
        let mut typed = 0;
        while at > 0 && !self.ends_in_rule(rows[at - 1]) {
            if typed == rows.len() / 2 {
                return &rows[..bottom];
            }
            at -= 1;
            typed += 1;
        }
        if at == 0 {
            return &rows[..bottom];
        }

        // The composer's top border: the row the scan stopped on, and only it.
        at -= 1;

        // The row the vendor hangs off that border with no blank row between
        // them — claude 2.1.270 right-aligns its effort there. It is the
        // vendor's own writing, and it is also what the step below would
        // otherwise find where it went looking for the spinner.
        if at > 0 && self.hint_row(rows[at - 1]) {
            at -= 1;
        }

        // And the line the vendor spins while a turn runs, which sits above
        // the box with a blank row between them. A vendor that draws its
        // working indicator in the top border itself — pi since 0.85.1 —
        // has already lost it with that border; what this step finds on
        // such a vendor is the row it keeps up there for a compaction.
        let mut above = at;
        while above > 0 && blank(rows[above - 1]) {
            above -= 1;
        }
        match above > 0 && self.spinning(rows[above - 1]) {
            true => &rows[..above - 1],
            false => &rows[..at],
        }
    }

    /// A row that is the vendor's rule and nothing else, which is what the
    /// composer's bottom border is. Never a blank row: every character of an
    /// empty string is a rule, and a blank row is not a border.
    fn rule_row(&self, row: &str) -> bool {
        let drawn = row.trim();
        !drawn.is_empty() && drawn.chars().all(|glyph| glyph == self.rule)
    }

    /// A row the vendor's rule ends. The composer's top border carries a
    /// right-anchored label, so it is not a rule row — but its last character
    /// is the rule wherever the label breaks, and that is what makes it
    /// findable.
    fn ends_in_rule(&self, row: &str) -> bool {
        row.trim_end().ends_with(self.rule)
    }

    /// The vendor's mode footer, which is the last row of every pane it has
    /// the room to draw one in. Read from what the row opens with, so a footer
    /// the vendor indents is still a footer and a glyph mid-sentence is not.
    fn mode_footer(&self, row: &str) -> bool {
        let drawn = row.trim_start();
        self.mode.iter().any(|opening| drawn.starts_with(opening))
    }

    /// A row of the panel the vendor draws under its mode footer. Read from
    /// what the row opens with, the same way the footer is, so the indent the
    /// vendor puts in front of it costs nothing.
    fn beneath_row(&self, row: &str) -> bool {
        let drawn = row.trim_start();
        self.beneath
            .iter()
            .any(|opening| drawn.starts_with(opening))
    }

    /// The row the vendor draws directly above the composer's top border, told
    /// by any one of the fragments it carries: the row is right-aligned, so
    /// where it starts is the pane's width and not the vendor's choice.
    fn hint_row(&self, row: &str) -> bool {
        self.hint.iter().any(|fragment| row.contains(fragment))
    }

    /// The line the vendor spins while a turn runs, told apart from the line
    /// it leaves behind when the turn is over.
    ///
    /// Two anchors and either of them finds it: the fragments only the running
    /// line carries, all of them on the row, and the frames the vendor pulses
    /// in front of the message, any one of them at the head of it. Which of
    /// the two a vendor is read by is that vendor's own document to say.
    ///
    /// A vendor amx has measured neither for has nothing to find, and an empty
    /// list of fragments would say every row is one, so it says none is.
    pub fn spinning(&self, row: &str) -> bool {
        let fragments =
            !self.spinner.is_empty() && self.spinner.iter().all(|fragment| row.contains(fragment));
        fragments || self.framed(row)
    }

    /// Whether this vendor's document names a spinning row at all: fragments
    /// the row carries, or frames it opens with. A vendor measured for neither
    /// has no line to find, and a reader should not pay for a screen to look.
    pub fn spins(&self) -> bool {
        !self.spinner.is_empty() || !self.frames.is_empty()
    }

    /// A row one of the vendor's spinner frames opens. Read from what the row
    /// opens with, the same way the mode footer is: a frame the vendor indents
    /// still opens the row, and the same glyph mid-sentence opens nothing.
    fn framed(&self, row: &str) -> bool {
        let drawn = row.trim_start();
        self.frames.iter().any(|frame| drawn.starts_with(frame))
    }
}

/// A row with nothing on it.
fn blank(row: &str) -> bool {
    row.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pane of the second vendor's, chrome and all: a composer box drawn
    /// with `=`, a statusline, and a mode footer opening with a word rather
    /// than a glyph. Nothing on it is a string claude draws.
    const A_SECOND_VENDOR_PANE: &[&str] = &[
        "  It did the thing.",
        "",
        " = compose =",
        " >",
        " ===========",
        "  model: small",
        "  mode: careful",
    ];

    /// The same pane with a turn running on it, which puts the vendor's own
    /// spinner line above the box.
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

    /// claude's own chrome, transcribed from a live 2.1.237 on 2026-08-21.
    const A_CLAUDE_PANE: &[&str] = &[
        "  Ran the migration.",
        "",
        "──────────────────────────── execute amx-v2 ─",
        "❯ ",
        "─────────────────────────────────────────────",
        "  Opus 5 │ amx-main (main) │ xhigh",
        "  ⏵⏵ accept edits on (shift+tab to cycle)",
    ];

    /// claude 2.1.270 with a subagent running, transcribed from
    /// `/tmp/measure-270/c4-bgline-w100.txt` on 2026-09-14 at 100 columns. The
    /// vendor draws the agents it is running UNDER its own mode footer, so the
    /// last row of the pane is a subagent's and not the anchor the walk needs,
    /// and a right-aligned `● high · /effort` sits directly on top of the
    /// composer's border with no blank row between.
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

    /// The same vendor a turn later, from `/tmp/measure-270/c8-plan-after.txt`:
    /// the footer is the last row again, but the hint row is drawn over the
    /// composer with the spinner of the running turn directly above it and no
    /// blank row anywhere between the three.
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

    /// A pane nobody has typed in yet, from `/tmp/measure-270/2-fresh.txt`:
    /// the welcome box, the hint row over the composer, and nothing else.
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

    /// A pane whose turn is over, from `/tmp/measure-270/4-idle.txt`. This
    /// vendor draws no hint row on it — which is why the row is measured as
    /// one the walk looks for rather than one it can count on.
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

    fn claude() -> &'static Furniture {
        crate::rules::of("claude").furniture()
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
        // The walk is the same walk; every glyph it steps on is the second
        // vendor's, and claude shares not one of them.
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
        // Anchors are not nearly right on somebody else's chrome — they are
        // absent. The walk finds no footer, and a screen it cannot read the
        // bottom of keeps every row it has.
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
        // Which is the floor: amx does not know where this vendor's work ends
        // and its chrome begins, so it cuts nothing rather than guessing at a
        // border. Costing furniture on the screen, never a row of work off it.
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
        // The footer is no longer the last row this vendor draws: 2.1.270 puts
        // the agents a turn started under it, a blank row and then one row an
        // agent. A walk that wanted the footer at the bottom found a subagent
        // there instead and kept all thirty rows, chrome and all.
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
        // The hint row sits on the composer's top border with no blank row
        // between, so the step that looks for the spinner above the box met it
        // instead and both rows survived onto the card. Above it here is a
        // running turn's spinner, which goes with it.
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
        // Nobody has typed in this one, so what the hint row stands on is the
        // blank middle of a fresh pane and the welcome box is the whole of the
        // screen's content.
        assert_eq!(claude().cut(A_FRESH_PANE), &A_FRESH_PANE[..24]);
    }

    #[test]
    fn furniture_a_pane_with_no_hint_row_is_cut_where_it_always_was() {
        // The hint row is a row the walk looks for and never one it requires:
        // this vendor draws none here, and the cut is the one it made before
        // the row was measured at all.
        assert_eq!(claude().cut(AN_IDLE_PANE), &AN_IDLE_PANE[..25]);
    }

    #[test]
    fn furniture_a_panel_taller_than_the_measurement_keeps_the_whole_screen() {
        // Which is the law the walk is built on paying out again: the rows
        // under the footer are stepped over by position, and a step that runs
        // out of room before it finds its anchor gives back everything it took
        // rather than guessing where the work ends.
        let mut pane = A_CLAUDE_PANE.to_vec();
        pane.extend(std::iter::repeat_n(
            "  ◯ general-purpose  Preparing…  3s",
            9,
        ));
        assert_eq!(claude().cut(&pane), pane);
    }
}
