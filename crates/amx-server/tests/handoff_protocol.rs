//! W05: the descriptor transfer and the staged commit (`docs/09-m3-plan.md`
//! §3, D-M3-6).
//!
//! Everything here runs over a real unix socket with real descriptors, because
//! every claim being made is about the kernel or about a peer that can die
//! mid-sentence — neither survives being modelled. The half not under test is
//! a scripted fake: a raw socket this file writes lines and descriptor
//! messages down by hand, so it can be as wrong, as slow, or as dead as §3's
//! crash table needs it to be.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use amx_server::handoff::fd;
use amx_server::handoff::protocol::wire::{
    Line, MAX_MANIFEST_LINE, MAX_STAGE_LINE, read_line, write_line,
};
use amx_server::handoff::protocol::{
    Ending, Exporter, HandoffError, HandoffListener, Importer, Stage, Timeouts, Token, exporter,
    importer, socket_path,
};
use support::TempDir;

/// How long a stage may take before a test calls it a hang rather than a wait.
const PATIENCE: Duration = Duration::from_secs(5);

/// How long a deadline loop waits between looks at its condition.
const TICK: Duration = Duration::from_millis(5);

/// Bounds short enough that a stalled peer is a test and not a coffee break.
///
/// §3's own numbers are 30 s, 500 ms and 5 s ([`Timeouts::DEFAULT`], pinned
/// below); what these prove is that the bound is honoured at all, which is a
/// property of the code and not of the number.
fn brisk() -> Timeouts {
    Timeouts {
        stage: Duration::from_millis(150),
        owned_ack: Duration::from_millis(80),
        socket_free: Duration::from_millis(150),
    }
}

/// A manifest stand-in: the codec is W04's, and this protocol never reads it.
fn manifest() -> serde_json::Value {
    serde_json::json!({ "manifest": 1, "panes": [] })
}

/// A private directory, a socket path in it, and the token that opens it.
struct Rig {
    dir: TempDir,
    path: PathBuf,
    token: Token,
}

impl Rig {
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let path = socket_path(dir.path(), std::process::id());
        Self {
            dir,
            path,
            token: Token::mint(),
        }
    }

    /// A file with `mark` in it, standing in for one pane's terminal.
    ///
    /// A regular file is the only object whose *identity* a test can read back
    /// after it crosses, which is what pairing an index to a pane means here.
    fn marked(&self, mark: &str) -> File {
        let path = self.dir.path().join(mark);
        std::fs::write(&path, mark.as_bytes()).expect("write the marker");
        File::open(&path).expect("open the marker")
    }
}

/// The half of the protocol this test is scripting by hand.
struct Fake(UnixStream);

impl Fake {
    /// Read timeouts so a broken expectation fails instead of hanging.
    fn bounded(stream: UnixStream) -> Self {
        stream.set_read_timeout(Some(PATIENCE)).expect("read bound");
        stream
            .set_write_timeout(Some(PATIENCE))
            .expect("write bound");
        Self(stream)
    }

    fn say(&mut self, line: &Line) {
        write_line(&mut self.0, line).expect("the scripted peer speaks");
    }

    fn hear(&mut self, cap: usize) -> Line {
        read_line(&mut self.0, cap).expect("the scripted peer listens")
    }

    fn hand(&mut self, index: u8, master: BorrowedFd<'_>) {
        fd::send(self.0.as_fd(), index, master).expect("the scripted peer hands a descriptor over");
    }

    /// Hand this end to a real process and kill it.
    ///
    /// §3's table talks about peers that die — "its fds close with the process
    /// (kernel)" — so the socket has to be closed by a `SIGKILL` and not by
    /// this test dropping a value. Moving the descriptor into the child's
    /// stdin is what makes the kill the only thing left holding it.
    fn killed(self) {
        let mut child = shell(
            "while :; do sleep 1; done",
            Stdio::from(OwnedFd::from(self.0)),
        );
        child.kill().expect("kill the peer");
        child.wait().expect("reap the peer");
    }
}

