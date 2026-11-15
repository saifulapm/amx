//! `amx adopt --pid` and `--session`: move a loose claude into an amx pane.
//!
//! No process can be moved into tmux, so the claude is ended and its
//! conversation resumed in a new pane, the way `amx resume` brings an agent
//! back. In order:
//!
//! 1. Read the claude's account dir from /proc/<pid>/environ and its flags from
//!    /proc/<pid>/cmdline.
//! 2. Send SIGTERM, and wait up to [`PATIENCE`] for the pid to end, checking
//!    procStart so a reused pid is not mistaken for it. Two live claudes on
//!    one session write one transcript, so nothing starts while it lives.
//! 3. Write a stopped record with a handoff, then resume it: idle, no
//!    worktree, in the claude's directory, on its session, with its flags and
//!    its account dir.
//!
//! Every step runs under the adopt lock, so two moves of one claude make one
//! record: the second finds the session held.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::cli::AdoptArgs;
use crate::config::Config;
use crate::loose::{self, CONFIG_ENV, Loose};
use crate::spawn::{self, Handoff};
use crate::store::{Agent, Event, Meta, Phase, now};
use crate::tmux::PaneId;
use crate::verbs::{adopt, ls, new, resume};
use crate::{exit, paths, registry};

/// How long the old claude has to end after SIGTERM.
const PATIENCE: Duration = Duration::from_secs(5);

/// What the record's event log says happened.
const MOVED: &str = "moved";

/// Run the move against the machine.
pub fn from_env(args: &AdoptArgs) -> Result<i32> {
    let root = paths::state_root()?;
    let env = spawn::env_snapshot(std::env::vars());
    let home = std::env::home_dir().context("no home directory")?;
    let mut out = std::io::stdout().lock();
    run(&root, &env, &home, crate::config::current(), args, &mut out)
}

/// The move, with everything it reads named.
pub fn run(
    root: &Path,
    env: &BTreeMap<String, String>,
    home: &Path,
    config: &Config,
    args: &AdoptArgs,
    out: &mut impl Write,
) -> Result<i32> {
    let held = adopt::hold(root)?;
    let planned = plan(root, env, home, config, args)?;
    carry_out(root, env, &planned)?;
    drop(held);
    writeln!(out, "moved to amx: amx attach {}", planned.id)?;
    Ok(exit::OK)
}

/// `amx adopt` typed as `!amx adopt` inside a claude outside tmux: move the
/// claude this command runs under, found by its session.
///
/// amx is then a child of the claude it ends, and dies with it. So only the
/// checks and the record happen here; ending the claude and starting its pane
/// are left to `amx _move`, started in a session of its own so that it
/// outlives this process.
pub fn self_from_env(args: &AdoptArgs, session: &str) -> Result<i32> {
    let root = paths::state_root()?;
    let env = spawn::env_snapshot(std::env::vars());
    let home = std::env::home_dir().context("no home directory")?;
    let args = AdoptArgs {
        session: Some(session.to_string()),
        // The claude is at its prompt, since its person typed this; claude may
        // still call itself busy while it runs the command.
        force: true,
        ..args.clone()
    };
    let planned = {
        let _held = adopt::hold(&root)?;
        plan(&root, &env, &home, crate::config::current(), &args)?
    };
    spawn::write_owned(&planned_file(&root, &planned.id)?, &planned)?;
    let started = std::process::Command::new(std::env::current_exe()?)
        .args([MOVE_VERB, &planned.id])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(e) = started {
        let _ = std::fs::remove_dir_all(paths::agent_dir_in(&root, &planned.id)?);
        return Err(anyhow::Error::new(e).context("starting the move"));
    }
    let mut out = std::io::stdout().lock();
    writeln!(out, "moved to amx: amx attach {}", planned.id)?;
    Ok(exit::OK)
}

/// The hidden verb that finishes a move [`self_from_env`] planned.
pub const MOVE_VERB: &str = "_move";

/// `amx _move <id>`: end the claude the record's plan names and start its
/// pane, then tell the claude's old terminal where it went.
pub fn finish_from_env(id: &str) -> Result<i32> {
    // In a session of its own, away from the claude's terminal and its signals.
    let _ = nix::unistd::setsid();
    let root = paths::state_root()?;
    let env = spawn::env_snapshot(std::env::vars());
    let file = planned_file(&root, id)?;
    let planned: Planned = serde_json::from_str(
        &std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?,
    )
    .with_context(|| format!("reading {}", file.display()))?;
    let _ = std::fs::remove_file(&file);
    let _held = adopt::hold(&root)?;
    let carried = carry_out(&root, &env, &planned);
    // The claude's terminal is the one place its person is looking.
    if let Some(tty) = &planned.tty
        && let Ok(mut terminal) = std::fs::OpenOptions::new().write(true).open(tty)
    {
        let _ = match &carried {
            Ok(()) => writeln!(terminal, "moved to amx: amx attach {id}"),
            Err(e) => writeln!(terminal, "amx: {e:#}"),
        };
    }
    carried.map(|()| exit::OK)
}

