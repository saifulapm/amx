//! Starting an agent: where its pane goes, and what the pane is handed.
//!
//! Nothing amx starts stays resident. A pane runs `amx _boot <id>`, which
//! reads the handoff its record holds, puts the environment back, and execs
//! the vendor with `amx _exit` behind it. From then on the pane belongs to the
//! vendor, and amx is only ever a reader of the record and the screen.
//!
//! Two things travel in the handoff file rather than on the tmux command line.
//! The **task** is arbitrary text, and a tmux command line is the one place it
//! could be read as syntax. The **environment** is the one `new` was run with:
//! a tmux server started an hour ago carries an hour-old environment, and an
//! agent that inherited it would be missing whatever its owner exported since.
//! The file is the owner's alone to read, because their environment is in it.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::registry;
use crate::store::{Agent, Meta};
use crate::tmux::{PaneId, Server, SessionId};

/// What the pane is handed at birth.
pub const HANDOFF: &str = "handoff.json";

/// The window agents tile into.
pub const WALL: &str = "amx-wall";

/// The socket amx's own server listens on, and the session it keeps the wall
/// in.
pub const PRIVATE: &str = "amx";

/// Test-only override of the private server's socket name, so a suite never
/// reaches the machine's real agents.
const SOCKET_ENV: &str = "AMX_TMUX_SOCKET";

/// The variables that belong to the pane a command was typed in, not to the
/// pane it starts: tmux's own two, and the shell's idea of where it is.
const NOT_INHERITED: [&str; 4] = ["TMUX", "TMUX_PANE", "PWD", "OLDPWD"];

/// How long `_boot` waits for the record whose pane it is.
const RECORD_PATIENCE: Duration = Duration::from_secs(10);

/// Everything the pane needs to become an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    /// What the agent was asked to do.
    pub task: String,
    /// The vendor and its arguments, as an argv.
    pub command: Vec<String>,
    /// The environment to run it in.
    pub env: BTreeMap<String, String>,
}

/// Where a new agent's pane goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Tiled into the wall, where somebody can see it.
    Wall,
    /// In a session of its own that nobody is attached to.
    Background,
}

/// The environment an agent inherits, given the one `new` was run with.
pub fn env_snapshot(vars: impl IntoIterator<Item = (String, String)>) -> BTreeMap<String, String> {
    vars.into_iter()
        .filter(|(name, _)| !NOT_INHERITED.contains(&name.as_str()))
        .collect()
}

/// Where a spawn's three dials are pointed, each of them a value the vendor
/// would take or [`registry::DEFAULT`] for one nobody turned.
///
/// Resolving them is `new`'s business, because they come from what the caller
/// typed and what the config holds. Turning them into flags is the registry's,
/// and happens once, here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dials {
    pub model: String,
    pub permission: String,
    pub effort: String,
}

impl Default for Dials {
    /// Every dial left where the vendor's own configuration puts it, which
    /// amx says by sending no flag.
    fn default() -> Dials {
        Dials {
            model: registry::DEFAULT.to_string(),
            permission: registry::DEFAULT.to_string(),
            effort: registry::DEFAULT.to_string(),
        }
    }
}

/// The vendor's argv: the configured command, the dials that are turned,
/// whatever the caller passed through, and the task last — where a prompt
/// goes.
///
/// A dial yields to the same flag written by hand, wherever it was written, so
/// the vendor is never handed one flag twice.
pub fn vendor_command(
    agent: &str,
    dials: &Dials,
    vendor_args: &[String],
    task: &str,
) -> Vec<String> {
    let mut command: Vec<String> = agent.split_whitespace().map(str::to_string).collect();
    command.extend(registry::inject(
        agent,
        &dials.model,
        &dials.permission,
        &dials.effort,
        vendor_args,
    ));
    command.push(task.to_string());
    command
}

/// Write the handoff, readable by nobody else: the person's environment is in
/// it.
pub fn write_handoff(dir: &Path, handoff: &Handoff) -> Result<()> {
    let path = dir.join(HANDOFF);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("creating {}", path.display()))?;
    let mut bytes = serde_json::to_vec_pretty(handoff).context("writing the handoff")?;
    bytes.push(b'\n');
    file.write_all(&bytes)
        .with_context(|| format!("writing {}", path.display()))
}

pub fn read_handoff(dir: &Path) -> Result<Handoff> {
    let path = dir.join(HANDOFF);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))
}

