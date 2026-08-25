//! claude's own furniture, told apart from an agent's work.
//!
//! One pane, two authors: the transcript rows an agent earned, and the chrome
//! the vendor draws under them — composer box, statusline, mode footer, the
//! spinner of a running turn. Two surfaces read a captured pane and neither
//! wants the furniture: the card the view floats over an agent, and `amx
//! logs` printing a screen into somebody's terminal. The walk lives here so
//! the two cannot drift apart, and every anchor in it is measured — see the
//! comments below, and assets/screen-rules.toml for the law they follow.

/// claude's own furniture, cut off the bottom of a capture.
///
/// The vendor draws the same block under every pane it has the room for: the
/// composer's top border, whatever is staged in the box, the composer's bottom
/// border, the statusline, and the mode footer. None of it is the agent's
/// work, and all of it stands between a person and the rows they opened the
/// card to read.
///
/// **Read from the bottom, and every step capped.** A rule that found the last
/// footer row and cut everything below it reads the same and is not: an agent
/// that quotes a mode footer — `amx send` delivers captures of other panes —
/// and then stops on a permission prompt would have the quotation found as the
/// anchor and the prompt cut out from under it. From the bottom a quotation is
/// unreachable, because a screen with a real prompt on it does not end in a
/// footer. Where a step meets a shape it was not measured against it gives
/// back what it cut by position and keeps what it cut by an anchor, so what a
/// wrong number costs is furniture left on the screen and never a row of work
/// taken off it.
///
/// Measured against a live claude 2.1.237 on 2026-08-21 at 100, 30, 24, 23,
/// 22, 21 and 20 columns and at pane heights 30, 12, 10, 9 and 8, with the
/// composer empty and with three and ten rows staged in it.
pub fn cut<'a, 'b>(rows: &'a [&'b str]) -> &'a [&'b str] {
    // Past the blank rows a pane is padded out with, to the last row the
    // vendor actually drew on.
    let mut at = rows.len();
    while at > 0 && blank(rows[at - 1]) {
        at -= 1;
    }

    // The anchor. No footer, no cut: the screens carrying none are the
    // blocking prompts, the full-screen dialogs, a pane too small for the
    // vendor to draw its chrome in, and the seconds after a paste — and on
    // every one of them the whole screen is the right answer.
    if at == 0 || !mode_footer(rows[at - 1]) {
        return rows;
    }
    at -= 1;
    let footer = at;

    // The statusline, which is whatever somebody configured and is not always
    // there at all, so it is stepped over by position. The cap is what keeps
    // the walk off the transcript: claude renders a transient warning flush
    // against the composer's top border with no blank row between them, and a
    // walk that ran upward until a blank row would have eaten it.
    let mut stepped = 0;
    while at > 0 && !rule_row(rows[at - 1]) {
        if stepped == STATUSLINE {
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
    while at > 0 && borders < BOTTOM && rule_row(rows[at - 1]) {
        at -= 1;
        borders += 1;
    }
    let bottom = at;

    // Everything staged in the composer, however many rows of it there are.
    // The walk is between the box's two borders now, so these rows are taken
    // by position and never because one was recognised; what stops it is the
    // top border, which ends in its rule wherever the label breaks. Reaching
    // the cap means that border was never found, and a step that cannot find
    // its border gives back what it took.
    let mut typed = 0;
    while at > 0 && !ends_in_rule(rows[at - 1]) {
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

    // And the line claude spins while a turn runs, which sits above the box
    // with a blank row between them.
    let mut above = at;
    while above > 0 && blank(rows[above - 1]) {
        above -= 1;
    }
    match above > 0 && spinning(rows[above - 1]) {
        true => &rows[..above - 1],
        false => &rows[..at],
    }
}

/// How many rows of statusline the walk will step over to reach the composer's
/// bottom border: a margin over the one row measured, not a measured maximum.
const STATUSLINE: usize = 2;

/// How many rows the composer's bottom border can take. One at 22 columns and
/// wider, which is every pane 2.1.237 draws a footer in at all; two below
/// that, where the box is wider than the pane and wraps.
const BOTTOM: usize = 2;

/// The rule claude draws its composer's box with.
const RULE: char = '─';

/// What claude's mode footer opens with. The two glyphs are the whole of the
/// anchor: the words after them truncate as the pane narrows and are gone by
/// 30 columns, and these are present in all six permission modes at every
/// width from 24 to 220.
const MODE: [&str; 2] = ["⏵⏵", "⏸"];

/// The two fragments claude's turn spinner always carries — the ellipsis
/// before its elapsed time and the separator after it. Punctuation rather than
/// any word, so the vendor renaming its gerunds does not move the anchor, and
/// neither fragment is on the line it leaves behind when the turn is over.
const SPINNING: [&str; 2] = ["… (", "s · "];

/// A row with nothing on it.
fn blank(row: &str) -> bool {
    row.trim().is_empty()
}

/// A row that is the vendor's rule and nothing else, which is what the
/// composer's bottom border is. Never a blank row: every character of an empty
/// string is a rule, and a blank row is not a border.
fn rule_row(row: &str) -> bool {
    let drawn = row.trim();
    !drawn.is_empty() && drawn.chars().all(|c| c == RULE)
}

/// A row the vendor's rule ends. The composer's top border carries a
/// right-anchored label, so it is not a rule row — but its last character is
/// the rule wherever the label breaks, and that is what makes it findable.
fn ends_in_rule(row: &str) -> bool {
    row.trim_end().ends_with(RULE)
}

/// claude's mode footer, which is the last row of every pane the vendor has
/// the room to draw one in. Read from what the row opens with, so a footer the
/// vendor indents is still a footer and a glyph mid-sentence is not.
fn mode_footer(row: &str) -> bool {
    let drawn = row.trim_start();
    MODE.iter().any(|glyph| drawn.starts_with(glyph))
}

/// The line claude spins while a turn runs, told apart from the line it leaves
/// behind when the turn is over by the ellipsis and the elapsed time.
fn spinning(row: &str) -> bool {
    SPINNING.iter().all(|fragment| row.contains(fragment))
}
