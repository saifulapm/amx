//! Tier 2 of D-M3-9: the bridge over a **real ssh connection**.
//!
//! `tests/skew.rs` and `crates/amx/tests/bridge.rs` cover every byte of amx
//! code in the remote path, on every platform, by putting a socketpair where
//! ssh would be. What they cannot cover is ssh itself — that `-T` really keeps
//! a pty out of the stream, that the remote login shell really runs the bridge
//! script as written, that framed binary really survives the channel. This
//! suite does, against a loopback sshd on 127.0.0.1 with a throwaway key.
//!
//! # Where it runs, and why not everywhere
//!
//! Linux only, and only when `AMX_TEST_SSHD` is set (R-M3-6). darwin runners
//! cannot host a loopback sshd — the daemon there is under `launchd` and the
//! Remote Login service, not a binary a test can start unprivileged into a
//! temp directory — so the ssh-transport tier is Linux-gated by design and the
//! skip below says so rather than pretending the coverage exists. A real second
//! machine is not a CI resource at all; it is the live-smoke step.
//!
//! # How the far side is isolated
//!
//! sshd's own `SetEnv` carries the test's roots into the session it starts, so
//! the "remote" amx writes into the same temp directories the local harness
//! owns and touches nothing of the developer's. `PATH` goes the same way,
//! pointing at the binary under test — which is also, not incidentally, the
//! thing a real user's non-interactive ssh `PATH` most often gets wrong, and
//! the reason `crate::remote::ssh`'s bridge script looks in `~/.local/bin` too.
//!
//! # The one wrapper, and what it does not do
//!
//! ssh reads `~/.ssh/config` from the **passwd** entry's home directory, not
//! from `$HOME` — measured on the build machine, not assumed — so a harness
//! that only redirects `$HOME` cannot hand ssh a port, a key and a host alias.
//! What it can do is put a two-line `ssh` on `PATH` that adds `-F <config>` and
//! `exec`s the real binary with **amx's argv untouched**. Everything under test
//! stays real: the connection, the absent pty, the login shell parsing the
//! bridge script, the framed bytes on the channel. The wrapper supplies exactly
//! what a user's own `~/.ssh/config` would, and nothing else.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use rig::{ALT_ENTER, Env, shows, wait_until};

/// The variable that admits this suite. Set by `scripts/ci.sh` on Linux when a
/// usable `sshd` is on the machine.
const GATE: &str = "AMX_TEST_SSHD";

/// The host alias the generated `~/.ssh/config` defines.
const ALIAS: &str = "amx-loopback";

const ROWS: u16 = 24;
const COLS: u16 = 80;

/// Why this suite is not running, or `None` if it is.
fn skipped() -> Option<String> {
    if cfg!(target_os = "macos") {
        return Some(
            "darwin cannot host the loopback sshd this needs — its daemon lives \
             under launchd and Remote Login, not in a temp directory a test can \
             start it into. The bridge-as-child tier (tests/skew.rs, \
             crates/amx/tests/bridge.rs) covers every byte of amx code in this \
             path here; only the ssh transport itself is Linux-gated (R-M3-6)."
                .to_owned(),
        );
    }
    if std::env::var_os(GATE).is_none() {
        return Some(format!(
            "${GATE} is unset, so no loopback sshd was asked for. \
             `scripts/ci.sh` sets it on Linux when sshd is present; set it by \
             hand to run this locally (R-M3-6)."
        ));
    }
    if sshd_binary().is_none() {
        return Some(format!(
            "${GATE} is set but no sshd binary was found; install openssh-server \
             or unset the variable."
        ));
    }
    if ssh_binary().is_none() {
        return Some(format!(
            "${GATE} is set but no ssh binary was found; install openssh-client \
             or unset the variable."
        ));
    }
    None
}

/// Where sshd lives, if it is installed.
fn sshd_binary() -> Option<PathBuf> {
    first_file(&["/usr/sbin/sshd", "/usr/bin/sshd", "/sbin/sshd"])
}

/// Where the real ssh lives — needed absolutely, because the wrapper this suite
/// puts on `PATH` would otherwise find itself.
fn ssh_binary() -> Option<PathBuf> {
    first_file(&["/usr/bin/ssh", "/bin/ssh", "/usr/local/bin/ssh"])
}

