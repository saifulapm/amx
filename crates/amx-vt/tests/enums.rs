#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
//! D-M0-2: every wrapper enum is a lossless view of its `sys::` constants.
//!
//! The wrapper never re-declares a numeric value, so the failure mode this
//! guards against is not a typo — a typo is a compile error at the mapping
//! site. It is a *re-vendor*: a header that renumbers, merges or removes a
//! constant. Then `to_sys` and `from_sys` stop being inverses, or two variants
//! collapse onto one value, and this test says so.

use amx_vt::{
    CellContent, CellWide, ClipboardLocation, ClipboardWriteResult, ColorScheme, CursorStyle,
    Dirty, Key, KeyAction, KittyKeyFlags, Mods, PointSpace, RowSemanticPrompt, Screen, StyleColor,
    Underline,
};

/// Assert the mapping is a bijection between the variants and their constants.
macro_rules! assert_round_trips {
    ($($enumeration:ty),+ $(,)?) => {
        $({
            let all = <$enumeration>::ALL;
            let name = <$enumeration>::NAME;
            assert!(!all.is_empty(), "{name} has no variants");

            let mut seen = Vec::with_capacity(all.len());
            for variant in all {
                let raw = variant.to_sys();
                assert_eq!(
                    <$enumeration>::from_sys(raw),
                    Some(*variant),
                    "{name}::{variant:?} does not survive a round trip through {raw}",
                );
                assert!(
                    !seen.contains(&raw),
                    "{name}::{variant:?} shares constant {raw} with an earlier variant",
                );
                seen.push(raw);
            }
        })+
    };
}

#[test]
fn every_wrapper_enum_round_trips_through_its_sys_constant() {
    assert_round_trips!(
        CellContent,
        CellWide,
        ClipboardLocation,
        ClipboardWriteResult,
        ColorScheme,
        CursorStyle,
        Dirty,
        Key,
        KeyAction,
        PointSpace,
        RowSemanticPrompt,
        Screen,
        StyleColor,
        Underline,
    );
}

/// The key table is the one big enum, and the one most likely to gain entries
/// upstream. If it ever stops covering the whole header this is the tripwire.
#[test]
fn the_key_table_covers_every_key_in_the_header() {
    assert_eq!(Key::ALL.len(), 176, "the vendored key table changed size");
    assert_eq!(Key::ALL.first(), Some(&Key::Unidentified));
    assert_eq!(Key::ALL.last(), Some(&Key::Paste));
}

/// A value from outside the mapping is data, not a panic: the vendored API is
/// explicitly unstable, so a wrapper that assumed exhaustiveness would be
/// asserting something the header disclaims.
#[test]
fn an_unmapped_constant_maps_to_none_rather_than_panicking() {
    let past_the_end = Dirty::ALL
        .iter()
        .map(|dirty| dirty.to_sys())
        .max()
        .expect("at least one variant")
        + 1;

    assert_eq!(Dirty::from_sys(past_the_end), None);
}

/// Bitmask types are not enums and cannot round-trip variant by variant; what
/// they must do is keep every flag distinct and compose without collision.
#[test]
fn every_flag_is_a_distinct_single_bit() {
    for flags in [
        Mods::ALL
            .iter()
            .map(|flag| u32::from(flag.bits()))
            .collect(),
        KittyKeyFlags::ALL
            .iter()
            .map(|flag| u32::from(flag.bits()))
            .collect::<Vec<_>>(),
    ] {
        let mut combined = 0u32;
        for bits in &flags {
            assert_eq!(bits.count_ones(), 1, "{bits} is not a single bit");
            assert_eq!(combined & bits, 0, "{bits} collides with an earlier flag");
            combined |= bits;
        }
    }
}

#[test]
fn flags_compose_and_test_as_a_set() {
    let mods = Mods::CTRL | Mods::SHIFT;

    assert!(mods.contains(Mods::CTRL));
    assert!(mods.contains(Mods::SHIFT));
    assert!(!mods.contains(Mods::ALT));
    assert!(mods.contains(Mods::NONE), "the empty set is always present");
    assert_eq!(Mods::from_bits(mods.bits()), mods);
}
