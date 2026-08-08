//! M3's and M4's additive fields, and the both-directions tolerance that lets
//! them ride protocol v1 (R-M1-8, `docs/09-m3-plan.md` §4,
//! `docs/11-m4-plan.md` §3).
//!
//! "Additive, no version bump" is a claim with two halves, and a field that
//! only satisfies one of them strands a peer:
//!
//! - a peer that **sends** the field must be understood by a build that has it,
//!   and its absence must decode to the same value it always did;
//! - a peer that **does not** send it must produce exactly the bytes the older
//!   peer produced, so nothing downstream sees a shape it has never seen.
//!
//! Both are asserted per field here rather than left to the goldens, because a
//! golden freezes one example and this is about the two shapes at once.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::GridGeneration;
use amx_core::agent::{AgentSnapshot, AgentState, AgentWorkspace, StatusCause};
use amx_proto::control::{Call, Method, agent, session, stream, workspace};
use amx_proto::rpc::RpcError;
use amx_proto::stream::StreamKind;
use serde_json::json;

fn pane() -> amx_core::PaneId {
    "00000000-0000-0000-0000-0000000000a1".parse().unwrap()
}

/// D-M3-7: `stream.bind` grows an optional `generation`, and a bind without one
/// is byte-for-byte the call M0 froze.
#[test]
fn stream_bind_with_generation_reads_at_v1_and_without_it_still_parses() {
    // A re-bind after a reconnect claims what this client last saw.
    let resumed = Call::decode(
        "stream.bind",
        Some(json!({ "kind": "pane_grid", "pane": pane(), "generation": 7 })),
    )
    .expect("a bind carrying a generation is a v1 call");
    assert_eq!(resumed.method(), Method::StreamBind);
    let Call::StreamBind(resumed) = resumed else {
        panic!("decoded the wrong variant");
    };
    assert_eq!(resumed.kind, StreamKind::PaneGrid { pane: pane() });
    assert_eq!(
        resumed.generation,
        Some(GridGeneration::from_raw(7)),
        "the claimed generation is what decides keyframe-or-nothing",
    );

    // A first bind — every bind in the tree today — omits it, and decodes to
    // the `None` that means "send me whatever you would have sent before".
    let fresh = Call::decode(
        "stream.bind",
        Some(json!({ "kind": "pane_grid", "pane": pane() })),
    )
    .expect("a bind without a generation is the same v1 call");
    let Call::StreamBind(fresh) = fresh else {
        panic!("decoded the wrong variant");
    };
    assert_eq!(fresh.kind, resumed.kind);
    assert_eq!(fresh.generation, None);

    // And re-encoding that one produces the bytes a pre-M3 peer parses: the
    // key is absent, not present-and-null, which is the difference between an
    // additive field and a wire change.
    let encoded = Call::StreamBind(fresh).params().expect("re-encode");
    assert_eq!(
        encoded,
        json!({ "kind": "pane_grid", "pane": pane() }),
        "an absent generation must serialize to nothing at all",
    );

    // The other direction of the tolerance rule, on the same row: a field this
    // build has never heard of is ignored rather than fatal.
    let from_the_future = Call::decode(
        "stream.bind",
        Some(json!({
            "kind": "pane_grid",
            "pane": pane(),
            "generation": 7,
            "predictive_echo": { "horizon_ms": 40 },
        })),
    )
    .expect("an unknown field is dropped, never a decode failure");
    assert_eq!(from_the_future.method(), Method::StreamBind);
}

