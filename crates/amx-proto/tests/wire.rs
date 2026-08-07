//! The wire contracts: the frame cap, skew tolerance, and the method table.

use std::collections::BTreeSet;

use amx_core::{GridGeneration, PaneId, RowId, SessionId, ShortNumber};
use amx_proto::control::{Call, Method, SPECS, pane, session};
use amx_proto::error::NegotiationError;
use amx_proto::frame::{CONTROL_CHANNEL, FRAME_HEADER_LEN, MAX_CONTROL_FRAME};
use amx_proto::hello::{ClientInfo, Feature, Hello, Resume, ServerInfo};
use amx_proto::rpc::{Request, RequestId};
use amx_proto::stream::{FlowControl, Priority, StreamId, StreamKind};
use amx_proto::{FrameError, FrameHeader};

#[test]
fn frame_header_rejects_control_frame_over_1mib() {
    let ok = FrameHeader::new(MAX_CONTROL_FRAME as u32, CONTROL_CHANNEL);
    assert_eq!(FrameHeader::decode(ok.encode()).unwrap(), ok);

    let over = FrameHeader::new(MAX_CONTROL_FRAME as u32 + 1, CONTROL_CHANNEL);
    assert_eq!(
        FrameHeader::decode(over.encode()),
        Err(FrameError::ControlFrameTooLarge {
            len: MAX_CONTROL_FRAME + 1
        }),
        "the cap is enforced from the 5-byte header, before any payload is read"
    );

    // u32::MAX is the worst case a peer can declare: it must be refused from
    // the header rather than turned into an allocation.
    let huge = FrameHeader::new(u32::MAX, CONTROL_CHANNEL);
    assert!(FrameHeader::decode(huge.encode()).is_err());

    // The cap is the control channel's; binary streams negotiate their own.
    let stream = FrameHeader::new(MAX_CONTROL_FRAME as u32 + 1, 3);
    assert!(FrameHeader::decode(stream.encode()).is_ok());
    assert!(stream.check_stream_len(1024).is_err());
    assert!(stream.check_stream_len(u32::MAX).is_ok());
}

#[test]
fn frame_header_is_five_little_endian_bytes() {
    let header = FrameHeader::new(0x0102_0304, 7);
    assert_eq!(header.encode(), [0x04, 0x03, 0x02, 0x01, 0x07]);
    assert_eq!(header.encode().len(), FRAME_HEADER_LEN);
    assert_eq!(FrameHeader::decode(header.encode()).unwrap(), header);
    assert!(matches!(
        FrameHeader::decode_slice(&[0, 0]),
        Err(FrameError::TruncatedHeader { .. })
    ));
}

#[test]
fn hello_deserializes_with_unknown_fields_present() {
    // A newer peer sends fields this build has never heard of, at every level.
    // 04 §4: "Unknown JSON fields are ignored by contract." Nothing reachable
    // from Hello may be `deny_unknown_fields`, or the N/N-1 window is a lie.
    let json = r#"{
        "proto": [1, 3],
        "features": ["grid.stream", "some.future.feature"],
        "client": {
            "name": "amx",
            "version": "9.9.9",
            "term": "xterm-ghostty",
            "future_client_field": {"nested": true}
        },
        "resume": {
            "last_seq": 41,
            "generations": [],
            "future_resume_field": [1, 2, 3]
        },
        "future_top_level_field": "ignored"
    }"#;

    let hello: Hello = serde_json::from_str(json).expect("unknown fields must be ignored");
    assert_eq!(hello.proto, (1, 3));
    assert!(hello.features.contains(&Feature::GRID_STREAM));
    assert!(
        hello
            .features
            .contains(&Feature::named("some.future.feature")),
        "an unknown feature name survives as a name, it is not a decode error"
    );
    assert_eq!(hello.client.version, "9.9.9");
    assert_eq!(hello.resume.map(|r| r.last_seq), Some(41));
}

