//! The machine `wait_retry.rs` drives: a session, its server, and the
//! background verbs that outlive one.
//!
//! Split out on arrival rather than after: the suite is four process-level
//! tests over three long-lived children each, and the scaffolding that makes
//! those observable is most of its lines. The `#[path]` convention is
//! `crates/amx-server/tests/flow_control.rs`'s.

use std::collections::BTreeSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};

use amx_client::net::{self, Session};
use amx_core::{PaneId, SessionId};
use amx_proto::ClientInfo;
use amx_proto::control::wait::WaitReply;
use amx_proto::frame::{CONTROL_CHANNEL, FRAME_HEADER_LEN, FrameHeader};
use amx_proto::hello::{Hello, ServerInfo};
use amx_proto::rpc::{Request, Response, RpcError};
use amx_server::conn::events::cancelled;
use amx_server::session::probe::probe;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinHandle;

use crate::support::{Env, Output, PATIENCE, wait_until};

/// A session on its own machine, with one pane and a shell that is nobody's
/// login shell.
///
/// `/bin/sh` pinned for the reason every process-level suite here pins it: a
/// pane on the developer's own shell is slow to start and an interrupted zsh
/// leaves a lock on their real history file.
pub struct Session1 {
    pub env: Env,
    pub server: Option<Child>,
    pub pane: String,
}

impl Session1 {
    pub fn new(tag: &str) -> Self {
        let env = Env::new(tag);
        std::fs::create_dir_all(env.root().join("config/amx")).expect("config dir");
        shell(&env, "/bin/sh");
        let mut session = Self {
            env,
            server: None,
            pane: String::new(),
        };
        session.serve();
        session.env.run(&["workspace", "create"]).ok();
        session.pane = state(&session.env)["panes"][0]["pane"]
            .as_str()
            .expect("the session has a pane")
            .to_owned();
        session.persisted();
        session
    }

    /// Wait until the durable snapshot names this session's pane.
    ///
    /// `Persist` writes on a debounce, so a server killed the instant after a
    /// `workspace.create` has a pane on disk only by luck — and a restore that
    /// found nothing would make every test below pass or fail for the wrong
    /// reason.
    pub fn persisted(&self) {
        let snapshot = self
            .env
            .root()
            .join("state/amx")
            .join(&self.env.session)
            .join("session.json");
        let pane = self.pane.clone();
        wait_until("the snapshot names the pane", || {
            std::fs::read_to_string(&snapshot).is_ok_and(|text| text.contains(&pane))
        });
    }

    /// Start a server for this session and wait until it answers.
    pub fn serve(&mut self) {
        let child = self
            .env
            .command()
            .args(["server", "--session", &self.env.session])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn amx server");
        self.server = Some(child);
        let socket = self.env.socket();
        wait_until("the server binds", || {
            probe(&socket).expect("probe").is_running()
        });
    }

    /// Kill this session's server outright and wait until its socket stops
    /// answering.
    ///
    /// `SIGKILL` and not `session stop`: a clean stop unlinks the socket and
    /// writes a final snapshot, which is a session *ending*. What a standing
    /// verb has to survive is a server that stops mid-sentence.
    pub fn kill_server(&mut self) {
        let mut server = self.server.take().expect("a running server");
        server.kill().expect("kill the server");
        server.wait().expect("reap the server");
        let socket = self.env.socket();
        wait_until("the socket stops answering", || {
            !probe(&socket).is_ok_and(|state| state.is_running())
        });
    }
}

impl Drop for Session1 {
    fn drop(&mut self) {
        self.env.stop();
        if let Some(mut server) = self.server.take() {
            let _ = server.kill();
            let _ = server.wait();
        }
    }
}

/// Point this environment's config at `path` as the shell every pane spawns.
pub fn shell(env: &Env, path: &str) {
    std::fs::write(
        env.root().join("config/amx/config.toml"),
        format!("[terminal]\nshell = \"{path}\"\n"),
    )
    .expect("write config.toml");
}

