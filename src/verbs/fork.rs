//! `amx fork`: start a new agent on a copy of another agent's conversation.
//!
//! The copy gets its own id, record and pane; only the conversation comes from
//! the origin, copied by the vendor from the recorded session id. An agent with
//! no recorded session cannot be forked. Where the vendor has a start flag, the
//! copy opens under its amx id so it can be resumed or forked later; otherwise
//! (claude) the record waits for the copy's first report to name its session.
//!
//! - The copy runs in the origin's directory, including uncommitted work, but
//!   never records the origin's worktree, so `stop` on the copy cannot remove
//!   it.
//! - The copy's log opens with a fork event naming its origin, written before
//!   the pane exists.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use crate::spawn::{self, Handoff};
use crate::store::{Agent, Event, Meta, now};
use crate::vendor::{Capability, ForkSpec, Resume, Vendor};
use crate::verbs::{new, resume};
use crate::{Severity, exit, paths, said};

/// Event kind that opens a copy's log, naming its origin.
const FORKED: &str = "fork";

/// Run the verb against the machine.
///
/// Caps come from the origin's project config.
pub fn from_env(id: &str, task: Option<&str>) -> Result<i32> {
    let root = paths::state_root()?;
    let env = spawn::env_snapshot(std::env::vars());
    let mut out = std::io::stdout().lock();
    let to_terminal = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let mut problems = std::io::stderr().lock();
    run(&root, id, task, &env, &mut out, &mut problems, to_terminal)
}

