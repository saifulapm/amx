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

/// Where a tmux server's own process is standing.
///
/// A server holds the working directory it was started in for as long as it
/// lives, and a directory can be deleted out from under it. Every pane it
/// forks afterwards inherits a place that is not there, and the command in
/// that pane dies before it draws anything — which is what makes this worth
/// asking about rather than an idle curiosity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCwd {
    pub pid: i32,
    /// The directory it is standing in, named the way the kernel names it and
    /// without the marker below.
    pub path: PathBuf,
    /// Whether that directory has been deleted.
    pub stale: bool,
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

    /// Where this server is standing, when that can be known.
    ///
    /// `None` three ways, none of them a fault to report: no server is
    /// listening on the socket, this is not a platform where one process can
    /// read another's working directory, or `/proc` would not answer. Only a
    /// server amx can see standing somewhere is worth judging.
    ///
    /// `list-sessions` is what asks, because it fails on a socket with no
    /// server rather than starting one.
    pub fn cwd(&self) -> Option<ServerCwd> {
        let printed = self.run(&["list-sessions", "-F", "#{pid}"]).ok()?;
        let pid: i32 = printed.lines().next()?.trim().parse().ok()?;
        let (path, stale) = standing(pid)?;
        Some(ServerCwd { pid, path, stale })
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

        let printed = again_if_the_server_went(|| self.run(&borrow(&args)))?;
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

    /// What is on each of these panes' screens now, sanitized, in the order
    /// they were asked about.
    ///
    /// One invocation for the lot of them. A capture is a fork, an exec and a
    /// round trip to the server — a millisecond and a half of it — and a
    /// reading of a wall takes one per agent it cannot account for from the
    /// record: twenty agents is twenty of them, in a row, on the thread
    /// somebody is waiting at. tmux takes a sequence of commands in one
    /// invocation, so the wall costs the one.
    ///
    /// The screens are told apart by a marker printed in front of each, and
    /// the marker is this call's own — see [`marker`] — because what is on a
    /// pane is somebody else's text and a word it happened to be showing
    /// would cut the batch in the wrong place.
    ///
    /// A pane that has gone since the list was taken answers with `None`. It
    /// ends the invocation where it stands, because a sequence runs until one
    /// command fails and stops there, so the panes behind it are asked again
    /// without it rather than going with it. A server that answers nothing at
    /// all is not asked again: that is about the server, and the panes on it
    /// have nothing to say either way.
    pub fn captures(&self, panes: &[PaneId]) -> Vec<Option<String>> {
        let mut screens: Vec<Option<String>> = vec![None; panes.len()];
        let mut from = 0;
        while from < panes.len() {
            let marker = marker();
            let (ended_well, printed) = self.printed(&borrow(&batch(&panes[from..], &marker)));
            let Some(answered) = answered(&printed, &marker, ended_well, panes.len() - from) else {
                break;
            };
            for (at, screen) in answered.iter().enumerate() {
                screens[from + at] = Some(sanitize(screen.trim_end()));
            }
            if ended_well {
                break;
            }
            // Past the pane the sequence stopped at, which has answered.
            from += answered.len() + 1;
        }
        screens
    }

    /// Run one tmux command line and answer with whether it ended well and
    /// what it printed.
    ///
    /// The one call that wants a failure's output rather than a sentence about
    /// it: a sequence stops at the first command that fails, and everything
    /// the commands before it printed is on stdout and is what the caller came
    /// for.
    fn printed(&self, args: &[&str]) -> (bool, String) {
        match self.command().args(args).output() {
            Ok(out) => (
                out.status.success(),
                String::from_utf8_lossy(&out.stdout).into_owned(),
            ),
            Err(_) => (false, String::new()),
        }
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

/// The one command line that captures every one of these panes: a marker and
/// a capture apiece, with the semicolons tmux reads a sequence by.
///
/// The marker goes in front of its capture rather than after it, so that a
/// capture that never happened is a marker with nothing under it rather than
/// a gap with nothing to name it.
fn batch(panes: &[PaneId], marker: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    for pane in panes {
        if !args.is_empty() {
            args.push(";".to_string());
        }
        args.extend(["display-message", "-p", marker, ";"].map(str::to_string));
        args.extend(["capture-pane", "-p", "-J", "-t", pane.as_str()].map(str::to_string));
    }
    args
}

/// A line to tell one pane's screen from the next one's in a batch.
///
/// Made here rather than written down, because a marker is only a marker
/// while no pane is showing it: whatever is on a screen is somebody else's
/// text, and an agent that had printed the word amx cuts at would have its
/// screen cut in two. The process and a count that never repeats in it are
/// enough — nothing outside this call ever sees one, so no pane can be
/// showing it.
fn marker() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "amx-capture-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// The screens one invocation answered for, in the order the panes were asked
/// about: all of them where the sequence ran to the end, and the ones before
/// the failure where it did not.
///
/// The output is cut at the markers, and anything printed before the first of
/// them is nobody's screen — that is the server talking about itself. Not one
/// marker coming back is that same server saying nothing at all, which is what
/// `None` is for: it is about the server rather than about any of these panes.
///
/// Nothing is answered for past the panes that were asked about. A pane
/// showing this call's own marker would cut its own screen into two sections,
/// and a wall going up is not worth a panic over a coincidence.
fn answered(printed: &str, marker: &str, ended_well: bool, asked: usize) -> Option<Vec<String>> {
    let mut screens: Vec<String> = Vec::new();
    for line in printed.lines() {
        if line == marker {
            screens.push(String::new());
        } else if let Some(screen) = screens.last_mut() {
            screen.push_str(line);
            screen.push('\n');
        }
    }
    if screens.is_empty() {
        return None;
    }
    // The last marker of a sequence that failed is the pane it failed on: the
    // marker went out and the capture under it never did.
    if !ended_well {
        screens.pop();
    }
    screens.truncate(asked);
    Some(screens)
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

/// Ask once more when the first answer was that nothing was listening.
///
/// A server shutting down after its last session ended still holds its socket
/// for a moment, and a client that arrives inside that moment is told the
/// server exited. Nothing was half-done — a server that went changed nothing
/// — and the next client on that socket starts a fresh one, so the question
/// is simply worth asking again.
///
/// Once, and no further. Two servers going under one caller is not a race any
/// more, and a loop would sit forever on a socket nobody is going to answer.
fn again_if_the_server_went<T>(mut attempt: impl FnMut() -> Result<T>) -> Result<T> {
    match attempt() {
        Err(e) if is_no_server(&e) => attempt(),
        answer => answer,
    }
}

/// Where process `pid` is standing, and whether that place still exists.
///
/// Linux only. `/proc/<pid>/cwd` is the one place a process's working
/// directory is readable from outside it, and there is no equivalent elsewhere
/// that does not cost a dependency. Off Linux amx says nothing rather than
/// guessing.
#[cfg(target_os = "linux")]
fn standing(pid: i32) -> Option<(PathBuf, bool)> {
    let link = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    // The kernel marks a deleted directory by suffixing the link it answers
    // with. Stat cannot confirm it — /proc/<pid>/cwd still resolves, because
    // the process holds the unlinked inode open — so the marker is the only
    // signal there is. A directory genuinely named `x (deleted)` wears the
    // same suffix, and is told apart by still being there.
    match unlinked(&link) {
        Some(path) if !link.exists() => Some((path, true)),
        _ => Some((link, false)),
    }
}

#[cfg(not(target_os = "linux"))]
fn standing(_pid: i32) -> Option<(PathBuf, bool)> {
    None
}

/// A cwd link with the kernel's `(deleted)` marker taken off, when it wore
/// one.
fn unlinked(link: &Path) -> Option<PathBuf> {
    Some(PathBuf::from(
        link.as_os_str().to_str()?.strip_suffix(" (deleted)")?,
    ))
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

    /// Start a server on this socket from a client standing in `cwd`.
    ///
    /// A server takes its working directory from whichever client started it,
    /// not from the `-c` a session was asked for: measured, and the whole
    /// reason this is not `new_session`.
    fn serve_from(server: &Server, cwd: &Path) {
        let out = server
            .command()
            .args(["new-session", "-d"])
            .args(IDLE)
            .current_dir(cwd)
            .output()
            .expect("starting a server");
        assert!(out.status.success(), "{out:?}");
    }

    #[test]
    fn a_cwd_link_the_kernel_marked_deleted_reads_as_the_path_without_it() {
        assert_eq!(
            unlinked(Path::new("/tmp/gone (deleted)")),
            Some(PathBuf::from("/tmp/gone"))
        );
        assert_eq!(unlinked(Path::new("/srv/app")), None);
        // The marker is a suffix, not a word that appears anywhere.
        assert_eq!(unlinked(Path::new("/tmp/(deleted)/app")), None);
    }

    #[test]
    fn a_server_says_where_it_is_standing() {
        let dir = tempfile::TempDir::new().unwrap();
        // The kernel answers with the path it resolved, so compare against
        // that rather than the tempdir's name: /tmp is a symlink on some
        // machines.
        let want = dir.path().canonicalize().unwrap();
        let server = TestServer::new();
        serve_from(&server, dir.path());

        let standing = server.cwd().expect("a running server stands somewhere");
        assert!(standing.pid > 0, "{standing:?}");
        assert_eq!(standing.path, want);
        assert!(
            !standing.stale,
            "the directory is still there: {standing:?}"
        );
    }

    #[test]
    fn a_server_whose_directory_was_deleted_says_so() {
        let dir = tempfile::TempDir::new().unwrap();
        let want = dir.path().canonicalize().unwrap();
        let server = TestServer::new();
        serve_from(&server, dir.path());

        // What poisons a server: the directory it is standing in goes, and it
        // carries on holding a place that is not there any more.
        std::fs::remove_dir_all(dir.path()).unwrap();

        let standing = server.cwd().expect("it is still running");
        assert!(standing.stale, "{standing:?}");
        assert_eq!(
            standing.path, want,
            "the name it is still holding, without the kernel's marker"
        );
    }

    #[test]
    fn a_socket_with_no_server_is_standing_nowhere() {
        let server = TestServer::new();
        assert_eq!(server.cwd(), None, "nothing is listening on it");
    }

    /// What tmux says to a client that reached a server on its way out, in
    /// the shape `run` hands it on: measured against tmux 3.5a.
    fn the_server_went() -> anyhow::Error {
        anyhow::anyhow!("tmux new-session -d: server exited unexpectedly")
    }

    #[test]
    fn a_session_asked_for_as_the_server_went_is_asked_for_again() {
        let asked = std::cell::Cell::new(0);
        let answer = again_if_the_server_went(|| {
            asked.set(asked.get() + 1);
            match asked.get() {
                1 => Err(the_server_went()),
                _ => Ok("$1 %2"),
            }
        });
        assert_eq!(asked.get(), 2, "the first answer was nobody listening");
        assert_eq!(
            answer.unwrap(),
            "$1 %2",
            "and the second is what the caller gets"
        );
    }

    #[test]
    fn a_failure_that_is_not_the_server_going_is_asked_once_and_no_more() {
        let asked = std::cell::Cell::new(0);
        let answer: Result<&str> = again_if_the_server_went(|| {
            asked.set(asked.get() + 1);
            Err(anyhow::anyhow!(
                "tmux new-session -d: duplicate session: a1b"
            ))
        });
        assert_eq!(asked.get(), 1, "a name already taken is not a race");
        assert!(format!("{:#}", answer.unwrap_err()).contains("duplicate session"));
    }

    #[test]
    fn a_second_server_going_is_the_error_the_caller_hears() {
        let asked = std::cell::Cell::new(0);
        let answer: Result<&str> = again_if_the_server_went(|| {
            asked.set(asked.get() + 1);
            Err(anyhow::anyhow!(
                "tmux new-session -d: server exited {}",
                asked.get()
            ))
        });
        assert_eq!(asked.get(), 2, "asked again, once, and no further");
        assert!(
            format!("{:#}", answer.unwrap_err()).contains("server exited 2"),
            "the second failure is the one reported"
        );
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
    fn tmux_a_detached_session_can_be_told_to_outlive_its_last_client() {
        // What every agent rests on: without this, tmux destroys a session the
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

    /// A pane with one word printed on it and nothing else happening.
    fn a_pane_saying(server: &Server, word: &str) -> PaneId {
        let script = format!("printf '{word}\\n'; while :; do sleep 0.05; done");
        let (_, pane) = server
            .new_session(&Spawn {
                command: &["sh", "-c", &script],
                ..Spawn::default()
            })
            .unwrap();
        until(&format!("{word} to reach the screen"), || {
            server.capture(&pane).is_ok_and(|s| s.contains(word))
        });
        pane
    }

    #[test]
    fn tmux_reads_a_wall_of_screens_in_one_call() {
        let server = TestServer::new();
        let panes = ["FIRST", "SECOND", "THIRD"]
            .map(|word| a_pane_saying(&server, word))
            .to_vec();

        let screens = server.captures(&panes);
        assert_eq!(screens.len(), panes.len());
        for (at, word) in ["FIRST", "SECOND", "THIRD"].iter().enumerate() {
            let screen = screens[at].as_deref().expect("every pane answered");
            assert!(screen.contains(word), "{at}: {screen:?}");
            // Each pane's own screen and nobody else's: the batch is cut at
            // markers, and a cut in the wrong place is one pane wearing the
            // next one's words.
            for other in ["FIRST", "SECOND", "THIRD"].iter().filter(|w| *w != word) {
                assert!(!screen.contains(other), "{at}: {screen:?}");
            }
        }

        // The same screen a single capture gives, sieve and all: a batch that
        // read differently would have every rule matched against something
        // else than what `capture` was measured on.
        assert_eq!(
            screens[1].as_deref(),
            Some(server.capture(&panes[1]).unwrap().as_str())
        );
        assert!(server.captures(&[]).is_empty());
    }

    #[test]
    fn tmux_a_pane_that_went_costs_its_own_screen_and_no_others() {
        // tmux runs a sequence of commands until one of them fails and stops
        // there, so a pane that went between the listing and the capture would
        // otherwise take every pane behind it in the batch with it.
        let server = TestServer::new();
        let first = a_pane_saying(&server, "FIRST");
        let second = a_pane_saying(&server, "SECOND");
        let gone = PaneId::new("%404").unwrap();

        let screens = server.captures(&[gone.clone(), first, gone.clone(), second, gone]);
        assert_eq!(screens[0], None);
        assert!(screens[1].as_deref().unwrap().contains("FIRST"));
        assert_eq!(screens[2], None);
        assert!(screens[3].as_deref().unwrap().contains("SECOND"));
        assert_eq!(screens[4], None);
    }

    #[test]
    fn tmux_a_batch_is_cut_at_its_own_markers_and_no_further() {
        let screens = |printed, ended_well, asked| answered(printed, "M", ended_well, asked);
        let said = |lines: &[&str]| Some(lines.iter().map(|line| line.to_string()).collect());

        assert_eq!(
            screens("M\nfirst\nM\nsecond\n", true, 2),
            said(&["first\n", "second\n"])
        );
        // The sequence stopped at the third pane, which answered nothing. The
        // two before it did, and are worth keeping.
        assert_eq!(
            screens("M\nfirst\nM\nsecond\nM\n", false, 3),
            said(&["first\n", "second\n"])
        );
        // Not one marker: the server said nothing, and that is about the
        // server rather than about any of these panes.
        assert_eq!(screens("", false, 3), None);
        assert_eq!(screens("no server running\n", false, 3), None);
        // A pane showing the marker itself cuts its own screen in two. Nobody
        // is answered for who was not asked about.
        assert_eq!(
            screens("M\nfirst\nM\nsecond\nM\nand the rest of second\n", true, 2),
            said(&["first\n", "second\n"])
        );
    }

    #[test]
    fn tmux_a_server_that_is_not_there_answers_for_none_of_its_panes() {
        // Nothing is listening, so not one marker comes back. That is about
        // the server rather than about any of these panes, and asking again
        // pane by pane would be a fork each for the same silence.
        let server = TestServer::new();
        let panes: Vec<PaneId> = (1..=3)
            .map(|n| PaneId::new(format!("%{n}")).unwrap())
            .collect();
        assert_eq!(server.captures(&panes), vec![None, None, None]);
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
