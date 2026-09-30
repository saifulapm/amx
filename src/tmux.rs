//! The tmux command line, wrapped.
//!
//! amx is never in the byte path: an agent is a tmux pane, and everything amx
//! does to it is a `tmux(1)` invocation. Invariants:
//!
//! - Ids, never names. `%pane` and `$session` ids are stored and targeted:
//!   target syntax splits a name at `:`, and `-t 0` reads as an index.
//! - A value read is no liveness check: `display -p -t <gone>` prints nothing
//!   and succeeds. A pane is alive while `list-panes` lists it.
//! - A pane number is no identity: tmux numbers panes from `%0` per server, so a
//!   restarted server reuses them. A pane answers for the id stamped in
//!   [`ID_OPTION`], else for the id its [`SESSION_PREFIX`] session is named
//!   for; see [`Server::pane_owners`].
//! - Pane options are read with `show-options -p`, since a `#{@option}` format
//!   falls back to the global scope. [`Server::pane_owners`] reads
//!   [`ID_OPTION`] as a format anyway, because amx only sets it per pane.
//! - Captures are sanitized: control characters (including the 8-bit CSI
//!   U+009B, which `capture-pane` passes through) and invisible format
//!   characters become spaces. `capture_painted` is the one raw capture.
//! - A conf, when set, rides every call, because whichever call starts the
//!   server decides the file it reads. amx sets none; the tests do.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The oldest tmux amx supports.
pub const MINIMUM_VERSION: (u32, u32) = (3, 2);

/// The pane option holding the id of the agent a pane answers for.
///
/// Set on adopted panes, which sit in someone else's session; see
/// [`crate::verbs::adopt`]. Pane-scoped options need tmux 3.0.
pub const ID_OPTION: &str = "@amx-id";

/// The prefix of the session name a placed agent gets; see
/// [`crate::spawn::place`].
pub const SESSION_PREFIX: &str = "amx-";

/// Where a tmux server listens.
///
/// `-L <name>` for servers amx starts, because tmux recreates a named socket's
/// directory after a reboot and does not for `-S <path>`. `-S <path>` is for
/// the server named by `$TMUX`.
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

/// The working directory of a tmux server's process.
///
/// A server keeps the directory it started in. Once that directory is deleted,
/// every pane it forks starts somewhere that does not exist and its command
/// dies before drawing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCwd {
    pub pid: i32,
    /// The directory, without the kernel's ` (deleted)` suffix.
    pub path: PathBuf,
    /// Whether the directory has been deleted.
    pub stale: bool,
}

