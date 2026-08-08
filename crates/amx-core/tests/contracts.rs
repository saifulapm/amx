//! The core contracts: what the event stream promises and how effects fold.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::agent::{
    Activity, AgentKind, AgentSnapshot, AgentState, AgentWorkspace, CoverageClass, ExitAuthority,
    HookToken, RefKind, RefSource, SessionRef, StatusCause,
};
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
fn the_m1_event_variants_round_trip_and_tag_themselves() {
    let restored = Event::SessionRestored {
        workspaces: 2,
        panes: 5,
        lost: 1,
        degraded: 3,
    };
    let json = serde_json::to_value(&restored).unwrap();
    assert_eq!(json["event"], "session_restored");
    assert_eq!(json["lost"], 1);
    assert_eq!(
        serde_json::from_value::<Event>(json).unwrap(),
        restored,
        "the restore summary must survive the bus's own encoding",
    );

    let reloaded = Event::ConfigReloaded {
        rejected_sections: 1,
    };
    let json = serde_json::to_value(&reloaded).unwrap();
    assert_eq!(json["event"], "config_reloaded");
    assert_eq!(serde_json::from_value::<Event>(json).unwrap(), reloaded);
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

// ------------------------------------------------------------ the agent layer

#[test]
fn the_m2_event_variants_round_trip_and_tag_themselves() {
    let pane = PaneId::new_v4();

    let status = Event::AgentStatus {
        pane,
        from: Some(AgentState::Working),
        to: AgentState::Blocked,
        cause: StatusCause::Hook,
    };
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(json["event"], "agent_status");
    assert_eq!(json["from"], "working");
    assert_eq!(json["to"], "blocked");
    assert_eq!(json["cause"], "hook");
    assert_eq!(serde_json::from_value::<Event>(json).unwrap(), status);

    // A pane's *first* status has nothing to come from, and `from` is absent
    // rather than null so a consumer reading the field cannot mistake "there
    // was no previous state" for "the previous state was something I do not
    // recognise".
    let first = Event::AgentStatus {
        pane,
        from: None,
        to: AgentState::Quiet,
        cause: StatusCause::Probe,
    };
    let json = serde_json::to_value(&first).unwrap();
    assert!(json.get("from").is_none(), "{json}");
    assert_eq!(serde_json::from_value::<Event>(json).unwrap(), first);

    let identified = Event::AgentIdentified {
        pane,
        kind: AgentKind::new("claude").unwrap(),
    };
    let json = serde_json::to_value(&identified).unwrap();
    assert_eq!(json["event"], "agent_identified");
    assert_eq!(
        json["kind"], "claude",
        "a kind is a bare string on the wire"
    );
    assert_eq!(serde_json::from_value::<Event>(json).unwrap(), identified);

    for (event, tag) in [
        (
            Event::AttentionEnqueued {
                pane,
                workspace: None,
                name: None,
                reason: None,
                since: None,
            },
            "attention_enqueued",
        ),
        (
            Event::AttentionDequeued {
                pane,
                workspace: None,
                name: None,
                reason: None,
                since: None,
            },
            "attention_dequeued",
        ),
    ] {
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event"], tag);
        assert_eq!(serde_json::from_value::<Event>(json).unwrap(), event);
    }
}

/// D15's identity block rides the attention events additively, in both
/// directions.
///
/// Two halves, and a field that satisfies only one of them strands a peer: a
/// build that *sends* the block must be understood by one that has it, and a
/// build that sends none must produce exactly the bytes M2 produced — the keys
/// absent, not present-and-null.
#[test]
fn the_attention_identity_block_is_additive_in_both_directions() {
    let pane = PaneId::new_v4();
    let workspace = WorkspaceId::new_v4();

    let bare = Event::AttentionEnqueued {
        pane,
        workspace: None,
        name: None,
        reason: None,
        since: None,
    };
    let json = serde_json::to_value(&bare).unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "event": "attention_enqueued", "pane": pane }),
        "a hub that knows no names writes the bytes M2 wrote",
    );

    let named = Event::AttentionDequeued {
        pane,
        workspace: Some(AgentWorkspace {
            id: workspace,
            name: Some("api".to_owned()),
        }),
        name: Some("backend".to_owned()),
        reason: Some("permission_dialog".to_owned()),
        since: Some(1_754_650_000_000),
    };
    let json = serde_json::to_value(&named).unwrap();
    assert_eq!(
        json["workspace"],
        serde_json::json!({ "id": workspace, "name": "api" })
    );
    assert_eq!(json["name"], "backend");
    assert_eq!(
        json["reason"], "permission_dialog",
        "the reason is the detector's own name, not a taxonomy of amx's",
    );
    assert_eq!(json["since"], 1_754_650_000_000_u64);
    assert_eq!(serde_json::from_value::<Event>(json).unwrap(), named);

    // And an event from a build that has never heard of the block still
    // decodes, to the absences that mean "this build did not say".
    let older: Event =
        serde_json::from_value(serde_json::json!({ "event": "attention_enqueued", "pane": pane }))
            .unwrap();
    assert_eq!(older, bare);

    // A workspace with no label is the id alone, which is the other absence a
    // renderer has to be ready for.
    let anonymous = AgentWorkspace::unnamed(workspace);
    assert_eq!(
        serde_json::to_value(&anonymous).unwrap(),
        serde_json::json!({ "id": workspace }),
    );
}

