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

/// A rasterized screen that also remembers what each cell was painted with.
///
/// The value is `(character, SGR parameters in force)`, where the parameters
/// are the accumulated `CSI … m` state as the client wrote it — `"1;4;38;2;…"`
/// — and the empty string is "no styling". Deliberately the *client's* spelling
/// rather than a resolved colour: what M3's exit criterion asks (§7 step 4) is
/// that the two screens either side of a live upgrade are painted identically,
/// and comparing what the client emitted answers that without this rig growing
/// a second opinion about what a colour is.
pub type StyledScreen = BTreeMap<(u16, u16), (char, String)>;

/// Replay `bytes` into the screen they leave behind.
#[must_use]
pub fn rasterize(bytes: &[u8]) -> Screen {
    rasterize_styled(bytes)
        .into_iter()
        .map(|(at, (c, _))| (at, c))
        .collect()
}

/// Replay `bytes` into the screen they leave behind, styles and all.
///
/// The same parse as [`rasterize`], which delegates here: SGR is no longer
/// discarded but accumulated into the state each printed cell is stamped with.
/// `CSI 0 m` (and a bare `CSI m`) reset it; anything else appends, because that
/// is what SGR does and because the grid synthesizer W04 wrote emits a pen as
/// however many sequences it takes.
#[must_use]
pub fn rasterize_styled(bytes: &[u8]) -> StyledScreen {
    let text = String::from_utf8_lossy(bytes);
    let mut cells = StyledScreen::new();
    let mut row = 0_u16;
    let mut col = 0_u16;
    let mut pen = String::new();
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
                if terminator == 'm' {
                    apply_sgr(&mut pen, &params);
                }
                apply_csi(&mut cells, &mut row, &mut col, &params, terminator);
            }
            '\r' => col = 0,
            '\n' => row = row.saturating_add(1),
            _ => {
                cells.insert((row, col), (c, pen.clone()));
                col = col.saturating_add(1);
            }
        }
    }
    cells
}

/// Fold one SGR sequence into the pen in force.
fn apply_sgr(pen: &mut String, params: &str) {
    // `CSI m` and `CSI 0 m` are the same sequence: everything back to default.
    if params.is_empty() || params.split(';').all(|p| p == "0" || p.is_empty()) {
        pen.clear();
        return;
    }
    if !pen.is_empty() {
        pen.push(';');
    }
    pen.push_str(params);
}

/// Apply one CSI sequence to the screen state.
fn apply_csi(
    cells: &mut StyledScreen,
    row: &mut u16,
    col: &mut u16,
    params: &str,
    terminator: char,
) {
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
        // End of input inside a sequence: the stream is append-only and reads
        // chunk at arbitrary boundaries (darwin ptys split mid-CSI where Linux
        // happened not to), so a trailing partial sequence is normal — the
        // next accumulation completes it. Dropping it here parses the same
        // bytes the completed stream will re-parse in full.
        '\0' => {}
        other => panic!("the rasterizer met an unknown CSI terminator {other:?} ({params:?})"),
    }
}

/// Whether the screen `bytes` paint shows `text` somewhere.
#[must_use]
pub fn shows(bytes: &[u8], text: &str) -> bool {
    render(&rasterize(bytes)).contains(text)
}

/// The cells of `screen` on the row holding `needle`, from where it starts.
///
/// What a "compare the sentinel cell for cell" assertion needs and nothing
/// else: the characters and the pen each was painted with, in column order,
/// for `len` columns from the first cell of the first row that spells `needle`.
/// `None` when the screen does not hold it at all.
#[must_use]
pub fn styled_run(screen: &StyledScreen, needle: &str, len: usize) -> Option<Vec<(char, String)>> {
    let rows = screen.keys().map(|&(r, _)| r).max().map_or(0, |r| r + 1);
    let cols = screen.keys().map(|&(_, c)| c).max().map_or(0, |c| c + 1);
    let wanted: Vec<char> = needle.chars().collect();
    for r in 0..rows {
        let line: Vec<char> = (0..cols)
            .map(|c| screen.get(&(r, c)).map_or(' ', |(ch, _)| *ch))
            .collect();
        for start in 0..line.len().saturating_sub(wanted.len().saturating_sub(1)) {
            if line[start..start + wanted.len()] == wanted[..] {
                let from = u16::try_from(start).unwrap_or(u16::MAX);
                return Some(
                    (0..len)
                        .map(|n| {
                            let c = from.saturating_add(u16::try_from(n).unwrap_or(0));
                            screen.get(&(r, c)).cloned().unwrap_or((' ', String::new()))
                        })
                        .collect(),
                );
            }
        }
    }
    None
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
mod partial_tail {
    use super::{rasterize, render};

    /// Reads chunk at arbitrary boundaries; a buffer ending mid-CSI must
    /// parse as far as the complete prefix and never panic.
    #[test]
    fn a_trailing_partial_sequence_is_tolerated_not_fatal() {
        let cells = rasterize(b"\x1b[1;1Hok\x1b[3;");
        assert!(render(&cells).contains("ok"));
    }
}
