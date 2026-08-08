//! W14: M3's exit criterion, over the real binary (`docs/09-m3-plan.md` §7).
//!
//! The roadmap's sentence is "upgrade amx under 5 running agents — none die, no
//! visible screen content lost, waits keep waiting. Attach to the home machine
//! from a laptop over SSH." §7 turns it into six numbered steps and this file is
//! those steps, in order, against a real `amx server`, a real `amx attach` on a
//! real pseudoterminal, a real standing `amx wait`, a real `amx events --json`
//! relay, and a real `amx session handoff` aimed at the binary the suite itself
//! was built from.
//!
//! **Current-vs-current**, and labelled as such. Only one version of amx
//! exists, so the successor is a build of the same tree — exactly as the M0 skew
//! harness has run since M0, and exactly what D-M3-6's *window* check makes
//! legitimate rather than a coincidence. What that proves is that the protocol,
//! the manifest and the descriptors survive a process change; what it cannot
//! prove is that they survive a *version* change, and nothing here claims to.
//!
//! **What stands in.** The agents, for the reason M2's exit suite gives
//! (R-M2-8): CI runners have no agent tooling, so the five agents are the
//! scripted stand-ins in `tests/support/agent.rs`, whose hook payloads are V01's
//! recordings and whose screens are matched by the shipped `claude.toml`. What
//! is real here is everything the swap touches: five pty children, five
//! terminals, five grids with styled content on them, and the descriptors that
//! carry them across.
//!
//! **What green here does not mean.** Green tests do not exit this milestone —
//! `docs/notes/m3-live-smoke.md` is the other half, and the record now says so
//! four times over.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

#[path = "agents/fixtures.rs"]
mod fixtures;
#[path = "handoff_exit/harness.rs"]
mod harness;

use rig::agent;
use rig::{pids_with_arg, rasterize_styled, shows, styled_run};
use serde_json::{Value, json};

use fixtures::Rig;
use harness::non_blank;
use harness::{
    Standing, a_silent_successor, alive, exclusive, gapped, history_window, not_an_amx,
    paint_sentinels, seed_three_workspaces, sentinel_of, seqs, sorted, wait_for_repaint,
    wait_for_successor, wait_report,
};

/// The client the exit criterion attaches: §7 step 1's 200×50.
const CLIENT_ROWS: u16 = 50;
/// See [`CLIENT_ROWS`].
const CLIENT_COLS: u16 = 200;

/// How many columns of a sentinel are compared cell for cell.
///
/// `AMX-SENTINEL ` plus a two-character tag is fifteen; twenty covers it with
/// room, and every column past the text is a blank the paint left behind —
/// worth comparing too, because a blank that comes back carrying the wrong
/// background is exactly the fidelity bound W04 recorded (R-M3-2).
const SENTINEL_WIDTH: usize = 20;

/// How many non-blank characters the client's screen must hold to be worth
/// comparing at all.
///
/// The T19 lesson, taken literally: two screens that are both blank compare
/// equal and prove nothing, so every screen assertion here is paired with a
/// claim that the screen demonstrably held content. Five agent panes at 200×50
/// paint thousands; two hundred is a floor that only a genuinely empty client
/// falls through.
const MIN_PAINTED: usize = 200;

// ------------------------------------------------------------ the exit test

