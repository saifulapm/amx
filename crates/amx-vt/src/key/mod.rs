//! Key encoding: physical keys, modifiers, and the encoder.
//!
//! Key encoding lives on the *server* side of amx even though the client owns
//! input, because the encoding depends on terminal state — DECCKM, keypad
//! mode, `modifyOtherKeys`, and the Kitty keyboard flags the application asked
//! for. `ghostty_key_encoder_setopt_from_terminal` reads all of those off a
//! live terminal in one call (`include/ghostty/vt/key/encoder.h:169`), which is
//! why [`KeyEncoder::sync_from`] exists and why the client never has to guess.
//!
//! The client still forwards raw bytes for anything it does not need to
//! interpret (D-M0-4); this is the path for structured key events, which is
//! what a keybinding replayed by the server produces.
//!
//! [`Key`] lives in this module rather than in [`crate::enums`] because it is
//! 176 variants on its own, and in its own file inside it for the same reason:
//! the module budget is a forcing function.

mod table;

pub use table::Key;

use std::ops::{BitOr, BitOrAssign};

use crate::enums::vt_enum;
use crate::error::{Error, Result, check};
use crate::sys;
use crate::terminal::Terminal;

vt_enum! {
    /// Press, release, or auto-repeat.
    KeyAction: sys::GhosttyKeyAction::Type {
        /// The key came up.
        Release = sys::GhosttyKeyAction::GHOSTTY_KEY_ACTION_RELEASE,
        /// The key went down.
        Press = sys::GhosttyKeyAction::GHOSTTY_KEY_ACTION_PRESS,
        /// The key is held and repeating.
        Repeat = sys::GhosttyKeyAction::GHOSTTY_KEY_ACTION_REPEAT,
    }
}