/// A shell that does nothing but hold what it was given.
fn shell(script: &str, stdin: Stdio) -> Child {
    Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .stdin(stdin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn a peer")
}

/// A real exporter that has authenticated a scripted importer.
fn exporter_authenticated(rig: &Rig) -> (Exporter<exporter::SendingManifest>, Fake) {
    let listener = HandoffListener::bind(rig.path.clone()).expect("bind the handoff socket");
    let mut fake = Fake::bounded(UnixStream::connect(&rig.path).expect("connect"));
    fake.say(&Line::Token {
        token: rig.token.expose().to_owned(),
    });
    let exporter = listener
        .accept(rig.token.clone(), brisk())
        .expect("accept")
        .authenticate()
        .expect("the right token");
    assert!(
        !rig.path.exists(),
        "the handoff socket outlived the connection that authenticated on it",
    );
    (exporter, fake)
}

/// A real exporter with the manifest away and the descriptors next.
fn exporter_at_masters(rig: &Rig) -> (Exporter<exporter::SendingMasters>, Fake) {
    let (exporter, mut fake) = exporter_authenticated(rig);
    let exporter = exporter
        .send_manifest(manifest())
        .expect("send the manifest");
    assert!(matches!(
        fake.hear(MAX_MANIFEST_LINE),
        Line::Manifest { .. }
    ));
    fake.say(&Line::Validated);
    let exporter = exporter.await_validated().expect("validated");
    (exporter, fake)
}

/// A real importer that has validated a scripted exporter's manifest.
fn importer_at_masters(rig: &Rig, panes: usize) -> (Importer<importer::ReceivingMasters>, Fake) {
    let listener = UnixListener::bind(&rig.path).expect("bind");
    let importer = Importer::connect(&rig.path, &rig.token, brisk()).expect("connect");
    let mut fake = Fake::bounded(listener.accept().expect("accept").0);
    assert!(matches!(fake.hear(MAX_STAGE_LINE), Line::Token { .. }));
    fake.say(&Line::Manifest {
        manifest: manifest(),
    });
    let (carried, importer) = importer.recv_manifest().expect("the manifest");
    assert_eq!(carried, manifest(), "the manifest crossed changed");
    let importer = importer.validated(panes).expect("validate");
    assert!(matches!(fake.hear(MAX_STAGE_LINE), Line::Validated));
    (importer, fake)
}

/// Read `len` bytes off a descriptor.
fn read_bytes(from: BorrowedFd<'_>, len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    let got = rustix::io::read(from, &mut buf).expect("read the descriptor");
    buf.truncate(got);
    buf
}

/// §3's own numbers, so a well-meaning edit to the defaults is a failing test.
#[test]
fn the_stage_bounds_are_the_ones_the_protocol_states() {
    assert_eq!(Timeouts::DEFAULT.stage, Duration::from_secs(30));
    assert_eq!(Timeouts::DEFAULT.owned_ack, Duration::from_millis(500));
    assert_eq!(Timeouts::DEFAULT.socket_free, Duration::from_secs(5));
}

/// The kernel fact the whole handoff design rests on (D-M3-3, unix(7)).
///
/// "An SCM_RIGHTS transfer is a reference to an open file description …
/// semantically equivalent to `dup(2)` into the file descriptor table of
/// another process." Two consequences amx leans on, pinned here rather than
/// quoted: the descriptors share a *file position*, so the receiver picks up
/// exactly where the sender stopped — which is why the importer resuming reads
/// after `owned` loses nothing the child wrote during the frozen window — and
/// the object survives until the last reference to it closes.
#[test]
fn an_fd_crosses_a_socketpair_and_reads_the_same_description() {
    let dir = TempDir::new("desc");
    let path = dir.path().join("shared");
    std::fs::write(&path, b"abcdef").expect("seed the file");
    let near = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open");

    let (send_on, recv_on) = UnixStream::pair().expect("socketpair");
    fd::send(send_on.as_fd(), 7, near.as_fd()).expect("send the descriptor");
    let (index, crossed) = fd::recv(recv_on.as_fd()).expect("receive the descriptor");
    assert_eq!(index, 7, "the pane index did not survive the crossing");

    // Nothing spawned from here on inherits somebody else's terminal.
    let flags = rustix::io::fcntl_getfd(crossed.as_fd()).expect("descriptor flags");
    assert!(
        flags.contains(rustix::io::FdFlags::CLOEXEC),
        "a received descriptor is inheritable, so every later `exec` keeps a \
         copy of a pane's terminal",
    );

    // One offset, shared: three bytes read through the sender's descriptor
    // move the receiver's too.
    assert_eq!(read_bytes(near.as_fd(), 3), b"abc");
    assert_eq!(
        read_bytes(crossed.as_fd(), 3),
        b"def",
        "the descriptors have separate file positions, so they are separate \
         open file descriptions and the handoff would lose whatever the \
         exporter had already read past",
    );

    // And writing through the crossed one writes at that shared position.
    rustix::io::write(crossed.as_fd(), b"ghi").expect("write through the crossed descriptor");
    assert_eq!(std::fs::read(&path).expect("re-read"), b"abcdefghi");

    // The description outlives the sender's own reference (D-M3-3: "the pty is
    // torn down only when the last reference closes").
    drop(near);
    assert_eq!(std::fs::read(&path).expect("re-read"), b"abcdefghi");
    rustix::io::write(crossed.as_fd(), b"jkl").expect("write after the sender let go");
    assert_eq!(std::fs::read(&path).expect("re-read"), b"abcdefghijkl");
}

/// One descriptor per message, paired to its manifest entry by the one byte
/// beside it — and a pairing the two sides disagree about is fatal, not
/// repaired by reordering (D-M3-6 point 3).
#[test]
fn each_message_carries_its_pane_index_and_a_mismatch_aborts() {
    let rig = Rig::new("pair");
    let (importer, mut fake) = importer_at_masters(&rig, 3);
    let marks = ["pane-a", "pane-b", "pane-c"];
    let terminals: Vec<File> = marks.iter().map(|mark| rig.marked(mark)).collect();
    for (position, terminal) in terminals.iter().enumerate() {
        fake.hand(position as u8, terminal.as_fd());
    }
    let (masters, _importer) = importer.recv_masters().expect("three descriptors");
    assert_eq!(masters.len(), 3);
    for (position, master) in masters.iter().enumerate() {
        assert_eq!(
            read_bytes(master.as_fd(), 32),
            marks[position].as_bytes(),
            "descriptor {position} is not the pane the manifest says it is",
        );
    }

    // The same run with the second message claiming the third pane's index.
    let rig = Rig::new("mism");
    let (importer, mut fake) = importer_at_masters(&rig, 3);
    let first = rig.marked("pane-a");
    let second = rig.marked("pane-b");
    fake.hand(0, first.as_fd());
    fake.hand(2, second.as_fd());
    let refused = importer
        .recv_masters()
        .expect_err("a descriptor paired with the wrong entry");
    assert!(
        matches!(
            refused,
            HandoffError::PaneIndexMismatch {
                position: 1,
                index: 2
            }
        ),
        "a mispaired descriptor was accepted or misreported: {refused:?}",
    );
    assert_eq!(refused.ending(), Ending::Abort);
    // The state machine went with the error — there is no importer left to
    // serve anything — and the peer was told why rather than finding a closed
    // socket.
    let told = fake.hear(MAX_STAGE_LINE);
    let Line::Abort { reason } = told else {
        panic!("the peer was not told why the handoff ended: {told:?}");
    };
    assert!(
        reason.contains("pane index"),
        "the abort does not name the fault: {reason}",
    );
}

/// Every stage where a side waits is bounded, and the bound produces an abort
/// rather than a process that never comes back.
///
/// Each case here is one real state machine facing a peer that connected,
/// said what it had to say to get to that stage, and then went silent — which
/// is the shape of a successor that hung rather than died, and the shape a
/// timeout is the only defence against.
#[test]
fn every_stage_times_out_to_abort_not_hang() {
    let started = Instant::now();

    // Step 1–2: nobody ever connects.
    let rig = Rig::new("acpt");
    let listener = HandoffListener::bind(rig.path.clone()).expect("bind");
    let refused = listener
        .accept(rig.token.clone(), brisk())
        .expect_err("an importer that never started");
    assert_stalled(&refused, Stage::Token);

    // Step 3: it connects and never sends its token.
    let rig = Rig::new("tokn");
    let listener = HandoffListener::bind(rig.path.clone()).expect("bind");
    let _silent = UnixStream::connect(&rig.path).expect("connect");
    let refused = listener
        .accept(rig.token.clone(), brisk())
        .expect("accept")
        .authenticate()
        .expect_err("a token that never came");
    assert_stalled(&refused, Stage::Token);

    // Step 5: the exporter never sends the manifest.
    let rig = Rig::new("mani");
    let listener = UnixListener::bind(&rig.path).expect("bind");
    let importer = Importer::connect(&rig.path, &rig.token, brisk()).expect("connect");
    let _silent = listener.accept().expect("accept");
    let refused = importer
        .recv_manifest()
        .expect_err("a manifest that never came");
    assert_stalled(&refused, Stage::Manifest);

    // Steps 6–7: the importer never answers the manifest.
    let rig = Rig::new("vald");
    let (exporter, _fake) = exporter_authenticated(&rig);
    let refused = exporter
        .send_manifest(manifest())
        .expect("send")
        .await_validated()
        .expect_err("a verdict that never came");
    assert_stalled(&refused, Stage::Validated);

    // Steps 8–9: the descriptors never come.
    let rig = Rig::new("mast");
    let (importer, _fake) = importer_at_masters(&rig, 1);
    let refused = importer
        .recv_masters()
        .expect_err("descriptors that never came");
    assert_stalled(&refused, Stage::Masters);

    // Step 10: the importer never says it restored.
    let rig = Rig::new("rest");
    let (exporter, _fake) = exporter_at_masters(&rig);
    let refused = exporter
        .send_masters(&[])
        .expect("no descriptors is a legal run")
        .await_restored()
        .expect_err("a restore that never came");
    assert_stalled(&refused, Stage::Restored);

    // Steps 11–12: the importer never binds the session socket.
    let rig = Rig::new("redy");
    let (exporter, mut fake) = exporter_at_masters(&rig);
    let exporter = exporter.send_masters(&[]).expect("send");
    fake.say(&Line::Restored);
    let refused = exporter
        .await_restored()
        .expect("restored")
        .await_ready()
        .expect_err("a bind that never happened");
    assert_stalled(&refused, Stage::Ready);

    // Step 13: the exporter never commits — §3's hardest row, and the one the
    // importer must answer by serving nothing at all.
    let rig = Rig::new("comt");
    let (importer, mut fake) = importer_at_masters(&rig, 0);
    let (_none, importer) = importer.recv_masters().expect("no descriptors");
    let importer = importer.restored().expect("restored");
    assert!(matches!(fake.hear(MAX_STAGE_LINE), Line::Restored));
    let importer = importer.ready().expect("ready");
    assert!(matches!(fake.hear(MAX_STAGE_LINE), Line::Ready));
    let refused = importer
        .await_commit()
        .expect_err("a commit that never came");
    assert_stalled(&refused, Stage::Committed);

    // Step 15: the advisory ack. It has a bound too, and missing it is not a
    // failure — ownership already moved.
    let rig = Rig::new("ownd");
    let (exporter, mut fake) = exporter_at_masters(&rig);
    let exporter = exporter.send_masters(&[]).expect("send");
    fake.say(&Line::Restored);
    let exporter = exporter.await_restored().expect("restored");
    fake.say(&Line::Ready);
    let exporter = exporter
        .await_ready()
        .expect("ready")
        .commit()
        .expect("commit");
    assert!(matches!(fake.hear(MAX_STAGE_LINE), Line::Committed));
    assert_eq!(exporter.ending(), Ending::Committed);
    assert!(
        !exporter.await_owned(),
        "an ack that never arrived was reported as one that did",
    );

    assert!(
        started.elapsed() < PATIENCE,
        "the stage bounds are not bounding: {:?}",
        started.elapsed(),
    );
}

/// A stalled stage is a timeout at that stage, and a timeout is an abort.
fn assert_stalled(err: &HandoffError, stage: Stage) {
    assert!(
        matches!(err, HandoffError::Timeout { .. }),
        "the {stage} stage did not report a stalled peer as a timeout: {err:?}",
    );
    assert_eq!(err.stage(), stage);
    assert_eq!(err.ending(), Ending::Abort);
}

/// §3's crash table, row by row, with a peer that is killed rather than closed.
///
/// The table's claim is about processes dying — "its fds close with the
/// process (kernel)" — so every case hands its socket to a real child and
/// `SIGKILL`s it. What each survivor is left holding is [`Ending`]: `Abort`
/// everywhere before the commit, `Committed` after it, and nothing else,
/// because the abort rule is strict — no partial import ever serves.
#[test]
fn a_crash_at_each_stage_leaves_what_the_table_says() {
    // Rows 0–3: the importer dies before its token. Nothing was touched.
    let rig = Rig::new("cr03");
    let listener = HandoffListener::bind(rig.path.clone()).expect("bind");
    let dying = Fake::bounded(UnixStream::connect(&rig.path).expect("connect"));
    let exporter = listener.accept(rig.token.clone(), brisk()).expect("accept");
    dying.killed();
    assert_gone(
        &exporter.authenticate().expect_err("a dead importer"),
        Stage::Token,
    );

    // Row 4–5 seen from the successor: the exporter dies while quiescing, so
    // the manifest never comes and the importer has touched nothing.
    let rig = Rig::new("cr45");
    let listener = UnixListener::bind(&rig.path).expect("bind");
    let importer = Importer::connect(&rig.path, &rig.token, brisk()).expect("connect");
    let mut dying = Fake::bounded(listener.accept().expect("accept").0);
    assert!(matches!(dying.hear(MAX_STAGE_LINE), Line::Token { .. }));
    dying.killed();
    assert_gone(
        &importer.recv_manifest().expect_err("a dead exporter"),
        Stage::Manifest,
    );

    // Rows 5–7: the importer dies over the manifest. The exporter resumes its
    // panes and goes on serving.
    let rig = Rig::new("cr57");
    let (exporter, mut dying) = exporter_authenticated(&rig);
    let exporter = exporter.send_manifest(manifest()).expect("send");
    assert!(matches!(
        dying.hear(MAX_MANIFEST_LINE),
        Line::Manifest { .. }
    ));
    dying.killed();
    assert_gone(
        &exporter.await_validated().expect_err("a dead importer"),
        Stage::Validated,
    );

    // Rows 8–10: the importer dies mid-restore. Its descriptors close with it
    // and it never read a pty; nothing observed the swap.
    let rig = Rig::new("cr810");
    let (exporter, dying) = exporter_at_masters(&rig);
    let terminal = rig.marked("pane-a");
    let exporter = exporter
        .send_masters(&[terminal.as_fd()])
        .expect("send the descriptor");
    let crossed = fd::recv(dying.0.as_fd()).expect("the peer took it");
    assert_eq!(crossed.0, 0);
    dying.killed();
    assert_gone(
        &exporter.await_restored().expect_err("a dead importer"),
        Stage::Restored,
    );
    // Killing it did not take the pane's file with it: the exporter's own
    // reference is still a working one, which is what "nothing observed the
    // swap" means at the descriptor level.
    assert_eq!(read_bytes(terminal.as_fd(), 32), b"pane-a");

    // Rows 11–12: the importer dies failing to bind. Same answer, later stage.
    let rig = Rig::new("cr1112");
    let (exporter, mut dying) = exporter_at_masters(&rig);
    let exporter = exporter.send_masters(&[]).expect("send");
    dying.say(&Line::Restored);
    let exporter = exporter.await_restored().expect("restored");
    dying.killed();
    assert_gone(
        &exporter.await_ready().expect_err("a dead importer"),
        Stage::Ready,
    );

    // Row 13 lost: the exporter dies before the commit lands. Strict abort —
    // a wedged-not-dead exporter must never meet a live importer, so the
    // successor serves nothing at all.
    let rig = Rig::new("cr13");
    let (importer, mut dying) = importer_at_masters(&rig, 0);
    let (_none, importer) = importer.recv_masters().expect("no descriptors");
    let importer = importer.restored().expect("restored");
    assert!(matches!(dying.hear(MAX_STAGE_LINE), Line::Restored));
    let importer = importer.ready().expect("ready");
    assert!(matches!(dying.hear(MAX_STAGE_LINE), Line::Ready));
    dying.killed();
    let refused = importer.await_commit().expect_err("a dead exporter");
    assert_gone(&refused, Stage::Committed);

    // After 13: the exporter is done regardless. A dead importer costs it the
    // advisory ack and nothing else, and there is no `abort` on the state it
    // is holding — the commit is where undoing stops being possible.
    let rig = Rig::new("cr14");
    let (exporter, mut dying) = exporter_at_masters(&rig);
    let exporter = exporter.send_masters(&[]).expect("send");
    dying.say(&Line::Restored);
    let exporter = exporter.await_restored().expect("restored");
    dying.say(&Line::Ready);
    let exporter = exporter
        .await_ready()
        .expect("ready")
        .commit()
        .expect("commit");
    assert_eq!(exporter.ending(), Ending::Committed);
    dying.killed();
    assert!(!exporter.await_owned());
}

/// A killed peer is a closed socket at that stage, and that is an abort.
fn assert_gone(err: &HandoffError, stage: Stage) {
    assert!(
        matches!(
            err,
            HandoffError::PeerGone { .. } | HandoffError::Timeout { .. }
        ),
        "the {stage} stage did not report a killed peer as gone: {err:?}",
    );
    assert_eq!(err.stage(), stage);
    assert_eq!(err.ending(), Ending::Abort);
}

/// The token rides the importer's stdin, and a wrong one buys nothing.
///
/// D-M3-6 point 1: herdr passes the token as a command-line argument, and
/// `/proc/*/cmdline` is world-readable on Linux, so that leaks the secret to
/// every local user. The socket's 0600 mode is the real wall; a secret that
/// leaks anyway is worse than no secret.
#[test]
fn the_token_travels_on_stdin_and_a_wrong_token_is_refused() {
    // A pipe is what the importer's stdin is, so this is the real path.
    let token = Token::mint();
    let (reader, mut writer) = std::io::pipe().expect("a pipe");
    writer
        .write_all(format!("{}\n", token.expose()).as_bytes())
        .expect("write the token");
    drop(writer);
    let mut reader = reader;
    let read_back = Token::read_from(&mut reader).expect("read the token off stdin");
    assert!(read_back.matches(token.expose()));
    assert!(
        !read_back.matches(Token::mint().expose()),
        "any token matches, so the check is not one",
    );

    // The same token through a real child's stdin, with its command line
    // checked for the leak the design is avoiding.
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg("read line; printf '%s' \"$line\"; sleep 30")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the importer stand-in");
    let mut stdin = child.stdin.take().expect("its stdin");
    stdin
        .write_all(format!("{}\n", token.expose()).as_bytes())
        .expect("write the token to stdin");
    drop(stdin);
    let mut echoed = vec![0u8; token.expose().len()];
    child
        .stdout
        .as_mut()
        .expect("its stdout")
        .read_exact(&mut echoed)
        .expect("read what it got");
    assert_eq!(
        String::from_utf8(echoed).expect("text"),
        token.expose(),
        "the token did not reach the child on stdin",
    );
    assert_absent_from_cmdline(&child, token.expose());
    child.kill().expect("kill");
    child.wait().expect("reap");

    // And a wrong token is refused by name, told to the peer, and fatal to the
    // importer that offered it.
    let rig = Rig::new("wrng");
    let listener = HandoffListener::bind(rig.path.clone()).expect("bind");
    let importer = Importer::connect(&rig.path, &Token::mint(), brisk()).expect("connect");
    let refused = listener
        .accept(rig.token.clone(), brisk())
        .expect("accept")
        .authenticate()
        .expect_err("a wrong token");
    assert!(
        matches!(refused, HandoffError::BadToken),
        "a wrong token was not refused by name: {refused:?}",
    );
    assert_eq!(refused.ending(), Ending::Abort);
    let told = importer
        .recv_manifest()
        .expect_err("nothing follows a refused token");
    assert!(
        matches!(told, HandoffError::Aborted { .. }),
        "the importer was left guessing why: {told:?}",
    );
    assert_eq!(told.ending(), Ending::Abort);
}

/// On Linux, prove the token is not on the child's command line.
///
/// `/proc/<pid>/cmdline` is the exact file the decision is about. Darwin has
/// no such file and `ps` needs privileges for another user's arguments, so
/// there the claim rests on the code: nothing in this tree puts a token in an
/// argv, and the stdin half above runs on both platforms.
#[cfg(target_os = "linux")]
fn assert_absent_from_cmdline(child: &Child, token: &str) {
    let cmdline = std::fs::read(format!("/proc/{}/cmdline", child.id())).expect("its cmdline");
    let cmdline = String::from_utf8_lossy(&cmdline);
    assert!(
        !cmdline.contains(token),
        "the token is on a world-readable command line",
    );
}

#[cfg(not(target_os = "linux"))]
fn assert_absent_from_cmdline(_child: &Child, _token: &str) {}

/// The whole staged commit, both halves real, over a real socket, with a real
/// pty crossing it.
///
/// Two threads because both machines block, which is what they are: §3 runs
/// once per upgrade and the descriptor passing has no async surface worth
/// inventing for it. The order the stages come in is the assertion — each
/// side's next method is the only one its current state has.
#[test]
fn the_staged_commit_walks_every_stage_in_order() {
    use amx_core::platform::{Pty, PtyCommand, WinSize};
    use amx_server::platform::UnixPty;

    let rig = Rig::new("walk");
    let session = UnixPty
        .spawn(&PtyCommand {
            program: "/bin/sh".into(),
            args: ["-c", "read line; printf 'GOT:%s\\n' \"$line\"; sleep 30"]
                .iter()
                .map(Into::into)
                .collect(),
            env: vec![("TERM".into(), "dumb".into())],
            cwd: None,
            size: WinSize { rows: 24, cols: 80 },
        })
        .expect("a real pty");

    let listener = HandoffListener::bind(rig.path.clone()).expect("bind");
    let path = rig.path.clone();
    let token = rig.token.clone();

    std::thread::scope(|scope| {
        let successor = scope.spawn(move || -> Vec<OwnedFd> {
            let importer = Importer::connect(&path, &token, Timeouts::DEFAULT).expect("connect");
            let (carried, importer) = importer.recv_manifest().expect("the manifest");
            assert_eq!(carried, manifest());
            let importer = importer.validated(1).expect("validate");
            let (masters, importer) = importer.recv_masters().expect("the descriptors");
            let importer = importer.restored().expect("restored");
            // Step 12 is the caller's probe-and-bind; the protocol only cares
            // that nothing is served until the commit arrives.
            let importer = importer.ready().expect("ready");
            let importer = importer.await_commit().expect("the commit");
            importer.owned();
            masters
        });

        let exporter = listener
            .accept(rig.token.clone(), Timeouts::DEFAULT)
            .expect("accept")
            .authenticate()
            .expect("the token");
        let exporter = exporter.send_manifest(manifest()).expect("the manifest");
        let exporter = exporter.await_validated().expect("validated");
        let exporter = exporter
            .send_masters(&[session.as_fd()])
            .expect("the descriptor");
        let exporter = exporter.await_restored().expect("restored");
        // Step 11 is the caller's gateway retirement; the state's name is
        // where it goes.
        let exporter = exporter.await_ready().expect("ready");
        let exporter = exporter.commit().expect("commit");
        assert_eq!(exporter.ending(), Ending::Committed);
        assert!(exporter.await_owned(), "the successor never acknowledged");

        let masters = successor.join().expect("the successor thread");
        assert_eq!(masters.len(), 1);

        // The pane the successor holds is the one the exporter had: write
        // through the crossed descriptor and the exporter's child answers.
        rustix::io::write(masters[0].as_fd(), b"handoff\n").expect("write through the successor");
        let seen = read_until(masters[0].as_fd(), b"GOT:handoff");
        assert!(
            seen.windows(11).any(|window| window == b"GOT:handoff"),
            "the descriptor that crossed does not reach the pane's child: {:?}",
            String::from_utf8_lossy(&seen),
        );
    });
}

/// Read until `needle` shows up, or give up loudly.
fn read_until(from: BorrowedFd<'_>, needle: &[u8]) -> Vec<u8> {
    let mut seen = Vec::new();
    let mut buf = [0u8; 4096];
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        match rustix::io::read(from, &mut buf) {
            Ok(0) => break,
            Ok(count) => {
                seen.extend_from_slice(&buf[..count]);
                if seen.windows(needle.len()).any(|window| window == needle) {
                    return seen;
                }
            }
            Err(rustix::io::Errno::AGAIN | rustix::io::Errno::INTR) => {
                std::thread::sleep(TICK); // (TICK)
            }
            Err(err) => panic!("reading the crossed descriptor: {err}"),
        }
    }
    panic!("never saw {:?}", String::from_utf8_lossy(needle));
}

/// A path helper the orchestrators both build from, pinned so two sessions
/// upgrading at once cannot collide on one socket.
#[test]
fn the_handoff_socket_is_named_for_the_exporter() {
    let first = socket_path(Path::new("/run/amx/work"), 4242);
    let second = socket_path(Path::new("/run/amx/work"), 4243);
    assert_eq!(first, PathBuf::from("/run/amx/work/handoff-4242.sock"));
    assert_ne!(first, second);
}
