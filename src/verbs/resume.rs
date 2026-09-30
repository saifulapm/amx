//! `amx resume`: restart an agent's command on the vendor session it had.
//!
//! The id, directory, branch and event log stay the same; only the pane is
//! new. A resume needs the session recorded from the vendor's reports and the
//! handoff written at spawn. An adopted agent has a session but no handoff, so
//! it cannot be resumed.
//!
//! - The pane is placed before the record is touched. There is no pane id to
//!   record until tmux makes one, and a failed place leaves the agent as it
//!   ended.
//! - The record update (the pane, then the reset to `starting`) happens under
//!   the writer lock taken before the pane exists. The new pane's hooks wait on
//!   that lock, so none of them sees a record that still says the agent ended.
//! - A message rides the vendor argv, which cannot race the vendor's startup
//!   the way a `send` would. A send event is still logged before the vendor can
//!   report, so `result` waits for the turn the message starts.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::spawn::{self, Handoff};
use crate::store::{Agent, Event, Meta, Phase, State};
use crate::tmux::Server;
use crate::vendor::{self, Capability, Resume, SessionSpec, Vendor};
use crate::verbs::{new, send};
use crate::{complain, derive, exit, paths, store, warn, worktree};

/// Event kind logged when an agent is resumed.
pub(crate) const RESUMED: &str = "resume";

/// Outcome of a resume started from `amx attach` or the view.
pub enum Comeback {
    /// Back in a pane on its old session.
    Back,
    /// Nothing was started, for this reason.
    No(String),
}

/// Resume an agent whose pane is gone, with an optional first message.
///
/// For `amx attach` and the view. The caller has already found the pane gone.
/// A missing session or handoff and a full cap come back as
/// [`Comeback::No`]; anything else is an error. The message rides the vendor
/// argv, as with `amx resume <id> <message>`.
pub fn picked_up(
    root: &Path,
    id: &str,
    message: Option<&str>,
    env: &BTreeMap<String, String>,
) -> Result<Comeback> {
    let agent = Agent::open(root, id)?;
    let meta = agent.meta()?;
    if let Err(why) = to_continue(&meta) {
        return Ok(Comeback::No(why));
    }
    if let Err(why) = to_start(agent.dir(), id) {
        return Ok(Comeback::No(why));
    }
    let _place = match take_a_place(root, id, &meta.dir)? {
        Ok(place) => place,
        Err(full) => return Ok(Comeback::No(full)),
    };
    bring_back(root, id, message, env)?;
    Ok(Comeback::Back)
}

/// Whether the agent has both a usable session and a handoff to resume from.
///
/// The cap is not checked: it can change before the resume runs.
pub fn can_come_back(meta: &Meta, dir: &Path) -> bool {
    to_continue(meta).is_ok() && to_start(dir, &meta.id).is_ok()
}

/// Run the verb against the machine.
///
/// Caps come from the agent's own project config.
pub fn from_env(id: Option<&str>, message: Option<&str>, all: bool) -> Result<i32> {
    let root = paths::state_root()?;
    let env = spawn::env_snapshot(std::env::vars());
    let mut out = std::io::stdout().lock();
    run(&root, id, message, all, &env, &mut out)
}

/// The verb, against the given state root and environment.
pub fn run(
    root: &Path,
    id: Option<&str>,
    message: Option<&str>,
    all: bool,
    env: &BTreeMap<String, String>,
    out: &mut impl Write,
) -> Result<i32> {
    match id {
        Some(id) if !all => one(root, id, message, env, out),
        _ => sweep(root, env, out),
    }
}

/// Resume one named agent.
fn one(
    root: &Path,
    id: &str,
    message: Option<&str>,
    env: &BTreeMap<String, String>,
    out: &mut impl Write,
) -> Result<i32> {
    let view = derive::view(root, id, store::now())?;
    // Checked before the phase: a command row never takes a message,
    // whatever its phase.
    if message.is_some() && is_a_command(&view) {
        complain!(
            "amx resume: {id} is a command and cannot take a message; \
             run it again with `amx new --exec`"
        );
        return Ok(exit::FAILURE);
    }
    // When tmux cannot be asked, the view falls back to the record, which
    // reads a parked agent as idle. Raise tmux's error instead.
    if !view.phase().is_terminal() {
        Server::from_socket(view.meta.socket.clone()).answers_for_now(&view.meta.pane, id)?;
    }
    if !nothing_is_running(&view) {
        warn!(
            "amx resume: {id} is {}; stop it before resuming it",
            view.phase()
        );
        return Ok(exit::BLOCKED);
    }
    let _place = match take_a_place(root, id, &view.meta.dir)? {
        Ok(place) => place,
        Err(full) => {
            warn!("amx resume: {full}");
            return Ok(exit::BLOCKED);
        }
    };

    bring_back(root, id, message, env)?;
    writeln!(out, "{id} resumed")?;
    Ok(exit::OK)
}

