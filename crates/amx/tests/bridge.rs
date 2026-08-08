//! W11 acceptance: the byte bridge, and the local half that drives it (D-M3-9).
//!
//! Tier 1 of the three the plan names: **no ssh is involved anywhere in this
//! file**, and it therefore runs on every platform, every run. That is not a
//! weaker test than the loopback-sshd one — it is the tier that covers every
//! byte of *amx* code in the remote path, because ssh's only job is to move
//! stdio between two machines and a socketpair moves it between two processes.
//! `tests/remote_ssh.rs` covers the transport itself, on Linux, when a loopback
//! sshd is available (R-M3-6).
//!
//! The seeding test fakes only `ssh` — a script on `PATH` — and runs the real
//! `amx --remote` against it on a real terminal. What it exercises is amx's
//! whole decision: probe the platform, compare it against this machine's, offer
//! or refuse, and stream the bytes.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::fs;
use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use amx_client::net::Session;
use amx_proto::ClientInfo;
use amx_server::session::probe::probe;
use serde_json::json;

mod support;

use support::{Rig, wait_until};

const ROWS: u16 = 24;
const COLS: u16 = 80;

// ---------------------------------------------------------------- the splice

/// Spawn `amx _bridge <extra>` with a socketpair as its stdin and stdout, the
/// way `ssh` gives it the channel's two halves.
fn bridge_child(rig: &Rig, exe: &Path, extra: &[&str]) -> (Child, tokio::net::UnixStream) {
    let (mine, theirs) = StdUnixStream::pair().expect("socketpair");
    let stdin = OwnedFd::from(theirs.try_clone().expect("dup"));
    let stdout = OwnedFd::from(theirs);
    let child = rig
        .command(exe)
        .arg("_bridge")
        .args(extra)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn amx _bridge");
    mine.set_nonblocking(true).expect("non-blocking");
    let local = tokio::net::UnixStream::from_std(mine).expect("adopt");
    (child, local)
}

fn client() -> ClientInfo {
    ClientInfo {
        name: "amx-bridge-test".to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    }
}

#[tokio::test]
async fn a_bridge_child_over_a_socketpair_attaches_and_round_trips_a_verb() {
    let mut rig = Rig::new("brdg");
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_amx"));
    rig.serve(&exe);

    let (mut child, local) = bridge_child(&rig, &exe, &[]);

    // The client is handed the socketpair and told nothing else. Every byte
    // from here down — the hello, the negotiated version, the reply — crossed
    // a subprocess's stdio, and `Session::attach` never learned it.
    let (mut session, welcome) = Session::attach(local, client(), false, None)
        .await
        .expect("negotiate over the bridge");
    assert_eq!(
        welcome.proto,
        amx_proto::version::PROTO_MAX,
        "the handshake negotiated over the bridge exactly as it does locally"
    );

    let reply = session
        .call("ping", json!({}))
        .await
        .expect("ping over the bridge");
    assert!(
        reply["seq"].is_u64(),
        "a verb answered over the bridge: {reply}"
    );

    // The session on the far side of the splice is the one the socket serves,
    // not a second one the bridge invented: a workspace created through the
    // bridge is visible to a client connected straight to the socket.
    let created = session
        .call("workspace.create", json!({ "focus": false }))
        .await
        .expect("create a workspace over the bridge");
    let workspace = created["workspace"].clone();
    assert!(workspace.is_string(), "{created}");

    let direct = amx_client::net::connect(&rig.socket())
        .await
        .expect("connect straight to the socket");
    let (mut beside, _) = Session::attach(direct, client(), false, None)
        .await
        .expect("negotiate on the socket");
    let state = beside
        .call("session.state", json!({}))
        .await
        .expect("state on the socket");
    assert!(
        state["workspaces"]
            .as_array()
            .expect("a workspace list")
            .iter()
            .any(|w| w["workspace"] == workspace),
        "the bridge reached some other session: {workspace} is not in {state}"
    );
    drop(beside);

    // Dropping the client ends the splice: stdin reaches end of input, the
    // bridge half-closes, the server retires the connection, and the child
    // exits of its own accord.
    drop(session);
    wait_until("the bridge child to exit", || {
        child.try_wait().expect("try_wait").is_some()
    });
    let status = child.wait().expect("reap");
    assert!(
        status.success(),
        "a splice that ran to the end of its session exited {status:?}"
    );
}