#[test]
fn hello_round_trips_and_keeps_unknown_features() {
    let hello = Hello {
        proto: (1, 1),
        features: BTreeSet::from([
            Feature::GRID_STREAM,
            Feature::HISTORY_RANGES,
            Feature::named("experimental.thing"),
        ]),
        client: ClientInfo {
            name: "amx".into(),
            version: "0.1.0".into(),
            term: None,
        },
        attach: false,
        resume: Some(Resume {
            last_seq: 7,
            generations: vec![(PaneId::new_v4(), GridGeneration::FIRST.next())],
        }),
    };

    let json = serde_json::to_string(&hello).unwrap();
    assert_eq!(serde_json::from_str::<Hello>(&json).unwrap(), hello);
}

#[test]
fn stream_control_types_round_trip_serde() {
    let pane = PaneId::new_v4();
    let kinds = [
        StreamKind::PaneGrid { pane },
        StreamKind::History { pane },
        StreamKind::RawPaneIo { pane },
        StreamKind::Graphics { pane },
    ];
    for kind in kinds {
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(serde_json::from_str::<StreamKind>(&json).unwrap(), kind);
        assert_eq!(kind.pane(), pane);
    }

    let stream = StreamId::new(4);
    let signals = [
        FlowControl::StreamCap {
            stream,
            max_frame: 4096,
        },
        FlowControl::Pause { stream },
        FlowControl::Resume { stream },
        FlowControl::Resync { stream },
    ];
    for signal in signals {
        let json = serde_json::to_string(&signal).unwrap();
        assert_eq!(serde_json::from_str::<FlowControl>(&json).unwrap(), signal);
    }
}

#[test]
fn priority_is_strict_control_then_grid_then_bulk() {
    assert!(Priority::Control < Priority::Grid);
    assert!(Priority::Grid < Priority::Bulk);

    let pane = PaneId::new_v4();
    assert_eq!(
        Priority::of(&StreamKind::PaneGrid { pane }),
        Priority::Grid,
        "grid deltas never outrank a control reply"
    );
    assert_eq!(Priority::of(&StreamKind::History { pane }), Priority::Bulk);
}

#[test]
fn stream_ids_map_onto_frame_channels_but_never_onto_control() {
    assert_eq!(StreamId::new(3).channel(), Some(3));
    assert_eq!(
        StreamId::new(u16::from(CONTROL_CHANNEL)).channel(),
        None,
        "channel 0 is the control channel and is never a bound stream"
    );
    assert_eq!(StreamId::new(256).channel(), None);
}

#[test]
fn method_table_generates_matching_names_variants_and_specs() {
    assert_eq!(Method::ALL.len(), SPECS.len());

    for method in Method::ALL {
        let spec = method.spec();
        assert_eq!(spec.method, *method, "SPECS row is out of order");
        assert_eq!(spec.wire, method.wire_name());
        assert_eq!(Method::from_wire_name(spec.wire), Some(*method));

        // The wire name and the CLI path are the same table row seen twice.
        assert_eq!(spec.cli.join("."), spec.wire);

        // Serde names come from the same row, so they cannot drift from it.
        let json = serde_json::to_string(method).unwrap();
        assert_eq!(json, format!("\"{}\"", spec.wire));
        assert_eq!(serde_json::from_str::<Method>(&json).unwrap(), *method);
    }

    assert_eq!(Method::from_wire_name("pane.no_such_method"), None);
}

#[test]
fn unknown_method_decodes_to_method_not_found_not_a_decode_failure() {
    let error = Call::decode("pane.teleport", None).unwrap_err();
    assert_eq!(
        error.code,
        amx_proto::rpc::RpcError::METHOD_NOT_FOUND,
        "an unknown method is a typed reply, never a dropped connection"
    );
}

#[test]
fn call_decodes_params_into_the_row_s_payload_type() {
    let pane = PaneId::new_v4();
    let params = serde_json::json!({
        "pane": pane,
        "direction": "vertical",
        "future_param": 1
    });

    let call = Call::decode("pane.split", Some(params)).unwrap();
    assert_eq!(call.method(), Method::PaneSplit);
    let Call::PaneSplit(split) = call else {
        panic!("decoded the wrong variant");
    };
    assert_eq!(split.pane, pane);
    assert_eq!(split.direction, pane::SplitDirection::Vertical);
    assert_eq!(split.cwd, None);
}

