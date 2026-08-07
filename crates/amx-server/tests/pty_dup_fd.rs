//! `Control::DupFd`: getting a pty master out of a quiesced actor (D-M3-3).
//!
//! The missing piece of the quiesce/release machinery M0 built. `State::
//! Quiesced` already documents itself as "the state M3's descriptor handoff
//! lands in: the terminal is still owned and still open, but nothing is being
//! read from it or written to it, so its state cannot move under the process
//! taking it over" — and until now there was no way to get the descriptor out
//! of the actor that owns it.
//!
//! Driven over a real pty with a real child, because the two claims are about
//! the kernel: that the duplicate is the *same open file description* (unix(7):
//! an SCM_RIGHTS transfer is "semantically equivalent to `dup(2)` into the file
//! descriptor table of another process"), and that it does not travel into
//! anything this process spawns afterwards.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::ffi::OsString;
use std::os::fd::{AsFd, OwnedFd};
use std::time::{Duration, Instant};

use amx_core::platform::{Pty, PtyCommand, WinSize};
use amx_server::platform::UnixPty;
use amx_server::pty::{PtyActor, PtyActorConfig, PtyActorError, PtyActorHandle};
use bytes::Bytes;

/// The initial size every test spawns at.
const SIZE: WinSize = WinSize { rows: 24, cols: 80 };

/// How long a read loop waits for the kernel to have something.
const PATIENCE: Duration = Duration::from_secs(5);

/// How long that loop waits between looks.
const TICK: Duration = Duration::from_millis(5);

/// A shell script to run on a fresh pty.
fn shell(script: &str) -> PtyCommand {
    PtyCommand {
        program: OsString::from("/bin/sh"),
        args: ["-c", script].iter().map(OsString::from).collect(),
        env: vec![(OsString::from("TERM"), OsString::from("dumb"))],
        cwd: None,
        size: SIZE,
    }
}

/// Read from a raw descriptor until `needle` shows up, or give up loudly.
///
/// The duplicate is a bare `OwnedFd` — that is the whole point, it is what
/// would cross a socket — so this reads it the way the importer will, with
/// `rustix`, rather than through any wrapper that could be doing the work.
fn read_until(fd: &OwnedFd, needle: &[u8]) -> Vec<u8> {
    let mut seen = Vec::new();
    let mut buf = [0u8; 4096];
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        match rustix::io::read(fd.as_fd(), &mut buf) {
            Ok(0) => break,
            Ok(count) => {
                seen.extend_from_slice(&buf[..count]);
                if seen.windows(needle.len()).any(|w| w == needle) {
                    return seen;
                }
            }
            Err(rustix::io::Errno::AGAIN | rustix::io::Errno::INTR) => {
                std::thread::sleep(TICK); // (TICK)
            }
            Err(err) => panic!("reading the duplicate: {err} (saw {seen:?})"),
        }
    }
    panic!(
        "never saw {:?}; saw {:?}",
        String::from_utf8_lossy(needle),
        String::from_utf8_lossy(&seen)
    );
}

/// An actor over a real pty running `script`, with a parser that keeps nothing.
fn actor(script: &str) -> (PtyActorHandle, std::thread::JoinHandle<()>) {
    let session = UnixPty.spawn(&shell(script)).expect("spawn");
    let config = PtyActorConfig::new(session, Box::new(|_bytes, _responses: &mut Vec<Bytes>| {}));
    PtyActor::spawn(config).expect("actor")
}

#[test]
fn dup_fd_answers_only_in_quiesced_and_yields_a_working_duplicate() {
    // A child that waits for a line and then answers, so the duplicate can be
    // proven to reach the *process*, not just the kernel buffer.
    let (handle, thread) = actor("read line; printf 'GOT:%s\\n' \"$line\"; sleep 30");

    // Running: refused, and named for what is wrong with it rather than
    // borrowed from the input path's vocabulary.
    let refused = handle.dup_fd().expect_err("a running actor holds its pty");
    assert!(
        matches!(refused, PtyActorError::NotQuiesced),
        "a running actor must refuse by name, not by accident: {refused:?}",
    );

    handle.quiesce().expect("quiesce");
    let duplicate = handle.dup_fd().expect("a quiesced actor hands it over");

    // The copy does not travel: whatever this process spawns next must not
    // inherit somebody else's terminal (W01's finding, applied to a descriptor
    // this code makes on purpose).
    let flags = rustix::io::fcntl_getfd(duplicate.as_fd()).expect("the descriptor flags");
    assert!(
        flags.contains(rustix::io::FdFlags::CLOEXEC),
        "the duplicate is inheritable, so every process spawned from here on \
         keeps a copy of a pane's terminal",
    );

    // The same open file description, and the child is on the other end of it:
    // write through the duplicate while the actor is quiesced and reading
    // nothing, and the child answers.
    rustix::io::write(duplicate.as_fd(), b"handoff\n").expect("write through the duplicate");
    let seen = read_until(&duplicate, b"GOT:handoff");
    assert!(
        seen.windows(11).any(|w| w == b"GOT:handoff"),
        "the duplicate does not reach the child: {:?}",
        String::from_utf8_lossy(&seen),
    );

    // Resuming puts the terminal back in motion, and the door closes again.
    handle.resume().expect("resume");
    assert!(
        matches!(handle.dup_fd(), Err(PtyActorError::NotQuiesced)),
        "a resumed actor is a running actor",
    );

    // And a released one is past handing anything over at all.
    handle.quiesce().expect("re-quiesce");
    handle.release().expect("release");
    let after = handle
        .dup_fd()
        .expect_err("a released actor has nothing to give");
    assert!(
        matches!(after, PtyActorError::Released | PtyActorError::Gone),
        "a released actor must say so: {after:?}",
    );

    drop(duplicate);
    drop(handle);
    thread.join().expect("actor thread");
}

