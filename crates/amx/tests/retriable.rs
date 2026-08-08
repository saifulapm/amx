//! DR-16, over the real binary: a refusal that names itself retriable is asked
//! again, and a mutating verb is asked again with it.
//!
//! The claim has two halves and they pull against each other, which is why
//! `pane.send_text` is the verb under test rather than a read. `cmd::call`'s
//! [`reissuable`] list refuses to repeat anything that types into a pane: a
//! connection that died after the request was written cannot say whether it
//! landed, and a second prompt is worse than an error. `RETRIABLE` is the
//! session saying the opposite in a code — *I did not act* — so the same verb
//! that must never be repeated on a dead socket is safe to repeat on this
//! answer. A test that only proved a read gets retried would prove the part
//! that already worked.
//!
//! The session here is a stub rather than a handoff, for the reason
//! `wait_retry.rs` gives about abandoned waits: the real producer of this
//! refusal is a pane quiesced inside a swap, and a swap retires its socket a
//! moment later, so a test built on one would be racing the close. The stub is
//! well-behaved and stays open, so the retry has nowhere to hide.
//!
//! The refusal is built with [`RpcError::new`] on the shipped constant, not on
//! a literal `-32001`, so the fixture cannot drift from the wire.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use std::collections::BTreeSet;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use amx_core::{PaneId, SessionId};
use amx_proto::frame::{CONTROL_CHANNEL, FRAME_HEADER_LEN, FrameHeader};
use amx_proto::hello::{Hello, ServerInfo};
use amx_proto::rpc::{Request, Response, RpcError};
use serde_json::json;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinHandle;

use support::{Env, Output};

/// What the session refuses the first time it is asked, and how.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// "Ask me again": the code DR-16 added.
    Retriable,
    /// "You asked wrong": what the same condition answered before it.
    InvalidParams,
}

impl Refusal {
    fn error(self) -> RpcError {
        match self {
            Self::Retriable => RpcError::new(
                RpcError::RETRIABLE,
                "the pane is not accepting input".to_owned(),
            ),
            Self::InvalidParams => RpcError::new(
                RpcError::INVALID_PARAMS,
                "the pane is not accepting input".to_owned(),
            ),
        }
    }
}

/// A session socket that refuses the first call it is asked and answers the
/// next one.
///
/// One [`SessionId`] across every connection: this stands in for one session
/// holding still, not for two sessions.
struct Refusing {
    /// One entry per call this socket was asked, in order.
    asked: Arc<Mutex<Vec<String>>>,
    accepting: JoinHandle<()>,
}

impl Refusing {
    fn listening(env: &Env, how: Refusal) -> Self {
        std::fs::create_dir_all(env.runtime_dir()).expect("the runtime directory");
        let listener = UnixListener::bind(env.socket()).expect("bind the session socket");
        let asked = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&asked);
        let session = SessionId::new_v4();
        let accepting = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, Arc::clone(&seen), session, how));
            }
        });
        Self { asked, accepting }
    }

    /// The methods this socket was asked, in order.
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl Drop for Refusing {
    fn drop(&mut self) {
        self.accepting.abort();
    }
}

/// One connection: `Hello`/`Welcome`, then calls — the first refused, every
/// later one answered.
async fn serve(
    mut stream: UnixStream,
    asked: Arc<Mutex<Vec<String>>>,
    session: SessionId,
    how: Refusal,
) {
    let Some(frame) = read_frame(&mut stream).await else {
        return;
    };
    // The connect probe says hello and hangs up on the first answering byte, so
    // a frame that is not a hello is not this socket's business.
    let Ok(hello) = serde_json::from_slice::<Hello>(&frame) else {
        return;
    };
    let welcome = hello
        .accept(
            ServerInfo {
                name: "amx-refusing-test".to_owned(),
                version: "0".to_owned(),
            },
            &BTreeSet::new(),
            1,
            session,
        )
        .expect("negotiate");
    write_frame(&mut stream, &serde_json::to_vec(&welcome).unwrap()).await;

    while let Some(frame) = read_frame(&mut stream).await {
        let Ok(request) = serde_json::from_slice::<Request>(&frame) else {
            return;
        };
        let first = {
            let mut asked = asked.lock().unwrap();
            asked.push(request.method.clone());
            asked.len() == 1
        };
        let response = if first {
            Response::err(request.id, how.error())
        } else {
            Response::ok(
                request.id,
                json!({ "pane": PaneId::new_v4().to_string(), "seq": 7 }),
            )
        };
        write_frame(&mut stream, &serde_json::to_vec(&response).unwrap()).await;
    }
}

/// One control frame's payload, or `None` when the peer closed.
async fn read_frame(stream: &mut UnixStream) -> Option<Vec<u8>> {
    let mut header = [0_u8; FRAME_HEADER_LEN];
    stream.read_exact(&mut header).await.ok()?;
    let header = FrameHeader::decode(header).ok()?;
    let mut payload = vec![0_u8; header.payload_len()];
    stream.read_exact(&mut payload).await.ok()?;
    Some(payload)
}

/// Write one control frame.
async fn write_frame(stream: &mut UnixStream, payload: &[u8]) {
    let len = u32::try_from(payload.len()).expect("a small frame");
    let header = FrameHeader::new(len, CONTROL_CHANNEL);
    let _ = stream.write_all(&header.encode()).await;
    let _ = stream.write_all(payload).await;
}

/// Run one `pane send-text` against `env`'s socket, off the runtime's threads.
async fn send_text(env: &Env, pane: PaneId) -> Output {
    let child = env
        .command()
        .args([
            "pane",
            "send-text",
            "--params",
            &json!({ "target": pane.to_string(), "text": "hello" }).to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn amx");
    let out = tokio::task::spawn_blocking(move || child.wait_with_output().expect("wait for amx"))
        .await
        .expect("join the child");
    Output::of(&out)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_retriable_refusal_is_asked_again_even_for_a_verb_that_types() {
    let env = Env::new("retr-y");
    let socket = Refusing::listening(&env, Refusal::Retriable);
    let pane = PaneId::new_v4();

    let out = send_text(&env, pane).await;

    assert_eq!(out.code, Some(0), "the retry never succeeded: {out:?}");
    assert_eq!(
        socket.asked(),
        vec!["pane.send_text".to_owned(), "pane.send_text".to_owned()],
        "the refusal was not asked again"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_same_refusal_spelled_invalid_params_is_reported_and_not_asked_again() {
    let env = Env::new("retr-n");
    let socket = Refusing::listening(&env, Refusal::InvalidParams);
    let pane = PaneId::new_v4();

    let out = send_text(&env, pane).await;

    let why = out.failed().to_owned();
    assert!(
        why.contains("not accepting input"),
        "the refusal was not reported: {why}"
    );
    assert_eq!(
        socket.asked(),
        vec!["pane.send_text".to_owned()],
        "a plain refusal must be reported, not retried"
    );
}
