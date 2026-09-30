//! Measuring and truncating text in terminal columns.
//!
//! Everything here measures in columns, not chars: a wide character is one
//! char and two columns.

use ratatui::text::Span;

/// The columns a run of spans takes.
pub(super) fn said(spans: &[Span<'static>]) -> usize {
    spans.iter().map(Span::width).sum()
}

/// The separator between two items on one row.
pub(super) const SEPARATOR: &str = " · ";

/// The glyph every rule is drawn with.
///
/// A light dashed line: a solid `─` fills the whole cell and reads brighter
/// than dim text beside it at the same colour. It also differs from the `─`
/// claude draws its chrome in, which [`crate::furniture`] looks for.
pub(super) const RULE: &str = "┈";

/// `text` with control and invisible format characters replaced, safe to hand
/// to a terminal.
pub(super) fn inert(text: &str) -> String {
    crate::tmux::sanitize(text)
}

/// The columns `text` takes, measured the way ratatui draws it.
pub(in crate::tui) fn width_of(text: &str) -> usize {
    Span::raw(text).width()
}

/// The columns one character takes.
pub(in crate::tui) fn char_width(c: char) -> usize {
    width_of(c.encode_utf8(&mut [0; 4]))
}

/// `text` cut to `width` columns, with an ellipsis where it was cut.
pub(in crate::tui) fn fit(text: &str, width: usize) -> String {
    if width_of(text) <= width {
        return text.to_string();
    }
    match width {
        0 => String::new(),
        width => format!("{}…", head(text, width - 1)),
    }
}

/// The longest prefix of `text` that fits in `width` columns.
pub(in crate::tui) fn head(text: &str, width: usize) -> String {
    let mut kept = String::new();
    let mut used = 0;
    for one in text.chars() {
        let wide = char_width(one);
        if used + wide > width {
            break;
        }
        used += wide;
        kept.push(one);
    }
    kept
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
