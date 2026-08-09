//! The ssh child: a socketpair for stdio, and the two probes around it.
//!
//! Every command built here is argv-as-data on the local side and a
//! *single-quoted* string on the remote one, for the reason ssh(1) states: the
//! arguments are joined with spaces and handed to the remote login shell, so
//! quoting is amx's job and not ssh's. [`sq`] is the whole of that job.
//!
//! And because it is the *login* shell that reads that string, nothing amx
//! sends is shell syntax: every command leaves here through [`via_sh`], which
//! is where the reasoning lives.
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

/// What the far side is asked about itself, for [`Remote::platform`].
///
/// Two `uname`s rather than `uname -sm`, so the split is on newlines and never
/// on a space inside a field — the *output* is two lines, the script is one, as
/// [`via_sh`] requires.
const UNAME_SCRIPT: &str = "uname -s; uname -m";

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
            .arg(via_sh(&bridge_script(session)))
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
            .arg(via_sh(UNAME_SCRIPT))
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
    /// `script` is a `sh` script and is wrapped as one ([`via_sh`]); the stdin
    /// ssh gives the login shell is inherited straight through to it.
    ///
    /// # Errors
    ///
    /// If ssh cannot be started, or the script exits non-zero.
    pub async fn feed(&self, script: &str, stdin: Stdio) -> anyhow::Result<()> {
        let out = self
            .command()
            .arg(via_sh(script))
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
        "if command -v amx >/dev/null 2>&1; \
         then exec amx _bridge --daemonize --session {name}; \
         elif [ -x \"$HOME/.local/bin/amx\" ]; \
         then exec \"$HOME/.local/bin/amx\" _bridge --daemonize --session {name}; \
         else echo {marker} >&2; exit {NOT_FOUND}; fi",
        name = sq(session.as_str()),
        marker = sq(NO_REMOTE_AMX),
    )
}

