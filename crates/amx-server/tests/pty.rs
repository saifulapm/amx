//! The Unix pty layer: opening a terminal and driving it.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use amx_core::platform::{
    PlatformError, ProcessId, ProcessTree, Pty, PtyCommand, PtySession, WinSize,
};
use amx_server::platform::{UnixProcessTree, UnixPty, UnixPtySession};

/// The initial size every test spawns at.
const SIZE: WinSize = WinSize { rows: 24, cols: 80 };

/// How long a test waits for a child to say something.
const PATIENCE: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------- real ptys

/// Opening a pty is process-global (`/proc/self/fd` is one table), so the
/// tests that count descriptors and the tests that add them take turns.
fn pty_turn() -> MutexGuard<'static, ()> {
    static TURN: OnceLock<Mutex<()>> = OnceLock::new();
    TURN.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// A shell script to run on a fresh pty.
fn shell(script: &str) -> PtyCommand {
    plain("/bin/sh", &["-c", script])
}

/// A program to run on a fresh pty.
fn plain(program: &str, args: &[&str]) -> PtyCommand {
    PtyCommand {
        program: OsString::from(program),
        args: args.iter().map(OsString::from).collect(),
        env: vec![(OsString::from("TERM"), OsString::from("xterm-256color"))],
        cwd: None,
        size: SIZE,
    }
}

/// The device number of a process's controlling terminal; 0 means it has none.
///
/// The comm field of `stat` is an arbitrary string in parentheses, so the
/// fields after it are counted from the last `)` rather than from the front.
fn controlling_terminal(process: ProcessId) -> i32 {
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", process.0)).expect("stat");
    let after_comm = &stat[stat.rfind(')').expect("comm") + 1..];
    // state, ppid, pgrp, session, tty_nr
    after_comm
        .split_whitespace()
        .nth(4)
        .expect("tty_nr")
        .parse()
        .expect("tty_nr")
}

/// Every descriptor in this process that points at a terminal.
fn parent_pty_fds() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/proc/self/fd") else {
        return Vec::new();
    };
    let mut targets: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .map(|target| target.to_string_lossy().into_owned())
        .filter(|target| target.starts_with("/dev/pts/") || target == "/dev/ptmx")
        .collect();
    targets.sort();
    targets
}

/// Read from `session` until `needle` shows up or the patience runs out.
fn read_until(session: &mut UnixPtySession, needle: &[u8]) -> Vec<u8> {
    let deadline = Instant::now() + PATIENCE;
    let mut seen = Vec::new();
    let mut buf = [0u8; 512];
    while Instant::now() < deadline {
        match session.read(&mut buf) {
            Ok(0) => break,
            Ok(count) => {
                seen.extend_from_slice(&buf[..count]);
                if contains(&seen, needle) {
                    break;
                }
            }
            Err(PlatformError::WouldBlock) => thread::sleep(Duration::from_millis(5)),
            Err(err) => panic!("pty read failed: {err}"),
        }
    }
    seen
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn process_tree_answers_for_a_live_child_and_stops_when_it_is_gone() {
    let _turn = pty_turn();
    let tree = UnixProcessTree;
    let me = ProcessId(std::process::id());
    let mut session = UnixPty.spawn(&plain("cat", &[])).expect("spawn");
    let child = session.child();

    assert_eq!(
        tree.cwd(me).expect("own cwd"),
        std::env::current_dir().expect("cwd")
    );
    assert!(tree.is_alive(child));
    assert!(
        tree.children(me).expect("children").contains(&child),
        "a pty child is a child of this process"
    );

    session.kill().expect("kill");
    let deadline = Instant::now() + PATIENCE;
    while session.try_wait().expect("try_wait").is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !tree.is_alive(child),
        "a collected child is no longer a process"
    );
}

#[test]
fn spawn_leaves_exactly_one_parent_pty_fd() {
    let _turn = pty_turn();
    let before = parent_pty_fds();

    let mut session = UnixPty.spawn(&shell("read line")).expect("spawn");
    let after = parent_pty_fds();

    assert_eq!(
        after.len(),
        before.len() + 1,
        "the master should be the only terminal descriptor the spawn adds: {before:?} -> {after:?}"
    );

    session.kill().expect("kill");
}

#[test]
fn child_gets_the_pty_as_its_controlling_terminal() {
    let _turn = pty_turn();
    // Deliberately not a shell: a shell claims a controlling terminal for
    // itself, so it would pass this test for a pty that handed it none.
    let mut session = UnixPty.spawn(&plain("cat", &[])).expect("spawn");

    assert_ne!(
        controlling_terminal(session.child()),
        0,
        "the child should have a controlling terminal, not just the descriptors"
    );
    assert_eq!(
        session.foreground_group().expect("foreground group"),
        session.child(),
        "and it should be this pty, whose foreground group is the child's own"
    );

    session.kill().expect("kill");
}

#[test]
fn resize_delivers_sigwinch_to_the_child() {
    let _turn = pty_turn();
    let mut session = UnixPty
        .spawn(&shell(
            "trap 'echo WINCH' WINCH; echo READY; while :; do sleep 0.1; done",
        ))
        .expect("spawn");

    let ready = read_until(&mut session, b"READY");
    assert!(
        contains(&ready, b"READY"),
        "child never installed its trap: {:?}",
        String::from_utf8_lossy(&ready)
    );

    session
        .resize(WinSize {
            rows: 40,
            cols: 120,
        })
        .expect("resize");

    let seen = read_until(&mut session, b"WINCH");
    assert!(
        contains(&seen, b"WINCH"),
        "resize did not signal the child: {:?}",
        String::from_utf8_lossy(&seen)
    );

    session.kill().expect("kill");
}
