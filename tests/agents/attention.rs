//! The attention queue, from the hub to the key a person presses.
//!
//! D-M2-8 makes the queue `AgentHub` state exposed as *state*: `session.state`
//! carries it in block order, `agent.next` focuses its head, the status line
//! renders `⚑N` from the same truth, and an external notifier consumes the
//! identical queue. Four consumers, one source — so the seam worth proving is
//! that they agree, over the wire and on a real client's screen.
//!
//! The client half is the part no in-process suite can reach: `⚑N` is rendered
//! by the real client from events it subscribed to on a real connection, and
//! the `next-attention` chord is a keystroke on a real pseudoterminal.

use rig::agent;
use rig::screen::render;
use rig::{ALT_ENTER, PREFIX};

use super::fixtures::Rig;

/// Three agents block in a known order; the key walks exactly that set.
///
/// `agent.next` focuses the head and does not rotate — the queue is the
/// ordering, and a pane leaves it by being answered. So "cycles the blocked
/// set" is a traversal: focus the head, answer it, focus the next. The
/// assertion is that the traversal is exactly the blocked panes, in block
/// order, and that it ends honestly on an empty queue rather than with an
/// error dialog nobody asked for.
#[tokio::test]
async fn next_attention_cycles_the_blocked_set_in_block_order() {
    let mut rig = Rig::start("cycl").await;
    let a = rig.start_agent("a1").await;
    let b = rig.start_agent("a2").await;
    let c = rig.start_agent("a3").await;

    // Blocked in a deliberate order that is not creation order, so "block
    // order" and "the order they happen to be in the state tree" cannot be
    // confused for each other.
    for target in [&b, &c, &a] {
        rig.block(target).await;
    }
    assert_eq!(
        rig.attention().await,
        vec![b.pane, c.pane, a.pane],
        "the queue is ordered by when each pane blocked, not by when it was made"
    );

    let mut walked = Vec::new();
    for expected in [&b, &c, &a] {
        let head = rig.call("agent.next", serde_json::json!({})).await;
        walked.push(head["pane"].as_str().unwrap_or_default().to_owned());
        assert_eq!(
            head["workspace"],
            expected.workspace.to_string(),
            "the reply names the workspace the head lives in: {head}"
        );
        // Answering the dialog is what takes a pane out of the queue.
        rig.drive(&expected.name, agent::CANCEL).await;
        rig.wait_status(expected.pane, "idle").await;
    }
    assert_eq!(
        walked,
        [b.pane.to_string(), c.pane.to_string(), a.pane.to_string()],
        "the key walked exactly the blocked set, in block order"
    );

    // An empty queue is an empty reply, not an error: 03 §4 has no chrome, and
    // a prefix key that raised a dialog for "nothing is waiting" would be some.
    let empty = rig.call("agent.next", serde_json::json!({})).await;
    assert!(empty.get("pane").is_none(), "{empty}");
    assert_eq!(empty["waiting"], 0, "{empty}");
    rig.shutdown_within(rig::PATIENCE);
}

/// The whole client-facing seam: a hook edge becomes a glyph and a focus move.
///
/// Nothing here polls. The client subscribes on attach, folds the attention
/// events into its model, and renders `⚑N` from the queue it mirrors; the chord
/// issues `agent.next` and lets the server's `FocusChanged` move it. Both ends
/// are the shipped binary — this is the one test in the tree where the status
/// line's attention count is read off a pseudoterminal.
#[tokio::test]
async fn a_block_reaches_the_clients_indicator_and_the_chord_follows_it() {
    let mut rig = Rig::start("flag").await;
    let mut term = rig
        .env
        .attach_on_tty(&[], super::fixtures::ROWS, super::fixtures::COLS);
    term.wait_for(ALT_ENTER);

    let a = rig.start_agent("a1").await;
    let b = rig.start_agent("a2").await;
    rig.block(&a).await;
    rig.block(&b).await;

    term.wait_output("the status line to count both blocked agents", |seen| {
        render(&rig::rasterize(seen)).contains("⚑2")
    });

    // One key. The client calls `agent.next`, the server switches workspace and
    // focuses, and the client learns where it is from the event — never from a
    // local guess about which pane it moved to.
    term.chord(b'a');
    term.wait_output("the focused pane to be the head of the queue", |seen| {
        render(&rig::rasterize(seen)).contains(&a.name)
    });

    // Answering the first dialog takes it out of the queue, and the indicator
    // has to notice. The `StatusLine` cache renders once and freezes if its
    // equality guard forgets an input, which is why the *decrement* is the
    // assertion and not the appearance.
    rig.drive(&a.name, agent::CANCEL).await;
    rig.wait_status(a.pane, "idle").await;
    term.wait_output("the indicator to fall to one", |seen| {
        let screen = render(&rig::rasterize(seen));
        screen.contains("⚑1") && !screen.contains("⚑2")
    });

    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));
    rig.shutdown_within(rig::PATIENCE);
}

/// A prefix key on a session with nothing waiting does nothing, loudly enough.
///
/// The regression this guards is a client that treats an empty reply as an
/// error and paints something. There is nothing to paint — 03 §4 has no
/// chrome, and "nothing is waiting" is not an occasion to grow some.
///
/// A bare session rather than one with an agent in it: what is being asserted
/// is the *absence* of a repaint, so the fewer things on screen that could move
/// on their own, the stronger the claim.
#[tokio::test]
async fn the_attention_chord_on_an_empty_queue_paints_nothing() {
    let mut rig = Rig::start("noat").await;
    let mut term = rig
        .env
        .attach_on_tty(&[], super::fixtures::ROWS, super::fixtures::COLS);
    term.wait_for(ALT_ENTER);

    let before = render(&term.wait_settled());
    assert!(!before.contains('⚑'), "nothing is waiting:\n{before}");
    term.chord(b'a');
    let after = render(&term.wait_settled());
    assert_eq!(
        before, after,
        "a next-attention on an empty queue changed the screen"
    );

    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));
    rig.shutdown_within(rig::PATIENCE);
}

/// The prefix byte the chords above are built on, named so the import is used
/// even when a platform skips a test.
const _: u8 = PREFIX;
