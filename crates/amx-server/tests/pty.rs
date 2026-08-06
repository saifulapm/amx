//! The Unix pty layer: opening a terminal, and the actor that owns one.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use amx_core::platform::{
    PlatformError, ProcessId, ProcessTree, Pty, PtyCommand, PtySession, WinSize,
};
use amx_server::platform::{UnixProcessTree, UnixPty, UnixPtySession};
use amx_server::pty::{ChildExit, PtyActor, PtyActorConfig, PtyActorHandle, ReadCallback};
use bytes::Bytes;

/// The initial size every test spawns at.
const SIZE: WinSize = WinSize { rows: 24, cols: 80 };

/// How long a test waits for a child to say something.
const PATIENCE: Duration = Duration::from_secs(5);

/// How long a poll loop waits between looks at its condition.
const TICK: Duration = Duration::from_millis(5);

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
            Err(PlatformError::WouldBlock) => thread::sleep(TICK),
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
        thread::sleep(TICK);
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

#[test]
fn reader_eof_reports_child_exit_status() {
    let _turn = pty_turn();
    let session = UnixPty.spawn(&shell("exit 7")).expect("spawn");

    let (exits, exited) = mpsc::channel();
    let mut config = PtyActorConfig::new(session, Box::new(|_bytes, _responses| {}));
    config.on_exit = Some(Box::new(move |exit| {
        let _ = exits.send(exit);
    }));
    let (handle, thread) = PtyActor::spawn(config).expect("actor");

    assert_eq!(
        exited.recv_timeout(PATIENCE).expect("exit report"),
        ChildExit::Code(7),
        "the status the child exited with should survive the end of its terminal"
    );
    drop(handle);
    thread.join().expect("actor thread");
}

// ------------------------------------------------------------ the actor

/// A pty stand-in over a socket pair.
///
/// It exists to make the actor's own behaviour observable: how much of a write
/// the terminal accepts at a time is a property of the terminal, and here it is
/// a knob rather than a race.
struct FakeSession {
    io: UnixStream,
    chunk: usize,
    writes: Arc<AtomicUsize>,
}

impl FakeSession {
    /// The actor's end, the test's end, and the write-call counter.
    fn pair(chunk: usize) -> (Self, UnixStream, Arc<AtomicUsize>) {
        let (near, far) = UnixStream::pair().expect("socket pair");
        near.set_nonblocking(true).expect("non-blocking");
        far.set_read_timeout(Some(PATIENCE)).expect("read timeout");
        let writes = Arc::new(AtomicUsize::new(0));
        let session = Self {
            io: near,
            chunk,
            writes: Arc::clone(&writes),
        };
        (session, far, writes)
    }
}

impl AsFd for FakeSession {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.io.as_fd()
    }
}

impl PtySession for FakeSession {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, PlatformError> {
        match (&self.io).read(buf) {
            Ok(count) => Ok(count),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                Err(PlatformError::WouldBlock)
            }
            Err(err) => Err(PlatformError::Io(err)),
        }
    }

    fn write(&mut self, buf: &[u8]) -> Result<usize, PlatformError> {
        self.writes.fetch_add(1, Ordering::Relaxed);
        let take = buf.len().min(self.chunk);
        match (&self.io).write(&buf[..take]) {
            Ok(count) => Ok(count),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                Err(PlatformError::WouldBlock)
            }
            Err(err) => Err(PlatformError::Io(err)),
        }
    }

    fn resize(&mut self, _size: WinSize) -> Result<(), PlatformError> {
        Ok(())
    }

    fn child(&self) -> ProcessId {
        ProcessId(0)
    }

    fn foreground_group(&self) -> Result<ProcessId, PlatformError> {
        Err(PlatformError::Unsupported("socket pair has no terminal"))
    }

    fn try_wait(&mut self) -> Result<Option<Option<i32>>, PlatformError> {
        Ok(None)
    }

    fn kill(&mut self) -> Result<(), PlatformError> {
        Ok(())
    }
}