/// The exporter's copies closing must not hang up on the child.
///
/// The lifetime fact the whole handoff design leans on (D-M3-3): the pty is
/// torn down only when the *last* reference to the open file description
/// closes, so the exporter releasing its actor after commit costs the child
/// nothing — the importer's descriptor keeps the terminal open. Proven here
/// with one process standing in for both, because the property is about
/// descriptions and not about processes.
#[test]
fn a_released_actor_does_not_hang_up_a_terminal_something_else_still_holds() {
    let (handle, thread) = actor("read line; printf 'GOT:%s\\n' \"$line\"; sleep 30");

    handle.quiesce().expect("quiesce");
    let duplicate = handle.dup_fd().expect("the duplicate");
    handle.release().expect("release");
    drop(handle);
    thread.join().expect("actor thread");

    // The actor thread is gone and its session dropped; the terminal is still
    // there, and so is the child on the far end of it.
    rustix::io::write(duplicate.as_fd(), b"survived\n").expect("write after the actor is gone");
    let seen = read_until(&duplicate, b"GOT:survived");
    assert!(
        seen.windows(12).any(|w| w == b"GOT:survived"),
        "the child hung up when the exporter closed its own copy: {:?}",
        String::from_utf8_lossy(&seen),
    );
}

/// A duplicate taken from a quiesced actor sees what the child wrote *after*
/// the quiesce, in order.
///
/// D-M3-6, stated precisely: "after `quiesce()` returns, the exporter no longer
/// reads, so child output accumulates in the kernel's pty buffer; a child that
/// fills it blocks in `write(2)` — flow-controlled, not lost. The importer
/// resumes reading on the same open file description after `owned`, so the
/// bytes come out exactly where the exporter stopped."
#[test]
fn output_produced_while_quiesced_comes_out_of_the_duplicate() {
    let (handle, thread) = actor("read line; printf 'AFTER\\n'; sleep 30");

    handle.quiesce().expect("quiesce");
    let duplicate = handle.dup_fd().expect("the duplicate");

    // The actor is reading nothing, so this round trip exists only in the
    // kernel until the duplicate collects it.
    rustix::io::write(duplicate.as_fd(), b"go\n").expect("write through the duplicate");
    let seen = read_until(&duplicate, b"AFTER");
    assert!(
        seen.windows(5).any(|w| w == b"AFTER"),
        "output produced during the frozen window was lost: {:?}",
        String::from_utf8_lossy(&seen),
    );

    drop(duplicate);
    handle.shutdown();
    drop(handle);
    thread.join().expect("actor thread");
}

/// A pty this test opened directly, to pin what the refusals are *about*.
///
/// Without this, `NotQuiesced` and `Released` could both be produced by an
/// actor that simply never worked; here the same descriptor is proven live
/// before any actor is involved.
#[test]
fn the_refusals_are_about_state_and_not_about_a_broken_terminal() {
    let session = UnixPty.spawn(&shell("sleep 30")).expect("spawn");
    let flags = rustix::io::fcntl_getfd(session.as_fd()).expect("the descriptor flags");
    assert!(
        flags.contains(rustix::io::FdFlags::CLOEXEC),
        "the pty layer's own master is inheritable",
    );

    let config = PtyActorConfig::new(session, Box::new(|_bytes, _responses: &mut Vec<Bytes>| {}));
    let (handle, thread) = PtyActor::spawn(config).expect("actor");
    assert!(matches!(handle.dup_fd(), Err(PtyActorError::NotQuiesced)));

    handle.quiesce().expect("quiesce");
    let duplicate = handle.dup_fd().expect("the same terminal, handed over");
    assert!(
        rustix::io::write(duplicate.as_fd(), b"\n").is_ok(),
        "the descriptor the actor refused while running is a working one",
    );

    drop(duplicate);
    handle.shutdown();
    drop(handle);
    thread.join().expect("actor thread");
}
