//! T17: session lifecycle — the connect probe, named sessions, the daemon
//! spawn, the registry (04 §1).
//!
//! Everything here drives real sockets under a real temp runtime root. The
//! rules being pinned are the ones a second implementation would get wrong:
//! a socket file is never evidence of a running server, two named sessions
//! share nothing, and a stale socket is disambiguated by a connect probe rather
//! than by a lock file that would need a staleness rule of its own.
//!
//! `SIGTERM`-driven stop is not here: `session::registry::stop` signals the
//! process on the other end of the socket, and an in-process server's peer is
//! this test binary. It is covered end to end over two processes by
//! `crates/amx/tests/session_cli.rs::session_stop_shuts_down_cleanly_and_removes_the_socket`.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::os::fd::AsFd as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use amx_core::{Ctx, Env, SessionName};
use amx_proto::frame::{CONTROL_CHANNEL, FRAME_HEADER_LEN};
use amx_proto::{ClientInfo, FrameHeader, Hello, Request, RequestId, Response, Welcome};
use amx_server::session::probe::Probe;
use amx_server::session::registry::{DeleteError, StopOutcome};
use amx_server::session::serve::{ServeReport, StopOn};
use amx_server::session::{daemon, probe, registry, serve};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::task::JoinHandle;

mod support;

use support::{PATIENCE, TempDir, wait_until};

/// An `Env` whose every root is under `root` — the whole point of `Env` being
/// a value: two of these in one process are two independent machines.
fn env_under(root: &Path) -> Env {
    Env {
        runtime_dir: Some(root.join("run")),
        state_home: Some(root.join("state")),
        config_home: Some(root.join("config")),
        home: Some(root.join("home")),
        session: None,
        tmpdir: Some(root.join("tmp")),
        mise_installs_dir: None,
    }
}

fn ctx_for(env: &Env, name: &str) -> Ctx {
    Ctx::for_session(SessionName::new(name).expect("valid session name"), env)
        .expect("derive the session context")
}

/// A server for one `Ctx`, running until it is cancelled.
struct Session {
    ctx: Ctx,
    served: JoinHandle<Result<ServeReport, serve::ServeError>>,
}

impl Session {
    /// Start one and wait until its socket answers.
    async fn start(ctx: Ctx) -> Self {
        let served = tokio::spawn(serve::serve(ctx.clone(), StopOn::Cancellation));
        daemon::await_ready(&ctx.socket, PATIENCE)
            .await
            .expect("the server bound its socket");
        Self { ctx, served }
    }

    async fn stop(self) -> ServeReport {
        self.ctx.cancel.cancel();
        self.served
            .await
            .expect("the serve task finished")
            .expect("the server ran")
    }
}

// ------------------------------------------------------------- the probe

#[tokio::test]
async fn a_socket_file_is_never_evidence_that_a_server_is_running() {
    let dir = TempDir::new("probe");
    let env = env_under(dir.path());
    let ctx = ctx_for(&env, "probe");

    assert_eq!(
        probe::probe(&ctx.socket).expect("probe"),
        Probe::Absent,
        "nothing has ever bound here"
    );

    let session = Session::start(ctx.clone()).await;
    assert_eq!(
        probe::probe(&ctx.socket).expect("probe"),
        Probe::Running,
        "a server answered"
    );
    session.stop().await;

    // A clean shutdown takes the socket with it, so leave one behind the way a
    // killed server would: bind and drop, which unlinks nothing.
    drop(std::os::unix::net::UnixListener::bind(&ctx.socket).expect("leave a stale socket"));
    assert!(ctx.socket.exists());
    assert_eq!(
        probe::probe(&ctx.socket).expect("probe"),
        Probe::Stale,
        "the file outlived its process, and only connecting can tell"
    );
}

