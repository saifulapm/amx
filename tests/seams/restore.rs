//! The restore↔pane-spawn seam: what a restored session does to the client
//! that arrives after it.
//!
//! `Core::seed_first_workspace` returns early when any workspace exists, which
//! is what makes restore-then-attach compose with the first-attach seed at no
//! cost (D-M1-9). U06 proved that against a `Core` in-process. The claim this
//! file adds is the one only the real binary can make: the *ordering* holds
//! across a process boundary too — restore runs between the gateway's bind and
//! its accept loop, so the earliest client that can possibly connect already
//! finds the session populated, and there is no window in which it seeds a
//! third workspace over the top of two that came back.

use rig::env::processes_with_arg;
use rig::{ALT_ENTER, Env, rasterize, wait_until};

use crate::fixtures::{
    COLS, ROWS, connected, dir, durable_view, marker_shell, painted, rename, session_state,
    snapshot_mentions, split_in, switch, workspace,
};

#[tokio::test]
async fn an_attaching_client_never_reseeds_a_restored_session() {
    let shell = marker_shell("seed", ":");
    let mut env = Env::new("seed");
    env.set_var("SHELL", &shell.path());

    // Two workspaces, so a seeded third would be unmistakable — and neither of
    // them created by an attach, so the seed has never run in this session.
    let server = env.server();
    let mut wire = connected(&env).await;
    let (build, build_root) = workspace(&mut wire, "build").await;
    let (_, agents_root) = workspace(&mut wire, "agents").await;
    let editor = split_in(&mut wire, build_root, &dir(&env.scratch(), "editing")).await;
    rename(&mut wire, editor, "editor").await;
    split_in(&mut wire, agents_root, &dir(&env.scratch(), "building")).await;
    switch(&mut wire, build).await;
    wait_until("the save records both workspaces", || {
        snapshot_mentions(&env, "editor")
            && snapshot_mentions(&env, "build")
            && snapshot_mentions(&env, "agents")
    });
    let before = durable_view(&session_state(&mut wire).await);
    drop(wire);

    server.kill_dash_9();
    wait_until("the killed server's shells are reaped", || {
        processes_with_arg(shell.marker()) == 0
    });

    // The restart, and then the thing that has never happened to this session:
    // a client attaching. The client is real and its handshake carries
    // `attach: true`, which is the request that seeds a workspace on an empty
    // session — every rig suite before this one either attached to a session
    // it had seeded itself or asked over a non-attaching raw connection.
    let server = env.server();
    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    term.wait_output("a fully painted frame", |seen| painted(&rasterize(seen)));

    let mut wire = connected(&env).await;
    let state = session_state(&mut wire).await;
    assert_eq!(
        state["workspaces"].as_array().expect("workspaces").len(),
        2,
        "an attach to a restored session must seed nothing: {state}"
    );
    assert_eq!(
        durable_view(&state),
        before,
        "the attach changed the restored session"
    );
    assert_eq!(
        state["restore"],
        serde_json::json!({ "restored": 6, "lost": 0, "degraded": 0 }),
        "two workspaces and four panes came back cleanly: {state}"
    );

    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));
    server.shutdown();
}
