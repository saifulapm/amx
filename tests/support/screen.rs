//! A rasterizer for what a client painted.
//!
//! Two clients attached at the same size should end up on the same screen no
//! matter how their byte streams were chunked or in what order frames landed.
//! Comparing raw bytes can't say that; replaying the bytes into a cell grid
//! can. This is deliberately only the vocabulary the client's `FrameWriter`
//! emits — cursor addressing, erase, SGR, the alt-screen switch — so an
//! unknown escape is a loud failure, not a silent misrender.

use std::collections::BTreeMap;

/// A rasterized screen: populated cells by `(row, col)`, zero-based.
pub type Screen = BTreeMap<(u16, u16), char>;

/// Replay `bytes` into the screen they leave behind.
#[must_use]
pub fn rasterize(bytes: &[u8]) -> Screen {
    let text = String::from_utf8_lossy(bytes);
    let mut cells = Screen::new();
    let mut row = 0_u16;
    let mut col = 0_u16;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                if chars.peek() != Some(&'[') {
                    // A bare escape the client never writes; skip its follower.
                    let _ = chars.next();
                    continue;
                }
                let _ = chars.next();
                let mut params = String::new();
                let mut terminator = '\0';
                for c2 in chars.by_ref() {
                    if c2.is_ascii_digit() || c2 == ';' || c2 == '?' {
                        params.push(c2);
                    } else {
                        terminator = c2;
                        break;
                    }
                }
                if terminator == '\0' && chars.peek().is_none() {
                    // The capture ended inside this sequence: a wait polls the
                    // stream at arbitrary byte boundaries, so a frame's tail
                    // may not have arrived yet. The screen holds at the last
                    // whole sequence; the next poll reparses the completed
                    // tail. Only end-of-input is tolerated — an unknown
                    // terminator mid-stream still panics below.
                    break;
                }
                apply_csi(&mut cells, &mut row, &mut col, &params, terminator);
            }
            '\r' => col = 0,
            '\n' => row = row.saturating_add(1),
            _ => {
                cells.insert((row, col), c);
                col = col.saturating_add(1);
            }
        }
    }
    cells
}

/// Apply one CSI sequence to the screen state.
fn apply_csi(cells: &mut Screen, row: &mut u16, col: &mut u16, params: &str, terminator: char) {
    match terminator {
        'H' => {
            let mut parts = params.split(';');
            let r: u16 = parts.next().unwrap_or("1").parse().unwrap_or(1);
            let c: u16 = parts.next().unwrap_or("1").parse().unwrap_or(1);
            *row = r.saturating_sub(1);
            *col = c.saturating_sub(1);
        }
        'J' => {
            // The client only ever clears the whole screen.
            cells.clear();
        }
        'K' => {
            // Erase to end of line.
            let clear: Vec<(u16, u16)> = cells
                .range((*row, *col)..(*row, u16::MAX))
                .map(|(&key, _)| key)
                .collect();
            for key in clear {
                cells.remove(&key);
            }
        }
        // SGR, cursor visibility, alt screen, kitty keyboard: no cell effect.
        'm' | 'h' | 'l' | 'u' => {}
        other => panic!("the rasterizer met an unknown CSI terminator {other:?} ({params:?})"),
    }
}

/// Whether the screen `bytes` paint shows `text` somewhere.
#[must_use]
pub fn shows(bytes: &[u8], text: &str) -> bool {
    render(&rasterize(bytes)).contains(text)
}

/// Render a screen as text, one line per row, for failure messages.
#[must_use]
pub fn render(screen: &Screen) -> String {
    let rows = screen.keys().map(|&(r, _)| r).max().map_or(0, |r| r + 1);
    let cols = screen.keys().map(|&(_, c)| c).max().map_or(0, |c| c + 1);
    let mut out = String::new();
    for r in 0..rows {
        for c in 0..cols {
            out.push(screen.get(&(r, c)).copied().unwrap_or(' '));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::rasterize;

    #[test]
    fn a_capture_ending_mid_sequence_keeps_the_last_whole_frame() {
        let screen = rasterize(b"\x1b[1;1Hab\x1b[3;");
        assert_eq!(screen.get(&(0, 0)), Some(&'a'));
        assert_eq!(screen.get(&(0, 1)), Some(&'b'));
    }

    #[test]
    #[should_panic(expected = "unknown CSI terminator")]
    fn an_unknown_terminator_mid_stream_is_still_loud() {
        let _ = rasterize(b"\x1b[3;\0after");
    }
}