#[test]
fn a_bridge_exits_with_the_connect_error_when_no_session_answers() {
    let rig = Rig::new("brno");
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_amx"));

    // No `--daemonize`: a bridge that cannot reach a session must say so before
    // the first protocol byte rather than splice a socket nobody serves.
    let done = rig.run(&exe, &["_bridge"]);
    let err = done.failed();
    assert!(
        err.contains("connect to session") && err.contains(&rig.session),
        "the refusal names the session it could not reach: {done:?}"
    );
    assert!(
        !rig.socket().exists(),
        "a bridge without --daemonize started a server"
    );
}

#[tokio::test]
async fn bridge_daemonize_starts_a_server_when_none_answers() {
    let rig = Rig::new("brdm");
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_amx"));
    assert!(
        !probe(&rig.socket()).expect("probe").is_running(),
        "nothing may be running before this starts one"
    );

    let (mut child, local) = bridge_child(&rig, &exe, &["--daemonize"]);

    let (mut session, _welcome) = Session::attach(local, client(), false, None)
        .await
        .expect("negotiate with the server the bridge started");
    let reply = session
        .call("ping", json!({}))
        .await
        .expect("ping the server the bridge started");
    assert!(reply["seq"].is_u64(), "{reply}");

    // And it is a real daemonized server on the real socket, not something the
    // bridge holds in its own process: it answers a probe from outside.
    assert!(
        probe(&rig.socket()).expect("probe").is_running(),
        "--daemonize left nothing listening on {}",
        rig.socket().display()
    );

    drop(session);
    let _ = child.wait();
    // Started here, so stopped here: a daemon outlives the test that spawned it.
    let _ = rig.run(&exe, &["session", "stop"]);
}

// ---------------------------------------------------------------- seeding

/// A fake `ssh` on `PATH`, which answers each of the three commands amx sends.
///
/// It branches on the remote command string alone, which is what makes it a
/// test of amx's side: the bridge script, the `uname` probe and the install
/// script are told apart by their own content, so a change to any of them that
/// broke the shape would land here as a failure rather than as a silent
/// fallthrough.
fn fake_ssh(rig: &Rig) -> PathBuf {
    let dir = rig.root().join("fake-bin");
    fs::create_dir_all(&dir).expect("create the fake bin dir");
    let path = dir.join("ssh");
    fs::write(
        &path,
        "#!/bin/sh\n\
         # ssh -T -e none <host> <command>\n\
         command=$5\n\
         case \"$command\" in\n\
         *_bridge*) echo 'amx-bridge: no amx on this host' >&2; exit 127 ;;\n\
         *uname*) printf '%s\\n%s\\n' \"$AMX_FAKE_OS\" \"$AMX_FAKE_ARCH\" ;;\n\
         *) cat > \"$AMX_FAKE_INSTALL\" ;;\n\
         esac\n",
    )
    .expect("write the fake ssh");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod the fake ssh");
    dir
}