#[test]
fn method_table_generates_the_clap_tree() {
    let commands = amx_proto::control::cli::method_commands();
    let names: Vec<_> = commands.iter().map(|c| c.get_name().to_owned()).collect();
    assert!(names.contains(&"ping".to_owned()));
    assert!(names.contains(&"pane".to_owned()));
    assert!(names.contains(&"workspace".to_owned()));

    let pane_command = commands
        .iter()
        .find(|c| c.get_name() == "pane")
        .expect("pane group");
    assert!(
        pane_command
            .get_subcommands()
            .any(|c| c.get_name() == "split"),
        "pane.split is reachable as `amx pane split`"
    );

    for spec in SPECS {
        assert!(amx_proto::control::cli::has_path(spec.cli));
    }
}

#[test]
fn method_table_generates_matching_serde_names_dispatch_and_clap_tree() {
    // The M0 verb surface: ping + session.state + workspace{create,rename,
    // kill,switch} + pane{split,zoom,swap,move,close,focus,resize,history} +
    // stream.bind + client.viewport; plus M1's pane.rename and session.report.
    assert_eq!(Method::ALL.len(), 18);
    assert_eq!(Method::ALL.len(), SPECS.len());

    for method in Method::ALL {
        let spec = method.spec();
        assert_eq!(spec.wire, method.wire_name());
        assert_eq!(Method::from_wire_name(spec.wire), Some(*method));
        assert_eq!(spec.cli.join("."), spec.wire);

        let json = serde_json::to_string(method).unwrap();
        assert_eq!(json, format!("\"{}\"", spec.wire));
        assert_eq!(serde_json::from_str::<Method>(&json).unwrap(), *method);

        assert!(
            amx_proto::control::cli::has_path(spec.cli),
            "{} is not reachable from the generated clap tree",
            spec.wire
        );
    }

    // `crates/amx-proto/tests/dispatch.rs`'s `StubServer` implements
    // `Dispatch` for every row above: a row without a handler fails that
    // file's build, not this one's, so dispatch completeness is proven there.
}

#[test]
fn negotiation_picks_highest_common_version() {
    // window() is (1, 1) today; exercise the general algorithm directly so
    // this test keeps meaning once PROTO_MAX moves past 1.
    assert_eq!(amx_proto::version::negotiate((1, 1), (1, 1)).unwrap(), 1);
    assert_eq!(amx_proto::version::negotiate((1, 3), (2, 4)).unwrap(), 3);
    assert_eq!(amx_proto::version::negotiate((1, 5), (3, 3)).unwrap(), 3);
    assert_eq!(
        amx_proto::version::negotiate((2, 2), (1, 5)).unwrap(),
        2,
        "highest common version, not the client's preference or the server's max"
    );

    let no_overlap = amx_proto::version::negotiate((1, 1), (2, 2)).unwrap_err();
    assert!(matches!(
        no_overlap,
        NegotiationError::NoCommonVersion { .. }
    ));

    let malformed = amx_proto::version::negotiate((5, 3), (1, 9)).unwrap_err();
    assert!(matches!(
        malformed,
        NegotiationError::MalformedWindow { .. }
    ));
}

#[test]
fn negotiation_intersects_features_rather_than_requiring_equality() {
    let hello = Hello {
        proto: (1, 1),
        features: BTreeSet::from([
            Feature::GRID_STREAM,
            Feature::HISTORY_RANGES,
            Feature::named("client.only.thing"),
        ]),
        client: ClientInfo {
            name: "amx".into(),
            version: "0.1.0".into(),
            term: None,
        },
        attach: false,
        resume: None,
    };
    let supported = BTreeSet::from([Feature::GRID_STREAM, Feature::RAW_PANE_IO]);

    let welcome = hello
        .accept(
            ServerInfo {
                name: "amxd".into(),
                version: "0.1.0".into(),
            },
            &supported,
            9,
            SessionId::new_v4(),
        )
        .unwrap();

    assert_eq!(welcome.proto, 1);
    assert_eq!(
        welcome.features,
        BTreeSet::from([Feature::GRID_STREAM]),
        "neither side's extra feature survives — only the intersection does"
    );
}