#[tokio::test]
async fn a_dead_listener_kept_connectable_by_an_inherited_descriptor_is_stale() {
    let dir = TempDir::new("inherited");
    let env = env_under(dir.path());
    let ctx = ctx_for(&env, "inherited");
    std::fs::create_dir_all(&ctx.runtime_dir).expect("create the runtime dir");

    // The state a concurrent fork leaves behind: fork copies every open
    // descriptor and close-on-exec closes the copies only at exec, so for a
    // moment after its owner closes, a dead server's listener is still
    // connectable — connects complete into a backlog nothing will ever
    // accept. A duplicated descriptor closed shortly after is that exact
    // kernel state, minus the fork.
    let listener = std::os::unix::net::UnixListener::bind(&ctx.socket).expect("bind");
    let inherited = listener
        .as_fd()
        .try_clone_to_owned()
        .expect("duplicate the listening descriptor");
    drop(listener);

    // The window is the scenario, not a wait for a condition: the probe must
    // be mid-connection when the descriptor goes. Expiring short only moves
    // the close before the probe's connect, which reads refused — still
    // `Stale`; expiring long is covered by the probe's one-second patience.
    let exec = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20)); // deliberate
        drop(inherited);
    });
    assert_eq!(
        probe::probe(&ctx.socket).expect("probe"),
        Probe::Stale,
        "a backlog nobody will ever accept from is not a running server"
    );
    exec.join().expect("the closing thread");
}

#[tokio::test]
async fn a_listener_that_holds_on_without_answering_is_never_stale() {
    let dir = TempDir::new("holdout");
    let env = env_under(dir.path());
    let ctx = ctx_for(&env, "holdout");
    std::fs::create_dir_all(&ctx.runtime_dir).expect("create the runtime dir");

    // Same inherited-descriptor state, but the holder never lets go. The
    // probe cannot tell this from a wedged-but-alive server, and the safe
    // reading of "alive" is the one that binds nothing over it.
    let listener = std::os::unix::net::UnixListener::bind(&ctx.socket).expect("bind");
    let inherited = listener
        .as_fd()
        .try_clone_to_owned()
        .expect("duplicate the listening descriptor");
    drop(listener);

    assert_eq!(
        probe::probe(&ctx.socket).expect("probe"),
        Probe::Running,
        "an unanswered connection that stays open is treated as a live holder"
    );
    drop(inherited);
}

#[tokio::test]
async fn stale_socket_is_replaced_after_a_failed_connect_probe() {
    let dir = TempDir::new("stale");
    let env = env_under(dir.path());
    let ctx = ctx_for(&env, "stale");
    std::fs::create_dir_all(&ctx.runtime_dir).expect("create the runtime dir");

    drop(std::os::unix::net::UnixListener::bind(&ctx.socket).expect("leave a stale socket"));
    assert!(ctx.socket.exists(), "the stale file is there to start with");

    assert_eq!(
        probe::clear_if_stale(&ctx.socket).expect("probe"),
        Probe::Stale
    );
    assert!(
        !ctx.socket.exists(),
        "a socket nothing answers on is removed, not worked around"
    );

    // And the path is free again: a server binds where the stale file was.
    let session = Session::start(ctx.clone()).await;
    assert_eq!(probe::probe(&ctx.socket).expect("probe"), Probe::Running);
    session.stop().await;
}

#[tokio::test]
async fn clear_if_stale_never_removes_a_socket_that_answers() {
    let dir = TempDir::new("live");
    let env = env_under(dir.path());
    let ctx = ctx_for(&env, "live");

    let session = Session::start(ctx.clone()).await;
    assert_eq!(
        probe::clear_if_stale(&ctx.socket).expect("probe"),
        Probe::Running
    );
    assert!(
        ctx.socket.exists(),
        "the removal is for sockets nothing answers on, and this one answered"
    );
    session.stop().await;
}

// ---------------------------------------------------------- named sessions