/// D-M4-3 and D-M4-4 on the snapshot every surface reads.
///
/// `reason` and `since` are optional because their absence is a real state —
/// a tier-3 `busy` that no named rule produced, and a status re-derived rather
/// than observed entering — and a zero `since` would read as 1970.
#[test]
fn a_snapshot_carries_a_reason_and_an_entry_instant_or_says_it_has_neither() {
    let unknown = AgentSnapshot::unidentified(7);
    assert_eq!(
        unknown.reason, None,
        "no named rule produced a tier-3 quiet"
    );
    assert_eq!(unknown.since, None);
    let json = serde_json::to_value(&unknown).unwrap();
    assert!(json.get("reason").is_none(), "{json}");
    assert!(json.get("since").is_none(), "{json}");

    let blocked = AgentSnapshot {
        kind: Some(AgentKind::new("claude").unwrap()),
        state: AgentState::Blocked,
        cause: StatusCause::Screen,
        transition_seq: 51,
        attention: Some(0),
        session_ref: None,
        reason: Some("permission_dialog".to_owned()),
        since: Some(1_754_650_000_000),
    };
    let json = serde_json::to_value(&blocked).unwrap();
    assert_eq!(json["reason"], "permission_dialog");
    assert_eq!(json["since"], 1_754_650_000_000_u64);
    assert_eq!(
        serde_json::from_value::<AgentSnapshot>(json).unwrap(),
        blocked
    );

    // A snapshot written by a build from before M4 still parses, which is what
    // lets `since` ride the handoff manifest and the persisted session without
    // a version of their own (R-M4-4).
    let older: AgentSnapshot = serde_json::from_value(serde_json::json!({
        "state": "idle", "cause": "hook", "transition_seq": 3
    }))
    .unwrap();
    assert_eq!(older.reason, None);
    assert_eq!(older.since, None);
}

#[test]
fn only_the_full_class_may_leave_a_held_state_on_a_hook_edge() {
    // V01 measured every user-initiated exit as silent on both shipped agents,
    // so `edges` — the class both of them ship in — cannot be handed the
    // authority to close a turn, and there is no way to forge one.
    for class in [
        CoverageClass::Edges,
        CoverageClass::Identity,
        CoverageClass::None,
    ] {
        assert!(
            ExitAuthority::from_hook(class).is_none(),
            "{class} must not take an exit from a hook",
        );
        assert!(class.exit_is_screen_owned(), "{class}");
        assert!(class.needs_screen(), "{class}");
    }
    assert_eq!(
        ExitAuthority::from_hook(CoverageClass::Full).map(ExitAuthority::cause),
        Some(StatusCause::Hook),
    );
    assert!(!CoverageClass::Full.exit_is_screen_owned());

    // Entry is the asymmetry: an `edges` agent's entry edges apply instantly
    // even though its exits never do.
    assert!(CoverageClass::Edges.hook_entries_apply());
    assert!(!CoverageClass::Edges.hook_exits_apply());
    assert!(!CoverageClass::Identity.hook_entries_apply());

    // The two exits that always exist, for every class.
    assert_eq!(ExitAuthority::from_screen().cause(), StatusCause::Screen);
    assert_eq!(
        ExitAuthority::from_staleness().cause(),
        StatusCause::Staleness,
    );
}

