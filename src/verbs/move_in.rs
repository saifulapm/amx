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
    let _held = adopt::hold(root)?;
    let mut dirs = loose::config_dirs(env, home, config);
    // A pid names its own account dir, which may be one amx does not scan.
    if let Some(theirs) = args.pid.and_then(account_of).map(std::path::PathBuf::from)
        && !dirs.contains(&theirs)
    {
        dirs.push(theirs);
    }
    let found = loose::find(&dirs, &ls::held(root)?, loose::proc_start);
    let Some(claude) = found.into_iter().find(|it| {
        Some(it.pid) == args.pid || Some(it.session.as_str()) == args.session.as_deref()
    }) else {
        match (args.pid, &args.session) {
            (Some(pid), _) => bail!("no claude outside amx has pid {pid}"),
            (_, Some(session)) => bail!("no claude outside amx is on session {session}"),
            _ => bail!("name the claude to move with --pid or --session"),
        }
    };

    let account = account_of(claude.pid);
    let flags = flags_of(&cmdline_of(claude.pid));
    end(&claude)?;

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
    let agent = match written(root, &meta, &claude, command, flags) {
        Ok(agent) => agent,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&claimed);
            return Err(e.context(recovery(&claude)));
        }
    };

    let pinned = BTreeMap::from([(CONFIG_ENV.to_string(), account)]);
    if let Err(e) = resume::bring_back(root, &id, None, env, &pinned) {
        if let Ok(writer) = agent.writer() {
            let _ = writer.update_state(|state| state.state = Phase::Failed);
        }
        return Err(e.context(recovery(&claude)));
    }
    writeln!(out, "moved to amx: amx attach {id}")?;
    Ok(exit::OK)
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
