//! `amx fork` — start an agent on a copy of another one's conversation.
//!
//! A fork is a second agent, not a continuation: it gets an id, a record and a
//! pane of its own, and the only thing it takes from the agent it was made
//! from is the conversation. The vendor is what copies that — `--resume` names
//! the session and `--fork-session` says to branch it rather than carry it on —
//! so the recorded session id is the whole of what a fork needs, and an agent
//! that never announced one cannot be forked at all.
//!
//! The copy needs a session of its own as well as the one it took, and where
//! the vendor declares a flag to open one under, that session is the id amx
//! minted for the copy: a vendor that reports nothing has no other way to be
//! told which session the copy is, and a copy amx cannot name is one nobody
//! can resume or fork again. Where the vendor declares no such flag, the
//! record waits for the copy's own first report, which is claude's way.
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
use crate::vendor::{Capability, ForkSpec, Resume, Vendor};
use crate::verbs::resume;
use crate::{Severity, exit, ids, paths, said};

/// What amx records when it copies a conversation.
const FORKED: &str = "fork";

/// How many minted ids to try to claim before giving up.
const MAX_CLAIMS: usize = 8;

/// Run the verb against the machine.
///
/// The config the caller holds is the person's file, and nothing here reads
/// it: a copy runs where the agent it copies ran, and the cap it answers to is
/// that project's.
pub fn from_env(_config: &Config, id: &str, task: Option<&str>) -> Result<i32> {
    let root = paths::state_root()?;
    let env = spawn::env_snapshot(std::env::vars());
    let mut out = std::io::stdout().lock();
    let to_terminal = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let mut problems = std::io::stderr().lock();
    run(&root, id, task, &env, &mut out, &mut problems, to_terminal)
}