/// Whether the row is an `--exec` command, which records no vendor.
fn is_a_command(view: &derive::View) -> bool {
    view.meta.agent.is_none()
}

/// Whether no vendor is running in the agent's pane.
///
/// True for an ended agent and for one amx parked, which reads idle but has no
/// pane (see [`crate::verbs::park`]). Any other idle agent is a vendor at its
/// prompt, and starting a second one over it would split the conversation.
fn nothing_is_running(view: &derive::View) -> bool {
    view.phase().is_terminal() || view.verdict.evidence == derive::Evidence::LetGo
}

/// Resume every agent whose pane disappeared, as after a tmux server died.
///
/// Agents that finished, or were stopped with `amx stop`, are left alone.
fn sweep(root: &Path, env: &BTreeMap<String, String>, out: &mut impl Write) -> Result<i32> {
    let stopped: Vec<_> = derive::views(root, store::now())?
        .into_iter()
        .filter(lost_its_pane)
        .collect();
    if stopped.is_empty() {
        writeln!(out, "nothing to bring back")?;
        return Ok(exit::OK);
    }

    for view in stopped {
        // Each agent counts against its own project's cap. A full project
        // skips its agents; the others still come back.
        let _place = match take_a_place(root, view.id(), &view.meta.dir)? {
            Ok(place) => place,
            Err(full) => {
                warn!("amx resume: {}: {full}", view.id());
                continue;
            }
        };
        // One failure does not stop the sweep.
        match bring_back(root, view.id(), None, env) {
            Ok(()) => writeln!(out, "{} resumed", view.id())?,
            Err(e) => complain!("amx resume: {}: {e:#}", view.id()),
        }
    }
    Ok(exit::OK)
}

/// Whether the agent stopped because its pane vanished, rather than by
/// `amx stop`.
fn lost_its_pane(view: &derive::View) -> bool {
    view.phase() == Phase::Stopped && view.verdict.evidence == derive::Evidence::Gone
}

/// Claim a place for `id` under the caps of the project `dir` belongs to, or
/// return the refusal. The place is held until the claim is dropped.
fn take_a_place(root: &Path, id: &str, dir: &Path) -> Result<Result<store::Claim, String>> {
    let taken = new::take_a_place(root, dir, || Ok(((), paths::agent_dir_in(root, id)?)))?;
    Ok(taken.map(|((), place)| place))
}

