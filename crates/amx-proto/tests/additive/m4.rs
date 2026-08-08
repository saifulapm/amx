//! M4's additive fields, and the both-directions tolerance that lets them ride
//! protocol v1 (`docs/11-m4-plan.md` §3).
//!
//! A child module of [`super`] rather than a second suite, so the two
//! milestones share one statement of what "additive" has to mean — see that
//! file's header. Split out because M3's half and M4's half change for
//! different reasons and together they crossed the module budget.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::agent::{AgentSnapshot, AgentState, AgentWorkspace, StatusCause};
use amx_proto::control::{Call, Method, agent, session};
use amx_proto::rpc::RpcError;
use serde_json::json;

use super::pane;

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
