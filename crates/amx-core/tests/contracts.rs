//! The core contracts: what the event stream promises and how effects fold.

use amx_core::event::{Delivery, Envelope, Event};
use amx_core::{Effect, EffectSet, GridGeneration, Level, PaneId, Scheduled, WorkspaceId};

#[test]
fn event_delivery_enum_round_trips_serde() {
    let pane = PaneId::new_v4();
    let workspace = WorkspaceId::new_v4();

    let deliveries = vec![
        Delivery::Event(Envelope {
            seq: 1,
            event: Event::PaneCreated { pane, workspace },
        }),
        Delivery::Event(Envelope {
            seq: 2,
            event: Event::PaneResized {
                pane,
                rows: 24,
                cols: 80,
                generation: GridGeneration::FIRST.next(),
            },
        }),
        Delivery::Event(Envelope {
            seq: 3,
            event: Event::PaneExited {
                pane,
                status: Some(0),
            },
        }),
        Delivery::Event(Envelope {
            seq: 4,
            event: Event::FocusChanged {
                workspace,
                pane: None,
            },
        }),
        Delivery::Gap { from: 5, to: 41 },
    ];

    for delivery in deliveries {
        let json = serde_json::to_string(&delivery).unwrap();
        let back: Delivery = serde_json::from_str(&json).unwrap();
        assert_eq!(delivery, back, "round trip changed {json}");
    }
}

#[test]
fn gap_is_a_delivery_variant_not_an_out_of_band_flag() {
    // The contract is that loss is *visible*: a subscriber cannot consume the
    // stream without handling the gap, because the gap is what `recv` hands it.
    let gap = Delivery::Gap { from: 7, to: 9 };
    let json = serde_json::to_value(&gap).unwrap();
    assert_eq!(json["delivery"], "gap");
    assert_eq!(json["from"], 7);
    assert_eq!(json["to"], 9);
}

#[test]
fn effect_set_orders_nothing_below_pane_damage_below_layout_below_full() {
    assert!(Level::Nothing < Level::PaneDamage);
    assert!(Level::PaneDamage < Level::Layout);
    assert!(Level::Layout < Level::Full);

    let pane = PaneId::new_v4();
    let ordered = [
        Effect::Nothing,
        Effect::PaneDamage(pane),
        Effect::Layout,
        Effect::Full,
    ];

    // Absorbing in any order keeps the strongest effect: the fold is a maximum,
    // so a weak effect arriving after a strong one cannot weaken the batch.
    for (i, strong) in ordered.iter().enumerate() {
        for weak in &ordered[..=i] {
            let mut set = EffectSet::new();
            set.absorb(*weak);
            set.absorb(*strong);
            assert_eq!(set.level(), strong.level(), "{weak:?} then {strong:?}");

            let mut reversed = EffectSet::new();
            reversed.absorb(*strong);
            reversed.absorb(*weak);
            assert_eq!(reversed.level(), strong.level(), "{strong:?} then {weak:?}");
        }
    }
}

#[test]
fn effect_set_starts_empty_and_drains_into_a_reused_schedule() {
    let first = PaneId::new_v4();
    let second = PaneId::new_v4();

    let mut set = EffectSet::new();
    assert!(set.is_empty());

    set.absorb(Effect::PaneDamage(first));
    set.absorb(Effect::PaneDamage(second));
    set.absorb(Effect::PaneDamage(first));
    assert!(!set.is_empty());
    assert_eq!(set.panes(), [first, second], "panes are deduplicated");

    let mut scheduled = Scheduled::new();
    set.drain_into(&mut scheduled);
    assert_eq!(scheduled.level(), Level::PaneDamage);
    assert_eq!(scheduled.panes(), [first, second]);

    // Draining resets the accumulator for the next batch...
    assert!(set.is_empty());
    assert_eq!(set.panes(), []);

    // ...and draining again clears what the schedule held rather than appending.
    set.absorb(Effect::Layout);
    set.drain_into(&mut scheduled);
    assert_eq!(scheduled.level(), Level::Layout);
    assert_eq!(scheduled.panes(), [], "stale panes do not survive a drain");
}

#[test]
fn ids_round_trip_through_display_and_from_str() {
    let pane = PaneId::new_v4();
    let parsed: PaneId = pane.to_string().parse().unwrap();
    assert_eq!(pane, parsed);

    let json = serde_json::to_string(&pane).unwrap();
    assert_eq!(
        json,
        format!("\"{pane}\""),
        "ids are bare strings on the wire"
    );
    assert_eq!(pane, serde_json::from_str::<PaneId>(&json).unwrap());

    assert!("not-a-uuid".parse::<PaneId>().is_err());
}

#[test]
fn grid_generation_is_monotonic() {
    let first = GridGeneration::FIRST;
    assert!(first < first.next());
    assert!(first.next() < first.next().next());
}