macro_rules! tmux_id {
    ($name:ident, $sigil:literal, $what:literal) => {
        #[doc = concat!("A tmux ", $what, " id, `", $sigil, "` and all.")]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(String);

        impl $name {
            /// Take an id as tmux printed it. A name in its place is refused
            /// here instead of failing later as an unaddressable target.
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
tmux_id!(PaneId, '%', "pane");

/// Which agent each pane on a server answers for, from one listing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneOwners(HashMap<PaneId, String>);

impl PaneOwners {
    /// Whether `pane` answers for agent `id`.
    ///
    /// False for a pane the server does not list, one stamped with another id,
    /// and one in a session named for another agent.
    pub fn pane_answers_for(&self, pane: &PaneId, id: &str) -> bool {
        self.0.get(pane).is_some_and(|owner| owner == id)
    }
}

/// What to create, and where.
#[derive(Debug, Default, Clone)]
pub struct Spawn<'a> {
    /// A session name for display. amx never targets by it.
    pub name: Option<&'a str>,
    /// The new pane's working directory.
    pub cwd: Option<&'a Path>,
    /// The pane's argv. Empty runs the user's shell.
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

    /// The server named by `$TMUX`, whose value is
    /// `<socket path>,<pid>,<session index>`.
    pub fn from_tmux_env(value: &str) -> Option<Self> {
        let path = value.split(',').next().filter(|p| !p.is_empty())?;
        Some(Self::at(path))
    }

    /// The server at a recorded socket.
    pub fn from_socket(socket: Socket) -> Self {
        Self { socket, conf: None }
    }

    /// Pass `-f conf` on every call, so whichever call starts the server reads
    /// it instead of `~/.tmux.conf`.
    #[cfg(test)]
    pub fn with_conf(mut self, conf: impl Into<PathBuf>) -> Self {
        self.conf = Some(conf.into());
        self
    }

    /// The socket, for recording.
    pub fn socket(&self) -> &Socket {
        &self.socket
    }

    /// A `tmux` command for this server, before its subcommand.
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

    /// Run one tmux command and return its stdout, trailing whitespace
    /// trimmed.
    pub fn run(&self, args: &[&str]) -> Result<String> {
        self.run_with_stdin(args, None)
    }

    /// [`Server::run`], with `stdin` written to the command's input.
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
        // The pipe is dropped after the write, which is the EOF tmux waits for.
        let wrote = match (stdin, child.stdin.take()) {
            (Some(bytes), Some(mut pipe)) => pipe.write_all(bytes),
            _ => Ok(()),
        };

        // Wait even if the write failed, or a tmux that exited without reading
        // stays a zombie.
        let out = child
            .wait_with_output()
            .with_context(|| format!("waiting for `tmux {}`", args.join(" ")))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            bail!("tmux {}: {}", args.join(" "), stderr.trim());
        }
        wrote.with_context(|| format!("writing to `tmux {}`", args.join(" ")))?;
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    }

    /// Whether a server is listening on this socket.
    #[cfg(test)]
    pub fn is_alive(&self) -> bool {
        self.run(&["list-sessions", "-F", "#{session_id}"]).is_ok()
    }

    /// Where this server's process is standing, if that can be read.
    ///
    /// `None` when no server is listening, off Linux, or when `/proc` cannot be
    /// read. Asked with `list-sessions`, which fails on a dead socket instead
    /// of starting a server.
    pub fn cwd(&self) -> Option<ServerCwd> {
        let printed = self.run(&["list-sessions", "-F", "#{pid}"]).ok()?;
        let pid: i32 = printed.lines().next()?.trim().parse().ok()?;
        let (path, stale) = standing(pid)?;
        Some(ServerCwd { pid, path, stale })
    }

    /// Kill the server and everything on it. A server already gone is success.
    ///
    /// The socket file is removed too: tmux 3.7 leaves it after `kill-server`,
    /// and test servers would pile up files. A socket that something still
    /// answers at is left alone.
    #[cfg(test)]
    pub fn kill(&self) -> Result<()> {
        let going = match self.run(&["kill-server"]) {
            // The server took the order, so its socket is dead even if it is
            // still open.
            Ok(_) => true,
            // No server. The file is dead unless something else is listening
            // at it, which the probe checks.
            Err(e) if is_no_server(&e) => false,
            Err(e) => return Err(e),
        };
        let path = match &self.socket {
            Socket::Name(name) => socket_dir().join(name),
            Socket::Path(path) => path.clone(),
        };
        if going || nobody_answers(&path) {
            // Already gone is fine.
            let _ = std::fs::remove_file(&path);
        }
        Ok(())
    }

    /// Create a detached session and return it with its first pane.
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
        push_spawn(&mut args, spawn);

        let printed = again_if_the_server_went(|| self.run(&borrow(&args)))?;
        let (session, pane) = printed
            .split_once(' ')
            .with_context(|| format!("new-session printed {printed:?}"))?;
        Ok((SessionId::new(session)?, PaneId::new(pane)?))
    }

    /// The session named `name`, if any.
    ///
    /// No server listening is `None`. Sessions are listed and matched by name
    /// because only the id addresses one reliably.
    pub fn session_named(&self, name: &str) -> Result<Option<SessionId>> {
        let listed = match self.run(&["list-sessions", "-F", "#{session_id} #{session_name}"]) {
            Ok(listed) => listed,
            Err(e) if is_no_server(&e) => return Ok(None),
            Err(e) => return Err(e),
        };
        named(&listed, name).map(SessionId::new).transpose()
    }

    /// Every pane on the server.
    pub fn panes(&self) -> Result<Vec<PaneId>> {
        self.run(&["list-panes", "-a", "-F", "#{pane_id}"])?
            .lines()
            .map(PaneId::new)
            .collect()
    }

    /// Whether `list-panes` lists `pane`.
    ///
    /// About the pane number only, which tmux reuses. Whether an agent still
    /// has its pane is [`Server::pane_answers_for`].
    pub fn pane_alive(&self, pane: &PaneId) -> bool {
        self.panes().is_ok_and(|panes| panes.contains(pane))
    }

    /// Which agent each pane on this server answers for, in one `list-panes`.
    ///
    /// The stamp comes before the session name in the format, because a
    /// session name may contain spaces and an id may not. An unset option
    /// prints as an empty field.
    pub fn pane_owners(&self) -> Result<PaneOwners> {
        let format = format!("#{{pane_id}} #{{{ID_OPTION}}} #{{session_name}}");
        Ok(owners(&self.run(&["list-panes", "-a", "-F", &format])?))
    }

    /// Whether `pane` answers for agent `id`; see [`PaneOwners`].
    ///
    /// A server that cannot be asked reads as the pane being lost.
    pub fn pane_answers_for(&self, pane: &PaneId, id: &str) -> bool {
        self.pane_owners()
            .is_ok_and(|owners| owners.pane_answers_for(pane, id))
    }

    /// [`Server::pane_answers_for`], for callers about to act on the answer.
    ///
    /// No server listening is `false`, but any other tmux failure is an error:
    /// reading it as a lost pane could delete a live tree or start a second
    /// pane beside the first.
    pub fn answers_for_now(&self, pane: &PaneId, id: &str) -> Result<bool> {
        Ok(self.owners_for_now()?.pane_answers_for(pane, id))
    }

    /// [`Server::pane_owners`], with no server listening read as no owners and
    /// any other failure as an error.
    pub fn owners_for_now(&self) -> Result<PaneOwners> {
        match self.pane_owners() {
            Ok(owners) => Ok(owners),
            Err(e) if is_no_server(&e) => Ok(PaneOwners::default()),
            Err(e) => Err(e.context("tmux could not be asked")),
        }
    }

    /// Read one format from a pane.
    ///
    /// A value read: a gone pane prints an empty string, the same as an empty
    /// format. Use [`Server::pane_alive`] for liveness.
    pub fn pane_field(&self, pane: &PaneId, format: &str) -> Result<String> {
        self.run(&["display-message", "-p", "-t", pane.as_str(), format])
    }

    /// The pid of the pane's process group leader.
    pub fn pane_pid(&self, pane: &PaneId) -> Result<i32> {
        let printed = self.pane_field(pane, "#{pane_pid}")?;
        printed
            .trim()
            .parse()
            .with_context(|| format!("pane {pane} reported pid {printed:?}"))
    }

    /// Whether someone is looking at this pane: it is the active pane of the
    /// active window of a session with a client attached.
    ///
    /// A value read, so a gone pane reads as unwatched. Callers use this to
    /// decide whether to notify, where an extra notice is the safer error.
    pub fn pane_watched(&self, pane: &PaneId) -> bool {
        self.pane_field(pane, "#{pane_active} #{window_active} #{session_attached}")
            .is_ok_and(|printed| watched_flags(&printed))
    }

    /// The tty of every client attached to this server.
    ///
    /// `list-clients` does not start a server, so a dead socket fails, which
    /// reads as no clients.
    pub fn client_ttys(&self) -> Vec<PathBuf> {
        self.run(&["list-clients", "-F", "#{client_tty}"])
            .map(|listed| listed.lines().map(PathBuf::from).collect())
            .unwrap_or_default()
    }

    /// The pane's visible screen, sanitized.
    pub fn capture(&self, pane: &PaneId) -> Result<String> {
        let raw = self.run(&["capture-pane", "-p", "-J", "-t", pane.as_str()])?;
        Ok(sanitize(&raw))
    }

    /// The screens of `panes`, sanitized and in order, from one invocation.
    ///
    /// Each capture is a fork and a round trip, so a wall of twenty agents
    /// would otherwise cost twenty. Screens are split at a per-call
    /// [`marker`] printed before each capture, since pane text can contain
    /// any fixed string.
    ///
    /// A pane gone since the listing is `None`. tmux stops a command sequence
    /// at the first failure, so the panes after it are asked again in a new
    /// invocation. A server that prints nothing at all is not asked again.
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
            // Resume after the pane the sequence failed on.
            from += answered.len() + 1;
        }
        screens
    }

    /// Run one tmux command and return whether it succeeded, with its stdout.
    ///
    /// For [`Server::captures`], which needs what a sequence printed before
    /// the command that failed.
    fn printed(&self, args: &[&str]) -> (bool, String) {
        match self.command().args(args).output() {
            Ok(out) => (
                out.status.success(),
                String::from_utf8_lossy(&out.stdout).into_owned(),
            ),
            Err(_) => (false, String::new()),
        }
    }

    /// The pane's screen with its escape sequences kept.
    ///
    /// The one unsanitized capture. Callers parse it with [`crate::ansi`],
    /// which yields text runs and their styles; only that text is sanitized
    /// and drawn.
    pub fn capture_painted(&self, pane: &PaneId) -> Result<String> {
        self.run(&["capture-pane", "-p", "-e", "-J", "-t", pane.as_str()])
    }

    /// Paste `text` into the pane as a bracketed paste.
    ///
    /// The text goes through a buffer loaded from stdin, never through argv,
    /// where tmux could parse it.
    pub fn paste(&self, pane: &PaneId, text: &str) -> Result<()> {
        let buffer = format!("amx-{}", pane.as_str().trim_start_matches('%'));
        self.run_with_stdin(&["load-buffer", "-b", &buffer, "-"], Some(text.as_bytes()))?;
        self.run(&[
            "paste-buffer",
            "-d", // delete the buffer afterwards
            "-p", // bracketed, so the agent sees a paste, not keystrokes
            "-b",
            &buffer,
            "-t",
            pane.as_str(),
        ])?;
        Ok(())
    }

    /// Send keys to the pane by tmux key name (`Enter`, `Escape`, `C-c`, ...).
    pub fn send_keys(&self, pane: &PaneId, keys: &[&str]) -> Result<()> {
        let mut args = vec!["send-keys", "-t", pane.as_str()];
        args.extend_from_slice(keys);
        self.run(&args)?;
        Ok(())
    }

    /// Pipe everything the pane prints from now on into `command`'s stdin.
    ///
    /// `command` is run by `sh` in the tmux server's environment, not the
    /// pane's. `-o` makes this a toggle: on a pane already piped it closes
    /// that pipe and opens nothing, so it never replaces an existing pipe.
    pub fn pipe_pane(&self, pane: &PaneId, command: &str) -> Result<()> {
        self.run(&["pipe-pane", "-o", "-t", pane.as_str(), &literal(command)])?;
        Ok(())
    }

    /// Run a shell command on this server after `delay` seconds.
    ///
    /// amx has no daemon and the server outlives every amx process, so the
    /// server holds the timer, and the timer dies with it. `-b` runs it in the
    /// background, `-d` delays it. As with [`Server::pipe_pane`], `command`
    /// runs under `sh` in the server's environment.
    pub fn run_after(&self, delay: u64, command: &str) -> Result<()> {
        let command = literal(command);
        self.run(&["run-shell", "-b", "-d", &delay.to_string(), &command])?;
        Ok(())
    }

    /// Set a pane-scoped option.
    pub fn set_pane_option(&self, pane: &PaneId, name: &str, value: &str) -> Result<()> {
        self.run(&["set-option", "-p", "-t", pane.as_str(), name, value])?;
        Ok(())
    }

    /// A pane-scoped option's value, or `None` when this pane does not set it.
    ///
    /// Read from `show-options -p`, since a `#{@name}` format falls back to a
    /// global value.
    #[cfg(test)]
    pub fn pane_option(&self, pane: &PaneId, name: &str) -> Result<Option<String>> {
        let listed = self.run(&["show-options", "-p", "-t", pane.as_str()])?;
        Ok(listed.lines().find_map(|line| {
            let (key, value) = line.split_once(' ')?;
            (key == name).then(|| unquote(value))
        }))
    }

    /// Set a session-scoped option.
    pub fn set_session_option(&self, session: &SessionId, name: &str, value: &str) -> Result<()> {
        self.run(&["set-option", "-t", session.as_str(), name, value])?;
        Ok(())
    }

    /// Kill one pane. Its window and session go with their last pane.
    pub fn kill_pane(&self, pane: &PaneId) -> Result<()> {
        self.run(&["kill-pane", "-t", pane.as_str()])?;
        Ok(())
    }

    /// Kill one session and every pane in it.
    pub fn kill_session(&self, session: &SessionId) -> Result<()> {
        self.run(&["kill-session", "-t", session.as_str()])?;
        Ok(())
    }

    /// The command that attaches this terminal to `session`, for the caller to
    /// exec.
    pub fn attach_command(&self, session: &SessionId) -> Command {
        let mut cmd = self.command();
        cmd.arg("attach-session").arg("-t").arg(session.as_str());
        cmd
    }

    /// Bind root-table `C-z` as the way out of an agent's session.
    ///
    /// In a session named with [`SESSION_PREFIX`] the client switches back to
    /// its last session (the view's, inside tmux), or detaches when that is
    /// gone or there was none. In any other session the key goes to the pane.
    /// The explicit comparison is needed because tmux reads a bare `0`, the
    /// name of a default first session, as false.
    ///
    /// Rebound on every hand-over, since the server may have restarted. claude
    /// binds `C-z` to suspend itself, which is useless in a pane with no shell.
    pub fn bind_way_back(&self) -> Result<()> {
        self.run(&[
            "bind-key",
            "-n",
            "C-z",
            "if-shell",
            "-F",
            &format!("#{{m:{SESSION_PREFIX}*,#{{session_name}}}}"),
            "if-shell -F '#{!=:#{client_last_session},}' 'switch-client -l' 'detach-client'",
            "send-keys C-z",
        ])?;
        Ok(())
    }
}

