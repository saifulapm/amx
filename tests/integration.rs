//! T19's exit criteria, made real: the wired client against the real binary.
//!
//! Every test here drives `amx` on a real pseudoterminal over the real socket
//! and asserts on consequences no blank-grid client could fake — bytes typed
//! into the terminal change the filesystem through the pane's child process,
//! shell output crosses the wire and lands in the rasterized screen, a resize
//! reaches the child as `SIGWINCH` with the projected dimensions, and a
//! detach/reattach round-trips the identical, non-blank grid.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use rig::env::processes_with_arg;
use rig::screen::render;
use rig::wire::result_of;
use rig::{ALT_ENTER, Env, Wire, shows};
use serde_json::json;

const ROWS: u16 = 24;
const COLS: u16 = 80;

/// The pane a lone workspace shows at 24x80: content area 23 rows minus the
/// border, so the child's grid is 21x78.
const INNER: (u16, u16) = (21, 78);

// ------------------------------------------------- typed bytes reach the child

#[tokio::test]
async fn typed_bytes_reach_the_panes_child_process() {
    let env = Env::new("typed");
    let hit = env.scratch().join("typed-hit");

    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    // The seeded shell's prompt arriving on screen proves the grid stream is
    // live before anything is typed at it.
    term.wait_output("the shell prompt to render", |seen| shows(seen, "$"));

    term.type_line(&format!("touch {}", hit.display()));
    // The echo discriminates: typed bytes that never render never reached the
    // pty; an echoed line whose file never lands failed in the shell. The
    // needle is the line's *tail*: darwin's `/bin/sh` is bash 3.2, whose
    // readline horizontally scrolls a line longer than the pane is wide (the
    // deep `$TMPDIR` there guarantees that) and keeps only the tail on
    // screen, where a wrapping display keeps the whole line.
    term.wait_output("the typed line to echo", |seen| shows(seen, "typed-hit"));
    term.wait_until("the typed command's file to appear", || hit.is_file());

    // And the round trip back: output produced by the child crosses the wire
    // into this client's screen. `printf` so the typed line itself cannot
    // satisfy the assertion.
    term.type_line("printf 'ok-%s\\n' typed-mark");
    term.wait_output("the child's output to render", |seen| {
        shows(seen, "ok-typed-mark")
    });

    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));
}

// ----------------------------------------------- resize reaches the child

