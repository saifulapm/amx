//! Effect callbacks: trampolines, userdata routing, and the effect buffer.
//!
//! `include/ghostty/vt/terminal.h:77` is the rule this module is built around:
//!
//! > All callbacks are invoked synchronously during
//! > `ghostty_terminal_vt_write()`. Callbacks **must not** call
//! > `ghostty_terminal_vt_write()` on the same terminal (no reentrancy). And
//! > callbacks must be very careful to not block for too long or perform
//! > expensive operations, since they are blocking further IO processing.
//!
//! So every trampoline body here is a push onto a caller-supplied buffer and
//! nothing else. Nothing calls back into the terminal, nothing locks, nothing
//! allocates on the reply path once the buffer has warmed up.
//!
//! Two shapes of callback exist and they are not interchangeable:
//!
//! - **Push** callbacks (`WRITE_PTY`, `BELL`, `TITLE_CHANGED`, `PWD_CHANGED`,
//!   `CLIPBOARD_WRITE`) report something that happened. They land in
//!   [`Effects`].
//! - **Answer** callbacks (`DEVICE_ATTRIBUTES`, `XTVERSION`, `ENQUIRY`,
//!   `COLOR_SCHEME`) are asked a question and return the answer, which
//!   libghostty-vt then formats and emits through `WRITE_PTY`. Their answers
//!   come from [`QueryAnswers`], fixed on the terminal, because a callback
//!   cannot go and ask anyone.
//!
//! That second shape is why `device_attributes_query_reply_is_captured_in_order`
//! is a meaningful test: the DA reply is produced inside the same `vt_write`
//! that parsed the query, so it takes its place in the reply byte stream at
//! exactly the point the application will expect it.

use std::cell::Cell;
use std::ffi::c_void;
use std::slice;

use crate::enums::{ClipboardLocation, ClipboardWriteResult, ColorScheme};
use crate::sys;

/// Everything one `vt_write` produced, in order.
///
/// The caller owns this and passes it in, which is what makes the ordering
/// guarantee composable: the PTY actor holds one per pane, hands it to
/// [`Terminal::write`](crate::Terminal::write), drains it, and hands the same
/// buffer back next time. Reusing it keeps the reply path allocation-free.
#[derive(Debug, Default)]
pub struct Effects {
    replies: Vec<u8>,
    events: Vec<TerminalEvent>,
}

impl Effects {
    /// An empty buffer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes the terminal wants written back to the pty, concatenated in the
    /// order it produced them.
    ///
    /// This is the whole ordering contract: an out-of-band reply cannot
    /// overtake an in-band one because both are appended here, by the same
    /// thread, inside the same `vt_write`.
    #[must_use]
    pub fn replies(&self) -> &[u8] {
        &self.replies
    }

    /// Side effects that are not pty writes, in the order they occurred.
    #[must_use]
    pub fn events(&self) -> &[TerminalEvent] {
        &self.events
    }

    /// Whether the terminal produced nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.replies.is_empty() && self.events.is_empty()
    }

    /// Drop the contents, keeping the capacity for the next write.
    pub fn clear(&mut self) {
        self.replies.clear();
        self.events.clear();
    }
}

/// A side effect a VT sequence asked for that is not a pty write.
///
/// Named for what it *is* — something the terminal reported — rather than for
/// the `Effect` it used to be called. That name was one of three unrelated
/// enums spelling it (`amx_core::Effect` is render dirtiness, the agent
/// tracker's is a status transition), and three types with one name in one
/// workspace is a reading hazard rather than a coincidence (DR-10).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TerminalEvent {
    /// A BEL (0x07) was received.
    Bell,
    /// OSC 0 or OSC 2 changed the title; read it with
    /// [`Terminal::title`](crate::Terminal::title).
    TitleChanged,
    /// OSC 7, OSC 9 or OSC 1337 changed the working directory; read it with
    /// [`Terminal::pwd`](crate::Terminal::pwd).
    PwdChanged,
    /// OSC 52 (or iTerm2 OSC 1337 Copy) asked for a clipboard write.
    ///
    /// An empty `contents` is a request to clear the destination, which is
    /// distinct from one entry whose data is empty.
    ClipboardWrite {
        /// Which clipboard.
        location: ClipboardLocation,
        /// One entry per MIME representation of the same logical value; they
        /// are to be committed atomically.
        contents: Vec<ClipboardContent>,
    },
}

