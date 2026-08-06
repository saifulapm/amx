//! Safe wrapper over libghostty-vt.
//!
//! The terminal core is an FFI object with paged scrollback, modes and dirty
//! flags. 04 §3 states the ownership rule this crate exists to enforce: "the
//! parser I/O thread **exclusively owns the libghostty-vt instance**; VT state
//! is never shared or swapped … At frame boundaries the parser copies damaged
//! visible rows + cursor into a derived, double-buffered POD cell snapshot
//! published lock-free for render."
//!
//! Two properties carry that rule into the type system.
//!
//! 1. [`Terminal`] is [`Send`] and **not** [`Sync`]. Ownership moves onto the
//!    parser thread once; a shared reference can never leave it. The exclusive
//!    ownership rule is checked by the compiler rather than by review.
//! 2. Readers never see the terminal. They see [`Snapshot`]s: plain data,
//!    copied out of the render state, published by an `Arc` swap. A reader
//!    holding one cannot observe the grid changing underneath it and cannot
//!    contend with the parser for it.
//!
//! The shape of a frame:
//!
//! ```no_run
//! use amx_vt::{Effects, RenderState, Snapshots, Terminal, TerminalOptions};
//!
//! # fn main() -> Result<(), amx_vt::Error> {
//! let mut terminal = Terminal::new(TerminalOptions::default())?;
//! let mut render = RenderState::new()?;
//! let mut snapshots = Snapshots::new(80, 24);
//! let mut effects = Effects::new();
//!
//! // Whatever came off the pty. Query replies land in `effects` in order,
//! // produced by callbacks that run inside this call and do nothing but push.
//! effects.clear();
//! terminal.write(b"hello\x1b[c", &mut effects);
//! let reply = effects.replies().to_vec();
//!
//! // At a frame boundary: read the terminal once, then copy the damaged rows
//! // into the snapshot readers will take.
//! render.update(&mut terminal)?;
//! snapshots.publish(&mut render)?;
//! let frame = snapshots.latest();
//! assert!(!frame.damage().is_empty());
//! # let _ = reply;
//! # Ok(())
//! # }
//! ```

pub mod callbacks;
pub mod enums;
pub mod error;
pub mod history;
pub mod key;
pub mod render;
pub mod snapshot;
pub mod sys;
pub mod terminal;

pub use callbacks::{ClipboardContent, DeviceAttributes, Effect, Effects, QueryAnswers};
pub use enums::{
    CellContent, CellWide, ClipboardLocation, ClipboardWriteResult, ColorScheme, CursorStyle,
    Dirty, PointSpace, RowSemanticPrompt, Screen, StyleColor, Underline,
};
pub use error::{Error, Result, UnknownVariant};
pub use history::{Point, RowRead, Scroll, Scrollbar, TrackedRow};
pub use key::{Key, KeyAction, KeyEncoder, KeyEvent, KittyKeyFlags, Mods};
pub use render::{Colors, Cursor, RenderState, Rgb, Style};
pub use snapshot::{Cell, Row, Snapshot, SnapshotRef, Snapshots, TextRef};
pub use terminal::{CursorPosition, Mode, Terminal, TerminalOptions};
