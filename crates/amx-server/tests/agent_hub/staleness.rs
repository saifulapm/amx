//! The M4 exit's first defect, driven against a real hub: a held state is not
//! demoted by the clock while tier 2 can see the pane.
//!
//! `docs/notes/m4-live-smoke.md` §6.8 is the run this suite answers. Five real
//! Claude Code sessions, one asked for a file write, watched once a second and
//! touched by nothing:
//!
//! ```text
//! [   0.0s] status=working  reason='UserPromptSubmit'   queued=False queue=0
//! [   4.1s] status=blocked  reason='PermissionRequest'  queued=True  queue=1
//! [  39.5s] status=idle     reason=None                 queued=False queue=0
//! ```
//!
//! The permission dialog was still on the pane. `amx agents` reported five idle
//! agents, the board showed no flag at all, and `amx agent next` answered
//! `waiting: 0`. §4.8 had recorded the same shape against a stand-in five days
//! earlier, where `agent explain` reported the shipped `permission_dialog` rule
//! matching the screen in the same second the pane was called idle.
//!
//! Why no suite caught it: the fusion machine's own tests feed a tracker that
//! tier 2 never speaks to, which is a real pane and is also the *only* pane the
//! old rule was right about. Nothing drove a hub that had a manifest, a painted
//! dialog and a deadline at once — which is the ordinary case and is what these
//! four do. The clock here is `start_paused`, so thirty seconds is free and the
//! run is not a wall clock against the machine.
//!
//! The four panes, and the rule they draw between them:
//!
//! | Pane | Tier 2, at the fire | Outcome |
//! |---|---|---|
//! | a dialog the manifest reads | asserts `blocked` | held |
//! | a dialog the manifest cannot name | nothing, and it is looking | held |
//! | a manifest bound after the pane's last damage | nothing yet — until the fire asks | held |
//! | a stanza whose manifest never loaded | nothing, and nothing ever will | **demoted**, which is 04 §5 clause (c) |

use std::time::Duration;

use amx_core::agent::{AgentState, StatusCause};
use amx_proto::control::agent::HookEvent;

use crate::fixtures::{FakePane, IMPATIENT_CLAUDE, Rig, pane_id, report, screen, token, wait_for};
use crate::support::TempDir;

/// Comfortably past [`STALENESS`](amx_server::agent::fusion::STALENESS), which
/// is 30 s, on a clock that is paused — so the cost of asking is nothing.
const PAST_STALENESS: Duration = Duration::from_secs(31);

/// The Write/Edit permission dialog, as §6.8 recorded it off a real session.
///
/// The shipped `permission_dialog` rule requires `contains = ["do you want to
/// proceed?"]` and this dialog does not say that — it names the file instead —
/// so tier 2 has no opinion about the screen at all. That gap is a manifest
/// fix and is not this task's; what *is* this task's is that a block amx cannot
/// corroborate is still a block amx was told about.
const WRITE_DIALOG: &str = "\
 Write file

   exit-probe.txt

 Do you want to overwrite exit-probe.txt?
 ❯ 1. Yes
   2. Yes, and don't ask again this session
   3. No

 Esc to cancel · Tab to amend";

/// A `claude` whose stanza names a manifest that is not there.
///
/// Not a contrivance: a catalog entry that failed to load leaves exactly this
/// pane, and so does an agent nobody has written rules for. It is V01 §7 edge
/// case 13's pane — hooks that assert entries and never an exit, and no screen
/// anybody is reading — and it is the one the staleness exit exists for.
const CLAUDE_WITHOUT_RULES: &str = r#"
[[agent]]
id          = "claude"
aliases     = ["claude-code"]
label       = "Claude Code"
executables = ["claude"]
coverage    = "edges"
start       = ["claude"]
resume      = ["claude", "--resume", "{ref}"]
ref_kind    = "id"
manifest    = "no-such-manifest.toml"
startup_grace_ms = 0
hook_events = ["SessionStart", "UserPromptSubmit", "Stop", "PermissionRequest"]
"#;

