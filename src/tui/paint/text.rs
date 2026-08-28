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
