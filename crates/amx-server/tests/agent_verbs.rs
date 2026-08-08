//! V13: `agent.start`, `agent.prompt` and the addressing both of them take.
//!
//! Three claims, and each test is one of them.
//!
//! **The registry decides what runs.** `agent start dev --kind fake` spawns the
//! stanza's argv and nothing else, appends the caller's extras after it, and
//! makes the pane addressable by giving it the name as its label — D-M2-9's
//! "the agent's name is the pane's label", which is why there is no second
//! namespace and no second rename verb.
//!
//! **Readiness is evidence, not optimism.** A start returns `ready` only once
//! the pane's status names the agent it asked for *and* reads idle, and never
//! before the stanza's startup grace — the window in which the fusion machine
//! itself refuses to read the screen. When the evidence does not arrive, the
//! answer is `timed_out` with the pane **still running**, which is herdr's
//! semantics: a failed handshake is something to go and look at, not something
//! to clean up behind the user's back.
//!
//! **A wait waits for a transition, not for a state.** `agent prompt --wait
//! blocked` sent to a pane that is *already* blocked must not return: the
//! predicate carries a floor, the bus sequence the prompt was submitted at, and
//! `AgentSnapshot::transition_seq` has to be later than it. That is the test
//! that matters, and it is the last one in the file.
//!
//! The agents are `/bin/sh` scripts painting three markers, read by a manifest
//! planted in this session's own override directory (D-M2-2's test seam). Fake
//! screens, real everything else: a real `Core` spawning real children on real
//! ptys, a real hub, the shipped fusion machine, the shipped rule engine.
//! `harness.rs` says why the calls go through `dispatch::handle` rather than a
//! socket, and which wiring gap that is standing in for.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::PaneId;
use serde_json::{Value, json};

#[path = "agent_verbs/harness.rs"]
mod harness;
mod support;

use harness::{
    BLOCKED_MARK, FAKE, IDLE_MARK, IMPOSTOR, Rig, SHY, SLOW, SLOW_GRACE_MS, blocked_from_the_start,
    blocks_after, idles, plant_script,
};

/// Read a pane id out of a reply field.
fn pane_of(reply: &Value, field: &str) -> PaneId {
    reply[field]
        .as_str()
        .unwrap_or_else(|| panic!("{field} is a pane id: {reply}"))
        .parse()
        .expect("a pane id")
}

/// The whole of `agent.start`'s happy path: registry argv, label, readiness.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_start_spawns_from_the_registry_labels_the_pane_and_returns_ready() {
    let mut rig = Rig::start("astr").await;
    let record = rig.recording("argv");
    plant_script(
        rig.scripts(),
        FAKE,
        &format!("printf '%s\\n' \"$@\" > {}; {}", record.display(), idles()),
    );

    let reply = rig
        .call(
            "agent.start",
            json!({ "name": "dev", "kind": FAKE, "args": ["--model", "haiku"] }),
        )
        .await;
    assert_eq!(reply["readiness"], "ready", "{reply}");
    let pane = pane_of(&reply, "pane");

    // The child really ran the stanza's program with the caller's extras after
    // it — argv as data, the registry deciding, not the caller.
    let want = "--model\nhaiku\n";
    let argv = String::from_utf8(rig.recorded(&record, want.len()).await)
        .expect("the fake agent records utf-8");
    assert_eq!(argv, want);

    // The name is the label, which is the whole of D-M2-9. Read off the entry
    // the wait settled on rather than off a fresh one: the `agent` block is
    // `Core`'s mirror of the hub's view and arrives after the reply does, so a
    // state read taken here comes back with the label and no agent at all.
    let state = rig.wait_pane_agent(pane).await;
    assert_eq!(state["label"], "dev");
    assert_eq!(state["agent"]["kind"], FAKE, "{state}");
    assert_eq!(reply["agent"]["state"], "idle", "{reply}");
    rig.stop().await;
}

/// Readiness is identity **and** an observed idle, and never inside the grace.
///
/// The grace assertion is a lower bound on elapsed time, which is the one
/// timing claim that cannot flake: a deadline can be late, never early. It is
/// here because without it a start returns while the agent is still painting
/// its banner — the tracker's opening assumption read back as evidence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readiness_is_identity_confirmed_plus_idle_observed_with_timeout() {
    let mut rig = Rig::start("ardy").await;
    plant_script(rig.scripts(), SLOW, &idles());

    let began = tokio::time::Instant::now();
    let reply = rig
        .call("agent.start", json!({ "name": "slowly", "kind": SLOW }))
        .await;
    let took = began.elapsed();

    assert_eq!(reply["readiness"], "ready", "{reply}");
    assert_eq!(reply["agent"]["kind"], SLOW, "{reply}");
    assert_eq!(reply["agent"]["state"], "idle", "{reply}");
    assert!(
        took >= std::time::Duration::from_millis(SLOW_GRACE_MS),
        "the `{SLOW}` stanza states a {SLOW_GRACE_MS} ms startup grace and the \
         handshake answered inside it, after {took:?}",
    );
    rig.stop().await;
}

