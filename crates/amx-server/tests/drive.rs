//! V12: the pane-driving surface of 04 §8 — `send_text`, `send_keys`, `run`,
//! `read` — over the real socket, against real children.
//!
//! Every write test asserts what the **child** received, not what the server
//! sent, because those are different claims and only the first one is the
//! product. The children are `/bin/sh` scripts that put their terminal in raw
//! mode and record their standard input to a file a byte at a time (`dd bs=1`,
//! which writes each byte as it reads it, so a test can watch the file grow
//! instead of waiting for a buffer to flush). Raw mode is what makes the
//! recording exact: with the line discipline's echo and CR/LF translation off,
//! the bytes in the file are the bytes amx queued.
//!
//! The expected encodings are not this suite's invention. They were measured
//! against the vendored key encoder (`crates/amx-vt/tests/key.rs` covers the
//! same ground one layer down) and they are the classic xterm sequences:
//! `ctrl+h` is `0x08`, `f1` is `ESC O P`, `shift+tab` is `ESC [ Z`, and an
//! application that has pushed the Kitty disambiguation flag gets
//! `ESC [ 104;5 u` for the same `ctrl+h` instead — which is the whole reason
//! encoding happens on the pane's parser thread rather than in the caller.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::path::{Path, PathBuf};

use amx_core::PaneId;
use amx_proto::RpcOutcome;
use serde_json::{Value, json};

mod support;

use support::{Client, PATIENCE, Server, TICK, TempDir, result_of};

/// A session, an attached client, and a directory the children record into.
struct Rig {
    server: Server,
    client: Client,
    dir: TempDir,
    root: PaneId,
    next: u64,
}

impl Rig {
    /// Start a session, attach, and learn the seeded workspace's root pane.
    async fn start(tag: &str) -> Self {
        let server = Server::start(tag).await;
        // `attach: true` is what seeds an empty session with its first
        // workspace, and therefore with the pane every test here splits from.
        let mut client = server.connect().await;
        client.hello_as_attach(amx_proto::version::window()).await;
        let dir = TempDir::new(tag);
        let state = request(&mut client, 1, "session.state", json!({})).await;
        let root = state["panes"][0]["pane"]
            .as_str()
            .expect("the seeded workspace has a pane")
            .parse()
            .expect("a pane id");
        Self {
            server,
            client,
            dir,
            root,
            next: 2,
        }
    }

    /// Make one call, expecting it to succeed.
    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        request(&mut self.client, self.next, method, params).await
    }

    /// Make one call, expecting it to fail, and answer with the error.
    async fn call_err(&mut self, method: &str, params: Value) -> amx_proto::RpcError {
        self.next += 1;
        let response = self.client.request(self.next, method, params).await.outcome;
        match response {
            RpcOutcome::Error(err) => err,
            RpcOutcome::Result(value) => panic!("{method} unexpectedly succeeded: {value}"),
        }
    }

    /// Split off a pane running `script` under `/bin/sh`.
    async fn spawn(&mut self, script: &str) -> PaneId {
        let reply = self
            .call(
                "pane.split",
                json!({
                    "pane": self.root.to_string(),
                    "direction": "vertical",
                    "command": ["/bin/sh", "-c", script],
                }),
            )
            .await;
        reply["pane"]
            .as_str()
            .expect("the split names its pane")
            .parse()
            .expect("a pane id")
    }

    /// Where a child records what it was sent.
    fn recording(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// The pane's visible grid, as `pane.read` serves it.
    async fn screen(&mut self, pane: PaneId) -> Vec<String> {
        let reply = self
            .call("pane.read", json!({ "target": pane.to_string() }))
            .await;
        reply["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|row| row.as_str().unwrap_or_default().to_owned())
            .collect()
    }

    /// Wait until the pane has painted `needle` somewhere on its grid.
    ///
    /// The readiness signal every script here uses: a child that has printed
    /// its marker has already run the `stty` before it, so its recording starts
    /// from the first byte amx sends.
    async fn wait_for_screen(&mut self, pane: PaneId, needle: &str) {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            let screen = self.screen(pane).await;
            if screen.iter().any(|row| row.contains(needle)) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the pane never painted {needle:?}: {screen:?}"
            );
            tokio::time::sleep(TICK).await;
        }
    }

    async fn stop(self) {
        drop(self.client);
        self.server.shutdown().await;
    }
}