/// What `uname` says here, read the same way amx reads it.
fn local_uname(flag: &str) -> String {
    let out = Command::new("uname").arg(flag).output().expect("run uname");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[test]
fn a_missing_remote_amx_offers_seeding_only_on_matching_uname_and_refuses_cross_platform_with_the_reason()
 {
    let rig = Rig::new("seed");
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_amx"));
    let fake = fake_ssh(&rig);
    let path = format!(
        "{}:{}",
        fake.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let landed = rig.root().join("streamed-amx");

    // --------------------------------------------- the platforms do not match
    //
    // A machine that is genuinely not this one, whichever this one is: the
    // fixture is derived from the local `uname` rather than hard-coded, so the
    // refusal is exercised on both tier-1 platforms and not only on the one
    // that happens not to be named here.
    let (other_os, other_arch) = if local_uname("-s") == "Linux" {
        ("Darwin", "arm64")
    } else {
        ("Linux", "x86_64")
    };
    let mut command = rig.command(&exe);
    command
        .env("PATH", &path)
        .env("AMX_FAKE_OS", other_os)
        .env("AMX_FAKE_ARCH", other_arch)
        .env("AMX_FAKE_INSTALL", &landed);
    let mut term = support::env::spawn_on_tty(&mut command, &["--remote", "elsewhere"], ROWS, COLS);
    term.wait_output("the refusal", |seen| {
        let text = String::from_utf8_lossy(seen);
        text.contains("cannot seed elsewhere")
    });
    let refusal = String::from_utf8_lossy(term.output()).into_owned();
    assert!(
        refusal.contains(other_os) && refusal.contains(other_arch),
        "the refusal names the platform it found: {refusal}"
    );
    assert!(
        refusal.contains(&local_uname("-s")) && refusal.contains(&local_uname("-m")),
        "and the platform it is: {refusal}"
    );
    assert!(
        refusal.contains("no release channel exists yet"),
        "and why that is the end of it, rather than a promise: {refusal}"
    );
    assert_ne!(term.wait(), Some(0), "a refusal is not a success");
    assert!(
        !landed.exists(),
        "a cross-platform refusal streamed the binary anyway"
    );

    // ------------------------------------------------- the platforms do match
    let mut command = rig.command(&exe);
    command
        .env("PATH", &path)
        .env("AMX_FAKE_OS", local_uname("-s"))
        .env("AMX_FAKE_ARCH", local_uname("-m"))
        .env("AMX_FAKE_INSTALL", &landed);
    let mut term = support::env::spawn_on_tty(&mut command, &["--remote", "twin"], ROWS, COLS);
    term.wait_output("the offer", |seen| {
        String::from_utf8_lossy(seen).contains("Install it there?")
    });
    let offer = String::from_utf8_lossy(term.output()).into_owned();
    assert!(
        offer.contains("twin has no amx"),
        "the offer says what is wrong first: {offer}"
    );
    assert!(
        offer.contains("~/.local/bin/amx"),
        "and where it proposes to put one: {offer}"
    );

    // Nothing has been written yet: the confirmation is what installs.
    assert!(
        !landed.exists(),
        "the offer installed before it was answered"
    );
    term.send(b"y\n");
    term.wait_output("the install to be reported", |seen| {
        String::from_utf8_lossy(seen).contains("Installed amx at twin:~/.local/bin/amx")
    });
    assert_eq!(term.wait(), Some(0));

    assert_eq!(
        fs::read(&landed).expect("the streamed binary"),
        fs::read(&exe).expect("the binary under test"),
        "what arrived on the far side is not this binary"
    );
}

#[test]
fn declining_the_offer_installs_nothing_and_is_not_a_failure() {
    let rig = Rig::new("nose");
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_amx"));
    let fake = fake_ssh(&rig);
    let landed = rig.root().join("streamed-amx");

    let mut command = rig.command(&exe);
    command
        .env(
            "PATH",
            format!(
                "{}:{}",
                fake.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("AMX_FAKE_OS", local_uname("-s"))
        .env("AMX_FAKE_ARCH", local_uname("-m"))
        .env("AMX_FAKE_INSTALL", &landed);
    let mut term = support::env::spawn_on_tty(&mut command, &["--remote", "twin"], ROWS, COLS);
    term.wait_output("the offer", |seen| {
        String::from_utf8_lossy(seen).contains("Install it there?")
    });
    term.send(b"n\n");
    term.wait_output("the decline", |seen| {
        String::from_utf8_lossy(seen).contains("Nothing was installed.")
    });
    assert_eq!(
        term.wait(),
        Some(0),
        "declining to install is a decision, not an error"
    );
    assert!(!landed.exists(), "a declined offer installed anyway");
}

// ------------------------------------------------------------ the --remote surface

#[test]
fn remote_takes_a_host_and_refuses_a_second_one() {
    let (remote, rest) = amx::remote::split(["amx", "--remote", "box", "--session", "work"])
        .expect("one host parses");
    assert_eq!(remote.expect("a host").host(), "box");
    assert_eq!(rest, ["amx", "--session", "work"]);

    let (remote, rest) = amx::remote::split(["amx", "--remote=box"]).expect("the = spelling too");
    assert_eq!(remote.expect("a host").host(), "box");
    assert_eq!(rest, ["amx"]);

    let (remote, rest) = amx::remote::split(["amx", "attach"]).expect("no host is the local path");
    assert!(remote.is_none());
    assert_eq!(rest, ["amx", "attach"]);

    let err = amx::remote::split(["amx", "--remote", "a", "--remote", "b"])
        .expect_err("two hosts have no correct reading");
    assert!(format!("{err:#}").contains("twice"), "{err:#}");

    let err = amx::remote::split(["amx", "--remote"]).expect_err("a flag with no host");
    assert!(format!("{err:#}").contains("wants a host"), "{err:#}");
}

#[test]
fn remote_is_documented_in_help_even_though_clap_never_matches_it() {
    let rig = Rig::new("rhlp");
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_amx"));

    let done = rig.run(&exe, &["--help"]);
    let out = done.ok();
    assert!(
        out.contains("--remote <HOST>"),
        "a flag missing from --help is a flag nobody finds: {done:?}",
    );

    // And it is still stripped before clap parses, which is what makes the
    // declaration documentary: a `--remote` that reached the tree would make
    // `amx --remote box attach` an attach on *this* machine.
    let (remote, rest) =
        amx::remote::split(["amx", "--remote", "box", "attach"]).expect("one host parses");
    assert_eq!(remote.expect("a host").host(), "box");
    assert_eq!(rest, ["amx", "attach"]);
}

#[test]
fn a_remote_verb_says_which_machine_it_would_have_run_on() {
    let rig = Rig::new("rvrb");
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_amx"));

    let done = rig.run(&exe, &["--remote", "box", "session", "list"]);
    let err = done.failed();
    assert!(
        err.contains("runs on this machine"),
        "a verb that silently ran on the wrong host would be the worst answer: {done:?}"
    );
    assert!(err.contains("box"), "{done:?}");
}

// ---------------------------------------------- login shells on the far side

/// Every shell a remote user might have in their passwd entry that this machine
/// happens to have. `/bin/sh` is always here; the rest are a bonus.
///
/// `command -v` rather than a table of paths: these arrive from a distribution,
/// from homebrew and from `/usr/local` on different machines, and a table
/// written here would be a table written from memory.
fn login_shells() -> Vec<PathBuf> {
    ["sh", "bash", "zsh", "fish", "csh", "tcsh"]
        .iter()
        .filter_map(|name| {
            let found = Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("command -v {name}"))
                .output()
                .ok()?;
            let path = PathBuf::from(String::from_utf8_lossy(&found.stdout).trim());
            (found.status.success() && path.is_file()).then_some(path)
        })
        .collect()
}

#[test]
fn a_session_name_crosses_every_login_shell_intact() {
    // The two levels the real bridge command has, and the reason this is worth
    // a test of its own: `sq` quotes the session name *into* the script, and
    // `via_sh` quotes the whole script again *for the login shell*. Each level
    // on its own is easy; the nesting is where the shells disagree. fish is the
    // one that gives `\` a meaning inside single quotes, so the `'\''` a name's
    // own quote produces became an escape at the outer level and reached `sh`
    // as a word with unbalanced quotes — measured, then fixed by escaping the
    // backslash too.
    // `!mine` and not just `mine!`: csh expands `!` only when a word follows
    // it, so a bang at the end of a word would let the history rule through
    // unnoticed.
    let name = "it's !mine! $HOME `x` \"q\" back\\slash";
    let script = format!("printf %s {}", amx::remote::ssh::sq(name));
    let command = amx::remote::ssh::via_sh(&script);

    let shells = login_shells();
    assert!(
        shells.len() > 1,
        "only {shells:?} here, so this would prove nothing about a login shell \
         that is not `sh`; install fish"
    );
    for shell in &shells {
        let out = Command::new(shell)
            .arg("-c")
            .arg(&command)
            .stdin(Stdio::null())
            .output()
            .unwrap_or_else(|err| panic!("run {}: {err}", shell.display()));
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            name,
            "{} did not hand `sh` the name it was given (stderr: {})",
            shell.display(),
            String::from_utf8_lossy(&out.stderr).trim(),
        );
    }
}
