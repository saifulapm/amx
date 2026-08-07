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

/// A process's controlling terminal, or `None` when it has none.
///
/// The comm field of `stat` is an arbitrary string in parentheses, so the
/// fields after it are counted from the last `)` rather than from the front.
#[cfg(target_os = "linux")]
fn controlling_terminal(process: ProcessId) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", process.0)).expect("stat");
    let after_comm = &stat[stat.rfind(')').expect("comm") + 1..];
    // state, ppid, pgrp, session, tty_nr
    let tty_nr: i32 = after_comm
        .split_whitespace()
        .nth(4)
        .expect("tty_nr")
        .parse()
        .expect("tty_nr");
    (tty_nr != 0).then(|| tty_nr.to_string())
}

/// A process's controlling terminal, or `None` when it has none.
///
/// darwin has no `/proc`; `ps -o tty=` reads the same field without
/// entitlements, printing `??` for a process with no terminal.
#[cfg(target_os = "macos")]
fn controlling_terminal(process: ProcessId) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "tty=", "-p", &process.0.to_string()])
        .output()
        .expect("run ps");
    let tty = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!tty.is_empty() && !tty.starts_with('?')).then_some(tty)
}

/// Every descriptor in this process that points at a terminal.
#[cfg(target_os = "linux")]
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

/// Every descriptor in this process that points at a terminal.
///
/// darwin's `/dev/fd` entries are not symlinks the way `/proc/self/fd`'s
/// are, so each descriptor is named through `fcntl(F_GETPATH)` instead;
/// its pty devices are `/dev/ptmx` masters and `/dev/ttys*` slaves. A
/// descriptor closed between the listing and the naming drops out, which
/// reads the same as it never having been listed.
#[cfg(target_os = "macos")]
fn parent_pty_fds() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/dev/fd") else {
        return Vec::new();
    };
    let mut targets: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_string_lossy().parse::<i32>().ok())
        .filter_map(|fd| {
            // SAFETY: the descriptor is only borrowed for the duration of
            // the `F_GETPATH` call and nothing here closes or stores it.
            let fd = unsafe { BorrowedFd::borrow_raw(fd) };
            rustix::fs::getpath(fd).ok()
        })
        .map(|path| path.to_string_lossy().into_owned())
        .filter(|target| target.starts_with("/dev/ttys") || target == "/dev/ptmx")
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

    assert!(
        controlling_terminal(session.child()).is_some(),
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

/// The queue property `pane.run` rests on: a pair is two writes, in order.
///
/// One chunk is one `write()` to the terminal, so queueing the paste and its
/// submitting `CR` as a pair is what puts a read boundary between them — the
/// thing a paste-aware TUI needs in order to see the `CR` as a keypress rather
/// than as trailing whitespace of the paste. The A/B against the concatenation
/// is here because the byte stream is identical either way: the difference is
/// entirely in how it is delivered, and only the write count can see it.
///
/// What this cannot test is the child's side. The failure that motivated the
/// pairing is a live one, about 3% of turn-starting prompts against real
/// Claude Code, and it depends on where the child's read boundaries fall — no
/// CI assertion reaches it. The trials are in `docs/notes/m2-live-smoke.md`
/// §8.2; this test pins the half that is deterministic.
#[test]
fn a_queued_pair_reaches_the_terminal_as_two_writes_in_order() {
    const PASTE: &[u8] = b"\x1b[200~echo hi\x1b[201~";
    const SUBMIT: &[u8] = b"\r";

    let (session, mut far, writes) = FakeSession::pair(4096);
    let (handle, thread) = fake_actor(
        session,
        Duration::from_millis(50),
        Box::new(|_bytes, _responses| {}),
    );

    handle
        .try_write_input_pair(Bytes::from_static(PASTE), Bytes::from_static(SUBMIT))
        .expect("queue the pair");
    let mut seen = vec![0u8; PASTE.len() + SUBMIT.len()];
    far.read_exact(&mut seen).expect("the pair");
    assert_eq!(
        seen,
        [PASTE, SUBMIT].concat(),
        "the submit must follow the paste, and follow it immediately"
    );
    assert_eq!(
        writes.load(Ordering::Relaxed),
        2,
        "a pair is two chunks, and one chunk is one write"
    );

    // The same bytes as one chunk: one write, which is what the live smoke
    // measured losing the submit.
    handle
        .try_write_input(Bytes::from([PASTE, SUBMIT].concat()))
        .expect("queue the concatenation");
    far.read_exact(&mut seen).expect("the concatenation");
    assert_eq!(
        writes.load(Ordering::Relaxed),
        3,
        "concatenated, the same bytes are one write — the shape this replaced"
    );

    handle.shutdown();
    thread.join().expect("actor thread");
}

/// Nothing of anyone else's can land between the halves of a pair.
///
/// The interloper is a second thread queueing into the same pane while pairs
/// go in — which is not a hypothetical: a pane's input queue takes drives from
/// the parser thread *and* the keystrokes a connection forwards, so "two
/// chunks queued back to back" is a claim about two producers, not one.
/// Reserving both slots bounds the capacity and nothing else; only placing
/// both sends under the queue's ordering lock makes them adjacent. Without
/// that lock this test fails, which is how the requirement was found.
#[test]
fn nothing_can_be_queued_between_the_halves_of_a_pair() {
    const PASTE: &[u8] = b"\x1b[200~p\x1b[201~";
    const ROUNDS: usize = 200;

    let (session, mut far, _writes) = FakeSession::pair(4096);
    let (handle, thread) = fake_actor(
        session,
        Duration::from_millis(50),
        Box::new(|_bytes, _responses| {}),
    );

    // Drained while the writers run, so neither the socket nor the input queue
    // backs up and starts refusing sends for reasons this test is not about.
    let reader = thread::spawn(move || {
        let mut stream = Vec::new();
        while stream.iter().filter(|byte| **byte == b'\r').count() < ROUNDS {
            let mut buf = [0u8; 512];
            let read = far.read(&mut buf).expect("the stream");
            assert_ne!(read, 0, "the terminal closed before the pairs arrived");
            stream.extend_from_slice(&buf[..read]);
        }
        stream
    });
    let interloper = {
        let handle = handle.clone();
        thread::spawn(move || {
            for _ in 0..ROUNDS * 4 {
                let _ = handle.try_write_input(Bytes::from_static(b"X"));
            }
        })
    };
    for _ in 0..ROUNDS {
        handle
            .try_write_input_pair(Bytes::from_static(PASTE), Bytes::from_static(b"\r"))
            .expect("queue the pair");
    }
    interloper.join().expect("interloper thread");
    let stream = reader.join().expect("reader thread");

    for (index, window) in stream.windows(PASTE.len() + 1).enumerate() {
        if window.starts_with(PASTE) {
            assert_eq!(
                window[PASTE.len()],
                b'\r',
                "byte {index}: a paste was separated from its submit"
            );
        }
    }

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