/// Declare a bitmask newtype over `sys::` flag constants.
macro_rules! vt_flags {
    (
        $(#[$meta:meta])*
        $name:ident: $raw:ty {
            $( $(#[$flag_meta:meta])* $flag:ident = $constant:path ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
        pub struct $name($raw);

        impl $name {
            /// No flags set.
            pub const NONE: Self = Self(0);
            $( $(#[$flag_meta])* pub const $flag: Self = Self($constant as $raw); )+

            /// Every individual flag, in header order.
            pub const ALL: &'static [Self] = &[ $( Self::$flag, )+ ];

            /// The raw bits.
            #[must_use]
            pub const fn bits(self) -> $raw {
                self.0
            }

            /// Wrap raw bits, including any this build does not name.
            #[must_use]
            pub const fn from_bits(bits: $raw) -> Self {
                Self(bits)
            }

            /// Whether every flag in `other` is set here.
            #[must_use]
            pub const fn contains(self, other: Self) -> bool {
                self.0 & other.0 == other.0
            }
        }

        impl BitOr for $name {
            type Output = Self;
            fn bitor(self, other: Self) -> Self {
                Self(self.0 | other.0)
            }
        }

        impl BitOrAssign for $name {
            fn bitor_assign(&mut self, other: Self) {
                self.0 |= other.0;
            }
        }
    };
}

vt_flags! {
    /// Which modifier keys are held.
    ///
    /// The `*_SIDE` bits say which side of a modifier is down and are only
    /// meaningful when the modifier's own bit is set; a platform that cannot
    /// tell left from right simply leaves them clear.
    Mods: sys::GhosttyMods {
        /// Shift.
        SHIFT = sys::GHOSTTY_MODS_SHIFT,
        /// Control.
        CTRL = sys::GHOSTTY_MODS_CTRL,
        /// Alt, or Option.
        ALT = sys::GHOSTTY_MODS_ALT,
        /// Super, Command, or Windows.
        SUPER = sys::GHOSTTY_MODS_SUPER,
        /// Caps Lock is on.
        CAPS_LOCK = sys::GHOSTTY_MODS_CAPS_LOCK,
        /// Num Lock is on.
        NUM_LOCK = sys::GHOSTTY_MODS_NUM_LOCK,
        /// The right Shift, rather than the left.
        SHIFT_RIGHT = sys::GHOSTTY_MODS_SHIFT_SIDE,
        /// The right Control.
        CTRL_RIGHT = sys::GHOSTTY_MODS_CTRL_SIDE,
        /// The right Alt.
        ALT_RIGHT = sys::GHOSTTY_MODS_ALT_SIDE,
        /// The right Super.
        SUPER_RIGHT = sys::GHOSTTY_MODS_SUPER_SIDE,
    }
}

vt_flags! {
    /// The Kitty keyboard protocol modes an application has enabled.
    ///
    /// Read them off a live terminal with
    /// [`Terminal::kitty_keyboard_flags`]; amx never chooses them, the
    /// application does.
    KittyKeyFlags: sys::GhosttyKittyKeyFlags {
        /// Disambiguate escape codes.
        DISAMBIGUATE = sys::GHOSTTY_KITTY_KEY_DISAMBIGUATE,
        /// Report press *and* release.
        REPORT_EVENTS = sys::GHOSTTY_KITTY_KEY_REPORT_EVENTS,
        /// Report alternate key codes.
        REPORT_ALTERNATES = sys::GHOSTTY_KITTY_KEY_REPORT_ALTERNATES,
        /// Report keys the terminal would normally consume.
        REPORT_ALL = sys::GHOSTTY_KITTY_KEY_REPORT_ALL,
        /// Report the text associated with a key.
        REPORT_ASSOCIATED = sys::GHOSTTY_KITTY_KEY_REPORT_ASSOCIATED,
    }
}

/// One key event, ready to encode.
#[derive(Debug)]
pub struct KeyEvent {
    raw: sys::GhosttyKeyEvent,
    /// The event borrows this rather than owning it
    /// (`include/ghostty/vt/key/event.h:438`), so it has to live here.
    text: Vec<u8>,
}

// SAFETY: the event is a plain owned allocation with no thread affinity; the
// library only touches it through the calls made here, which take `&mut self`.
unsafe impl Send for KeyEvent {}

impl KeyEvent {
    /// A press of [`Key::Unidentified`] with no modifiers.
    ///
    /// # Errors
    ///
    /// [`Error::OutOfMemory`] if the library cannot allocate.
    pub fn new() -> Result<Self> {
        let mut raw: sys::GhosttyKeyEvent = std::ptr::null_mut();
        // SAFETY: `raw` is a live out-pointer; NULL selects the default allocator.
        check(unsafe { sys::ghostty_key_event_new(std::ptr::null(), &raw mut raw) })?;
        if raw.is_null() {
            return Err(Error::InvalidValue);
        }
        Ok(Self {
            raw,
            text: Vec::new(),
        })
    }

    /// Set press, release or repeat.
    pub fn set_action(&mut self, action: KeyAction) {
        // SAFETY: the event is live and exclusively ours.
        unsafe { sys::ghostty_key_event_set_action(self.raw, action.to_sys()) };
    }

    /// Set the physical key.
    pub fn set_key(&mut self, key: Key) {
        // SAFETY: the event is live and exclusively ours.
        unsafe { sys::ghostty_key_event_set_key(self.raw, key.to_sys()) };
    }

    /// Set the modifiers held at the time of the event.
    pub fn set_mods(&mut self, mods: Mods) {
        // SAFETY: the event is live and exclusively ours.
        unsafe { sys::ghostty_key_event_set_mods(self.raw, mods.bits()) };
    }

    /// Set the modifiers the platform already consumed producing the text.
    pub fn set_consumed_mods(&mut self, mods: Mods) {
        // SAFETY: the event is live and exclusively ours.
        unsafe { sys::ghostty_key_event_set_consumed_mods(self.raw, mods.bits()) };
    }

    /// Set the unmodified UTF-8 text the key produces on this layout.
    ///
    /// The header is explicit that this must be the text *before* any Ctrl or
    /// Meta transformation, and must not carry C0 controls — for those, leave
    /// it empty and let the encoder work from the logical key.
    pub fn set_text(&mut self, text: &[u8]) {
        self.text.clear();
        self.text.extend_from_slice(text);
        let (ptr, len) = if self.text.is_empty() {
            (std::ptr::null(), 0)
        } else {
            (self.text.as_ptr().cast(), self.text.len())
        };
        // SAFETY: the event is live and exclusively ours. The event borrows
        // this pointer, and `self.text` outlives the event because both are
        // owned by `self` and `text` is only ever replaced through this method.
        unsafe { sys::ghostty_key_event_set_utf8(self.raw, ptr, len) };
    }

    /// Set the codepoint the key would produce with no shift applied.
    pub fn set_unshifted_codepoint(&mut self, codepoint: u32) {
        // SAFETY: the event is live and exclusively ours.
        unsafe { sys::ghostty_key_event_set_unshifted_codepoint(self.raw, codepoint) };
    }

    /// The physical key.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidValue`] if the library reports a key this build does not
    /// know.
    pub fn key(&self) -> Result<Key> {
        // SAFETY: the event is live.
        let raw = unsafe { sys::ghostty_key_event_get_key(self.raw) };
        Key::from_sys(raw).ok_or(Error::InvalidValue)
    }

    /// The modifiers held at the time of the event.
    #[must_use]
    pub fn mods(&self) -> Mods {
        // SAFETY: the event is live.
        Mods::from_bits(unsafe { sys::ghostty_key_event_get_mods(self.raw) })
    }
}

impl Drop for KeyEvent {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful `ghostty_key_event_new` and
        // is freed exactly once. `self.text` is dropped after this, so the
        // borrowed pointer never outlives its referent.
        unsafe { sys::ghostty_key_event_free(self.raw) };
    }
}

/// Encodes key events into the byte sequences a terminal application expects.
#[derive(Debug)]
pub struct KeyEncoder {
    raw: sys::GhosttyKeyEncoder,
}

// SAFETY: as `KeyEvent` — an owned allocation with no thread affinity.
unsafe impl Send for KeyEncoder {}

impl KeyEncoder {
    /// An encoder with the library defaults: legacy encoding, no Kitty flags.
    ///
    /// # Errors
    ///
    /// [`Error::OutOfMemory`] if the library cannot allocate.
    pub fn new() -> Result<Self> {
        let mut raw: sys::GhosttyKeyEncoder = std::ptr::null_mut();
        // SAFETY: `raw` is a live out-pointer; NULL selects the default allocator.
        check(unsafe { sys::ghostty_key_encoder_new(std::ptr::null(), &raw mut raw) })?;
        if raw.is_null() {
            return Err(Error::InvalidValue);
        }
        Ok(Self { raw })
    }

    /// Copy cursor-key, keypad, alt-escape, `modifyOtherKeys` and Kitty flags
    /// off a live terminal.
    ///
    /// This is the call that keeps encoding correct as the application changes
    /// modes underneath: everything that affects encoding is terminal state,
    /// and this reads all of it at once.
    pub fn sync_from(&mut self, terminal: &Terminal) {
        // SAFETY: both handles are live; the call only reads the terminal.
        unsafe { sys::ghostty_key_encoder_setopt_from_terminal(self.raw, terminal.as_raw()) };
    }

    /// Override the Kitty keyboard flags.
    pub fn set_kitty_flags(&mut self, flags: KittyKeyFlags) {
        let bits = flags.bits();
        // SAFETY: the encoder is live; `bits` outlives the call and matches the
        // GhosttyKittyKeyFlags the option documents.
        unsafe {
            sys::ghostty_key_encoder_setopt(
                self.raw,
                sys::GhosttyKeyEncoderOption::GHOSTTY_KEY_ENCODER_OPT_KITTY_FLAGS,
                std::ptr::from_ref(&bits).cast(),
            );
        }
    }

    /// Turn DECCKM — application cursor keys — on or off.
    pub fn set_cursor_key_application(&mut self, enabled: bool) {
        self.set_bool(
            sys::GhosttyKeyEncoderOption::GHOSTTY_KEY_ENCODER_OPT_CURSOR_KEY_APPLICATION,
            enabled,
        );
    }

    /// Turn application keypad mode on or off.
    pub fn set_keypad_key_application(&mut self, enabled: bool) {
        self.set_bool(
            sys::GhosttyKeyEncoderOption::GHOSTTY_KEY_ENCODER_OPT_KEYPAD_KEY_APPLICATION,
            enabled,
        );
    }

    fn set_bool(&mut self, option: sys::GhosttyKeyEncoderOption::Type, value: bool) {
        // SAFETY: the encoder is live; `value` outlives the call and matches
        // the bool the option documents.
        unsafe {
            sys::ghostty_key_encoder_setopt(self.raw, option, std::ptr::from_ref(&value).cast());
        }
    }

    /// Encode `event`, appending to `out`, and report how many bytes it added.
    ///
    /// Zero is a normal answer: an unmodified modifier key encodes to nothing.
    /// The buffer is grown only when the library says it is too small, so a
    /// reused buffer stops allocating after the first few events.
    ///
    /// # Errors
    ///
    /// Propagates any library failure other than the too-small-buffer retry.
    pub fn encode(&self, event: &KeyEvent, out: &mut Vec<u8>) -> Result<usize> {
        loop {
            let start = out.len();
            let spare = out.capacity() - start;
            let mut written: usize = 0;
            let buffer = if spare == 0 {
                std::ptr::null_mut()
            } else {
                // SAFETY: `start <= capacity`, so the offset is in bounds of
                // the allocation.
                unsafe { out.as_mut_ptr().add(start) }
            };
            // SAFETY: both handles are live; `buffer`/`spare` describe the
            // uninitialised tail of `out`, which the library only writes to.
            let result = unsafe {
                sys::ghostty_key_encoder_encode(
                    self.raw,
                    event.raw,
                    buffer.cast(),
                    spare,
                    &raw mut written,
                )
            };
            match check(result) {
                Ok(()) => {
                    // SAFETY: the library wrote `written` bytes at `start`, and
                    // `start + written <= capacity` because the call succeeded.
                    unsafe { out.set_len(start + written) };
                    return Ok(written);
                }
                // `written` is the required capacity in this case.
                Err(Error::OutOfSpace) => out.reserve(written.max(1)),
                Err(other) => return Err(other),
            }
        }
    }
}

impl Drop for KeyEncoder {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful `ghostty_key_encoder_new`
        // and is freed exactly once.
        unsafe { sys::ghostty_key_encoder_free(self.raw) };
    }
}