/// A handshake that never lands is reported as a failure, and the pane lives.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_timeout_reports_failure_but_leaves_the_pane_running() {
    let mut rig = Rig::start("atmo").await;
    // Named for the stanza's `start`, which its `executables` list does not
    // name: nothing can identify this pane, so readiness has nothing to
    // confirm and the deadline is the only way out.
    plant_script(rig.scripts(), SHY, &idles());

    let reply = rig
        .call(
            "agent.start",
            json!({ "name": "never", "kind": SHY, "timeout_ms": 400 }),
        )
        .await;
    assert_eq!(reply["readiness"], "timed_out", "{reply}");

    // Honest failure, live pane: the caller is told which pane to go and look
    // at, and the pane is still there when they do.
    let pane = pane_of(&reply, "pane");
    let state = rig.pane_state(pane).await;
    assert_eq!(state["label"], "never");
    // Waited for, not read: the readiness deadline this start expired on is the
    // dispatcher's own timer and says nothing about the child, which paints
    // whenever the pty gets to it. The wait fails with the screen, so a pane
    // that really did die still reports what was on it.
    rig.wait_screen(pane, IDLE_MARK).await;

    // The other half of "identity confirmed": a pane that came up, was
    // identified and reads idle — as somebody else. An `impostor` stanza whose
    // `start` runs the fake agent's program is exactly the misconfiguration
    // this catches, and answering `ready` to it would be answering about the
    // wrong agent.
    plant_script(rig.scripts(), FAKE, &idles());
    let reply = rig
        .call(
            "agent.start",
            json!({ "name": "wrong", "kind": IMPOSTOR, "timeout_ms": 400 }),
        )
        .await;
    assert_eq!(reply["readiness"], "timed_out", "{reply}");
    assert_eq!(reply["agent"]["state"], "idle", "{reply}");
    assert_eq!(reply["agent"]["kind"], FAKE, "{reply}");
    rig.stop().await;
}

/// UUID first, then a label unique among agent panes, then an error that names
/// the candidates.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn addressing_resolves_uuid_then_unique_label_and_names_ambiguity() {
    let mut rig = Rig::start("aadr").await;
    plant_script(rig.scripts(), FAKE, &idles());
    // Both starts wait for the state tree to name their agent, because that is
    // what the rules below resolve against: `Scope::Agent` admits a pane only
    // once its *mirrored* status carries a kind, and the mirror lands after the
    // start reply. Without the wait, rule 2 is refused with "no agent is
    // running in it" — about the pane the line above just started.
    let (_, first) = rig.start_agent("one", FAKE).await;
    let (_, second) = rig.start_agent("two", "faux").await;

    // Rule 2: a unique label among agent panes.
    let reply = rig
        .call("agent.prompt", json!({ "target": "two", "text": "hi" }))
        .await;
    assert_eq!(pane_of(&reply, "pane"), second);

    // Rule 1: a UUID wins over a label, even a label spelling another pane's
    // UUID — which is the reason the order is fixed rather than convenient.
    rig.call(
        "pane.rename",
        json!({ "pane": second.to_string(), "label": first.to_string() }),
    )
    .await;
    let reply = rig
        .call(
            "agent.prompt",
            json!({ "target": first.to_string(), "text": "hi" }),
        )
        .await;
    assert_eq!(pane_of(&reply, "pane"), first);

    // Rule 3: two agent panes under one label is an error naming both.
    rig.call(
        "pane.rename",
        json!({ "pane": second.to_string(), "label": "one" }),
    )
    .await;
    let err = rig
        .call_err("agent.prompt", json!({ "target": "one", "text": "hi" }))
        .await;
    assert!(
        err.message.contains(&first.to_string()) && err.message.contains(&second.to_string()),
        "the ambiguity must name its candidates: {}",
        err.message
    );

    // And a name nothing carries says so plainly rather than timing out.
    let err = rig
        .call_err("agent.prompt", json!({ "target": "nobody", "text": "hi" }))
        .await;
    assert!(err.message.contains("nobody"), "{}", err.message);
    rig.stop().await;
}