/// §7 steps 1–4: five agents, one client, one swap, and nothing notices.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn five_agents_ride_a_live_upgrade_and_nothing_notices() {
    let _one_at_a_time = exclusive().await;
    let mut rig = Rig::start("m3x").await;
    let agents = seed_three_workspaces(&mut rig).await;
    paint_sentinels(&mut rig, &agents).await;

    // A real client, on a real terminal, at the size §7 names. It is attached
    // *before* the swap and never touched again: whether it comes back by
    // itself is W09's half of the criterion.
    let mut client = rig.env.attach_on_tty(&[], CLIENT_ROWS, CLIENT_COLS);
    // Rasterized rather than byte-matched: the client addresses the cursor
    // between cells, so a phrase that is contiguous on the screen need not be
    // contiguous in the stream that painted it.
    client.wait_output("the client to paint a sentinel", |bytes| {
        shows(bytes, agent::SENTINEL_TEXT)
    });
    let painted_before = rasterize_styled(client.output());
    let sentinel_before = styled_run(&painted_before, agent::SENTINEL_TEXT, SENTINEL_WIDTH)
        .expect("the client painted a sentinel before the swap");
    assert!(
        non_blank(client.output()) > MIN_PAINTED,
        "the client's screen must demonstrably hold content before it is \
         compared against anything"
    );

    // §7 step 2: a wait standing on a pane that has not blocked, from a second
    // connection, and a relay recording sequence numbers.
    let watched = agents[4].clone();
    let mut standing = Standing::start(
        &rig.env,
        "wait",
        &[
            "wait",
            "--until",
            "blocked",
            "--target",
            &watched.name,
            // The caller's own patience, and generous. Without one, W09 gives a
            // re-issued call a ten-second redial window measured from the first
            // attempt — enough for a swap on an idle machine, and observed too
            // tight for one on a loaded runner, where the freeze, the transfer,
            // the successor's assembly and the socket changing hands all
            // compete for the same cores. The wait under test is the standing
            // one, not the default.
            "--timeout",
            "120s",
        ],
    );
    let state = rig.state().await;
    let from = state["seq"]
        .as_u64()
        .expect("state carries its capture seq");
    let relay = Standing::start(
        &rig.env,
        "events",
        &["events", "--json", "--after-seq", &from.to_string()],
    );
    assert!(
        standing.running(),
        "the wait returned before anything blocked"
    );

    // What must survive: five children, the session's identity, the sequence
    // space, and the row ids a client already holds.
    let marker = rig.agents.program().to_string_lossy().into_owned();
    let children = sorted(&pids_with_arg(&marker));
    assert_eq!(
        children.len(),
        5,
        "five scripted agents should be running: {children:?}"
    );
    let before = rig.reconnect().await;
    let rows_before = history_window(&mut rig, agents[0].pane).await;
    // How much the client had painted before the swap. Its output buffer is
    // append-only, so "it repainted" is "the stream grew after the server it
    // was talking to stopped existing" — and without that, comparing the
    // rasterized screen either side would be comparing bytes written before
    // the swap against themselves.
    let painted_bytes = client.output().len();
    let exporter = rig.take_server();
    let exporter_pid = exporter.pid();

    // ------------------------------------------------------------ the swap
    let handed = rig
        .call(
            "session.handoff",
            json!({ "binary": rig.env.exe().to_string_lossy() }),
        )
        .await;
    assert_eq!(handed["accepted"], json!(true), "{handed}");

    // §7 step 4, in order. First: the successor answers on the same socket,
    // with the same `SessionId` and a larger sequence.
    //
    // Waited for by **peer pid**, not by "the socket answers": §3 keeps the
    // exporter's gateway alive through `restored` precisely so that an `amx`
    // typed mid-handoff finds a live server rather than daemonizing a rival,
    // so "something answers" is true from before the swap starts. A different
    // process on the same path is the only observation that means the swap
    // landed — the same one `amx update apply` polls on.
    wait_for_successor(&rig.env.socket(), exporter_pid).await;
    let after = rig.reconnect().await;
    assert_eq!(
        after.session, before.session,
        "a client that saw a different SessionId would drop every cache it \
         carried across, which is the opposite of a handoff"
    );
    assert!(
        after.seq > before.seq,
        "the successor's bus restarted rather than continued: {} then {}",
        before.seq,
        after.seq
    );

    // Every child pid alive, and the same ones. Not a count: five before and
    // five after is also what a session that killed five and started five
    // would look like.
    assert_eq!(
        sorted(&pids_with_arg(&marker)),
        children,
        "the swap did not keep the same five processes"
    );

    // Every sentinel, on the server's side, still on its pane.
    for a in &agents {
        let screen = rig.screen(a.pane).await;
        assert!(
            screen.iter().any(|row| row.contains(&sentinel_of(a))),
            "{}'s sentinel did not cross: its screen is\n{}",
            a.name,
            screen.join("\n"),
        );
    }

    // And on the client's, which reconnected by itself and repainted — styled
    // identically, cell for cell, and demonstrably not blank. Nothing touched
    // this client: the connection it had died with the exporter's gateway, and
    // coming back is its own doing (D-M3-7).
    client.wait_output("the client to repaint after the swap", |bytes| {
        bytes.len() > painted_bytes
    });
    let painted_after = wait_for_repaint(&mut client, sentinel_before.len());
    assert_eq!(
        painted_after, sentinel_before,
        "the client's own screen lost the sentinel's styling across the swap"
    );
    assert!(
        non_blank(client.output()) > MIN_PAINTED,
        "the client repainted a blank screen, which would compare equal to \
         nothing and prove nothing"
    );

    // The live path works afterwards too, which a repaint alone does not say:
    // a fresh paint in the pane this client is looking at has to reach its
    // screen through a stream bound to the successor.
    let watching = agents
        .iter()
        .find(|a| a.workspace == agents[2].workspace)
        .expect("the client's workspace holds an agent");
    rig.drive(&watching.name, &format!("{} after", agent::SENTINEL))
        .await;
    rig.wait_screen(watching.pane, &format!("{} after", agent::SENTINEL_TEXT))
        .await;
    let fresh = format!("{} after", agent::SENTINEL_TEXT);
    client.wait_output("the client to show a paint made after the swap", |bytes| {
        shows(bytes, &fresh)
    });

    // §7 step 4: the fake agent then blocks, and the standing wait — issued
    // against a server that no longer exists — returns satisfied.
    rig.block(&watched).await;
    let waited = standing.finish();
    assert_eq!(waited.code, Some(0), "the standing wait failed: {waited:?}");
    let reply: Value = serde_json::from_str(waited.stdout.trim())
        .unwrap_or_else(|err| panic!("the wait printed {:?} ({err})", waited.stdout));
    assert_eq!(
        reply["satisfied"],
        json!(true),
        "the wait kept waiting through the swap and then answered: {reply}"
    );

    // The relay's stream: gapless, or gap-marked, and never silently short.
    let recorded = relay.stop();
    let deliveries = harness::parse(&recorded.stdout);
    let sequence = seqs(&deliveries);
    assert!(
        !sequence.is_empty(),
        "the relay recorded nothing across a swap that published: {:?}",
        recorded.stdout
    );
    assert!(
        sequence.windows(2).all(|pair| pair[1] > pair[0]),
        "the relay's sequences went backwards or repeated: {sequence:?}"
    );
    if !gapped(&deliveries) {
        assert!(
            sequence.windows(2).all(|pair| pair[1] == pair[0] + 1),
            "the relay's stream is neither contiguous nor gap-marked, which is \
             the one outcome the bus contract forbids: {sequence:?}"
        );
    }

    // The old server exited zero, inside its own post-commit drain bound.
    let drained = exporter.shutdown_within(rig::PATIENCE);
    assert!(
        drained.is_ok(),
        "the exporter did not finish its drain: {drained:?}"
    );
    assert!(!alive(exporter_pid), "pid {exporter_pid} is still running");

    // The row ids a pre-swap fetch returned still address the same rows. What
    // is compared is the *window* `session.state` reports, which is the only
    // history surface this rig speaks — a byte-level refetch needs a bound
    // `pane.history` stream and nothing here has one. It is still the claim
    // that matters: the head did not move (nothing committed in between) and
    // the floor did not rise past it, so every id the exporter issued is one
    // the successor can still be asked for.
    let rows_after = history_window(&mut rig, agents[0].pane).await;
    assert_eq!(
        rows_after.0, rows_before.0,
        "the row-id space restarted across the swap"
    );
    assert!(
        rows_after.1 <= rows_before.0,
        "the successor evicted rows the exporter had issued: floor {} against \
         a head of {}",
        rows_after.1,
        rows_before.0
    );

    client.kill_dash_9();
    rig.env.stop();
}