/// One MIME representation inside a clipboard write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardContent {
    /// The MIME type.
    pub mime: Vec<u8>,
    /// The decoded, binary-safe payload. Protocol-level base64 is already gone.
    pub data: Vec<u8>,
}

/// What the terminal answers when an application asks it who it is.
///
/// These are fixed at terminal construction because the callbacks that serve
/// them run inside `vt_write` and cannot consult anything that might block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryAnswers {
    /// The DA1/DA2/DA3 response.
    pub device_attributes: DeviceAttributes,
    /// The XTVERSION (`CSI > q`) string. Empty reports libghostty's default.
    pub xtversion: Vec<u8>,
    /// The ENQ (0x05) answerback. Empty sends nothing.
    pub enquiry: Vec<u8>,
    /// The answer to `CSI ? 996 n`. `None` ignores the query.
    pub color_scheme: Option<ColorScheme>,
}

/// The device attributes reported for `CSI c`, `CSI > c` and `CSI = c`.
///
/// The numbers are named from `include/ghostty/vt/device.h`, never spelled out:
/// `GHOSTTY_DA_CONFORMANCE_VT220` and friends are what the header calls them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAttributes {
    /// DA1 conformance level (the `Pp` parameter).
    pub conformance_level: u16,
    /// DA1 feature codes. At most 64 are sent; the rest are dropped.
    pub features: Vec<u16>,
    /// DA2 terminal type.
    pub device_type: u16,
    /// DA2 firmware version.
    pub firmware_version: u16,
    /// DA2 ROM cartridge number, always zero for an emulator.
    pub rom_cartridge: u16,
    /// DA3 unit id, sent as eight uppercase hex digits.
    pub unit_id: u32,
}

impl Default for DeviceAttributes {
    /// A VT220 that speaks ANSI colour — what a modern application expects and
    /// the least amx can claim without lying.
    fn default() -> Self {
        Self {
            conformance_level: sys::GHOSTTY_DA_CONFORMANCE_VT220 as u16,
            features: vec![sys::GHOSTTY_DA_FEATURE_ANSI_COLOR as u16],
            device_type: sys::GHOSTTY_DA_DEVICE_TYPE_VT220 as u16,
            firmware_version: 0,
            rom_cartridge: 0,
            unit_id: 0,
        }
    }
}

/// The userdata every callback receives, owned by the terminal.
///
/// Boxed by the terminal so its address is stable for the terminal's whole
/// life, which is the only requirement `GHOSTTY_TERMINAL_OPT_USERDATA` makes.
#[derive(Debug)]
pub(crate) struct EffectState {
    /// The buffer the current `vt_write` is filling — null outside one.
    ///
    /// A `Cell` rather than a `&mut` field because the callback reaches this
    /// state through a raw pointer while the writer still holds the terminal:
    /// interior mutability is what keeps the two from ever being aliasing
    /// `&mut`s.
    sink: Cell<*mut Effects>,
    answers: QueryAnswers,
}

impl EffectState {
    pub(crate) fn new(answers: QueryAnswers) -> Box<Self> {
        Box::new(Self {
            sink: Cell::new(std::ptr::null_mut()),
            answers,
        })
    }

    pub(crate) fn answers(&self) -> &QueryAnswers {
        &self.answers
    }

    pub(crate) fn set_answers(&mut self, answers: QueryAnswers) {
        self.answers = answers;
    }

    /// Point the callbacks at `effects` until the returned guard drops.
    pub(crate) fn bind<'a>(&'a self, effects: &'a mut Effects) -> Bound<'a> {
        self.sink.set(std::ptr::from_mut(effects));
        Bound { state: self }
    }
}

/// Unbinds the sink on drop, including on an unwind out of `vt_write`.
#[derive(Debug)]
pub(crate) struct Bound<'a> {
    state: &'a EffectState,
}

