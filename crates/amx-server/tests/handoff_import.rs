//! W07: the successor assembles from the manifest (`docs/09-m3-plan.md` §3,
//! D-M3-5, D-M3-12).
//!
//! Every test here runs the real import assembly against a real exporter — the
//! W05 state machine, driven by hand where W06's orchestrator will drive it —
//! over a real unix socket, carrying real pty masters with real children on the
//! far end. Nothing about a handoff survives being modelled: the descriptor is
//! the point, the child's liveness is the point, and "the pane did not move
//! before the commit" is a claim about a terminal that a fake terminal cannot
//! make.
//!
//! The exporter's half is scripted rather than orchestrated because the
//! orchestrator is W06's and does not exist yet. What the script does is
//! exactly §3's left-hand column: freeze a pane, capture it, hand the manifest
//! over, hand the descriptors over, retire the session socket at `restored`,
//! and commit.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

#[path = "handoff_import/harness.rs"]
mod harness;
mod support;

use std::os::fd::AsFd;
use std::time::Duration;

use amx_core::agent::{AgentKind, AgentState, HookToken, RefSource};
use amx_core::{Delivery, Event, SessionId, SessionName, ShortNumber};
use amx_proto::control::agent as proto;
use amx_server::handoff::manifest::SessionIdentity;
use amx_server::handoff::protocol::{Ending, HandoffError, HandoffListener, Timeouts};
use harness::{
    Frozen, Plan, Rig, SIZE, SLEEPER, Script, Watcher, agent, brisk, document, manifest, read_pane,
    spawn_exporter, state_of, wait_for_screen,
};
use serde_json::json;
use tokio::sync::oneshot;

