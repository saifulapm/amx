//! Tier 2 of D-M3-9: the bridge over a **real ssh connection**.
//!
//! `tests/skew.rs` and `crates/amx/tests/bridge.rs` cover every byte of amx
//! code in the remote path, on every platform, by putting a socketpair where
//! ssh would be. What they cannot cover is ssh itself — that `-T` really keeps
//! a pty out of the stream, that the remote login shell really runs the bridge
//! script as written, that framed binary really survives the channel. This
//! suite does, against a loopback sshd on 127.0.0.1 with a throwaway key.
//!
//! # More than one login shell, because "the remote login shell" is not one
//!
//! The loopback sshd runs as the developer, whose login shell is POSIX, so on
//! its own this suite would only ever prove the remote command runs under `sh`.
//! It did — and `--remote` was still unusable against every host whose login
//! shell is fish, csh or tcsh, because ssh hands the command to *that* shell
//! and amx used to send it POSIX syntax. So the second case forces each
//! non-POSIX shell the machine has onto the far side in turn and asserts the
//! attach works anyway; see [`Sshd::force_command`] for how, and
//! [`refuses_posix`] for why the list is measured rather than trusted.
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

/// Login shells that are in ordinary daily use and cannot parse `sh`.
///
/// Not an exhaustive list of non-POSIX shells and not meant to be: one of them
/// qualifying is enough to run the case, and none of them qualifying is a
/// stated skip rather than a silent pass.
const NON_POSIX_SHELLS: &[&str] = &["fish", "csh", "tcsh"];

/// A script shaped exactly like the one amx sends — a POSIX `if`, and the work
/// on the line after `then` — printing a marker instead of doing the work.
const POSIX_PROBE: &str = "if true; then\nprintf %s ran-it\nfi\n";

/// What a shell that parsed [`POSIX_PROBE`] prints.
const PROBE_MARKER: &str = "ran-it";

/// The same work in the shape amx now sends: one line, quoted, for `/bin/sh` to
/// parse rather than the login shell.
///
/// One line matters as much as the quoting. csh answers `Unmatched '''.` to a
/// newline inside single quotes, so a wrapping that fixed fish with a
/// multi-line payload would have broken csh in the same commit.
const WRAPPED_PROBE: &str = "/bin/sh -c 'if true; then printf %s ran-it; fi'";