fn first_file(candidates: &[&str]) -> Option<PathBuf> {
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

/// A loopback sshd, its keys, and the client config that reaches it.
struct Sshd {
    child: Child,
    dir: PathBuf,
    ssh_config: PathBuf,
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Sshd {
    /// Start one, carrying `env`'s roots and `path` into every session it
    /// serves, and write the `~/.ssh/config` entry that reaches it.
    fn start(env: &Env, path: &str) -> Self {
        let dir = env.home().join("sshd");
        fs::create_dir_all(&dir).expect("create the sshd dir");
        keygen(&dir.join("host_key"));
        keygen(&dir.join("id"));
        fs::copy(dir.join("id.pub"), dir.join("authorized_keys")).expect("authorize the key");

        let port = free_port();
        // Every variable the remote amx needs, and no more. Without them sshd
        // hands the session a `PATH` with no amx on it and the developer's own
        // XDG roots, and "the remote session" would be the developer's.
        //
        // **One `SetEnv` line, not five.** sshd takes the first value it
        // obtains for a keyword and ignores later ones, so five directives set
        // one variable and silently drop four — measured on the build machine,
        // where the four that vanished were exactly the roots that keep this
        // test out of the developer's home. `HOME` and `SHELL` cannot be set
        // here at all: sshd assigns both from the passwd entry after `SetEnv`
        // is applied, which is why the pane's shell is pinned in amx's own
        // config file instead.
        let config = format!(
            "Port {port}\n\
             ListenAddress 127.0.0.1\n\
             HostKey {dir}/host_key\n\
             PidFile {dir}/pid\n\
             AuthorizedKeysFile {dir}/authorized_keys\n\
             StrictModes no\n\
             UsePAM no\n\
             PasswordAuthentication no\n\
             KbdInteractiveAuthentication no\n\
             PubkeyAuthentication yes\n\
             SetEnv PATH={path} XDG_RUNTIME_DIR={home}/run \
             XDG_STATE_HOME={home}/state XDG_CONFIG_HOME={home}/config\n",
            dir = dir.display(),
            home = env.home().display(),
        );
        let config_path = dir.join("sshd_config");
        fs::write(&config_path, config).expect("write sshd_config");

        let log = dir.join("sshd.log");
        let child = Command::new(sshd_binary().expect("an sshd binary"))
            .arg("-f")
            .arg(&config_path)
            .arg("-D")
            .arg("-E")
            .arg(&log)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the loopback sshd");

        wait_until("the loopback sshd to listen", || {
            std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
        });

        // The client half: an ssh config naming this daemon, and the wrapper
        // that hands it to ssh. See the module header for why `$HOME` is not
        // enough — ssh reads the passwd entry's home, not the variable.
        let ssh_config = dir.join("ssh_config");
        fs::write(
            &ssh_config,
            format!(
                "Host {ALIAS}\n\
                 \x20   HostName 127.0.0.1\n\
                 \x20   Port {port}\n\
                 \x20   IdentityFile {dir}/id\n\
                 \x20   IdentitiesOnly yes\n\
                 \x20   StrictHostKeyChecking no\n\
                 \x20   UserKnownHostsFile /dev/null\n\
                 \x20   BatchMode yes\n\
                 \x20   LogLevel ERROR\n",
                dir = dir.display(),
            ),
        )
        .expect("write the ssh config");

        Self {
            child,
            dir,
            ssh_config,
        }
    }

    /// A directory holding an `ssh` that adds `-F <config>` and execs the real
    /// one, for the front of the client's `PATH`.
    fn wrapper_dir(&self) -> PathBuf {
        let dir = self.dir.join("bin");
        fs::create_dir_all(&dir).expect("create the wrapper dir");
        let path = dir.join("ssh");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nexec {real} -F {config} \"$@\"\n",
                real = ssh_binary().expect("a real ssh").display(),
                config = self.ssh_config.display(),
            ),
        )
        .expect("write the ssh wrapper");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod the wrapper");
        dir
    }

    /// Everything sshd logged, for a failure that needs it.
    fn log(&self) -> String {
        fs::read_to_string(self.dir.join("sshd.log")).unwrap_or_default()
    }
}

fn keygen(path: &Path) {
    let out = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", ""])
        .arg("-f")
        .arg(path)
        .output()
        .expect("run ssh-keygen");
    assert!(out.status.success(), "ssh-keygen failed: {out:?}");
}

/// A port nothing is listening on, as of a moment ago.
///
/// The kernel picks it, which is the only source that knows; the window between
/// releasing it and sshd binding it is the one race here, and it is narrow
/// enough that losing it shows up as a plain bind failure rather than as
/// anything subtle.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
    listener.local_addr().expect("the bound address").port()
}

#[test]
fn loopback_ssh_attach_renders_a_real_pane() {
    if let Some(reason) = skipped() {
        eprintln!("loopback_ssh_attach_renders_a_real_pane: skipped — {reason}");
        return;
    }

    let mut env = Env::new("sshr");
    // The pane's shell, pinned in the remote server's own config rather than
    // left to sshd's environment: `SHELL` there is the passwd entry's, which is
    // the developer's login shell and a different prompt on every machine.
    // `[terminal] shell` wins over `$SHELL` (D-M1-8), and the config root sshd
    // hands the far side is this harness's.
    fs::write(env.config_path(), "[terminal]\nshell = \"/bin/sh\"\n")
        .expect("write the remote config");
    let exe = env.exe();
    let bin_dir = exe.parent().expect("the binary's directory").to_path_buf();
    let sshd = Sshd::start(&env, &bin_dir.display().to_string());
    env.set_var(
        "PATH",
        &format!(
            "{}:{}",
            sshd.wrapper_dir().display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    );

    // The local client is `amx --remote <alias>` and nothing else: no flag says
    // "this is remote" below this line, because there is no such flag. What
    // makes it remote is which end of a socketpair the client was handed.
    let mut term = env.attach_on_tty(&["--remote", ALIAS], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    term.wait_output("a remote pane to render its prompt", |seen| {
        shows(seen, "$")
    });

    // A pane that renders a prompt is a pane whose child is alive on the far
    // side of the ssh channel. Prove it drives: bytes typed here reach that
    // child, and what the child prints comes back through the same channel.
    term.type_line("printf 'ok-%s\\n' over-ssh");
    term.wait_output("the remote child's output to render", |seen| {
        shows(seen, "ok-over-ssh")
    });

    // And the session is genuinely on the far side of a bridge: the server the
    // remote command started is answering the socket sshd's environment named.
    assert!(
        amx_server::session::probe::probe(&env.socket()).is_ok_and(|p| p.is_running()),
        "no server is answering {}; sshd said:\n{}",
        env.socket().display(),
        sshd.log(),
    );

    // Detach ends the client, not the session: the same chord, the same exit,
    // over ssh.
    term.chord(b'd');
    assert_eq!(term.wait(), Some(0), "sshd said:\n{}", sshd.log());
    assert!(
        amx_server::session::probe::probe(&env.socket()).is_ok_and(|p| p.is_running()),
        "detaching took the remote session with it"
    );
}
