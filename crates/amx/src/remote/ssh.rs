//! The ssh child: a socketpair for stdio, and the two probes around it.
//!
//! Every command built here is argv-as-data on the local side and a
//! *single-quoted* string on the remote one, for the reason ssh(1) states: the
//! arguments are joined with spaces and handed to the remote login shell, so
//! quoting is amx's job and not ssh's. [`sq`] is the whole of that job.
//!
//! Two flags are load-bearing on every invocation and neither is decoration.
//! `-T` disables pseudo-terminal allocation: the bridge carries framed binary,
//! and a pty in the path would translate it. `-e none` disables the escape
//! character, so a `~` at the start of a line inside the stream is data rather
//! than a request to close the connection.

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::process::Stdio;

use amx_core::SessionName;
use anyhow::Context as _;
use tokio::io::AsyncReadExt as _;
use tokio::net::UnixStream;
use tokio::process::{Child, Command};

/// What the remote command prints when it can find no amx to exec.
///
/// Written by amx's own shell fragment rather than left to the login shell's
/// wording, so "the far side has no amx" is detected by a string this crate
/// chose and not by pattern-matching whatever `sh`, `bash` or `zsh` happens to
/// say about a command it could not find.
pub const NO_REMOTE_AMX: &str = "amx-bridge: no amx on this host";

/// The exit status a shell gives a command it could not run, and the one the
/// fragment above exits with deliberately.
const NOT_FOUND: i32 = 127;

/// How much of ssh's stderr is kept for a diagnostic.
///
/// A bridge that runs for hours writes nothing here, so this bound is only ever
/// reached by something shouting — and a shouting peer must not be able to grow
/// this process without limit.
const MAX_STDERR: u64 = 8 * 1024;

/// A host `--remote` names, and the ssh invocations that reach it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Remote {
    host: String,
}

/// What `uname -s` and `uname -m` say about a machine.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Platform {
    /// `uname -s`, verbatim: `Linux`, `Darwin`.
    pub os: String,
    /// `uname -m`, verbatim: `x86_64`, `arm64`, `aarch64`.
    pub arch: String,
}

impl Platform {
    /// This machine's, read from `uname` rather than from `std::env::consts`.
    ///
    /// The comparison seeding makes is against the remote's `uname` output, and
    /// mapping Rust's `linux`/`aarch64` onto `Linux`/`arm64` would be a table
    /// written from memory. Running the same program on both ends is the only
    /// version of this check that cannot be wrong about a platform nobody
    /// tested it on.
    ///
    /// # Errors
    ///
    /// If `uname` cannot be run or says nothing.
    pub fn local() -> anyhow::Result<Self> {
        Ok(Self {
            os: uname("-s")?,
            arch: uname("-m")?,
        })
    }
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.os, self.arch)
    }
}

/// Run `uname <flag>` on this machine and return its single line.
fn uname(flag: &str) -> anyhow::Result<String> {
    let out = std::process::Command::new("uname")
        .arg(flag)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("run `uname {flag}`"))?;
    anyhow::ensure!(
        out.status.success(),
        "`uname {flag}` failed: {}",
        String::from_utf8_lossy(&out.stderr).trim(),
    );
    let line = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    anyhow::ensure!(!line.is_empty(), "`uname {flag}` printed nothing");
    Ok(line)
}

/// The far side answered, and what it answered was that it has no amx.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Missing {
    /// Everything ssh and the remote shell wrote to stderr, for the message
    /// the caller prints when it cannot offer a remedy.
    pub said: String,
}

impl Remote {
    /// A host as `--remote` spelled it — `host`, `user@host`, or an ssh alias.
    #[must_use]
    pub fn new(host: String) -> Self {
        Self { host }
    }

    /// The destination, for messages and for the ssh command line.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Start `amx _bridge` on the far side and hand back this end of its stdio.
    ///
    /// The socketpair is the trick the whole design rests on: the child gets
    /// one end as both stdin and stdout, and the caller gets the other as an
    /// ordinary `UnixStream` to attach a client to.
    ///
    /// # Errors
    ///
    /// If the socketpair cannot be made, or ssh cannot be started.
    pub fn open(&self, session: &SessionName) -> anyhow::Result<(Bridge, UnixStream)> {
        let (mine, theirs) = StdUnixStream::pair().context("make the bridge socketpair")?;
        let stdin = OwnedFd::from(theirs.try_clone().context("dup the bridge socket")?);
        let stdout = OwnedFd::from(theirs);

        let child = self
            .command()
            .arg(bridge_script(session))
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::piped())
            .spawn()
            .context("start ssh")?;