/// Every listening tmux server of this user, by socket path.
///
/// tmux keeps one socket directory per user (`$TMUX_TMPDIR`, else `/tmp`, then
/// `tmux-<uid>`), and it may hold several servers. Socket files outlive killed
/// servers, so only sockets that accept a connection count; a refused connect
/// costs a syscall where asking tmux would cost a process.
pub fn servers_here() -> Vec<Server> {
    listening_in(&socket_dir())
        .into_iter()
        .map(Server::at)
        .collect()
}

/// Whether nothing accepts connections at this socket.
///
/// Only asked when `kill-server` found no server. A server that took the order
/// keeps its socket open for a moment (about 15ms on tmux 3.7), so probing it
/// then would leave the file behind.
#[cfg(test)]
fn nobody_answers(socket: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(socket).is_err()
}

/// The sockets in `dir` that accept a connection.
fn listening_in(dir: &Path) -> Vec<PathBuf> {
    sockets_in(dir)
        .into_iter()
        .filter(|socket| std::os::unix::net::UnixStream::connect(socket).is_ok())
        .collect()
}

/// This user's tmux socket directory. An empty `$TMUX_TMPDIR` counts as unset,
/// as it does for tmux.
fn socket_dir() -> PathBuf {
    let tmp = std::env::var_os("TMUX_TMPDIR")
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    tmp.join(format!("tmux-{}", nix::unistd::Uid::current()))
}