/// The handoff-attempt row on `session.report` is additive in both directions.
///
/// A server that has never been asked to hand itself over writes the report M1
/// wrote; one that has answers with the row, and a pre-M3 reader would drop it
/// by the same contract that lets this build read a v2 field it has never seen.
#[test]
fn the_session_report_handoff_row_is_additive_in_both_directions() {
    let quiet = session::ReportReply {
        seq: 41,
        report: session::RestoreReport::default(),
        handoff: None,
    };
    assert_eq!(
        serde_json::to_value(&quiet).expect("encode"),
        json!({ "seq": 41, "report": { "entries": [] } }),
        "a server that has seen no handoff writes the bytes M1 wrote",
    );

    let attempted = session::ReportReply {
        seq: 42,
        report: session::RestoreReport::default(),
        handoff: Some(session::HandoffAttempt {
            outcome: session::HandoffOutcome::Aborted,
            stage: session::HandoffStage::Manifest,
            binary: "/tmp/amx".into(),
            reason: Some("manifest version outside the read window".into()),
        }),
    };
    let round_tripped: session::ReportReply =
        serde_json::from_value(serde_json::to_value(&attempted).expect("encode")).expect("decode");
    assert_eq!(round_tripped, attempted);

    // A report from a build that has never heard of the row still parses here.
    let older: session::ReportReply =
        serde_json::from_value(json!({ "seq": 9, "report": { "entries": [] } })).expect("decode");
    assert_eq!(older.handoff, None);
}

/// D-M3-10: `workspace.create` grows an optional `worktree` block, and a create
/// without one is byte-for-byte the call M0 froze.
///
/// The block is how `amx work <branch>` tells the server both facts it needs at
/// once — the membership `done` and restore read back, and the directory the new
/// workspace's shell opens in. Every other caller sends no worktree, which is
/// why the direction that matters most here is the *absent* one.
#[test]
fn workspace_create_with_a_worktree_reads_at_v1_and_without_it_still_parses() {
    let plain = workspace::CreateParams {
        label: Some("scratch".to_owned()),
        focus: true,
        worktree: None,
    };
    assert_eq!(
        serde_json::to_value(&plain).expect("encode"),
        json!({ "label": "scratch", "focus": true }),
        "a create with no worktree writes the bytes M0 wrote",
    );

    let on_a_tree = workspace::CreateParams {
        label: Some("feat".to_owned()),
        focus: true,
        worktree: Some(amx_core::Worktree {
            repo: "/src/amx".into(),
            branch: "feat".to_owned(),
            path: "/src/amx--feat".into(),
        }),
    };
    let encoded = serde_json::to_value(&on_a_tree).expect("encode");
    assert_eq!(
        encoded["worktree"],
        json!({ "repo": "/src/amx", "branch": "feat", "path": "/src/amx--feat" }),
        "the block rides as the three fields `amx work done` and restore join on",
    );
    let round_tripped: workspace::CreateParams = serde_json::from_value(encoded).expect("decode");
    assert_eq!(round_tripped, on_a_tree);

    // A create from a build that has never heard of the field still decodes,
    // to the `None` that means "an ordinary workspace".
    let older: workspace::CreateParams =
        serde_json::from_value(json!({ "focus": false })).expect("decode");
    assert_eq!(older.worktree, None);
    assert_eq!(older, workspace::CreateParams::default());
}

/// The stages are ordered as the protocol runs, so "it got at least as far as
/// X" is a comparison rather than a table.
///
/// Pinned because the ordering is a promise the derive makes silently: a
/// variant inserted in the wrong place would reorder it without a word.
#[test]
fn the_handoff_stages_are_ordered_as_the_protocol_runs() {
    use session::HandoffStage::{
        Commit, Descriptors, Manifest, PreFlight, Quiesce, Ready, Restore, Retire,
    };

    let ordered = [
        PreFlight,
        Quiesce,
        Manifest,
        Descriptors,
        Restore,
        Retire,
        Ready,
        Commit,
    ];
    for pair in ordered.windows(2) {
        assert!(
            pair[0] < pair[1],
            "{:?} must precede {:?}",
            pair[0],
            pair[1]
        );
    }

    // And every stage's wire name is the snake_case of what §3 calls it, so a
    // `session report` row reads like the protocol document.
    assert_eq!(
        serde_json::to_value(PreFlight).expect("encode"),
        json!("pre_flight")
    );
    assert_eq!(
        serde_json::to_value(Descriptors).expect("encode"),
        json!("descriptors")
    );
}