/// The server this agent will live on: the one the caller is already inside,
/// or amx's own.
pub fn server(state_root: &Path) -> Result<Server> {
    if let Some(inside) = std::env::var("TMUX").ok().filter(|v| !v.is_empty())
        && let Some(server) = Server::from_tmux_env(&inside)
    {
        return Ok(server);
    }

    let socket = std::env::var(SOCKET_ENV)
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| PRIVATE.to_string());
    Ok(Server::named(socket).with_conf(conf_file(state_root)?))
}

/// amx's own tmux conf, put where a server can be told to read it.
///
/// It is written out rather than pointed at inside the binary because tmux
/// reads a path; it is rewritten whenever it differs, so an amx that has been
/// upgraded does not leave the old one behind.
pub fn conf_file(state_root: &Path) -> Result<PathBuf> {
    let bundled = include_str!("../assets/amx.tmux.conf");
    let root = state_root.parent().unwrap_or(state_root);
    std::fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;

    let path = root.join("amx.tmux.conf");
    if std::fs::read_to_string(&path).is_ok_and(|existing| existing == bundled) {
        return Ok(path);
    }
    std::fs::write(&path, bundled).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Start a pane running `command`, where `placement` says it goes.
pub fn place(
    server: &Server,
    placement: Placement,
    id: &str,
    cwd: &Path,
    command: &[String],
    lock: &Path,
) -> Result<PaneId> {
    match placement {
        Placement::Background => background(server, id, cwd, command),
        Placement::Wall => wall(server, cwd, command, lock),
    }
}

/// A session of its own that nobody is attached to.
fn background(server: &Server, id: &str, cwd: &Path, command: &[String]) -> Result<PaneId> {
    let name = format!("amx-{id}");
    let mut args = vec![
        "new-session".to_string(),
        "-d".to_string(),
        "-s".to_string(),
        name,
        "-c".to_string(),
        cwd.to_string_lossy().into_owned(),
        "-P".to_string(),
        "-F".to_string(),
        "#{session_id} #{pane_id}".to_string(),
        "--".to_string(),
    ];
    args.extend(command.iter().cloned());

    let printed = server.run(&borrow(&args))?;
    let (session, pane) = printed
        .split_once(' ')
        .with_context(|| format!("new-session printed {printed:?}"))?;

    // Without this, tmux destroys the session the moment whoever looked in on
    // it detaches again.
    server.set_session_option(&SessionId::new(session)?, "destroy-unattached", "off")?;
    PaneId::new(pane)
}

/// Tiled into the wall, made if it is not there.
///
/// The lock is what keeps two `new`s a moment apart from building two walls:
/// finding and creating are two steps, and between them the answer to "is
/// there a wall?" is still no.
fn wall(server: &Server, cwd: &Path, command: &[String], lock: &Path) -> Result<PaneId> {
    let _held = hold(lock)?;

    let session = match session_for_wall(server)? {
        Some(session) => session,
        // No server, no session: one call makes the server, the session, the
        // wall and the agent's own pane.
        None => return open_wall_session(server, cwd, command),
    };

    match server.window_named(&session, WALL)? {
        Some(window) => {
            let mut args = vec![
                "split-window".to_string(),
                "-t".to_string(),
                window.as_str().to_string(),
                "-c".to_string(),
                cwd.to_string_lossy().into_owned(),
                "-P".to_string(),
                "-F".to_string(),
                "#{pane_id}".to_string(),
                "--".to_string(),
            ];
            args.extend(command.iter().cloned());
            let pane = PaneId::new(server.run(&borrow(&args))?)?;
            server.select_layout(&window, "tiled")?;
            Ok(pane)
        }
        None => {
            let mut args = vec![
                "new-window".to_string(),
                "-t".to_string(),
                session.as_str().to_string(),
                "-n".to_string(),
                WALL.to_string(),
                "-c".to_string(),
                cwd.to_string_lossy().into_owned(),
                "-P".to_string(),
                "-F".to_string(),
                "#{pane_id}".to_string(),
                "--".to_string(),
            ];
            args.extend(command.iter().cloned());
            PaneId::new(server.run(&borrow(&args))?)
        }
    }
}

/// The session the wall belongs in: the caller's own when there is one, else
/// amx's.
fn session_for_wall(server: &Server) -> Result<Option<SessionId>> {
    // Inside tmux, the caller's pane says which session it is in. A value read
    // on a pane that is gone answers emptily, which reads as "not inside".
    if let Ok(pane) = std::env::var("TMUX_PANE")
        && !pane.is_empty()
        && let Ok(session) = server.run(&["display-message", "-p", "-t", &pane, "#{session_id}"])
        && !session.is_empty()
    {
        return Ok(Some(SessionId::new(session)?));
    }

    // Otherwise amx's own session, if this server is running one.
    server.session_named(PRIVATE)
}

/// The first agent on amx's own server: everything at once.
fn open_wall_session(server: &Server, cwd: &Path, command: &[String]) -> Result<PaneId> {
    let mut args = vec![
        "new-session".to_string(),
        "-d".to_string(),
        "-s".to_string(),
        PRIVATE.to_string(),
        "-n".to_string(),
        WALL.to_string(),
        "-c".to_string(),
        cwd.to_string_lossy().into_owned(),
        "-P".to_string(),
        "-F".to_string(),
        "#{pane_id}".to_string(),
        "--".to_string(),
    ];
    args.extend(command.iter().cloned());
    PaneId::new(server.run(&borrow(&args))?)
}

/// The lock everything that could build a wall takes first — an agent
/// arriving, and a person opening the cockpit. It is one lock or it is no
/// lock at all, so the path lives here rather than at each caller.
pub fn wall_lock(root: &Path) -> PathBuf {
    root.join("wall.lock")
}

/// Hold a lock for as long as the returned value lives.
pub fn hold(path: &Path) -> Result<nix::fcntl::Flock<std::fs::File>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusive)
        .map_err(|(_, errno)| errno)
        .with_context(|| format!("locking {}", path.display()))
}

