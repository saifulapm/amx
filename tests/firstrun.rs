//! First run, end to end: a bare `amx` on a fresh session lands its user in
//! a seeded workspace whose root pane runs a real, live shell.
//!
//! **Scope note, honestly stated** (the same one `adversarial.rs` carries).
//! In this build no grid bytes cross the socket — no control method binds a
//! stream channel yet — so an attached client paints chrome over an empty
//! model, and "the user sees the shell's prompt" cannot yet be asserted
//! against the rendered grid. What *is* asserted is everything below that
//! last hop, at process level: the real client on a real pty paints a full
//! frame, and the attach left the session running exactly one shell process —
//! the seeded root pane's. The shell is planted by the test (`SHELL` travels
//! on the spawned command, never through this process's environment), so the
//! process is identified in the process table independently of anything the
//! server reports. When stream binding lands, these tests tighten to
//! asserting the prompt's rows in the rasterized screen rather than change
//! shape.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

use rig::env::processes_with_arg;
use rig::{ALT_ENTER, Env, Screen, TempDir, rasterize, wait_until};

const ROWS: u16 = 24;
const COLS: u16 = 80;

/// Whether a rasterized screen shows a fully painted frame: the status line
/// padded across the whole bottom row.
fn painted(screen: &Screen) -> bool {
    (0..COLS).all(|col| screen.contains_key(&(ROWS - 1, col)))
}

/// Plant a fake login shell whose path carries a per-test marker, so the
/// seeded root pane's process is recognizable in `/proc` no matter what the
/// machine's real `$SHELL` is.
///
/// The script announces itself and then blocks on its pty's input — a shell
/// shape, held open without any wall-clock wait for hygiene to object to.
fn marker_shell(dir: &TempDir, tag: &str) -> (PathBuf, String) {
    let marker = format!("amx-rig-{tag}-{}", std::process::id());
    let path = dir.path().join(&marker);
    std::fs::write(&path, "#!/bin/sh\necho ready\nread _held\n").expect("write the shell");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("make it executable");
    (path, marker)
}

#[test]
fn first_attach_seeds_a_workspace_with_a_live_shell() {
    let shells = TempDir::new("firstrun-shell");
    let (shell, marker) = marker_shell(&shells, "firstrun");
    let mut env = Env::new("firstrun");
    env.set_var("SHELL", &shell.to_string_lossy());

    // Bare `amx`: probe, daemonize, attach. The seeded workspace's root shell
    // is a real process by the time the client has painted, because the
    // welcome — and so everything after it — is written only after the core
    // finished seeding.
    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    term.wait_output("a fully painted frame", |seen| painted(&rasterize(seen)));
    wait_until("the seeded root pane's shell appears", || {
        processes_with_arg(&marker) == 1
    });

    // A second, later attach finds the workspace and seeds nothing: still
    // exactly one shell once it has painted (its welcome came after its
    // attach was served, so any second seed would already be visible).
    let mut second = env.attach_on_tty(&[], ROWS, COLS);
    second.wait_for(ALT_ENTER);
    second.wait_output("a fully painted frame", |seen| painted(&rasterize(seen)));
    assert_eq!(
        processes_with_arg(&marker),
        1,
        "a second attach must not seed a second workspace"
    );

    // Detaching leaves the seeded shell running: it belongs to the session,
    // not to the client that caused it to exist.
    second.chord(b'd');
    assert_eq!(second.wait(), Some(0), "detach is a clean exit");
    term.chord(b'd');
    assert_eq!(term.wait(), Some(0), "detach is a clean exit");
    assert_eq!(
        processes_with_arg(&marker),
        1,
        "the seeded shell outlives every client"
    );
}

#[test]
fn two_racing_first_attaches_seed_exactly_one_workspace() {
    let shells = TempDir::new("firstrun-race-shell");
    let (shell, marker) = marker_shell(&shells, "firstrun-race");
    let mut env = Env::new("firstrun-race");
    env.set_var("SHELL", &shell.to_string_lossy());

    // Two bare `amx` processes at once, on a session nobody has started: they
    // race the daemonize (one loses the bind and attaches anyway) and then
    // race the first attach. Both must land, and the session must end up with
    // one seeded shell, not two.
    let mut first = env.attach_on_tty(&[], ROWS, COLS);
    let mut second = env.attach_on_tty(&[], ROWS, COLS);
    first.wait_for(ALT_ENTER);
    second.wait_for(ALT_ENTER);
    first.wait_output("a fully painted frame", |seen| painted(&rasterize(seen)));
    second.wait_output("a fully painted frame", |seen| painted(&rasterize(seen)));

    // Both handshakes are complete, so both attach commands have been served:
    // whatever seeding was ever going to happen has happened.
    wait_until("the seeded root pane's shell appears", || {
        processes_with_arg(&marker) >= 1
    });
    assert_eq!(
        processes_with_arg(&marker),
        1,
        "two racing first attaches must seed one workspace, not two"
    );

    first.chord(b'd');
    assert_eq!(first.wait(), Some(0));
    second.chord(b'd');
    assert_eq!(second.wait(), Some(0));
}