/// The verb, with everything it reads named.
#[allow(clippy::too_many_arguments)]
pub fn run(
    root: &Path,
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
    // It is the cap of the project the copy will run in, which is the one the
    // agent it copies ran in: the config the caller holds is the person's file
    // and says nothing about that project.
    let (theirs, _) = crate::config::for_dir_in(&meta.dir, root);
    let project = spawn::project_of(&meta.dir);
    // What the copy is for is what it was given to do, and the task it was
    // copied from when it was given nothing: a row with no task on it says
    // nothing about itself, and this one is about the same work as the agent
    // it came from.
    let task = prompt.unwrap_or(&meta.task);
    // Counted and claimed in one step, as in `new`.
    let taken = spawn::take_a_place(root, &project, theirs.max_agents, theirs.max_total, || {
        let (copy, dir) = claim(root, task)?;
        Ok(((copy, dir.clone()), dir))
    })?;
    let ((copy, dir), _place) = match taken {
        Ok(taken) => taken,
        Err(full) => {
            writeln!(
                problems,
                "{}",
                said(Severity::Warned, &format!("amx fork: {full}"), to_terminal)
            )?;
            return Ok(exit::BLOCKED);
        }
    };
    // The id is minted before the argv is built, because a vendor that
    // declares a start flag is asked to open the copy under it.
    let command = copying(&recorded, &session, &copy, prompt);
    let opened = opened_under(&recorded, &copy);
    let launched = launched_with(&recorded);

    // From here a failure leaves nothing behind, as in `new`: the directory is
    // this fork's own, so removing it can never take another agent's record.
    match start(
        root, &copy, &meta, &session, task, command, opened, launched, env,
    ) {
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
#[allow(clippy::too_many_arguments)]
fn start(
    root: &Path,
    id: &str,
    origin: &Meta,
    session: &str,
    task: &str,
    command: Vec<String>,
    opened: Option<String>,
    launched: Option<String>,
    env: &BTreeMap<String, String>,
) -> Result<()> {
    let dir = paths::agent_dir_in(root, id)?;
    let mut env = env.clone();
    // What the file says this harness runs with, from the project the copy
    // will run in, which is the one the agent it copies ran in — the door the
    // cap is read through. Then amx's own id over the top, as in `new`: a
    // table that set the id would have the copy reporting as somebody else.
    if let Some(agent) = &origin.agent {
        spawn::harness_env(
            &mut env,
            &crate::config::for_dir_in(&origin.dir, root).0,
            agent,
        );
    }
    env.insert(crate::hook::ID_ENV.to_string(), id.to_string());
    spawn::write_boot_env(&dir, &env)?;
    spawn::write_handoff(
        &dir,
        &Handoff {
            task: task.to_string(),
            command,
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

    // A pane with no record is a copy nothing can find or stop, waiting on a
    // record that is never coming: it goes with the record that failed.
    let recorded = spawn::record(
        root,
        &Meta {
            role: None,
            parent: None,
            depth: 0,
            id: id.to_string(),
            task: task.to_string(),
            agent: launched,
            // The copy runs the origin's conversation, launched the origin's
            // way: same vendor, same model, same effort. Nothing on a fork's
            // command line turns a dial, so nothing here can differ.
            model: origin.model.clone(),
            effort: origin.effort.clone(),
            dir: origin.dir.clone(),
            // amx cut nothing for this agent. The tree it runs in belongs to
            // the agent it was copied from, and a copy that wrote that tree
            // down as its own would be one `amx stop` away from removing it.
            worktree: None,
            branch: None,
            base: None,
            socket: server.socket().clone(),
            pane: pane.clone(),
            bg: false,
            session: opened,
            transcript: None,
            created: now(),
        },
    );
    if recorded.is_err() {
        let _ = server.kill_pane(&pane);
    }
    recorded.map(|_| ())
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
/// command's — a start flag asks the vendor to open one, and a `--resume` is
/// what the last resume of the original left behind. `--fork-session` goes too,
/// so that a copy of a copy asks for one fork rather than two.
///
/// What this command answers with is both halves: the session the copy is
/// branched from, and `copy` — the id amx minted for it — as the session the
/// copy itself opens, for a vendor that declares a flag to ask for one.
fn copying(handoff: &Handoff, session: &str, copy: &str, prompt: Option<&str>) -> Vec<String> {
    build_copy(handoff, session, copy, prompt, spawn::vendor_of(handoff))
}

/// What [`Meta::session`] is recorded as for the copy: the id amx minted for
/// it, the moment a vendor that declares a start flag is asked to open it
/// under that id, rather than left `None` for a report that vendor never
/// sends.
///
/// `None` from a vendor that declares no start flag, which is claude: its own
/// Started hook names the session the copy opened, and the copy's record waits
/// for it exactly as an agent's does.
///
/// The same question [`build_copy`] answers while building the argv, asked of
/// the same spelling, for the caller writing the record rather than the
/// command.
fn opened_under(handoff: &Handoff, copy: &str) -> Option<String> {
    resume::spelling(spawn::vendor_of(handoff))
        .start
        .map(|_| copy.to_string())
}

/// What [`Meta::agent`] is recorded as for the copy: the word the agent it was
/// copied from was launched with, which is the word the copy is launched with
/// too.
///
/// Read off the handoff rather than off the original's record, because the
/// handoff is what a fork already stands on — an agent whose words are gone
/// cannot be copied at all — while the record says nothing about the vendor on
/// every agent started before amx kept it.
fn launched_with(handoff: &Handoff) -> Option<String> {
    handoff.command.first().cloned()
}

/// [`copying`], with the vendor passed in rather than looked up, so a shape
/// the table has never seen can be proved out here too. `None` is a command
/// amx has no entry for, spelled the way [`spelling`] says.
///
/// A vendor branches one of three ways. [`ForkSpec::Marker`] rides beside the
/// resume flag: the copy opens through `resume` exactly as a continuation
/// does, and the marker is what turns that into a branch rather than a
/// carry-on. [`ForkSpec::Origin`] is the flag itself: it carries the session
/// to copy, and `resume` is not written at all, because this vendor's copy
/// is not asking to continue anything. [`ForkSpec::Subcommand`] is the same
/// as an origin, spelled as a word right after the program.
///
/// Whichever way, a vendor that declares a start flag is handed `copy` beside it:
/// the copy is a second agent, and a vendor that reports nothing has no other
/// way to be told which session that agent is.
fn build_copy(
    handoff: &Handoff,
    session: &str,
    copy: &str,
    prompt: Option<&str>,
    vendor: Option<&Vendor>,
) -> Vec<String> {
    let spec = &resume::spelling(vendor);
    let ends_options = vendor.and_then(|vendor| vendor.ends_options);
    let fork = spec
        .fork
        .expect("a copy is only asked of a vendor that declares how it branches");

    let mut command = resume::without_session(handoff, vendor, spec);
    if let ForkSpec::Marker(marker) = fork {
        // A bare word, never carrying a value of its own: a copy of a
        // copy drops it here rather than asking the vendor to branch
        // twice.
        command.retain(|word| word != marker);
    }

    match fork {
        ForkSpec::Marker(marker) => {
            let resume = spec.resume_args(session);
            match spec.resume {
                Resume::Subcommand(_) => drop(command.splice(1..1, resume)),
                Resume::Flag { .. } => command.extend(resume),
            }
            command.push(marker.to_string());
        }
        ForkSpec::Origin(flag) => push_flag(&mut command, flag, spec.joined(), session),
        ForkSpec::Subcommand(word) => {
            drop(command.splice(1..1, [word.to_string(), session.to_string()]));
        }
    }
    if let Some(start) = spec.start {
        push_flag(&mut command, start, spec.joined(), copy);
    }
    if let Some(prompt) = prompt {
        command.extend(ends_options.map(str::to_string));
        command.push(spawn::as_words(vendor, prompt));
    }
    command
}

/// A flag and its value, joined with `=` or as two words, whichever the
/// vendor's own spelling says.
fn push_flag(command: &mut Vec<String>, flag: &str, joined: bool, value: &str) {
    if joined {
        command.push(format!("{flag}={value}"));
    } else {
        command.push(flag.to_string());
        command.push(value.to_string());
    }
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
    if !vendor.can(Capability::Fork) {
        return Some(format!(
            "{id} runs {}, which cannot branch a conversation, so there is no \
             copy to ask it for. carry this one on with `amx resume {id}`, or \
             start a fresh agent with `amx new`",
            vendor.name
        ));
    }
    // A capability with no spelling to answer it: refused the same way as an
    // absent capability, because there is just as little here to ask for a
    // copy with.
    vendor.session.is_none().then(|| {
        format!(
            "{id} runs {}, which names no session vocabulary, so there is no \
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
    if !resume::is_session_id(session) {
        bail!(
            "the session recorded for {} is not a session id, so it will not be handed on",
            meta.id
        );
    }
    Ok(session.to_string())
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
    use crate::vendor::SessionSpec;
    use crate::vendor::second::{BRANCHING, SECOND};
    use tempfile::TempDir;

    fn handoff(command: &[&str], task: &str) -> Handoff {
        Handoff {
            task: task.to_string(),
            command: command.iter().map(|word| word.to_string()).collect(),
        }
    }

    fn meta(id: &str, session: Option<&str>) -> Meta {
        Meta {
            role: None,
            parent: None,
            depth: 0,
            id: id.to_string(),
            task: "fix the login bug".to_string(),
            agent: None,
            model: None,
            effort: None,
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
            copying(&started, "abc-123", "port-it-b2c", None),
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
    fn fork_records_the_vendor_the_agent_it_copied_was_launched_with() {
        // The copy is launched with the original's own words, so what runs it
        // is the first of them — and the original's record cannot answer for
        // an agent started before amx kept the vendor on it.
        let started = handoff(
            &["claude", "--model", "opus", "fix the login bug"],
            "fix the login bug",
        );
        assert_eq!(launched_with(&started).as_deref(), Some("claude"));
        assert_eq!(launched_with(&handoff(&[], "")), None);
    }

    #[test]
    fn fork_ends_pis_options_before_a_prompt_and_drops_the_old_end() {
        // The original's task goes with the `--` in front of it, spaced or
        // not, and a prompt of the copy's own goes behind a `--` of its own,
        // with a space in front of its `@`.
        let started = handoff(
            &["pi", "--session-id", "abc-123", "--", " @alice asked"],
            "@alice asked",
        );
        assert_eq!(
            copying(&started, "abc-123", "port-it-b2c", None),
            ["pi", "--fork", "abc-123", "--session-id", "port-it-b2c"]
        );
        assert_eq!(
            copying(&started, "abc-123", "port-it-b2c", Some("@bob too")),
            [
                "pi",
                "--fork",
                "abc-123",
                "--session-id",
                "port-it-b2c",
                "--",
                " @bob too"
            ]
        );
    }

    #[test]
    fn fork_carries_a_prompt_on_the_prompt_flag_and_drops_the_old_one() {
        // A vendor that takes a message only on a flag was handed its task as
        // one `--say=` word, with a popup's space after it. The copy drops
        // that word and gets its own prompt the same way.
        let saying = Vendor {
            prompt_flag: Some("--say"),
            popups: &['#'],
            ends_options: None,
            ..BRANCHING
        };
        let started = handoff(&["second", "--alone", "--say=look at #3 "], "look at #3");
        assert_eq!(
            build_copy(&started, "abc-123", "port-it-b2c", None, Some(&saying)),
            ["second", "fork", "abc-123", "--alone"]
        );
        assert_eq!(
            build_copy(
                &started,
                "abc-123",
                "port-it-b2c",
                Some("-v on #4"),
                Some(&saying)
            ),
            ["second", "fork", "abc-123", "--alone", "--say=-v on #4 "]
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
            copying(
                &started,
                "abc-123",
                "port-it-b2c",
                Some("now do it with sqlite")
            ),
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
    fn fork_drops_a_task_a_role_put_its_brief_in_front_of() {
        // `new` hands the vendor `brief\n\ntask` as one word and records the
        // task alone, so the copy must not send the old task again.
        let started = handoff(
            &["claude", "You are a scout.\n\nfix the login bug"],
            "fix the login bug",
        );
        assert_eq!(
            copying(&started, "abc-123", "port-it-b2c", None),
            ["claude", "--resume=abc-123", "--fork-session"]
        );

        let started = handoff(
            &[
                "pi",
                "--session-id",
                "abc-123",
                "--",
                "You are a scout.\n\nfix the login bug",
            ],
            "fix the login bug",
        );
        assert_eq!(
            copying(&started, "abc-123", "port-it-b2c", None),
            ["pi", "--fork", "abc-123", "--session-id", "port-it-b2c"]
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
            copying(&started, "abc-123", "port-it-b2c", None),
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
                copying(&started, "def-456", "port-it-b2c", None),
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
            copying(&started, "def-456", "port-it-b2c", None),
            ["claude", "--verbose", "--resume=def-456", "--fork-session"]
        );
    }

    #[test]
    fn fork_answers_a_vendor_that_branches_by_naming_the_origin() {
        // The other shape ForkSpec offers: the flag itself carries the
        // session to copy, and `resume` is never written, because this
        // vendor's copy is not asking to continue anything. This one declares
        // no start flag either, so the minted id is nowhere in the argv.
        let spec = SessionSpec {
            start: None,
            resume: Resume::Flag {
                flag: "--resume",
                joined: true,
            },
            conflicts: &["--session-id"],
            fork: Some(ForkSpec::Origin("--branch-from")),
        };
        let vendor = Vendor {
            session: Some(spec),
            ..SECOND
        };
        let started = handoff(&["pi", "--model", "big", "go"], "go");
        assert_eq!(
            build_copy(&started, "abc-123", "port-it-b2c", None, Some(&vendor)),
            ["pi", "--model", "big", "--branch-from=abc-123"]
        );

        // A copy of a copy asks for one origin, not two.
        let started = handoff(&["pi", "--branch-from=old", "go"], "go");
        assert_eq!(
            build_copy(&started, "def-456", "port-it-b2c", None, Some(&vendor)),
            ["pi", "--branch-from=def-456"]
        );
    }

    #[test]
    fn fork_writes_a_subcommand_resume_right_after_the_program() {
        // A vendor that resumes with a subcommand branches the same way: the
        // program, the word, the origin's id, then the marker beside them.
        let spec = SessionSpec {
            start: None,
            resume: Resume::Subcommand("resume"),
            conflicts: &[],
            fork: Some(ForkSpec::Marker("--fork")),
        };
        let vendor = Vendor {
            session: Some(spec),
            ..SECOND
        };
        let started = handoff(&["codex", "resume", "old", "--model", "big", "go"], "go");
        assert_eq!(
            build_copy(
                &started,
                "abc-123",
                "port-it-b2c",
                Some("next"),
                Some(&vendor)
            ),
            [
                "codex", "resume", "abc-123", "--model", "big", "--fork", "next"
            ]
        );
    }

    #[test]
    fn fork_writes_a_subcommand_fork_right_after_the_program_and_no_resume() {
        // The word and the origin's id open the argv, the prompt goes behind
        // the vendor's end of options, and nothing the recorded command said
        // about which session it opened survives: not the task's own `--`,
        // not a resume's words, not an earlier fork's.
        let branching = Some(&BRANCHING);
        for written in [
            &["second", "-m", "large", "--", "go"][..],
            &["second", "again", "old", "-m", "large", "--", "go"],
            &["second", "fork", "old", "-m", "large", "--", "go"],
            &["second", "-m", "large", "--open", "old", "--", "go"],
        ] {
            let started = handoff(written, "go");
            assert_eq!(
                build_copy(&started, "abc-123", "port-it-b2c", Some("next"), branching),
                ["second", "fork", "abc-123", "-m", "large", "--", "next"],
                "{written:?}"
            );
            assert_eq!(
                build_copy(&started, "abc-123", "port-it-b2c", None, branching),
                ["second", "fork", "abc-123", "-m", "large"],
                "{written:?}"
            );
        }
        // Only right after the program is it the fork's word.
        let started = handoff(&["second", "-m", "fork", "--", "go"], "go");
        assert_eq!(
            build_copy(&started, "abc-123", "port-it-b2c", None, branching),
            ["second", "fork", "abc-123", "-m", "fork"]
        );
    }

    #[test]
    fn fork_opens_the_copy_under_the_id_amx_minted_for_it() {
        // pi's own spelling, read off the table: the flag naming the session
        // to branch carries the origin, and the start flag beside it carries
        // the copy's own id. Without that id the copy answers to nothing amx
        // chose, and a vendor with no hooks never reports the one it opened.
        let pi = crate::registry::entry("pi");
        let started = handoff(
            &["pi", "--model", "big", "--session-id", "abc-123", "go"],
            "go",
        );
        assert_eq!(
            build_copy(&started, "abc-123", "port-it-b2c", None, pi),
            [
                "pi",
                "--model",
                "big",
                "--fork",
                "abc-123",
                "--session-id",
                "port-it-b2c"
            ]
        );
        assert_eq!(
            opened_under(&started, "port-it-b2c"),
            Some("port-it-b2c".to_string()),
            "and the record says the same id the argv asked for"
        );

        // A copy of a copy branches from the copy's own session, under an id
        // of its own again.
        let started = handoff(
            &[
                "pi",
                "--fork",
                "abc-123",
                "--session-id",
                "port-it-b2c",
                "go",
            ],
            "go",
        );
        assert_eq!(
            build_copy(&started, "port-it-b2c", "redo-it-c3d", None, pi),
            ["pi", "--fork", "port-it-b2c", "--session-id", "redo-it-c3d"]
        );
    }

    #[test]
    fn fork_asks_for_no_id_from_a_vendor_that_reports_the_one_it_opened() {
        // claude declares no start flag: its own Started hook names the
        // session the copy opened, and the id it wants there is not the one
        // amx mints. The argv is what it always was, and the record waits for
        // that hook exactly as it did.
        let started = handoff(&["claude", "--model", "opus", "go"], "go");
        assert_eq!(
            copying(&started, "abc-123", "port-it-b2c", None),
            [
                "claude",
                "--model",
                "opus",
                "--resume=abc-123",
                "--fork-session"
            ]
        );
        assert_eq!(opened_under(&started, "port-it-b2c"), None);
    }

    #[test]
    fn fork_records_the_minted_id_only_where_the_copy_opens_under_it() {
        assert_eq!(
            opened_under(&handoff(&["pi", "go"], "go"), "port-it-b2c"),
            Some("port-it-b2c".to_string())
        );
        assert_eq!(
            opened_under(&handoff(&["claude", "go"], "go"), "port-it-b2c"),
            None,
            "no start flag was offered, so nothing was minted to record"
        );
        assert_eq!(
            opened_under(&handoff(&["mock-claude", "go"], "go"), "port-it-b2c"),
            None,
            "and a command amx has measured nothing about is read as claude's"
        );
    }

    #[test]
    fn fork_refuses_a_vendor_that_names_no_session_vocabulary() {
        // A capability with nothing behind it, which is a different way of
        // being unable to answer to the one the vendor's own name is missing
        // from `capabilities` entirely, and refused the same way.
        let cannot = Vendor {
            session: None,
            capabilities: &[Capability::Fork],
            ..SECOND
        };
        let said =
            cannot_branch(Some(&cannot), "fix-login-a1b").expect("it names no session vocabulary");
        assert!(said.contains("fix-login-a1b"), "{said}");
        assert!(said.contains(cannot.name), "{said}");
        assert!(said.contains("no session vocabulary"), "{said}");
        assert!(said.contains("amx resume fix-login-a1b"), "{said}");
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
            parent: None,
            depth: 0,
            dir: dir.to_path_buf(),
            ..meta("fix-login-a1b", session)
        };
        Agent::create(root.path(), &meta).unwrap();
        (root, meta)
    }

    /// The verb, with nowhere for its output to go but a buffer.
    fn fork(root: &Path, id: &str) -> Result<(i32, String, String)> {
        forked(root, id, false)
    }

    /// The same, with the kind of stderr named.
    fn forked(root: &Path, id: &str, to_terminal: bool) -> Result<(i32, String, String)> {
        let (mut out, mut problems) = (Vec::new(), Vec::new());
        let code = run(
            root,
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
        //
        // The key is written where the copy will run, because that is the
        // project a fork is counted against — and a project's own file beats
        // whatever the person put in theirs, so this holds on any machine.
        let here = TempDir::new().unwrap();
        let (root, _) = a_record(Some("abc-123"), here.path());
        let origin = Agent::open(root.path(), "fix-login-a1b").unwrap();
        spawn::write_handoff(
            origin.dir(),
            &handoff(&["claude", "fix the login bug"], "fix the login bug"),
        )
        .unwrap();
        std::fs::create_dir(here.path().join(".amx")).unwrap();
        std::fs::write(here.path().join(".amx/config.toml"), "max_agents = 0\n").unwrap();
        crate::consent::allow_in(
            root.path(),
            &crate::paths::project_config(here.path()).unwrap(),
        )
        .unwrap();

        let (code, _, plain) = forked(root.path(), "fix-login-a1b", false).unwrap();
        assert_eq!(code, exit::BLOCKED);
        assert!(plain.starts_with("amx fork: "), "{plain:?}");
        assert!(plain.contains("max_agents is 0"), "{plain:?}");
        assert!(!plain.contains('\u{1b}'), "{plain:?}");

        let (_, _, painted) = forked(root.path(), "fix-login-a1b", true).unwrap();
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
