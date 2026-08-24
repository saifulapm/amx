//! The tmux command line, wrapped.
//!
//! amx is never in the byte path: an agent is a real tmux pane, and everything
//! amx does to it is a `tmux(1)` invocation. The laws this module keeps, each
//! one paid for:
//!
//! * **Ids, never names.** `%pane`, `@window`, `$session` are what get stored
//!   and targeted. A window named `build: api` can never be addressed at all —
//!   tmux target syntax splits at the colon — and `new-window -t 0` reads the
//!   target as an index, so a session named `0` collides with itself.
//! * **A value read is not a liveness check.** `display -p -t <gone>` answers
//!   emptily and happily. Liveness is the pane appearing in `list-panes`.
//! * **Pane options are read with `show-options -p`,** never through a
//!   `#{@option}` format: format lookup walks up to the global scope, so one
//!   `set -g` would answer for every pane on the server.
//! * **A capture is sanitized.** Control characters — including the 8-bit CSI
//!   at U+009B, which `capture-pane` passes through verbatim — become spaces,
//!   and so do the invisible format characters. Replaced, never deleted:
//!   deleting a zero-width space is how `ad\u{200b}min` reads as `admin`. The
//!   one exception says so in its own name: `capture_painted` keeps the
//!   escapes because they are what it was called for, and hands the reader the
//!   job of walking them.
//! * **A conf, where one is asked for, rides every call.** tmux reads a config
//!   file when it starts a server and on no later call, and the server is born
//!   by whichever call arrives first. amx asks for none: an agent's session
//!   goes on the person's own server, under the file they wrote for it. It is
//!   the tests that ask, so that nothing in a developer's `~/.tmux.conf` can
//!   change what they measure.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The oldest tmux amx runs against.
pub const MINIMUM_VERSION: (u32, u32) = (3, 2);

/// Where a tmux server listens.
///
/// `-L <name>` for servers amx starts: tmux creates the socket's directory for
/// a named socket and, after a reboot, `-S <path>` will not. `-S <path>` is for
/// the server amx was handed by `$TMUX`, which names its socket by path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Socket {
    Name(String),
    Path(PathBuf),
}

/// A tmux server, addressed the way it was recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    socket: Socket,
    conf: Option<PathBuf>,
}