/// Every one of [`NON_POSIX_SHELLS`] this machine has, wherever it keeps it.
///
/// `command -v` rather than a table of paths, because these arrive from a
/// distribution, from homebrew and from `/usr/local` on different machines and
/// a table written here would be a table written from memory.
fn non_posix_shells() -> Vec<PathBuf> {
    NON_POSIX_SHELLS
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

/// Whether `shell` refuses [`POSIX_PROBE`] outright — the failure amx was
/// reported with, rather than merely a shell that is not `sh`.
///
/// **Not every non-POSIX shell can catch this bug, which is why there is a
/// probe here and not just a `command -v`.** csh and tcsh read a script line by
/// line: handed amx's old bridge script they misparse
/// `if command -v amx …; then`, print a complaint — and then run the `exec` on
/// the next line anyway. The wrong thing happens *and* the right thing happens,
/// so a case staged only on tcsh would attach happily with the bug in place.
/// fish parses the whole script before running any of it, so a syntax error
/// means nothing runs at all — the reported failure exactly. This tells the two
/// apart by measurement instead of a hard-coded preference that would rot when
/// a fourth shell shows up.
fn refuses_posix(shell: &Path) -> bool {
    Command::new(shell)
        .arg("-c")
        .arg(POSIX_PROBE)
        .output()
        .is_ok_and(|out| !String::from_utf8_lossy(&out.stdout).contains(PROBE_MARKER))
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
    ///
    /// `login_shell` stages the far side's *login shell* for the one case the
    /// developer's own passwd entry cannot supply: see [`Self::force_command`].
    fn start(env: &Env, path: &str, login_shell: Option<&Path>) -> Self {
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
             XDG_STATE_HOME={home}/state XDG_CONFIG_HOME={home}/config\n\
             {forced}",
            dir = dir.display(),
            home = env.home().display(),
            forced = login_shell.map(Self::force_command).unwrap_or_default(),
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

    /// The directive that makes `shell` read amx's command, as a login shell
    /// would.
    ///
    /// sshd assigns `SHELL` from the passwd entry after `SetEnv` is applied
    /// (see above), so the far side's login shell cannot be set by a variable
    /// and the developer's own is whatever it is. `ForceCommand` is the seam
    /// that is left: sshd runs it *instead of* the requested command, with the
    /// requested one in `$SSH_ORIGINAL_COMMAND` — so `shell -c "<command>"` is
    /// character for character what sshd itself would have run had `shell` been
    /// in the passwd entry. Nothing about amx's string is touched on the way.
    ///
    /// One line, and it is the only `ForceCommand` in the file: sshd takes the
    /// first value it obtains for a keyword and drops the rest, which is the
    /// same rule the single `SetEnv` line above exists for.
    fn force_command(shell: &Path) -> String {
        format!(
            "ForceCommand exec {shell} -c \"$SSH_ORIGINAL_COMMAND\"\n",
            shell = shell.display(),
        )
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
    let sshd = Sshd::start(&env, &bin_dir.display().to_string(), None);
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

#[test]
fn loopback_ssh_attaches_through_a_login_shell_that_cannot_parse_sh() {
    const NAME: &str = "loopback_ssh_attaches_through_a_login_shell_that_cannot_parse_sh";
    if let Some(reason) = skipped() {
        eprintln!("{NAME}: skipped — {reason}");
        return;
    }
    let shells = non_posix_shells();
    if shells.is_empty() {
        eprintln!(
            "{NAME}: skipped — none of {NON_POSIX_SHELLS:?} is installed here, and \
             this case needs a real login shell that is not `sh` to be worth \
             anything. Install fish to run it."
        );
        return;
    }
    if !shells.iter().any(|shell| refuses_posix(shell)) {
        eprintln!(
            "{NAME}: skipped — {shells:?} are installed but every one of them \
             muddles through a POSIX script instead of refusing it, so this case \
             would pass with the bug in place (see `refuses_posix`). Install fish \
             to run it."
        );
        return;
    }

    // Every one of them, not only the one that reproduces the bug. csh and
    // tcsh cannot catch the original failure, but they are exactly what catches
    // a *fix* that trades one broken shell for another — a quoted payload with
    // a newline in it is `Unmatched '''.` to csh and fine to fish.
    for shell in &shells {
        attaches_through(shell);
    }
}

/// Attach over the loopback sshd with `shell` reading amx's remote command.
fn attaches_through(shell: &Path) {
    // The staging is only meaningful if the shell can run what amx sends
    // *after* the fix — otherwise a green result would mean the far side never
    // ran anything either way.
    let wrapped = Command::new(shell)
        .arg("-c")
        .arg(WRAPPED_PROBE)
        .output()
        .expect("run the wrapped probe");
    assert!(
        String::from_utf8_lossy(&wrapped.stdout).contains(PROBE_MARKER),
        "{} could not run `/bin/sh -c '<script>'` either: {wrapped:?}",
        shell.display(),
    );

    let mut env = Env::new("sshf");
    fs::write(env.config_path(), "[terminal]\nshell = \"/bin/sh\"\n")
        .expect("write the remote config");
    let exe = env.exe();
    let bin_dir = exe.parent().expect("the binary's directory").to_path_buf();
    // The whole of the difference from the case above: the far side reads
    // amx's command with a shell that has no `if`, no `fi` and no `VAR=value`.
    let sshd = Sshd::start(&env, &bin_dir.display().to_string(), Some(shell));
    env.set_var(
        "PATH",
        &format!(
            "{}:{}",
            sshd.wrapper_dir().display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    );

    // The two ways the far side refuses: amx's own "no amx here" sentence, and
    // any error the binary printed. Both end the wait, so a failure is the
    // assertion below and its account of what the shell said — not a timeout
    // waiting for a prompt that was never coming.
    let refused = |seen: &[u8]| shows(seen, "has no amx") || shows(seen, "amx: ");
    let mut term = env.attach_on_tty(&["--remote", ALIAS], ROWS, COLS);
    term.wait_output_or(
        "a remote pane to render its prompt, or amx to give up on the far side",
        |seen| shows(seen, "$") || refused(seen),
        || sshd.log(),
    );
    assert!(
        !refused(term.output()),
        "{} could not run amx's remote command, so `--remote` is unusable against \
         every host whose login shell is that one — and a host with a working amx \
         on it gets told it has none. One simple command, on one line. The screen \
         held:\n{}\nsshd said:\n{}",
        shell.display(),
        rig::screen::render(&rig::rasterize(term.output())),
        sshd.log(),
    );

    // And it is a real session, not merely a pane that painted: the far side
    // parsed the whole script, found amx, and the server it started answers.
    term.type_line("printf 'ok-%s\\n' past-the-login-shell");
    term.wait_output("the remote child's output to render", |seen| {
        shows(seen, "ok-past-the-login-shell")
    });
    assert!(
        amx_server::session::probe::probe(&env.socket()).is_ok_and(|p| p.is_running()),
        "no server is answering {}; sshd said:\n{}",
        env.socket().display(),
        sshd.log(),
    );

    term.chord(b'd');
    assert_eq!(term.wait(), Some(0), "sshd said:\n{}", sshd.log());
}