/// The prompt reaches the child through `pane.run`, and the wait returns on the
/// block that follows it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_prompt_submits_via_run_and_wait_blocked_returns_on_the_next_block() {
    let mut rig = Rig::start("apwb").await;
    let heard = rig.recording("heard");
    let text = "write the tests";
    plant_script(rig.scripts(), FAKE, &blocks_after(text.len(), &heard));

    // The prompt below addresses by label, so the start has to be settled in
    // the state tree and not merely answered: `Scope::Agent` reads the mirror.
    let (start, pane) = rig.start_agent("dev", FAKE).await;
    assert_eq!(start["readiness"], "ready", "{start}");

    let reply = rig
        .call(
            "agent.prompt",
            json!({ "target": "dev", "text": text, "wait": "blocked", "timeout_ms": 10_000 }),
        )
        .await;

    // What the *child* received, not what the server sent.
    assert_eq!(
        String::from_utf8(rig.recorded(&heard, text.len()).await).expect("utf-8"),
        text,
    );
    assert_eq!(reply["satisfied"], true, "{reply}");
    assert_eq!(reply["agent"]["state"], "blocked", "{reply}");
    // The transition that satisfied the wait is later than the submission,
    // which is what "the next block" means.
    let floor = reply["submitted_seq"].as_u64().expect("a floor");
    let landed = reply["agent"]["transition_seq"]
        .as_u64()
        .expect("a transition seq");
    assert!(landed > floor, "{landed} should be later than {floor}");
    assert_eq!(pane_of(&reply, "pane"), pane);
    rig.stop().await;
}

/// A pane that was already blocked cannot satisfy a `--wait blocked`.
///
/// The one that matters. The agent here blocks before the prompt is ever sent
/// and never moves again, so the only thing that could end the wait is the
/// state it was in when the wait began — and the floor is what refuses it. Left
/// out, this test returns `satisfied: true` in microseconds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_wait_uses_transition_seq_so_a_prior_blocked_state_cannot_satisfy_it() {
    let mut rig = Rig::start("apfl").await;
    plant_script(rig.scripts(), FAKE, &blocked_from_the_start());

    // Readiness times out on purpose: this agent is never idle. The pane is
    // running and tracked, which is all the prompt below needs.
    let start = rig
        .call(
            "agent.start",
            json!({ "name": "stuck", "kind": FAKE, "timeout_ms": 400 }),
        )
        .await;
    let pane = pane_of(&start, "pane");
    rig.wait_status(pane, "blocked").await;

    let reply = rig
        .call(
            "agent.prompt",
            json!({ "target": "stuck", "text": "hello", "wait": "blocked", "timeout_ms": 400 }),
        )
        .await;
    assert_eq!(
        reply["satisfied"], false,
        "the block that was already there must not satisfy the wait: {reply}"
    );
    // The pane is still blocked — the wait failed to find a *transition*, not
    // to find the state, and the reply reports the state honestly.
    assert_eq!(reply["agent"]["state"], "blocked", "{reply}");
    let screen = rig.screen(pane).await;
    assert!(
        screen.iter().any(|row| row.contains(BLOCKED_MARK)),
        "{screen:?}"
    );
    rig.stop().await;
}

/// A name that could not be resolved back is refused before anything spawns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_name_no_resolver_could_give_back_is_refused_before_the_spawn() {
    let mut rig = Rig::start("anam").await;
    plant_script(rig.scripts(), FAKE, &idles());
    let before = rig.state().await["panes"].as_array().expect("panes").len();

    for name in ["", "   ", "00000000-0000-4000-8000-000000000000"] {
        let err = rig
            .call_err("agent.start", json!({ "name": name, "kind": FAKE }))
            .await;
        assert_eq!(
            err.code,
            amx_proto::rpc::RpcError::INVALID_PARAMS,
            "{name:?}"
        );
    }

    rig.call("agent.start", json!({ "name": "dev", "kind": FAKE }))
        .await;
    let err = rig
        .call_err("agent.start", json!({ "name": "dev", "kind": FAKE }))
        .await;
    assert!(err.message.contains("dev"), "{}", err.message);

    // One spawn, from the one call that was allowed to have one.
    let after = rig.state().await["panes"].as_array().expect("panes").len();
    assert_eq!(after, before + 1, "a refused start must not leave a pane");
    rig.stop().await;
}

/// An agent no stanza names is refused, and the error says what this session
/// does have.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_agent_kind_is_refused_with_the_registry_named() {
    let mut rig = Rig::start("aunk").await;
    let err = rig
        .call_err("agent.start", json!({ "name": "dev", "kind": "nosuch" }))
        .await;
    assert!(err.message.contains("nosuch"), "{}", err.message);
    assert!(
        err.message.contains(FAKE),
        "the refusal should name the agents this session knows: {}",
        err.message
    );
    rig.stop().await;
}
