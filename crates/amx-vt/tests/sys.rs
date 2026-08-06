#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
//! The generated FFI surface, exercised end to end.
//!
//! These tests drive `amx_vt::sys` directly because that is all T02 ships: they
//! prove the vendored library links, that the bindings name the symbols the
//! wrapper (T06) is going to build on, and that the vendored patch behaves the
//! way `vendor/libghostty-vt.patches.md` claims.

use amx_vt::sys;

/// DEC private mode 2027, grapheme clustering.
///
/// `GHOSTTY_MODE_GRAPHEME_CLUSTER` is a macro over the `ghostty_mode_new`
/// static inline, so bindgen emits neither; `include/ghostty/vt/modes.h:116`
/// defines the encoding as `(value & 0x7FFF) | (ansi << 15)`, which for the
/// DEC-private mode 2027 is 2027.
const MODE_GRAPHEME_CLUSTER: sys::GhosttyMode = 2027;

/// A terminal handle that frees itself, so a failing assert cannot leak.
struct Terminal(sys::GhosttyTerminal);

impl Drop for Terminal {
    fn drop(&mut self) {
        // SAFETY: self.0 came from a successful ghostty_terminal_new and is
        // freed exactly once, here.
        unsafe { sys::ghostty_terminal_free(self.0) };
    }
}

impl Terminal {
    fn new(cols: u16, rows: u16) -> Self {
        let mut terminal: sys::GhosttyTerminal = std::ptr::null_mut();
        let options = sys::GhosttyTerminalOptions {
            cols,
            rows,
            max_scrollback: 0,
        };
        // SAFETY: `terminal` is a live out-pointer and the default allocator is
        // requested with NULL, as the header documents.
        let result = unsafe { sys::ghostty_terminal_new(std::ptr::null(), &mut terminal, options) };
        assert_eq!(result, sys::GhosttyResult::GHOSTTY_SUCCESS);
        assert!(!terminal.is_null());
        Self(terminal)
    }

    fn write(&self, bytes: &str) {
        // SAFETY: the slice outlives the call, which is all vt_write requires;
        // no callback is installed, so there is no reentrancy.
        unsafe { sys::ghostty_terminal_vt_write(self.0, bytes.as_ptr().cast(), bytes.len()) };
    }

    fn mode(&self, mode: sys::GhosttyMode) -> bool {
        let mut value = false;
        // SAFETY: `value` is a live out-pointer for the documented bool.
        let result = unsafe { sys::ghostty_terminal_mode_get(self.0, mode, &mut value) };
        assert_eq!(result, sys::GhosttyResult::GHOSTTY_SUCCESS);
        value
    }

    /// The first `count` cells of the top viewport row, one string per cell.
    ///
    /// A cell that continues a wide character or a cluster reads as empty,
    /// which is what makes the grapheme assertions below meaningful.
    fn first_row(&self, count: usize) -> Vec<String> {
        // SAFETY: every handle below is created here, used while alive, and
        // freed on the way out; the out-pointers match the types the header
        // documents for each data kind.
        unsafe {
            let mut state: sys::GhosttyRenderState = std::ptr::null_mut();
            assert_eq!(
                sys::ghostty_render_state_new(std::ptr::null(), &mut state),
                sys::GhosttyResult::GHOSTTY_SUCCESS
            );
            assert_eq!(
                sys::ghostty_render_state_update(state, self.0),
                sys::GhosttyResult::GHOSTTY_SUCCESS
            );

            let mut rows: sys::GhosttyRenderStateRowIterator = std::ptr::null_mut();
            assert_eq!(
                sys::ghostty_render_state_row_iterator_new(std::ptr::null(), &mut rows),
                sys::GhosttyResult::GHOSTTY_SUCCESS
            );
            assert_eq!(
                sys::ghostty_render_state_get(
                    state,
                    sys::GhosttyRenderStateData::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                    (&raw mut rows).cast(),
                ),
                sys::GhosttyResult::GHOSTTY_SUCCESS
            );
            assert!(sys::ghostty_render_state_row_iterator_next(rows));

            let mut cells: sys::GhosttyRenderStateRowCells = std::ptr::null_mut();
            assert_eq!(
                sys::ghostty_render_state_row_cells_new(std::ptr::null(), &mut cells),
                sys::GhosttyResult::GHOSTTY_SUCCESS
            );
            assert_eq!(
                sys::ghostty_render_state_row_get(
                    rows,
                    sys::GhosttyRenderStateRowData::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                    (&raw mut cells).cast(),
                ),
                sys::GhosttyResult::GHOSTTY_SUCCESS
            );

            let mut out = Vec::with_capacity(count);
            while out.len() < count && sys::ghostty_render_state_row_cells_next(cells) {
                let mut bytes = [0u8; 64];
                let mut buffer = sys::GhosttyBuffer {
                    ptr: bytes.as_mut_ptr(),
                    cap: bytes.len(),
                    len: 0,
                };
                assert_eq!(
                    sys::ghostty_render_state_row_cells_get(
                        cells,
                        sys::GhosttyRenderStateRowCellsData::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
                        (&raw mut buffer).cast(),
                    ),
                    sys::GhosttyResult::GHOSTTY_SUCCESS
                );
                out.push(String::from_utf8(bytes[..buffer.len].to_vec()).expect("cells are utf8"));
            }

            sys::ghostty_render_state_row_cells_free(cells);
            sys::ghostty_render_state_row_iterator_free(rows);
            sys::ghostty_render_state_free(state);
            out
        }
    }
}