        mine.set_nonblocking(true)
            .context("set the bridge socket non-blocking")?;
        let local = UnixStream::from_std(mine).context("adopt the bridge socket")?;
        Ok((Bridge { child }, local))
    }

    /// Ask the far side what platform it is.
    ///
    /// # Errors
    ///
    /// If ssh fails, or `uname` says something with fewer than two lines in it.
    pub async fn platform(&self) -> anyhow::Result<Platform> {
        let out = self
            .command()
            // Two lines rather than `uname -sm`, so the split is on newlines
            // and never on a space inside a field.
            .arg("uname -s\nuname -m\n")
            .stdin(Stdio::null())
            .output()
            .await
            .context("run uname over ssh")?;
        anyhow::ensure!(
            out.status.success(),
            "could not read {}'s platform: {}",
            self.host,
            String::from_utf8_lossy(&out.stderr).trim(),
        );
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        let os = lines.next().unwrap_or_default().to_owned();
        let arch = lines.next().unwrap_or_default().to_owned();
        anyhow::ensure!(
            !os.is_empty() && !arch.is_empty(),
            "{} answered `uname` with {text:?}",
            self.host,
        );
        Ok(Platform { os, arch })
    }

    /// Run `script` on the far side with `stdin` as its input.
    ///
    /// The one write path: [`crate::remote::seed`] streams a binary through it.
    ///
    /// # Errors
    ///
    /// If ssh cannot be started, or the script exits non-zero.
    pub async fn feed(&self, script: &str, stdin: Stdio) -> anyhow::Result<()> {
        let out = self
            .command()
            .arg(script)
            .stdin(stdin)
            .output()
            .await
            .context("run the install script over ssh")?;
        anyhow::ensure!(
            out.status.success(),
            "the install on {} failed ({}): {}",
            self.host,
            out.status,
            String::from_utf8_lossy(&out.stderr).trim(),
        );
        Ok(())
    }

    /// `ssh -T -e none <host>`, with the command still to be added.
    fn command(&self) -> Command {
        let mut command = Command::new("ssh");
        command.arg("-T").arg("-e").arg("none").arg(&self.host);
        command
    }
}

/// A running ssh child holding one end of the bridge.
#[derive(Debug)]
pub struct Bridge {
    child: Child,
}

impl Bridge {
    /// The handshake never happened: find out from the child why, and say so.
    ///
    /// `Ok` means the far side ran, looked, and had no amx to exec — the one
    /// failure with a remedy, which [`crate::remote::seed`] offers. Anything
    /// else comes back as an error carrying ssh's own words rather than amx's
    /// guess at what they would have been.
    ///
    /// # Errors
    ///
    /// If ssh failed for any reason other than a missing remote amx.
    pub async fn diagnose(mut self, cause: &anyhow::Error) -> anyhow::Result<Missing> {
        let mut said = String::new();
        if let Some(stderr) = self.child.stderr.take() {
            let mut raw = Vec::new();
            let _ = stderr.take(MAX_STDERR).read_to_end(&mut raw).await;
            said = String::from_utf8_lossy(&raw).trim().to_owned();
        }
        let status = self.child.wait().await.context("wait for ssh")?;

        anyhow::ensure!(
            status.code() == Some(NOT_FOUND) || said.contains(NO_REMOTE_AMX),
            "the bridge did not answer ({status}): {}",
            if said.is_empty() {
                format!("{cause:#}")
            } else {
                said
            },
        );
        Ok(Missing { said })
    }

    /// Reap the child, for a bridge whose session has ended.
    ///
    /// The far side exits by itself when the splice ends, so this is the
    /// tidy-up for the case where it has not noticed yet — a local client that
    /// detached while the remote server was still writing.
    pub async fn finish(mut self) {
        let _ = self.child.kill().await;
    }
}

/// The command `ssh` runs on the far side.
///
/// Two places are looked in and no more: `PATH`, which is where an installed
/// amx belongs, and `~/.local/bin`, which is where [`crate::remote::seed`] puts
/// one — so a seeded host works on the next attach even when its
/// non-interactive `PATH` does not carry that directory, which on most systems
/// it does not. Anything else exits [`NOT_FOUND`] with [`NO_REMOTE_AMX`], which
/// is the local side's cue to offer seeding.
fn bridge_script(session: &SessionName) -> String {
    format!(
        "if command -v amx >/dev/null 2>&1; then\n\
         exec amx _bridge --daemonize --session {name}\n\
         elif [ -x \"$HOME/.local/bin/amx\" ]; then\n\
         exec \"$HOME/.local/bin/amx\" _bridge --daemonize --session {name}\n\
         else\n\
         echo {marker} >&2\n\
         exit {NOT_FOUND}\n\
         fi\n",
        name = sq(session.as_str()),
        marker = sq(NO_REMOTE_AMX),
    )
}

/// `text`, quoted so a POSIX shell reads it as one literal word.
///
/// Single quotes suppress every expansion a shell has, and the only character
/// they cannot contain is a single quote — which is closed, escaped and
/// reopened, the one construction that works in every `sh`.
#[must_use]
pub fn sq(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_quoting_survives_every_character_a_session_name_may_hold() {
        assert_eq!(sq("work"), "'work'");
        assert_eq!(sq("two words"), "'two words'");
        // A session name is validated as a path component, so all of these are
        // legal names and all of them are shell metacharacters.
        assert_eq!(sq("$(rm -rf ~)"), "'$(rm -rf ~)'");
        assert_eq!(sq("a;b"), "'a;b'");
        assert_eq!(sq("it's"), r"'it'\''s'");
    }

    #[test]
    fn the_bridge_script_quotes_the_session_and_names_both_places_it_looks() {
        let script = bridge_script(&SessionName::new("it's mine").expect("a legal name"));
        assert!(script.contains(r"--session 'it'\''s mine'"), "{script}");
        assert!(script.contains("command -v amx"), "{script}");
        assert!(script.contains("$HOME/.local/bin/amx"), "{script}");
        assert!(script.contains(&format!("exit {NOT_FOUND}")), "{script}");
    }
}