/// The `session.handoff` reply says only whether the protocol *started*.
///
/// Frozen as a shape here because the distinction is the whole of D-M3-8's
/// caller contract: the connection that asked dies at gateway retirement, so a
/// reply that claimed completion would be claiming something it cannot know.
#[test]
fn a_handoff_reply_carries_acceptance_and_a_reason_only_when_refused() {
    let accepted = session::HandoffReply {
        accepted: true,
        reason: None,
        seq: 41,
    };
    assert_eq!(
        serde_json::to_value(&accepted).expect("encode"),
        json!({ "accepted": true, "seq": 41 }),
    );

    let refused = session::HandoffReply {
        accepted: false,
        reason: Some("no overlapping handoff window".into()),
        seq: 41,
    };
    let bytes = serde_json::to_value(&refused).expect("encode");
    assert_eq!(bytes["accepted"], json!(false));
    assert!(bytes["reason"].is_string(), "a refusal says why: {bytes}");

    // Params: the timeout is optional and absent means the server's own budget.
    let minimal: stream::BindReply = serde_json::from_value(json!({
        "stream": 1, "channel": 1, "max_frame": 1024
    }))
    .expect("decode");
    assert_eq!(minimal.channel, 1);
    let params: session::HandoffParams =
        serde_json::from_value(json!({ "binary": "/tmp/amx" })).expect("decode");
    assert_eq!(params.timeout_ms, None);
}

/// D-M3-11: `session.state`'s pane rows grow an optional `cwd`, and a row
/// without one is byte-for-byte the row M0 froze.
///
/// The reader is `amx layout export`, which wrote no `cwd` at all until this
/// field existed — D-M3-11 says the reply "already carries … cwds" and four of
/// its five were there.
#[test]
fn a_pane_state_cwd_reads_at_v1_and_without_it_still_parses() {
    let row = session::PaneState {
        pane: pane(),
        short: amx_core::ShortNumber::FIRST,
        label: None,
        cwd: Some("/home/s/amx".into()),
        rows: 24,
        cols: 80,
        history_head: amx_core::RowId::from_raw(0),
        history_floor: amx_core::RowId::from_raw(0),
        agent: None,
        mouse: None,
    };
    let bytes = serde_json::to_value(&row).expect("encode");
    assert_eq!(bytes["cwd"], json!("/home/s/amx"));
    let round_tripped: session::PaneState = serde_json::from_value(bytes).expect("decode");
    assert_eq!(round_tripped, row);

    // A pane the server has recorded no cwd for produces the bytes it always
    // produced: the key is absent, not null.
    let bare = serde_json::to_value(session::PaneState { cwd: None, ..row }).expect("encode");
    assert!(
        bare.get("cwd").is_none(),
        "an unset cwd is a key nobody sees: {bare}",
    );

    // And a row from a build that has never heard of the field still decodes.
    let older: session::PaneState = serde_json::from_value(json!({
        "pane": pane(), "short": 1, "rows": 24, "cols": 80,
        "history_head": 0, "history_floor": 0
    }))
    .expect("decode");
    assert_eq!(older.cwd, None);
}

// ------------------------------------------------------------------ M4's five