/// The allowlisted surfaces are all present and callable.
///
/// Taking the addresses is enough: a missing symbol is a link error and a
/// renamed one is a compile error, which is the whole point of generating the
/// bindings from the vendored headers on every build.
#[test]
fn bindings_expose_terminal_render_and_key_symbols() {
    let terminal: [*const (); 6] = [
        sys::ghostty_terminal_new as *const (),
        sys::ghostty_terminal_free as *const (),
        sys::ghostty_terminal_reset as *const (),
        sys::ghostty_terminal_resize as *const (),
        sys::ghostty_terminal_vt_write as *const (),
        sys::ghostty_terminal_mode_set as *const (),
    ];
    let render: [*const (); 4] = [
        sys::ghostty_render_state_new as *const (),
        sys::ghostty_render_state_begin_update as *const (),
        sys::ghostty_render_state_end_update as *const (),
        sys::ghostty_render_state_row_cells_get as *const (),
    ];
    let key: [*const (); 3] = [
        sys::ghostty_key_encoder_new as *const (),
        sys::ghostty_key_encoder_setopt as *const (),
        sys::ghostty_key_event_new as *const (),
    ];
    let osc_and_grid: [*const (); 3] = [
        sys::ghostty_osc_new as *const (),
        sys::ghostty_osc_next as *const (),
        sys::ghostty_grid_ref_cell as *const (),
    ];
    for symbol in terminal
        .iter()
        .chain(&render)
        .chain(&key)
        .chain(&osc_and_grid)
    {
        assert!(!symbol.is_null());
    }

    // Enum constants arrive as values, not as a Rust enum: the wrapper maps
    // them through an exhaustive match, so a renumbering has to break there.
    assert_eq!(sys::GhosttyResult::GHOSTTY_SUCCESS, 0);
    assert_ne!(
        sys::GhosttyResult::GHOSTTY_INVALID_VALUE,
        sys::GhosttyResult::GHOSTTY_SUCCESS
    );
}

#[test]
fn vt_write_leaves_the_written_text_in_the_grid() {
    let terminal = Terminal::new(10, 3);
    terminal.write("hi");

    assert_eq!(terminal.first_row(3), ["h", "i", ""]);
}

#[test]
fn resize_and_reset_run_over_the_ffi_boundary() {
    let terminal = Terminal::new(10, 3);
    terminal.write("hi");
    // SAFETY: the terminal is live; the cell pixel dimensions only feed the
    // size reports we do not use here.
    let result = unsafe { sys::ghostty_terminal_resize(terminal.0, 20, 5, 0, 0) };
    assert_eq!(result, sys::GhosttyResult::GHOSTTY_SUCCESS);
    assert_eq!(terminal.first_row(3), ["h", "i", ""]);

    // SAFETY: the terminal is live.
    unsafe { sys::ghostty_terminal_reset(terminal.0) };
    assert_eq!(terminal.first_row(3), ["", "", ""]);
}

/// R7: clustering is on for every new terminal and a reset does not take it
/// away. Without `0001-grapheme-cluster-default-mode.patch` the first
/// assertion fails outright and the flag lands in two cells.
#[test]
fn grapheme_cluster_mode_is_default_and_survives_full_reset() {
    let flag = "\u{1F1FA}\u{1F1F8}";

    let terminal = Terminal::new(10, 3);
    assert!(
        terminal.mode(MODE_GRAPHEME_CLUSTER),
        "2027 is on at creation"
    );

    terminal.write(flag);
    assert_eq!(terminal.first_row(3), [flag, "", ""]);

    // RIS through the stream restores the terminal's *default* modes, which is
    // exactly what the patch changes.
    terminal.write("\x1bc");
    assert!(terminal.mode(MODE_GRAPHEME_CLUSTER), "2027 survives RIS");

    // A reset and the text that follows it inside one write batch: nothing
    // outside libghostty-vt gets a chance to re-apply the mode in between.
    terminal.write(&format!("\x1bc{flag}"));
    assert_eq!(terminal.first_row(3), [flag, "", ""]);
}

#[test]
fn grapheme_cluster_mode_holds_a_zwj_sequence_in_one_cell() {
    // Woman + ZWJ + laptop: one cluster, and two cells wide.
    let zwj = "\u{1F469}\u{200D}\u{1F4BB}";
    let terminal = Terminal::new(10, 3);
    terminal.write(zwj);

    assert_eq!(terminal.first_row(3), [zwj, "", ""]);
}