/// Make one call and unwrap its result.
async fn request(client: &mut Client, id: u64, method: &str, params: Value) -> Value {
    let response = client.request(id, method, params).await;
    result_of(&response).clone()
}

/// Wait until `path` holds at least `want` bytes, and answer with them.
async fn recorded(path: &Path, want: usize) -> Vec<u8> {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let bytes = std::fs::read(path).unwrap_or_default();
        if bytes.len() >= want {
            return bytes;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the child recorded {} of {want} bytes: {:?}",
            bytes.len(),
            String::from_utf8_lossy(&bytes)
        );
        tokio::time::sleep(TICK).await;
    }
}

/// A script that goes raw, says `ready`, then records `count` bytes of input.
fn recorder(out: &Path, count: usize) -> String {
    format!(
        "stty raw -echo; printf 'ready\\r\\n'; dd bs=1 count={count} of={} 2>/dev/null; \
         printf 'done\\r\\n'; sleep 60",
        out.display()
    )
}

// ------------------------------------------------------------------- tests

/// `pane.send_text` adds nothing and translates nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_text_delivers_bytes_to_the_child_verbatim() {
    let mut rig = Rig::start("stxt").await;
    let out = rig.recording("text");
    let text = "héllo\tworld";
    let pane = rig.spawn(&recorder(&out, text.len())).await;
    rig.wait_for_screen(pane, "ready").await;

    let reply = rig
        .call(
            "pane.send_text",
            json!({ "target": pane.to_string(), "text": text }),
        )
        .await;
    assert_eq!(reply["pane"], pane.to_string());

    assert_eq!(recorded(&out, text.len()).await, text.as_bytes());
    rig.stop().await;
}

/// A pane is addressable by its label, and an ambiguous label is an error that
/// names the candidates rather than picking one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driving_addresses_a_pane_by_label_and_names_an_ambiguous_one() {
    let mut rig = Rig::start("labl").await;
    let out = rig.recording("text");
    let pane = rig.spawn(&recorder(&out, 2)).await;
    rig.wait_for_screen(pane, "ready").await;
    rig.call(
        "pane.rename",
        json!({ "pane": pane.to_string(), "label": "build" }),
    )
    .await;

    let reply = rig
        .call("pane.send_text", json!({ "target": "build", "text": "hi" }))
        .await;
    assert_eq!(reply["pane"], pane.to_string());
    assert_eq!(recorded(&out, 2).await, b"hi");

    // A second pane under the same label: the target no longer names one pane,
    // and the error says which two it could have meant.
    let other = rig.spawn("sleep 60").await;
    rig.call(
        "pane.rename",
        json!({ "pane": other.to_string(), "label": "build" }),
    )
    .await;
    let err = rig
        .call_err("pane.send_text", json!({ "target": "build", "text": "hi" }))
        .await;
    assert_eq!(err.code, amx_proto::RpcError::INVALID_PARAMS);
    assert!(
        err.message.contains(&pane.to_string()) && err.message.contains(&other.to_string()),
        "the ambiguity names both candidates: {}",
        err.message
    );

    let err = rig
        .call_err(
            "pane.send_text",
            json!({ "target": "nothing-is-called-this", "text": "hi" }),
        )
        .await;
    assert_eq!(err.code, amx_proto::RpcError::INVALID_PARAMS);
    rig.stop().await;
}