macro_rules! tmux_id {
    ($name:ident, $sigil:literal, $what:literal) => {
        #[doc = concat!("A tmux ", $what, " id, `", $sigil, "` and all.")]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(String);

        impl $name {
            /// Take an id as tmux printed it, refusing anything that is not
            /// one — a name that slipped in where an id belongs is a bug that
            /// surfaces only when the name turns out to be unaddressable.
            pub fn new(id: impl Into<String>) -> Result<Self> {
                let id = id.into();
                if !id.starts_with($sigil) || id.len() < 2 {
                    bail!(concat!("not a tmux ", $what, " id: {:?}"), id);
                }
                Ok(Self(id))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

tmux_id!(SessionId, '$', "session");
tmux_id!(WindowId, '@', "window");
tmux_id!(PaneId, '%', "pane");

/// What to create, and where.
#[derive(Debug, Default, Clone)]
pub struct Spawn<'a> {
    /// A name for the session or window: convenience for a person reading the
    /// status line, never how amx addresses the thing afterwards.
    pub name: Option<&'a str>,
    /// A name for the window a new session brings with it. Unnamed, tmux calls
    /// it after whatever the pane is running, which changes under it.
    pub window: Option<&'a str>,
    /// The working directory the new pane starts in.
    pub cwd: Option<&'a Path>,
    /// The command the pane runs. Empty means the user's shell.
    pub command: &'a [&'a str],
}

impl Server {
    /// A server addressed by socket name (`-L`).
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            socket: Socket::Name(name.into()),
            conf: None,
        }
    }

    /// A server addressed by socket path (`-S`).
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            socket: Socket::Path(path.into()),
            conf: None,
        }
    }

    /// The server this process is already inside, read from `$TMUX`, whose
    /// value is `<socket path>,<pid>,<session index>`.
    pub fn from_tmux_env(value: &str) -> Option<Self> {
        let path = value.split(',').next().filter(|p| !p.is_empty())?;
        Some(Self::at(path))
    }

    /// The server as it was recorded.
    pub fn from_socket(socket: Socket) -> Self {
        Self { socket, conf: None }
    }

    /// Ride this conf on every call, so whichever one starts the server reads
    /// it rather than `~/.tmux.conf`.
    pub fn with_conf(mut self, conf: impl Into<PathBuf>) -> Self {
        self.conf = Some(conf.into());
        self
    }

    /// How to address this server again, for the record on disk.
    pub fn socket(&self) -> &Socket {
        &self.socket
    }

    /// A `tmux` command line for this server, before its subcommand.
    pub fn command(&self) -> Command {
        let mut cmd = Command::new("tmux");
        if let Some(conf) = &self.conf {
            cmd.arg("-f").arg(conf);
        }
        match &self.socket {
            Socket::Name(name) => cmd.arg("-L").arg(name),
            Socket::Path(path) => cmd.arg("-S").arg(path),
        };
        cmd
    }

    /// Run one tmux command line and answer with its stdout, trailing
    /// whitespace trimmed.
    pub fn run(&self, args: &[&str]) -> Result<String> {
        self.run_with_stdin(args, None)
    }

    /// The same, with bytes on the command's stdin.
    pub fn run_with_stdin(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String> {
        let mut cmd = self.command();
        cmd.args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });

        let mut child = cmd
            .spawn()
            .with_context(|| format!("running `tmux {}`", args.join(" ")))?;
        if let Some(bytes) = stdin {
            let mut pipe = child.stdin.take().expect("stdin was asked for");
            pipe.write_all(bytes)
                .with_context(|| format!("writing to `tmux {}`", args.join(" ")))?;
            // Dropping the pipe is the end-of-input tmux waits for.
        }

        let out = child
            .wait_with_output()
            .with_context(|| format!("waiting for `tmux {}`", args.join(" ")))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            bail!("tmux {}: {}", args.join(" "), stderr.trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    }

    /// Whether a server is listening on this socket at all.
    pub fn is_alive(&self) -> bool {
        self.run(&["list-sessions", "-F", "#{session_id}"]).is_ok()
    }

    /// End the server and everything on it. A server that is already gone is
    /// the outcome asked for, not a failure.
    pub fn kill(&self) -> Result<()> {
        match self.run(&["kill-server"]) {
            Ok(_) => Ok(()),
            Err(e) if is_no_server(&e) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Create a detached session, and answer with it and its first pane.
    pub fn new_session(&self, spawn: &Spawn<'_>) -> Result<(SessionId, PaneId)> {
        let mut args = vec![
            "new-session".to_string(),
            "-d".to_string(),
            "-P".to_string(),
            "-F".to_string(),
            "#{session_id} #{pane_id}".to_string(),
        ];
        if let Some(name) = spawn.name {
            args.push("-s".to_string());
            args.push(name.to_string());
        }
        if let Some(window) = spawn.window {
            args.push("-n".to_string());
            args.push(window.to_string());
        }
        push_spawn(&mut args, spawn);

        let printed = self.run(&borrow(&args))?;
        let (session, pane) = printed
            .split_once(' ')
            .with_context(|| format!("new-session printed {printed:?}"))?;
        Ok((SessionId::new(session)?, PaneId::new(pane)?))
    }

    /// Create a window in `session`, and answer with it and its first pane.
    pub fn new_window(&self, session: &SessionId, spawn: &Spawn<'_>) -> Result<(WindowId, PaneId)> {
        let mut args = vec![
            "new-window".to_string(),
            "-t".to_string(),
            session.as_str().to_string(),
            "-P".to_string(),
            "-F".to_string(),
            "#{window_id} #{pane_id}".to_string(),
        ];
        if let Some(name) = spawn.name {
            args.push("-n".to_string());
            args.push(name.to_string());
        }
        push_spawn(&mut args, spawn);

        let printed = self.run(&borrow(&args))?;
        let (window, pane) = printed
            .split_once(' ')
            .with_context(|| format!("new-window printed {printed:?}"))?;
        Ok((WindowId::new(window)?, PaneId::new(pane)?))
    }

    /// Split `window`'s current pane, and answer with the pane that appeared.
    pub fn split_window(&self, window: &WindowId, spawn: &Spawn<'_>) -> Result<PaneId> {
        let mut args = vec![
            "split-window".to_string(),
            "-t".to_string(),
            window.as_str().to_string(),
            "-P".to_string(),
            "-F".to_string(),
            "#{pane_id}".to_string(),
        ];
        push_spawn(&mut args, spawn);
        PaneId::new(self.run(&borrow(&args))?)
    }

    /// The session with this name, or `None`.
    ///
    /// A server nothing is listening on is a server with no sessions in it,
    /// which is that same `None` and not a failure.
    ///
    /// Names are listed and looked through rather than targeted: a name is
    /// what a person reads on a status line, and the id beside it is the only
    /// thing that addresses the session again.
    pub fn session_named(&self, name: &str) -> Result<Option<SessionId>> {
        let listed = match self.run(&["list-sessions", "-F", "#{session_id} #{session_name}"]) {
            Ok(listed) => listed,
            Err(e) if is_no_server(&e) => return Ok(None),
            Err(e) => return Err(e),
        };
        named(&listed, name).map(SessionId::new).transpose()
    }

    /// The window with this name in `session`, or `None`.
    pub fn window_named(&self, session: &SessionId, name: &str) -> Result<Option<WindowId>> {
        let listed = self.run(&[
            "list-windows",
            "-t",
            session.as_str(),
            "-F",
            "#{window_id} #{window_name}",
        ])?;
        named(&listed, name).map(WindowId::new).transpose()
    }

    /// Re-lay a window's panes — `tiled` is the wall's layout.
    pub fn select_layout(&self, window: &WindowId, layout: &str) -> Result<()> {
        self.run(&["select-layout", "-t", window.as_str(), layout])?;
        Ok(())
    }

    /// Every pane on the server.
    pub fn panes(&self) -> Result<Vec<PaneId>> {
        self.run(&["list-panes", "-a", "-F", "#{pane_id}"])?
            .lines()
            .map(PaneId::new)
            .collect()
    }

    /// Whether the pane is still there — asked of `list-panes`, because a
    /// value read answers for a gone pane as happily as for a live one.
    pub fn pane_alive(&self, pane: &PaneId) -> bool {
        self.panes().is_ok_and(|panes| panes.contains(pane))
    }

    /// Read one format from a pane. A **value read**: an empty answer means
    /// the format was empty *or* the pane is gone, and this cannot tell you
    /// which. Ask [`Server::pane_alive`] for that.
    pub fn pane_field(&self, pane: &PaneId, format: &str) -> Result<String> {
        self.run(&["display-message", "-p", "-t", pane.as_str(), format])
    }

    /// The process id of the pane's process group leader.
    pub fn pane_pid(&self, pane: &PaneId) -> Result<i32> {
        let printed = self.pane_field(pane, "#{pane_pid}")?;
        printed
            .trim()
            .parse()
            .with_context(|| format!("pane {pane} reported pid {printed:?}"))
    }

    /// Whether somebody is looking at this pane right now.
    ///
    /// Three flags, read together, and every one of them has to be set: the
    /// pane is the active one in its window, the window is the one its session
    /// is showing, and a client is attached to that session. Being active is
    /// not enough — a session nobody has ever attached to has an active pane
    /// too, and it is on nobody's screen.
    ///
    /// A **value read**, so a pane that has gone answers emptily and reads as
    /// unwatched. Whatever asks this is deciding whether to interrupt
    /// somebody, and that is the direction worth being wrong in: a
    /// notification nobody needed beats a question nobody was told about.
    pub fn pane_watched(&self, pane: &PaneId) -> bool {
        self.pane_field(pane, "#{pane_active} #{window_active} #{session_attached}")
            .is_ok_and(|printed| watched_flags(&printed))
    }

    /// What is on the pane's screen now, sanitized.
    pub fn capture(&self, pane: &PaneId) -> Result<String> {
        let raw = self.run(&["capture-pane", "-p", "-J", "-t", pane.as_str()])?;
        Ok(sanitize(&raw))
    }

    /// The same screen with the paint the pane was drawn in kept.
    ///
    /// The one capture that is not sanitized here, because the escapes are
    /// what it is for. Whatever reads it has to walk them: [`crate::ansi`]
    /// consumes every escape sequence and answers with runs of text and the
    /// style each was drawn in, and it is that text — never this string — that
    /// is made inert and handed to a terminal.
    pub fn capture_painted(&self, pane: &PaneId) -> Result<String> {
        self.run(&["capture-pane", "-p", "-e", "-J", "-t", pane.as_str()])
    }

    /// Put `text` into the pane as a bracketed paste.
    ///
    /// The text travels through a buffer on stdin, never in the argv: it is
    /// arbitrary, and an argv is the one place it could be read as tmux
    /// syntax.
    pub fn paste(&self, pane: &PaneId, text: &str) -> Result<()> {
        let buffer = format!("amx-{}", pane.as_str().trim_start_matches('%'));
        self.run_with_stdin(&["load-buffer", "-b", &buffer, "-"], Some(text.as_bytes()))?;
        self.run(&[
            "paste-buffer",
            "-d", // the buffer is this paste's, and goes with it
            "-p", // bracketed, so the agent reads it as a paste and not as keys
            "-b",
            &buffer,
            "-t",
            pane.as_str(),
        ])?;
        Ok(())
    }

    /// Send keys to the pane by tmux key name (`Enter`, `Escape`, `C-c`, …).
    pub fn send_keys(&self, pane: &PaneId, keys: &[&str]) -> Result<()> {
        let mut args = vec!["send-keys", "-t", pane.as_str()];
        args.extend_from_slice(keys);
        self.run(&args)?;
        Ok(())
    }

    /// Set a pane-scoped option.
    pub fn set_pane_option(&self, pane: &PaneId, name: &str, value: &str) -> Result<()> {
        self.run(&["set-option", "-p", "-t", pane.as_str(), name, value])?;
        Ok(())
    }

    /// Read a pane-scoped option, or `None` when this pane does not set it.
    ///
    /// The whole pane-scoped set is listed and looked through rather than
    /// asked for by name: `show-options -p` answers for this pane only, while
    /// a `#{@name}` format would walk up and hand back a global.
    pub fn pane_option(&self, pane: &PaneId, name: &str) -> Result<Option<String>> {
        let listed = self.run(&["show-options", "-p", "-t", pane.as_str()])?;
        Ok(listed.lines().find_map(|line| {
            let (key, value) = line.split_once(' ')?;
            (key == name).then(|| unquote(value))
        }))
    }

    /// Unset a pane-scoped option, so nothing amx set outlives the agent.
    pub fn unset_pane_option(&self, pane: &PaneId, name: &str) -> Result<()> {
        self.run(&["set-option", "-p", "-u", "-t", pane.as_str(), name])?;
        Ok(())
    }

    /// Set a session-scoped option (`destroy-unattached`, and the rest).
    pub fn set_session_option(&self, session: &SessionId, name: &str, value: &str) -> Result<()> {
        self.run(&["set-option", "-t", session.as_str(), name, value])?;
        Ok(())
    }

    /// Kill one pane. A window and a session go when their last pane does.
    pub fn kill_pane(&self, pane: &PaneId) -> Result<()> {
        self.run(&["kill-pane", "-t", pane.as_str()])?;
        Ok(())
    }

    /// Kill one session and every pane in it.
    pub fn kill_session(&self, session: &SessionId) -> Result<()> {
        self.run(&["kill-session", "-t", session.as_str()])?;
        Ok(())
    }

    /// The command that hands this terminal to a session, for a caller that
    /// means to exec it.
    pub fn attach_command(&self, session: &SessionId) -> Command {
        let mut cmd = self.command();
        cmd.arg("attach-session").arg("-t").arg(session.as_str());
        cmd
    }
}

/// The directory and command shared by every creating verb.
fn push_spawn(args: &mut Vec<String>, spawn: &Spawn<'_>) {
    if let Some(cwd) = spawn.cwd {
        args.push("-c".to_string());
        args.push(cwd.to_string_lossy().into_owned());
    }
    if !spawn.command.is_empty() {
        // Everything past this is the pane's argv, not tmux's.
        args.push("--".to_string());
        args.extend(spawn.command.iter().map(|arg| arg.to_string()));
    }
}

fn borrow(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

/// The id tmux listed beside `name`, in a listing of `<id> <name>` lines.
fn named<'a>(listed: &'a str, name: &str) -> Option<&'a str> {
    listed.lines().find_map(|line| {
        let (id, listed) = line.split_once(' ')?;
        (listed == name).then_some(id)
    })
}