// ------------------------------------------------------------------- tests

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manifest_and_fds_become_a_serving_session_with_the_same_session_id() {
    let rig = Rig::new("id");
    let mut frozen = Frozen::new(SLEEPER).await;
    let session = SessionId::new_v4();
    let carried = manifest(session, 4242, &[&frozen], Vec::new());
    let held = rig.hold_socket();
    let listener = HandoffListener::bind(rig.handoff.clone()).expect("bind the handoff socket");

    let exporter = spawn_exporter(Script {
        listener,
        token: rig.token.clone(),
        timeouts: Timeouts::DEFAULT,
        manifest: document(&carried),
        masters: vec![
            frozen
                .master
                .try_clone()
                .expect("a descriptor to hand over"),
        ],
        held: Some(held),
        plan: Plan::Commit,
    });
    let successor = rig.start_import(Timeouts::DEFAULT);
    let mut client = successor.client().await;
    let welcome = client.hello(amx_proto::version::window()).await;

    assert_eq!(
        welcome.session, session,
        "the successor continues the session's identity, or every client drops its caches",
    );
    let state = state_of(&mut client).await;
    assert_eq!(
        state["workspaces"][0]["label"].as_str(),
        Some("carried"),
        "the workspace crossed with its label: {state}",
    );
    assert_eq!(
        state["panes"][0]["pane"].as_str(),
        Some(frozen.pane().to_string().as_str()),
        "the pane crossed under the id it had: {state}",
    );
    assert_eq!(
        state["panes"][0]["rows"].as_u64(),
        Some(u64::from(SIZE.rows)),
        "the pane crossed at the size it was captured at: {state}",
    );

    exporter
        .await
        .expect("the exporter thread ended")
        .expect("the exporter walked every stage");
    frozen.retire().await;
    successor.stop().await;
    frozen.kill_child();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn panes_stay_quiescent_until_committed_and_resume_after() {
    let rig = Rig::new("quie");
    // The child answers exactly once, when something types at its terminal —
    // so "did the successor read the pty?" is a question with a visible answer
    // rather than a race with a clock.
    let mut frozen = Frozen::new("read line; printf 'RESUMED'").await;
    // Typed *before* the successor exists, so the answer is sitting in the
    // kernel's pty buffer for the whole assembly. That is what makes this a
    // test of the closed gate rather than of the quiesce alone: a successor
    // that read its inherited terminal while building itself would have
    // swallowed these bytes, and an abort would then have destroyed them.
    frozen.type_in(b"go\n");
    let carried = manifest(SessionId::new_v4(), 10, &[&frozen], Vec::new());
    let listener = HandoffListener::bind(rig.handoff.clone()).expect("bind the handoff socket");

    // The exporter stops at `ready` and waits, which is the window §3 gives
    // the importer to be bound, visible, and touching nothing.
    let (release, released) = oneshot::channel::<()>();
    let token = rig.token.clone();
    let document = document(&carried);
    let master = frozen
        .master
        .try_clone()
        .expect("a descriptor to hand over");
    let exporter = tokio::task::spawn_blocking(move || -> Result<(), HandoffError> {
        let exporter = listener.accept(token, Timeouts::DEFAULT)?;
        let exporter = exporter.authenticate()?;
        let exporter = exporter.send_manifest(document)?;
        let exporter = exporter.await_validated()?;
        let exporter = exporter.send_masters(&[master.as_fd()])?;
        let exporter = exporter.await_restored()?;
        let exporter = exporter.await_ready()?;
        let _ = released.blocking_recv();
        let exporter = exporter.commit()?;
        exporter.await_owned();
        Ok(())
    });
    let successor = rig.start_import(Timeouts::DEFAULT);
    let mut client = successor.client().await;
    let _ = client.hello(amx_proto::version::window()).await;

    // The pane is served, and it is frozen: the child answered a while ago and
    // nothing may read that answer while the exporter still owns the session.
    tokio::time::sleep(Duration::from_millis(300)).await; // deliberate
    let before = read_pane(&mut client, frozen.pane(), 2).await;
    assert!(
        !before.contains("RESUMED"),
        "an uncommitted successor read its inherited terminal: {before:?}",
    );

    release.send(()).expect("the exporter is waiting");
    let after = wait_for_screen(&mut client, frozen.pane(), |screen| {
        screen.contains("RESUMED")
    })
    .await;
    assert!(
        after.contains("RESUMED"),
        "the committed successor never resumed the pane: {after:?}",
    );

    exporter
        .await
        .expect("the exporter thread ended")
        .expect("the exporter walked every stage");
    frozen.retire().await;
    successor.stop().await;
    frozen.kill_child();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bus_continues_from_the_inherited_seq_and_welcome_reports_it() {
    let rig = Rig::new("seq");
    let mut frozen = Frozen::new(SLEEPER).await;
    let inherited = 9_000;
    let carried = manifest(SessionId::new_v4(), inherited, &[&frozen], Vec::new());
    let listener = HandoffListener::bind(rig.handoff.clone()).expect("bind the handoff socket");
    let exporter = spawn_exporter(Script {
        listener,
        token: rig.token.clone(),
        timeouts: Timeouts::DEFAULT,
        manifest: document(&carried),
        masters: vec![
            frozen
                .master
                .try_clone()
                .expect("a descriptor to hand over"),
        ],
        held: None,
        plan: Plan::Commit,
    });
    let successor = rig.start_import(Timeouts::DEFAULT);
    let mut client = successor.client().await;
    let welcome = client.hello(amx_proto::version::window()).await;

    assert!(
        welcome.seq >= inherited,
        "the welcome reports {} against an inherited head of {inherited}",
        welcome.seq,
    );
    // Subscribed from the exporter's last sequence, which is exactly what a
    // client resuming across the swap asks for: everything the successor has
    // published sits behind it in the replay ring.
    let (mut watcher, _) = Watcher::open(&successor.ctx.socket, Some(inherited)).await;
    let response = client
        .request(2, "workspace.create", json!({ "label": "after" }))
        .await;
    let _ = support::result_of(&response);

    let first = watcher.wait_for(|_| true).await;
    assert_eq!(
        first.seq,
        inherited + 1,
        "the successor's first event is the exporter's last plus one — gapless, never a restart at zero: {:?}",
        first.event,
    );
    let created = watcher
        .wait_for(|event| matches!(event, Event::WorkspaceCreated { .. }))
        .await;
    assert!(
        created.seq > inherited,
        "and the sequence keeps climbing from there: {created:?}",
    );
    let state = state_of(&mut client).await;
    assert!(
        state["seq"].as_u64().unwrap_or_default() > inherited,
        "and `session.state` answers from the same head: {state}",
    );

    exporter
        .await
        .expect("the exporter thread ended")
        .expect("the exporter walked every stage");
    frozen.retire().await;
    successor.stop().await;
    frozen.kill_child();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_inherited_child_exit_is_detected_by_eof_and_reports_unknown_status() {
    let rig = Rig::new("exit");
    // Exits 7 when told to. A pane that spawned this child would report 7; a
    // pane that inherited it must report nothing at all (D-M3-12).
    let mut frozen = Frozen::new("read line; exit 7").await;
    let carried = manifest(SessionId::new_v4(), 1, &[&frozen], Vec::new());
    let listener = HandoffListener::bind(rig.handoff.clone()).expect("bind the handoff socket");
    let exporter = spawn_exporter(Script {
        listener,
        token: rig.token.clone(),
        timeouts: Timeouts::DEFAULT,
        manifest: document(&carried),
        masters: vec![
            frozen
                .master
                .try_clone()
                .expect("a descriptor to hand over"),
        ],
        held: None,
        plan: Plan::Commit,
    });
    let successor = rig.start_import(Timeouts::DEFAULT);
    successor.ready().await;
    let (mut watcher, _) = Watcher::open(&successor.ctx.socket, None).await;
    exporter
        .await
        .expect("the exporter thread ended")
        .expect("the exporter walked every stage");
    // The exporter's own pane host is gone before the child ends, which is what
    // makes this a child no live actor in this process ever forked.
    frozen.retire().await;

    frozen.type_in(b"go\n");
    let event = watcher
        .wait_for(|event| matches!(event, Event::PaneExited { .. }))
        .await
        .event;
    let Event::PaneExited { pane, status } = event else {
        unreachable!("the predicate matched a pane exit")
    };
    assert_eq!(pane, frozen.pane(), "the exit is the inherited pane's");
    assert_eq!(
        status, None,
        "`waitpid` is a parent's call: an inherited child's exit is detected, never invented",
    );

    successor.stop().await;
    frozen.kill_child();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn strict_abort_on_missing_commit_serves_nothing() {
    let rig = Rig::new("abrt");
    let mut frozen = Frozen::new(SLEEPER).await;
    let carried = manifest(SessionId::new_v4(), 3, &[&frozen], Vec::new());
    let listener = HandoffListener::bind(rig.handoff.clone()).expect("bind the handoff socket");
    let exporter = spawn_exporter(Script {
        listener,
        token: rig.token.clone(),
        timeouts: brisk(),
        manifest: document(&carried),
        masters: vec![
            frozen
                .master
                .try_clone()
                .expect("a descriptor to hand over"),
        ],
        held: None,
        plan: Plan::WedgeBeforeCommit,
    });
    let successor = rig.start_import(brisk());
    // It binds, because §3 has it bind before the commit — and then it must
    // give the socket back rather than serve a session nobody handed it.
    let socket = successor.ctx.socket.clone();
    let failure = successor.failure().await;

    assert_eq!(
        failure.ending(),
        Ending::Abort,
        "a commit that never arrived is §3's abort row: {failure}",
    );
    assert!(
        !socket.exists(),
        "the strict abort left {} behind",
        socket.display(),
    );
    assert!(
        tokio::net::UnixStream::connect(&socket).await.is_err(),
        "something is still answering on the session socket",
    );

    let _ = exporter.await;
    frozen.retire().await;
    frozen.kill_child();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manifest_for_another_session_is_refused_before_a_descriptor_is_taken() {
    let rig = Rig::new("wrng");
    let mut frozen = Frozen::new(SLEEPER).await;
    let mut carried = manifest(SessionId::new_v4(), 3, &[&frozen], Vec::new());
    // The exporter is handing over a session this process is not. §3 step 6
    // calls it session-dir identity; what it prevents is one session's panes
    // being served behind another session's socket, which is what an upgrade of
    // a *named* session did before the exporter learned to pass `--session`.
    carried.session_name = Some(SessionIdentity {
        session: SessionName::new("somebody-else").expect("a valid session name"),
        socket: rig.ctx.socket.with_file_name("not-this-one"),
    });
    let listener = HandoffListener::bind(rig.handoff.clone()).expect("bind the handoff socket");
    let exporter = spawn_exporter(Script {
        listener,
        token: rig.token.clone(),
        timeouts: brisk(),
        manifest: document(&carried),
        masters: vec![
            frozen
                .master
                .try_clone()
                .expect("a descriptor to hand over"),
        ],
        held: None,
        plan: Plan::Commit,
    });
    let socket = rig.ctx.socket.clone();
    let failure = rig.start_import(brisk()).failure().await;

    assert_eq!(
        failure.ending(),
        Ending::Abort,
        "a manifest for another session is an abort: {failure}",
    );
    assert!(
        failure.to_string().contains("somebody-else"),
        "the refusal names the session it was handed: {failure}",
    );
    assert!(
        !socket.exists(),
        "it bound {} for a session it was not",
        socket.display(),
    );

    // And the identity a successor *is* passes, which is what makes the check a
    // check rather than a wall: the same comparison, both ways.
    let mine = SessionIdentity::of(&rig.ctx);
    let mut matching = carried.clone();
    matching.session_name = Some(mine.clone());
    assert!(matching.check_session_identity(&mine).is_ok());
    // As does a manifest from an exporter that never carried the field.
    matching.session_name = None;
    assert!(matching.check_session_identity(&mine).is_ok());

    let _ = exporter.await;
    frozen.retire().await;
    frozen.kill_child();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restored_agents_report_their_manifest_status_without_a_flap() {
    let rig = Rig::new("agnt");
    let mut first = Frozen::new(SLEEPER).await;
    let mut second = Frozen::new(SLEEPER).await;
    let inherited = 500;
    // The queue's order is not the manifest's order: the second pane blocked
    // first, and block time is exactly what a screen cannot recover (R-M3-13).
    let agents = vec![
        agent(first.pane(), AgentState::Blocked, Some(1)),
        agent(second.pane(), AgentState::Blocked, Some(0)),
    ];
    let carried = manifest(SessionId::new_v4(), inherited, &[&first, &second], agents);
    let listener = HandoffListener::bind(rig.handoff.clone()).expect("bind the handoff socket");
    let exporter = spawn_exporter(Script {
        listener,
        token: rig.token.clone(),
        timeouts: Timeouts::DEFAULT,
        manifest: document(&carried),
        masters: vec![
            first.master.try_clone().expect("a descriptor to hand over"),
            second
                .master
                .try_clone()
                .expect("a descriptor to hand over"),
        ],
        held: None,
        plan: Plan::Commit,
    });
    let successor = rig.start_import(Timeouts::DEFAULT);
    let mut client = successor.client().await;
    let _ = client.hello(amx_proto::version::window()).await;
    exporter
        .await
        .expect("the exporter thread ended")
        .expect("the exporter walked every stage");

    let state = state_of(&mut client).await;
    let statuses: Vec<(&str, &str)> = state["panes"]
        .as_array()
        .expect("panes")
        .iter()
        .map(|pane| {
            (
                pane["pane"].as_str().unwrap_or_default(),
                pane["agent"]["state"].as_str().unwrap_or("none"),
            )
        })
        .collect();
    assert!(
        statuses.iter().all(|(_, state)| *state == "blocked"),
        "both agents came back blocked, as the exporter last said: {statuses:?}",
    );
    assert_eq!(
        state["attention"]
            .as_array()
            .expect("the attention queue")
            .iter()
            .map(|pane| pane.as_str().unwrap_or_default().to_owned())
            .collect::<Vec<_>>(),
        vec![second.pane().to_string(), first.pane().to_string()],
        "the queue kept the order it crossed in: {state}",
    );

    // The flap, measured. A successor that re-derived these would have
    // published an `agent_identified` and an `agent_status` per pane, and
    // re-entered both at the tail of the attention queue in the order it
    // happened to see them. Subscribing from the exporter's last sequence
    // replays everything this process has published since — and none of it may
    // be about an agent.
    let (mut watcher, _) = Watcher::open(&successor.ctx.socket, Some(inherited)).await;
    let published: Vec<Event> = watcher
        .drain(Duration::from_millis(400))
        .await
        .into_iter()
        .map(|delivery| match delivery {
            Delivery::Event(envelope) => envelope.event,
            Delivery::Gap { from, to } => panic!("the replay ring dropped {from}..={to}"),
        })
        .collect();
    let flapped: Vec<&Event> = published
        .iter()
        .filter(|event| {
            matches!(
                event,
                Event::AgentStatus { .. }
                    | Event::AgentIdentified { .. }
                    | Event::AttentionEnqueued { .. }
                    | Event::AttentionDequeued { .. }
            )
        })
        .collect();
    assert!(
        flapped.is_empty(),
        "the swap announced agent transitions that never happened: {flapped:?}",
    );

    first.retire().await;
    second.retire().await;
    successor.stop().await;
    first.kill_child();
    second.kill_child();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_inherited_agents_hook_reports_are_still_attributed_to_it() {
    let rig = Rig::new("tokn");
    let mut pane = Frozen::new(SLEEPER).await;
    // The token this pane's child was spawned with. It lives in the child's
    // environment and cannot be changed there, so a successor that minted a
    // fresh one would drop every report the agent sends until it restarts.
    let carried = HookToken::new("carried-across-the-swap");
    pane.carry_token(carried.clone());
    let agents = vec![agent(pane.pane(), AgentState::Idle, None)];
    let carried_manifest = manifest(SessionId::new_v4(), 500, &[&pane], agents);
    let listener = HandoffListener::bind(rig.handoff.clone()).expect("bind the handoff socket");
    let exporter = spawn_exporter(Script {
        listener,
        token: rig.token.clone(),
        timeouts: Timeouts::DEFAULT,
        manifest: document(&carried_manifest),
        masters: vec![pane.master.try_clone().expect("a descriptor to hand over")],
        held: None,
        plan: Plan::Commit,
    });
    let successor = rig.start_import(Timeouts::DEFAULT);
    let mut client = successor.client().await;
    let _ = client.hello(amx_proto::version::window()).await;
    exporter
        .await
        .expect("the exporter thread ended")
        .expect("the exporter walked every stage");

    let claude = AgentKind::new("claude").expect("a valid agent kind");
    let report = |token: &HookToken| {
        let params = proto::ReportParams {
            pane: pane.pane(),
            token: token.clone(),
            agent: claude.clone(),
            source: RefSource::for_agent(&claude),
            event: proto::HookEvent::PermissionRequest,
            seq: 1,
            scope: proto::ReportScope::Parent,
            session_id: None,
            transcript_path: None,
            start_source: None,
            tool: None,
            notification: None,
        };
        serde_json::to_value(params).expect("the report serializes")
    };

    let accepted = client.request(7, "agent.report", report(&carried)).await;
    assert_eq!(
        support::result_of(&accepted)["accepted"],
        json!(true),
        "the successor knows the token the inherited child carries: {accepted:?}",
    );

    // The guard is still a guard: a report from somewhere else is still
    // dropped, so what crossed is this pane's token and not a hole.
    let stranger = client
        .request(8, "agent.report", report(&HookToken::new("stranger")))
        .await;
    assert_eq!(
        support::result_of(&stranger)["accepted"],
        json!(false),
        "a foreign token is still nobody's hook: {stranger:?}",
    );

    pane.retire().await;
    successor.stop().await;
    pane.kill_child();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn short_numbers_cross_the_handoff_unchanged_and_the_gaps_stay_free() {
    // 04 §6's numbers are stable across a restart *and* across an upgrade, so
    // a manifest's numbers are adopted verbatim — gaps included. The gaps are
    // what make this more than a copy: the server that wrote them never reused
    // a number, and the successor's first split has to take the lowest one
    // free rather than counting on from the highest.
    let rig = Rig::new("shorts");
    let mut frozen = Frozen::new(SLEEPER).await;
    let session = SessionId::new_v4();
    let mut carried = manifest(session, 11, &[&frozen], Vec::new());
    carried.state.workspaces[0].short = ShortNumber::new(4);
    carried.state.panes[0].short = ShortNumber::new(6);

    let held = rig.hold_socket();
    let listener = HandoffListener::bind(rig.handoff.clone()).expect("bind the handoff socket");
    let exporter = spawn_exporter(Script {
        listener,
        token: rig.token.clone(),
        timeouts: Timeouts::DEFAULT,
        manifest: document(&carried),
        masters: vec![
            frozen
                .master
                .try_clone()
                .expect("a descriptor to hand over"),
        ],
        held: Some(held),
        plan: Plan::Commit,
    });
    let successor = rig.start_import(Timeouts::DEFAULT);
    let mut client = successor.client().await;
    let _ = client.hello(amx_proto::version::window()).await;

    let state = state_of(&mut client).await;
    assert_eq!(
        (
            state["workspaces"][0]["short"].as_u64(),
            state["panes"][0]["short"].as_u64(),
        ),
        (Some(4), Some(6)),
        "the numbers crossed as they were, not renumbered from one: {state}",
    );

    let split = client
        .request(
            40,
            "pane.split",
            json!({ "pane": frozen.pane().to_string(), "direction": "vertical" }),
        )
        .await;
    assert_eq!(
        support::result_of(&split)["short"].as_u64(),
        Some(1),
        "the successor's first split takes the lowest free number, not the one \
         past the highest the exporter had reached: {split:?}",
    );

    exporter
        .await
        .expect("the exporter thread ended")
        .expect("the exporter walked every stage");
    frozen.retire().await;
    successor.stop().await;
    frozen.kill_child();
}
