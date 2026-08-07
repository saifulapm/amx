//! V11 acceptance: `amx events --json`, the extension surface of 04 §8.
//!
//! "Any program can `amx events --json` (subscribe stream) and call any API
//! method." This suite is the program on the other end of that pipe, run
//! against the real binary and a real socket, because the thing being checked
//! is what a *consumer* sees — NDJSON on stdout, one delivery per line, gaps
//! included — and none of that is observable from inside the server.
//!
//! The gap is produced through the documented resume path rather than by
//! stalling a reader: a consumer that saw a gap re-queries `session.state` and
//! resumes from the sequence that reply carries, so `--after-seq` is the CLI
//! half of that contract, and asking to resume from a sequence the replay
//! buffer has already dropped is exactly the case the contract has to survive.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::io::{BufRead as _, BufReader};
use std::process::{Child, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Instant;

use amx_server::session::probe::probe;
use serde_json::{Value, json};

mod support;

use support::{Env, PATIENCE, wait_until};

/// How many terminal titles the scripted pane paints.
///
/// Each is a bus event twice over — the pane actor publishes it and `Core`
/// republishes it (R-M2-3, recorded and deliberately not extended by M2) — so
/// this outruns the server's 1024-event replay buffer, which is what makes a
/// resume from sequence 1 a resume from a sequence that is gone.
const TITLES: usize = 900;

/// The bus head this test waits for before it resumes from sequence 1.
const PAST_REPLAY: u64 = 1_100;

/// How many deliveries after the gap are checked for order.
const AFTER_GAP: usize = 6;

/// The stream survives a resume from a dropped sequence: the loss is one `gap`
/// line, and everything after it arrives in order.
#[test]
fn events_json_documents_and_survives_the_gap_resync_round_trip() {
    let env = Env::new("evgap");
    // `$SHELL` pinned for the same reason `tests/drive.rs` pins it in-process:
    // a session on the defaults puts the developer's own login shell behind
    // every pane, which is slow and leaves an interrupted zsh holding a lock on
    // their real history file.
    let mut server = env
        .command()
        .args(["server", "--session", &env.session])
        .env("SHELL", "/bin/sh")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn amx server");
    wait_until("the server binds", || {
        probe(&env.socket()).expect("probe").is_running()
    });

    // The help text is where the contract lives for the people writing
    // consumers, so it is part of the deliverable and not decoration.
    let help = env.run(&["events", "--help"]);
    let help = help.ok();
    for phrase in ["gap", "session state", "--after-seq"] {
        assert!(
            help.contains(phrase),
            "`amx events --help` must state the resync contract; {phrase:?} is missing from:\n{help}"
        );
    }

    let root = seed(&env);
    flood(&env, &root, TITLES);
    wait_until("the bus outruns its replay buffer", || {
        head(&env) > PAST_REPLAY
    });

    let mut stream = Stream::start(&env, &["events", "--json", "--after-seq", "1"]);

    // First line: the loss, named. Not an error, not a silent jump.
    let first = stream.line();
    assert_eq!(
        first["delivery"],
        json!("gap"),
        "a resume from a dropped sequence must open with a gap: {first}"
    );
    let from = first["from"].as_u64().expect("a gap names its first loss");
    let to = first["to"].as_u64().expect("a gap names its last loss");
    assert_eq!(
        from, 2,
        "the gap starts at the first sequence after the one resumed from"
    );
    assert!(to >= from, "a gap is never empty");

    // And the stream continues, in order, for as long as the session keeps
    // transitioning. That is what "survives" means: a consumer that handled
    // the gap loses nothing afterwards.
    let mut previous = to;
    let mut events = 0;
    while events < AFTER_GAP {
        env.run(&[
            "pane",
            "rename",
            "--params",
            &json!({ "pane": root, "label": format!("r{events}") }).to_string(),
        ])
        .ok();
        match stream.line() {
            line if line["delivery"] == json!("event") => {
                let seq = line["seq"].as_u64().expect("an envelope carries its seq");
                assert!(
                    seq > previous,
                    "sequences never go backwards: {seq} after {previous}"
                );
                previous = seq;
                events += 1;
            }
            // More loss is legitimate — this consumer reads one line per
            // rename and the pane behind it is still painting — and it is
            // announced the same way rather than skipped.
            line if line["delivery"] == json!("gap") => {
                previous = line["to"].as_u64().expect("a gap names its last loss");
            }
            line => panic!("unknown delivery kind in {line}"),
        }
    }

    stream.stop();
    env.stop();
    let _ = server.kill();
    let _ = server.wait();
}

/// Open a workspace, and answer with the pane it landed in.
fn seed(env: &Env) -> String {
    env.run(&["workspace", "create", "--params", "{}"]).ok();
    state(env)["panes"][0]["pane"]
        .as_str()
        .expect("the new workspace has a pane")
        .to_owned()
}

/// Split off a pane that paints `count` terminal titles and then goes quiet.
fn flood(env: &Env, root: &str, count: usize) {
    let script = format!(
        "i=0; while [ $i -lt {count} ]; do printf '\\033]0;t%s\\007' $i; i=$((i+1)); done; \
         exec sleep 300"
    );
    let params = json!({
        "pane": root,
        "direction": "vertical",
        "command": ["/bin/sh", "-c", script],
    });
    env.run(&["pane", "split", "--params", &params.to_string()])
        .ok();
}

/// The session's state, which carries the bus sequence it was captured at.
fn state(env: &Env) -> Value {
    let out = env.run(&["session", "state", "--params", "{}"]);
    serde_json::from_str(out.ok()).expect("session.state replies with JSON")
}

/// The bus head, as `session.state` reports it.
fn head(env: &Env) -> u64 {
    state(env)["seq"]
        .as_u64()
        .expect("a state reply carries its seq")
}

/// A running `amx events --json`, read line by line with a deadline.
///
/// The lines are pulled on a thread because the process under test is a
/// long-lived stream with no end: a blocking read in the test body would have
/// no way to fail rather than hang, and hanging is not a test result.
struct Stream {
    child: Child,
    lines: Receiver<String>,
}

impl Stream {
    fn start(env: &Env, args: &[&str]) -> Self {
        let mut child = env
            .command()
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn amx events");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        Self { child, lines }
    }

    /// The next delivery, parsed, or a failure naming what was waited for.
    fn line(&mut self) -> Value {
        let deadline = Instant::now() + PATIENCE;
        let left = deadline.saturating_duration_since(Instant::now());
        match self.lines.recv_timeout(left) {
            Ok(line) => serde_json::from_str(&line)
                .unwrap_or_else(|err| panic!("not NDJSON: {line:?} ({err})")),
            Err(RecvTimeoutError::Timeout) => panic!("no delivery within {PATIENCE:?}"),
            Err(RecvTimeoutError::Disconnected) => panic!("amx events stopped streaming"),
        }
    }

    fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