/// Place the agent in a new pane on its old session and update the record.
///
/// Runs under the agent's writer lock, so the ended check and the respawn are
/// one step: a concurrent second resume waits, then finds the `starting` state
/// and live pane the first wrote. The checks in [`one`] and [`sweep`] only
/// give friendlier refusals.
fn bring_back(
    root: &Path,
    id: &str,
    message: Option<&str>,
    env: &BTreeMap<String, String>,
) -> Result<()> {
    let agent = Agent::open(root, id)?;
    let writer = agent.writer()?;

    // Read the raw record: deriving a view may take this same lock. A pane
    // that answers for another id (a reused pane number) counts as gone.
    let current = writer.state()?;
    let meta = agent.meta()?;
    if !current.state.is_terminal()
        && Server::from_socket(meta.socket.clone()).answers_for_now(&meta.pane, &meta.id)?
    {
        bail!("{id} is already running");
    }

    let session = to_continue(&meta).map_err(anyhow::Error::msg)?;
    to_start(agent.dir(), id).map_err(anyhow::Error::msg)?;

    let recorded = spawn::read_handoff(agent.dir())
        .with_context(|| format!("reading how {id} was started"))?;
    let dir = ready_dir(&meta)?;

    // The caller's current environment, as in `new`. Harness pairs come from
    // the agent's own project config, then amx's id goes on top so a harness
    // table cannot override it.
    let mut env = env.clone();
    if let Some(agent) = &meta.agent {
        spawn::harness_env(&mut env, &crate::config::for_dir(&meta.dir).0, agent);
    }
    env.insert(crate::hook::ID_ENV.to_string(), id.to_string());
    spawn::write_boot_env(agent.dir(), &env)?;
    spawn::write_handoff(agent.dir(), &handed_on(&recorded, session, message))?;

    // Place before touching the record. On failure, drop the boot env and
    // restore the handoff so the agent stays as it ended.
    let (server, pane) = match new::place_boot(id, &dir) {
        Ok(placed) => placed,
        Err(e) => {
            let _ = std::fs::remove_file(agent.dir().join(spawn::BOOT_ENV));
            spawn::write_handoff(agent.dir(), &recorded)?;
            return Err(e);
        }
    };

    // Still under the writer lock, so the new pane's hooks wait for all of
    // this. The pane goes first, so a reader that sees `starting` sees the
    // new pane.
    writer.update_meta(|meta| {
        meta.socket = server.socket().clone();
        meta.pane = pane;
    })?;
    writer.append(&Event::new(
        RESUMED,
        serde_json::json!({ "session": session }),
    ))?;
    writer.update_state_heard(agent.heartbeat(), |state| {
        *state = State {
            // Without a message no turn starts, so nothing reports after the
            // session opens. See `State::opens_idle`.
            opens_idle: message.is_none(),
            ..state.for_a_new_session()
        }
    })?;
    // Log the message as a send before the vendor can report, in the same
    // order as `send::deliver`, so a `result` in another shell waits for this
    // turn. There is no paste: the message is already on the argv.
    if let Some(message) = message {
        writer.append(&Event::new(
            send::SEND,
            serde_json::json!({ "text": message }),
        ))?;
        writer.observe(|state| state.seq += 1)?;
    }
    Ok(())
}

/// The handoff for this resume: the continuation argv, plus `message` as the
/// new task when there is one.
///
/// The message goes last, where `new` puts a task, and becomes the handoff's
/// task so the next resume strips it again. [`Meta::task`] is not changed.
fn handed_on(recorded: &Handoff, session: &str, message: Option<&str>) -> Handoff {
    let mut command = continuing(recorded, session);
    if let Some(message) = message {
        command.extend(spawn::ends_options_of(recorded).map(str::to_string));
        command.push(spawn::as_words(spawn::vendor_of(recorded), message));
    }
    Handoff {
        task: message.unwrap_or(&recorded.task).to_string(),
        command,
    }
}

/// The recorded argv rewritten to resume `session`.
///
/// Drops the task, which the session already has, and every flag naming a
/// session (a start flag such as `--session-id`, or an earlier `--resume`),
/// then adds the vendor's resume flag or subcommand.
fn continuing(handoff: &Handoff, session: &str) -> Vec<String> {
    build_continuation(handoff, session, spawn::vendor_of(handoff))
}

/// [`continuing`] with the vendor passed in, so tests can use vendors outside
/// the table.
fn build_continuation(handoff: &Handoff, session: &str, vendor: Option<&Vendor>) -> Vec<String> {
    let spec = spelling(vendor);
    let mut command = without_session(handoff, vendor, &spec);
    let resume = spec.resume_args(session);
    match spec.resume {
        Resume::Subcommand(_) => drop(command.splice(1..1, resume)),
        Resume::Flag { .. } => command.extend(resume),
    }
    command
}

/// The recorded argv without its task and without any word naming a session.
///
/// Shared by resume and fork. The task is the last word, matched as a suffix
/// because `new` writes a role brief or subagent digest in front of it in the
/// same word. A session flag's value is taken only when the next word does not
/// start with `-`: claude documents `--resume`'s value as optional.
pub(crate) fn without_session(
    handoff: &Handoff,
    vendor: Option<&Vendor>,
    spec: &SessionSpec,
) -> Vec<String> {
    let task = spawn::as_typed(vendor, &handoff.task);
    let mut words = handoff.command.clone().into_iter().peekable();
    let mut command: Vec<String> = Vec::new();

    while let Some(word) = words.next() {
        // The task, possibly with a brief in front and a popup space after.
        // An empty task matches nothing.
        if words.peek().is_none() && !handoff.task.is_empty() && word.ends_with(&task) {
            break;
        }
        // The end-of-options word `new` put before the task goes with it.
        if words.len() == 1 && Some(word.as_str()) == vendor.and_then(|vendor| vendor.ends_options)
        {
            continue;
        }
        // A subcommand only counts right after the program.
        let first = handoff.command.len() - words.len() - 1 == 1;
        let Some(value_is_a_word_of_its_own) = spec.names_a_session(&word, first) else {
            command.push(word);
            continue;
        };
        if value_is_a_word_of_its_own && words.peek().is_some_and(|next| !next.starts_with('-')) {
            words.next();
        }
    }
    command
}