#[test]
fn tier_three_cannot_spell_blocked() {
    // 04 §5's "busy/quiet, never fake `blocked`", as a property of the types:
    // the only way from an `Activity` to an `AgentState` is this conversion,
    // and neither arm reaches `Blocked`.
    for activity in [Activity::Busy, Activity::Quiet] {
        let state = AgentState::from(activity);
        assert!(!state.wants_attention(), "{state} came from tier 3");
        assert!(!state.is_agent(), "{state} came from tier 3");
        assert!(!state.is_held(), "{state} came from tier 3");
    }
    assert!(AgentState::Blocked.wants_attention());
    assert!(AgentState::Blocked.is_held());
    assert!(AgentState::Working.is_held());
    assert!(!AgentState::Idle.is_held());
}

#[test]
fn a_session_ref_cannot_be_built_or_parsed_out_of_shape() {
    let ok = SessionRef::new(RefKind::Id, "c9a3c73b-b184-4871-8e98-79b46b87b635").unwrap();
    assert_eq!(ok.kind(), RefKind::Id);

    // Every rule D-M2-7 states, refused at the constructor...
    assert!(SessionRef::new(RefKind::Id, "").is_err());
    assert!(SessionRef::new(RefKind::Id, "a\u{1b}[2Jb").is_err());
    assert!(SessionRef::new(RefKind::Path, "relative/path.jsonl").is_err());
    assert!(SessionRef::new(RefKind::Path, "/abs/path.jsonl").is_ok());
    assert!(SessionRef::new(RefKind::Id, "x".repeat(513)).is_err());

    // ...and again on the way in from disk, which is the gate that matters:
    // a `session.json` is user-editable, and this is what stops a hand-edited
    // one from reaching an argv (D-M2-7's snapshot-read gate).
    let hostile = serde_json::json!({ "kind": "path", "value": "../../etc/passwd" });
    assert!(serde_json::from_value::<SessionRef>(hostile).is_err());

    let json = serde_json::to_value(&ok).unwrap();
    assert_eq!(json["kind"], "id");
    assert_eq!(serde_json::from_value::<SessionRef>(json).unwrap(), ok);
}

#[test]
fn a_ref_source_names_the_agent_it_claims() {
    let claude = AgentKind::new("claude").unwrap();
    let source = RefSource::for_agent(&claude);
    assert_eq!(source.as_str(), "amx:claude");
    assert_eq!(source.agent(), claude);

    // The allowlist's shape half. The other half — that this id names a stanza,
    // and the *same* stanza the ref claims — needs the registry and is V03's.
    for foreign in ["claude", "amx:", "herdr:claude", "amx:../claude"] {
        assert!(
            RefSource::new(foreign).is_err(),
            "{foreign} is not one of amx's own hook paths",
        );
        assert!(serde_json::from_value::<RefSource>(serde_json::json!(foreign)).is_err());
    }
}

#[test]
fn an_agent_kind_is_a_bare_validated_string() {
    for good in ["claude", "codex", "open-code", "kilo_2"] {
        assert_eq!(AgentKind::new(good).unwrap().as_str(), good);
    }
    for bad in ["", "Claude", "cl aude", "../claude", &"x".repeat(65)] {
        assert!(AgentKind::new(bad).is_err(), "{bad:?} must not be a kind");
        assert!(serde_json::from_value::<AgentKind>(serde_json::json!(bad)).is_err());
    }
}

#[test]
fn a_hook_token_does_not_print_itself() {
    // A token that reached a log line would outlive the pane that minted it,
    // in a file the user did not expect to hold one.
    let token = HookToken::new("b7f0c1a4d9e25638");
    assert_eq!(token.as_str(), "b7f0c1a4d9e25638");
    let shown = format!("{token:?}");
    assert!(!shown.contains("b7f0"), "{shown}");
}