/// §7 step 5: the same session, with the swap replaced by an abort.
///
/// Two injections, both over the real binary and both landing before the
/// commit: a successor that is not an amx at all (refused at the pre-flight,
/// before a pane is touched) and one that answers the capability probe and then
/// never authenticates (aborted at the token stage, after every pane has been
/// quiesced and captured). After each, the session must still be *serving* —
/// answering calls, with panes that still respond to input — and `amx session
/// report` must say what happened.
///
/// The stages further in (manifest, descriptors, restore, retire, ready) need a
/// peer that speaks §3's protocol and then stops, which is a fake importer
/// rather than a binary; W06 drives all seven rows of §3's crash table that way
/// in `crates/amx-server/tests/handoff_export.rs`. What this test adds is the
/// half that suite cannot have: the abort landing on a real session with real
/// agents in it, seen from the outside.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_abort_before_the_commit_leaves_a_session_that_is_still_serving() {
    let _one_at_a_time = exclusive().await;
    let mut rig = Rig::start("m3a").await;
    let agents = seed_three_workspaces(&mut rig).await;
    paint_sentinels(&mut rig, &agents).await;
    let marker = rig.agents.program().to_string_lossy().into_owned();
    let children = sorted(&pids_with_arg(&marker));
    let session = rig.reconnect().await.session;
    let serving = rig.server_pid();

    for (binary, stage, why) in [
        (not_an_amx(&rig), "pre_flight", "handoff capabilities"),
        (a_silent_successor(&rig), "quiesce", "token"),
    ] {
        let refused = rig
            .call(
                "session.handoff",
                json!({ "binary": binary, "timeout_ms": 2_000 }),
            )
            .await;

        // The pre-flight refusal is answered on the call; a stage that fails
        // after the freeze is answered by the *reply* being accepted and the
        // rollback happening behind it, so both are read from the report.
        let _ = refused;
        let report = wait_report(&mut rig, stage).await;
        assert_eq!(
            report["handoff"]["outcome"],
            json!(if stage == "pre_flight" {
                "refused"
            } else {
                "aborted"
            }),
            "the report says what became of the attempt: {report}"
        );
        assert!(
            report["handoff"]["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains(why)),
            "the reason names what went wrong: {report}"
        );

        // Still this server, still these children, still serving: the reply to
        // an ordinary call is the definition of the first, and a pane that
        // answers input is the definition of the third.
        assert_eq!(
            rig.reconnect().await.session,
            session,
            "the session changed hands after an abort"
        );
        assert_eq!(rig.server_pid(), serving, "a successor took over anyway");
        assert_eq!(
            sorted(&pids_with_arg(&marker)),
            children,
            "the rollback did not put every child back"
        );
        for a in &agents {
            rig.drive(&a.name, agent::PROMPT).await;
            rig.wait_status(a.pane, "working").await;
            rig.drive(&a.name, agent::STOP).await;
            rig.wait_status(a.pane, "idle").await;
        }
    }

    rig.env.stop();
}