/// Where a planned move waits for `amx _move`.
fn planned_file(root: &Path, id: &str) -> Result<std::path::PathBuf> {
    Ok(paths::agent_dir_in(root, id)?.join(PLANNED))
}

/// The plan `amx _move` reads, written and read by the same amx a moment
/// apart.
const PLANNED: &str = "move.json";

/// A move whose record is written and whose claude still runs.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Planned {
    pub id: String,
    pub claude: Loose,
    /// The claude's `CLAUDE_CONFIG_DIR`, `None` for none.
    pub account: Option<String>,
    /// The claude's terminal, told where it went once it has gone.
    pub tty: Option<std::path::PathBuf>,
}

/// Check the claude may be moved, then write its stopped record and handoff.
///
/// Run under the adopt lock. The record names the claude's session at once,
/// so a second move of the same claude finds it held.
fn plan(
    root: &Path,
    env: &BTreeMap<String, String>,
    home: &Path,
    config: &Config,
    args: &AdoptArgs,
) -> Result<Planned> {
    // An agent's own claude would end with the one it moves, mid-command.
    if let Some(id) = env.get(crate::hook::ID_ENV)
        && Agent::open(root, id).is_ok()
    {
        bail!("refused: an amx agent cannot adopt another claude");
    }
    let mut dirs = loose::config_dirs(env, home, config);
    // A pid names its own account dir, which may be one amx does not scan.
    if let Some(theirs) = args.pid.and_then(account_of).map(std::path::PathBuf::from)
        && !dirs.contains(&theirs)
    {
        dirs.push(theirs);
    }
    let claude = named(&dirs, &ls::held(root)?, args)?;
    if !args.force {
        match claude.status.as_str() {
            "busy" => bail!(
                "refused: {} is mid-turn; --force adopts anyway, and the turn in progress is lost",
                claude.pid
            ),
            "shell" => bail!(
                "refused: {} has background work running; --force adopts anyway, and ends it",
                claude.pid
            ),
            _ => {}
        }
    }

    let account = account_of(claude.pid);
    let flags = flags_of(&cmdline_of(claude.pid));
    let tty = std::fs::read_link(format!("/proc/{}/fd/0", claude.pid))
        .ok()
        .filter(|it| it.starts_with("/dev/pts") || it.starts_with("/dev/tty"));
    let command = claude_command(config);
    let task = match &args.task {
        Some(task) => task.clone(),
        None => label(&claude.cwd),
    };
    let (id, claimed) = new::claim(root, args.name.as_deref(), &task)?;
    let meta = Meta {
        role: None,
        parent: None,
        depth: 0,
        id: id.clone(),
        task,
        agent: Some(command.join(" ")),
        model: None,
        effort: None,
        dir: claude.cwd.clone(),
        worktree: None,
        branch: None,
        base: None,
        socket: spawn::server()?.socket().clone(),
        // Replaced by the resume, which places the pane.
        pane: PaneId::new("%0")?,
        bg: false,
        session: Some(claude.session.clone()),
        transcript: None,
        created: now(),
    };
    if let Err(e) = written(root, &meta, &claude, command, flags) {
        let _ = std::fs::remove_dir_all(&claimed);
        return Err(e);
    }
    Ok(Planned {
        id,
        claude,
        account,
        tty,
    })
}

/// End the planned claude, then start its pane.
///
/// A claude that will not end takes its record with it: nothing was started.
/// A pane that will not start leaves the record failed, and says how to get
/// the conversation back by hand.
fn carry_out(root: &Path, env: &BTreeMap<String, String>, planned: &Planned) -> Result<()> {
    if let Err(e) = end(&planned.claude) {
        let _ = std::fs::remove_dir_all(paths::agent_dir_in(root, &planned.id)?);
        return Err(e);
    }
    let pinned = BTreeMap::from([(CONFIG_ENV.to_string(), planned.account.clone())]);
    if let Err(e) = resume::bring_back(root, &planned.id, None, env, &pinned) {
        if let Ok(agent) = Agent::open(root, &planned.id)
            && let Ok(writer) = agent.writer()
        {
            let _ = writer.update_state(|state| state.state = Phase::Failed);
        }
        return Err(e.context(recovery(&planned.claude)));
    }
    if !settles(root, &planned.id) {
        bail!(
            "the new claude in {} ended as it started; {}",
            planned.id,
            recovery(&planned.claude)
        );
    }
    Ok(())
}