/// The socket files in `dir`, skipping anything else stored there.
fn sockets_in(dir: &Path) -> Vec<PathBuf> {
    use std::os::unix::fs::FileTypeExt;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_socket()))
        .map(|entry| entry.path())
        .collect()
}

/// Append the `-c` directory and the command shared by the creating commands.
fn push_spawn(args: &mut Vec<String>, spawn: &Spawn<'_>) {
    if let Some(cwd) = spawn.cwd {
        args.push("-c".to_string());
        args.push(literal(&cwd.to_string_lossy()));
    }
    if !spawn.command.is_empty() {
        // Everything after `--` is the pane's argv, not tmux's.
        args.push("--".to_string());
        args.extend(spawn.command.iter().map(|arg| arg.to_string()));
    }
}

/// Escape `#` so tmux reads `text` as written instead of as a `#{...}` format.
fn literal(text: &str) -> String {
    text.replace('#', "##")
}

fn borrow(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

/// One command sequence capturing every pane, each capture preceded by a
/// `display-message` of `marker`.
///
/// The marker goes first so a capture that never ran is a marker with nothing
/// after it.
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

/// A line separating one pane's screen from the next in a batch.
///
/// Built from the pid and a counter, so no pane can already be showing it.
fn marker() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "amx-capture-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// Split a batch's output at `marker` into screens, in the order asked.
///
/// Output before the first marker is ignored, and no marker at all is `None`:
/// the server said nothing about any pane. At most `asked` screens are
/// returned, in case a pane was showing the marker itself.
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
    // In a failed sequence the last marker's capture is the one that failed.
    if !ended_well {
        screens.pop();
    }
    screens.truncate(asked);
    Some(screens)
}

/// Parse `<pane id> <stamp> <session name>` lines: each pane answers for its
/// stamped id, else the id its session is named for.
///
/// Panes with neither, and lines in any other shape, are left out.
fn owners(listed: &str) -> PaneOwners {
    let mut owners = HashMap::new();
    for line in listed.lines() {
        let Some((pane, rest)) = line.split_once(' ') else {
            continue;
        };
        let Some((stamp, session)) = rest.split_once(' ') else {
            continue;
        };
        let owner = match stamp.is_empty() {
            false => stamp,
            true => match session.strip_prefix(SESSION_PREFIX) {
                Some(id) if !id.is_empty() => id,
                _ => continue,
            },
        };
        let Ok(pane) = PaneId::new(pane) else {
            continue;
        };
        owners.insert(pane, owner.to_string());
    }
    PaneOwners(owners)
}

/// The id listed beside `name` in `<id> <name>` lines.
fn named<'a>(listed: &'a str, name: &str) -> Option<&'a str> {
    listed.lines().find_map(|line| {
        let (id, listed) = line.split_once(' ')?;
        (listed == name).then_some(id)
    })
}

/// Whether all three flags [`Server::pane_watched`] reads are set.
///
/// `session_attached` is a client count, so any value above zero counts.
fn watched_flags(printed: &str) -> bool {
    let flags: Vec<&str> = printed.split_whitespace().collect();
    flags.len() == 3
        && flags
            .iter()
            .all(|flag| flag.parse::<u32>().is_ok_and(|set| set > 0))
}

/// Strip the quotes tmux puts around some option values.
#[cfg(test)]
fn unquote(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
        .to_string()
}

/// Whether a tmux error means no server is listening: a socket that never
/// existed, one with no server behind it, or a server that exited mid-call.
fn is_no_server(err: &anyhow::Error) -> bool {
    let said = format!("{err:#}");
    said.contains("error connecting to")
        || said.contains("no server running")
        || said.contains("server exited")
}

/// A socket no tmux can be started against, for tests: the NUL in the name
/// makes the command fail before it runs, as it does with no tmux installed.
#[cfg(test)]
pub(crate) fn unaskable() -> Socket {
    Socket::Name("amx\0unaskable".to_string())
}

/// Retry once when the first attempt found no server.
///
/// A server exiting after its last session holds its socket for a moment, and
/// a client arriving then is told the server exited. Nothing was changed, and
/// the next client starts a fresh server. Only once: a second failure is not
/// that race, and a loop could spin on a socket nobody will answer.
fn again_if_the_server_went<T>(mut attempt: impl FnMut() -> Result<T>) -> Result<T> {
    match attempt() {
        Err(e) if is_no_server(&e) => attempt(),
        answer => answer,
    }
}

/// Process `pid`'s working directory, and whether it has been deleted.
///
/// Linux only, through `/proc/<pid>/cwd`; elsewhere there is no way to read it
/// without another dependency.
#[cfg(target_os = "linux")]
fn standing(pid: i32) -> Option<(PathBuf, bool)> {
    let link = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    // The kernel marks a deleted directory with a ` (deleted)` suffix, and stat
    // cannot tell because the process still holds the inode. A directory
    // really named `x (deleted)` still exists, which tells the two apart.
    match unlinked(&link) {
        Some(path) if !link.exists() => Some((path, true)),
        _ => Some((link, false)),
    }
}

#[cfg(not(target_os = "linux"))]
fn standing(_pid: i32) -> Option<(PathBuf, bool)> {
    None
}

/// The link without the kernel's ` (deleted)` suffix, if it had one.
fn unlinked(link: &Path) -> Option<PathBuf> {
    Some(PathBuf::from(
        link.as_os_str().to_str()?.strip_suffix(" (deleted)")?,
    ))
}

/// The installed tmux's major and minor version.
pub fn version() -> Result<(u32, u32)> {
    let out = Command::new("tmux")
        .arg("-V")
        .output()
        .context("running `tmux -V`: is tmux installed?")?;
    let text = String::from_utf8_lossy(&out.stdout);
    parse_version(&text).with_context(|| format!("cannot read a version from {text:?}"))
}