/// `text`, quoted so a shell reads it as one literal word.
///
/// Single quotes suppress every expansion a shell has, and the only character
/// they cannot contain is a single quote — which is closed, escaped and
/// reopened, the one construction that works in every `sh`.
///
/// `!` goes out the same way, and that one is csh's. csh expands history
/// **inside single quotes and in `-c` scripts alike**, so a session name
/// holding a `!` reaches a csh login shell as `mine: Event not found.` rather
/// than as a name. A backslash suppresses it there and means "the literal
/// character" in `sh`, `bash`, `zsh` and fish, so escaping costs nothing
/// anywhere else — measured against tcsh 6.24 and fish 4.8, not assumed.
///
/// And a backslash goes out the same way too, which is fish's. **fish is the
/// one shell that gives `\` a meaning inside single quotes**: `\'` and `\\` are
/// escapes there, where every POSIX shell reads a backslash between quotes as
/// itself. That matters because [`via_sh`] quotes a script that is *already*
/// quoted — a session name's `'\''` becomes a backslash inside the outer
/// quotes, and fish read it as an escape and handed `/bin/sh` a word with
/// unbalanced quotes in it. Escaping the backslash makes the nesting mean the
/// same thing in all of them.
///
/// The one character with no answer here is a newline, and only against csh:
/// csh has no spelling for a literal newline inside a word, quoted or not
/// (`Unmatched '''.`, tcsh 6.24). **That case is refused at the boundary rather
/// than encoded, and the refusal is [`SessionName::new`]'s** — an ASCII control
/// character is not a legal session name, so no caller can hand one to this
/// function. Two things settled that, and the second is the load-bearing one:
///
/// - There is no encoding both `sh` and csh read as the same word. Building the
///   byte on the far side (`"$(printf …)"`) moves the problem into the *inner*
///   script, where it is `sh`'s to expand — and command substitution eats
///   trailing newlines, so a name ending in one would still arrive wrong.
/// - Any encoding needs the far side to decode it, and **the far side's amx is
///   a different binary of an unknown version**. An older one would take the
///   encoded form literally and serve a *different session* under a name nobody
///   asked for. A refusal here is a sentence the user can act on; that is a
///   silent wrong answer.
///
/// So what crosses is what a session name may hold, and everything it may hold
/// is quoted above. amx's own scripts hold no newline either, by construction —
/// [`assert_one_simple_command`] asserts it.
#[must_use]
pub fn sq(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for c in text.chars() {
        match c {
            '\'' => out.push_str(r"'\''"),
            '\\' => out.push_str(r"'\\'"),
            '!' => out.push_str(r"'\!'"),
            _ => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// The interpreter every remote script is handed to.
///
/// Absolute, not `sh`: a `PATH` lookup is one more thing the far side's login
/// shell gets to have an opinion about, and `/bin/sh` exists on every system
/// amx targets.
const SH: &str = "/bin/sh";

/// `script`, wrapped so the remote **login shell** reads one simple command.
///
/// ssh hands the command string to the login shell of the remote *user*, and a
/// login shell is not required to be POSIX — fish, csh and tcsh are ordinary
/// choices, and none of them can parse `if … then … fi`, `dir="$HOME/bin"` or
/// `trap … EXIT`. A script sent bare is therefore not a script the far side
/// runs; it is a syntax error, and one amx used to read back as *"this host has
/// no amx"* — a wrong answer about the user's own machine, given confidently.
/// Measured against a host whose login shell is `/usr/bin/fish` and which had a
/// working amx on `PATH` the whole time.
///
/// So nothing amx sends is shell syntax. What the login shell sees is three
/// words — [`SH`], `-c`, and one [`sq`]-quoted blob — which is a simple command
/// in every shell there is, keyword-free and operator-free. The layering is the
/// part that is easy to get backwards: the *outer* string is the login shell's
/// to split into words, the *inner* one is [`SH`]'s to parse, and `$HOME`, `$$`
/// and every keyword belong to the inner one. Single quotes are what keep them
/// apart.
///
/// **`script` must be one line**, which is why every script in this crate is
/// written with `;` where a shell script would have a newline. csh and tcsh
/// reject a newline inside single quotes outright — `Unmatched '''.`, measured
/// against tcsh 6.24 — so a wrapping that fixed fish by putting a multi-line
/// payload in quotes would have broken csh in the same commit. `sh` reads `;`
/// and a newline identically, so the whole cost is the semicolons.
#[must_use]
pub fn via_sh(script: &str) -> String {
    format!("{SH} -c {}", sq(script))
}

/// The characters of `command` a shell reads as syntax: outside single quotes,
/// and not made literal by a backslash.
///
/// Everything else is one word being handed on untouched. Used by the tests
/// either side of this module to state the guarantee [`via_sh`] exists for.
#[cfg(test)]
pub(crate) fn syntax_of(command: &str) -> String {
    let mut chars = command.chars();
    let mut quoted = false;
    let mut syntax = String::new();
    while let Some(c) = chars.next() {
        match c {
            '\'' => quoted = !quoted,
            // Inside single quotes a backslash is an ordinary character; only
            // outside them does it make the next one literal.
            '\\' if !quoted => {
                chars.next();
            }
            _ if !quoted => syntax.push(c),
            _ => {}
        }
    }
    assert!(!quoted, "unbalanced single quotes in {command:?}");
    syntax
}

/// Assert `command` is one simple command, whatever shell is asked to run it.
///
/// Two halves, and both were paid for by a real shell. The first is exhaustive
/// rather than a keyword blacklist: if the only characters the reading shell
/// interprets are `/bin/sh -c`, then there is no keyword, no operator and no
/// expansion left for it to get wrong — that is fish's failure. The second is
/// that the whole thing is one line, quoted part included — that is csh's.
#[cfg(test)]
pub(crate) fn assert_one_simple_command(command: &str) {
    let syntax = syntax_of(command);
    assert_eq!(
        syntax.split_whitespace().collect::<Vec<_>>(),
        vec![SH, "-c"],
        "the login shell would have to interpret {syntax:?} in {command:?}",
    );
    // Named as well as excluded, because these are the words fish choked on.
    for keyword in ["if", "then", "elif", "else", "fi", ";", "$"] {
        assert!(
            !syntax.contains(keyword),
            "{keyword:?} reaches the login shell in {command:?}",
        );
    }
    assert!(
        !command.contains('\n'),
        "csh cannot hold a newline in a word, quoted or not: {command:?}",
    );
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
        // csh expands history inside single quotes, so `!` leaves the same way
        // a quote does — and reads as itself in every other shell.
        assert_eq!(sq("bang!"), r"'bang'\!''");
        assert_eq!(sq("!"), r"''\!''");
        // fish gives `\` a meaning inside single quotes and nothing else does,
        // so it leaves too — this is what makes `via_sh`'s nesting portable.
        assert_eq!(sq(r"a\b"), r"'a'\\'b'");
    }

    #[test]
    fn a_name_no_login_shell_could_carry_is_refused_before_it_reaches_the_quoting() {
        // The decision `sq` records: the newline case is closed at the
        // boundary, not encoded here. Every one of these is a name a POSIX
        // filesystem would take and csh could not — and none of them can be
        // built, so `bridge_script` cannot hold one.
        for hostile in [
            "two\nlines",
            "carriage\rreturn",
            "esc\u{1b}[2J",
            "nul\0byte",
        ] {
            let refused = SessionName::new(hostile);
            assert!(
                refused.is_err(),
                "{hostile:?} became a session name, and this module has no way \
                 to send it: {refused:?}",
            );
        }
        // And the refusal says which character it was about, without printing
        // the character into the terminal reading the message.
        let said = SessionName::new("two\nlines")
            .expect_err("refused")
            .to_string();
        assert!(said.contains("control character"), "{said}");
        assert!(!said.contains('\n'), "{said:?}");
    }

    #[test]
    fn the_bridge_script_quotes_the_session_and_names_both_places_it_looks() {
        let script = bridge_script(&SessionName::new("it's mine").expect("a legal name"));
        assert!(script.contains(r"--session 'it'\''s mine'"), "{script}");
        assert!(script.contains("command -v amx"), "{script}");
        assert!(script.contains("$HOME/.local/bin/amx"), "{script}");
        assert!(script.contains(&format!("exit {NOT_FOUND}")), "{script}");
    }

    #[test]
    fn the_remote_command_is_one_simple_command_so_a_non_posix_login_shell_can_run_it() {
        // Both commands this module sends, built exactly as `open` and
        // `platform` build them — a session name full of metacharacters
        // included, because it is the one part a caller supplies.
        let session = SessionName::new("it's mine!").expect("a legal name");
        assert_one_simple_command(&via_sh(&bridge_script(&session)));
        assert_one_simple_command(&via_sh(UNAME_SCRIPT));
    }

    /// The one word `sq` encoded, read back the way a shell reads it.
    fn unquote(word: &str) -> String {
        let mut chars = word.chars();
        let mut quoted = false;
        let mut text = String::new();
        while let Some(c) = chars.next() {
            match c {
                '\'' => quoted = !quoted,
                '\\' if !quoted => text.extend(chars.next()),
                _ => text.push(c),
            }
        }
        assert!(!quoted, "unbalanced single quotes in {word:?}");
        text
    }

    #[test]
    fn the_script_survives_the_wrapping_whole() {
        // The point of the quoting is that `sh` still reads what was written:
        // the keywords, the expansions and the marker all cross intact, they
        // simply cross as data rather than as syntax. The session name is
        // hostile on purpose — a quote, a bang and the metacharacters a path
        // component is allowed to hold.
        let name = "it's mine! $HOME `x` \"q\"";
        let script = bridge_script(&SessionName::new(name).expect("a legal name"));
        let command = via_sh(&script);
        assert_eq!(unquote(&command["/bin/sh -c ".len()..]), script);
        // And one level in, what `sh` then reads is the name itself.
        assert!(
            script.contains(&format!("--session {}", sq(name))),
            "{script}"
        );
        assert_eq!(unquote(&sq(name)), name);
    }

    #[test]
    fn syntax_of_reads_an_escaped_quote_as_a_character_and_not_as_a_delimiter() {
        // The `'\''` construction closes, escapes and reopens, so the text
        // after it is still quoted. A reader that merely toggled on every `'`
        // would call the tail of the word unquoted, and the guarantee above
        // would be vacuous for exactly the names that need it.
        assert_eq!(syntax_of(r"abc'de'fg"), "abcfg");
        assert_eq!(syntax_of(r"a 'b'\''c' d"), "a  d");
        assert_eq!(syntax_of(&sq("if x; then y; fi")), "");
    }
}