/// How long a moved claude has to open its session before the move is taken
/// as made.
const SETTLE: Duration = Duration::from_secs(10);

/// Whether the new pane's claude came up: its record reaches a live phase
/// before it ends. One that is still starting after [`SETTLE`] counts as up.
fn settles(root: &Path, id: &str) -> bool {
    let deadline = Instant::now() + SETTLE;
    while Instant::now() < deadline {
        let phase = Agent::open(root, id)
            .and_then(|agent| agent.state())
            .map(|state| state.state);
        match phase {
            Ok(Phase::Idle | Phase::Working | Phase::Waiting) => return true,
            Ok(phase) if phase.is_terminal() => return false,
            _ => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    true
}

/// The claude `--pid` or `--session` names, or why it cannot be moved.
fn named(
    dirs: &[std::path::PathBuf],
    held: &std::collections::BTreeSet<String>,
    args: &AdoptArgs,
) -> Result<Loose> {
    let wanted = |it: &Loose| {
        Some(it.pid) == args.pid || Some(it.session.as_str()) == args.session.as_deref()
    };
    // The newest file for a session wins: a session resumed by hand after a
    // crash leaves the dead one's file behind.
    let mut files: Vec<Loose> = loose::files(dirs).into_iter().filter(wanted).collect();
    files.sort_by_key(|it| (it.runs(loose::claude_start), it.started));
    let Some(claude) = files.pop() else {
        match (args.pid, &args.session) {
            (Some(pid), _) => bail!(
                "refused: {pid} has no claude sessions file; --session <id> names its conversation"
            ),
            (_, Some(session)) => bail!("refused: no claude is running on session {session}"),
            _ => bail!("name the claude to move with --pid or --session"),
        }
    };
    if !claude.runs(loose::claude_start) {
        bail!("refused: {} is no longer running", claude.pid);
    }
    if held.contains(&claude.session) {
        bail!(
            "refused: {} is on a conversation an amx agent already holds",
            claude.pid
        );
    }
    Ok(claude)
}

/// Write the stopped record and the handoff its resume starts from.
fn written(
    root: &Path,
    meta: &Meta,
    claude: &Loose,
    command: Vec<String>,
    flags: Vec<String>,
) -> Result<Agent> {
    let agent = Agent::create(root, meta)?;
    // The handoff names no task: the conversation already has one.
    spawn::write_handoff(
        agent.dir(),
        &Handoff {
            task: String::new(),
            command: [command, flags].concat(),
        },
    )?;
    let writer = agent.writer()?;
    writer.append(&Event::new(
        MOVED,
        serde_json::json!({ "pid": claude.pid, "session": claude.session }),
    ))?;
    writer.update_state(|state| state.state = Phase::Stopped)?;
    drop(writer);
    Ok(agent)
}

/// How to get the conversation back by hand when the move fails after the
/// old claude has ended.
fn recovery(claude: &Loose) -> String {
    format!(
        "the claude was ended but its new pane did not start; continue it with \
         `cd {} && claude --resume {}`",
        claude.cwd.display(),
        claude.session
    )
}

/// SIGTERM the claude and wait for it to end.
fn end(claude: &Loose) -> Result<()> {
    // Checked again at the signal: `amx _move` signals a moment after the
    // plan, in another process, and the pid may have ended and been reused.
    if !claude.runs(loose::claude_start) {
        bail!("refused: {} is no longer running", claude.pid);
    }
    let pid = nix::unistd::Pid::from_raw(i32::try_from(claude.pid)?);
    nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM)
        .with_context(|| format!("ending {}", claude.pid))?;
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if loose::proc_start(claude.pid).as_deref() != Some(claude.proc_start.as_str()) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    bail!("{} did not end; nothing was started", claude.pid)
}

/// The account dir the claude runs under, `None` for none.
fn account_of(pid: u32) -> Option<String> {
    let environ = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    environ
        .split(|byte| *byte == 0)
        .filter_map(|pair| std::str::from_utf8(pair).ok())
        .find_map(|pair| {
            pair.strip_prefix(&format!("{CONFIG_ENV}="))
                .map(str::to_string)
        })
        .filter(|dir| !dir.is_empty())
}

/// The claude's command line, one word an argument.
fn cmdline_of(pid: u32) -> Vec<String> {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .unwrap_or_default()
        .split(|byte| *byte == 0)
        .filter(|word| !word.is_empty())
        .map(|word| String::from_utf8_lossy(word).into_owned())
        .collect()
}

/// claude's flags that take the next word as their value.
const TAKES_A_VALUE: &[&str] = &[
    "--model",
    "--permission-mode",
    "--effort",
    "--settings",
    "--setting-sources",
    "--add-dir",
    "--agent",
    "--agents",
    "--append-system-prompt",
    "--system-prompt",
    "--mcp-config",
    "--allowedTools",
    "--allowed-tools",
    "--disallowedTools",
    "--disallowed-tools",
    "--tools",
    "--fallback-model",
    "--name",
    "-n",
    "--plugin-dir",
    "--betas",
    "--permission-prompt-tool",
    "--max-budget-usd",
];

/// Flags naming a session, which the move replaces with its own resume.
const NAMES_A_SESSION: &[&str] = &["--resume", "-r", "--session-id"];

/// Flags that change which session opens, with no value.
const PICKS_A_SESSION: &[&str] = &["--continue", "-c", "--fork-session"];

/// The flags to start the claude with again, read from its command line.
///
/// The program comes first: the first word naming claude, so an interpreter
/// in front (`node cli.js`, `sh claude`) is skipped too. Flags are kept up to
/// the first word that is not one, which is the first prompt and is never
/// sent again. A flag amx does not know is kept alone, so a value it takes
/// reads as that prompt and ends the flags, which drops flags rather than
/// re-sending a prompt.
pub fn flags_of(cmdline: &[String]) -> Vec<String> {
    let program = cmdline
        .iter()
        .position(|word| word.contains("claude"))
        .unwrap_or(0);
    let mut words = cmdline.iter().skip(program + 1).peekable();
    let mut kept = Vec::new();
    while let Some(word) = words.next() {
        if !word.starts_with('-') || word == "--" {
            break;
        }
        let name = word.split('=').next().unwrap_or(word);
        let joined = word.contains('=');
        if NAMES_A_SESSION.contains(&name) {
            // `-r` alone opens a picker; a value is skipped only when present.
            if !joined && words.peek().is_some_and(|next| !next.starts_with('-')) {
                words.next();
            }
            continue;
        }
        if PICKS_A_SESSION.contains(&name) {
            continue;
        }
        kept.push(word.clone());
        if !joined
            && TAKES_A_VALUE.contains(&name)
            && let Some(value) = words.next()
        {
            kept.push(value.clone());
        }
    }
    kept
}

/// The configured agent command when it runs claude, else `claude`.
fn claude_command(config: &Config) -> Vec<String> {
    let configured: Vec<String> = config
        .agent
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let is_claude = configured
        .first()
        .and_then(|program| registry::read_as(program))
        .is_some_and(|vendor| vendor.name == "claude");
    match is_claude {
        true => configured,
        false => vec!["claude".to_string()],
    }
}

/// The row's name: `adopted <dir name>`.
fn label(dir: &Path) -> String {
    match dir.file_name() {
        Some(name) => format!("adopted {}", name.to_string_lossy()),
        None => "adopted claude".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split(' ').map(str::to_string).collect()
    }

    #[test]
    fn the_flags_are_kept_without_the_program_the_session_or_the_prompt() {
        assert_eq!(
            flags_of(&words(
                "/home/me/.local/bin/claude --model haiku --resume OLD say hi"
            )),
            ["--model", "haiku"]
        );
        assert_eq!(
            flags_of(&words(
                "/bin/sh /t/mock-claude --model=haiku -r OLD --verbose"
            )),
            ["--model=haiku", "--verbose"]
        );
        assert_eq!(
            flags_of(&words(
                "claude --allow-dangerously-skip-permissions I am using amx"
            )),
            ["--allow-dangerously-skip-permissions"]
        );
        assert_eq!(
            flags_of(&words(
                "claude -c --fork-session --session-id S -r --effort high"
            )),
            ["--effort", "high"]
        );
        assert_eq!(
            flags_of(&words("claude -- --model x")),
            Vec::<String>::new()
        );
        assert_eq!(flags_of(&[]), Vec::<String>::new());
    }

    #[test]
    fn an_unknown_flag_with_a_value_drops_what_follows_rather_than_resend_it() {
        assert_eq!(
            flags_of(&words("claude --mystery value --model haiku")),
            ["--mystery"]
        );
    }

    #[test]
    fn the_label_is_the_directory_name() {
        assert_eq!(label(Path::new("/work/app")), "adopted app");
        assert_eq!(label(Path::new("/")), "adopted claude");
    }
}