/// The three flags [`Server::pane_watched`] asks for, all of them set.
///
/// `session_attached` is a count of clients rather than a flag, and any of
/// them is somebody.
fn watched_flags(printed: &str) -> bool {
    let flags: Vec<&str> = printed.split_whitespace().collect();
    flags.len() == 3
        && flags
            .iter()
            .all(|flag| flag.parse::<u32>().is_ok_and(|set| set > 0))
}

/// tmux quotes an option value when it has to; take the quotes back off.
fn unquote(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
        .to_string()
}

/// Whether tmux is saying nothing is listening, which it says three ways: a
/// socket that was never there at all, one with no server behind it any more,
/// and a server that went while it was being asked.
fn is_no_server(err: &anyhow::Error) -> bool {
    let said = format!("{err:#}");
    said.contains("error connecting to")
        || said.contains("no server running")
        || said.contains("server exited")
}

/// The installed tmux's version, as major and minor.
pub fn version() -> Result<(u32, u32)> {
    let out = Command::new("tmux")
        .arg("-V")
        .output()
        .context("running `tmux -V`: is tmux installed?")?;
    let text = String::from_utf8_lossy(&out.stdout);
    parse_version(&text).with_context(|| format!("cannot read a version from {text:?}"))
}

/// Read `tmux 3.4a` and friends. tmux ships letters after the minor version,
/// and pre-releases call themselves `next-3.5`; neither changes the number amx
/// compares against its floor.
pub fn parse_version(text: &str) -> Option<(u32, u32)> {
    let token = text.split_whitespace().nth(1)?;
    let token = token.rsplit('-').next()?;
    let (major, rest) = token.split_once('.')?;
    let minor: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if minor.is_empty() {
        return None;
    }
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// Make a capture safe to match rules against and to print.
///
/// Every control character other than the newline becomes a space — including
/// U+0080–U+009F, where the 8-bit CSI lives that `capture-pane` passes through
/// verbatim. Invisible format characters become spaces too. Both are
/// *replaced*: deleting a zero-width space would let one identifier wear
/// another's spelling.
pub fn sanitize(raw: &str) -> String {
    raw.chars()
        .map(|c| match c {
            '\n' => '\n',
            c if c.is_control() || is_format(c) => ' ',
            c => c,
        })
        .collect()
}

/// The invisible format characters (Unicode `Cf`) worth neutralising: the
/// bidirectional overrides, the zero-width joiners and spaces, the byte order
/// mark, and the tag characters that can spell out a hidden line.
fn is_format(c: char) -> bool {
    matches!(c,
        '\u{00ad}'
        | '\u{0600}'..='\u{0605}'
        | '\u{061c}'
        | '\u{06dd}'
        | '\u{070f}'
        | '\u{180e}'
        | '\u{200b}'..='\u{200f}'
        | '\u{202a}'..='\u{202e}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{206f}'
        | '\u{feff}'
        | '\u{fff9}'..='\u{fffb}'
        | '\u{110bd}'
        | '\u{1d173}'..='\u{1d17a}'
        | '\u{e0001}'
        | '\u{e0020}'..='\u{e007f}')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// A private server of this test's own, gone when the test is.
    struct TestServer(Server);

    impl TestServer {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let tag = format!(
                "amx-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            // An empty conf, so nothing in the developer's ~/.tmux.conf can
            // change what these tests measure.
            Self(Server::named(tag).with_conf("/dev/null"))
        }
    }

    impl std::ops::Deref for TestServer {
        type Target = Server;
        fn deref(&self) -> &Server {
            &self.0
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            let _ = self.0.kill();
        }
    }

    /// A shell that sits there without exiting, so a pane stays a pane.
    const IDLE: &[&str] = &["sh", "-c", "while :; do sleep 0.05; done"];

    fn idle() -> Spawn<'static> {
        Spawn {
            command: IDLE,
            ..Spawn::default()
        }
    }

    /// Poll until `f` is happy, the way the code polls: no fixed sleep stands
    /// in for a state change.
    fn until(what: &str, mut f: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {what}");
    }

    #[test]
    fn tmux_version_reads_through_the_letters_and_prefixes() {
        assert_eq!(parse_version("tmux 3.2\n"), Some((3, 2)));
        assert_eq!(parse_version("tmux 3.4a"), Some((3, 4)));
        assert_eq!(parse_version("tmux next-3.5"), Some((3, 5)));
        assert_eq!(parse_version("tmux 3.7b\n"), Some((3, 7)));
        assert_eq!(parse_version("tmux master"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn tmux_installed_here_meets_the_floor() {
        let v = version().expect("tmux must be installed to run these tests");
        assert!(
            v >= MINIMUM_VERSION,
            "tmux {v:?} is below {MINIMUM_VERSION:?}"
        );
    }

    #[test]
    fn tmux_sanitizing_replaces_control_and_invisible_characters() {
        // Newlines are the capture's structure and survive; the rest of the
        // control range becomes spaces, U+009B included.
        assert_eq!(sanitize("a\nb"), "a\nb");
        assert_eq!(sanitize("a\u{1b}[2Jb"), "a [2Jb");
        assert_eq!(sanitize("a\u{9b}2Jb"), "a 2Jb");
        assert_eq!(sanitize("a\tb\u{7}"), "a b ");
        // Replaced, never deleted: the halves must not close up.
        assert_eq!(sanitize("ad\u{200b}min"), "ad min");
        assert_eq!(sanitize("a\u{202e}b\u{feff}c"), "a b c");
        assert_eq!(sanitize("plain text"), "plain text");
    }

    #[test]
    fn tmux_ids_are_ids_and_names_are_not() {
        assert_eq!(PaneId::new("%3").unwrap().as_str(), "%3");
        assert_eq!(WindowId::new("@1").unwrap().to_string(), "@1");
        assert_eq!(SessionId::new("$0").unwrap().as_str(), "$0");
        for bad in ["", "%", "3", "amx-view", "build: api", "@1"] {
            assert!(PaneId::new(bad).is_err(), "{bad:?} is not a pane id");
        }
    }

    #[test]
    fn tmux_reads_the_server_it_is_inside_from_the_environment() {
        let server = Server::from_tmux_env("/tmp/tmux-1000/default,4242,0").unwrap();
        assert_eq!(
            server.socket(),
            &Socket::Path(PathBuf::from("/tmp/tmux-1000/default"))
        );
        assert_eq!(Server::from_tmux_env(""), None);
    }

    #[test]
    fn tmux_conf_rides_every_call_because_any_of_them_may_start_the_server() {
        let server = Server::named("amx-example").with_conf("/etc/amx.conf");
        let cmd = server.command();
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy()).collect();
        assert_eq!(args, ["-f", "/etc/amx.conf", "-L", "amx-example"]);
    }

    #[test]
    fn tmux_a_recorded_socket_addresses_the_same_server_again() {
        // meta.json carries the socket, so a later verb reaches the server the
        // agent is actually on.
        for socket in [
            Socket::Name("amx".to_string()),
            Socket::Path(PathBuf::from("/tmp/tmux-1000/default")),
        ] {
            let json = serde_json::to_string(&socket).unwrap();
            let read: Socket = serde_json::from_str(&json).unwrap();
            assert_eq!(read, socket);
            assert_eq!(Server::from_socket(read).socket(), &socket);
        }
    }

    #[test]
    fn tmux_attaching_targets_the_session_by_id() {
        let server = Server::named("amx");
        let cmd = server.attach_command(&SessionId::new("$7").unwrap());
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy()).collect();
        assert_eq!(args, ["-L", "amx", "attach-session", "-t", "$7"]);
    }

    #[test]
    fn tmux_a_hidden_session_can_be_told_to_outlive_its_last_client() {
        // What `--bg` rests on: without this, tmux destroys a session the
        // moment nobody is attached to it.
        let server = TestServer::new();
        let (session, pane) = server.new_session(&idle()).unwrap();

        server
            .set_session_option(&session, "destroy-unattached", "off")
            .unwrap();
        assert_eq!(
            server
                .run(&[
                    "show-options",
                    "-t",
                    session.as_str(),
                    "-v",
                    "destroy-unattached"
                ])
                .unwrap(),
            "off"
        );
        assert!(server.pane_alive(&pane));
    }

    #[test]
    fn tmux_finds_a_session_and_a_window_by_the_name_they_wear() {
        let server = TestServer::new();
        // A socket nothing has ever listened on holds no sessions, and saying
        // so is not a failure. Nor is a server that has gone since — tmux has
        // a different sentence for each, and neither is an error to a question
        // about what sessions there are.
        assert_eq!(server.session_named("amx").unwrap(), None);

        let (session, _) = server
            .new_session(&Spawn {
                name: Some("amx"),
                ..idle()
            })
            .unwrap();
        assert_eq!(
            server.session_named("amx").unwrap().as_ref(),
            Some(&session)
        );
        assert_eq!(server.session_named("elsewhere").unwrap(), None);

        assert_eq!(server.window_named(&session, "amx-view").unwrap(), None);
        let (window, _) = server
            .new_window(
                &session,
                &Spawn {
                    name: Some("amx-view"),
                    ..idle()
                },
            )
            .unwrap();
        assert_eq!(
            server.window_named(&session, "amx-view").unwrap().as_ref(),
            Some(&window)
        );

        server.kill().unwrap();
        until("the server to go", || !server.is_alive());
        assert_eq!(server.session_named("amx").unwrap(), None);
    }

    #[test]
    fn tmux_starts_a_session_and_ends_a_server() {
        let server = TestServer::new();
        assert!(
            !server.is_alive(),
            "a socket nobody has used yet is not alive"
        );

        let (session, pane) = server
            .new_session(&Spawn {
                name: Some("first"),
                ..idle()
            })
            .unwrap();
        assert!(server.is_alive());
        assert!(server.pane_alive(&pane));
        assert!(server.panes().unwrap().contains(&pane));

        server.kill_session(&session).unwrap();
        until("the server to go with its last session", || {
            !server.is_alive()
        });
    }

    #[test]
    fn tmux_addresses_a_window_whose_name_no_target_could_reach() {
        let server = TestServer::new();
        let (session, _) = server.new_session(&idle()).unwrap();

        // A colon in a name splits tmux's target syntax, so the id is the only
        // way back to this window.
        let (window, pane) = server
            .new_window(
                &session,
                &Spawn {
                    name: Some("build: api"),
                    ..idle()
                },
            )
            .unwrap();

        let second = server.split_window(&window, &idle()).unwrap();
        assert_ne!(second, pane);
        server.select_layout(&window, "tiled").unwrap();

        let panes = server.panes().unwrap();
        assert!(panes.contains(&pane) && panes.contains(&second));
    }

    #[test]
    fn tmux_starts_a_pane_in_the_directory_it_was_given() {
        let dir = tempfile::TempDir::new().unwrap();
        let server = TestServer::new();
        let (_, pane) = server
            .new_session(&Spawn {
                cwd: Some(dir.path()),
                ..idle()
            })
            .unwrap();

        let path = server.pane_field(&pane, "#{pane_current_path}").unwrap();
        assert_eq!(
            std::fs::canonicalize(path).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
    }

    #[test]
    fn tmux_captures_what_is_on_the_screen() {
        let server = TestServer::new();
        let (_, pane) = server
            .new_session(&Spawn {
                command: &[
                    "sh",
                    "-c",
                    "printf 'HELLO \\033[31mRED\\033[0m\\n'; while :; do sleep 0.05; done",
                ],
                ..Spawn::default()
            })
            .unwrap();

        until("the marker to reach the screen", || {
            server.capture(&pane).is_ok_and(|s| s.contains("HELLO"))
        });
        let screen = server.capture(&pane).unwrap();
        assert!(screen.contains("RED"), "{screen:?}");
        assert!(!screen.contains('\u{1b}'), "a capture carries no escapes");
    }

    #[test]
    fn tmux_pastes_text_the_pane_then_reads() {
        let server = TestServer::new();
        let (_, pane) = server
            .new_session(&Spawn {
                command: &[
                    "sh",
                    "-c",
                    "read line; printf 'GOT:%s\\n' \"$line\"; while :; do sleep 0.05; done",
                ],
                ..Spawn::default()
            })
            .unwrap();

        // Text carrying the characters that would bite in an argv.
        let text = "fix the $PATH; rm -rf \"quoted\"";
        server.paste(&pane, text).unwrap();
        server.send_keys(&pane, &["Enter"]).unwrap();

        until("the pane to read the paste", || {
            server
                .capture(&pane)
                .is_ok_and(|s| s.contains("GOT:") && s.contains(text))
        });
    }

    #[test]
    fn tmux_pane_options_do_not_answer_for_the_whole_server() {
        let server = TestServer::new();
        let (_, pane) = server.new_session(&idle()).unwrap();

        assert_eq!(server.pane_option(&pane, "@amx-id").unwrap(), None);
        server
            .set_pane_option(&pane, "@amx-id", "fix-login-a1b")
            .unwrap();
        assert_eq!(
            server.pane_option(&pane, "@amx-id").unwrap().as_deref(),
            Some("fix-login-a1b")
        );

        // A global of the same name must not be mistaken for this pane's.
        server
            .run(&["set-option", "-g", "@amx-elsewhere", "global"])
            .unwrap();
        assert_eq!(server.pane_option(&pane, "@amx-elsewhere").unwrap(), None);

        server.unset_pane_option(&pane, "@amx-id").unwrap();
        assert_eq!(server.pane_option(&pane, "@amx-id").unwrap(), None);
    }

    #[test]
    fn tmux_liveness_comes_from_the_pane_list_not_from_a_value_read() {
        let server = TestServer::new();
        let (session, first) = server.new_session(&idle()).unwrap();
        let (_, second) = server.new_window(&session, &idle()).unwrap();

        assert!(server.pane_pid(&second).unwrap() > 0);
        server.kill_pane(&second).unwrap();
        until("the pane to leave the list", || !server.pane_alive(&second));

        // The gotcha the law exists for: the value read is happy either way.
        let answer = server.pane_field(&second, "#{pane_pid}");
        assert!(
            answer.as_deref().map(str::trim).unwrap_or("").is_empty(),
            "a value read on a gone pane must not answer as if it were alive: {answer:?}"
        );
        assert!(server.pane_alive(&first), "the other pane is untouched");
    }

    #[test]
    fn tmux_watching_a_pane_takes_all_three_flags() {
        assert!(watched_flags("1 1 1"));
        assert!(watched_flags("1 1 2"), "two clients are still somebody");
        for nobody in ["0 1 1", "1 0 1", "1 1 0", "1 1", "", "   ", "x y z"] {
            assert!(!watched_flags(nobody), "{nobody:?}");
        }
    }

    #[test]
    fn tmux_says_whether_anybody_is_looking_at_a_pane() {
        let server = TestServer::new();
        let (_, first) = server.new_session(&idle()).unwrap();

        // `new-session -d` leaves an active pane in an active window that no
        // client is attached to, which is the whole reason all three flags are
        // asked for.
        assert!(!server.pane_watched(&first), "nobody is attached");

        let window = WindowId::new(server.pane_field(&first, "#{window_id}").unwrap()).unwrap();
        let second = server.split_window(&window, &idle()).unwrap();
        assert!(!server.pane_watched(&second), "and it is not even active");

        // A pane that has gone answers emptily, and that reads as unwatched:
        // this question decides whether to interrupt somebody, and the wrong
        // direction to be wrong in is the silent one.
        server.kill_pane(&second).unwrap();
        until("the pane to leave the list", || !server.pane_alive(&second));
        assert!(!server.pane_watched(&second));
    }

    #[test]
    fn tmux_identifies_a_pane_by_the_command_it_was_started_with() {
        // Between fork and exec a pane reports `tmux` as its current command,
        // so what it was *started* with is the only stable identity.
        let server = TestServer::new();
        let (_, pane) = server.new_session(&idle()).unwrap();
        let started = server.pane_field(&pane, "#{pane_start_command}").unwrap();
        assert!(started.contains("sleep 0.05"), "{started:?}");
    }

    #[test]
    fn tmux_says_which_command_failed() {
        let server = TestServer::new();
        server.new_session(&idle()).unwrap();
        let err = server.run(&["kill-pane", "-t", "%404"]).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("kill-pane"), "{message}");
    }
}