/// The verb, against the given state root and environment.
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

    // Every refusal comes before anything is made.
    let session = copied_session(&meta)?;
    if !meta.dir.is_dir() {
        bail!(
            "{}, where {} ran, no longer exists",
            meta.dir.display(),
            meta.id
        );
    }
    let recorded = spawn::read_handoff(origin.dir())
        .with_context(|| format!("reading how {} was started", meta.id))?;
    if let Some(refusal) = cannot_branch(spawn::vendor_of(&recorded), &meta.id) {
        bail!(refusal);
    }

    // Without a prompt the copy is labelled with the origin's task.
    let task = prompt.unwrap_or(&meta.task);
    // Counted against the origin's project, where the copy runs, and claimed
    // in one step as in `new`.
    let taken = new::take_a_place(root, &meta.dir, || {
        let (copy, dir) = new::claim(root, None, task)?;
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
    // The argv needs the minted id for a vendor with a start flag.
    let command = copying(&recorded, &session, &copy, prompt);
    let opened = opened_under(&recorded, &copy);
    let launched = launched_with(&recorded);

    // On failure remove the copy's directory. The claim made it, so it holds
    // no other agent's record.
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

/// Write the copy's handoff and fork event, place its pane and record it.
///
/// Same order as `new`: the handoff before the pane, which reads it, and the
/// record after, once tmux has a pane id. The pane waits for the record before
/// starting the vendor. The fork event is written first so it opens the log.
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
    // Harness pairs from the origin's project config, then amx's id on top so
    // a harness table cannot override it.
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

    let (server, pane) = new::place_boot(id, &origin.dir)?;

    // A pane with no record could never be found or stopped, so kill it if
    // recording fails.
    let recorded = spawn::record(
        root,
        &Meta {
            role: None,
            parent: None,
            depth: 0,
            id: id.to_string(),
            task: task.to_string(),
            agent: launched,
            // Same model and effort as the origin: fork takes no dial flags.
            model: origin.model.clone(),
            effort: origin.effort.clone(),
            dir: origin.dir.clone(),
            // The tree belongs to the origin. Recording it here would let
            // `amx stop` on the copy remove it.
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

/// Log the fork event naming the origin and the session copied.
///
/// It goes on the copy's own log so it survives the origin being removed.
fn names_its_origin(root: &Path, id: &str, origin: &Meta, session: &str) -> Result<()> {
    Agent::open(root, id)?.writer()?.append(&Event::new(
        FORKED,
        serde_json::json!({ "from": origin.id, "session": session }),
    ))
}

/// The vendor argv for a copy of `session`, opened under `copy` where the
/// vendor has a start flag.
///
/// Built from the origin's argv without its task and session words (see
/// [`resume::without_session`]). An old fork marker is dropped too, so a copy
/// of a copy branches once.
fn copying(handoff: &Handoff, session: &str, copy: &str, prompt: Option<&str>) -> Vec<String> {
    build_copy(handoff, session, copy, prompt, spawn::vendor_of(handoff))
}

/// [`Meta::session`] for the copy: the minted id where the vendor has a start
/// flag, as [`build_copy`] passes it.
///
/// `None` otherwise (claude), where the copy's session-start hook reports it.
fn opened_under(handoff: &Handoff, copy: &str) -> Option<String> {
    resume::spelling(spawn::vendor_of(handoff))
        .start
        .map(|_| copy.to_string())
}

/// [`Meta::agent`] for the copy: the first word of the origin's argv.
///
/// Read from the handoff because older records do not store the vendor.
fn launched_with(handoff: &Handoff) -> Option<String> {
    handoff.command.first().cloned()
}

/// [`copying`] with the vendor passed in, so tests can use vendors outside
/// the table. `None` is a command with no table entry, spelled as claude.
///
/// [`ForkSpec::Marker`] resumes the origin and adds the marker to branch it.
/// [`ForkSpec::Origin`] is a flag carrying the origin's session, with no
/// resume. [`ForkSpec::Subcommand`] is the same, as a word after the program.
/// A vendor with a start flag also gets `copy` as the new session's id.
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
        // A marker takes no value. Drop an old one so a copy of a copy
        // branches once.
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

/// Push `flag` and `value` as `flag=value` or as two words.
fn push_flag(command: &mut Vec<String>, flag: &str, joined: bool, value: &str) {
    if joined {
        command.push(format!("{flag}={value}"));
    } else {
        command.push(flag.to_string());
        command.push(value.to_string());
    }
}

/// Why this vendor cannot fork a conversation, if it cannot.
///
/// Without fork support the vendor would reject the flags and the pane would
/// die at once, after an id and directory were spent. A command with no table
/// entry is not refused.
fn cannot_branch(vendor: Option<&Vendor>, id: &str) -> Option<String> {
    let vendor = vendor?;
    if !vendor.can(Capability::Fork) {
        return Some(format!(
            "{id} runs {}, which cannot fork a conversation; continue it with \
             `amx resume {id}`, or start a new agent with `amx new`",
            vendor.name
        ));
    }
    // A claimed capability with no session vocabulary is refused the same way.
    vendor.session.is_none().then(|| {
        format!(
            "{id} runs {}, and amx has no session support for it; continue it with \
             `amx resume {id}`, or start a new agent with `amx new`",
            vendor.name
        )
    })
}

/// The origin's recorded session, validated before it becomes an argument.
fn copied_session(meta: &Meta) -> Result<String> {
    let Some(session) = meta.session.as_deref() else {
        bail!(
            "no session was recorded for {}, so there is no conversation to copy; \
             start a new agent with `amx new`",
            meta.id
        );
    };
    if !resume::is_session_id(session) {
        bail!(
            "the session recorded for {} is not a session id, so amx will not pass it on",
            meta.id
        );
    }
    Ok(session.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::{PaneId, Socket};
    use crate::vendor::SessionSpec;
    use crate::vendor::second::{BRANCHING, SECOND};
    use std::path::PathBuf;
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
        // The copy runs the origin's argv, so its vendor is the first word.
        let started = handoff(
            &["claude", "--model", "opus", "fix the login bug"],
            "fix the login bug",
        );
        assert_eq!(launched_with(&started).as_deref(), Some("claude"));
        assert_eq!(launched_with(&handoff(&[], "")), None);
    }

    #[test]
    fn fork_ends_pis_options_before_a_prompt_and_drops_the_old_end() {
        // The old task goes with its `--`. A new prompt gets its own `--`, and
        // a leading `@` gets a space.
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
        // A prompt-flag vendor got the task as one `--say=` word with a popup
        // space. The copy drops it and gets its own prompt the same way.
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
        // The origin's task is not sent again; a new prompt goes last.
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
        // `new` passes `brief\n\ntask` as one word and records only the task,
        // so the copy must not send the old task again.
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
        // Every original argument, such as `--add-dir`, is kept.
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
        // Session words already in the argv are replaced: a resumed agent's
        // `--resume` and an earlier copy's `--fork-session`.
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

        // `--resume`'s value is optional, so a following flag is kept.
        let started = handoff(&["claude", "--resume", "--verbose", "go"], "go");
        assert_eq!(
            copying(&started, "def-456", "port-it-b2c", None),
            ["claude", "--verbose", "--resume=def-456", "--fork-session"]
        );
    }

    #[test]
    fn fork_answers_a_vendor_that_branches_by_naming_the_origin() {
        // ForkSpec::Origin: the flag carries the origin's session and no
        // resume is written. There is no start flag, so the minted id is
        // absent.
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

        // A copy of a copy names one origin, not two.
        let started = handoff(&["pi", "--branch-from=old", "go"], "go");
        assert_eq!(
            build_copy(&started, "def-456", "port-it-b2c", None, Some(&vendor)),
            ["pi", "--branch-from=def-456"]
        );
    }

    #[test]
    fn fork_writes_a_subcommand_resume_right_after_the_program() {
        // A subcommand resume: program, word, origin id, then the marker.
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
        // The fork word and origin id open the argv and the prompt follows
        // the end of options. No earlier session words survive: not the
        // task's `--`, a resume's, or an earlier fork's.
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
        // The word only counts right after the program.
        let started = handoff(&["second", "-m", "fork", "--", "go"], "go");
        assert_eq!(
            build_copy(&started, "abc-123", "port-it-b2c", None, branching),
            ["second", "fork", "abc-123", "-m", "fork"]
        );
    }

    #[test]
    fn fork_opens_the_copy_under_the_id_amx_minted_for_it() {
        // pi: the fork flag carries the origin and the start flag the copy's
        // own id, so the copy's session is one amx chose.
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

        // A copy of a copy branches from the copy's session under a new id.
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
        // claude has no start flag; its session-start hook reports the copy's
        // session, so no minted id is passed or recorded.
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
        // The capability is claimed but no vocabulary backs it.
        let cannot = Vendor {
            session: None,
            capabilities: &[Capability::Fork],
            ..SECOND
        };
        let said =
            cannot_branch(Some(&cannot), "fix-login-a1b").expect("it names no session vocabulary");
        assert!(said.contains("fix-login-a1b"), "{said}");
        assert!(said.contains(cannot.name), "{said}");
        assert!(said.contains("no session support"), "{said}");
        assert!(said.contains("amx resume fix-login-a1b"), "{said}");
    }

    #[test]
    fn fork_refuses_a_vendor_that_cannot_branch_a_conversation() {
        // The refusal names the vendor, the agent and what is missing.
        let said = cannot_branch(Some(&SECOND), "fix-login-a1b").expect("it cannot fork");
        assert!(said.contains("fix-login-a1b"), "{said}");
        assert!(said.contains(SECOND.name), "{said}");
        assert!(said.contains("cannot fork a conversation"), "{said}");
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

    /// Run the verb into buffers: the exit code, stdout and stderr.
    fn fork(root: &Path, id: &str) -> Result<(i32, String, String)> {
        forked(root, id, false)
    }

    /// Same as `fork`, with `to_terminal` chosen.
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
        // The refusal comes before any id or pane is made.
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
        // Two agents on one conversation look alike; the log records which
        // came from which.
        let root = TempDir::new().unwrap();
        let (copy, dir) = new::claim(root.path(), None, "fix the login bug").unwrap();
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
        // The cap is a refusal, so it is yellow, not red. The key goes in the
        // project config where the copy runs, which overrides the user's own
        // file on any machine.
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
        // A copy runs where the origin ran, so a removed tree means the fork
        // cannot start. Name the directory rather than leave it to tmux's
        // error.
        let (root, meta) = a_record(Some("abc-123"), Path::new("/nowhere/at/all"));

        let said = format!("{:#}", fork(root.path(), "fix-login-a1b").unwrap_err());
        assert!(said.contains(&meta.dir.display().to_string()), "{said}");
        assert!(said.contains("no longer exists"), "{said}");
    }
}
