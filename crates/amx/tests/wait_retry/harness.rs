//! The machine `wait_retry.rs` drives: a session, its server, and the
//! background verbs that outlive one.
//!
//! Split out on arrival rather than after: the suite is four process-level
//! tests over three long-lived children each, and the scaffolding that makes
//! those observable is most of its lines. The `#[path]` convention is
//! `crates/amx-server/tests/flow_control.rs`'s.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};

use amx_client::net::{self, Session};
use amx_proto::ClientInfo;
use amx_server::session::probe::probe;
use serde_json::{Value, json};

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