#[tokio::test]
async fn resize_delivers_sigwinch_and_the_child_sees_new_dimensions() {
    let env = Env::new("winch");
    let sizes = env.scratch().join("winch-sizes");

    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    term.wait_output("the shell prompt to render", |seen| shows(seen, "$"));

    // The trap is the proof of *signal delivery*: nothing runs it but a
    // process on the pane's pty receiving SIGWINCH, and what it records is
    // the size the pty holds at that moment. It lives in one non-interactive
    // `sh` held as the pty's foreground job, because the resize signal goes
    // to the foreground process *group* and an interactive shell is not
    // reliably in it (job control gives each foreground command a group of
    // its own), and because whether a pending trap runs while a shell reads
    // at its prompt is shell-specific — bash runs it there, dash does not.
    // Non-interactive sh pins both down: one group, and a pending trap runs
    // at every command boundary, which the spin on `:` supplies immediately.
    // `trap''-armed`: the quotes vanish in the shell, so the marker below can
    // only come from the echo *executing* — the typed line's own rendering,
    // wrapped or scrolled, never holds the contiguous marker and cannot
    // satisfy the wait before the baseline `stty` has run.
    term.type_line(&format!(
        "sh -c 'trap \"stty size >> {f}\" WINCH; stty size > {f}; \
         echo trap''-armed; while :; do :; done'",
        f = sizes.display()
    ));
    term.wait_output("the trap to arm", |seen| shows(seen, "trap-armed"));
    term.wait_until_or(
        "the baseline size to be recorded",
        || {
            std::fs::read_to_string(&sizes)
                .is_ok_and(|s| s.contains(&format!("{} {}", INNER.0, INNER.1)))
        },
        || {
            format!(
                "wanted {:?}, the file holds {:?}",
                format!("{} {}", INNER.0, INNER.1),
                std::fs::read_to_string(&sizes)
            )
        },
    );

    // Grow the client's terminal: the viewport re-declares, the server
    // re-projects the layout, the pane's pty resizes, the child hears it.
    term.resize(30, 100);
    let grown = (30 - 1 - 2, 100 - 2);
    let wanted = format!("{} {}", grown.0, grown.1);
    if !term.try_wait_until(|| std::fs::read_to_string(&sizes).is_ok_and(|s| s.contains(&wanted))) {
        // Discriminate where the resize died before failing. A SIGWINCH by
        // hand to the trap's shell separates the halves the kernel fused: a
        // trap that then records the grown size had a resized pty and a lost
        // signal; the old size again means the pty never resized; silence
        // means the trap's shell is gone. The server's own pane sizes then
        // separate "the viewport never landed" from "the resize was
        // commanded and lost below".
        let before = std::fs::read_to_string(&sizes).unwrap_or_default();
        let signalled = rig::env::signal_winch(&sizes.display().to_string());
        let fired =
            term.try_wait_until(|| std::fs::read_to_string(&sizes).unwrap_or_default() != before);
        let after = std::fs::read_to_string(&sizes).unwrap_or_default();
        let mut wire = Wire::connect(&env.socket()).await;
        wire.hello(amx_proto::version::window()).await;
        let state = wire.request("session.state", json!({})).await;
        panic!(
            "timed out waiting until the child records the post-SIGWINCH size: \
             wanted {wanted:?}, the file held {before:?}; after SIGWINCH by hand \
             to {signalled} trap shell(s) the trap {}, the file holds {after:?}; \
             the server sizes its panes {}; the client painted:\n{}",
            if fired { "fired" } else { "stayed silent" },
            result_of(&state)["panes"],
            render(&rig::rasterize(term.output())),
        );
    }

    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));
}

// -------------------------------------- detach, reattach, identical content

#[tokio::test]
async fn detach_and_reattach_shows_the_identical_non_blank_grid() {
    let env = Env::new("reattach");

    let mut first = env.attach_on_tty(&[], ROWS, COLS);
    first.wait_for(ALT_ENTER);
    first.type_line("printf 'ok-%s\\n' reattach-mark");
    first.wait_output("the marker to render", |seen| {
        shows(seen, "ok-reattach-mark")
    });
    let before = first.wait_settled();
    assert!(
        render(&before).contains("ok-reattach-mark"),
        "the baseline grid is non-blank and holds the marker"
    );

    // The civilised exit: prefix `d`, the input machine's own detach verb.
    first.chord(b'd');
    assert_eq!(first.wait(), Some(0), "prefix d detaches cleanly");

    let mut second = env.attach_on_tty(&[], ROWS, COLS);
    second.wait_for(ALT_ENTER);
    second.wait_output("the reattached screen to match the first", |seen| {
        rig::rasterize(seen) == before
    });
    let after = rig::rasterize(second.output());
    assert!(
        render(&after).contains("ok-reattach-mark"),
        "the reattached grid holds the same content, not a fresh blank:\n{}",
        render(&after)
    );

    second.chord(b'd');
    assert_eq!(second.wait(), Some(0));
}

// -------------------------------------- a second client's model, over the wire

