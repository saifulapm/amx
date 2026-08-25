//! `amx fork` — start an agent on a copy of another one's conversation.
//!
//! A fork is a second agent, not a continuation: it gets an id, a record and a
//! pane of its own, and the only thing it takes from the agent it was made
//! from is the conversation. The vendor is what copies that — `--resume` names
//! the session and `--fork-session` says to branch it rather than carry it on —
//! so the recorded session id is the whole of what a fork needs, and an agent
//! that never announced one cannot be forked at all.
//!
//! It runs where the agent it copies ran. A conversation is about the files it
//! was held over, down to the ones no commit has yet, and a tree of its own
//! would be a copy talking about work that is not there. What amx never does is
//! write that tree down as the copy's: the record says which worktree amx cut
//! for an agent, `stop` reads it to decide what to remove, and a copy claiming
//! its origin's tree would be one `stop` away from taking the original's work
//! with it.
//!
//! The copy's log opens with the line naming what it is a copy of, written
//! before the pane exists and so before the vendor has said anything. Two
//! agents on one conversation are otherwise indistinguishable, and the question
//! somebody asks a week later is which came first.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::spawn::{self, Handoff};
use crate::store::{Agent, Event, Meta, now};
use crate::vendor::{Capability, Vendor};
use crate::{Severity, exit, ids, paths, said};

/// What amx records when it copies a conversation.
const FORKED: &str = "fork";

/// The vendor's flag for the session to open.
const RESUME: &str = "--resume";

/// The vendor's flag for copying that session rather than continuing it.
const FORK: &str = "--fork-session";

/// Every way of writing a flag that decides which session the vendor opens.
///
/// These are the words a fork replaces rather than carries. Everything else the
/// agent was started with is the agent's and goes with it: a directory it was
/// given access to is one the copy still needs.
const NAMES_A_SESSION: [&str; 3] = ["--session-id", RESUME, "-r"];

/// How many minted ids to try to claim before giving up.
const MAX_CLAIMS: usize = 8;

/// Run the verb against the machine.
pub fn from_env(config: &Config, id: &str, task: Option<&str>) -> Result<i32> {
    let root = paths::state_root()?;
    let env = spawn::env_snapshot(std::env::vars());
    let mut out = std::io::stdout().lock();
    let to_terminal = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let mut problems = std::io::stderr().lock();
    run(
        &root,
        config,
        id,
        task,
        &env,
        &mut out,
        &mut problems,
        to_terminal,
    )
}