/// The vendor's session vocabulary, or claude's for a command with no table
/// entry.
pub(crate) fn spelling(vendor: Option<&Vendor>) -> SessionSpec {
    vendor
        .and_then(|vendor| vendor.session)
        .unwrap_or_else(unmeasured)
}

/// Claude's session vocabulary.
fn unmeasured() -> SessionSpec {
    vendor::claude::VENDOR
        .session
        .expect("claude declares a session vocabulary")
}

/// The agent's directory, restoring its worktree if `stop` removed it.
///
/// `stop` only removes a tree whose work is committed, so checking the branch
/// out again restores everything.
fn ready_dir(meta: &Meta) -> Result<PathBuf> {
    if meta.dir.is_dir() {
        return Ok(meta.dir.clone());
    }
    match (&meta.worktree, &meta.branch) {
        (Some(tree), Some(branch)) if tree == &meta.dir => {
            worktree::restore(&repo_above(tree)?, tree, branch)
                .with_context(|| format!("restoring the worktree {}", tree.display()))?;
            Ok(meta.dir.clone())
        }
        _ => bail!(
            "{}, where {} ran, no longer exists",
            meta.dir.display(),
            meta.id
        ),
    }
}

/// The repository holding the nearest existing ancestor of a removed tree.
fn repo_above(tree: &Path) -> Result<PathBuf> {
    let mut above = tree.parent();
    while let Some(dir) = above {
        if dir.is_dir() {
            return worktree::repo_root(dir)?
                .with_context(|| format!("{} is no longer in a git repository", dir.display()));
        }
        above = dir.parent();
    }
    bail!("no directory above {} exists", tree.display())
}

/// The recorded session to resume, or why there is none.
fn to_continue(meta: &Meta) -> Result<&str, String> {
    let Some(session) = meta.session.as_deref() else {
        return Err(format!(
            "no session was recorded for {}, so there is nothing to continue; \
             start a new agent with `amx new`",
            meta.id
        ));
    };
    // The id came from a hook payload; validate it where it becomes an
    // argument.
    if !is_session_id(session) {
        return Err(format!(
            "the session recorded for {} is not a session id, so amx will not pass it on",
            meta.id
        ));
    }
    Ok(session)
}

/// Whether there is a handoff to restart from and its vendor can resume, or
/// why not.
///
/// An adopted agent has a session but no handoff.
fn to_start(dir: &Path, id: &str) -> Result<(), String> {
    if !dir.join(spawn::HANDOFF).exists() {
        return Err(format!(
            "{id} was started by hand rather than by amx, so there is no command to start again"
        ));
    }
    // An unreadable handoff is reported by the respawn when it reads it.
    let recorded = spawn::read_handoff(dir).ok();
    match cannot_continue(recorded.as_ref().and_then(spawn::vendor_of), id) {
        Some(refusal) => Err(refusal),
        None => Ok(()),
    }
}

/// Why this vendor cannot resume a session, if it cannot.
///
/// Without resume support the vendor would reject the flag and the pane would
/// die at once while the record says the agent came back. A command with no
/// table entry is not refused.
fn cannot_continue(vendor: Option<&Vendor>, id: &str) -> Option<String> {
    let vendor = vendor?;
    if !vendor.can(Capability::Resume) {
        return Some(format!(
            "{id} runs {}, which cannot resume a session; start a new agent with `amx new`",
            vendor.name
        ));
    }
    // A claimed capability with no session vocabulary is refused the same way.
    vendor.session.is_none().then(|| {
        format!(
            "{id} runs {}, and amx has no session support for it; \
             start a new agent with `amx new`",
            vendor.name
        )
    })
}

