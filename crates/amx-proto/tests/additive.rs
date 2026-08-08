//! M3's additive fields, and the both-directions tolerance that lets them ride
//! protocol v1 (R-M1-8, `docs/09-m3-plan.md` §4). M4's are in [`m4`], under the
//! same rule.
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
use amx_proto::control::{Call, Method, session, stream, workspace};
use amx_proto::stream::StreamKind;
use serde_json::json;

#[path = "additive/m4.rs"]
mod m4;

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
