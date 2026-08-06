//! T13 acceptance: the terminal is restored on every exit path (04 §3,
//! D-M0-4). All three tests fail without `TerminalGuard`: comment out its
//! `Drop` impl (or the `SIGTERM` handling in `raw_mode_probe`) and the pty's
//! attributes after the guarded scope differ from what they were before it.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use std::io::{BufRead, BufReader};

use amx_client::term::TerminalGuard;

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