/// The verb, with everything it reads named.
#[allow(clippy::too_many_arguments)]
pub fn run(
    root: &Path,
    config: &Config,
    id: &str,
    prompt: Option<&str>,
    env: &BTreeMap<String, String>,
    out: &mut impl Write,
    problems: &mut impl Write,
    to_terminal: bool,
) -> Result<i32> {
    let origin = Agent::open(root, id)?;
    let meta = origin.meta()?;

    // Everything that would stop the fork is asked before anything is made:
    // the session there is to copy, the directory to copy it in, the words the
    // copy will be launched with, and whether the vendor those words name can
    // be asked for a copy at all.
    let session = copied_session(&meta)?;
    if !meta.dir.is_dir() {
        bail!(
            "{} is gone, and it is where {} ran",
            meta.dir.display(),
            meta.id
        );
    }
    let recorded = spawn::read_handoff(origin.dir())
        .with_context(|| format!("reading what {} was started with", meta.id))?;
    if let Some(refusal) = cannot_branch(spawn::vendor_of(&recorded), &meta.id) {
        bail!(refusal);
    }

    // The cap counts agents that are still going, and a fork is another one.
    let live = spawn::live(root)?;
    if live.len() >= config.max_agents {
        writeln!(
            problems,
            "{}",
            said(
                Severity::Warned,
                &format!(
                    "amx fork: {} agents already running, and max_agents is {}",
                    live.len(),
                    config.max_agents
                ),
                to_terminal
            )
        )?;
        return Ok(exit::BLOCKED);
    }

    // What the copy is for is what it was given to do, and the task it was
    // copied from when it was given nothing: a row with no task on it says
    // nothing about itself, and this one is about the same work as the agent
    // it came from.
    let task = prompt.unwrap_or(&meta.task);
    let command = copying(&recorded, &session, prompt);
    let (copy, dir) = claim(root, task)?;

    // From here a failure leaves nothing behind, as in `new`: the directory is
    // this fork's own, so removing it can never take another agent's record.
    match start(root, &copy, &meta, &session, task, command, env) {
        Ok(()) => {
            writeln!(out, "{copy}")?;
            Ok(exit::OK)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

/// Put the copy in a pane of its own, on the conversation it was made from.
///
/// The order is `new`'s, and for `new`'s reasons: the handoff before the pane,
/// because the pane reads it; the record after it, because there is no pane id
/// to record until tmux has made one; and the pane waits for that record, so
/// the vendor's first hook always has somewhere to go. What the copy came from
/// is written before any of it, so that the first line of its log is the one
/// amx wrote rather than the first thing the vendor said.
fn start(
    root: &Path,
    id: &str,
    origin: &Meta,
    session: &str,
    task: &str,
    command: Vec<String>,
    env: &BTreeMap<String, String>,
) -> Result<()> {
    let dir = paths::agent_dir_in(root, id)?;
    let mut env = env.clone();
    env.insert(crate::hook::ID_ENV.to_string(), id.to_string());
    spawn::write_handoff(
        &dir,
        &Handoff {
            task: task.to_string(),
            command,
            env,
        },
    )?;
    names_its_origin(root, id, origin, session)?;

    let server = spawn::server()?;
    let boot = vec![
        std::env::current_exe()?.to_string_lossy().into_owned(),
        "_boot".to_string(),
        id.to_string(),
    ];
    let pane = spawn::place(&server, id, &origin.dir, &boot)?;

    spawn::record(
        root,
        &Meta {
            id: id.to_string(),
            task: task.to_string(),
            dir: origin.dir.clone(),
            // amx cut nothing for this agent. The tree it runs in belongs to
            // the agent it was copied from, and a copy that wrote that tree
            // down as its own would be one `amx stop` away from removing it.
            worktree: None,
            branch: None,
            base: None,
            socket: server.socket().clone(),
            pane,
            bg: false,
            // The vendor mints a session id of its own for a copy, and amx
            // hears it from the first hook the pane fires, exactly as it hears
            // an agent's first session.
            session: None,
            transcript: None,
            created: now(),
        },
    )?;
    Ok(())
}

/// Write down what the copy is a copy of, and which conversation it took.
///
/// On the copy's own record, because that is where somebody asking about the
/// copy is looking. The agent it came from has nothing to say about it: it may
/// be forked again tomorrow, or have been forgotten by then, and a record that
/// depends on another agent's still being there is a record that goes quiet.
fn names_its_origin(root: &Path, id: &str, origin: &Meta, session: &str) -> Result<()> {
    Agent::open(root, id)?.writer()?.append(&Event::new(
        FORKED,
        serde_json::json!({ "from": origin.id, "session": session }),
    ))
}

/// The vendor's argv for a copy of a session it already has.
///
/// The copy is launched with what the original was launched with, minus the two
/// things this command decides for itself.
///
/// The **task** goes: it was put to the session in its first turn, and the copy
/// has that turn already. Every **flag naming a session** goes with it, because
/// which session the vendor opens is this command's answer and not the recorded
/// command's — `--session-id` asks it to start one, and a `--resume` is what the
/// last resume of the original left behind. `--fork-session` goes too, so that a
/// copy of a copy asks for one fork rather than two.
fn copying(handoff: &Handoff, session: &str, prompt: Option<&str>) -> Vec<String> {
    let mut words = handoff.command.clone().into_iter().peekable();
    let mut command: Vec<String> = Vec::new();

    while let Some(word) = words.next() {
        // Only the last word is the task, which is where `new` put it.
        if words.peek().is_none() && word == handoff.task {
            break;
        }
        if word == FORK {
            continue;
        }
        let Some(value_is_a_word_of_its_own) = names_a_session(&word) else {
            command.push(word);
            continue;
        };
        // The value goes with the flag it belongs to. A word that begins with
        // `-` is never one: the vendor documents `--resume`'s value as
        // optional, and an optional value is not taken from a word that could
        // be a flag in its own right.
        if value_is_a_word_of_its_own && words.peek().is_some_and(|next| !next.starts_with('-')) {
            words.next();
        }
    }

    command.push(format!("{RESUME}={session}"));
    command.push(FORK.to_string());
    command.extend(prompt.map(str::to_string));
    command
}

/// Whether a word is a flag naming a session, and if so whether its value is
/// the word after it rather than joined on with `=`.
fn names_a_session(word: &str) -> Option<bool> {
    NAMES_A_SESSION.iter().find_map(|flag| {
        if word == *flag {
            return Some(true);
        }
        word.strip_prefix(flag)
            .is_some_and(|rest| rest.starts_with('='))
            .then_some(false)
    })
}

/// Why this vendor cannot be asked for a copy of a conversation, when it
/// cannot.
///
/// The two flags below are claude's, and a vendor with no equivalent would
/// meet them as arguments it does not know: a pane that dies on its first line
/// with the reason scrolling past, after an id and a directory have been spent
/// on it. Saying it here is the same answer, before anything is made and in
/// words that name what is missing.
///
/// A command amx has no entry for is not refused. amx has measured nothing
/// about it, and nothing measured is no reason to take away what somebody's
/// own wrapper command does today.
fn cannot_branch(vendor: Option<&Vendor>, id: &str) -> Option<String> {
    let vendor = vendor?;
    (!vendor.can(Capability::Fork)).then(|| {
        format!(
            "{id} runs {}, which cannot branch a conversation, so there is no \
             copy to ask it for. carry this one on with `amx resume {id}`, or \
             start a fresh agent with `amx new`",
            vendor.name
        )
    })
}

/// The session a copy is made from: the one the agent recorded, checked at the
/// moment it is about to become a word on a command line.
///
/// An agent with none is refused rather than started over. Without the session
/// there is no conversation to copy, and what a fork would become is a fresh
/// agent on somebody else's task — which is `amx new`, said plainly.
fn copied_session(meta: &Meta) -> Result<String> {
    let Some(session) = meta.session.as_deref() else {
        bail!(
            "no session was ever recorded for {}, so there is no conversation to copy. \
             start a fresh agent with `amx new`",
            meta.id
        );
    };
    if !is_session_id(session) {
        bail!(
            "the session recorded for {} is not a session id, so it will not be handed on",
            meta.id
        );
    }
    Ok(session.to_string())
}

/// Whether a recorded session id is one.
///
/// A word amx is about to hand the vendor as an argument, checked for being a
/// word and nothing else: an id that could read as a flag, or that carries
/// anything but the characters an id is made of, is not passed on.
fn is_session_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value.starts_with('-')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Claim an id for the copy by making its directory, which is how `new` claims
/// one: the mkdir is the uniqueness check, two spawns in flight can both
/// believe a name is free, and only one of them can make the directory.
fn claim(root: &Path, task: &str) -> Result<(String, PathBuf)> {
    for _ in 0..MAX_CLAIMS {
        let id = ids::generate(task, root)?;
        let dir = paths::agent_dir_in(root, &id)?;
        if make_dir(&dir)? {
            return Ok((id, dir));
        }
    }
    bail!(
        "no id for {task:?} could be claimed under {} after {MAX_CLAIMS} draws",
        root.display()
    )
}

/// The copy's own directory, which nobody else has any business reading.
///
/// Deliberately not recursive: making the directory is the uniqueness claim, so
/// one that is already there has to answer false rather than stand in for one
/// this fork made.
fn make_dir(dir: &Path) -> Result<bool> {
    use std::os::unix::fs::DirBuilderExt;
    match std::fs::DirBuilder::new().mode(paths::DIR_MODE).create(dir) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e).with_context(|| format!("creating {}", dir.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::{PaneId, Socket};
    use crate::vendor::second::SECOND;
    use tempfile::TempDir;

    fn handoff(command: &[&str], task: &str) -> Handoff {
        Handoff {
            task: task.to_string(),
            command: command.iter().map(|word| word.to_string()).collect(),
            env: BTreeMap::new(),
        }
    }

    fn meta(id: &str, session: Option<&str>) -> Meta {
        Meta {
            id: id.to_string(),
            task: "fix the login bug".to_string(),
            dir: PathBuf::from("/srv/app"),
            worktree: None,
            branch: None,
            base: None,
            socket: Socket::Name("amx".to_string()),
            pane: PaneId::new("%7").unwrap(),
            bg: false,
            session: session.map(str::to_string),
            transcript: None,
            created: now(),
        }
    }

    #[test]
    fn fork_asks_the_vendor_to_copy_the_session_the_agent_opened() {
        let started = handoff(
            &["claude", "--model", "opus", "fix the login bug"],
            "fix the login bug",
        );
        assert_eq!(
            copying(&started, "abc-123", None),
            [
                "claude",
                "--model",
                "opus",
                "--resume=abc-123",
                "--fork-session"
            ],
            "the flag and its value are one word: the value is optional, and a \
             separate one would be read as a flag of its own"
        );
    }

    #[test]
    fn fork_puts_a_task_of_its_own_where_a_prompt_goes() {
        // The task the original was given is not handed over again — the copy
        // is the conversation that answered it — and a new one goes last,
        // where `new` puts a prompt.
        let started = handoff(
            &["claude", "--model", "opus", "fix the login bug"],
            "fix the login bug",
        );
        assert_eq!(
            copying(&started, "abc-123", Some("now do it with sqlite")),
            [
                "claude",
                "--model",
                "opus",
                "--resume=abc-123",
                "--fork-session",
                "now do it with sqlite"
            ]
        );
    }

    #[test]
    fn fork_carries_everything_the_agent_was_started_with() {
        // The arguments are the agent's, not the first turn's: a directory it
        // was given access to is one the copy still needs.
        let started = handoff(
            &[
                "claude",
                "--model",
                "opus",
                "--add-dir",
                "/srv/data",
                "--verbose",
                "port the importer",
            ],
            "port the importer",
        );
        assert_eq!(
            copying(&started, "abc-123", None),
            [
                "claude",
                "--model",
                "opus",
                "--add-dir",
                "/srv/data",
                "--verbose",
                "--resume=abc-123",
                "--fork-session"
            ]
        );
    }

    #[test]
    fn fork_asks_for_one_session_and_forks_it_once() {
        // Whatever the recorded command already says about which session to
        // open is this command's answer to give: a resumed agent's command
        // carries the `--resume` its last resume wrote, and a copy's carries
        // the `--fork-session` that made it.
        for written in [
            &["claude", "--add-dir", "/srv/data", "--resume=old"][..],
            &["claude", "--add-dir", "/srv/data", "--resume", "old"],
            &["claude", "--add-dir", "/srv/data", "-r", "old"],
            &["claude", "--session-id", "old", "--add-dir", "/srv/data"],
            &[
                "claude",
                "--add-dir",
                "/srv/data",
                "--resume=old",
                "--fork-session",
            ],
        ] {
            let started = handoff(written, "go");
            assert_eq!(
                copying(&started, "def-456", None),
                [
                    "claude",
                    "--add-dir",
                    "/srv/data",
                    "--resume=def-456",
                    "--fork-session"
                ],
                "{written:?}"
            );
        }

        // The value is optional, so the word after one is only its value when
        // it could be: a flag after `--resume` is a flag, and it stays.
        let started = handoff(&["claude", "--resume", "--verbose", "go"], "go");
        assert_eq!(
            copying(&started, "def-456", None),
            ["claude", "--verbose", "--resume=def-456", "--fork-session"]
        );
    }

    #[test]
    fn fork_refuses_a_vendor_that_cannot_branch_a_conversation() {
        // The refusal names the vendor, the agent and what is missing, because
        // what is missing is not something trying again would fix.
        let said = cannot_branch(Some(&SECOND), "fix-login-a1b").expect("it cannot fork");
        assert!(said.contains("fix-login-a1b"), "{said}");
        assert!(said.contains(SECOND.name), "{said}");
        assert!(said.contains("cannot branch a conversation"), "{said}");
        assert!(said.contains("amx resume fix-login-a1b"), "{said}");

        assert_eq!(
            cannot_branch(crate::registry::entry("claude"), "fix-login-a1b"),
            None,
            "the vendor amx was written against can"
        );
        assert_eq!(
            cannot_branch(None, "fix-login-a1b"),
            None,
            "and a command amx has no entry for is not amx's to refuse: \
             nothing measured is not a measurement"
        );
    }

    #[test]
    fn fork_hands_on_a_session_id_and_nothing_else() {
        assert!(is_session_id("6f1c9f4e-0d5b-4a51-9f6e-2b1f0c3d4e5a"));
        assert!(is_session_id("abc_123"));

        assert!(!is_session_id(""));
        assert!(!is_session_id("--dangerously-skip-permissions"));
        assert!(!is_session_id("abc 123"));
        assert!(!is_session_id("$(rm -rf /)"));
        assert!(!is_session_id("../../elsewhere"));
        assert!(!is_session_id(&"a".repeat(65)));
    }

    #[test]
    fn fork_refuses_an_agent_that_never_recorded_a_session_and_says_why() {
        let said = format!(
            "{:#}",
            copied_session(&meta("fix-login-a1b", None)).unwrap_err()
        );
        assert!(said.contains("fix-login-a1b"), "{said}");
        assert!(said.contains("no conversation to copy"), "{said}");
        assert!(said.contains("amx new"), "{said}");

        let said = format!(
            "{:#}",
            copied_session(&meta(
                "fix-login-a1b",
                Some("--dangerously-skip-permissions")
            ))
            .unwrap_err()
        );
        assert!(said.contains("not a session id"), "{said}");

        assert_eq!(
            copied_session(&meta("fix-login-a1b", Some("abc-123"))).unwrap(),
            "abc-123"
        );
    }

    /// A state root with one agent's record in it.
    fn a_record(session: Option<&str>, dir: &Path) -> (TempDir, Meta) {
        let root = TempDir::new().unwrap();
        let meta = Meta {
            dir: dir.to_path_buf(),
            ..meta("fix-login-a1b", session)
        };
        Agent::create(root.path(), &meta).unwrap();
        (root, meta)
    }

    /// The verb, with nowhere for its output to go but a buffer.
    fn fork(root: &Path, id: &str) -> Result<(i32, String, String)> {
        forked(root, id, &Config::default(), false)
    }

    /// The same, with the config and the kind of stderr named.
    fn forked(
        root: &Path,
        id: &str,
        config: &Config,
        to_terminal: bool,
    ) -> Result<(i32, String, String)> {
        let (mut out, mut problems) = (Vec::new(), Vec::new());
        let code = run(
            root,
            config,
            id,
            None,
            &BTreeMap::new(),
            &mut out,
            &mut problems,
            to_terminal,
        )?;
        Ok((
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(problems).unwrap(),
        ))
    }

    #[test]
    fn fork_of_an_agent_amx_has_no_record_of_is_refused() {
        let root = TempDir::new().unwrap();
        for typed in ["never-made-abc", "../elsewhere"] {
            let said = format!("{:#}", fork(root.path(), typed).unwrap_err());
            assert!(said.contains("no agent"), "{said}");
        }
    }

    #[test]
    fn fork_says_nothing_is_made_when_there_is_no_session_to_copy() {
        // The refusal comes before an id is minted or a pane is opened, so a
        // fork that cannot happen leaves the state root as it found it.
        let here = TempDir::new().unwrap();
        let (root, _) = a_record(None, here.path());

        let said = format!("{:#}", fork(root.path(), "fix-login-a1b").unwrap_err());
        assert!(said.contains("no conversation to copy"), "{said}");
        assert_eq!(
            crate::store::list(root.path()).unwrap(),
            ["fix-login-a1b"],
            "and nothing was made for the copy"
        );
    }

    #[test]
    fn fork_opens_the_copys_log_with_the_agent_it_was_copied_from() {
        // Two agents on one conversation are otherwise indistinguishable, and
        // the question somebody asks a week later is which came first.
        let root = TempDir::new().unwrap();
        let (copy, dir) = claim(root.path(), "fix the login bug").unwrap();
        assert!(dir.is_dir(), "the claim is the directory");

        names_its_origin(root.path(), &copy, &meta("fix-login-a1b", None), "abc-123").unwrap();

        let written = Agent::open(root.path(), &copy).unwrap().events().unwrap();
        assert_eq!(written.len(), 1, "{written:?}");
        assert_eq!(written[0].kind, FORKED);
        assert_eq!(written[0].payload["from"], "fix-login-a1b");
        assert_eq!(written[0].payload["session"], "abc-123");
    }

    #[test]
    fn fork_refuses_at_the_cap_in_yellow_on_a_terminal_and_plain_down_a_pipe() {
        // The cap is a refusal and not a failure: nothing went wrong, and amx
        // is saying what it will not do. Yellow says which of the two it is.
        let here = TempDir::new().unwrap();
        let (root, _) = a_record(Some("abc-123"), here.path());
        let origin = Agent::open(root.path(), "fix-login-a1b").unwrap();
        spawn::write_handoff(
            origin.dir(),
            &handoff(&["claude", "fix the login bug"], "fix the login bug"),
        )
        .unwrap();
        let full = Config {
            max_agents: 0,
            ..Config::default()
        };

        let (code, _, plain) = forked(root.path(), "fix-login-a1b", &full, false).unwrap();
        assert_eq!(code, exit::BLOCKED);
        assert!(plain.starts_with("amx fork: "), "{plain:?}");
        assert!(!plain.contains('\u{1b}'), "{plain:?}");

        let (_, _, painted) = forked(root.path(), "fix-login-a1b", &full, true).unwrap();
        assert!(painted.starts_with("\u{1b}[33mamx fork: "), "{painted:?}");
        assert!(painted.trim_end().ends_with("\u{1b}[39m"), "{painted:?}");
    }

    #[test]
    fn fork_says_so_when_the_directory_the_conversation_was_held_in_is_gone() {
        // A copy runs where the original ran, so a tree `stop` removed is a
        // fork that cannot start. Saying which directory beats tmux's own
        // account of a session it could not open.
        let (root, meta) = a_record(Some("abc-123"), Path::new("/nowhere/at/all"));

        let said = format!("{:#}", fork(root.path(), "fix-login-a1b").unwrap_err());
        assert!(said.contains(&meta.dir.display().to_string()), "{said}");
        assert!(said.contains("is gone"), "{said}");
    }
}