/// D-M4-1 and F-2: `session.state`'s pane rows grow an optional `mouse` block,
/// and a pane that asked for nothing is byte-for-byte the row M3 froze.
///
/// The block is two fields and not a boolean, and that is the correction the
/// spike forced (`docs/notes/m4-mouse-path.md` F-2): a pane that enabled
/// `?1000` without `?1006` expects the X10 encoding, so a reader deciding
/// whether to forward SGR bytes has to see the format as well as the event set.
#[test]
fn a_pane_state_mouse_block_reads_at_v1_and_without_it_still_parses() {
    let row = session::PaneState {
        pane: pane(),
        short: amx_core::ShortNumber::FIRST,
        label: None,
        cwd: None,
        rows: 24,
        cols: 80,
        history_head: amx_core::RowId::from_raw(0),
        history_floor: amx_core::RowId::from_raw(0),
        agent: None,
        mouse: Some(session::MouseMode {
            events: session::MouseEvents::Button,
            format: session::MouseFormat::Sgr,
        }),
    };
    let bytes = serde_json::to_value(&row).expect("encode");
    assert_eq!(
        bytes["mouse"],
        json!({ "events": "button", "format": "sgr" }),
        "both halves travel, because forwarding needs both",
    );
    let round_tripped: session::PaneState = serde_json::from_value(bytes).expect("decode");
    assert_eq!(round_tripped, row);

    // Every pane running a shell: the key is absent, not null, and not a
    // `none` variant every reader would have to handle.
    let bare = serde_json::to_value(session::PaneState { mouse: None, ..row }).expect("encode");
    assert!(
        bare.get("mouse").is_none(),
        "a pane that asked for nothing writes the bytes M3 wrote: {bare}",
    );

    // And a row from a server that has never heard of the field decodes to the
    // same absence, which is why "no tracking" and "no answer" may collapse:
    // both mean do not forward, and that is the only decision made with it.
    let older: session::PaneState = serde_json::from_value(json!({
        "pane": pane(), "short": 1, "rows": 24, "cols": 80,
        "history_head": 0, "history_floor": 0
    }))
    .expect("decode");
    assert_eq!(older.mouse, None);

    // The wire names are the terminal's own, so a mode read out of
    // libghostty-vt means the same thing on both sides of the socket.
    for (events, name) in [
        (session::MouseEvents::X10, "x10"),
        (session::MouseEvents::Normal, "normal"),
        (session::MouseEvents::Button, "button"),
        (session::MouseEvents::Any, "any"),
    ] {
        assert_eq!(serde_json::to_value(events).expect("encode"), json!(name));
    }
    for (format, name) in [
        (session::MouseFormat::X10, "x10"),
        (session::MouseFormat::Utf8, "utf8"),
        (session::MouseFormat::Sgr, "sgr"),
        (session::MouseFormat::Urxvt, "urxvt"),
        (session::MouseFormat::SgrPixels, "sgr_pixels"),
    ] {
        assert_eq!(serde_json::to_value(format).expect("encode"), json!(name));
    }
}

/// D-M4-3 and D-M4-4 ride `session.state` too, because the status line renders
/// the breakdown and must not need a second call to do it (D-M4-5).
///
/// `last_line` deliberately does **not**: it is `agent.list`'s alone.
#[test]
fn the_agent_block_on_session_state_carries_reason_and_since_and_no_last_line() {
    let snapshot = AgentSnapshot {
        kind: None,
        state: AgentState::Blocked,
        cause: StatusCause::Screen,
        transition_seq: 41,
        attention: Some(0),
        session_ref: None,
        reason: Some("permission_dialog".to_owned()),
        since: Some(1_754_650_000_000),
    };
    let bytes = serde_json::to_value(&snapshot).expect("encode");
    assert_eq!(bytes["reason"], json!("permission_dialog"));
    assert_eq!(bytes["since"], json!(1_754_650_000_000_u64));
    assert!(
        bytes.get("last_line").is_none(),
        "screen contents ride `agent.list` and nothing else (D-M4-5): {bytes}",
    );

    // The absences, and a snapshot from a pre-M4 peer decoding to them. This is
    // the direction that lets `since` cross a handoff manifest and a persisted
    // session without a version of its own (R-M4-4).
    let quiet = AgentSnapshot::unidentified(7);
    let bytes = serde_json::to_value(&quiet).expect("encode");
    assert!(bytes.get("reason").is_none(), "{bytes}");
    assert!(bytes.get("since").is_none(), "{bytes}");
    let older: AgentSnapshot =
        serde_json::from_value(json!({ "state": "quiet", "cause": "probe", "transition_seq": 7 }))
            .expect("decode");
    assert_eq!(older, quiet);
}

