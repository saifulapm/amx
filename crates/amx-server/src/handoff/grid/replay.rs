//! Everything a pane is that is not its visible grid, as a replay.
//!
//! The grid half lives in [`super`]; this is what goes around it, in the order
//! the seed applies them (D-M3-4, corrected by W04): the paint prologue, then
//! the packed history rows, then the grid, then the modes, then the cursor.
//! Each of the four is a function here, and the ordering between them is stated
//! on each rather than kept in one caller's head.

use amx_proto::stream::history::PackedRows;
use amx_vt::{Cursor, CursorStyle};

use super::emit::{put_csi, put_csi_space_q, put_cup, put_dec_mode, put_number};
use crate::handoff::manifest::PaneModes;

/// Append the cursor's shape, blink, visibility and position.
///
/// Emitted **after** the modes, not with the grid, and that ordering is not
/// cosmetic: replaying DEC mode 6 homes the cursor, so a position written
/// before the modes would be thrown away by the first origin-mode sequence that
/// follows it.
pub fn put_cursor(cursor: Cursor, out: &mut Vec<u8>) {
    // DECSCUSR pairs shape with blink: odd blinks, even is steady. A hollow
    // block is how an *unfocused* block renders rather than a shape anything
    // can select, so it replays as the block it is.
    let shape = match cursor.style {
        CursorStyle::Block | CursorStyle::BlockHollow => 1,
        CursorStyle::Underline => 3,
        CursorStyle::Bar => 5,
    };
    put_csi_space_q(out, shape + u32::from(!cursor.blinking));
    out.extend_from_slice(if cursor.visible {
        b"\x1b[?25h".as_slice()
    } else {
        b"\x1b[?25l".as_slice()
    });
    if cursor.visible_in_viewport {
        put_cup(out, usize::from(cursor.y), usize::from(cursor.x));
    }
}

/// Append the modes that have to hold *while* a grid is painted.
///
/// Five of them, each because the paint would otherwise mean something else:
/// wraparound off would drop the last column of every wrapped row, origin mode
/// would make the cursor addressing relative, insert mode would push text
/// sideways, synchronised output would hold the frames back, and grapheme
/// clustering decides whether a combining mark joins the cell before it or
/// takes one of its own — so that one is set to what the pane actually had
/// rather than to a safe value.
pub fn put_paint_prologue(modes: &PaneModes, out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1b[?7h\x1b[?6l\x1b[4l\x1b[?2026l");
    put_dec_mode(out, 2027, modes.is_set(2027, false));
    if modes.alternate {
        // The grid that was published is the active screen's, so the screen has
        // to be selected before it is painted. The rest of the modes follow the
        // paint; this one cannot.
        out.extend_from_slice(b"\x1b[?1049h");
    }
}

/// Append every carried mode, the kitty keyboard flags and the title.
///
/// Runs after the paint, which is what restores the modes the prologue forced.
pub fn put_modes(modes: &PaneModes, out: &mut Vec<u8>) {
    for mode in &modes.modes {
        if mode.ansi {
            put_csi(
                out,
                u32::from(mode.number),
                if mode.on { b'h' } else { b'l' },
            );
        } else {
            put_dec_mode(out, mode.number, mode.on);
        }
    }
    out.extend_from_slice(b"\x1b[=");
    put_number(out, u32::from(modes.kitty_flags));
    out.extend_from_slice(b";1u");
    if let Some(title) = &modes.title {
        // OSC 2 with a string terminator rather than BEL: the title is
        // arbitrary text and BEL would ring on a terminal that mis-parses it.
        out.extend_from_slice(b"\x1b]2;");
        out.extend_from_slice(title.as_bytes());
        out.extend_from_slice(b"\x1b\\");
    }
}

/// Turn packed history rows back into the bytes a terminal would have printed.
///
/// Soft-wrapped rows are joined into the logical line they came from and the
/// line is terminated once, so the replay re-wraps at the pane's *current*
/// width instead of at yesterday's — the same rule restore's sidecar replay
/// follows (D-M1-6), for the same reason.
pub fn put_rows_replay(packed: &[u8], out: &mut Vec<u8>) {
    let mut wrapped = false;
    for row in PackedRows::new(packed) {
        let Ok(row) = row else { break };
        out.extend_from_slice(row.text);
        wrapped = row.wrapped;
        if !wrapped {
            out.extend_from_slice(b"\r\n");
        }
    }
    if wrapped {
        out.extend_from_slice(b"\r\n");
    }
}