/// The combo grammar, and the encoding following the pane's own key modes.
///
/// The second half is the point: the same `ctrl+h` encodes to one byte for an
/// application in the legacy encoding and to `ESC [ 104;5 u` for one that has
/// pushed the Kitty disambiguation flag. Only the parser thread knows which,
/// because only it owns the terminal that recorded the flag.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_keys_grammar_encodes_ctrl_fn_and_kitty_aware_sequences() {
    let mut rig = Rig::start("keys").await;
    let legacy = rig.recording("legacy");
    let kitty = rig.recording("kitty");
    // 0x08 + ESC O P + ESC [ Z + CR is eight bytes; ESC [ 104;5 u + ESC [ 27 u
    // is thirteen. Between them the child pushes the Kitty flag and says so.
    let script = format!(
        "stty raw -echo; printf 'ready\\r\\n'; dd bs=1 count=8 of={} 2>/dev/null; \
         printf '\\033[>1u'; printf 'kitty\\r\\n'; dd bs=1 count=13 of={} 2>/dev/null; \
         printf 'done\\r\\n'; sleep 60",
        legacy.display(),
        kitty.display(),
    );
    let pane = rig.spawn(&script).await;
    rig.wait_for_screen(pane, "ready").await;

    let reply = rig
        .call(
            "pane.send_keys",
            json!({
                "target": pane.to_string(),
                "keys": ["ctrl+h", "f1", "shift+tab", "enter"],
            }),
        )
        .await;
    assert_eq!(reply["keys"], 4);
    assert_eq!(recorded(&legacy, 8).await, b"\x08\x1bOP\x1b[Z\r");

    rig.wait_for_screen(pane, "kitty").await;
    rig.call(
        "pane.send_keys",
        json!({ "target": pane.to_string(), "keys": ["ctrl+h", "esc"] }),
    )
    .await;
    assert_eq!(recorded(&kitty, 13).await, b"\x1b[104;5u\x1b[27u");

    // A combo this build cannot read is refused by name, and nothing is sent.
    let err = rig
        .call_err(
            "pane.send_keys",
            json!({ "target": pane.to_string(), "keys": ["ctrl+h", "hyper+x"] }),
        )
        .await;
    assert_eq!(err.code, amx_proto::RpcError::INVALID_PARAMS);
    assert!(
        err.message.contains("hyper"),
        "the error names the offending element: {}",
        err.message
    );
    rig.stop().await;
}

/// `pane.run` brackets only what asked to be bracketed, and always submits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_wraps_text_in_bracketed_paste_when_the_pane_enabled_it_and_submits() {
    let mut rig = Rig::start("run").await;
    let plain = rig.recording("plain");
    let pasted = rig.recording("pasted");
    // "echo hi" + CR is eight bytes; wrapped in ESC [ 200 ~ … ESC [ 201 ~ it is
    // twenty. The child enables bracketed paste between the two.
    let script = format!(
        "stty raw -echo; printf 'ready\\r\\n'; dd bs=1 count=8 of={} 2>/dev/null; \
         printf '\\033[?2004h'; printf 'paste\\r\\n'; dd bs=1 count=20 of={} 2>/dev/null; \
         printf 'done\\r\\n'; sleep 60",
        plain.display(),
        pasted.display(),
    );
    let pane = rig.spawn(&script).await;
    rig.wait_for_screen(pane, "ready").await;

    let reply = rig
        .call(
            "pane.run",
            json!({ "target": pane.to_string(), "text": "echo hi" }),
        )
        .await;
    assert_eq!(reply["bracketed"], false);
    assert_eq!(recorded(&plain, 8).await, b"echo hi\r");

    rig.wait_for_screen(pane, "paste").await;
    let reply = rig
        .call(
            "pane.run",
            json!({ "target": pane.to_string(), "text": "echo hi" }),
        )
        .await;
    assert_eq!(reply["bracketed"], true);
    assert_eq!(recorded(&pasted, 20).await, b"\x1b[200~echo hi\x1b[201~\r");

    // And the submit is a real one: a shell handed the same call runs it.
    let shell = rig
        .spawn("stty -echo; printf 'ready\\r\\n'; exec /bin/sh")
        .await;
    rig.wait_for_screen(shell, "ready").await;
    rig.call(
        "pane.run",
        json!({ "target": shell.to_string(), "text": "printf 'it-ran\\n'" }),
    )
    .await;
    rig.wait_for_screen(shell, "it-ran").await;
    rig.stop().await;
}

/// `pane.read` serves the visible grid, bottom-anchored by construction.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_returns_the_visible_rows_as_text() {
    let mut rig = Rig::start("read").await;
    let pane = rig
        .spawn("printf 'first line\\nsecond line\\nthird line\\n'; sleep 60")
        .await;
    rig.wait_for_screen(pane, "third line").await;

    let whole = rig.screen(pane).await;
    assert_eq!(whole.len(), 24, "the default grid is 24 rows: {whole:?}");
    assert_eq!(whole[0], "first line");
    assert_eq!(whole[1], "second line");
    assert_eq!(whole[2], "third line");
    assert_eq!(whole[3], "", "trailing blanks are trimmed away");

    // `lines` takes the last N rows, top to bottom — the tail of the *live*
    // grid, since scrollback is client-side (04 §3).
    let reply = rig
        .call(
            "pane.read",
            json!({ "target": pane.to_string(), "lines": 2 }),
        )
        .await;
    let tail: Vec<&str> = reply["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row.as_str().unwrap_or_default())
        .collect();
    assert_eq!(tail, vec!["", ""], "the last two rows are the blank bottom");
    rig.stop().await;
}

