//! The styled visible grid, as a replay.
//!
//! **W04 fills this.** libghostty-vt has no serialization API and the grid is
//! one mutable FFI object that cannot be copied out (06 R4/R5), so D-M3-4 does
//! what herdr's `initial_history_ansi` does with better inputs: synthesize the
//! screen from the published POD snapshot — cells already carry resolved fg/bg
//! and SGR attributes, rows already carry wrap flags — as cursor-addressed SGR
//! runs plus a final cursor position, and replay it into a fresh terminal on
//! the other side.
//!
//! That is what makes "no visible screen content lost" literal rather than
//! approximate: colors, attributes and layout survive, not just text. The
//! fidelity bound that does not close is history, which crosses unstyled —
//! R-M1-1's accepted precedent, inherited rather than rediscovered. R-M3-2 is
//! the risk this module carries: wide characters, spacer cells, wrapped rows
//! and grapheme clustering all have to survive synthesize→replay, and property
//! tests over random styled grids are the defense.