#[tokio::test]
async fn two_named_sessions_are_fully_isolated() {
    let dir = TempDir::new("named");
    let env = env_under(dir.path());
    let work = ctx_for(&env, "work");
    let play = ctx_for(&env, "play");

    assert_ne!(work.runtime_dir, play.runtime_dir);
    assert_ne!(work.socket, play.socket);
    assert_ne!(work.state_dir, play.state_dir);
    assert!(work.runtime_dir.ends_with("amx/work"));
    assert!(play.state_dir.ends_with("amx/play"));

    let work_server = Session::start(work.clone()).await;
    let play_server = Session::start(play.clone()).await;

    let work_id = ping(&work.socket).await;
    let play_id = ping(&play.socket).await;
    assert_ne!(
        work_id, play_id,
        "two names are two server instances, not two doors into one"
    );

    // Stopping one leaves the other untouched.
    work_server.stop().await;
    assert_eq!(probe::probe(&work.socket).expect("probe"), Probe::Absent);
    assert_eq!(probe::probe(&play.socket).expect("probe"), Probe::Running);
    assert_eq!(ping(&play.socket).await, play_id);

    play_server.stop().await;
}

#[test]
fn a_session_context_needs_a_runtime_root_and_a_state_root() {
    // A shared root ($TMPDIR here, /tmp when nothing at all is set) gets a
    // per-user amx level, so one user's directory cannot be planted for
    // another on a world-writable root.
    let dir = TempDir::new("tmpdir-fallback");
    let uid = rustix::process::getuid().as_raw();
    let env = Env {
        tmpdir: Some(dir.path().to_path_buf()),
        home: Some(PathBuf::from("/home/someone")),
        ..Env::default()
    };
    let ctx = Ctx::for_session(SessionName::default(), &env).expect("derive");
    assert_eq!(
        ctx.socket,
        dir.path().join(format!("amx-{uid}/default/sock"))
    );
    assert_eq!(
        ctx.state_dir,
        PathBuf::from("/home/someone/.local/state/amx/default")
    );
    let meta = std::fs::metadata(dir.path().join(format!("amx-{uid}")))
        .expect("the per-user level was created");
    assert_eq!(
        meta.permissions().mode() & 0o777,
        0o700,
        "the per-user level is private"
    );

    // A relative root would resolve against the process's cwd, so two runs of
    // the same session name would find two different sockets.
    let relative = Env {
        runtime_dir: Some(PathBuf::from("run")),
        home: Some(PathBuf::from("/home/someone")),
        ..Env::default()
    };
    assert!(Ctx::for_session(SessionName::default(), &relative).is_err());

    // A state root is still required: /tmp is a resort for the socket, whose
    // loss a reboot already implies, never for persistent state.
    let stateless = Env {
        tmpdir: Some(dir.path().to_path_buf()),
        ..Env::default()
    };
    assert!(Ctx::for_session(SessionName::default(), &stateless).is_err());
}

#[test]
fn a_planted_per_user_level_under_a_shared_root_is_refused() {
    let dir = TempDir::new("planted-level");
    let uid = rustix::process::getuid().as_raw();
    let env = Env {
        tmpdir: Some(dir.path().to_path_buf()),
        home: Some(PathBuf::from("/home/someone")),
        ..Env::default()
    };

    // A symlink where the per-user level belongs would hand the socket to
    // whatever directory the planter chose; it must be seen and refused.
    std::os::unix::fs::symlink("/somewhere/else", dir.path().join(format!("amx-{uid}")))
        .expect("plant a symlink");
    assert!(
        Ctx::for_session(SessionName::default(), &env).is_err(),
        "a planted symlink is a refusal, not a home"
    );
}

// -------------------------------------------------------------- the registry