#[test]
fn negotiation_fails_only_when_versions_do_not_overlap() {
    let hello = Hello {
        proto: (5, 9),
        features: BTreeSet::new(),
        client: ClientInfo::default(),
        attach: false,
        resume: None,
    };
    let error = hello
        .accept(
            ServerInfo::default(),
            &BTreeSet::new(),
            0,
            SessionId::new_v4(),
        )
        .unwrap_err();
    assert!(matches!(error, NegotiationError::NoCommonVersion { .. }));
}

#[test]
fn state_reply_with_restore_summary_round_trips_and_omits_when_none() {
    let pane = PaneId::new_v4();
    let mut reply = session::StateReply {
        seq: 9,
        focused_workspace: None,
        workspaces: vec![],
        panes: vec![session::PaneState {
            pane,
            short: ShortNumber::FIRST,
            label: Some("editor".to_owned()),
            rows: 24,
            cols: 80,
            history_head: RowId::from_raw(0),
            history_floor: RowId::from_raw(0),
        }],
        restore: Some(session::RestoreSummary {
            restored: 4,
            lost: 1,
            degraded: 2,
        }),
    };

    let json = serde_json::to_value(&reply).unwrap();
    assert_eq!(json["restore"]["lost"], 1);
    assert_eq!(json["panes"][0]["label"], "editor");
    assert_eq!(
        serde_json::from_value::<session::StateReply>(json).unwrap(),
        reply
    );

    // Absent, not null: both additions are optional, so a server that never
    // restored anything and a pane with no label produce exactly the bytes a
    // pre-M1 peer already knows how to read (R-M1-8).
    reply.restore = None;
    reply.panes[0].label = None;
    let json = serde_json::to_value(&reply).unwrap();
    assert!(json.get("restore").is_none(), "{json}");
    assert!(json["panes"][0].get("label").is_none(), "{json}");

    // And the same tolerance the other way: a summary from a newer amx with a
    // field this build never heard of still decodes.
    let from_the_future = serde_json::json!({
        "seq": 9,
        "workspaces": [],
        "panes": [],
        "restore": { "restored": 4, "lost": 1, "degraded": 2, "quarantined": 7 },
    });
    let decoded: session::StateReply = serde_json::from_value(from_the_future).unwrap();
    assert_eq!(decoded.restore.unwrap().lost, 1);
}

#[test]
fn a_restore_report_summarizes_its_own_entries() {
    let report = session::RestoreReport {
        entries: vec![
            loss(session::RestoreSeverity::Lost),
            loss(session::RestoreSeverity::Degraded),
            loss(session::RestoreSeverity::Degraded),
        ],
    };
    let summary = report.summary(4);
    assert_eq!(summary.restored, 4);
    assert_eq!(summary.lost, 1);
    assert_eq!(summary.degraded, 2);
    assert!(!summary.is_clean());
    assert!(session::RestoreReport::default().summary(4).is_clean());
}

fn loss(severity: session::RestoreSeverity) -> session::RestoreLoss {
    session::RestoreLoss {
        severity,
        entity: session::RestoreEntity::Pane,
        workspace: None,
        pane: Some(PaneId::new_v4()),
        label: None,
        path: None,
        reason: "for the count".to_owned(),
    }
}

#[test]
fn unknown_control_fields_are_ignored() {
    // A newer peer's envelope and params both carry fields this build has
    // never heard of; neither may become a decode failure (04 §4).
    let json = r#"{
        "jsonrpc": "2.0",
        "id": 4,
        "method": "pane.split",
        "params": {
            "pane": "00000000-0000-0000-0000-000000000001",
            "direction": "vertical",
            "future_param_field": true
        },
        "future_envelope_field": "ignored"
    }"#;

    let request: Request =
        serde_json::from_str(json).expect("unknown envelope fields must be ignored");
    assert_eq!(request.id, RequestId::Number(4));
    assert_eq!(request.method, "pane.split");

    let call = Call::decode(&request.method, request.params).unwrap();
    let Call::PaneSplit(split) = call else {
        panic!("decoded the wrong variant");
    };
    assert_eq!(split.direction, pane::SplitDirection::Vertical);
}