/// Start an actor over a fake session.
fn fake_actor(
    session: FakeSession,
    idle_timeout: Duration,
    on_read: ReadCallback,
) -> (PtyActorHandle, thread::JoinHandle<()>) {
    let mut config = PtyActorConfig::new(session, on_read);
    config.idle_timeout = idle_timeout;
    PtyActor::spawn(config).expect("actor")
}

#[test]
fn out_of_band_response_never_precedes_an_earlier_in_band_response() {
    let (session, mut far, _writes) = FakeSession::pair(4096);
    let (entered, reading) = mpsc::channel();
    let dispatched = Arc::new(AtomicBool::new(false));

    let in_band = {
        let dispatched = Arc::clone(&dispatched);
        Box::new(move |_bytes: &[u8], responses: &mut Vec<Bytes>| {
            // Announce that the parser is running, then give the out-of-band
            // writer every chance to get its answer in first.
            let _ = entered.send(());
            while !dispatched.load(Ordering::Acquire) {
                thread::sleep(TICK);
            }
            // Hold the parser open so the out-of-band answer has every
            // chance to jump the queue; the ordering assertion holds with or
            // without the window, the window just arms it.
            thread::sleep(Duration::from_millis(100)); // deliberate
            responses.push(Bytes::from_static(b"IN"));
        })
    };
    let (handle, thread) = fake_actor(session, Duration::from_millis(50), in_band);

    far.write_all(b"?").expect("ask the parser something");
    reading.recv_timeout(PATIENCE).expect("parser ran");

    let out_of_band = {
        let handle = handle.clone();
        thread::spawn(move || {
            dispatched.store(true, Ordering::Release);
            handle
                .write_terminal_response(|| Some(Bytes::from_static(b"OOB")))
                .expect("out-of-band response");
        })
    };

    let mut replies = [0u8; 5];
    far.read_exact(&mut replies).expect("both replies");
    assert_eq!(
        &replies, b"INOOB",
        "the reply the read produced first must reach the child first"
    );

    out_of_band.join().expect("out-of-band thread");
    handle.shutdown();
    thread.join().expect("actor thread");
}

#[test]
fn partial_write_resumes_at_the_correct_offset() {
    const CHUNK: usize = 7;
    let (session, mut far, writes) = FakeSession::pair(CHUNK);
    let (handle, thread) = fake_actor(
        session,
        Duration::from_millis(50),
        Box::new(|_bytes, _responses| {}),
    );

    let payload: Vec<u8> = (0..200u32).map(|index| (index % 251) as u8).collect();
    handle
        .try_write_input(Bytes::from(payload.clone()))
        .expect("queue input");

    let mut seen = vec![0u8; payload.len()];
    far.read_exact(&mut seen).expect("the whole payload");
    assert_eq!(
        seen, payload,
        "a partial write must resume where the terminal stopped, not repeat or skip"
    );
    assert!(
        writes.load(Ordering::Relaxed) >= payload.len() / CHUNK,
        "the terminal took {CHUNK} bytes at a time, so this was written in pieces"
    );

    handle.shutdown();
    thread.join().expect("actor thread");
}

#[test]
fn wake_pipe_makes_a_queued_write_visible_without_waiting_for_idle_timeout() {
    // Long enough that a test which waited for it would fail instead of pass.
    let idle = Duration::from_secs(60);
    let (session, mut far, _writes) = FakeSession::pair(4096);
    far.set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");
    let (handle, thread) = fake_actor(session, idle, Box::new(|_bytes, _responses| {}));

    // Let the actor reach its poll before queueing, so the wake is what
    // gets it out rather than the loop not having parked yet; a short
    // window only makes the test vacuous, never red.
    thread::sleep(Duration::from_millis(50)); // deliberate
    let started = Instant::now();
    handle
        .try_write_input(Bytes::from_static(b"ping"))
        .expect("queue input");

    let mut seen = [0u8; 4];
    far.read_exact(&mut seen).expect("input reached the pty");
    assert_eq!(&seen, b"ping");
    assert!(
        started.elapsed() < idle,
        "the write waited for the idle timeout instead of the wake"
    );

    handle.shutdown();
    thread.join().expect("actor thread");
}