/// §4.8, with the dialog the shipped manifest can read.
///
/// Thirty seconds of a screen that has stopped changing, because the thing on
/// it is waiting for a human. The pane keeps its state, keeps its place in the
/// queue, and keeps the instant it blocked at — that last one is what makes
/// "blocked for 11m" read `11m` on the surface that reads it, where before the
/// revival on the next repaint restamped it to `0s`.
#[tokio::test(start_paused = true)]
async fn a_block_the_screen_still_shows_outlives_the_deadline() {
    let root = TempDir::new("held");
    let rig = Rig::under(&root).registry(IMPATIENT_CLAUDE).start();
    let pane = FakePane::start(&rig.ctx.bus, pane_id(0));
    let hook = token("held");

    // Painted before anything arms a deadline, which is the order a real
    // session produces anyway: the dialog is what the pane is showing when the
    // hook that names it arrives.
    rig.started(&pane, &hook, Some("claude")).await;
    pane.paint(&screen("claude-blocked-permission.txt")).await;
    wait_for(
        || rig.view.get(pane.pane).map(|status| status.state) == Some(AgentState::Blocked),
        "the dialog on the grid never blocked the pane",
    )
    .await;
    rig.report(report(
        pane.pane,
        &hook,
        "claude",
        HookEvent::PermissionRequest,
    ))
    .await;
    rig.settle().await;

    let blocked = rig.view.get(pane.pane).expect("the pane is tracked");
    assert_eq!(rig.view.attention(), vec![pane.pane]);
    let transitions = rig.probe.transitions();

    // Nobody touches it, nothing repaints it, and the deadline arrives.
    tokio::time::advance(PAST_STALENESS).await;
    rig.settle().await;

    let after = rig.view.get(pane.pane).expect("the pane is still tracked");
    assert_eq!(
        after.state,
        AgentState::Blocked,
        "the dialog is still on the screen; the clock is not a witness against it",
    );
    assert_eq!(after.cause, blocked.cause);
    assert_eq!(after.reason, blocked.reason);
    assert_eq!(
        after.since, blocked.since,
        "nothing moved, so the instant it blocked at is untouched",
    );
    assert_eq!(
        rig.probe.transitions(),
        transitions,
        "a fold of timers alone publishes nothing",
    );
    assert_eq!(rig.view.attention(), vec![pane.pane]);
    assert_eq!(
        rig.next_attention().await.waiting,
        1,
        "`amx agent next` answered `waiting: 0` about this pane in the field",
    );

    // And it is not one reprieve. Five more minutes of the same silence.
    for _ in 0..10 {
        tokio::time::advance(PAST_STALENESS).await;
        rig.settle().await;
    }
    assert_eq!(
        rig.view.get(pane.pane).map(|status| status.state),
        Some(AgentState::Blocked),
    );

    rig.stop().await;
    pane.stop().await;
}

/// §6.8, with the dialog the shipped manifest *cannot* read.
///
/// This is the run that failed the milestone, and it is the harder half: there
/// is no screen verdict to corroborate the hook even in principle, so the only
/// thing standing between the block and the clock is the rule that silence from
/// a detector which is looking says nothing.
#[tokio::test(start_paused = true)]
async fn a_block_tier_2_cannot_name_outlives_the_deadline() {
    let root = TempDir::new("unread");
    let rig = Rig::under(&root).registry(IMPATIENT_CLAUDE).start();
    let pane = FakePane::start(&rig.ctx.bus, pane_id(0));
    let hook = token("unread");

    rig.started(&pane, &hook, Some("claude")).await;
    pane.paint(WRITE_DIALOG).await;
    wait_for(
        || rig.probe.evaluations() > 0,
        "the painted dialog was never evaluated",
    )
    .await;

    // The gap, asserted from this side so that closing it is visible here: the
    // dialog is on the screen and the shipped rules have nothing to say about
    // it. If a later manifest learns this phrasing, this line fails and the
    // screen above wants replacing with one the rules still cannot read.
    let explained = rig.explain(pane.pane).await;
    assert_eq!(
        explained.manifest.as_deref(),
        Some("bundled:claude.toml"),
        "tier 2 is bound and running here",
    );
    assert_eq!(
        explained.matched, None,
        "and it cannot see a Write dialog: `contains = [\"do you want to proceed?\"]`",
    );

    rig.report(report(
        pane.pane,
        &hook,
        "claude",
        HookEvent::PermissionRequest,
    ))
    .await;
    rig.settle().await;
    let blocked = rig.view.get(pane.pane).expect("the pane is tracked");
    assert_eq!(blocked.state, AgentState::Blocked);
    assert_eq!(
        blocked.cause,
        StatusCause::Hook,
        "the hook is the only detector this block has",
    );
    assert_eq!(blocked.reason.as_deref(), Some("PermissionRequest"));

    tokio::time::advance(PAST_STALENESS).await;
    rig.settle().await;

    let after = rig.view.get(pane.pane).expect("the pane is still tracked");
    assert_eq!(after.state, AgentState::Blocked);
    assert_eq!(after.reason.as_deref(), Some("PermissionRequest"));
    assert_eq!(after.since, blocked.since);
    assert_eq!(rig.view.attention(), vec![pane.pane]);
    assert_eq!(rig.next_attention().await.waiting, 1);

    rig.stop().await;
    pane.stop().await;
}