impl Drop for Bound<'_> {
    fn drop(&mut self) {
        self.state.sink.set(std::ptr::null_mut());
    }
}

/// Run `body` against the effect buffer of the write in progress.
///
/// # Safety
///
/// `userdata` must be the pointer the terminal was given, or null.
unsafe fn with_effects(userdata: *mut c_void, body: impl FnOnce(&mut Effects)) {
    // SAFETY: the caller guarantees the provenance; libghostty-vt passes back
    // exactly the pointer set with GHOSTTY_TERMINAL_OPT_USERDATA.
    let Some(state) = (unsafe { userdata.cast::<EffectState>().as_ref() }) else {
        return;
    };
    // SAFETY: the sink is non-null only between `bind` and the guard's drop,
    // i.e. only for the duration of a `vt_write` on the owning thread, and the
    // terminal is `!Sync` so no other thread can be inside that window.
    let Some(effects) = (unsafe { state.sink.get().as_mut() }) else {
        return;
    };
    body(effects);
}

/// Read the fixed answers the terminal was configured with.
///
/// # Safety
///
/// `userdata` must be the pointer the terminal was given, or null.
unsafe fn with_answers<R>(
    userdata: *mut c_void,
    body: impl FnOnce(&QueryAnswers) -> R,
) -> Option<R> {
    // SAFETY: as `with_effects`.
    let state = unsafe { userdata.cast::<EffectState>().as_ref() }?;
    Some(body(state.answers()))
}

unsafe extern "C" fn write_pty(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    // SAFETY: the userdata is ours; `data`/`len` describe a slice the header
    // documents as valid for the duration of the call, and it is only copied.
    unsafe {
        with_effects(userdata, |effects| {
            if data.is_null() || len == 0 {
                return;
            }
            effects
                .replies
                .extend_from_slice(slice::from_raw_parts(data, len));
        });
    }
}

unsafe extern "C" fn bell(_terminal: sys::GhosttyTerminal, userdata: *mut c_void) {
    // SAFETY: the userdata is ours.
    unsafe { with_effects(userdata, |effects| effects.events.push(TerminalEvent::Bell)) };
}

unsafe extern "C" fn title_changed(_terminal: sys::GhosttyTerminal, userdata: *mut c_void) {
    // SAFETY: the userdata is ours. The new title is *not* read here: doing so
    // would be a call back into the terminal from inside a callback.
    unsafe {
        with_effects(userdata, |effects| {
            effects.events.push(TerminalEvent::TitleChanged)
        })
    };
}

unsafe extern "C" fn pwd_changed(_terminal: sys::GhosttyTerminal, userdata: *mut c_void) {
    // SAFETY: the userdata is ours.
    unsafe {
        with_effects(userdata, |effects| {
            effects.events.push(TerminalEvent::PwdChanged)
        })
    };
}

unsafe extern "C" fn clipboard_write(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    write: *const sys::GhosttyClipboardWrite,
) -> sys::GhosttyClipboardWriteResult::Type {
    // SAFETY: the userdata is ours; `write` and everything it points at are
    // documented as borrowed for the duration of the call, and are copied.
    unsafe {
        with_effects(userdata, |effects| {
            let Some(write) = write.as_ref() else {
                return;
            };
            let Some(location) = ClipboardLocation::from_sys(write.location) else {
                return;
            };
            let entries = if write.contents.is_null() {
                &[][..]
            } else {
                slice::from_raw_parts(write.contents, write.contents_len)
            };
            let contents = entries
                .iter()
                .map(|entry| ClipboardContent {
                    mime: borrowed(entry.mime).to_vec(),
                    data: borrowed(entry.data).to_vec(),
                })
                .collect();
            effects
                .events
                .push(TerminalEvent::ClipboardWrite { location, contents });
        });
    }
    ClipboardWriteResult::Success.to_sys()
}

