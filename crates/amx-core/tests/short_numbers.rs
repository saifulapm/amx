//! The display mapping from identity to short number (04 §6, DR-6).
//!
//! Short numbers are the one thing in amx a user types that is not a UUID, and
//! the whole of what makes them usable is the two properties asserted here:
//! the number offered is the lowest one free, and a number nobody holds
//! resolves to nothing at all rather than to whatever moved into the slot.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::{PaneId, ShortNumber, ShortNumbers};

/// Four panes, in a fixed order, so a test can talk about "the third one".
fn panes<const N: usize>() -> [PaneId; N] {
    std::array::from_fn(|_| PaneId::new_v4())
}

#[test]
fn assignment_hands_out_the_lowest_free_number_and_is_idempotent() {
    let [a, b, c] = panes();
    let mut shorts = ShortNumbers::new();

    assert_eq!(shorts.assign(a), ShortNumber::FIRST);
    assert_eq!(shorts.assign(b), ShortNumber::new(2));
    assert_eq!(shorts.assign(c), ShortNumber::new(3));
    assert_eq!(
        shorts.assign(b),
        ShortNumber::new(2),
        "a second assignment returns the number the key already holds",
    );
    assert_eq!(shorts.len(), 3);
}

#[test]
fn a_released_number_is_reused_by_the_next_assignment_and_not_before() {
    let [a, b, c, d] = panes();
    let mut shorts = ShortNumbers::new();
    for pane in [a, b, c] {
        let _ = shorts.assign(pane);
    }

    assert_eq!(shorts.release(&b), Some(ShortNumber::new(2)));
    assert_eq!(
        shorts.get(&a),
        Some(ShortNumber::FIRST),
        "releasing one key renumbers nobody else",
    );
    assert_eq!(shorts.get(&c), Some(ShortNumber::new(3)));

    assert_eq!(
        shorts.assign(d),
        ShortNumber::new(2),
        "the freed number is the lowest one free, so the next assignment takes it",
    );
    let [e] = panes();
    assert_eq!(
        shorts.assign(e),
        ShortNumber::new(4),
        "and the one after it goes past the high-water mark again",
    );
}

#[test]
fn a_number_whose_object_is_gone_resolves_to_none() {
    let [a, b] = panes();
    let mut shorts = ShortNumbers::new();
    let first = shorts.assign(a);
    let second = shorts.assign(b);

    assert_eq!(shorts.resolve(first), Some(a));
    assert_eq!(shorts.resolve(second), Some(b));

    shorts.release(&a);
    assert_eq!(
        shorts.resolve(first),
        None,
        "released, so it names nothing — not the next key along",
    );
    assert_eq!(
        shorts.resolve(second),
        Some(b),
        "and the key that kept its number keeps its answer",
    );
    assert_eq!(
        shorts.resolve(ShortNumber::new(97)),
        None,
        "a number never handed out names nothing either",
    );
}

#[test]
fn resolution_follows_reuse_rather_than_remembering_the_old_holder() {
    // The other half of the sentence above: once the number *is* reused it
    // names its new holder, because the mapping is display sugar and never an
    // identity. Nothing addresses state by short number internally.
    let [a, b] = panes();
    let mut shorts = ShortNumbers::new();
    let number = shorts.assign(a);
    shorts.release(&a);
    assert_eq!(shorts.assign(b), number);
    assert_eq!(shorts.resolve(number), Some(b));
}

#[test]
fn adoption_takes_the_recorded_number_and_leaves_the_gaps_free() {
    // What restore does: a snapshot written by a server that never reused a
    // number comes back with the numbers it recorded, and the holes in it are
    // what the next pane gets.
    let [a, b, c] = panes();
    let mut shorts = ShortNumbers::new();
    assert_eq!(shorts.adopt(a, ShortNumber::new(2)), ShortNumber::new(2));
    assert_eq!(shorts.adopt(b, ShortNumber::new(5)), ShortNumber::new(5));

    assert_eq!(shorts.resolve(ShortNumber::new(5)), Some(b));
    assert_eq!(
        shorts.assign(c),
        ShortNumber::FIRST,
        "the lowest free number is the hole below the adopted ones",
    );
}

#[test]
fn adoption_refuses_to_let_two_keys_hold_one_number() {
    // A rewritten file rather than a session, but `resolve` must stay
    // single-valued whatever it is handed.
    let [a, b] = panes();
    let mut shorts = ShortNumbers::new();
    shorts.adopt(a, ShortNumber::new(3));
    let second = shorts.adopt(b, ShortNumber::new(3));

    assert_eq!(
        second,
        ShortNumber::FIRST,
        "the duplicate falls back to free"
    );
    assert_eq!(shorts.resolve(ShortNumber::new(3)), Some(a));
    assert_eq!(shorts.resolve(ShortNumber::FIRST), Some(b));

    assert_eq!(
        shorts.adopt(a, ShortNumber::new(3)),
        ShortNumber::new(3),
        "re-adopting the number a key already holds is not a collision",
    );
}

#[test]
fn retain_releases_every_key_it_rejects() {
    let [a, b, c] = panes();
    let mut shorts = ShortNumbers::new();
    for pane in [a, b, c] {
        let _ = shorts.assign(pane);
    }

    shorts.retain(|pane| *pane == c);
    assert_eq!(shorts.len(), 1);
    assert_eq!(shorts.get(&c), Some(ShortNumber::new(3)));
    assert_eq!(shorts.resolve(ShortNumber::FIRST), None);

    let [d] = panes();
    assert_eq!(
        shorts.assign(d),
        ShortNumber::FIRST,
        "the released numbers are free again",
    );
}

#[test]
fn the_mapping_survives_a_round_trip_through_the_snapshot_format() {
    // 04 §6: assigned at creation, serialized with the session, stable across
    // restarts. The mapping is state on disk, not a derivation from position.
    let [a, b] = panes();
    let mut shorts: ShortNumbers<PaneId> = ShortNumbers::new();
    shorts.adopt(a, ShortNumber::new(4));
    shorts.assign(b);

    let json = serde_json::to_string(&shorts).expect("serialize the mapping");
    let back: ShortNumbers<PaneId> = serde_json::from_str(&json).expect("read it back");

    assert_eq!(back.get(&a), Some(ShortNumber::new(4)));
    assert_eq!(back.resolve(ShortNumber::new(4)), Some(a));
    assert_eq!(back.resolve(ShortNumber::FIRST), Some(b));
}

#[test]
fn parsing_a_typed_number_is_a_question_about_the_string() {
    assert_eq!(ShortNumber::parse("1"), Some(ShortNumber::FIRST));
    assert_eq!(ShortNumber::parse("42"), Some(ShortNumber::new(42)));
    assert_eq!(ShortNumber::parse("0"), Some(ShortNumber::new(0)));

    for label in ["", " 1", "1 ", "+1", "-1", "1a", "0x2", "one", "1.0"] {
        assert_eq!(
            ShortNumber::parse(label),
            None,
            "{label:?} is a label, not a number",
        );
    }
    assert_eq!(
        ShortNumber::parse("4294967296"),
        None,
        "a number the counter cannot hold is not one",
    );
}