/// A program that exits the moment it starts, written into this environment.
///
/// Written rather than named: `/bin/true` is a path this suite happened to
/// share with the developer's machine and not with a macOS runner, and a
/// `[terminal] shell` the restore cannot spawn is not "a pane whose process
/// exits" — it is D-M1-9's *shell spawn fails → prune the pane*, which takes
/// the pane's only workspace with it and leaves the re-issued wait correctly
/// answering that no such pane exists. What the test actually needs from the
/// platform is `/bin/sh`, which is what every pane in this suite already runs.
pub fn exits_immediately(env: &Env) -> String {
    use std::os::unix::fs::PermissionsExt as _;

    let path = env.root().join("exit-now");
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write the exiting shell");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("make it exec");
    path.to_string_lossy().into_owned()
}

/// The session's state.
pub fn state(env: &Env) -> Value {
    serde_json::from_str(env.run(&["session", "state", "--params", "{}"]).ok())
        .expect("session.state replies with JSON")
}

/// How a test connection names itself.
pub fn client_info(name: &'static str) -> ClientInfo {
    ClientInfo {
        name: name.to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    }
}

/// A connection subscribed to the session's bus, for watching what other
/// connections do.
pub async fn watcher(env: &Env) -> Session {
    let stream = net::connect(&env.socket())
        .await
        .expect("connect a watcher");
    let (mut session, _welcome) = Session::attach(stream, client_info("amx-watch"), false, None)
        .await
        .expect("negotiate the watcher");
    // Before the call, never after: the pump is spawned inside the handler and
    // its first delivery can reach the socket ahead of the reply.
    session.collect_notifications();
    session
        .call("events.subscribe", json!({}))
        .await
        .expect("subscribe");
    session
}

/// Read the bus until some *other* connection attaches.
///
/// The watcher's own `client_attached` was published before it subscribed, so
/// the next one belongs to whoever the caller just started.
pub async fn await_attach(session: &mut Session) {
    let mut buf = Vec::new();
    let mut queue = Vec::new();
    let seen = tokio::time::timeout(PATIENCE, async {
        loop {
            session
                .read_frame_into(&mut buf)
                .await
                .expect("read a frame");
            let _lost = session.take_notifications(&mut queue);
            for notification in &queue {
                let attached = notification
                    .params
                    .as_ref()
                    .is_some_and(|params| params["event"]["event"] == json!("client_attached"));
                if attached {
                    return;
                }
            }
        }
    })
    .await;
    assert!(seen.is_ok(), "no connection reached the session");
}

/// A verb running in the background, writing to a file a test can watch grow.
pub struct Standing {
    pub child: Child,
    out: PathBuf,
    err: PathBuf,
}

impl Standing {
    /// Start `args` with its output captured to files under `env`'s root.
    ///
    /// Files rather than pipes: a test that has to watch a long-running
    /// relay's stdout *while it runs* cannot read a pipe without blocking on
    /// it, and a poll of a file is the same observation without the deadlock.
    pub fn start(env: &Env, tag: &str, args: &[&str]) -> Self {
        let out = env.root().join(format!("{tag}.out"));
        let err = env.root().join(format!("{tag}.err"));
        let child = env
            .command()
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(File::create(&out).expect("create stdout")))
            .stderr(Stdio::from(File::create(&err).expect("create stderr")))
            .spawn()
            .expect("spawn amx");
        Self { child, out, err }
    }

    /// Whether it is still running.
    pub fn running(&mut self) -> bool {
        self.child.try_wait().expect("try_wait").is_none()
    }

    /// What it has written to stdout so far.
    pub fn stdout(&self) -> String {
        read(&self.out)
    }

    /// Wait for it to finish and read what it wrote.
    pub fn finish(mut self) -> Output {
        let status = self.child.wait().expect("wait for amx");
        Output {
            code: status.code(),
            stdout: read(&self.out),
            stderr: read(&self.err),
        }
    }

    /// Stop it and read what it wrote.
    pub fn stop(mut self) -> Output {
        let _ = self.child.kill();
        let _ = self.child.wait();
        Output {
            code: Some(0),
            stdout: read(&self.out),
            stderr: read(&self.err),
        }
    }
}

