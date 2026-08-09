//! T13 acceptance: the terminal is restored on every exit path (04 §3,
//! D-M0-4). All three tests fail without `TerminalGuard`: comment out its
//! `Drop` impl (or the `SIGTERM` handling in `raw_mode_probe`) and the pty's
//! attributes after the guarded scope differ from what they were before it.
//!
//! X13 added the second half of "as it was found": mouse tracking, which the
//! guard asks for only when the user turned it on and releases exactly as far
//! as it asked (`docs/notes/m4-mouse-path.md` F-4).

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Mutex};

use amx_client::term::{
    ALT_SCREEN_ENTER, ALT_SCREEN_LEAVE, MOUSE_TRACK_ENTER, MOUSE_TRACK_LEAVE, TerminalGuard,
};

/// A `Write` whose bytes outlive the guard that owns it, so a test can read
/// what was written *after* the restore that wrote it.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<u8>>>);

impl Recorder {
    fn written(&self) -> Vec<u8> {
        self.0.lock().expect("not poisoned").clone()
    }
}

impl Write for Recorder {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("not poisoned").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Where `needle` starts in `haystack`, or `None`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[test]
fn terminal_modes_are_restored_after_normal_exit() {
    let pty = support::open_pty();
    let inspect = pty.slave.try_clone().expect("clone slave fd");
    let before = support::termios_snapshot(&inspect);

    {
        let guard = TerminalGuard::enter(pty.slave, Vec::new()).expect("enter raw mode");
        let during = support::termios_snapshot(&inspect);
        assert_ne!(
            during, before,
            "raw mode must actually change the tty's attributes"
        );
        drop(guard);
    }

    let after = support::termios_snapshot(&inspect);
    assert_eq!(
        after, before,
        "normal exit must restore the saved attributes"
    );
}

#[test]
fn terminal_modes_are_restored_after_panic() {
    let pty = support::open_pty();
    let inspect = pty.slave.try_clone().expect("clone slave fd");
    let before = support::termios_snapshot(&inspect);

    let slave = pty.slave;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _guard = TerminalGuard::enter(slave, Vec::new()).expect("enter raw mode");
        panic!("simulated crash while the terminal is guarded");
    }));
    assert!(result.is_err(), "the panic must have actually propagated");

    let after = support::termios_snapshot(&inspect);
    assert_eq!(
        after, before,
        "unwinding through a panic must still run the guard's Drop"
    );
}

#[test]
fn terminal_modes_are_restored_after_sigterm() {
    let pty = support::open_pty();
    let inspect = pty.slave.try_clone().expect("clone slave fd");
    let before = support::termios_snapshot(&inspect);

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_raw_mode_probe"))
        .stdin(std::process::Stdio::from(
            pty.slave
                .try_clone()
                .expect("clone slave fd for child stdin"),
        ))
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the raw-mode probe");
    drop(pty.slave);

    // The probe enters raw mode (which writes the alt-screen-enter sequence
    // to this same stdout pipe) before it prints the marker, so the line
    // carries that preamble too — check for the marker, not an exact match.
    let mut ready_line = String::new();
    BufReader::new(child.stdout.take().expect("child stdout"))
        .read_line(&mut ready_line)
        .expect("read the ready marker");
    assert!(
        ready_line.contains("ready"),
        "the probe must confirm raw mode before we signal it: {ready_line:?}"
    );

    let during = support::termios_snapshot(&inspect);
    assert_ne!(
        during, before,
        "the probe must have actually entered raw mode"
    );

    let pid = rustix::process::Pid::from_raw(child.id() as i32).expect("child pid");
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).expect("send SIGTERM");

    let status = child.wait().expect("wait for the probe to exit");
    assert!(
        status.success(),
        "the probe must exit cleanly on SIGTERM: {status:?}"
    );

    let after = support::termios_snapshot(&inspect);
    assert_eq!(
        after, before,
        "SIGTERM must still leave the guard's Drop to restore the tty"
    );
}

/// X01's outcome (b): the request costs the user their terminal's own
/// selection, so a client nobody configured must not make it. The guard writes
/// the alt-screen pair and nothing else.
#[test]
fn mouse_tracking_is_never_asked_for_unless_it_is_turned_on() {
    let pty = support::open_pty();
    let out = Recorder::default();

    {
        let guard = TerminalGuard::enter(pty.slave, out.clone()).expect("enter raw mode");
        assert!(!guard.mouse_requested());
    }

    assert_eq!(
        out.written(),
        [ALT_SCREEN_ENTER, ALT_SCREEN_LEAVE].concat(),
        "a default client wrote something other than the alt-screen pair",
    );
}

/// The release is the reverse of the request, ahead of the alt screen it was
/// asked for on top of — and it happens on the guard's own `Drop`, which is
/// what makes it one seam rather than four call sites.
#[test]
fn requested_mouse_tracking_is_released_before_the_alt_screen() {
    let pty = support::open_pty();
    let out = Recorder::default();

    {
        let mut guard = TerminalGuard::enter(pty.slave, out.clone()).expect("enter raw mode");
        guard.request_mouse();
        assert!(guard.mouse_requested());
        // Idempotent: a second ask writes nothing more.
        guard.request_mouse();
        assert_eq!(
            out.written(),
            [ALT_SCREEN_ENTER, MOUSE_TRACK_ENTER].concat()
        );
    }

    let written = out.written();
    assert_eq!(
        written,
        [
            ALT_SCREEN_ENTER,
            MOUSE_TRACK_ENTER,
            MOUSE_TRACK_LEAVE,
            ALT_SCREEN_LEAVE,
        ]
        .concat(),
        "the release must precede leaving the screen it was asked for on",
    );
    assert!(
        find(&written, MOUSE_TRACK_LEAVE) < find(&written, ALT_SCREEN_LEAVE),
        "tracking must stop while the alt screen is still up",
    );
}

/// X01 F-4: a client that resets every mode it *knows about* would clear
/// `?1007`, which both terminals the spike measured had set before amx started.
/// The guard touches `?1006` and `?1000` and nothing else, in either direction.
#[test]
fn the_guard_touches_no_mode_it_did_not_set() {
    let pty = support::open_pty();
    let out = Recorder::default();

    {
        let mut guard = TerminalGuard::enter(pty.slave, out.clone()).expect("enter raw mode");
        guard.request_mouse();
    }

    let written = out.written();
    for mode in ["1007", "1002", "1003", "1005", "1015", "1016"] {
        let needle = format!("?{mode}");
        assert!(
            find(&written, needle.as_bytes()).is_none(),
            "the guard wrote mode {mode}, which is not its to change: {written:?}",
        );
    }
}

/// A restore that already ran must not write the release a second time: the
/// `SIGTERM` arm calls `restore()` and the guard's `Drop` then calls it again.
#[test]
fn restoring_twice_releases_the_mouse_once() {
    let pty = support::open_pty();
    let out = Recorder::default();

    let mut guard = TerminalGuard::enter(pty.slave, out.clone()).expect("enter raw mode");
    guard.request_mouse();
    guard.restore();
    let once = out.written();
    assert!(!guard.mouse_requested());
    drop(guard);

    assert_eq!(out.written(), once, "the second restore wrote something");
}