/// `agent.next` grows an optional `workspace`, and an unscoped call writes the
/// bytes M2 froze.
#[test]
fn a_scoped_next_attention_reads_at_v1_and_an_unscoped_one_is_unchanged() {
    let unscoped = Call::AgentNext(agent::NextParams { workspace: None })
        .params()
        .expect("encode");
    assert_eq!(
        unscoped,
        json!({}),
        "the prefix key's call is the call it always was",
    );

    let workspace: amx_core::WorkspaceId = "00000000-0000-0000-0000-0000000000b1"
        .parse()
        .expect("a workspace id");
    let scoped = Call::decode("agent.next", Some(json!({ "workspace": workspace })))
        .expect("a scoped next is a v1 call");
    let Call::AgentNext(scoped) = scoped else {
        panic!("decoded the wrong variant");
    };
    assert_eq!(scoped.workspace, Some(workspace));

    // And an empty object still decodes, which is what the
    // struct-rather-than-unit shape was chosen for three milestones ago: the
    // field arrived and the wire shape did not change from `{}` to something
    // else, so a caller written against M2 keeps working unchanged.
    let bare = Call::decode("agent.next", Some(json!({}))).expect("an unscoped call");
    assert_eq!(bare.method(), Method::AgentNext);
    let Call::AgentNext(bare) = bare else {
        panic!("decoded the wrong variant");
    };
    assert_eq!(bare.workspace, None);
}

/// `agent.list`'s reply is optional-heavy on purpose, and every absence means
/// something a renderer has to be ready for.
#[test]
fn an_agent_list_row_says_what_it_does_not_know_rather_than_guessing() {
    let workspace: amx_core::WorkspaceId = "00000000-0000-0000-0000-0000000000b1"
        .parse()
        .expect("a workspace id");
    let sparse = agent::AgentEntry {
        workspace: AgentWorkspace::unnamed(workspace),
        pane: pane(),
        name: None,
        kind: None,
        status: AgentState::Quiet,
        reason: None,
        since: None,
        last_line: String::new(),
    };
    let bytes = serde_json::to_value(&sparse).expect("encode");
    assert_eq!(
        bytes,
        json!({
            "workspace": { "id": workspace },
            "pane": pane(),
            "status": "quiet",
            "last_line": "",
        }),
        "an unnamed pane in an unlabelled workspace writes four keys, and \
         `last_line` is one of them even when the screen is blank",
    );
    let round_tripped: agent::AgentEntry = serde_json::from_value(bytes).expect("decode");
    assert_eq!(round_tripped, sparse);

    // An empty session answers with an empty reply rather than an absent one.
    let empty = agent::ListReply {
        seq: 41,
        now: 1_754_650_000_000,
        attention: Vec::new(),
        agents: Vec::new(),
    };
    assert_eq!(
        serde_json::to_value(&empty).expect("encode"),
        json!({ "seq": 41, "now": 1_754_650_000_000_u64 }),
    );
    let round_tripped: agent::ListReply =
        serde_json::from_value(json!({ "seq": 41, "now": 1_754_650_000_000_u64 })).expect("decode");
    assert_eq!(round_tripped, empty);

    // And a request writes the scope key only when it is scoped.
    assert_eq!(
        Call::AgentList(agent::ListParams { workspace: None })
            .params()
            .expect("encode"),
        json!({}),
    );
    assert_eq!(
        Call::AgentList(agent::ListParams {
            workspace: Some(workspace)
        })
        .params()
        .expect("encode"),
        json!({ "workspace": workspace }),
    );
}

/// DR-16's retriable code is amx's second, and it is not the first.
///
/// A caller branches on the number, so the two must never collide: one means
/// "your question is still open, redial and ask it again" and the other means
/// "ask again in a moment, the session is busy becoming a different one".
#[test]
fn the_retriable_code_is_its_own_number_inside_amxs_reserved_range() {
    assert_eq!(RpcError::RETRIABLE, -32001);
    assert_ne!(RpcError::RETRIABLE, RpcError::WAIT_ABANDONED);
    for code in [RpcError::RETRIABLE, RpcError::WAIT_ABANDONED] {
        assert!(
            (-32099..=-32000).contains(&code),
            "{code} is outside JSON-RPC's implementation-defined server range",
        );
        for reserved in [
            RpcError::INVALID_REQUEST,
            RpcError::METHOD_NOT_FOUND,
            RpcError::INVALID_PARAMS,
            RpcError::INTERNAL_ERROR,
        ] {
            assert_ne!(code, reserved, "amx must not reuse a JSON-RPC 2.0 code");
        }
    }
}
