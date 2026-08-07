//! The transitions V01 measured, replayed against the real binary.
//!
//! Four of the thirteen cases in `docs/notes/hook-coverage.md` §7 are the ones
//! the fusion design exists for, and all four are **silences**: Esc during
//! generation, Esc during a tool call, a permission dialog answered "No", and
//! the same dialog cancelled with Esc. Nothing fires. The pane's status has to
//! move anyway, and the only witness is the screen.
//!
//! So each test here drives a real hook edge through the real `amx _hook`
//! binary, then takes the edge away and watches tier 2 finish the job. What is
//! asserted is not "the screen changed" — that would be a test of the script —
//! but that the *fused status* the wire reports followed it.

use std::time::Instant;

use rig::agent;

use super::fixtures::Rig;

/// V01 §7.1–2: Esc mid-generation and Esc mid-tool-call, both silent.
///
/// The hook says `Working` and then says nothing ever again. Only tier 2 can
/// end that turn, and the confirmation window plus its cap is the bound it has
/// to end it inside.
#[tokio::test]
async fn esc_interrupt_without_a_stop_event_settles_idle_within_the_bound() {
    let mut rig = Rig::start("esc").await;
    let a = rig.start_agent("solo").await;

    // Mid-generation: `UserPromptSubmit` and then the user's Esc.
    rig.drive(&a.name, agent::PROMPT).await;
    rig.wait_status(a.pane, "working").await;
    let interrupted = Instant::now();
    rig.drive(&a.name, agent::ESC).await;
    rig.wait_status(a.pane, "idle").await;
    let settled = interrupted.elapsed();

    // The screen is the witness the status followed, and it says so in the
    // words Claude Code paints.
    let screen = rig.screen(a.pane).await.join("\n");
    assert!(
        screen.contains(agent::INTERRUPTED_TEXT),
        "the pane is showing the interrupted screen:\n{screen}"
    );

    // Mid-tool-call: `PreToolUse`, then the same silence, and no `PostToolUse`
    // ever arrives to close it (V01 §7.2).
    rig.drive(&a.name, agent::TOOL).await;
    rig.wait_status(a.pane, "working").await;
    rig.drive(&a.name, agent::ESC).await;
    rig.wait_status(a.pane, "idle").await;

    // An upper bound rather than a measurement of the machine: the wire round
    // trips, the pty and a shell script's own scheduling are all inside this
    // number, so it is sized to catch "the staleness deadline did it, thirty
    // seconds later" and nothing finer.
    assert!(
        settled < rig::PATIENCE,
        "the interrupt took {settled:?} to settle, which is the staleness \
         deadline's work rather than the confirmation window's"
    );
    rig.shutdown_within(rig::PATIENCE);
}

/// V01 §7.3–4: a dialog answered "No", and one cancelled with Esc.
///
/// Byte-identical hook traces and byte-identical screens, because V01 measured
/// them indistinguishable — so this test drives the one command the fake has
/// for both, and the assertion is that leaving `Blocked` never needed a hook.
#[tokio::test]
async fn dialog_cancel_without_a_hook_event_unblocks_via_screen() {
    let mut rig = Rig::start("cncl").await;
    let a = rig.start_agent("solo").await;

    rig.block(&a).await;
    assert_eq!(
        rig.attention().await,
        vec![a.pane],
        "entering blocked enqueues the pane for attention (D-M2-8)"
    );

    rig.drive(&a.name, agent::CANCEL).await;
    rig.wait_status(a.pane, "idle").await;
    assert!(
        rig.attention().await.is_empty(),
        "leaving blocked dequeues it, whether or not a hook said so"
    );
    rig.shutdown_within(rig::PATIENCE);
}

/// V01 §7.5–6: the subagent noise a tool-using turn produces.
///
/// The anonymous `SubagentStop` lands one to three seconds after the parent's
/// `Stop` on essentially every tool-using turn, so this is the common path.
/// herdr's "never revive an idle pane" is a rule of the fusion machine here,
/// and the assertion is the strong form: not only does the state stay `idle`,
/// the transition sequence does not move — nothing was published at all.
#[tokio::test]
async fn subagent_stop_after_parent_stop_never_revives_the_idle_pane() {
    let mut rig = Rig::start("suba").await;
    let a = rig.start_agent("solo").await;

    // A nested subagent turn, inside the parent's own turn.
    rig.drive(&a.name, agent::PROMPT).await;
    rig.wait_status(a.pane, "working").await;
    rig.drive(&a.name, agent::SUBAGENT).await;
    rig.drive(&a.name, agent::STOP).await;
    rig.wait_status(a.pane, "idle").await;
    let settled = rig.transition_seq(a.pane).await;

    // …and then the late anonymous one, after the turn is over.
    rig.drive(&a.name, agent::LATE_SUBAGENT).await;
    // The report has to have *arrived* for its being ignored to mean anything,
    // so wait for the round trip the way the emitter's own suite does: one
    // further call that cannot be answered before the report was handled, since
    // both cross the same connection to the same actors.
    let seen = rig.pane_state(a.pane).await;
    assert_eq!(seen["agent"]["state"], "idle", "{seen}");
    assert_eq!(
        rig.transition_seq(a.pane).await,
        settled,
        "a subagent event published a transition on the parent's pane"
    );
    rig.shutdown_within(rig::PATIENCE);
}
