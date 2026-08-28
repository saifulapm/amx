//! Measuring and cutting what the screen says.
//!
//! Every band ends up here: a row is only as wide as the terminal, and what
//! goes on it has to be measured in the columns a terminal draws rather than
//! in the characters a string holds. Nothing here knows which band it is
//! working for.

use ratatui::text::Span;

/// How many columns a block of spans takes.
pub(super) fn said(spans: &[Span<'static>]) -> usize {
    spans.iter().map(|span| span.content.chars().count()).sum()
}

/// What stands between two things said on one row.
pub(super) const SEPARATOR: &str = " · ";

/// What a rule is drawn out of, wherever amx draws one: over a group, over a
/// project, over a group of keys, and along the edge a line being typed hangs
/// off.
///
/// The lightest dash box drawing has, rather than the solid `─` this used to
/// be. Both are dim, and dim is the same weight the summary column wears — but
/// a terminal draws a box-drawing glyph as ink across the whole cell, so a
/// solid rule reads brighter than the words beside it at the identical colour.
/// Half the cells left blank is what finally puts the two at the same weight,
/// and it is the only lever there is: the colour was already right.
///
/// Not the vendor's rule. claude draws its own chrome in `─` and amx reads
/// those rows to find the bottom of a pane — see [`crate::furniture`] — so the
/// two staying different characters is worth having.
pub(super) const RULE: &str = "┈";

/// Text out of an agent's own screen, made safe to hand a terminal. The paint
/// is gone by the time this runs, so what is left to neutralise is the
/// characters that were never paint: the controls and the invisible format
/// characters a row can be written to carry.
pub(super) fn inert(text: &str) -> String {
    crate::tmux::sanitize(text)
}

/// The columns `text` takes on a screen, which is not its characters: an
/// emoji is one char and two columns, and a row measured in characters
/// pushes its last column off the terminal's edge. ratatui's own measure,
/// so a row is cut by the same arithmetic it is drawn with.
pub(super) fn width_of(text: &str) -> usize {
    Span::raw(text).width()
}

/// `text`, cut to `width` with an ellipsis for what was cut.
pub(super) fn fit(text: &str, width: usize) -> String {
    if width_of(text) <= width {
        return text.to_string();
    }
    match width {
        0 => String::new(),
        1 => "…".to_string(),
        _ => {
            let mut kept = String::new();
            let mut used = 0;
            for one in text.chars() {
                let wide = width_of(one.encode_utf8(&mut [0; 4]));
                if used + wide > width - 1 {
                    break;
                }
                used += wide;
                kept.push(one);
            }
            kept.push('…');
            kept
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_cuts_text_without_losing_the_last_character_to_the_ellipsis() {
        assert_eq!(fit("short", 10), "short");
        assert_eq!(fit("exactly", 7), "exactly");
        assert_eq!(fit("too long by far", 8), "too lon…");
        assert_eq!(fit("anything", 1), "…");
        assert_eq!(fit("anything", 0), "");
    }
}