/// A session socket that abandons the first `wait` it is asked and answers the
/// next one.
///
/// The deterministic half of D-M3-7, and the reason it is not a handoff: an
/// exporter cuts every standing wait short with
/// [`RpcError::WAIT_ABANDONED`](amx_proto::RpcError::WAIT_ABANDONED) *and* then
/// closes the socket, so which of the two a client observes is a race decided
/// by how loaded the machine is. Here the reply always wins — the socket is
/// well-behaved and stays open — so a client that treats an abandoned wait as a
/// failure has nowhere to hide, on any machine, on every run.
///
/// The abandonment is [`cancelled`], the server's own constructor, so the
/// fixture cannot drift from what a real session sends.
pub struct Abandoning {
    /// The params of every `wait` this socket was asked, in order.
    asked: Arc<Mutex<Vec<Value>>>,
    accepting: JoinHandle<()>,
}

impl Abandoning {
    /// Bind `env`'s session socket and start answering on it.
    ///
    /// One [`SessionId`] across every connection: this stands in for a session
    /// that changed processes, not for two different sessions.
    pub fn listening(env: &Env, pane: PaneId) -> Self {
        std::fs::create_dir_all(env.runtime_dir()).expect("the runtime directory");
        let listener = UnixListener::bind(env.socket()).expect("bind the session socket");
        let asked = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&asked);
        let session = SessionId::new_v4();
        let accepting = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve_wait(stream, Arc::clone(&seen), session, pane));
            }
        });
        Self { asked, accepting }
    }

    /// The params of every `wait` it was asked, in order.
    pub fn asked(&self) -> Vec<Value> {
        self.asked.lock().unwrap().clone()
    }
}

impl Drop for Abandoning {
    fn drop(&mut self) {
        self.accepting.abort();
    }
}

/// One connection: `Hello`/`Welcome`, then `wait` calls — the first abandoned,
/// every later one answered.
async fn serve_wait(
    mut stream: UnixStream,
    asked: Arc<Mutex<Vec<Value>>>,
    session: SessionId,
    pane: PaneId,
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
                name: "amx-abandoning-test".to_owned(),
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
        let params = request.params.clone().unwrap_or(Value::Null);
        let first = {
            let mut asked = asked.lock().unwrap();
            asked.push(params);
            asked.len() == 1
        };
        let response = match (request.method.as_str(), first) {
            ("wait", true) => Response::err(request.id, cancelled()),
            ("wait", false) => Response::ok(
                request.id,
                serde_json::to_value(WaitReply {
                    pane,
                    satisfied: true,
                    agent: None,
                    status: Some(0),
                    seq: 2,
                })
                .unwrap(),
            ),
            (method, _) => Response::err(request.id, RpcError::method_not_found(method)),
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

pub fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// The reply a verb printed, as JSON.
pub fn reply(out: &Output) -> Value {
    serde_json::from_str(out.ok()).unwrap_or_else(|err| panic!("{err}: {out:?}"))
}

/// Give `pane` a label, publishing one event.
pub fn rename(env: &Env, pane: &str, label: &str) {
    env.run(&[
        "pane",
        "rename",
        "--params",
        &json!({ "pane": pane, "label": label }).to_string(),
    ])
    .ok();
}

/// Publish more events than the replay ring holds, over one connection.
///
/// `DEFAULT_REPLAY_CAPACITY` is 1024 and one `pane.rename` is one event. Over
/// a single socket that is a fraction of a second; a thousand `amx`
/// invocations would be a minute of process spawning to prove the same thing.
pub async fn flood(env: &Env) {
    let stream = net::connect(&env.socket())
        .await
        .expect("connect to the session");
    let (mut session, _welcome) = Session::attach(stream, client_info("amx-flood"), false, None)
        .await
        .expect("negotiate");
    let state = session
        .call("session.state", json!({}))
        .await
        .expect("read state");
    let pane = state["panes"][0]["pane"].clone();
    for n in 0..1_100 {
        session
            .call(
                "pane.rename",
                json!({ "pane": pane, "label": format!("f{n}") }),
            )
            .await
            .expect("rename");
    }
}