unsafe extern "C" fn device_attributes(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    out: *mut sys::GhosttyDeviceAttributes,
) -> bool {
    // SAFETY: the userdata is ours and `out` is the live out-parameter the
    // header documents.
    unsafe {
        let Some(out) = out.as_mut() else {
            return false;
        };
        with_answers(userdata, |answers| {
            let attributes = &answers.device_attributes;
            out.primary.conformance_level = attributes.conformance_level;
            let features =
                &attributes.features[..attributes.features.len().min(out.primary.features.len())];
            out.primary.features[..features.len()].copy_from_slice(features);
            out.primary.num_features = features.len();
            out.secondary.device_type = attributes.device_type;
            out.secondary.firmware_version = attributes.firmware_version;
            out.secondary.rom_cartridge = attributes.rom_cartridge;
            out.tertiary.unit_id = attributes.unit_id;
            true
        })
        .unwrap_or(false)
    }
}

unsafe extern "C" fn xtversion(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
) -> sys::GhosttyString {
    // SAFETY: the userdata is ours. The returned string borrows the answers,
    // which outlive the call because the terminal owns them.
    unsafe { with_answers(userdata, |answers| lend(&answers.xtversion)) }.unwrap_or(EMPTY_STRING)
}

unsafe extern "C" fn enquiry(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
) -> sys::GhosttyString {
    // SAFETY: as `xtversion`.
    unsafe { with_answers(userdata, |answers| lend(&answers.enquiry)) }.unwrap_or(EMPTY_STRING)
}

unsafe extern "C" fn color_scheme(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    out: *mut sys::GhosttyColorScheme::Type,
) -> bool {
    // SAFETY: the userdata is ours and `out` is the live out-parameter.
    unsafe {
        let Some(out) = out.as_mut() else {
            return false;
        };
        with_answers(userdata, |answers| {
            answers.color_scheme.is_some_and(|scheme| {
                *out = scheme.to_sys();
                true
            })
        })
        .unwrap_or(false)
    }
}

const EMPTY_STRING: sys::GhosttyString = sys::GhosttyString {
    ptr: std::ptr::null(),
    len: 0,
};

fn lend(bytes: &[u8]) -> sys::GhosttyString {
    sys::GhosttyString {
        ptr: bytes.as_ptr(),
        len: bytes.len(),
    }
}

/// # Safety
///
/// `string` must describe a slice valid for the duration of the callback.
unsafe fn borrowed<'a>(string: sys::GhosttyString) -> &'a [u8] {
    if string.ptr.is_null() || string.len == 0 {
        return &[];
    }
    // SAFETY: the caller guarantees the slice is live; the header documents
    // clipboard strings as borrowed for the callback's duration.
    unsafe { slice::from_raw_parts(string.ptr, string.len) }
}

/// The effect options, paired with the trampoline each one takes.
///
/// Installed together at construction: an effect that is not registered is
/// silently ignored by the terminal, so the list *is* the set of sequences amx
/// reacts to.
pub(crate) fn installers() -> [(sys::GhosttyTerminalOption::Type, *const c_void); 9] {
    [
        (
            sys::GhosttyTerminalOption::GHOSTTY_TERMINAL_OPT_WRITE_PTY,
            write_pty as *const (),
        ),
        (
            sys::GhosttyTerminalOption::GHOSTTY_TERMINAL_OPT_BELL,
            bell as *const (),
        ),
        (
            sys::GhosttyTerminalOption::GHOSTTY_TERMINAL_OPT_TITLE_CHANGED,
            title_changed as *const (),
        ),
        (
            sys::GhosttyTerminalOption::GHOSTTY_TERMINAL_OPT_PWD_CHANGED,
            pwd_changed as *const (),
        ),
        (
            sys::GhosttyTerminalOption::GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE,
            clipboard_write as *const (),
        ),
        (
            sys::GhosttyTerminalOption::GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES,
            device_attributes as *const (),
        ),
        (
            sys::GhosttyTerminalOption::GHOSTTY_TERMINAL_OPT_XTVERSION,
            xtversion as *const (),
        ),
        (
            sys::GhosttyTerminalOption::GHOSTTY_TERMINAL_OPT_ENQUIRY,
            enquiry as *const (),
        ),
        (
            sys::GhosttyTerminalOption::GHOSTTY_TERMINAL_OPT_COLOR_SCHEME,
            color_scheme as *const (),
        ),
    ]
    .map(|(option, function)| (option, function.cast::<c_void>()))
}