/// The half the hub owns: the answer the deadline consults has to be current.
///
/// A `claude` typed into a shell is tracked before it is identified, so its
/// screen is painted while no manifest is bound and the evaluation returns
/// without a verdict. The first hook report identifies it, binds the rules and
/// blocks it in the same breath — and then nothing repaints, so the rules that
/// could read the dialog never get a chance to. Damage is what schedules tier 2
/// and a pane waiting on a human is the one pane that produces none.
///
/// So the fire asks, against the frame the pane already holds. It is one
/// manifest pass and no I/O, and without it this pane's block would go the way
/// the field run's did — demoted on the strength of an evaluation that never
/// happened.
#[tokio::test(start_paused = true)]
async fn the_deadline_asks_tier_2_rather_than_the_last_thing_it_heard() {
    let root = TempDir::new("ask");
    let rig = Rig::under(&root).registry(IMPATIENT_CLAUDE).start();
    let pane = FakePane::start(&rig.ctx.bus, pane_id(0));
    let hook = token("ask");

    // Tracked as an unknown program: `/bin/sh` is the argv, nothing was
    // requested, and tier 3 has no manifest to offer.
    rig.started(&pane, &hook, None).await;
    pane.paint(&screen("claude-blocked-permission.txt")).await;
    rig.settle().await;
    assert_eq!(
        rig.probe.evaluations(),
        0,
        "a pane with no rules bound is a pane tier 2 has never read",
    );

    // The hook identifies the agent, binds `claude.toml` and asserts the block,
    // all from one report — and no damage follows it.
    rig.report(report(
        pane.pane,
        &hook,
        "claude",
        HookEvent::PermissionRequest,
    ))
    .await;
    rig.settle().await;
    assert_eq!(
        rig.view.get(pane.pane).map(|status| status.state),
        Some(AgentState::Blocked),
    );
    assert_eq!(
        rig.probe.evaluations(),
        0,
        "binding a manifest does not evaluate anything; damage does, and there is none",
    );

    tokio::time::advance(PAST_STALENESS).await;
    rig.settle().await;

    assert_eq!(
        rig.probe.evaluations(),
        1,
        "the fire is what finally reads the screen",
    );
    assert_eq!(
        rig.view.get(pane.pane).map(|status| status.state),
        Some(AgentState::Blocked),
        "and what it read was the dialog",
    );
    assert_eq!(rig.view.attention(), vec![pane.pane]);

    rig.stop().await;
    pane.stop().await;
}

/// 04 §5's clause (c), still doing its job — and the price of the rule, stated.
///
/// A stanza whose manifest never loaded has no tier 2 at all, so no verdict is
/// coming for this pane, ever. The dialog is on the screen exactly as it is in
/// the three panes above and it makes no difference, because nothing here can
/// look at it: without the deadline the pane would hold `Blocked` for the life
/// of the session, which is herdr's stuck-status bug that R-M2-11 sized this
/// number to avoid.
#[tokio::test(start_paused = true)]
async fn a_pane_nothing_is_reading_is_still_cleared_by_the_deadline() {
    let root = TempDir::new("blind");
    let rig = Rig::under(&root).registry(CLAUDE_WITHOUT_RULES).start();
    let pane = FakePane::start(&rig.ctx.bus, pane_id(0));
    let hook = token("blind");

    rig.started(&pane, &hook, Some("claude")).await;
    pane.paint(&screen("claude-blocked-permission.txt")).await;
    rig.report(report(
        pane.pane,
        &hook,
        "claude",
        HookEvent::PermissionRequest,
    ))
    .await;
    rig.settle().await;
    assert_eq!(
        rig.view.get(pane.pane).map(|status| status.state),
        Some(AgentState::Blocked),
    );
    assert_eq!(rig.view.attention(), vec![pane.pane]);

    tokio::time::advance(PAST_STALENESS).await;
    rig.settle().await;

    let after = rig.view.get(pane.pane).expect("the pane is still tracked");
    assert_eq!(after.state, AgentState::Idle);
    assert_eq!(after.cause, StatusCause::Staleness);
    assert_eq!(
        after.reason, None,
        "no detector caused this, so no detector is named",
    );
    assert!(rig.view.attention().is_empty());
    assert_eq!(rig.next_attention().await.waiting, 0);
    assert_eq!(
        rig.probe.evaluations(),
        0,
        "and nothing was evaluated on the way, because there is nothing to evaluate with",
    );

    rig.stop().await;
    pane.stop().await;
}