/// §7 step 6, everywhere: the bridge as a child, against a live session.
///
/// Tier 1 of D-M3-9 — `amx _bridge` spoken over a socketpair, no ssh involved,
/// every platform. W11 pins the transport itself (`crates/amx/tests/bridge.rs`,
/// and the whole conformance table over it in `tests/skew.rs`); what this adds
/// is that it works against the session this milestone is actually about, with
/// five agents in it and their panes rendering.
///
/// Tier 2 — the same thing over a real ssh connection to 127.0.0.1 — is
/// `tests/remote_ssh.rs`, gated on `AMX_TEST_SSHD` and Linux-only (R-M3-6).
/// Tier 3, a genuinely different machine, is the live smoke's and no CI's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_bridge_reaches_a_session_full_of_agents_and_drives_a_verb() {
    let _one_at_a_time = exclusive().await;
    let mut rig = Rig::start("m3b").await;
    let agents = seed_three_workspaces(&mut rig).await;
    paint_sentinels(&mut rig, &agents).await;

    let bridged = rig.env.run(&["_bridge", "--session", &rig.env.session]);
    assert!(
        bridged.code != Some(0) || bridged.stdout.is_empty(),
        "a bridge with nothing on its stdin has nothing to say: {bridged:?}"
    );

    // The bridge as a child of a real client: one end of a socketpair is the
    // bridge's stdio, the other is an ordinary `Session`.
    let mut wire = harness::bridged(&rig.env).await;
    let welcome = wire
        .hello((amx_proto::PROTO_MIN, amx_proto::PROTO_MAX))
        .await;
    assert_eq!(
        welcome.session,
        rig.reconnect().await.session,
        "the bridge reached a different session"
    );

    let response = wire.request("session.state", json!({})).await;
    let state = fixtures::ok(&response).clone();
    assert_eq!(
        state["panes"].as_array().map(Vec::len),
        Some(agents.len() + 3),
        "every pane is visible over the bridge: {state}"
    );
    let response = wire
        .request("pane.read", json!({ "target": agents[0].pane.to_string() }))
        .await;
    let read = fixtures::ok(&response).clone();
    let rows: Vec<String> = read["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row.as_str().unwrap_or_default().to_owned())
        .collect();
    assert!(
        rows.iter()
            .any(|row| row.contains(&sentinel_of(&agents[0]))),
        "a pane renders over the bridge: {rows:?}"
    );

    wire.abandon();
    rig.env.stop();
}