/// The response-order guarantee (04 §3) holds while `pane.run` drives the pane.
///
/// The child asks the terminal where its cursor is (`ESC [ 6 n`) once per round
/// and records everything it is handed. Two things must be true of the
/// recording, and both are properties of routing driven writes through the
/// thread that owns the terminal:
///
/// - every cursor report arrives **whole**: a driven write never lands inside
///   one, so the stream tokenises cleanly into reports and `x` submissions,
///   with none lost, torn or duplicated;
/// - the first token is the report for the query the child emitted *before*
///   this test knew the pane existed, so input the test sent strictly after
///   that reply was produced never overtakes it.
///
/// Per-round order after that is not asserted, and deliberately so: once the
/// test is ahead of the child, its next `x` genuinely is queued before the
/// child asks its next question, and claiming otherwise would be asserting a
/// race rather than a contract.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driven_input_never_reorders_against_query_replies() {
    const ROUNDS: usize = 20;
    /// `ESC [ 2 ; 1 R`: row two, column one — where the marker leaves the
    /// cursor, and where it stays, because nothing else is ever painted.
    const REPORT: &[u8] = b"\x1b[2;1R";
    /// What one `pane.run` of `x` puts in front of the child.
    const SUBMIT: &[u8] = b"x\r";

    let mut rig = Rig::start("ord").await;
    let out = rig.recording("stream");
    // The marker and the first query are one write, so a pane that has painted
    // `ready` has already had that query parsed and answered.
    let script = format!(
        "stty raw -echo; printf 'ready\\r\\n\\033[6n' >&2; i=0; \
         while [ $i -lt {ROUNDS} ]; do dd bs=1 count=8 2>/dev/null; \
         printf '\\033[6n' >&2; i=$((i+1)); done >> {}; sleep 60",
        out.display(),
    );
    let pane = rig.spawn(&script).await;
    rig.wait_for_screen(pane, "ready").await;

    for _ in 0..ROUNDS {
        rig.call(
            "pane.run",
            json!({ "target": pane.to_string(), "text": "x" }),
        )
        .await;
    }

    let want = ROUNDS * (REPORT.len() + SUBMIT.len());
    let stream = recorded(&out, want).await;
    let mut at = 0;
    let mut reports = 0;
    let mut submits = 0;
    let mut first = None;
    while at < stream.len() {
        let token = if stream[at..].starts_with(REPORT) {
            reports += 1;
            REPORT
        } else if stream[at..].starts_with(SUBMIT) {
            submits += 1;
            SUBMIT
        } else {
            panic!(
                "the child's input stream is torn at byte {at}: {:?}",
                String::from_utf8_lossy(&stream)
            );
        };
        first.get_or_insert(token);
        at += token.len();
    }
    assert_eq!(
        first,
        Some(REPORT),
        "the reply the child asked for first arrives first"
    );
    assert_eq!(reports, ROUNDS, "every query was answered exactly once");
    assert_eq!(submits, ROUNDS, "every submission arrived exactly once");
    rig.stop().await;
}

/// The four verbs answer honestly when the pane is not there to drive.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driving_a_pane_that_does_not_exist_is_an_error_not_a_silence() {
    let mut rig = Rig::start("gone").await;
    let ghost = PaneId::new_v4().to_string();
    for (method, params) in [
        ("pane.send_text", json!({ "target": ghost, "text": "x" })),
        ("pane.send_keys", json!({ "target": ghost, "keys": ["a"] })),
        ("pane.run", json!({ "target": ghost, "text": "x" })),
        ("pane.read", json!({ "target": ghost })),
    ] {
        let err = rig.call_err(method, params).await;
        assert_eq!(
            err.code,
            amx_proto::RpcError::INVALID_PARAMS,
            "{method}: {}",
            err.message
        );
    }
    rig.stop().await;
}

/// Bytes that would end a bracketed paste early are refused, not rewritten.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_refuses_text_carrying_a_paste_terminator() {
    let mut rig = Rig::start("term").await;
    let target = rig.root.to_string();
    let err = rig
        .call_err(
            "pane.run",
            json!({ "target": target, "text": "one\u{1b}[201~two" }),
        )
        .await;
    assert_eq!(err.code, amx_proto::RpcError::INVALID_PARAMS);
    rig.stop().await;
}