#[tokio::test]
async fn session_list_reports_running_servers_only() {
    let dir = TempDir::new("list");
    let env = env_under(dir.path());
    let live = ctx_for(&env, "live");
    let dead = ctx_for(&env, "dead");
    let cold = ctx_for(&env, "cold");

    let running = Session::start(live.clone()).await;

    // A session that died without cleaning up: directory there, socket there,
    // nothing answering.
    std::fs::create_dir_all(&dead.runtime_dir).expect("create");
    drop(std::os::unix::net::UnixListener::bind(&dead.socket).expect("stale socket"));
    // And one that was never started at all: directory, no socket.
    std::fs::create_dir_all(&cold.runtime_dir).expect("create");

    let listed = registry::list(&env).expect("list");
    let names: Vec<String> = listed
        .iter()
        .map(|entry| entry.session.to_string())
        .collect();
    assert_eq!(
        names,
        vec!["live".to_owned()],
        "a directory is evidence a session ran, never that it is running"
    );
    assert_eq!(listed[0].socket, live.socket);

    running.stop().await;
    assert!(
        registry::list(&env).expect("list").is_empty(),
        "a stopped session drops out of the list immediately"
    );
}

#[test]
fn listing_an_environment_that_has_never_run_a_session_is_empty_not_an_error() {
    let dir = TempDir::new("norunroot");
    let env = env_under(dir.path());
    assert!(registry::list(&env).expect("list").is_empty());
}

#[tokio::test]
async fn delete_refuses_a_running_session_and_removes_a_stopped_one() {
    let dir = TempDir::new("delete");
    let env = env_under(dir.path());
    let ctx = ctx_for(&env, "doomed");
    std::fs::create_dir_all(&ctx.state_dir).expect("create the state dir");

    let session = Session::start(ctx.clone()).await;
    let refused = registry::delete(&ctx).expect_err("a running session must not be deleted");
    assert!(matches!(refused, DeleteError::Running { .. }), "{refused}");
    assert!(ctx.runtime_dir.exists());
    assert!(ctx.state_dir.exists());

    session.stop().await;
    registry::delete(&ctx).expect("a stopped session deletes");
    assert!(!ctx.runtime_dir.exists(), "the runtime directory is gone");
    assert!(!ctx.state_dir.exists(), "and so is the state directory");

    let missing = registry::delete(&ctx).expect_err("nothing left to delete");
    assert!(
        matches!(missing, DeleteError::NoSuchSession { .. }),
        "{missing}"
    );
}

#[tokio::test]
async fn stopping_a_session_nobody_is_running_is_not_an_error() {
    let dir = TempDir::new("stopnone");
    let env = env_under(dir.path());
    let ctx = ctx_for(&env, "absent");
    assert_eq!(
        registry::stop(&ctx, PATIENCE).await.expect("stop"),
        StopOutcome::NotRunning
    );
}

// --------------------------------------------------------------- the runtime

#[tokio::test]
async fn cancelling_a_session_joins_every_task_and_removes_the_socket() {
    let dir = TempDir::new("shutdown");
    let env = env_under(dir.path());
    let ctx = ctx_for(&env, "shutdown");

    let session = Session::start(ctx.clone()).await;
    // One connected client, so the shutdown has a connection task to join too.
    // It has completed the handshake on purpose: a peer that connects and never
    // says hello pins its connection task and hangs this shutdown — see the
    // gap noted in the T17 report against `conn::serve`.
    let client = attach(&ctx.socket).await;

    let report = session.stop().await;
    assert!(report.clean(), "{report:?}");
    assert_eq!(
        report.shutdown.joined, 6,
        "the core, the agent hub, the gateway, persistence, the config watcher \
         and the export orchestrator are the runtime's tasks when signals are \
         off"
    );
    // The readiness probe is itself a connection, so the count is "at least the
    // client"; what matters is that every accept was joined, which `clean` is.
    assert!(report.gateway.accepted >= 1, "{report:?}");
    assert_eq!(
        u64::try_from(report.gateway.joined).unwrap(),
        report.gateway.accepted
    );
    assert!(
        !ctx.socket.exists(),
        "a clean shutdown leaves no socket for the next start to probe"
    );
    drop(client);
}