#[tokio::test]
async fn session_state_populates_a_second_clients_model() {
    let env = Env::new("second");

    let mut first = env.attach_on_tty(&[], ROWS, COLS);
    first.wait_for(ALT_ENTER);
    first.type_line("printf 'ok-%s\\n' second-mark");
    first.wait_output("the marker to render", |seen| shows(seen, "ok-second-mark"));

    // The wire view: the snapshot names the seeded workspace, its layout and
    // its focused pane — everything a fresh model folds.
    let mut wire = Wire::connect(&env.socket()).await;
    wire.hello(amx_proto::version::window()).await;
    let state = wire.request("session.state", json!({})).await;
    let state = result_of(&state);
    assert_eq!(state["workspaces"].as_array().expect("workspaces").len(), 1);
    assert!(state["focused_workspace"].is_string());
    assert!(state["workspaces"][0]["layout"]["root"].is_object());
    assert!(state["panes"][0]["pane"].is_string());
    let (rows, cols) = INNER;
    assert_eq!(
        state["panes"][0]["rows"], rows,
        "the active client's size drives the pane"
    );
    assert_eq!(state["panes"][0]["cols"], cols);

    // The living proof: a second real client folds that snapshot, binds the
    // pane's stream, and paints content it never typed.
    let mut second = env.attach_on_tty(&[], ROWS, COLS);
    second.wait_for(ALT_ENTER);
    second.wait_output("the first client's content in the second's model", |seen| {
        shows(seen, "ok-second-mark")
    });
    assert!(first.alive(), "the first client is untouched");

    first.chord(b'd');
    assert_eq!(first.wait(), Some(0));
    second.chord(b'd');
    assert_eq!(second.wait(), Some(0));
}

// ------------------------------------------------------- the picker, end to end

#[tokio::test]
async fn picker_switches_workspaces_end_to_end() {
    let env = Env::new("picker");

    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    term.wait_output("the shell prompt to render", |seen| shows(seen, "$"));

    // A second workspace, labelled, unfocused — created over the wire so the
    // client only learns of it the way any client does, via session.state.
    let mut wire = Wire::connect(&env.socket()).await;
    wire.hello(amx_proto::version::window()).await;
    let created = wire
        .request(
            "workspace.create",
            json!({ "label": "beta", "focus": false }),
        )
        .await;
    let beta = result_of(&created)["workspace"]
        .as_str()
        .expect("the new workspace's id")
        .to_owned();

    // Prefix p opens the picker; typing filters to the labelled workspace;
    // Enter chooses it. The status line naming `beta` proves the switch round
    // tripped: the label only reaches this client through a fresh
    // session.state fold after workspace.switch succeeded.
    term.chord(b'p');
    term.wait_output("the picker to open", |seen| shows(seen, "> "));
    term.send(b"beta\r");
    // The *status line* names the workspace — the picker's own rows also said
    // "beta", so the assertion reads the bottom row specifically.
    term.wait_output("the status line to name the new workspace", |seen| {
        let screen = rig::rasterize(seen);
        (0..COLS.saturating_sub(4)).any(|col| {
            "beta".chars().enumerate().all(|(at, ch)| {
                // Bottom-row cells only.
                #[allow(clippy::cast_possible_truncation, reason = "at < 4")]
                let col = col + at as u16;
                screen.get(&(ROWS - 1, col)) == Some(&ch)
            })
        })
    });

    let state = wire.request("session.state", json!({})).await;
    assert_eq!(
        result_of(&state)["focused_workspace"].as_str(),
        Some(beta.as_str()),
        "the server's focus followed the picker's choice"
    );

    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));
}

// --------------------------------------------------- the pane survives clients

#[tokio::test]
async fn a_process_started_by_typing_outlives_every_client() {
    let env = Env::new("outlives");
    let marker = format!("amx-rig-outlives-{}", std::process::id());

    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    term.wait_output("the shell prompt to render", |seen| shows(seen, "$"));

    // Start a process whose argv carries the marker, through nothing but
    // typed bytes. `read` rather than a real command on purpose: a builtin
    // keeps the shell — and the marker in its argv — alive, where a trailing
    // external command would be exec'd over it.
    term.type_line(&format!("sh -c 'echo held; read _held' {marker}"));
    // The echo discriminates input loss from a process-table probe failure.
    // The needle is the marker at the line's *tail*, which bash 3.2 (darwin's
    // `/bin/sh`) keeps on screen when readline horizontally scrolls a long
    // line — "held" sits at the head, which scrolling hides.
    term.wait_output("the typed line to echo", |seen| shows(seen, &marker));
    term.wait_until("the process to appear in the table", || {
        processes_with_arg(&marker) >= 1
    });

    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));
    assert!(
        processes_with_arg(&marker) >= 1,
        "detaching leaves the typed process running"
    );

    env.stop();
    rig::wait_until("session stop reaps the pane's children", || {
        processes_with_arg(&marker) == 0
    });
}