/// Parse `tmux 3.4a` and similar. The letter suffix, and the `next-` prefix of
/// a pre-release, do not change the number compared against the floor.
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
/// Control characters other than newline, including U+0080 to U+009F where
/// the 8-bit CSI lives, become spaces, and so do invisible format characters.
/// They are replaced with spaces: deleting a zero-width space would let
/// `ad\u{200b}min` read as `admin`.
pub fn sanitize(raw: &str) -> String {
    raw.chars()
        .map(|c| match c {
            '\n' => '\n',
            c if c.is_control() || is_format(c) => ' ',
            c => c,
        })
        .collect()
}

/// The invisible format characters (Unicode `Cf`) to neutralise: bidi
/// overrides, zero-width joiners and spaces, the byte order mark, and the tag
/// characters that can spell hidden text.
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

    /// A private server for one test, killed on drop.
    struct TestServer(Server);

    impl TestServer {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let tag = format!(
                "amx-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            // An empty conf, so the developer's ~/.tmux.conf cannot affect
            // the tests.
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

    /// A command that runs until killed, so the pane stays.
    const IDLE: &[&str] = &["sh", "-c", "while :; do sleep 0.05; done"];

    fn idle() -> Spawn<'static> {
        Spawn {
            command: IDLE,
            ..Spawn::default()
        }
    }

    /// Poll `f` until it returns true, failing after ten seconds.
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

    /// Start a server on this socket from a client whose cwd is `cwd`.
    ///
    /// A server takes its working directory from the client that started it,
    /// not from a session's `-c`, which is why this is not `new_session`.
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
        // Only a suffix counts.
        assert_eq!(unlinked(Path::new("/tmp/(deleted)/app")), None);
    }

    #[test]
    fn a_server_says_where_it_is_standing() {
        let dir = tempfile::TempDir::new().unwrap();
        // The kernel reports the resolved path, and /tmp is a symlink on some
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

        // Delete the directory the server is standing in.
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

    /// The error `run` reports for a client that reached an exiting server
    /// (tmux 3.5a).
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
        // Newlines survive; the rest of the control range, U+009B included,
        // becomes spaces.
        assert_eq!(sanitize("a\nb"), "a\nb");
        assert_eq!(sanitize("a\u{1b}[2Jb"), "a [2Jb");
        assert_eq!(sanitize("a\u{9b}2Jb"), "a 2Jb");
        assert_eq!(sanitize("a\tb\u{7}"), "a b ");
        // Replaced, so the halves do not join.
        assert_eq!(sanitize("ad\u{200b}min"), "ad min");
        assert_eq!(sanitize("a\u{202e}b\u{feff}c"), "a b c");
        assert_eq!(sanitize("plain text"), "plain text");
    }

    #[test]
    fn tmux_ids_are_ids_and_names_are_not() {
        assert_eq!(PaneId::new("%3").unwrap().as_str(), "%3");
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
        // meta.json records the socket, so later verbs reach the same server.
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
        // Without this, tmux destroys a session once no client is attached.
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
        // A socket nothing has listened on has no sessions, and neither does
        // a server that has gone. tmux words the two differently; neither is
        // an error.
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

        let windows = || {
            let format = "#{window_id} #{window_name}";
            let listed = ["list-windows", "-t", session.as_str(), "-F", format];
            server.run(&listed).unwrap()
        };
        assert_eq!(named(&windows(), "amx-view"), None);
        let create = ["new-window", "-t", session.as_str(), "-n", "amx-view"];
        let window = server
            .run(&[&create[..], &["-P", "-F", "#{window_id}", "--"], IDLE].concat())
            .unwrap();
        assert_eq!(named(&windows(), "amx-view"), Some(window.as_str()));

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

        // A colon splits tmux's target syntax, so only pane ids reach this
        // window.
        let create = ["new-window", "-t", session.as_str(), "-n", "build: api"];
        let printed = ["-P", "-F", "#{pane_id}", "--"];
        let pane = server.run(&[&create[..], &printed, IDLE].concat()).unwrap();
        let pane = PaneId::new(pane).unwrap();
        let split = ["split-window", "-t", pane.as_str()];
        let second = server.run(&[&split[..], &printed, IDLE].concat()).unwrap();
        let second = PaneId::new(second).unwrap();
        assert_ne!(second, pane);

        let panes = server.panes().unwrap();
        assert!(panes.contains(&pane) && panes.contains(&second));
        server.kill_pane(&second).unwrap();
        until("the split pane to go", || !server.pane_alive(&second));
        assert!(server.pane_alive(&pane));
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
    fn tmux_a_directory_with_a_hash_sign_is_the_directory_as_spelled() {
        let parent = tempfile::TempDir::new().unwrap();
        let dir = parent.path().join("a#{session_id}#S##b");
        std::fs::create_dir(&dir).unwrap();
        let server = TestServer::new();
        let (_, pane) = server
            .new_session(&Spawn {
                cwd: Some(&dir),
                ..idle()
            })
            .unwrap();

        let path = server.pane_field(&pane, "#{pane_current_path}").unwrap();
        assert_eq!(
            std::fs::canonicalize(&path).ok(),
            std::fs::canonicalize(&dir).ok(),
            "tmux read {path:?} for the directory"
        );
    }

    #[test]
    fn tmux_a_command_with_a_hash_sign_runs_as_spelled() {
        let parent = tempfile::TempDir::new().unwrap();
        let dir = parent.path().join("a#{session_id}#S##b");
        std::fs::create_dir(&dir).unwrap();
        let (kept, fired, go) = (dir.join("output"), dir.join("fired"), dir.join("go"));
        let script = format!(
            "while [ ! -f '{}' ]; do sleep 0.02; done; printf 'one\\n'; while :; do sleep 0.05; done",
            go.display()
        );
        let server = TestServer::new();
        let (_, pane) = server
            .new_session(&Spawn {
                command: &["sh", "-c", &script],
                ..Spawn::default()
            })
            .unwrap();

        server
            .pipe_pane(&pane, &format!("cat >> '{}'", kept.display()))
            .unwrap();
        std::fs::write(&go, "").unwrap();
        server
            .run_after(0, &format!("printf '#S' > '{}'", fired.display()))
            .unwrap();

        until("the pipe to reach the file as spelled", || {
            std::fs::read_to_string(&kept).is_ok_and(|text| text.contains("one"))
        });
        until("the command to run as spelled", || {
            std::fs::read_to_string(&fired).is_ok_and(|text| text == "#S")
        });
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
    fn tmux_pipes_everything_a_pane_prints_into_a_command() {
        let dir = tempfile::TempDir::new().unwrap();
        let kept = dir.path().join("output");
        let second = dir.path().join("second");
        let (first_word, last_word) = (dir.path().join("go"), dir.path().join("go-again"));
        let server = TestServer::new();

        // The pane prints nothing until told to, so the pipe is attached before
        // its first line.
        let script = format!(
            "while [ ! -f '{first}' ]; do sleep 0.02; done; printf 'one\\ntwo\\n'; \
             while [ ! -f '{last}' ]; do sleep 0.02; done; printf 'three\\n'; \
             while :; do sleep 0.05; done",
            first = first_word.display(),
            last = last_word.display(),
        );
        let (_, pane) = server
            .new_session(&Spawn {
                command: &["sh", "-c", &script],
                ..Spawn::default()
            })
            .unwrap();

        server
            .pipe_pane(&pane, &format!("cat >> '{}'", kept.display()))
            .unwrap();
        std::fs::write(&first_word, "").unwrap();
        until("the pane's output to reach the file", || {
            std::fs::read_to_string(&kept).is_ok_and(|text| text.lines().count() == 2)
        });
        assert_eq!(
            std::fs::read_to_string(&kept)
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            ["one", "two"]
        );

        // `-o` is a toggle: a second call closes the pipe and opens nothing,
        // so the second command never starts.
        server
            .pipe_pane(&pane, &format!("cat >> '{}'", second.display()))
            .unwrap();
        std::fs::write(&last_word, "").unwrap();
        until("the pane to say its last word on the screen", || {
            server
                .capture(&pane)
                .is_ok_and(|screen| screen.contains("three"))
        });
        assert_eq!(
            std::fs::read_to_string(&kept)
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            ["one", "two"],
            "the file stops where the pipe did"
        );
        assert!(!second.exists(), "and nothing was opened in its place");
    }

    #[test]
    fn tmux_runs_a_command_after_the_delay_and_not_before_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let fired = dir.path().join("fired");
        let server = TestServer::new();
        server.new_session(&idle()).unwrap();

        server
            .run_after(1, &format!("touch '{}'", fired.display()))
            .unwrap();
        // The delay counts from the call and load can only make the command
        // later, so a file already there means the delay was skipped.
        std::thread::sleep(Duration::from_millis(500));
        assert!(!fired.exists(), "it ran before its delay was up");

        until("the delayed command to run", || fired.exists());
    }

    /// A pane that prints `word` once and then idles.
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
            // Only this pane's words: a cut in the wrong place would mix
            // screens.
            for other in ["FIRST", "SECOND", "THIRD"].iter().filter(|w| *w != word) {
                assert!(!screen.contains(other), "{at}: {screen:?}");
            }
        }

        // The same text a single capture gives, sanitizing included, so rules
        // match what they were written against.
        assert_eq!(
            screens[1].as_deref(),
            Some(server.capture(&panes[1]).unwrap().as_str())
        );
        assert!(server.captures(&[]).is_empty());
    }

    #[test]
    fn tmux_a_pane_that_went_costs_its_own_screen_and_no_others() {
        // tmux stops a sequence at the first failure, so a pane gone since the
        // listing would otherwise cost every pane after it.
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
        // The sequence stopped at the third pane; the first two are kept.
        assert_eq!(
            screens("M\nfirst\nM\nsecond\nM\n", false, 3),
            said(&["first\n", "second\n"])
        );
        // No marker: the server said nothing about any pane.
        assert_eq!(screens("", false, 3), None);
        assert_eq!(screens("no server running\n", false, 3), None);
        // A pane showing the marker splits its own screen; nothing past the
        // panes asked about is returned.
        assert_eq!(
            screens("M\nfirst\nM\nsecond\nM\nand the rest of second\n", true, 2),
            said(&["first\n", "second\n"])
        );
    }

    #[test]
    fn tmux_a_server_that_is_not_there_answers_for_none_of_its_panes() {
        // No server, so no marker comes back, and asking pane by pane would
        // get the same silence.
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

        // Characters that would be interpreted in an argv.
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

        // A global of the same name is not this pane's.
        server
            .run(&["set-option", "-g", "@amx-elsewhere", "global"])
            .unwrap();
        assert_eq!(server.pane_option(&pane, "@amx-elsewhere").unwrap(), None);

        server
            .run(&["set-option", "-p", "-u", "-t", pane.as_str(), "@amx-id"])
            .unwrap();
        assert_eq!(server.pane_option(&pane, "@amx-id").unwrap(), None);
    }

    #[test]
    fn tmux_a_pane_answers_for_the_agent_written_on_it_or_the_one_its_session_names() {
        let server = TestServer::new();
        // A placed pane: its session is named for the agent, nothing stamped.
        let (_, placed) = server
            .new_session(&Spawn {
                name: Some(&format!("{SESSION_PREFIX}fix-login-a1b")),
                ..idle()
            })
            .unwrap();
        // An adopted pane: someone else's session, with a space in its name,
        // and the id stamped on the pane.
        let (_, adopted) = server
            .new_session(&Spawn {
                name: Some("my work"),
                ..idle()
            })
            .unwrap();
        server
            .set_pane_option(&adopted, ID_OPTION, "port-importer-c3d")
            .unwrap();

        let owners = server.pane_owners().unwrap();
        assert!(owners.pane_answers_for(&placed, "fix-login-a1b"));
        assert!(owners.pane_answers_for(&adopted, "port-importer-c3d"));
        // Neither pane answers for the other agent.
        assert!(!owners.pane_answers_for(&placed, "port-importer-c3d"));
        assert!(!owners.pane_answers_for(&adopted, "fix-login-a1b"));
        assert!(!owners.pane_answers_for(&PaneId::new("%404").unwrap(), "fix-login-a1b"));

        // A stamp outranks the session name: adopting can take over a pane
        // amx placed for another agent.
        server
            .set_pane_option(&placed, ID_OPTION, "port-importer-c3d")
            .unwrap();
        let owners = server.pane_owners().unwrap();
        assert!(!owners.pane_answers_for(&placed, "fix-login-a1b"));
        assert!(owners.pane_answers_for(&placed, "port-importer-c3d"));

        // The same question through the server.
        assert!(server.pane_answers_for(&adopted, "port-importer-c3d"));
        assert!(!server.pane_answers_for(&adopted, "fix-login-a1b"));
    }

    #[test]
    fn tmux_a_tmux_that_cannot_be_asked_is_not_an_answer() {
        // No server listening is an answer: nothing on it is anyone's.
        let gone = Server::named(format!("amx-no-such-server-{}", std::process::id()));
        let pane = PaneId::new("%0").unwrap();
        assert!(!gone.answers_for_now(&pane, "fix-login-a1b").unwrap());

        // A tmux that never ran said nothing, so the caller gets the error.
        let unasked = Server::from_socket(unaskable());
        let why = unasked.answers_for_now(&pane, "fix-login-a1b").unwrap_err();
        assert!(
            format!("{why:#}").starts_with("tmux could not be asked: "),
            "{why:#}"
        );
    }

    #[test]
    fn tmux_a_listing_of_owners_reads_the_stamp_before_the_session_name() {
        // Lines as tmux 3.7c prints them for a placed and an adopted pane. An
        // unset option is an empty field, hence the double space; the stamp
        // comes before the session name, which may contain spaces.
        let read = owners("%0  amx-fix-login-a1b\n%1 port-importer-c3d my work\n");
        assert!(read.pane_answers_for(&PaneId::new("%0").unwrap(), "fix-login-a1b"));
        assert!(read.pane_answers_for(&PaneId::new("%1").unwrap(), "port-importer-c3d"));

        // No stamp in someone's own session is no agent, and neither is a
        // session whose name is the bare prefix.
        let read = owners("%2  my work\n%3  amx\n");
        for nobody in ["", "my work", "amx", "work"] {
            assert!(!read.pane_answers_for(&PaneId::new("%2").unwrap(), nobody));
            assert!(!read.pane_answers_for(&PaneId::new("%3").unwrap(), nobody));
        }
    }

    #[test]
    fn tmux_liveness_comes_from_the_pane_list_not_from_a_value_read() {
        let server = TestServer::new();
        let (_, first) = server.new_session(&idle()).unwrap();
        let (_, second) = server.new_session(&idle()).unwrap();

        assert!(server.pane_pid(&second).unwrap() > 0);
        server.kill_pane(&second).unwrap();
        until("the pane to leave the list", || !server.pane_alive(&second));

        // The value read still succeeds on a gone pane.
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

        // `new-session -d` leaves an active pane in an active window with no
        // client attached, which is why all three flags are checked.
        assert!(!server.pane_watched(&first), "nobody is attached");

        // `-d` leaves the new pane inactive.
        let split = ["split-window", "-d", "-t", first.as_str(), "-P", "-F"];
        let second = server.run(&[&split[..], &["#{pane_id}", "--"], IDLE].concat());
        let second = PaneId::new(second.unwrap()).unwrap();
        assert!(!server.pane_watched(&second), "and it is not even active");

        // A gone pane reads as unwatched: a missed notification is the worse
        // error.
        server.kill_pane(&second).unwrap();
        until("the pane to leave the list", || !server.pane_alive(&second));
        assert!(!server.pane_watched(&second));
    }

    #[test]
    fn tmux_identifies_a_pane_by_the_command_it_was_started_with() {
        // Between fork and exec a pane reports `tmux` as its current command,
        // so the start command is the stable identity.
        let server = TestServer::new();
        let (_, pane) = server.new_session(&idle()).unwrap();
        let started = server.pane_field(&pane, "#{pane_start_command}").unwrap();
        assert!(started.contains("sleep 0.05"), "{started:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn tmux_that_exits_without_reading_its_input_is_waited_for() {
        fn unwaited() -> std::collections::BTreeSet<String> {
            std::fs::read_to_string("/proc/thread-self/children")
                .unwrap_or_default()
                .split_whitespace()
                .filter(|pid| {
                    std::fs::read_to_string(format!("/proc/{pid}/comm"))
                        .is_ok_and(|comm| comm.trim() == "tmux")
                })
                .map(str::to_string)
                .collect()
        }

        // `-V` prints the version and exits, so the input meets a closed pipe.
        let server = Server::named("amx-unused").with_conf("/dev/null");
        let input = vec![b'x'; 1 << 20];
        let before = unwaited();
        assert!(server.run_with_stdin(&["-V"], Some(&input)).is_err());
        let left: Vec<_> = unwaited().difference(&before).cloned().collect();
        assert!(left.is_empty(), "tmux was never waited for: {left:?}");
    }

    #[test]
    fn tmux_says_which_command_failed() {
        let server = TestServer::new();
        server.new_session(&idle()).unwrap();
        let err = server.run(&["kill-pane", "-t", "%404"]).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("kill-pane"), "{message}");
    }

    /// A client attached to `session`, running in a pane of the same server.
    ///
    /// `$TMUX` is unset first, since tmux refuses to attach from inside a pane.
    fn a_client_on(server: &Server, session: &SessionId) -> PaneId {
        let attach = server.attach_command(session);
        let mut argv: Vec<String> = ["env", "-u", "TMUX", "-u", "TMUX_PANE"]
            .map(String::from)
            .into();
        argv.push(attach.get_program().to_string_lossy().into_owned());
        argv.extend(
            attach
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned()),
        );
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let (_, pane) = server
            .new_session(&Spawn {
                command: &argv,
                ..Spawn::default()
            })
            .unwrap();
        pane
    }

    /// The tty of the client on `session`, or empty.
    fn client_on(server: &Server, session: &SessionId) -> String {
        server
            .run(&[
                "list-clients",
                "-t",
                session.as_str(),
                "-F",
                "#{client_tty}",
            ])
            .unwrap_or_default()
    }

    #[test]
    fn tmux_a_server_says_which_terminals_are_looking_at_it() {
        let server = TestServer::new();
        let (session, _) = server.new_session(&idle()).unwrap();
        assert!(
            server.client_ttys().is_empty(),
            "a session nobody is attached to has no terminal at the far end of it"
        );

        a_client_on(&server, &session);
        until("a client on the session", || {
            !client_on(&server, &session).is_empty()
        });
        assert_eq!(
            server.client_ttys(),
            vec![PathBuf::from(client_on(&server, &session))]
        );
    }

    #[test]
    fn tmux_a_server_that_will_not_answer_lists_no_terminals() {
        // No server: asking must neither fail nor start one.
        let server = TestServer::new();
        assert!(server.client_ttys().is_empty());
        assert!(!server.is_alive(), "the question started nothing");
    }

    #[test]
    fn tmux_the_servers_here_are_the_sockets_under_the_socket_directory() {
        let server = TestServer::new();
        server.new_session(&idle()).unwrap();
        let socket = PathBuf::from(
            server
                .run(&["display-message", "-p", "#{socket_path}"])
                .unwrap(),
        );

        let listed = servers_here();
        let listed: Vec<&Socket> = listed.iter().map(Server::socket).collect();
        assert!(
            listed.contains(&&Socket::Path(socket)),
            "this server is one of the person's: {listed:?}"
        );
    }

    #[test]
    fn tmux_only_a_socket_in_the_socket_directory_is_a_server() {
        // Other files and directories in the socket directory are skipped.
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("notes"), "").unwrap();
        std::fs::create_dir(dir.path().join("inner")).unwrap();
        let socket = dir.path().join("default");
        let _listening = std::os::unix::net::UnixListener::bind(&socket).unwrap();

        assert_eq!(sockets_in(dir.path()), vec![socket]);
        assert!(sockets_in(&dir.path().join("nowhere")).is_empty());
    }

    #[test]
    fn tmux_a_socket_nobody_answers_at_is_a_file_a_dead_server_left() {
        let dir = tempfile::TempDir::new().unwrap();
        let live = dir.path().join("default");
        let _listening = std::os::unix::net::UnixListener::bind(&live).unwrap();
        let dead = dir.path().join("amx-count-4242");
        drop(std::os::unix::net::UnixListener::bind(&dead).unwrap());
        assert!(dead.exists(), "the file outlives the listener");

        let mut sockets = sockets_in(dir.path());
        sockets.sort();
        assert_eq!(sockets, vec![dead, live.clone()], "both are socket files");
        assert_eq!(
            listening_in(dir.path()),
            vec![live],
            "and only one of them is a server"
        );
    }

    #[test]
    fn tmux_a_killed_server_takes_its_socket_file_with_it() {
        let server = TestServer::new();
        server.new_session(&idle()).unwrap();
        // Ask tmux for the socket path instead of deriving it as kill does.
        let socket = PathBuf::from(
            server
                .run(&["display-message", "-p", "#{socket_path}"])
                .unwrap(),
        );
        assert!(socket.exists(), "the server is listening at it");

        server.kill().unwrap();
        assert!(
            !socket.exists(),
            "the file went with the server: {socket:?}"
        );
    }

    #[test]
    fn tmux_a_killed_server_addressed_by_path_takes_its_file_too() {
        let dir = tempfile::TempDir::new().unwrap();
        let socket = dir.path().join("by-path");
        let server = Server::at(&socket).with_conf("/dev/null");
        server.new_session(&idle()).unwrap();
        assert!(socket.exists(), "the server is listening at it");

        server.kill().unwrap();
        assert!(
            !socket.exists(),
            "the file went with the server: {socket:?}"
        );
    }

    #[test]
    fn tmux_kill_leaves_a_socket_somebody_answers_at_alone() {
        // Whatever is bound here is not ours to remove. The thread hangs up on
        // each connection, since a tmux client waits for a reply until then.
        let dir = tempfile::TempDir::new().unwrap();
        let socket = dir.path().join("listening");
        let listening = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            while let Ok((client, _)) = listening.accept() {
                drop(client);
            }
        });

        let _ = Server::at(&socket).with_conf("/dev/null").kill();
        assert!(socket.exists(), "somebody is answering at it: {socket:?}");
    }

    #[test]
    fn tmux_ctrl_z_in_an_agents_session_goes_back_the_way_the_client_came() {
        let server = TestServer::new();
        // A view's session, a placed agent's session, and one of the person's
        // own. The last records every byte it receives, with the tty's signal
        // and line handling off, so C-z arrives at once as a byte.
        let (view, _) = server.new_session(&idle()).unwrap();
        let (agent, _) = server
            .new_session(&Spawn {
                name: Some(&format!("{SESSION_PREFIX}fix-login-a1b")),
                ..idle()
            })
            .unwrap();
        let dir = tempfile::TempDir::new().unwrap();
        let typed = dir.path().join("typed");
        let record = format!("stty -isig -icanon; exec cat > {}", typed.display());
        let (theirs, _) = server
            .new_session(&Spawn {
                name: Some("mine"),
                command: &["sh", "-c", &record],
                ..Spawn::default()
            })
            .unwrap();

        server.bind_way_back().unwrap();

        // Inside tmux: the client moves from the view to the agent, and C-z
        // moves it back.
        let pane = a_client_on(&server, &view);
        until("a client on the view", || {
            !client_on(&server, &view).is_empty()
        });
        let tty = client_on(&server, &view);
        server
            .run(&["switch-client", "-c", &tty, "-t", agent.as_str()])
            .unwrap();
        until("the client on the agent", || {
            client_on(&server, &agent) == tty
        });
        server
            .run(&["send-keys", "-t", pane.as_str(), "C-z"])
            .unwrap();
        until("the client back on the view", || {
            client_on(&server, &view) == tty
        });

        // In a session amx did not name, C-z reaches the pane and the client
        // stays.
        server
            .run(&["switch-client", "-c", &tty, "-t", theirs.as_str()])
            .unwrap();
        until("the client on their own session", || {
            client_on(&server, &theirs) == tty
        });
        server
            .run(&["send-keys", "-t", pane.as_str(), "C-z"])
            .unwrap();
        until("the byte in their pane", || {
            std::fs::read(&typed).is_ok_and(|bytes| bytes.contains(&0x1a))
        });
        assert_eq!(client_on(&server, &theirs), tty, "the client did not move");
        server.kill_pane(&pane).unwrap();

        // Outside tmux: a client attached straight to the agent's session has
        // nowhere to switch back to, so C-z detaches it.
        let lent = a_client_on(&server, &agent);
        until("a client on the agent", || {
            !client_on(&server, &agent).is_empty()
        });
        server
            .run(&["send-keys", "-t", lent.as_str(), "C-z"])
            .unwrap();
        until("the client gone", || client_on(&server, &agent).is_empty());
    }
}
