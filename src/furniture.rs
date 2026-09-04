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
    /// The rule the vendor draws its composer's box with.
    pub rule: char,
    /// How many rows of statusline the walk will step over to reach the
    /// composer's bottom border.
    pub statusline: usize,
    /// How many rows the composer's bottom border can take.
    pub bottom: usize,
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
    /// composer's top border, whatever is staged in the box, the composer's
    /// bottom border, the statusline, and the mode footer. None of it is the
    /// agent's work, and all of it stands between a person and the rows they
    /// opened the card to read.
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

        // And the line the vendor spins while a turn runs, which sits above
        // the box with a blank row between them.
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

    /// The line the vendor spins while a turn runs, told apart from the line
    /// it leaves behind when the turn is over by the fragments only the
    /// running one carries.
    ///
    /// A vendor amx has measured no spinner for has nothing to find, and an
    /// empty list would say every row is one, so it says none is.
    pub fn spinning(&self, row: &str) -> bool {
        !self.spinner.is_empty() && self.spinner.iter().all(|fragment| row.contains(fragment))
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

        let claude = crate::rules::bundled().furniture();
        assert_eq!(claude.cut(A_CLAUDE_PANE), ["  Ran the migration.", ""]);
    }

    #[test]
    fn furniture_one_vendors_anchors_cut_nothing_off_anothers_pane() {
        // Anchors are not nearly right on somebody else's chrome — they are
        // absent. The walk finds no footer, and a screen it cannot read the
        // bottom of keeps every row it has.
        assert_eq!(second().cut(A_CLAUDE_PANE), A_CLAUDE_PANE);
        assert_eq!(
            crate::rules::bundled()
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
    }
}