/// Whether a recorded session id is safe to pass as an argument: 1 to 64
/// ASCII alphanumerics, `-` or `_`, not starting with `-`.
pub(crate) fn is_session_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value.starts_with('-')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::{Evidence, Verdict};
    use crate::tmux::{PaneId, Socket};
    use crate::vendor::second::{BRANCHING, ELSEWHERE, SECOND};
    use tempfile::TempDir;

    fn handoff(command: &[&str], task: &str) -> Handoff {
        Handoff {
            task: task.to_string(),
            command: command.iter().map(|word| word.to_string()).collect(),
        }
    }

    /// A derived view of a test agent in `phase`, read from `evidence`.
    fn read_as(phase: Phase, evidence: Evidence) -> derive::View {
        derive::View {
            meta: Meta {
                role: None,
                parent: None,
                depth: 0,
                id: "fix-login-a1b".to_string(),
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
                session: Some("abc-123".to_string()),
                transcript: None,
                created: 1,
            },
            state: State {
                state: phase,
                ..State::default()
            },
            verdict: Verdict {
                phase,
                evidence,
                rule: None,
                age: 3_640,
                worked: 12,
            },
            doing: None,
        }
    }

    #[test]
    fn resume_brings_back_an_agent_whose_pane_amx_let_go() {
        // A parked agent reads idle but has no vendor running.
        assert!(nothing_is_running(&read_as(Phase::Idle, Evidence::LetGo)));

        // Ended agents have nothing running either.
        for phase in [Phase::Done, Phase::Failed, Phase::Stopped] {
            assert!(
                nothing_is_running(&read_as(phase, Evidence::Record)),
                "{phase}"
            );
        }

        // An agent with a live pane is refused in every phase, idle included.
        for phase in [
            Phase::Starting,
            Phase::Working,
            Phase::Waiting,
            Phase::Idle,
            Phase::Unknown,
        ] {
            assert!(
                !nothing_is_running(&read_as(phase, Evidence::Hooks)),
                "{phase}"
            );
        }
    }

    #[test]
    fn resume_all_brings_back_only_an_agent_whose_pane_went() {
        assert!(lost_its_pane(&read_as(Phase::Stopped, Evidence::Gone)));

        // An ending `amx stop` wrote is deliberate, so the sweep leaves it.
        assert!(!lost_its_pane(&read_as(Phase::Stopped, Evidence::Record)));
        for phase in [Phase::Done, Phase::Failed] {
            assert!(!lost_its_pane(&read_as(phase, Evidence::Record)), "{phase}");
        }
        assert!(!lost_its_pane(&read_as(Phase::Idle, Evidence::LetGo)));
    }

    #[test]
    fn resume_refuses_a_tmux_that_cannot_be_asked_and_starts_nothing() {
        // If tmux cannot be asked, the pane may still exist, so resuming could
        // start a second one.
        let state = TempDir::new().unwrap();
        let root = state.path().join("agents");
        std::fs::create_dir_all(&root).unwrap();
        let mut meta = read_as(Phase::Idle, Evidence::Record).meta;
        meta.agent = Some("claude".to_string());
        meta.socket = crate::tmux::unaskable();
        let agent = Agent::create(&root, &meta).unwrap();
        spawn::write_handoff(agent.dir(), &handoff(&["claude", "go"], "go")).unwrap();
        agent
            .writer()
            .unwrap()
            .observe(|state| {
                state.state = Phase::Idle;
                state.parked_at = 1_000;
            })
            .unwrap();
        let before = agent.state().unwrap();
        let env = BTreeMap::new();

        let mut out = Vec::new();
        let named = run(&root, Some("fix-login-a1b"), None, false, &env, &mut out);
        let picked = picked_up(&root, "fix-login-a1b", None, &env).map(|_| ());
        for why in [named.map(|_| ()), picked] {
            let why = why.unwrap_err();
            assert!(
                format!("{why:#}").starts_with("listing the tmux panes: "),
                "{why:#}"
            );
        }
        assert_eq!(agent.state().unwrap(), before);
        assert_eq!(agent.meta().unwrap(), meta);
        assert!(!agent.dir().join(spawn::BOOT_ENV).exists());
        assert!(out.is_empty());
    }

    #[test]
    fn resume_says_whether_there_is_anything_to_bring_an_agent_back_on() {
        let dir = TempDir::new().unwrap();
        let mut meta = read_as(Phase::Stopped, Evidence::Record).meta;

        // Needs both the session and the handoff.
        assert!(
            !can_come_back(&meta, dir.path()),
            "there is no command to start again"
        );
        spawn::write_handoff(dir.path(), &handoff(&["claude", "go"], "go")).unwrap();
        assert!(can_come_back(&meta, dir.path()));

        meta.session = None;
        assert!(
            !can_come_back(&meta, dir.path()),
            "there is no session to continue"
        );
    }

    #[test]
    fn resume_refuses_a_vendor_that_cannot_carry_a_session_on() {
        // The test vendor can resume, so build one that cannot. The check is on
        // the capability, not the name.
        let cannot = Vendor {
            capabilities: &[Capability::Adopt],
            ..SECOND
        };
        let said = cannot_continue(Some(&cannot), "fix-login-a1b").expect("it cannot resume");
        assert!(said.contains("fix-login-a1b"), "{said}");
        assert!(said.contains(cannot.name), "{said}");
        assert!(said.contains("cannot resume a session"), "{said}");
        assert!(said.contains("amx new"), "{said}");

        assert_eq!(cannot_continue(Some(&SECOND), "fix-login-a1b"), None);
        assert_eq!(
            cannot_continue(crate::registry::entry("claude"), "fix-login-a1b"),
            None,
            "the vendor amx was written against can"
        );
        assert_eq!(
            cannot_continue(None, "fix-login-a1b"),
            None,
            "and a command amx has no entry for is not amx's to refuse: \
             nothing measured is not a measurement"
        );
    }

    #[test]
    fn resume_refuses_a_vendor_that_names_no_session_vocabulary() {
        // The capability is claimed but no vocabulary backs it.
        let cannot = Vendor {
            session: None,
            ..SECOND
        };
        assert!(
            cannot.can(Capability::Resume),
            "the capability is claimed; only the spelling is missing"
        );
        let said = cannot_continue(Some(&cannot), "fix-login-a1b")
            .expect("it names no session vocabulary");
        assert!(said.contains("fix-login-a1b"), "{said}");
        assert!(said.contains(cannot.name), "{said}");
        assert!(said.contains("no session support"), "{said}");
        assert!(said.contains("amx new"), "{said}");
    }

    #[test]
    fn resume_reads_a_different_vendors_own_spelling_off_the_table() {
        // The test vendor resumes with a subcommand and has its own
        // conflicting flag.
        let started = handoff(&["second", "--open", "old", "go"], "go");
        assert_eq!(
            build_continuation(&started, "abc-123", Some(&SECOND)),
            ["second", "again", "abc-123"]
        );
    }

    #[test]
    fn resume_puts_a_subcommand_right_after_the_program_and_replaces_it() {
        // Program, subcommand, id, then the original flags. A second resume
        // replaces the first pair.
        let started = handoff(&["second", "--care", "quick", "go"], "go");
        let once = build_continuation(&started, "abc-123", Some(&SECOND));
        assert_eq!(once, ["second", "again", "abc-123", "--care", "quick"]);

        let resumed = Handoff {
            task: "go".to_string(),
            command: once,
        };
        assert_eq!(
            build_continuation(&resumed, "def-456", Some(&SECOND)),
            ["second", "again", "def-456", "--care", "quick"]
        );
    }

    #[test]
    fn resume_keeps_the_launch_words_and_a_keyed_dial_once() {
        // Launch words and a keyed dial name no session, so they stay after
        // the subcommand and id, and a second resume does not repeat them.
        let started = handoff(
            &[
                "second",
                "--alone",
                "-m",
                "large",
                "-c",
                "care=thorough",
                "go",
            ],
            "go",
        );
        let once = build_continuation(&started, "abc-123", Some(&BRANCHING));
        assert_eq!(
            once,
            [
                "second",
                "again",
                "abc-123",
                "--alone",
                "-m",
                "large",
                "-c",
                "care=thorough"
            ]
        );

        let resumed = Handoff {
            task: "go".to_string(),
            command: once,
        };
        let twice = build_continuation(&resumed, "def-456", Some(&BRANCHING));
        assert_eq!(twice[..3], ["second", "again", "def-456"]);
        for word in BRANCHING.launch {
            assert_eq!(twice.iter().filter(|w| w == word).count(), 1, "{word}");
        }
    }

    #[test]
    fn resume_says_which_half_is_missing_before_it_starts_anything() {
        // A missing handoff and a vendor that cannot resume get different
        // refusals.
        let dir = TempDir::new().unwrap();
        let said = to_start(dir.path(), "fix-login-a1b").expect_err("no handoff at all");
        assert!(said.contains("started by hand"), "{said}");

        spawn::write_handoff(dir.path(), &handoff(&["claude", "go"], "go")).unwrap();
        assert_eq!(to_start(dir.path(), "fix-login-a1b"), Ok(()));

        // A command with no table entry is not refused.
        spawn::write_handoff(dir.path(), &handoff(&["mock-claude", "go"], "go")).unwrap();
        assert_eq!(to_start(dir.path(), "fix-login-a1b"), Ok(()));
    }

    #[test]
    fn resume_ends_pis_options_before_a_message_and_drops_the_old_end() {
        // pi's task sits behind `--`, which is dropped with it. A message gets
        // its own `--`, and a leading `@` gets a space so pi does not read it
        // as a file.
        let started = handoff(
            &["pi", "--session-id", "abc-123", "--", " @alice asked"],
            "@alice asked",
        );
        let carried = handed_on(&started, "abc-123", None);
        assert_eq!(carried.command, ["pi", "--session-id", "abc-123"]);

        let carried = handed_on(&started, "abc-123", Some("@bob too"));
        assert_eq!(
            carried.command,
            ["pi", "--session-id", "abc-123", "--", " @bob too"]
        );
        let after = handed_on(&carried, "def-456", None);
        assert_eq!(after.command, ["pi", "--session-id", "def-456"]);
    }

    #[test]
    fn resume_drops_a_task_or_message_that_rode_on_the_prompt_flag() {
        // A prompt-flag vendor got the task as one `--say=` word with a brief
        // in front and a popup space after. Resume drops the whole word, for a
        // task or a message.
        let started = handoff(&["second", "--say=Brief.\n\nlook at #3 "], "look at #3");
        assert_eq!(
            build_continuation(&started, "abc-123", Some(&ELSEWHERE)),
            ["second", "again", "abc-123"]
        );

        let messaged = handoff(
            &["second", "again", "abc-123", "--say=-v is broken"],
            "-v is broken",
        );
        assert_eq!(
            build_continuation(&messaged, "def-456", Some(&ELSEWHERE)),
            ["second", "again", "def-456"]
        );
    }

    #[test]
    fn resume_puts_a_message_where_a_first_turn_goes() {
        let started = handoff(
            &["claude", "--model", "opus", "fix the login bug"],
            "fix the login bug",
        );

        // Without a message the task is unchanged.
        let carried = handed_on(&started, "abc-123", None);
        assert_eq!(carried.task, "fix the login bug");
        assert_eq!(
            carried.command,
            ["claude", "--model", "opus", "--resume=abc-123"]
        );

        // A message goes last and becomes the handoff's task.
        let carried = handed_on(&started, "abc-123", Some("and now the linter"));
        assert_eq!(carried.task, "and now the linter");
        assert_eq!(
            carried.command,
            [
                "claude",
                "--model",
                "opus",
                "--resume=abc-123",
                "and now the linter"
            ]
        );

        // So the next resume drops it.
        let after = handed_on(&carried, "def-456", None);
        assert_eq!(
            after.command,
            ["claude", "--model", "opus", "--resume=def-456"]
        );
    }

    #[test]
    fn resume_asks_the_vendor_to_continue_the_session_it_opened() {
        let started = handoff(
            &["claude", "--model", "opus", "fix the login bug"],
            "fix the login bug",
        );
        assert_eq!(
            continuing(&started, "abc-123"),
            ["claude", "--model", "opus", "--resume=abc-123"],
            "the flag and its value are one word: the value is optional, and a \
             separate one would be read as a flag of its own"
        );
    }

    #[test]
    fn resume_does_not_put_the_task_a_second_time() {
        // Only the last word is the task, even when it looks like a flag and
        // appears earlier too.
        let started = handoff(&["claude", "--model", "--model"], "--model");
        assert_eq!(
            continuing(&started, "abc"),
            ["claude", "--model", "--resume=abc"]
        );
    }

    #[test]
    fn resume_drops_a_task_a_role_put_its_brief_in_front_of() {
        // A role spawn passes `brief\n\ntask` as one word and records only
        // the task, so the task is matched as a suffix.
        let started = handoff(
            &["claude", "You are a scout.\n\nfix the login bug"],
            "fix the login bug",
        );
        assert_eq!(continuing(&started, "abc"), ["claude", "--resume=abc"]);

        // An empty task must not match the last word.
        let started = handoff(&["claude", "--model", "opus"], "");
        assert_eq!(
            continuing(&started, "abc"),
            ["claude", "--model", "opus", "--resume=abc"]
        );
    }

    #[test]
    fn resume_drops_the_flag_that_would_start_a_session_instead() {
        for started in [
            handoff(
                &["claude", "--session-id", "abc-123", "--model", "opus", "go"],
                "go",
            ),
            handoff(
                &["claude", "--session-id=abc-123", "--model", "opus", "go"],
                "go",
            ),
        ] {
            assert_eq!(
                continuing(&started, "abc-123"),
                ["claude", "--model", "opus", "--resume=abc-123"],
                "{:?}",
                started.command
            );
        }
    }

    #[test]
    fn resume_carries_everything_the_agent_was_started_with() {
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
            continuing(&started, "abc-123"),
            [
                "claude",
                "--model",
                "opus",
                "--add-dir",
                "/srv/data",
                "--verbose",
                "--resume=abc-123"
            ]
        );
    }

    #[test]
    fn resuming_twice_asks_for_one_session_and_not_two() {
        // A resumed command already carries `--resume`; the next resume
        // replaces it rather than adding a second.
        let started = handoff(&["claude", "--add-dir", "/srv/data", "go"], "go");
        let after_one = Handoff {
            command: continuing(&started, "abc-123"),
            ..started
        };
        assert_eq!(
            continuing(&after_one, "def-456"),
            ["claude", "--add-dir", "/srv/data", "--resume=def-456"]
        );

        // Every spelling is replaced, including one typed after `--` on
        // `amx new`.
        for written in [
            &["claude", "--resume", "old", "go"][..],
            &["claude", "--resume=old", "go"],
            &["claude", "-r", "old", "go"],
        ] {
            let started = handoff(written, "go");
            assert_eq!(
                continuing(&started, "def-456"),
                ["claude", "--resume=def-456"],
                "{written:?}"
            );
        }

        // `--resume`'s value is optional, so a following flag is kept.
        let started = handoff(&["claude", "--resume", "--verbose", "go"], "go");
        assert_eq!(
            continuing(&started, "def-456"),
            ["claude", "--verbose", "--resume=def-456"]
        );
    }

    #[test]
    fn resume_drops_the_flag_naming_the_session_a_copy_was_branched_from() {
        // A pi copy's command carries the fork flag and a start flag with the
        // copy's own id. Passing both again makes pi fail with "Session
        // already exists with id".
        for written in [
            &[
                "pi",
                "--fork",
                "abc-123",
                "--session-id",
                "port-it-b2c",
                "go",
            ][..],
            &["pi", "--fork=abc-123", "--session-id=port-it-b2c", "go"],
        ] {
            let started = handoff(written, "go");
            assert_eq!(
                build_continuation(&started, "port-it-b2c", crate::registry::entry("pi")),
                ["pi", "--session-id", "port-it-b2c"],
                "{written:?}"
            );
        }
    }

    #[test]
    fn resume_leaves_a_bare_fork_marker_where_the_vendor_wrote_it() {
        // claude's `--fork-session` names no session (the origin rides on
        // `--resume`), so resume leaves it.
        let started = handoff(
            &["claude", "--resume=abc-123", "--fork-session", "go"],
            "go",
        );
        assert_eq!(
            build_continuation(&started, "def-456", crate::registry::entry("claude")),
            ["claude", "--fork-session", "--resume=def-456"]
        );
    }

    #[test]
    fn resume_hands_on_a_session_id_and_nothing_else() {
        assert!(is_session_id("6f1c9f4e-0d5b-4a51-9f6e-2b1f0c3d4e5a"));
        assert!(is_session_id("abc_123"));

        assert!(!is_session_id(""));
        assert!(!is_session_id("--dangerously-skip-permissions"));
        assert!(!is_session_id("abc 123"));
        assert!(!is_session_id("$(rm -rf /)"));
        assert!(!is_session_id("../../elsewhere"));
        assert!(!is_session_id(&"a".repeat(65)));
    }
}