#[tokio::test]
async fn a_second_server_cannot_take_a_running_sessions_socket() {
    let dir = TempDir::new("second");
    let env = env_under(dir.path());
    let ctx = ctx_for(&env, "contested");

    let session = Session::start(ctx.clone()).await;

    // The second server derives its own `Ctx` for the same session — a
    // different process would — and must lose at `bind`.
    let rival = ctx_for(&env, "contested");
    let err = serve::serve(rival, StopOn::Cancellation)
        .await
        .expect_err("the socket is taken");
    assert!(err.to_string().contains("already listening"), "{err}");

    assert_eq!(probe::probe(&ctx.socket).expect("probe"), Probe::Running);
    session.stop().await;
}

// ------------------------------------------------------------ daemonization

#[tokio::test]
async fn spawn_detached_starts_a_session_leader_with_null_stdio() {
    let dir = TempDir::new("detach");
    let out = dir.path().to_path_buf();
    // `/proc/$$/stat`'s sixth field is the session id and `$$` is the shell's
    // own pid: they are equal only if the shell is a session leader, which is
    // exactly what `setsid` made it. Everything is read through `/proc/$$`
    // rather than `/proc/self` because `self` would be the `readlink` process,
    // whose stdout is the redirection this very line sets up.
    //
    // The fd probes capture first and redirect after, and that order is what
    // makes the script portable: `readlink /proc/$$/fd/1 > file` reports
    // `/dev/null` under bash (the external readlink is forked, `$$`'s own fd1
    // is untouched) but reports `file` itself under dash/ash — Ubuntu's
    // `/bin/sh` — which applies the redirect to the shell's fd1 before running
    // the command. A command substitution's subshell inherits the shell's fds
    // without modifying them, and `$$` still names the shell inside it.
    #[cfg(target_os = "linux")]
    let script = format!(
        "cut -d' ' -f6 /proc/$$/stat > {out}/sess; \
         echo $$ > {out}/pid; \
         fd0=$(readlink /proc/$$/fd/0); \
         fd1=$(readlink /proc/$$/fd/1); \
         fd2=$(readlink /proc/$$/fd/2); \
         printf '%s\\n' \"$fd0\" > {out}/fd0; \
         printf '%s\\n' \"$fd1\" > {out}/fd1; \
         printf '%s\\n' \"$fd2\" > {out}/fd2; \
         echo done > {out}/ok",
        out = out.display()
    );
    // darwin has no `/proc`, so the probes change shape: the fds are compared
    // against `/dev/null` by identity (`-ef` stats through `/dev/fd/$n` to
    // the underlying vnode), and the session id is read from *outside*, via
    // `getsid(2)` on the pid the script wrote — which is why the script then
    // holds itself alive: a session id can only be read off a live process.
    #[cfg(target_os = "macos")]
    let script = format!(
        "echo $$ > {out}/pid; \
         for n in 0 1 2; do \
           if [ /dev/fd/$n -ef /dev/null ]; \
           then echo /dev/null > {out}/fd$n; \
           else echo not-null > {out}/fd$n; fi; \
         done; \
         echo done > {out}/ok; \
         sleep 30",
        out = out.display()
    );
    let mut child = daemon::spawn_detached(
        Path::new("/bin/sh"),
        &[OsString::from("-c"), OsString::from(script)],
    )
    .expect("spawn");

    let ok = out.join("ok");
    wait_until("the detached process ran", || ok.exists()).await;

    let read = |name: &str| {
        std::fs::read_to_string(out.join(name))
            .expect("read")
            .trim()
            .to_owned()
    };
    #[cfg(target_os = "linux")]
    {
        let _ = child.wait();
        assert_eq!(
            read("sess"),
            read("pid"),
            "a daemon is a session leader: its terminal can close, hang up or \
             interrupt its own foreground group and none of it reaches the server"
        );
    }
    #[cfg(target_os = "macos")]
    {
        let pid: i32 = read("pid").parse().expect("the daemon's pid");
        let daemon = rustix::process::Pid::from_raw(pid).expect("a live pid");
        assert_eq!(
            rustix::process::getsid(Some(daemon)).expect("session id"),
            daemon,
            "a daemon is a session leader: its terminal can close, hang up or \
             interrupt its own foreground group and none of it reaches the server"
        );
        // The script held itself alive so the session id could be read off a
        // live process; hang it up rather than leaving it to linger.
        let _ = rustix::process::kill_process(daemon, rustix::process::Signal::TERM);
        let _ = child.wait();
    }
    for fd in ["fd0", "fd1", "fd2"] {
        assert_eq!(
            read(fd),
            "/dev/null",
            "{fd} must not be the client's terminal"
        );
    }
}