/// `amx _boot <id>`: become the agent.
///
/// The record is written by `new` once the pane exists, and this *is* that
/// pane, so the two cross. Waiting for it is what keeps the vendor's first
/// hooks from arriving before there is anywhere to put them.
pub fn boot(root: &Path, id: &str) -> Result<i32> {
    use std::os::unix::process::CommandExt;

    let dir = crate::paths::agent_dir_in(root, id)?;
    wait_for(&dir.join("meta.json"))?;
    let handoff = read_handoff(&dir)?;

    let Some(vendor) = handoff.command.first() else {
        bail!("the handoff for {id} names no command to run");
    };

    let mut command = std::process::Command::new("sh");
    command
        // `$0` is the vendor and `$@` its arguments, so the task never passes
        // through a shell's hands. What follows it records how it ended.
        .arg("-c")
        .arg(r#""$0" "$@"; "$AMX_BIN" _exit "$AMX_ID" $?"#)
        .arg(vendor)
        .args(&handoff.command[1..]);

    for (name, value) in &handoff.env {
        command.env(name, value);
    }
    command
        .env("AMX_BIN", std::env::current_exe()?)
        .env(crate::hook::ID_ENV, id);

    // Exec, so the pane's process is the vendor's and amx is not in its way.
    Err(command.exec()).context("starting the agent's command")
}

/// `_boot`, against the machine's own state directory.
pub fn boot_from_env(id: &str) -> Result<i32> {
    boot(&crate::paths::state_root()?, id)
}

/// Wait for a file somebody else is writing.
fn wait_for(path: &Path) -> Result<()> {
    let deadline = Instant::now() + RECORD_PATIENCE;
    while !path.exists() {
        if Instant::now() >= deadline {
            bail!("{} never arrived", path.display());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

/// The agents that are still going: their record says they have not finished,
/// and their pane is still there on the server it was recorded on.
pub fn live(root: &Path) -> Result<Vec<String>> {
    let mut live = Vec::new();
    for id in crate::store::list(root)? {
        let agent = Agent::open(root, &id)?;
        if agent.state()?.state.is_terminal() {
            continue;
        }
        let Ok(meta) = agent.meta() else { continue };
        if Server::from_socket(meta.socket).pane_alive(&meta.pane) {
            live.push(id);
        }
    }
    live.sort();
    Ok(live)
}

/// The record `new` writes once the pane exists.
pub fn record(root: &Path, meta: &Meta) -> Result<Agent> {
    Agent::create(root, meta)
}

fn borrow(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn spawn_the_agent_inherits_everything_but_the_pane_it_was_asked_from() {
        let snapshot = env_snapshot(vars(&[
            ("PATH", "/usr/bin"),
            ("ANTHROPIC_MODEL", "opus"),
            ("TMUX", "/tmp/tmux-1000/default,42,0"),
            ("TMUX_PANE", "%7"),
            ("PWD", "/srv/app"),
            ("OLDPWD", "/home/dev"),
        ]));

        assert_eq!(snapshot.get("PATH").unwrap(), "/usr/bin");
        assert_eq!(snapshot.get("ANTHROPIC_MODEL").unwrap(), "opus");
        for gone in NOT_INHERITED {
            assert!(
                !snapshot.contains_key(gone),
                "{gone} describes where the command was typed, not where the agent runs"
            );
        }
    }

    #[test]
    fn spawn_the_task_is_the_last_word_the_vendor_is_given() {
        let command = vendor_command(
            "claude --model opus",
            &Dials::default(),
            &["--session-id".to_string(), "abc-123".to_string()],
            "fix the login bug",
        );
        assert_eq!(
            command,
            [
                "claude",
                "--model",
                "opus",
                "--session-id",
                "abc-123",
                "fix the login bug"
            ]
        );
    }

    #[test]
    fn spawn_dials_become_flags_in_front_of_what_the_caller_passed_through() {
        let command = vendor_command(
            "claude",
            &Dials {
                model: "opus".to_string(),
                effort: "high".to_string(),
                ..Dials::default()
            },
            &["--session-id".to_string(), "abc-123".to_string()],
            "fix the login bug",
        );
        assert_eq!(
            command,
            [
                "claude",
                "--model",
                "opus",
                "--effort",
                "high",
                "--session-id",
                "abc-123",
                "fix the login bug"
            ],
            "the permission dial nobody turned sends no flag at all"
        );
    }

    #[test]
    fn spawn_dials_stand_down_from_a_flag_the_argv_already_carries() {
        // Whichever way the caller wrote it, and whether they wrote it in the
        // configured command or after the separator, claude is never handed
        // the same flag twice with the winner left to the vendor.
        let dials = Dials {
            model: "opus".to_string(),
            permission: "plan".to_string(),
            effort: "high".to_string(),
        };

        let command = vendor_command(
            "claude --effort max",
            &dials,
            &["--model=sonnet".to_string()],
            "fix the login bug",
        );
        assert_eq!(
            command,
            [
                "claude",
                "--effort",
                "max",
                "--permission-mode",
                "plan",
                "--model=sonnet",
                "fix the login bug"
            ]
        );
    }

    #[test]
    fn spawn_dials_send_nothing_to_a_vendor_the_table_has_no_entry_for() {
        // The stand-in every end to end test runs is unregistered, so a dial
        // set for it changes nothing about how it is launched.
        let command = vendor_command(
            "mock-claude",
            &Dials {
                model: "opus".to_string(),
                permission: "plan".to_string(),
                effort: "high".to_string(),
            },
            &[],
            "fix the login bug",
        );
        assert_eq!(command, ["mock-claude", "fix the login bug"]);
    }

    #[test]
    fn spawn_a_handoff_is_readable_by_nobody_else() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let handoff = Handoff {
            task: "fix the login bug".to_string(),
            command: vec!["claude".to_string(), "fix the login bug".to_string()],
            env: env_snapshot(vars(&[("ANTHROPIC_API_KEY", "not-a-real-key")])),
        };
        write_handoff(dir.path(), &handoff).unwrap();

        let mode = std::fs::metadata(dir.path().join(HANDOFF))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "somebody's environment is in it");
        assert_eq!(read_handoff(dir.path()).unwrap(), handoff);
    }

    #[test]
    fn spawn_the_conf_is_written_where_a_server_can_read_it() {
        let root = TempDir::new().unwrap();
        let state = root.path().join("agents");

        let path = conf_file(&state).unwrap();
        assert_eq!(path, root.path().join("amx.tmux.conf"));
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("history-limit")
        );

        // An amx that has been upgraded replaces what the last one left.
        std::fs::write(&path, "set -g mouse off\n").unwrap();
        let again = conf_file(&state).unwrap();
        assert!(
            std::fs::read_to_string(&again)
                .unwrap()
                .contains("history-limit")
        );
    }

    #[test]
    fn spawn_boot_gives_up_rather_than_waiting_for_a_record_that_is_not_coming() {
        let root = TempDir::new().unwrap();
        assert!(boot(root.path(), "../elsewhere").is_err(), "not an id");
    }
}