#[tokio::test]
async fn await_ready_waits_for_the_socket_and_gives_up_on_a_deadline() {
    let dir = TempDir::new("ready");
    let env = env_under(dir.path());
    let ctx = ctx_for(&env, "ready");

    let missing = daemon::await_ready(&ctx.socket, Duration::from_millis(20))
        .await
        .expect_err("nothing is coming");
    assert!(matches!(missing, daemon::DaemonError::NotReady { .. }));

    let starting = ctx.clone();
    let late = tokio::spawn(async move {
        // Start late so await_ready has to actually wait; a long delay
        // only makes it wait longer, never fail.
        tokio::time::sleep(Duration::from_millis(30)).await; // deliberate
        Session::start(starting).await
    });
    daemon::await_ready(&ctx.socket, PATIENCE)
        .await
        .expect("the server turned up");
    late.await.expect("started").stop().await;
}

// ------------------------------------------------------------------ helpers

/// Connect and complete the handshake.
async fn attach(socket: &Path) -> UnixStream {
    let mut stream = UnixStream::connect(socket).await.expect("connect");
    let hello = Hello {
        proto: amx_proto::version::window(),
        features: BTreeSet::new(),
        client: ClientInfo {
            name: "amx-t17-test".to_owned(),
            version: "0.0.0".to_owned(),
            term: None,
        },
        attach: false,
        resume: None,
    };
    send(&mut stream, &serde_json::to_vec(&hello).expect("encode")).await;
    let welcome: Welcome = serde_json::from_slice(&read_frame(&mut stream).await).expect("welcome");
    assert!(welcome.proto >= amx_proto::version::PROTO_MIN);
    stream
}

/// Hello, then `ping`: the session id the server answers with.
async fn ping(socket: &Path) -> String {
    let mut stream = attach(socket).await;
    let request = Request::new(
        RequestId::Number(1),
        "ping",
        Some(Value::Object(serde_json::Map::new())),
    );
    send(&mut stream, &serde_json::to_vec(&request).expect("encode")).await;
    let response: Response = serde_json::from_slice(&read_frame(&mut stream).await).expect("reply");
    let amx_proto::RpcOutcome::Result(result) = response.outcome else {
        panic!("ping failed");
    };
    result["session"].as_str().expect("a session id").to_owned()
}

async fn send(stream: &mut UnixStream, payload: &[u8]) {
    let header = FrameHeader::new(payload.len() as u32, CONTROL_CHANNEL);
    stream.write_all(&header.encode()).await.expect("write");
    stream.write_all(payload).await.expect("write");
}

async fn read_frame(stream: &mut UnixStream) -> Vec<u8> {
    let mut header = [0_u8; FRAME_HEADER_LEN];
    tokio::time::timeout(PATIENCE, stream.read_exact(&mut header))
        .await
        .expect("a frame arrived")
        .expect("read");
    let header = FrameHeader::decode(header).expect("decode");
    let mut payload = vec![0_u8; header.payload_len()];
    stream.read_exact(&mut payload).await.expect("read");
    payload
}
