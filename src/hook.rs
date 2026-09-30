//! `amx _hook` and `amx _exit`, the commands vendors and panes run back into
//! amx.
//!
//! `_hook` is wired into the vendor's hook settings. It reads one payload on
//! stdin, appends it to the agent's event log, folds it into the agent's
//! state, and always exits 0: a failing hook would interrupt the person's
//! agent, so every failure is silent.
//!
//! - The agent is found by `AMX_ID` in the pane's environment, or, for an
//!   adopted claude that has none, by the payload's session id (see
//!   [`by_session`]).
//! - It runs on every prompt and tool call while the vendor waits, so the
//!   common path only touches the agent's directory. tmux is asked only when a
//!   stop needs a notice or errand, and to set the park timer at a turn's end.
//! - For vendors whose wire listens (amx's pi extension), it prints the
//!   record's directory on stdout; see [`hears_the_answer`].
//!
//! `_exit` runs after the vendor's command in the same pane and records how it
//! ended.

use anyhow::Result;
use serde_json::Value;
use std::borrow::Cow;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::exit;
use crate::notify::{self, Notice};
use crate::spawn::quoted;
use crate::store::{Agent, Ask, Choice, Kind, Meta, Phase, Source, State};
use crate::tmux::Server;
use crate::vendor::{Hooks, Moment};

/// The environment variable naming the agent a pane belongs to. `_boot` sets
/// it in the pane, so every process the vendor starts inherits it.
pub const ID_ENV: &str = "AMX_ID";

/// Set in a process started inside an agent that is not the agent itself.
///
/// Everything in the pane inherits [`ID_ENV`], so a claude run from an agent's
/// shell would report under the agent's id. amx sets this on the one nested
/// claude it starts itself, the `summary_command` child in [`crate::derive`].
pub const NESTED_ENV: &str = "AMX_NESTED";

/// The file claude hands a session-start hook for variables its shells should
/// get (code.claude.com/docs/en/hooks). In claude 2.1.283 it reaches every
/// shell the session runs and none of its later hooks, which is where
/// [`NESTED_ENV`] belongs.
const ENV_FILE: &str = "CLAUDE_ENV_FILE";

/// Record one hook payload. Always returns `OK`.
pub fn from_env(stdin: &mut impl Read) -> i32 {
    // Before the config is read: a nested hook has nothing to record.
    if nested() {
        return exit::OK;
    }
    let id = std::env::var(ID_ENV).ok();
    let Ok(root) = crate::paths::state_root() else {
        return exit::OK;
    };
    let env_file = std::env::var_os(ENV_FILE).map(PathBuf::from);
    run(
        id.as_deref(),
        &root,
        stdin,
        &mut std::io::stdout().lock(),
        crate::config::current(),
        env_file.as_deref(),
    )
}

/// Whether this process was started inside an agent without being the agent.
///
/// Checked before reading stdin, since a nested hook has nothing to record.
/// Any value counts.
fn nested() -> bool {
    std::env::var_os(NESTED_ENV).is_some()
}

/// [`from_env`] with its inputs passed in.
pub fn run(
    id: Option<&str>,
    root: &Path,
    stdin: &mut impl Read,
    out: &mut impl Write,
    config: &Config,
    env_file: Option<&Path>,
) -> i32 {
    // Each early return is a payload that is not amx's or a record amx cannot
    // reach. Both stay quiet: the vendor is waiting on this process.
    let mut text = String::new();
    if stdin.read_to_string(&mut text).is_err() {
        return exit::OK;
    }
    let Ok(payload) = serde_json::from_str::<Value>(&text) else {
        return exit::OK;
    };
    let Some(agent) = whose(id, root, &payload) else {
        return exit::OK;
    };

    let _ = record(root, &agent, payload, config, env_file);
    if hears_the_answer(&agent) {
        let _ = writeln!(out, "{}", agent.dir().display());
    }
    exit::OK
}

/// Whether the hook's caller reads its stdout.
///
/// amx's pi extension does, and an adopted pi has no `AMX_DIR` in its pane,
/// so [`run`] prints the record's directory for it to stream to. Vendors wired
/// through settings treat hook output as their own (claude adds a
/// `UserPromptSubmit` hook's stdout to the conversation), so they get nothing.
/// An unknown vendor is treated like claude.
fn hears_the_answer(agent: &Agent) -> bool {
    agent
        .meta()
        .ok()
        .and_then(|meta| crate::registry::entry(meta.agent.as_deref()?))
        .and_then(|vendor| vendor.hooks.as_ref())
        .is_some_and(|hooks| hooks.wire.listens())
}

/// The agent a payload belongs to.
///
/// The pane's `AMX_ID` wins. An id with no record behind it ends the search:
/// the pane named an agent that is gone.
fn whose(id: Option<&str>, root: &Path, payload: &Value) -> Option<Agent> {
    match id {
        Some(id) => Agent::open(root, id).ok(),
        None => by_session(root, payload["session_id"].as_str()?),
    }
}

/// The record for a session, for payloads with no `AMX_ID`.
///
/// A claude that `amx adopt` took over was launched outside amx, so its pane
/// has no `AMX_ID`; the session id on every payload is what ties it to the
/// record adoption wrote.
///
/// One conversation can be on two records: an agent amx started and stopped,
/// and the claude someone resumed it in and adopted. Ended records are
/// skipped, and the newest remaining one wins.
///
/// This reads every record's meta, which costs a directory listing and a small
/// file per agent. A session index would be faster but could disagree with
/// the records.
fn by_session(root: &Path, session: &str) -> Option<Agent> {
    if session.is_empty() {
        return None;
    }

    let mut ids = crate::store::list(root).ok()?;
    ids.sort();
    ids.into_iter()
        .filter_map(|id| {
            let agent = Agent::open(root, &id).ok()?;
            let meta = agent.meta().ok()?;
            (meta.session.as_deref() == Some(session)).then_some((meta.created, agent))
        })
        .filter(|(_, agent)| !agent.state().is_ok_and(|state| state.state.is_terminal()))
        .max_by_key(|(created, _)| *created)
        .map(|(_, agent)| agent)
}

/// Record how the agent's command ended.
pub fn exited_from_env(id: &str, code: i32, config: &Config) -> i32 {
    let Ok(root) = crate::paths::state_root() else {
        return exit::OK;
    };
    exited(&root, id, code, config)
}

/// [`exited_from_env`] with the state root passed in.
pub fn exited(root: &Path, id: &str, code: i32, config: &Config) -> i32 {
    let Ok(agent) = Agent::open(root, id) else {
        return exit::OK;
    };
    let _ = record_exit(&agent, code, config);
    exit::OK
}

/// Whether this payload belongs to another conversation.
///
/// A claude launched from the agent's shell inherits `AMX_ID` and reports
/// under the agent's id; its start would overwrite the session and its stop
/// would end the agent's turn. [`NESTED_ENV`] covers a claude that knows it is
/// nested; this covers one that does not, by comparing the payload's session
/// with the record's.
///
/// A resume, clear or compact changes the agent's own session, and the vendor
/// says which in `source`. So under another session, a session start is
/// foreign only when the vendor calls it a fresh start, and any other event is
/// foreign. A record with no session yet accepts every payload.
fn anothers(meta: &Meta, payload: &Value) -> bool {
    let Some(session) = payload["session_id"].as_str().filter(|it| !it.is_empty()) else {
        return false;
    };
    let Some(ours) = meta.session.as_deref() else {
        return false;
    };
    let Some(hooks) = hooks(meta) else {
        return false;
    };
    session != ours
        && (moment(&hooks, payload) != Some(Moment::Started)
            || hooks
                .fresh_start
                .is_some_and(|fresh| payload["source"] == fresh))
}

/// Fold one payload into the agent's record under the writer lock, and start
/// whatever reaching the new phase sets off.
///
/// On the agent's own session start it also marks every shell the session
/// runs as nested, via the file the vendor handed the hook. That catches a
/// `claude -c` started from one of them, which [`anothers`] cannot: it
/// continues the agent's own session under its id.
fn record(
    root: &Path,
    agent: &Agent,
    payload: Value,
    config: &Config,
    env_file: Option<&Path>,
) -> Result<()> {
    let writer = agent.writer()?;
    let mut meta = agent.meta()?;
    if anothers(&meta, &payload) {
        return Ok(());
    }
    // The appended event is also what an errand gets on stdin.
    let kind = kind(&payload).unwrap_or("unknown").to_string();
    let event = crate::store::Event::new(kind, payload);
    writer.append(&event)?;
    let payload = &event.payload;

    let mut state = writer.state()?;
    let was = state.state;
    let cut = state.interrupted_at;
    let before = meta.clone();
    let format = crate::conversation::format_of(meta.agent.as_deref().unwrap_or_default());
    let payload: &Value = &without_a_synthetic_answer(payload, &meta, format);
    let notice = apply(payload, &mut state, &mut meta);

    // claude fires nothing when a turn is interrupted, so the record is still
    // working when the next prompt arrives and the phase does not change. Close
    // the cut turn at the interrupt stamp and open the new one now, or the gap
    // would count as work. Only for the agent's own prompt, which is what
    // cleared the stamp.
    if was == Phase::Working
        && cut > 0
        && state.interrupted_at == 0
        && hooks(&meta).is_some_and(|hooks| moment(&hooks, payload) == Some(Moment::Prompted))
    {
        state.worked = state.worked_by(cut);
        state.since = crate::store::now();
    }

    // Fall back to the transcript when the payload carried no answer. Asking
    // the pane would need a tmux call on the hook path.
    if state.state == Phase::Idle
        && state.result.is_none()
        && let Some(path) = &meta.transcript
        && let Ok(text) = std::fs::read_to_string(path)
        && let Some(format) = format
        && let Some(answer) = crate::conversation::answer(format, &text)
    {
        state.result = Some(answer);
        state.source = Some(Source::Transcript);
    }

    // The session title only exists in the transcript, and the vendor writes
    // it within seconds of the first prompt, so read it on every event rather
    // than waiting for the turn to end. A transcript without a title leaves the
    // recorded one alone.
    if let Some(format) = format
        && let Some(tail) = Agent::transcript_tail(&meta)
        && let Some(title) = crate::conversation::session_title(format, &tail)
    {
        state.session_title = Some(title);
    }

    let written = writer.update_state(|current| *current = state)?;
    if meta != before {
        writer.update_meta(|current| *current = meta.clone())?;
    }
    drop(writer);

    // Notice and errand share one fork, since the vendor is waiting on this
    // hook. Idle has no notice and is handled by `after_the_write`.
    let errand = reached(written.state, was, notice.is_some())
        .filter(|phase| *phase != Phase::Idle)
        .and_then(|phase| crate::errand::assembled(config, agent, &meta, phase, &event));
    notify::post(notice.as_ref(), config.notifications, errand.as_ref());
    after_the_write(root, agent, &meta, was, &written, &event, config);

    if hooks(&meta).is_some_and(|hooks| moment(&hooks, payload) == Some(Moment::Started))
        && payload["agent_id"].is_null()
        && let Some(file) = env_file
    {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(file)?;
        writeln!(file, "export {NESTED_ENV}=1")?;
    }
    Ok(())
}

/// The payload without an answer that is really claude's synthetic note.
///
/// claude ends a turn that never reached the model (an API error, a usage
/// limit) with a synthetic transcript entry and passes its text to the
/// turn-end hook as the answer. Only the transcript tells them apart, so it is
/// read only for a turn-end payload that carries an answer.
fn without_a_synthetic_answer<'a>(
    payload: &'a Value,
    meta: &Meta,
    format: Option<crate::vendor::Transcript>,
) -> Cow<'a, Value> {
    if hooks(meta).is_some_and(|hooks| moment(&hooks, payload) == Some(Moment::Ended))
        && let Some(answer) = payload["last_assistant_message"].as_str()
        && let Some(format) = format
        && let Some(tail) = Agent::transcript_tail(meta)
        && crate::conversation::synthetic_words(format, &tail)
            .iter()
            .any(|words| words == answer)
    {
        let mut payload = payload.clone();
        if let Some(fields) = payload.as_object_mut() {
            fields.remove("last_assistant_message");
        }
        return Cow::Owned(payload);
    }
    Cow::Borrowed(payload)
}

/// Start what reaching idle sets off: the `on_idle` errand and the park timer.
///
/// Run once, by whichever process wrote the phase: the hook when the vendor
/// reported the turn end, or a reader when only the pane showed it (see
/// `hear_what_went_unsaid` in [`crate::derive`]).
///
/// The errand is started directly instead of behind the hook's fork, because a
/// reader is multithreaded and forking it is unsafe. Whether the pane is
/// watched is asked only when a command is configured.
pub fn after_the_write(
    root: &Path,
    agent: &Agent,
    meta: &Meta,
    was: Phase,
    written: &State,
    event: &crate::store::Event,
    config: &Config,
) {
    if reached(written.state, was, false) != Some(Phase::Idle) {
        return;
    }
    let server = Server::from_socket(meta.socket.clone());
    if let Some(errand) = crate::errand::assembled(config, agent, meta, Phase::Idle, event) {
        notify::start(&errand, Some(server.pane_watched(&meta.pane)));
    }

    // amx has no daemon, so the pane's tmux server is asked to run `_park` once
    // the idle timeout passes (see [`crate::verbs::park`]).
    //
    // This runs on every event, and the project config lookup involves git,
    // so the idle check comes first.
    if let Some(delay) = parks_in(written, park_after(config, meta))
        && let Some(command) = park_command(root, agent.id())
    {
        // A timer that cannot be set only means the pane is kept.
        let _ = server.run_after(delay, &command);
    }
}

/// The moment an event brought the agent to, if a command may be configured
/// for it.
///
/// A moment is a change, not a phase: waiting counts only when the event
/// earned a notice (see [`apply`], which folds the several events of one stop
/// into one), and idle only when the agent was not already idle (the idle
/// nudge repeats an ended turn). Other phases are transitions, or the command
/// ending, which [`record_exit`] reports.
fn reached(now: Phase, was: Phase, told: bool) -> Option<Phase> {
    match now {
        Phase::Waiting => told.then_some(Phase::Waiting),
        Phase::Idle => (was != Phase::Idle).then_some(Phase::Idle),
        _ => None,
    }
}

/// The idle timeout before this agent's pane is parked.
///
/// Uses the project's config over the person's, as `_park` does when the timer
/// fires; a timer set from one value and checked against another would fire
/// when `_park` will not act. A person who turned parking off keeps it off,
/// and that check reads no files.
fn park_after(config: &Config, meta: &Meta) -> u64 {
    if config.park_after == 0 {
        return 0;
    }
    crate::config::for_dir(&meta.dir).0.park_after
}

/// The park delay for this state, if it should get a timer: only when idle.
///
/// `_park` re-reads the record when it fires, so a turn that ends twice sets
/// two timers and the second finds the pane already gone.
fn parks_in(state: &State, park_after: u64) -> Option<u64> {
    (park_after > 0 && state.state == Phase::Idle).then_some(park_after)
}

/// The command the tmux server runs when the park timer fires.
///
/// `run-shell` runs in the server's environment, not this process's, so the
/// state directory is passed explicitly; otherwise tests and anyone setting
/// `$AMX_STATE_DIR` would park records in the default root. `current_exe` is
/// used because the server's `PATH` may name another amx or none.
fn park_command(root: &Path, id: &str) -> Option<String> {
    let over = root.parent().filter(|over| !over.as_os_str().is_empty())?;
    let exe = std::env::current_exe().ok()?;
    Some(format!(
        "env AMX_STATE_DIR={} {} _park {}",
        quoted(&over.to_string_lossy()),
        quoted(&exe.to_string_lossy()),
        quoted(id),
    ))
}

/// Record how the command ended, and notify if it is worth it.
fn record_exit(agent: &Agent, code: i32, config: &Config) -> Result<()> {
    let writer = agent.writer()?;
    let event = crate::store::Event::new("exit", serde_json::json!({ "code": code }));
    writer.append(&event)?;

    let state = writer.update_state_heard(agent.heartbeat(), |state| {
        state.exit = Some(code);
        // The pane is gone, so a pending question can never be answered, and
        // it would otherwise sit in front of the agent's answer in every
        // reader.
        state.asks(None);
        // A stopped agent exits with a signal's code moments later; it stays
        // stopped.
        if state.state != Phase::Stopped {
            state.state = if code == 0 {
                Phase::Done
            } else {
                Phase::Failed
            };
        }
    })?;
    drop(writer);

    let notice = Notice::finished(agent.id(), state.state, state.exit);

    // Run the errand only for a phase this exit wrote. A stopped agent keeps
    // `stop`'s phase, and `stop` already ran that errand.
    let errand = (state.state != Phase::Stopped)
        .then(|| agent.meta().ok())
        .flatten()
        .and_then(|meta| crate::errand::assembled(config, agent, &meta, state.state, &event));
    notify::post(notice.as_ref(), config.notifications, errand.as_ref());
    Ok(())
}

/// The payload's event name as the vendor spells it; what the event log
/// keeps.
fn kind(payload: &Value) -> Option<&str> {
    payload["hook_event_name"].as_str()
}

/// The hook table of the record's vendor (see [`crate::vendor::hooks_for`]).
fn hooks(meta: &Meta) -> Option<Hooks> {
    crate::vendor::hooks_for(meta.agent.as_deref().unwrap_or_default())
}

/// The moment a payload reports, if `hooks` maps its event.
fn moment(hooks: &Hooks, payload: &Value) -> Option<Moment> {
    hooks.moment(kind(payload)?)
}

/// Whether a payload is about the question tool, which draws a menu and waits
/// instead of doing work.
///
/// In claude 2.1.240 one question fires three events for one screen: the tool
/// call draws the menu, a permission event naming the same tool follows 10 to
/// 30 ms later, and a notification about six seconds after that.
fn menu(hooks: &Hooks, payload: &Value) -> bool {
    payload["tool_name"] == hooks.question_tool
}

/// Whether the notification's type is `what`.
fn typed(payload: &Value, what: &str) -> bool {
    payload["notification_type"] == what
}

/// Whether a prompt was typed into the session by the vendor itself (see
/// [`crate::vendor::Hooks`]'s `injected`).
fn injected(hooks: &Hooks, payload: &Value) -> bool {
    payload["prompt"]
        .as_str()
        .is_some_and(|prompt| hooks.injected.iter().any(|tag| prompt.starts_with(tag)))
}

/// Background shells and other tasks the vendor reports still running.
///
/// The list includes finished tasks, so only those marked running count. A
/// task of type `shell` is a shell; anything else is an agent it started.
fn running(payload: &Value) -> (u32, u32) {
    let Some(tasks) = payload["background_tasks"].as_array() else {
        return (0, 0);
    };
    tasks
        .iter()
        .filter(|task| task["status"] == "running")
        .fold((0, 0), |(shells, agents), task| {
            if task["type"] == "shell" {
                (shells + 1, agents)
            } else {
                (shells, agents + 1)
            }
        })
}

/// The summary line for tasks still running.
fn still_running(shells: u32, agents: u32) -> String {
    let counted = |n: u32, one: &str| match n {
        1 => format!("1 {one}"),
        many => format!("{many} {one}s"),
    };
    match (shells, agents) {
        (shells, 0) => format!("{} running", counted(shells, "shell")),
        (0, agents) => format!("{} running", counted(agents, "agent")),
        (shells, agents) => format!(
            "{} and {} running",
            counted(shells, "shell"),
            counted(agents, "agent")
        ),
    }
}

/// Fold one payload into the state and meta.
///
/// Events are read through the record vendor's own hook table:
///
/// - [`Started`](Moment::Started) records the session id and transcript. Only
///   this moment sets the session, since subagents' payloads carry their own.
/// - [`Taken`](Moment::Taken) is a queued message entering the running turn;
///   it changes nothing here and is logged for `send` and the card.
/// - [`Prompted`](Moment::Prompted) and [`Calling`](Moment::Calling) mean
///   working, except a [`menu`] call, which waits and carries every question.
/// - [`Asked`](Moment::Asked) is a permission box going up, unless it names the
///   menu tool. [`Refused`](Moment::Refused) is the only sign the box closed
///   with the tool refused.
/// - [`Notified`](Moment::Notified) means stopped on a question, with its
///   words. The choices are on the pane, filled in later by a reader. The
///   notification type is the only thing that names a permission prompt, and
///   the only thing that tells the idle nudge from a question.
/// - [`Ended`](Moment::Ended) ends the turn and carries the freshest answer
///   (the transcript lags). If it lists background tasks still running, the
///   agent stays working; see [`crate::store::State`]'s `background`.
/// - A subagent's payload (`agent_id` set) and any payload for an ended record
///   change nothing.
/// - Any event about the agent clears the interrupt and park stamps, since
///   the vendor spoke.
///
/// Returns the notice this stop is worth, once per stop. A menu fires three
/// events (the call, a permission event for the same tool, a notification),
/// so the record's phase is what folds them into one notice. A menu counts as
/// a new stop even when the record already reads waiting, because nothing amx
/// installs fires when a box is approved.
///
/// The call knows the question best and arrives first, so the later two must
/// not overwrite it: a permission event for the menu tool, and a notification
/// while a call is pending, leave the question as the call set it. A permission box cannot be up over a menu: the box's own tool call
/// would have retired the menu's call first.
pub fn apply(payload: &Value, state: &mut State, meta: &mut Meta) -> Option<Notice> {
    apply_in(&hooks(meta)?, payload, state, meta)
}

/// [`apply`], read through `hooks`.
fn apply_in(hooks: &Hooks, payload: &Value, state: &mut State, meta: &mut Meta) -> Option<Notice> {
    if !payload["agent_id"].is_null() || state.state.is_terminal() {
        return None;
    }
    // The vendor spoke, so the interrupt stamp comes off regardless of the
    // event (see [`crate::derive::cut_short`]). Comparing timestamps does not
    // work: both are whole seconds and tie often.
    state.interrupted_at = 0;
    // A vendor that speaks has a pane; `send` refuses a record still marked
    // parked.
    state.parked_at = 0;

    // An adopted record has the session but not the transcript, whose path
    // was announced before the record existed. Take it from the first report
    // about the record's own session.
    if meta.transcript.is_none()
        && let Some(session) = meta.session.as_deref()
        && payload["session_id"].as_str() == Some(session)
        && let Some(transcript) = payload["transcript_path"].as_str()
    {
        meta.transcript = Some(transcript.into());
    }
    let was_waiting = state.state == Phase::Waiting;

    let screen = match moment(hooks, payload)? {
        Moment::Started => {
            // `/clear` starts a new session in the same pane; nothing from the
            // old one is its answer.
            if payload["source"] == "clear"
                && let Some(session) = payload["session_id"].as_str()
                && meta.session.as_deref() != Some(session)
            {
                *state = State {
                    state: state.state,
                    since: state.since,
                    ..state.for_a_new_session()
                };
            }
            if let Some(session) = payload["session_id"].as_str() {
                meta.session = Some(session.to_string());
            }
            if let Some(transcript) = payload["transcript_path"].as_str() {
                meta.transcript = Some(transcript.into());
            }
            // `amx resume` with no message: the vendor restores the session and
            // waits at its prompt, and no turn will follow. Without this the
            // record would sit at `starting` until a reader recognised the
            // prompt, which a custom footer can hide.
            if std::mem::take(&mut state.opens_idle) {
                state.state = Phase::Idle;
            }
            Screen::Clear
        }

        Moment::Prompted => {
            state.state = Phase::Working;
            state.turn_open = true;
            // Anything an interrupt put back in the composer went out with this
            // prompt or was cleared.
            state.composer_holds.clear();
            state.summary = None;
            state.asks(None);
            // The shell count belonged to the previous turn.
            state.background = 0;
            // A new turn clears the last answer, so a turn that ends without
            // one does not hand the old answer to `result`. A prompt the vendor
            // injected itself keeps it.
            if !injected(hooks, payload) {
                state.result = None;
                state.source = None;
            }
            Screen::Clear
        }

        Moment::Taken => Screen::Clear,

        Moment::Calling if menu(hooks, payload) => {
            state.state = Phase::Waiting;
            // The menu is what it is doing.
            state.summary = None;
            state.background = 0;
            state.asks_all(asked(&payload["tool_input"]));
            state.kind = Some(Kind::Question);
            // Nothing else will report the wait: the vendor sends its idle
            // notice only when nothing is open, and a menu is.
            Screen::Fresh
        }

        Moment::Calling => {
            state.state = Phase::Working;
            state.background = 0;
            if let Some(tool) = payload["tool_name"].as_str() {
                state.summary = Some(format!("Running {tool}"));
            }
            state.asks(None);
            Screen::Clear
        }

        // The vendor asking itself to draw the menu. There is no box: the
        // screen is the menu, drawn by the call 10 to 30 ms earlier, which
        // carried every question. So this only says the agent stopped;
        // writing the permission sentence would have dropped the call.
        //
        // Its `tool_input` is the call's own (claude 2.1.240). When amx missed
        // the call (a hook wired mid-turn, a hook process that died), this is
        // the only other copy of the questions, so a record without a call
        // takes it.
        Moment::Asked if menu(hooks, payload) => {
            state.state = Phase::Waiting;
            state.summary = None;
            if state.pending().is_none() {
                state.asks_all(asked(&payload["tool_input"]));
            }
            // Whatever amx missed, this screen is a question, and the kind
            // decides what may be sent back.
            state.kind = Some(Kind::Question);
            // `Waiting`, not `Fresh`: this is the same menu, announced again
            // right after the call. It is news only to a record that missed the
            // call.
            Screen::Waiting
        }

        // Fired as the box goes up, about six seconds before the notification
        // that repeats it. The payload names the tool but not the sentence, so
        // the vendor's sentence is built from the tool name; with no tool
        // named, a reader quotes the pane later.
        Moment::Asked => {
            state.state = Phase::Waiting;
            state.summary = None;
            state.asks(
                payload["tool_name"]
                    .as_str()
                    .and_then(|tool| hooks.permission_sentence(tool)),
            );
            state.kind = Some(Kind::Permission);
            Screen::Waiting
        }

        // The only hook saying the box closed without the tool running. The
        // turn continues, and the next call will say what it is doing. A
        // prompt raised outside a turn (pi does this for extensions) closes
        // back to idle.
        Moment::Refused => {
            state.state = match state.turn_open {
                true => Phase::Working,
                false => Phase::Idle,
            };
            state.summary = None;
            state.asks(None);
            Screen::Clear
        }

        // The vendor's idle nudge comes only when nothing is open: the turn
        // ended, said again later. Its words are not a question, and nothing
        // amx thought was outstanding is on screen.
        //
        // After a turn that left shells running, the nudge only means the
        // model is idle, so nothing moves; otherwise it would flip the record
        // to idle while the shells still run.
        Moment::Notified if typed(payload, hooks.idle_notice) && state.background > 0 => {
            return None;
        }

        Moment::Notified if typed(payload, hooks.idle_notice) => {
            // Already idle, the nudge repeats that the turn ended, and the
            // finished turn's line stays: it is asked for once per turn.
            if state.state != Phase::Idle {
                state.summary = None;
            }
            state.state = Phase::Idle;
            state.asks(None);
            Screen::Clear
        }

        // With a call pending, the menu is what is on screen, and this notice
        // says less about it (claude 2.1.240 sends only "Claude needs your
        // permission"). A box cannot be up over the menu, so keep the call's
        // question.
        Moment::Notified if state.pending().is_some() => {
            state.state = Phase::Waiting;
            Screen::Waiting
        }

        Moment::Notified => {
            // An untyped notification after a turn that ended with an answer
            // can only be an older vendor's idle nudge: nothing is open after
            // an answered turn. Taking its words as a question would flip the
            // record back to waiting.
            //
            // Unless the vendor names a question kind beside the notice.
            let question = payload["kind"]
                .as_str()
                .is_some_and(|kind| hooks.question_kinds.contains(&kind));
            if !question
                && state.state == Phase::Idle
                && state.result.is_some()
                && payload["notification_type"].is_null()
            {
                return None;
            }
            state.state = Phase::Waiting;
            state.asks(payload["message"].as_str().map(str::to_string));
            if typed(payload, hooks.permission_notice) {
                state.kind = Some(Kind::Permission);
            } else if question {
                state.kind = Some(Kind::Question);
            }
            Screen::Waiting
        }

        Moment::Ended => {
            state.turn_open = false;
            state.asks(None);
            let (shells, agents) = running(payload);
            state.background = shells + agents;
            // The model finished but its tasks have not. Callers wait on the
            // turn and the park timer ends it, and both are about the agent:
            // `_park` once took a pane whose shells were still running ten
            // minutes after the stop. So the agent stays working.
            if state.background > 0 {
                state.state = Phase::Working;
                state.summary = Some(still_running(shells, agents));
            } else {
                state.state = Phase::Idle;
                state.summary = None;
            }
            // Keep the answer whether or not tasks remain, but not a cut-off
            // or failed one, and not over an answer an injected turn kept.
            if let Some(answer) = payload["last_assistant_message"].as_str()
                && !matches!(payload["stop_reason"].as_str(), Some("aborted" | "error"))
                && state.result.is_none()
            {
                state.result = Some(answer.to_string());
                state.source = Some(Source::Payload);
            }
            Screen::Clear
        }
    };

    // One stop, one notice. A box or notification arriving while already
    // waiting has been reported; a menu is always a new stop.
    let told = match screen {
        Screen::Fresh => true,
        Screen::Waiting => !was_waiting,
        Screen::Clear => false,
    };
    told.then(|| Notice::waiting(&meta.id, state.question.as_deref()))
}

/// What an event leaves on the agent's pane.
enum Screen {
    /// Nothing to answer: the agent is working, or nothing changed.
    Clear,
    /// Something to answer, possibly already on screen (the box over a known
    /// call, or the notification repeating a box). The previous phase says
    /// whether it is news.
    Waiting,
    /// Something new to answer. A menu is always this: its call fires before
    /// anything asks permission, so whatever the record was waiting on before
    /// is behind it.
    Fresh,
}

/// Every question a [`menu`] call is about to show.
///
/// The tool takes up to four questions, shown as tabs one at a time. Headers,
/// descriptions, previews and multi-select flags exist only in the payload,
/// and a narrow pane elides even the tab count. Questions with no text are
/// skipped.
fn asked(input: &Value) -> Vec<Ask> {
    let Some(questions) = input["questions"].as_array() else {
        return Vec::new();
    };
    questions
        .iter()
        .filter_map(|question| {
            Some(Ask {
                header: question["header"].as_str().map(str::to_string),
                text: question["question"].as_str()?.to_string(),
                options: choices(&question["options"]),
                multi: question["multiSelect"] == true,
                answer: None,
            })
        })
        .collect()
}

/// The choices under one question. The label is what an answer names and
/// returns; the description only explains it.
fn choices(options: &Value) -> Vec<Choice> {
    let Some(options) = options.as_array() else {
        return Vec::new();
    };
    options
        .iter()
        .filter_map(|option| {
            Some(Choice {
                label: option["label"].as_str()?.to_string(),
                description: option["description"].as_str().map(str::to_string),
                preview: option["preview"].as_str().map(str::to_string),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Meta;
    use crate::tmux::{PaneId, Socket};
    use crate::vendor::claude;
    use serde_json::json;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn meta() -> Meta {
        Meta {
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
            session: None,
            transcript: None,
            created: 1,
        }
    }

    /// A config that sends no notices and sets no timers. A timer would be a
    /// real tmux call against whatever server the record names.
    fn quiet() -> Config {
        Config {
            notifications: crate::config::Delivery::Off,
            park_after: 0,
            ..Config::default()
        }
    }

    /// claude's event name for a moment.
    fn named(moment: Moment) -> &'static str {
        claude::HOOKS
            .events
            .iter()
            .find(|wiring| wiring.moment == moment)
            .expect("the vendor names every moment")
            .event
    }

    /// Fold a payload into a fresh record and return the result.
    fn fold(payload: Value) -> (State, Meta, Option<Notice>) {
        let mut state = State::default();
        let mut meta = meta();
        let notice = apply(&payload, &mut state, &mut meta);
        (state, meta, notice)
    }

    #[test]
    fn hook_a_payload_is_read_by_the_moment_the_vendor_named_it() {
        // Event names come from the vendor's entry, never spelled here, so a
        // vendor rename cannot leave this file silently reading the old name.
        for wiring in claude::HOOKS.events {
            assert_eq!(
                moment(&claude::HOOKS, &json!({ "hook_event_name": wiring.event })),
                Some(wiring.moment),
                "{}",
                wiring.event
            );
        }
        assert_eq!(
            moment(&claude::HOOKS, &json!({ "hook_event_name": "PostToolUse" })),
            None
        );
        assert_eq!(moment(&claude::HOOKS, &json!({})), None);
    }

    #[test]
    fn hook_the_menu_is_the_tool_the_vendor_says_draws_one() {
        // Which tool draws the menu is the vendor's word; every other tool is
        // work.
        let (state, _, _) = fold(json!({
            "hook_event_name": named(Moment::Calling),
            "tool_name": claude::HOOKS.question_tool,
            "tool_input": { "questions": [{ "question": "Which fixture?", "options": [] }] }
        }));
        assert_eq!(state.state, Phase::Waiting);
        assert_eq!(state.kind, Some(Kind::Question));
        assert_eq!(state.summary, None, "a menu is not a tool running");
    }

    #[test]
    fn hook_a_notification_means_what_the_vendor_typed_it() {
        // The notification types that tell an ended turn from a question come
        // from the vendor's entry.
        let (idle, _, _) = fold(json!({
            "hook_event_name": named(Moment::Notified),
            "message": "Claude is waiting for your input",
            "notification_type": claude::HOOKS.idle_notice
        }));
        assert_eq!(idle.state, Phase::Idle);
        assert_eq!(idle.question, None, "nothing is open on the session");

        let (box_, _, _) = fold(json!({
            "hook_event_name": named(Moment::Notified),
            "message": "Claude needs your permission to use Bash",
            "notification_type": claude::HOOKS.permission_notice
        }));
        assert_eq!(box_.state, Phase::Waiting);
        assert_eq!(box_.kind, Some(Kind::Permission));
    }

    /// Fold a payload into a fresh record, read through `hooks`.
    fn fold_in(hooks: &Hooks, payload: Value) -> State {
        let mut state = State::default();
        apply_in(hooks, &payload, &mut state, &mut meta());
        state
    }

    /// `hooks`' event name for a moment.
    fn named_in(hooks: &Hooks, moment: Moment) -> &'static str {
        hooks
            .events
            .iter()
            .find(|wiring| wiring.moment == moment)
            .expect("the vendor names the moment")
            .event
    }

    #[test]
    fn hook_under_another_vendors_words_claudes_notice_types_are_only_notices() {
        // Notification types belong to the vendor that sent them. Under the
        // second vendor, claude's idle type is an ordinary notice.
        let second = &crate::vendor::second::HOOKS;
        let idle = fold_in(
            second,
            json!({
                "hook_event_name": named_in(second, Moment::Notified),
                "message": "Claude is waiting for your input",
                "notification_type": claude::HOOKS.idle_notice
            }),
        );
        assert_eq!(idle.state, Phase::Waiting);
        let box_ = fold_in(
            second,
            json!({
                "hook_event_name": named_in(second, Moment::Notified),
                "message": "Claude needs your permission to use Bash",
                "notification_type": claude::HOOKS.permission_notice
            }),
        );
        assert_ne!(box_.kind, Some(Kind::Permission));
        let own = fold_in(
            second,
            json!({
                "hook_event_name": named_in(second, Moment::Notified),
                "notification_type": second.idle_notice
            }),
        );
        assert_eq!(own.state, Phase::Idle);
    }

    #[test]
    fn hook_under_another_vendors_words_claudes_question_tool_is_work() {
        let second = &crate::vendor::second::HOOKS;
        let state = fold_in(
            second,
            json!({
                "hook_event_name": named_in(second, Moment::Calling),
                "tool_name": claude::HOOKS.question_tool,
                "tool_input": { "questions": [{ "question": "Which fixture?", "options": [] }] }
            }),
        );
        assert_eq!(state.state, Phase::Working);
        assert_eq!(state.kind, None);
        let menu = fold_in(
            second,
            json!({
                "hook_event_name": named_in(second, Moment::Calling),
                "tool_name": second.question_tool,
                "tool_input": { "questions": [{ "question": "Which fixture?", "options": [] }] }
            }),
        );
        assert_eq!(menu.kind, Some(Kind::Question));
    }

    #[test]
    fn hook_a_permission_box_is_worded_by_the_vendor_that_drew_it() {
        let second = &crate::vendor::second::HOOKS;
        let state = fold_in(
            second,
            json!({ "hook_event_name": named_in(second, Moment::Asked), "tool_name": "Bash" }),
        );
        assert_eq!(
            state.question.as_deref(),
            Some("second may not run Bash yet")
        );
        assert_eq!(state.kind, Some(Kind::Permission));
    }

    #[test]
    fn hook_a_pi_input_notice_is_a_question_from_the_hook_alone() {
        // pi names what it drew beside the notice, and each kind is something
        // the person answers, so no screen reading is needed.
        let mut state = State::default();
        let mut meta = Meta {
            agent: Some("pi".to_string()),
            ..meta()
        };
        let notice = apply(
            &json!({
                "hook_event_name": "ui_prompt_start",
                "kind": "input",
                "message": "Name the branch"
            }),
            &mut state,
            &mut meta,
        );
        assert_eq!(state.state, Phase::Waiting);
        assert_eq!(state.kind, Some(Kind::Question));
        assert_eq!(state.question.as_deref(), Some("Name the branch"));
        assert!(notice.is_some());
    }

    #[test]
    fn hook_a_pi_record_hears_nothing_in_claudes_words() {
        // claude's event names mean nothing on a pi record.
        let mut meta = Meta {
            agent: Some("pi".to_string()),
            ..meta()
        };
        for wiring in claude::HOOKS.events {
            let mut state = State::default();
            let payload = json!({ "hook_event_name": wiring.event, "tool_name": "Bash" });
            assert!(apply(&payload, &mut state, &mut meta).is_none());
            assert_eq!(state, State::default(), "{}", wiring.event);
        }
    }

    #[test]
    fn hook_a_fresh_start_is_the_vendors_own_word_for_one() {
        // claude's `startup` under another session is another process. pi has
        // no such word, so no pi session start is foreign.
        let payload = json!({
            "session_id": "nested",
            "hook_event_name": "SessionStart",
            "source": "startup"
        });
        let claude = Meta {
            session: Some("ours".to_string()),
            ..meta()
        };
        assert!(anothers(&claude, &payload));
        let pi = Meta {
            agent: Some("pi".to_string()),
            ..claude.clone()
        };
        let opening = json!({
            "session_id": "nested",
            "hook_event_name": "session_start",
            "source": "startup"
        });
        assert!(!anothers(&pi, &opening));
    }

    #[test]
    fn hook_a_prompt_starts_a_turn() {
        let (state, _, notice) = fold(json!({
            "session_id": "abc-123",
            "hook_event_name": "UserPromptSubmit",
            "prompt": "fix the login bug",
            "prompt_id": "p1"
        }));
        assert_eq!(state.state, Phase::Working);
        assert_eq!(state.question, None, "a new turn answers the old question");
        assert_eq!(notice, None, "a turn starting is not worth an interruption");
    }

    #[test]
    fn hook_a_prompt_empties_the_composer_an_interrupt_filled() {
        // pi put queued sends back in its composer on interrupt, and `send`
        // refuses while they sit there. A prompt submits the composer.
        let mut state = State {
            state: Phase::Idle,
            composer_holds: vec!["and the linter".to_string()],
            ..State::default()
        };
        let mut meta = Meta {
            agent: Some("pi".to_string()),
            ..meta()
        };

        apply(
            &json!({ "hook_event_name": "agent_start" }),
            &mut state,
            &mut meta,
        );
        assert_eq!(state.state, Phase::Working);
        assert_eq!(state.composer_holds, Vec::<String>::new());
    }

    #[test]
    fn hook_a_new_turn_retires_the_last_turns_answer() {
        // Otherwise a turn that ends without an answer would leave the last
        // one on the record for `result`.
        let mut state = State {
            state: Phase::Idle,
            result: Some("the tests pass now".to_string()),
            source: Some(Source::Payload),
            ..State::default()
        };
        let mut meta = meta();

        apply(
            &json!({ "hook_event_name": "UserPromptSubmit", "prompt": "and now the linter" }),
            &mut state,
            &mut meta,
        );
        assert_eq!(state.state, Phase::Working);
        assert_eq!(state.result, None);
        assert_eq!(state.source, None);
    }

    #[test]
    fn hook_a_tool_call_says_what_the_agent_is_doing() {
        let (state, _, _) = fold(json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": "cargo test" }
        }));
        assert_eq!(state.state, Phase::Working);
        assert_eq!(state.summary.as_deref(), Some("Running Bash"));
    }

    #[test]
    fn hook_a_notification_is_a_question_and_an_interruption() {
        let (state, _, notice) = fold(json!({
            "hook_event_name": "Notification",
            "message": "Claude needs your permission to use Bash"
        }));
        assert_eq!(state.state, Phase::Waiting);
        assert_eq!(
            state.question.as_deref(),
            Some("Claude needs your permission to use Bash")
        );
        let notice = notice.expect("somebody has to be told");
        assert!(notice.title.contains("fix-login-a1b"));
    }

    #[test]
    fn hook_the_choices_go_wherever_the_question_they_answer_goes() {
        // Options belong to the question they were read under; under the next
        // question they would be the wrong keys.
        let asked = State {
            state: Phase::Waiting,
            question: Some("Do you want to proceed?".to_string()),
            options: vec!["Yes".to_string(), "No".to_string()],
            ..State::default()
        };

        for payload in [
            json!({ "hook_event_name": "Notification", "message": "Claude needs your permission to use Write" }),
            json!({ "hook_event_name": "PreToolUse", "tool_name": "Bash" }),
            json!({ "hook_event_name": "UserPromptSubmit", "prompt": "carry on" }),
            json!({ "hook_event_name": "Stop", "last_assistant_message": "done" }),
        ] {
            let mut state = asked.clone();
            apply(&payload, &mut state, &mut meta());
            assert!(state.options.is_empty(), "{payload}");
        }
    }

    #[test]
    fn hook_the_vendor_names_the_kind_of_prompt_it_is_notifying_about() {
        // claude 2.1.237's permission notification carries
        // `notification_type: permission_prompt`. The message alone reads like
        // any other notice.
        let (state, _, notice) = fold(json!({
            "hook_event_name": "Notification",
            "message": "Claude needs your permission to use Bash",
            "notification_type": "permission_prompt"
        }));
        assert_eq!(state.state, Phase::Waiting);
        assert_eq!(state.kind, Some(Kind::Permission));
        assert_eq!(
            state.question.as_deref(),
            Some("Claude needs your permission to use Bash")
        );
        assert!(notice.is_some(), "somebody has to be told");
    }

    #[test]
    fn hook_a_permission_request_is_the_box_the_instant_it_goes_up() {
        // In claude 2.1.237, PermissionRequest fires as the box goes up, about
        // six seconds before the notification. It names the tool but not the
        // sentence, so the sentence is built as the vendor builds it.
        let (state, _, notice) = fold(json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "Bash",
            "tool_input": { "command": "rm -rf build" },
            "permission_suggestions": []
        }));
        assert_eq!(state.state, Phase::Waiting);
        assert_eq!(state.kind, Some(Kind::Permission));
        assert_eq!(
            state.question.as_deref(),
            Some("Claude needs your permission to use Bash")
        );
        assert_eq!(state.summary, None, "a box is not a tool running");
        assert!(
            notice.is_some(),
            "the six-second blind window was the point"
        );

        // A box with no tool named is still a permission box; a reader fills
        // in the pane's words.
        let (state, _, _) = fold(json!({ "hook_event_name": "PermissionRequest" }));
        assert_eq!(state.state, Phase::Waiting);
        assert_eq!(state.kind, Some(Kind::Permission));
        assert_eq!(state.question, None);
    }

    #[test]
    fn hook_the_notification_repeating_a_known_box_interrupts_nobody_twice() {
        let mut state = State::default();
        let mut meta = meta();
        let told = apply(
            &json!({ "hook_event_name": "PermissionRequest", "tool_name": "Bash" }),
            &mut state,
            &mut meta,
        );
        assert!(told.is_some());

        let again = apply(
            &json!({
                "hook_event_name": "Notification",
                "message": "Claude needs your permission to use Bash",
                "notification_type": "permission_prompt"
            }),
            &mut state,
            &mut meta,
        );
        assert_eq!(state.state, Phase::Waiting);
        assert_eq!(state.kind, Some(Kind::Permission));
        assert_eq!(again, None, "one box, one interruption");
    }

    #[test]
    fn hook_the_menu_decides_what_answers_it() {
        // claude 2.1.240, manual and auto mode: one AskUserQuestion fires three
        // events, and the two knowing least arrive last. The call carries every
        // question, the permission event only the tool name, and the
        // notification six seconds later only "Claude needs your permission".
        // Each used to overwrite the call's question, so the card offered a
        // permission box's keys over a menu.
        let mut state = State::default();
        let mut meta = meta();

        apply(
            &json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "AskUserQuestion",
                "tool_input": { "questions": [{
                    "question": "Which features should be enabled?",
                    "header": "Features",
                    "options": [
                        { "label": "Logging", "description": "Write a log file" },
                        { "label": "Metrics", "description": "Export counters" },
                        { "label": "Tracing", "description": "Emit spans" }
                    ],
                    "multiSelect": true
                }] }
            }),
            &mut state,
            &mut meta,
        );

        for payload in [
            json!({
                "hook_event_name": "PermissionRequest",
                "tool_name": "AskUserQuestion",
                "tool_input": { "questions": [] }
            }),
            json!({
                "hook_event_name": "Notification",
                "message": "Claude needs your permission",
                "notification_type": "permission_prompt"
            }),
        ] {
            apply(&payload, &mut state, &mut meta);
            assert_eq!(state.state, Phase::Waiting, "{payload}");
            assert_eq!(
                state.kind,
                Some(Kind::Question),
                "the screen is the menu the whole time: {payload}"
            );
            assert_eq!(
                state.question.as_deref(),
                Some("Which features should be enabled?"),
                "{payload}"
            );
            assert_eq!(
                state.options,
                ["Logging", "Metrics", "Tracing"],
                "{payload}"
            );
            assert!(state.multi(), "and it still takes more than one: {payload}");
        }
    }

    #[test]
    fn hook_the_permission_box_over_a_menu_carries_the_menu() {
        // In claude 2.1.240 the permission event for the question tool carries
        // the call's `tool_input` verbatim. Usually the call arrived 10 ms
        // earlier; when amx missed it, this is the only other copy.
        let (state, _, _) = fold(json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "AskUserQuestion",
            "permission_mode": "default",
            "tool_input": { "questions": [{
                "question": "Which fixture should the port keep?",
                "header": "Fixture",
                "options": [{ "label": "SQLite" }, { "label": "Docker" }],
                "multiSelect": false
            }] }
        }));
        assert_eq!(state.state, Phase::Waiting);
        assert_eq!(state.kind, Some(Kind::Question));
        assert_eq!(
            state.question.as_deref(),
            Some("Which fixture should the port keep?")
        );
        assert_eq!(state.options, ["SQLite", "Docker"]);

        // A record that has the call keeps it.
        let mut state = State::default();
        let mut meta = meta();
        apply(
            &json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "AskUserQuestion",
                "tool_input": { "questions": [{
                    "question": "Which fixture should the port keep?",
                    "options": [{ "label": "the sqlite one" }]
                }] }
            }),
            &mut state,
            &mut meta,
        );
        apply(
            &json!({
                "hook_event_name": "PermissionRequest",
                "tool_name": "AskUserQuestion",
                "tool_input": { "questions": [] }
            }),
            &mut state,
            &mut meta,
        );
        assert_eq!(state.options, ["the sqlite one"]);
    }

    #[test]
    fn hook_the_leave_to_draw_a_menu_is_a_stop_nobody_has_heard_of_yet() {
        // Normally the call 10 ms earlier already raised the notice and this
        // event adds none. When amx missed the call (a hook wired mid-turn, a
        // hook process that died), this is the first word of the menu, and its
        // questions are the notice.
        let mut state = State::default();
        let mut meta = meta();
        let told = apply(
            &json!({
                "hook_event_name": "PermissionRequest",
                "tool_name": "AskUserQuestion",
                "tool_input": { "questions": [{
                    "question": "Which fixture should the port keep?",
                    "header": "Fixture",
                    "options": [{ "label": "SQLite" }, { "label": "Docker" }],
                    "multiSelect": false
                }] }
            }),
            &mut state,
            &mut meta,
        )
        .expect("a menu nothing has mentioned is a stop somebody has to hear about");
        assert_eq!(told.body, "Which fixture should the port keep?");

        // The notification six seconds later repeats the same screen.
        let again = apply(
            &json!({
                "hook_event_name": "Notification",
                "message": "Claude needs your permission",
                "notification_type": "permission_prompt"
            }),
            &mut state,
            &mut meta,
        );
        assert_eq!(again, None, "one stop, one interruption");
        assert_eq!(state.state, Phase::Waiting);
        assert_eq!(state.kind, Some(Kind::Question), "and it is still the menu");
    }

    #[test]
    fn hook_a_box_over_another_tool_is_still_a_box() {
        // A permission box after a menu is a new stop: the box's own tool call
        // comes first and retires the menu's call.
        let mut state = State::default();
        let mut meta = meta();

        apply(
            &json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "AskUserQuestion",
                "tool_input": { "questions": [{
                    "question": "Which fixture should the port keep?",
                    "options": [{ "label": "the sqlite one" }]
                }] }
            }),
            &mut state,
            &mut meta,
        );
        assert_eq!(state.kind, Some(Kind::Question));

        for payload in [
            json!({ "hook_event_name": "PreToolUse", "tool_name": "Bash" }),
            json!({ "hook_event_name": "PermissionRequest", "tool_name": "Bash" }),
        ] {
            apply(&payload, &mut state, &mut meta);
        }
        assert_eq!(state.state, Phase::Waiting);
        assert_eq!(state.kind, Some(Kind::Permission));
        assert_eq!(
            state.question.as_deref(),
            Some("Claude needs your permission to use Bash")
        );
        assert!(state.asking.is_empty(), "and the menu is gone");
    }

    #[test]
    fn hook_one_stop_interrupts_once_however_many_events_say_so() {
        // One AskUserQuestion used to raise three desktop notifications. The
        // three events carry different sentences, so matching words cannot
        // dedupe them; what they share is that the agent had already stopped.
        let mut state = State::default();
        let mut meta = meta();

        let told = apply(
            &json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "AskUserQuestion",
                "tool_input": { "questions": [{
                    "question": "Which fixture should the port keep?",
                    "options": [{ "label": "the sqlite one" }]
                }] }
            }),
            &mut state,
            &mut meta,
        )
        .expect("the transition into waiting is the interruption");
        assert_eq!(told.body, "Which fixture should the port keep?");

        for payload in [
            json!({ "hook_event_name": "PermissionRequest", "tool_name": "AskUserQuestion" }),
            json!({
                "hook_event_name": "Notification",
                "message": "Claude needs your permission to use Ask User Question",
                "notification_type": "permission_prompt"
            }),
            json!({
                "hook_event_name": "Notification",
                "message": "Claude is waiting for your input"
            }),
        ] {
            assert_eq!(
                apply(&payload, &mut state, &mut meta),
                None,
                "one stop, one interruption: {payload}"
            );
            assert_eq!(state.state, Phase::Waiting, "{payload}");
        }
    }

    #[test]
    fn hook_coherence_the_question_that_notified_three_times() {
        // Payloads captured from claude 2.1.240 in default permission mode,
        // with a hook on every event logging its stdin. One AskUserQuestion
        // fired the call, the permission event for the same tool, and the
        // notification, in that order.
        //
        // The notification names no tool, so no reading of the words tells one
        // stop from three. The last two must also leave the call's question
        // alone.
        let asking = json!({ "questions": [{
            "question": "Which fixture should the port keep?",
            "header": "Fixture choice",
            "options": [
                { "label": "SQLite fixture", "description": "Use the SQLite-based test fixture" },
                { "label": "Docker fixture", "description": "Use the Docker-based test fixture" }
            ],
            "multiSelect": false
        }] });

        let mut state = State::default();
        let mut meta = meta();
        let told: Vec<Notice> = [
            json!({ "hook_event_name": "SessionStart", "source": "startup" }),
            json!({
                "hook_event_name": "UserPromptSubmit",
                "prompt": "Call the AskUserQuestion tool right now"
            }),
            json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "AskUserQuestion",
                "tool_input": asking,
                "tool_use_id": "toolu_01LREQkFsYwfpQu1XpSPa6rp"
            }),
            json!({
                "hook_event_name": "PermissionRequest",
                "tool_name": "AskUserQuestion",
                "tool_input": asking
            }),
            json!({
                "hook_event_name": "Notification",
                "message": "Claude needs your permission",
                "notification_type": "permission_prompt"
            }),
        ]
        .iter()
        .filter_map(|payload| apply(payload, &mut state, &mut meta))
        .collect();

        assert_eq!(told.len(), 1, "one stop, one notice: {told:?}");
        assert_eq!(
            told[0].body, "Which fixture should the port keep?",
            "the event that stopped the agent is the one that knew the question"
        );
        assert_eq!(state.state, Phase::Waiting, "and it is still waiting");

        // Waiting on the menu, not a box: only the call knew the question.
        assert_eq!(state.kind, Some(Kind::Question));
        assert_eq!(
            state.question.as_deref(),
            Some("Which fixture should the port keep?")
        );
        assert_eq!(state.options, ["SQLite fixture", "Docker fixture"]);
    }

    #[test]
    fn hook_coherence_the_menu_behind_an_answered_box_is_its_own_stop() {
        // One turn, two stops, captured from claude 2.1.240: a Bash box
        // answered with 1, then a menu. In between claude fired
        // PostToolUse(Bash), which amx does not install, so it is omitted here
        // too. Nothing that reaches this function said the box closed, so the
        // record still read waiting when the menu went up.
        //
        // Two screens need two notices.
        let asking = json!({ "questions": [{
            "question": "Which fixture should the port keep?",
            "header": "Fixture",
            "options": [
                { "label": "SQLite fixture", "description": "Use SQLite for the port" },
                { "label": "Docker fixture", "description": "Use Docker for the port" }
            ],
            "multiSelect": false
        }] });
        let running = json!({
            "command": "touch /home/saiful/probe/marker.txt",
            "description": "Create marker file"
        });

        let mut state = State::default();
        let mut meta = meta();
        let told: Vec<Notice> = [
            json!({ "hook_event_name": "UserPromptSubmit", "permission_mode": "default" }),
            json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Bash",
                "tool_input": running,
                "tool_use_id": "toolu_01MjEq6c8WXNCAN1JVaeRm24"
            }),
            json!({
                "hook_event_name": "PermissionRequest",
                "tool_name": "Bash",
                "tool_input": running
            }),
            json!({
                "hook_event_name": "Notification",
                "message": "Claude needs your permission",
                "notification_type": "permission_prompt"
            }),
            json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "AskUserQuestion",
                "tool_input": asking,
                "tool_use_id": "toolu_01P5TGAepraiDjF8PFyYTdcR"
            }),
            json!({
                "hook_event_name": "PermissionRequest",
                "tool_name": "AskUserQuestion",
                "tool_input": asking
            }),
            json!({
                "hook_event_name": "Notification",
                "message": "Claude needs your permission",
                "notification_type": "permission_prompt"
            }),
        ]
        .iter()
        .filter_map(|payload| apply(payload, &mut state, &mut meta))
        .collect();

        assert_eq!(told.len(), 2, "two stops, two notices: {told:?}");
        assert_eq!(
            told[0].body, "Claude needs your permission to use Bash",
            "the box is the first thing anybody had to answer"
        );
        assert_eq!(
            told[1].body, "Which fixture should the port keep?",
            "and the menu is the screen it left behind it"
        );
    }

    #[test]
    fn hook_coherence_the_second_menu_of_one_turn_is_a_second_stop() {
        // Two menus in one turn. Answering the first fired PostToolUse, which
        // amx does not install, so the record reads waiting throughout. Each
        // menu is a separate stop.
        let fixture = json!({ "questions": [{
            "question": "Which fixture should the port keep?",
            "header": "Fixture",
            "options": [
                { "label": "SQLite fixture", "description": "Use SQLite for the port" },
                { "label": "Docker fixture", "description": "Use Docker for the port" }
            ],
            "multiSelect": false
        }] });
        let port = json!({ "questions": [{
            "question": "Which port should be bound?",
            "header": "Port",
            "options": [
                { "label": "3000", "description": "Bind to port 3000" },
                { "label": "8080", "description": "Bind to port 8080" }
            ],
            "multiSelect": false
        }] });

        let mut state = State::default();
        let mut meta = meta();
        let told: Vec<Notice> = [
            json!({ "hook_event_name": "UserPromptSubmit", "permission_mode": "default" }),
            json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "AskUserQuestion",
                "tool_input": fixture,
                "tool_use_id": "toolu_013QLrB2kwSKCX1e9v17nFQp"
            }),
            json!({
                "hook_event_name": "PermissionRequest",
                "tool_name": "AskUserQuestion",
                "tool_input": fixture
            }),
            json!({
                "hook_event_name": "Notification",
                "message": "Claude needs your permission",
                "notification_type": "permission_prompt"
            }),
            json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "AskUserQuestion",
                "tool_input": port,
                "tool_use_id": "toolu_01UreAQSVJXR8d9K9oXj7eJD"
            }),
            json!({
                "hook_event_name": "PermissionRequest",
                "tool_name": "AskUserQuestion",
                "tool_input": port
            }),
            json!({
                "hook_event_name": "Notification",
                "message": "Claude needs your permission",
                "notification_type": "permission_prompt"
            }),
        ]
        .iter()
        .filter_map(|payload| apply(payload, &mut state, &mut meta))
        .collect();

        // Each notice went out when its menu appeared. This test does not
        // check what the box and notification after each menu do to the
        // question.
        assert_eq!(told.len(), 2, "two menus, two notices: {told:?}");
        assert_eq!(told[0].body, "Which fixture should the port keep?");
        assert_eq!(
            told[1].body, "Which port should be bound?",
            "the second question is the one still up"
        );
    }

    #[test]
    fn hook_a_stop_amx_cannot_name_is_still_one_interruption() {
        // A box naming no tool gets a generic notice. The notification's words
        // six seconds later reach the record without a second notice.
        let mut state = State::default();
        let mut meta = meta();

        let told = apply(
            &json!({ "hook_event_name": "PermissionRequest" }),
            &mut state,
            &mut meta,
        )
        .expect("somebody still has to be told");
        assert!(!told.body.is_empty(), "there is always something to say");

        let again = apply(
            &json!({
                "hook_event_name": "Notification",
                "message": "Claude needs your permission to use Bash",
                "notification_type": "permission_prompt"
            }),
            &mut state,
            &mut meta,
        );
        assert_eq!(again, None, "one stop, one interruption");
        assert_eq!(
            state.question.as_deref(),
            Some("Claude needs your permission to use Bash"),
            "and the words still reach the record"
        );
    }

    #[test]
    fn hook_the_stop_after_the_agent_went_back_to_work_is_told() {
        // One notice per stop: once the agent is back at work, the next stop
        // is news.
        let mut state = State {
            turn_open: true,
            ..State::default()
        };
        let mut meta = meta();

        assert!(
            apply(
                &json!({ "hook_event_name": "PermissionRequest", "tool_name": "Bash" }),
                &mut state,
                &mut meta,
            )
            .is_some()
        );
        apply(
            &json!({ "hook_event_name": "PermissionDenied", "tool_name": "Bash" }),
            &mut state,
            &mut meta,
        );
        assert_eq!(state.state, Phase::Working);

        assert!(
            apply(
                &json!({ "hook_event_name": "PermissionRequest", "tool_name": "Write" }),
                &mut state,
                &mut meta,
            )
            .is_some(),
            "a second box is a second stop"
        );
    }

    #[test]
    fn hook_an_mcp_tools_box_is_worded_the_vendors_way_and_said_once() {
        // For `mcp__<server>__<tool>`, the vendor's sentence uses the last
        // segment with underscores as spaces and each word capitalised. It
        // must match the pane's sentence.
        let mut state = State::default();
        let mut meta = meta();
        let told = apply(
            &json!({
                "hook_event_name": "PermissionRequest",
                "tool_name": "mcp__playwright__browser_click"
            }),
            &mut state,
            &mut meta,
        );
        assert!(told.is_some());
        assert_eq!(
            state.question.as_deref(),
            Some("Claude needs your permission to use Browser Click"),
            "the sentence on the record is the sentence on the pane"
        );

        let again = apply(
            &json!({
                "hook_event_name": "Notification",
                "message": "Claude needs your permission to use Browser Click",
                "notification_type": "permission_prompt"
            }),
            &mut state,
            &mut meta,
        );
        assert_eq!(again, None, "one box, one interruption");
    }

    #[test]
    fn hook_a_denied_permission_puts_the_agent_back_to_work() {
        // No PostToolUse follows a tool that never ran, so without this the
        // record would read waiting for the rest of the turn.
        let mut state = State {
            state: Phase::Waiting,
            question: Some("Claude needs your permission to use Bash".to_string()),
            options: vec!["Yes".to_string(), "No".to_string()],
            kind: Some(Kind::Permission),
            turn_open: true,
            ..State::default()
        };
        let notice = apply(
            &json!({
                "hook_event_name": "PermissionDenied",
                "tool_name": "Bash",
                "tool_use_id": "toolu_01",
                "reason": "user denied"
            }),
            &mut state,
            &mut meta(),
        );
        assert_eq!(
            state.state,
            Phase::Working,
            "the turn goes on with the refusal in it"
        );
        assert_eq!(state.question, None);
        assert!(state.options.is_empty());
        assert_eq!(state.kind, None);
        assert_eq!(notice, None, "nobody is being asked for anything now");
    }

    #[test]
    fn hook_a_prompt_closed_outside_a_turn_leaves_it_idle() {
        // pi raises extension dialogs in or out of a turn. Closed outside a
        // turn, there is no work to return to.
        let pi = &crate::vendor::pi::HOOKS;
        let raised = json!({ "hook_event_name": "ui_prompt_start", "kind": "confirm", "message": "Trust this folder?" });
        let closed = json!({ "hook_event_name": "ui_prompt_end", "kind": "confirm" });

        let mut state = State::default();
        let mut meta = meta();
        apply_in(pi, &raised, &mut state, &mut meta);
        assert_eq!(state.state, Phase::Waiting);
        apply_in(pi, &closed, &mut state, &mut meta);
        assert_eq!(state.state, Phase::Idle, "no turn was open to go back to");
        assert_eq!(state.question, None);

        let mut state = State::default();
        for payload in [
            json!({ "hook_event_name": "agent_start" }),
            raised.clone(),
            closed.clone(),
        ] {
            apply_in(pi, &payload, &mut state, &mut meta);
        }
        assert_eq!(state.state, Phase::Working, "the turn goes on");

        apply_in(
            pi,
            &json!({ "hook_event_name": "agent_settled" }),
            &mut state,
            &mut meta,
        );
        apply_in(pi, &raised, &mut state, &mut meta);
        apply_in(pi, &closed, &mut state, &mut meta);
        assert_eq!(state.state, Phase::Idle, "the turn that was open has ended");
    }

    #[test]
    fn hook_a_notification_of_no_named_kind_leaves_the_kind_where_it_was() {
        // Untyped notifications keep their original meaning: stopped on these
        // words.
        let mut state = State {
            state: Phase::Waiting,
            kind: Some(Kind::Permission),
            ..State::default()
        };
        apply(
            &json!({
                "hook_event_name": "Notification",
                "message": "Claude needs your permission to use Bash"
            }),
            &mut state,
            &mut meta(),
        );
        assert_eq!(state.kind, Some(Kind::Permission));
    }

    #[test]
    fn hook_coherence_a_nudge_about_an_idle_session_is_not_a_question() {
        // The idle nudge comes only when nothing is open, so its words are not
        // a question. Taking them as one would leave "Claude is waiting for
        // your input" as the thing to answer.
        let mut state = State {
            state: Phase::Waiting,
            summary: Some("Running Bash".to_string()),
            question: Some("Claude needs your permission to use Bash".to_string()),
            options: vec!["Yes".to_string(), "No".to_string()],
            kind: Some(Kind::Permission),
            ..State::default()
        };
        let notice = apply(
            &json!({
                "hook_event_name": "Notification",
                "message": "Claude is waiting for your input",
                "notification_type": "idle_prompt"
            }),
            &mut state,
            &mut meta(),
        );
        assert_eq!(state.state, Phase::Idle, "the vendor says nothing is open");
        assert_eq!(state.question, None);
        assert!(state.options.is_empty());
        assert_eq!(state.kind, None);
        assert_eq!(state.summary, None, "and it is running nothing");
        assert_eq!(notice, None, "an agent going quiet interrupts nobody");
    }

    #[test]
    fn hook_coherence_a_late_nudge_never_overwrites_the_answer() {
        let mut state = State {
            state: Phase::Idle,
            result: Some("I fixed the login bug.".to_string()),
            source: Some(Source::Payload),
            ..State::default()
        };
        apply(
            &json!({
                "hook_event_name": "Notification",
                "message": "Claude is waiting for your input",
                "notification_type": "idle_prompt"
            }),
            &mut state,
            &mut meta(),
        );
        assert_eq!(state.state, Phase::Idle);
        assert_eq!(state.result.as_deref(), Some("I fixed the login bug."));
        assert_eq!(state.source, Some(Source::Payload));
        assert_eq!(state.question, None);
    }

    #[test]
    fn hook_coherence_a_late_nudge_keeps_the_finished_turns_line() {
        // The summary command is asked once per turn, so a line cleared by the
        // nudge a minute after the turn ended would never come back.
        let mut state = State {
            state: Phase::Idle,
            result: Some("I fixed the login bug.".to_string()),
            summary: Some("fixed the login bug".to_string()),
            ..State::default()
        };
        apply(
            &json!({
                "hook_event_name": "Notification",
                "message": "Claude is waiting for your input",
                "notification_type": "idle_prompt"
            }),
            &mut state,
            &mut meta(),
        );
        assert_eq!(state.state, Phase::Idle);
        assert_eq!(state.summary.as_deref(), Some("fixed the login bug"));
    }

    #[test]
    fn hook_coherence_an_untyped_nudge_never_undoes_an_answered_turn() {
        // Older vendors send the idle nudge untyped. After a turn ended with
        // an answer it can only be that nudge, and taking it as a question
        // would flip the record back to waiting.
        let mut state = State {
            state: Phase::Idle,
            result: Some("I fixed the login bug.".to_string()),
            source: Some(Source::Payload),
            ..State::default()
        };
        let notice = apply(
            &json!({
                "hook_event_name": "Notification",
                "message": "Claude is waiting for your input"
            }),
            &mut state,
            &mut meta(),
        );
        assert_eq!(state.state, Phase::Idle, "the turn is still over");
        assert_eq!(state.question, None);
        assert_eq!(state.result.as_deref(), Some("I fixed the login bug."));
        assert_eq!(notice, None, "an agent going quiet interrupts nobody");
    }

    #[test]
    fn hook_a_question_the_vendor_asks_is_a_kind_of_its_own() {
        // AskUserQuestion fires only the pre-tool hook. It draws a menu and
        // waits, and its payload carries the questions before the pane does.
        let (state, _, notice) = fold(json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "AskUserQuestion",
            "tool_input": {
                "questions": [{
                    "question": "Which fixture should the port keep?",
                    "header": "Fixture",
                    "options": [
                        { "label": "the sqlite one", "description": "no daemon to run" },
                        { "label": "the docker one", "description": "closer to production" }
                    ],
                    "multiSelect": false
                }]
            }
        }));
        assert_eq!(state.state, Phase::Waiting);
        assert_eq!(state.kind, Some(Kind::Question));
        assert_eq!(
            state.question.as_deref(),
            Some("Which fixture should the port keep?")
        );
        assert_eq!(state.options, ["the sqlite one", "the docker one"]);
        assert_eq!(state.summary, None, "it is not running anything");
        assert!(
            notice.is_some(),
            "no notification follows this one, so nobody would be told at all"
        );
    }

    #[test]
    fn hook_a_call_that_asks_several_questions_reaches_the_record_whole() {
        // Three questions in one call, as claude 2.1.240 sends it (see
        // docs/question-shapes.md), with multiSelect per question. Only the
        // first is on screen; the payload is the only record of the rest.
        let (state, _, notice) = fold(json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "AskUserQuestion",
            "tool_input": {
                "questions": [
                    {
                        "question": "Which runtime should the service target?",
                        "header": "Runtime",
                        "options": [
                            { "label": "Node", "description": "Widest library support" },
                            { "label": "Deno", "description": "Batteries included" }
                        ],
                        "multiSelect": false
                    },
                    {
                        "question": "Which store should hold sessions?",
                        "header": "Storage",
                        "options": [
                            { "label": "Redis", "description": "Fast, volatile" },
                            { "label": "Postgres", "description": "Durable, already deployed" }
                        ],
                        "multiSelect": false
                    },
                    {
                        "question": "Which rollout steps should run?",
                        "header": "Rollout",
                        "options": [
                            { "label": "Canary", "description": "Five percent first" },
                            { "label": "Migrate", "description": "Run the schema change" },
                            { "label": "Announce", "description": "Post to the channel" }
                        ],
                        "multiSelect": true
                    }
                ]
            }
        }));

        assert_eq!(state.state, Phase::Waiting);
        assert_eq!(state.kind, Some(Kind::Question));
        assert!(notice.is_some(), "no notification follows this one");

        // The question on screen, where readers look for it.
        assert_eq!(
            state.question.as_deref(),
            Some("Which runtime should the service target?")
        );
        assert_eq!(state.options, ["Node", "Deno"]);

        // The two behind it, with what no screen shows.
        assert_eq!(state.asking.len(), 3);
        assert_eq!(state.asking[1].header.as_deref(), Some("Storage"));
        assert_eq!(state.asking[1].text, "Which store should hold sessions?");
        assert_eq!(state.asking[1].labels(), ["Redis", "Postgres"]);
        assert_eq!(
            state.asking[0].options[0].description.as_deref(),
            Some("Widest library support")
        );
        assert!(!state.asking[0].multi);
        assert!(state.asking[2].multi, "the flag is per question");
        assert!(state.asking.iter().all(|ask| ask.answer.is_none()));
        assert!(!state.multi(), "and the one showing takes one choice");
    }

    #[test]
    fn hook_a_question_that_takes_more_than_one_choice_says_so() {
        // Only the payload says so. The screen draws `[ ]` boxes and a Submit
        // row, which a screen reading would mistake for labels.
        let (state, _, _) = fold(json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "AskUserQuestion",
            "tool_input": {
                "questions": [{
                    "question": "Which features should be enabled?",
                    "header": "Features",
                    "options": [
                        { "label": "Logging", "description": "Write a log file" },
                        { "label": "Metrics", "description": "Export counters" }
                    ],
                    "multiSelect": true
                }]
            }
        }));
        assert!(state.multi());
        assert_eq!(state.options, ["Logging", "Metrics"]);
        assert!(!state.asking[0].takes_notes(), "and it offers no note");
    }

    #[test]
    fn hook_a_choices_preview_is_what_puts_a_notes_field_on_a_question() {
        // In claude 2.1.240 a `preview` on any choice draws the notes field,
        // and `n` does nothing without one. The previewed screen also drops the
        // free-text row, so the pane cannot tell them apart.
        let (state, _, _) = fold(json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "AskUserQuestion",
            "tool_input": {
                "questions": [{
                    "question": "Which header layout should the page use?",
                    "header": "Layout",
                    "options": [
                        {
                            "label": "Stacked",
                            "description": "Title over subtitle",
                            "preview": "+----------+\n| TITLE    |\n+----------+"
                        },
                        { "label": "Inline", "description": "Title beside subtitle" }
                    ],
                    "multiSelect": false
                }]
            }
        }));
        assert!(state.asking[0].takes_notes());
        assert_eq!(
            state.asking[0].options[0].preview.as_deref(),
            Some("+----------+\n| TITLE    |\n+----------+")
        );
        assert_eq!(state.asking[0].options[1].preview, None);
    }

    #[test]
    fn hook_a_question_amx_cannot_read_is_still_a_question_of_that_kind() {
        // An unreadable menu is still a question to answer.
        for input in [
            json!({ "questions": [] }),
            json!({ "questions": "malformed" }),
            json!({ "questions": [{ "header": "Fixture", "options": [] }] }),
            json!({}),
        ] {
            let (state, _, _) = fold(json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "AskUserQuestion",
                "tool_input": input
            }));
            assert_eq!(state.state, Phase::Waiting, "{}", state.state);
            assert_eq!(state.kind, Some(Kind::Question));
            assert_eq!(state.question, None);
            assert!(state.asking.is_empty(), "{input}");
        }
    }

    #[test]
    fn hook_a_question_that_is_over_takes_its_kind_with_it() {
        let asked = State {
            state: Phase::Waiting,
            question: Some("Which fixture should the port keep?".to_string()),
            options: vec!["the sqlite one".to_string()],
            kind: Some(Kind::Question),
            ..State::default()
        };

        for payload in [
            json!({ "hook_event_name": "PreToolUse", "tool_name": "Bash" }),
            json!({ "hook_event_name": "UserPromptSubmit", "prompt": "carry on" }),
            json!({ "hook_event_name": "Stop", "last_assistant_message": "done" }),
        ] {
            let mut state = asked.clone();
            apply(&payload, &mut state, &mut meta());
            assert_eq!(state.kind, None, "{payload}");
            assert_eq!(state.question, None);
        }
    }

    #[test]
    fn hook_the_end_of_a_turn_carries_the_answer() {
        let (state, _, notice) = fold(json!({
            "hook_event_name": "Stop",
            "stop_hook_active": false,
            "last_assistant_message": "I fixed the login bug."
        }));
        assert_eq!(state.state, Phase::Idle);
        assert_eq!(state.result.as_deref(), Some("I fixed the login bug."));
        assert_eq!(
            state.source,
            Some(Source::Payload),
            "the payload beats the transcript, which lags it"
        );
        assert_eq!(state.question, None);
        assert_eq!(state.background, 0, "it left nothing of its own running");
        assert_eq!(notice, None, "an idle agent is on the wall already");
    }

    #[test]
    fn hook_a_turn_that_left_shells_running_is_still_a_turn() {
        // claude lists shells still running in its turn-end payload. Reading
        // that as idle once set the park timer, and `_park` took the pane
        // while the shells still ran.
        let (state, _, notice) = fold(json!({
            "hook_event_name": "Stop",
            "stop_hook_active": false,
            "last_assistant_message": "I started the build.",
            "background_tasks": [
                { "id": "bash_1", "type": "shell", "status": "running" },
                { "id": "bash_2", "type": "shell", "status": "completed" },
                { "id": "bash_3", "type": "shell", "status": "running" }
            ]
        }));
        assert_eq!(state.state, Phase::Working);
        assert_eq!(state.background, 2, "the finished one is not one of them");
        assert_eq!(state.summary.as_deref(), Some("2 shells running"));
        assert_eq!(
            state.result.as_deref(),
            Some("I started the build."),
            "the model has answered, whatever is still running under it"
        );
        assert_eq!(state.source, Some(Source::Payload));
        assert_eq!(parks_in(&state, 3_600), None, "and nothing takes its pane");
        assert_eq!(notice, None, "a working agent is on the wall already");

        // Singular for one.
        let (one, _, _) = fold(json!({
            "hook_event_name": "Stop",
            "background_tasks": [{ "id": "bash_1", "type": "shell", "status": "running" }]
        }));
        assert_eq!(one.summary.as_deref(), Some("1 shell running"));
    }

    #[test]
    fn hook_a_turn_that_left_agents_running_names_them_apart_from_shells() {
        // Tasks not typed `shell` are agents, and the summary names them so.
        let (state, _, _) = fold(json!({
            "hook_event_name": "Stop",
            "last_assistant_message": "Two reviewers are reading the diff.",
            "background_tasks": [
                { "id": "bash_1", "type": "shell", "status": "running" },
                { "id": "a1", "type": "local_agent", "status": "running" },
                { "id": "a2", "type": "local_agent", "status": "running" },
                { "id": "a3", "type": "local_agent", "status": "completed" }
            ]
        }));
        assert_eq!(state.state, Phase::Working);
        assert_eq!(state.background, 3);
        assert_eq!(
            state.summary.as_deref(),
            Some("1 shell and 2 agents running")
        );
        assert_eq!(
            state.result.as_deref(),
            Some("Two reviewers are reading the diff.")
        );

        let (agents, _, _) = fold(json!({
            "hook_event_name": "Stop",
            "background_tasks": [{ "id": "a1", "type": "local_agent", "status": "running" }]
        }));
        assert_eq!(agents.summary.as_deref(), Some("1 agent running"));
    }

    #[test]
    fn hook_a_prompt_the_vendor_typed_itself_keeps_the_answer_before_it() {
        // A finished background task or another agent's message arrives as a
        // prompt nobody typed. The reply to it is not the task's answer.
        let mut state = State {
            state: Phase::Working,
            summary: Some("1 shell running".to_string()),
            background: 1,
            result: Some("I started the build.".to_string()),
            source: Some(Source::Payload),
            ..State::default()
        };
        let mut meta = meta();

        for prompt in [
            "<task-notification>\n<task-id>bash_1</task-id>\n<status>completed</status>",
            "<agent-message from=\"reviewer\">looks fine</agent-message>",
        ] {
            apply(
                &json!({ "hook_event_name": "UserPromptSubmit", "prompt": prompt }),
                &mut state,
                &mut meta,
            );
            assert_eq!(state.state, Phase::Working, "{prompt}");
            assert_eq!(state.result.as_deref(), Some("I started the build."));
            assert_eq!(state.source, Some(Source::Payload));

            apply(
                &json!({ "hook_event_name": "Stop", "last_assistant_message": "Noted." }),
                &mut state,
                &mut meta,
            );
            assert_eq!(state.state, Phase::Idle, "{prompt}");
            assert_eq!(
                state.result.as_deref(),
                Some("I started the build."),
                "its end does not replace the answer on the record"
            );
        }

        // A tag past the start is a person's prompt.
        apply(
            &json!({ "hook_event_name": "UserPromptSubmit", "prompt": "what is a <task-notification>?" }),
            &mut state,
            &mut meta,
        );
        assert_eq!(state.result, None);
    }

    #[test]
    fn hook_an_aborted_or_failed_turn_has_no_answer() {
        // pi ends an aborted or failed turn with whatever partial text it had,
        // and says why in the payload.
        for reason in ["aborted", "error"] {
            let (state, _, _) = fold(json!({
                "hook_event_name": "Stop",
                "stop_reason": reason,
                "last_assistant_message": "Let me look at the"
            }));
            assert_eq!(state.state, Phase::Idle, "{reason}");
            assert_eq!(state.result, None, "{reason}");
            assert_eq!(state.source, None, "{reason}");
        }

        let (state, _, _) = fold(json!({
            "hook_event_name": "Stop",
            "stop_reason": "stop",
            "last_assistant_message": "Done."
        }));
        assert_eq!(state.result.as_deref(), Some("Done."));
    }

    #[test]
    fn hook_the_idle_nudge_over_a_running_shell_moves_nothing() {
        // With shells running, the idle nudge only means the model is idle.
        let mut state = State {
            state: Phase::Working,
            summary: Some("2 shells running".to_string()),
            background: 2,
            ..State::default()
        };
        let before = state.clone();
        let notice = apply(
            &json!({
                "hook_event_name": "Notification",
                "message": "Claude is waiting for your input",
                "notification_type": "idle_prompt"
            }),
            &mut state,
            &mut meta(),
        );
        assert_eq!(state, before, "the shells are still running");
        assert_eq!(notice, None);
    }

    #[test]
    fn hook_the_next_thing_the_agent_does_retires_the_shell_count() {
        // The count belongs to the ended turn; the next prompt or tool call
        // clears it.
        let ended = State {
            state: Phase::Working,
            summary: Some("2 shells running".to_string()),
            background: 2,
            ..State::default()
        };

        for payload in [
            json!({ "hook_event_name": "UserPromptSubmit", "prompt": "carry on" }),
            json!({ "hook_event_name": "PreToolUse", "tool_name": "Bash" }),
            json!({ "hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion" }),
        ] {
            let mut state = ended.clone();
            apply(&payload, &mut state, &mut meta());
            assert_eq!(state.background, 0, "{payload}");
        }
    }

    #[test]
    fn hook_the_session_is_learned_only_from_the_event_that_owns_it() {
        let (state, meta, _) = fold(json!({
            "session_id": "abc-123",
            "transcript_path": "/home/dev/.claude/projects/x/abc-123.jsonl",
            "hook_event_name": "SessionStart",
            "source": "startup"
        }));
        assert_eq!(meta.session.as_deref(), Some("abc-123"));
        assert_eq!(
            meta.transcript,
            Some(PathBuf::from("/home/dev/.claude/projects/x/abc-123.jsonl"))
        );
        assert_eq!(state.state, Phase::Starting, "and it moves nothing");

        // Every payload carries a session id, but only a session start sets it.
        let mut meta = meta.clone();
        let mut state = State::default();
        apply(
            &json!({ "session_id": "some-subagents-session", "hook_event_name": "Stop" }),
            &mut state,
            &mut meta,
        );
        assert_eq!(meta.session.as_deref(), Some("abc-123"));

        // A resume replaces it.
        apply(
            &json!({
                "session_id": "def-456",
                "transcript_path": "/t/def-456.jsonl",
                "hook_event_name": "SessionStart",
                "source": "resume"
            }),
            &mut state,
            &mut meta,
        );
        assert_eq!(meta.session.as_deref(), Some("def-456"));
    }

    #[test]
    fn hook_an_adopted_agent_learns_its_transcript_from_its_own_reports() {
        // `adopt` takes the session from the pane's environment but cannot
        // know the transcript: the vendor announced it before the record
        // existed. Seen with a pi adopted mid-session.
        let mut meta = meta();
        meta.session = Some("01a0-adopted".to_string());
        meta.transcript = None;
        let mut state = State::default();

        // A report about another session is not this agent's conversation.
        apply(
            &json!({
                "session_id": "another",
                "transcript_path": "/t/another.jsonl",
                "hook_event_name": "UserPromptSubmit"
            }),
            &mut state,
            &mut meta,
        );
        assert_eq!(meta.transcript, None);

        // The first report about its own session sets the transcript.
        apply(
            &json!({
                "session_id": "01a0-adopted",
                "transcript_path": "/t/01a0-adopted.jsonl",
                "hook_event_name": "Stop",
                "last_assistant_message": "done"
            }),
            &mut state,
            &mut meta,
        );
        assert_eq!(
            meta.transcript,
            Some(PathBuf::from("/t/01a0-adopted.jsonl"))
        );

        // A later report does not move it.
        apply(
            &json!({
                "session_id": "01a0-adopted",
                "transcript_path": "/t/elsewhere.jsonl",
                "hook_event_name": "UserPromptSubmit"
            }),
            &mut state,
            &mut meta,
        );
        assert_eq!(
            meta.transcript,
            Some(PathBuf::from("/t/01a0-adopted.jsonl"))
        );
    }

    #[test]
    fn hook_a_subagents_work_is_not_the_agents_state() {
        let mut state = State {
            state: Phase::Waiting,
            question: Some("Run the migration?".to_string()),
            ..State::default()
        };
        let mut meta = meta();

        for payload in [
            json!({ "hook_event_name": "PreToolUse", "tool_name": "Read", "agent_id": "sub-1" }),
            json!({ "hook_event_name": "Stop", "agent_id": "sub-1", "last_assistant_message": "done" }),
        ] {
            let notice = apply(&payload, &mut state, &mut meta);
            assert_eq!(state.state, Phase::Waiting, "{payload}");
            assert_eq!(state.question.as_deref(), Some("Run the migration?"));
            assert_eq!(notice, None);
        }
    }

    #[test]
    fn hook_a_word_from_the_vendor_takes_an_interrupt_stamp_off_the_record() {
        // Any word from the vendor clears the interrupt stamp, whatever second
        // it lands in. Comparing timestamps instead let an interrupt in the
        // same second as the last hook read as none, which on a tool-heavy
        // turn is most of them.
        for payload in [
            json!({ "hook_event_name": "PreToolUse", "tool_name": "Bash" }),
            json!({ "hook_event_name": "UserPromptSubmit", "prompt": "carry on" }),
        ] {
            let mut state = State {
                state: Phase::Working,
                interrupted_at: 1_000,
                ..State::default()
            };
            apply(&payload, &mut state, &mut meta());
            assert_eq!(state.interrupted_at, 0, "{payload}");
        }

        // A subagent's event is not the agent speaking.
        let mut state = State {
            state: Phase::Working,
            interrupted_at: 1_000,
            ..State::default()
        };
        apply(
            &json!({ "hook_event_name": "PreToolUse", "tool_name": "Read", "agent_id": "sub-1" }),
            &mut state,
            &mut meta(),
        );
        assert_eq!(state.interrupted_at, 1_000, "a subagent's work is not it");
    }

    #[test]
    fn hook_a_word_from_the_vendor_takes_a_park_stamp_off_the_record() {
        // An agent that speaks has a pane; a stamp left standing would have
        // `send` refuse it as parked.
        for payload in [
            json!({ "hook_event_name": "SessionStart", "session_id": "s-1" }),
            json!({ "hook_event_name": "UserPromptSubmit", "prompt": "carry on" }),
        ] {
            let mut state = State {
                state: Phase::Idle,
                parked_at: 1_000,
                ..State::default()
            };
            apply(&payload, &mut state, &mut meta());
            assert_eq!(state.parked_at, 0, "{payload}");
        }

        // A subagent's event is not the agent speaking.
        let mut state = State {
            state: Phase::Idle,
            parked_at: 1_000,
            ..State::default()
        };
        apply(
            &json!({ "hook_event_name": "PreToolUse", "tool_name": "Read", "agent_id": "sub-1" }),
            &mut state,
            &mut meta(),
        );
        assert_eq!(state.parked_at, 1_000, "a subagent's work is not it");
    }

    #[test]
    fn hook_a_record_that_has_ended_stays_ended() {
        let mut state = State {
            state: Phase::Done,
            exit: Some(0),
            result: Some("all done".to_string()),
            ..State::default()
        };
        let mut meta = meta();

        apply(
            &json!({ "hook_event_name": "Stop", "last_assistant_message": "late" }),
            &mut state,
            &mut meta,
        );
        assert_eq!(state.state, Phase::Done);
        assert_eq!(state.result.as_deref(), Some("all done"));
    }

    #[test]
    fn hook_a_payload_amx_does_not_know_changes_nothing() {
        for payload in [
            json!({ "hook_event_name": "PostToolUse", "tool_name": "Bash" }),
            json!({ "hook_event_name": "SessionEnd" }),
            json!({}),
            json!("not even an object"),
        ] {
            let (state, meta, notice) = fold(payload.clone());
            assert_eq!(state, State::default(), "{payload}");
            assert_eq!(meta.session, None);
            assert_eq!(notice, None);
        }
    }

    // The commands themselves.

    fn an_agent(root: &Path) -> Agent {
        Agent::create(root, &meta()).unwrap()
    }

    fn hook(root: &Path, id: &str, payload: &str) -> i32 {
        run(
            Some(id),
            root,
            &mut payload.as_bytes(),
            &mut std::io::sink(),
            &quiet(),
            None,
        )
    }

    /// The state document on disk, parsed as plain JSON.
    fn written(agent: &Agent) -> Value {
        let text = std::fs::read_to_string(agent.dir().join("state.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn hook_a_permission_and_a_question_reach_the_record_as_different_kinds() {
        let root = TempDir::new().unwrap();
        let asked = an_agent(root.path());
        let allowed = Agent::create(
            root.path(),
            &Meta {
                parent: None,
                depth: 0,
                id: "port-cli-batch-c3d".to_string(),
                ..meta()
            },
        )
        .unwrap();

        hook(
            root.path(),
            asked.id(),
            &json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "AskUserQuestion",
                "tool_input": { "questions": [{ "question": "Which fixture?", "options": [] }] }
            })
            .to_string(),
        );
        hook(
            root.path(),
            allowed.id(),
            &json!({
                "hook_event_name": "Notification",
                "message": "Claude needs your permission to use Bash",
                "notification_type": "permission_prompt"
            })
            .to_string(),
        );

        assert_eq!(written(&asked)["question"]["kind"], "question");
        assert_eq!(written(&asked)["question"]["text"], "Which fixture?");
        assert_eq!(written(&allowed)["question"]["kind"], "permission");
    }

    #[test]
    fn hook_records_the_event_and_moves_the_state() {
        let root = TempDir::new().unwrap();
        let agent = an_agent(root.path());

        let code = hook(
            root.path(),
            agent.id(),
            r#"{"hook_event_name":"PreToolUse","tool_name":"Bash"}"#,
        );
        assert_eq!(code, exit::OK);

        assert_eq!(agent.state().unwrap().state, Phase::Working);
        let events = agent.events().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "PreToolUse");
        assert_eq!(events[0].payload["tool_name"], "Bash");
    }

    #[test]
    fn hook_a_moment_is_a_phase_the_agent_had_not_reached() {
        // Waiting counts only with a notice: one screen fires three events.
        assert_eq!(
            reached(Phase::Waiting, Phase::Working, true),
            Some(Phase::Waiting)
        );
        assert_eq!(
            reached(Phase::Waiting, Phase::Waiting, false),
            None,
            "a box answered by the notification repeating it"
        );

        // Idle counts only from another phase: the idle nudge repeats an
        // ended turn.
        assert_eq!(
            reached(Phase::Idle, Phase::Working, false),
            Some(Phase::Idle)
        );
        assert_eq!(reached(Phase::Idle, Phase::Idle, false), None);

        // Other phases are transitions, or the command ending, which
        // `record_exit` handles.
        for phase in [
            Phase::Starting,
            Phase::Working,
            Phase::Done,
            Phase::Failed,
            Phase::Stopped,
            Phase::Unknown,
        ] {
            assert_eq!(reached(phase, Phase::Working, true), None, "{phase}");
        }
    }

    #[test]
    fn hook_only_an_idle_turn_is_worth_a_timer() {
        // Only an agent idle at its prompt gets a timer; `_park` would refuse
        // any other phase.
        let idle = State {
            state: Phase::Idle,
            ..State::default()
        };
        assert_eq!(parks_in(&idle, 3_600), Some(3_600));
        assert_eq!(parks_in(&idle, 0), None, "the key is off");

        for phase in [
            Phase::Starting,
            Phase::Working,
            Phase::Waiting,
            Phase::Done,
            Phase::Failed,
            Phase::Stopped,
            Phase::Unknown,
        ] {
            let state = State {
                state: phase,
                ..State::default()
            };
            assert_eq!(parks_in(&state, 3_600), None, "{phase}");
        }
    }

    #[test]
    fn hook_a_person_who_turned_parking_off_is_not_asked_a_second_time() {
        // A project cannot re-enable parking the person turned off, and the
        // check reads no files.
        assert_eq!(park_after(&quiet(), &meta()), 0);
    }

    #[test]
    fn hook_the_timer_names_the_root_this_hook_is_writing_to() {
        // `run-shell` runs in the tmux server's environment, not this
        // process's, so the timer must carry the state root or it would park
        // records in the default root.
        let exe = std::env::current_exe().expect("a binary to name");
        assert_eq!(
            park_command(Path::new("/state/amx/agents"), "fix-login-a1b").expect("a command"),
            format!(
                "env AMX_STATE_DIR='/state/amx' {} _park 'fix-login-a1b'",
                quoted(&exe.to_string_lossy())
            ),
            "the variable names the directory the agents live under"
        );

        // Each argument is one shell word, whatever it contains.
        let odd = park_command(Path::new("/it's/agents"), "fix-login-a1b").expect("a command");
        assert!(odd.contains(r"'/it'\''s'"), "{odd}");

        // A root with no parent names no state directory.
        assert_eq!(park_command(Path::new("agents"), "fix-login-a1b"), None);
    }

    #[test]
    fn hook_a_timer_it_cannot_set_ends_the_way_everything_else_here_does() {
        // The timer is set on the pane's server; if that server is gone, the
        // failure is silent and the pane simply keeps its memory.
        let root = TempDir::new().unwrap();
        let agent = Agent::create(
            root.path(),
            &Meta {
                parent: None,
                depth: 0,
                socket: Socket::Name(format!("amx-no-such-server-{}", std::process::id())),
                ..meta()
            },
        )
        .unwrap();

        assert_eq!(
            run(
                Some(agent.id()),
                root.path(),
                &mut r#"{"hook_event_name":"Stop","last_assistant_message":"done"}"#.as_bytes(),
                &mut std::io::sink(),
                &Config {
                    park_after: 5,
                    ..quiet()
                },
                None,
            ),
            exit::OK
        );
        assert_eq!(
            agent.state().unwrap().state,
            Phase::Idle,
            "and the turn that ended is on the record all the same"
        );
    }

    #[test]
    fn hook_an_adopted_claude_is_found_by_the_session_its_payload_names() {
        // A claude amx did not start has no `AMX_ID`, so the session `adopt`
        // recorded is what identifies it.
        let root = TempDir::new().unwrap();
        let adopted = Agent::create(
            root.path(),
            &Meta {
                parent: None,
                depth: 0,
                session: Some("abc-123".to_string()),
                ..meta()
            },
        )
        .unwrap();

        assert_eq!(
            run(
                None,
                root.path(),
                &mut r#"{"session_id":"abc-123","hook_event_name":"PreToolUse","tool_name":"Bash"}"#
                    .as_bytes(),
                &mut std::io::sink(),
                &quiet(),
                None,
            ),
            exit::OK
        );
        let state = adopted.state().unwrap();
        assert_eq!(state.state, Phase::Working);
        assert_eq!(state.summary.as_deref(), Some("Running Bash"));
        assert_eq!(adopted.events().unwrap().len(), 1, "and it is written down");

        // A session with no record belongs to nobody.
        assert_eq!(
            run(
                None,
                root.path(),
                &mut r#"{"session_id":"def-456","hook_event_name":"Stop","last_assistant_message":"done"}"#
                    .as_bytes(),
                &mut std::io::sink(),
                &quiet(),
                None,
            ),
            exit::OK
        );
        assert_eq!(adopted.state().unwrap().result, None);
    }

    #[test]
    fn hook_the_nested_variable_is_read_off_the_process_environment() {
        // Setting the variable would affect every thread in the suite, so this
        // only checks that its absence means not nested.
        assert!(!nested());
    }

    #[test]
    fn hook_a_payload_naming_another_session_is_another_processs() {
        let mut ours = meta();
        ours.session = Some("abc-123".to_string());

        for payload in [
            json!({ "session_id": "nested", "hook_event_name": "Stop", "last_assistant_message": "done" }),
            json!({ "session_id": "nested", "hook_event_name": "SessionStart", "source": "startup" }),
        ] {
            assert!(anothers(&ours, &payload), "{payload}");
        }

        for payload in [
            // The agent's own session changing.
            json!({ "session_id": "def-456", "hook_event_name": "SessionStart", "source": "resume" }),
            json!({ "session_id": "def-456", "hook_event_name": "SessionStart", "source": "clear" }),
            json!({ "session_id": "def-456", "hook_event_name": "SessionStart", "source": "compact" }),
            // A start with no source is the agent's own.
            json!({ "session_id": "def-456", "hook_event_name": "SessionStart" }),
            // Payloads that say nothing to the contrary are the agent's.
            json!({ "hook_event_name": "Stop", "last_assistant_message": "done" }),
            json!({ "session_id": "", "hook_event_name": "Stop" }),
            // A subagent reports under the agent's session; `apply` ignores its
            // work.
            json!({ "session_id": "abc-123", "hook_event_name": "Stop", "agent_id": "sub-1" }),
        ] {
            assert!(!anothers(&ours, &payload), "{payload}");
        }

        // A record with no session has nothing to compare against.
        assert!(!anothers(
            &meta(),
            &json!({ "session_id": "nested", "hook_event_name": "Stop" })
        ));
    }

    #[test]
    fn hook_a_nested_claude_reporting_under_the_agents_id_is_dropped_whole() {
        // A claude launched from the agent's shell inherits `AMX_ID`. Its start
        // would overwrite the session and its stop would end the agent's turn;
        // its session id gives it away.
        let root = TempDir::new().unwrap();
        let agent = Agent::create(
            root.path(),
            &Meta {
                parent: None,
                depth: 0,
                session: Some("abc-123".to_string()),
                ..meta()
            },
        )
        .unwrap();
        let before = agent.state().unwrap();

        for payload in [
            r#"{"session_id":"nested","hook_event_name":"Stop","last_assistant_message":"the nested one"}"#,
            r#"{"session_id":"nested","hook_event_name":"SessionStart","source":"startup","transcript_path":"/t/nested.jsonl"}"#,
        ] {
            assert_eq!(
                run(
                    Some(agent.id()),
                    root.path(),
                    &mut payload.as_bytes(),
                    &mut std::io::sink(),
                    &quiet(),
                    None,
                ),
                exit::OK,
                "{payload}"
            );
        }
        // Nothing is logged either: readers take turn ends off the log.
        assert!(
            agent.events().unwrap().is_empty(),
            "nothing is written down"
        );
        assert_eq!(agent.state().unwrap(), before, "and nothing has moved");
        let meta = agent.meta().unwrap();
        assert_eq!(meta.session.as_deref(), Some("abc-123"));
        assert_eq!(meta.transcript, None);

        // The agent's own report under the same id is recorded.
        assert_eq!(
            run(
                Some(agent.id()),
                root.path(),
                &mut r#"{"session_id":"abc-123","hook_event_name":"Stop","last_assistant_message":"I fixed the login bug."}"#
                    .as_bytes(),
                &mut std::io::sink(),
                &quiet(),
                None,
            ),
            exit::OK
        );
        assert_eq!(
            agent.state().unwrap().result.as_deref(),
            Some("I fixed the login bug.")
        );
        assert_eq!(agent.events().unwrap().len(), 1);
    }

    #[test]
    fn hook_the_agents_own_start_marks_its_shells_nested() {
        // claude sources the session-start env file into every shell it runs,
        // but not into its own later hooks (claude 2.1.283). So a claude
        // started from one of those shells, `claude -c` included, knows it is
        // nested.
        let root = TempDir::new().unwrap();
        let agent = Agent::create(
            root.path(),
            &Meta {
                session: Some("abc-123".to_string()),
                ..meta()
            },
        )
        .unwrap();
        let file = root.path().join("sessionstart-hook-1.sh");
        std::fs::write(&file, "export PATH=/opt/bin:$PATH\n").unwrap();
        let hear = |payload: &str| {
            run(
                Some(agent.id()),
                root.path(),
                &mut payload.as_bytes(),
                &mut std::io::sink(),
                &quiet(),
                Some(&file),
            )
        };

        // Another process's start, a subagent's, and a prompt are not the
        // agent's session start.
        hear(r#"{"session_id":"nested","hook_event_name":"SessionStart","source":"startup"}"#);
        hear(r#"{"session_id":"abc-123","hook_event_name":"SessionStart","agent_id":"sub-1"}"#);
        hear(r#"{"session_id":"abc-123","hook_event_name":"UserPromptSubmit","prompt":"go"}"#);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "export PATH=/opt/bin:$PATH\n"
        );

        hear(r#"{"session_id":"abc-123","hook_event_name":"SessionStart","source":"startup"}"#);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "export PATH=/opt/bin:$PATH\nexport AMX_NESTED=1\n",
            "added to what the file had"
        );
    }

    #[test]
    fn hook_a_cleared_session_carries_nothing_over() {
        // `/clear` starts a new session in the same pane and drops the old
        // answer; what `resume` keeps stays.
        let root = TempDir::new().unwrap();
        let agent = Agent::create(
            root.path(),
            &Meta {
                session: Some("abc-123".to_string()),
                ..meta()
            },
        )
        .unwrap();
        agent
            .writer()
            .unwrap()
            .observe(|state| {
                state.state = Phase::Idle;
                state.since = 100;
                state.seq = 3;
                state.worked = 40;
                state.name = Some("login".to_string());
                state.session_title = Some("Fix the login bug".to_string());
                state.summary = Some("Running Bash".to_string());
                state.result = Some("I fixed the login bug.".to_string());
                state.source = Some(Source::Payload);
                state.question = Some("Allow Bash?".to_string());
                state.options = vec!["Yes".to_string(), "No".to_string()];
            })
            .unwrap();

        run(
            Some(agent.id()),
            root.path(),
            &mut r#"{"session_id":"def-456","hook_event_name":"SessionStart","source":"clear"}"#
                .as_bytes(),
            &mut std::io::sink(),
            &quiet(),
            None,
        );

        let state = agent.state().unwrap();
        assert_eq!(state.result, None);
        assert_eq!(state.source, None);
        assert_eq!(state.summary, None);
        assert_eq!(state.session_title, None);
        assert_eq!(state.question, None);
        assert!(state.options.is_empty());
        assert_eq!(
            (state.seq, state.worked, state.name.as_deref()),
            (3, 40, Some("login"))
        );
        assert_eq!(
            (state.state, state.since),
            (Phase::Idle, 100),
            "the prompt the clear left is the one it was at"
        );
        assert_eq!(agent.meta().unwrap().session.as_deref(), Some("def-456"));
    }

    #[test]
    fn hook_a_prompt_after_an_interrupt_closes_the_cut_turn() {
        // claude says nothing when a turn is interrupted, so the record still
        // reads working at the next prompt. The cut turn ends at the stamp, and
        // the hour until this prompt is not work.
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta()).unwrap();
        let cut = crate::store::now() - 3_600;
        agent
            .writer()
            .unwrap()
            .observe(|state| {
                *state = State {
                    state: Phase::Working,
                    since: cut - 60,
                    last_event: cut - 5,
                    worked: 20,
                    interrupted_at: cut,
                    ..State::default()
                }
            })
            .unwrap();

        let before = crate::store::now();
        assert_eq!(
            run(
                Some(agent.id()),
                root.path(),
                &mut r#"{"hook_event_name":"UserPromptSubmit","prompt":"carry on"}"#.as_bytes(),
                &mut std::io::sink(),
                &quiet(),
                None,
            ),
            exit::OK
        );
        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Working);
        assert_eq!(state.interrupted_at, 0);
        assert_eq!(state.worked, 80, "the cut turn is closed at the stamp");
        assert!(state.since >= before, "and the new one opens now");
    }

    #[test]
    fn hook_a_message_steered_into_a_codex_turn_keeps_it_working() {
        // codex reports a message steered into a running turn as a second
        // UserPromptSubmit with the same turn_id. It clears what a prompt
        // clears but keeps the turn open and its span unbroken. Payloads from
        // codex 0.157.1.
        let mut lines = include_str!("../tests/codex/hooks/UserPromptSubmit.jsonl").lines();
        let (first, steer) = (lines.next().unwrap(), lines.next().unwrap());
        let turn = |line: &str| serde_json::from_str::<Value>(line).unwrap()["turn_id"].clone();
        assert_eq!(turn(first), turn(steer), "the steer is the same turn");

        let root = TempDir::new().unwrap();
        let agent = Agent::create(
            root.path(),
            &Meta {
                agent: Some("codex --no-daemon".to_string()),
                session: Some("01a0e495-b6aa-7022-ba93-84d2107c2d0e".to_string()),
                ..meta()
            },
        )
        .unwrap();
        assert_eq!(
            crate::vendor::hooks_for("codex --no-daemon"),
            Some(crate::vendor::codex::HOOKS)
        );
        let send = |line: &str| {
            run(
                Some(agent.id()),
                root.path(),
                &mut line.as_bytes(),
                &mut std::io::sink(),
                &quiet(),
                None,
            )
        };
        assert_eq!(send(first), exit::OK);
        let opened = crate::store::now() - 120;
        agent
            .writer()
            .unwrap()
            .observe(|state| {
                assert_eq!(state.state, Phase::Working);
                state.since = opened;
                state.worked = 20;
                state.summary = Some("Running Bash".to_string());
            })
            .unwrap();

        assert_eq!(send(steer), exit::OK);
        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Working);
        assert!(state.turn_open);
        assert_eq!(
            (state.since, state.worked),
            (opened, 20),
            "the span is the turn's, unbroken"
        );
        assert_eq!(state.summary, None, "a prompt clears what a tool said");
    }

    #[test]
    fn hook_an_opencode_turn_reads_each_plugin_name_as_its_moment() {
        // Every event name the plugin (assets/opencode/tui.js) reports,
        // folded into an opencode record in the order a turn sends them.
        let root = TempDir::new().unwrap();
        let agent = Agent::create(
            root.path(),
            &Meta {
                agent: Some("opencode --standalone".to_string()),
                ..meta()
            },
        )
        .unwrap();
        assert_eq!(
            crate::vendor::hooks_for("opencode --standalone"),
            Some(crate::vendor::opencode::HOOKS)
        );
        let send = |payload: Value| {
            run(
                Some(agent.id()),
                root.path(),
                &mut payload.to_string().as_bytes(),
                &mut std::io::sink(),
                &quiet(),
                None,
            )
        };
        let id = "ses_f11706b91ffeWbVsNizrHjNypj";

        assert_eq!(
            send(json!({
                "hook_event_name": "session.selected",
                "session_id": id,
                "source": "startup"
            })),
            exit::OK
        );
        assert_eq!(agent.meta().unwrap().session.as_deref(), Some(id));

        send(json!({
            "hook_event_name": "session.execution.started",
            "session_id": id,
            "prompt": "say hi"
        }));
        let state = agent.state().unwrap();
        assert_eq!((state.state, state.turn_open), (Phase::Working, true));

        send(json!({
            "hook_event_name": "session.tool.called",
            "session_id": id,
            "tool_name": "bash",
            "tool_input": { "command": "ls" }
        }));
        assert_eq!(
            agent.state().unwrap().summary.as_deref(),
            Some("Running bash")
        );

        send(json!({
            "hook_event_name": "permission.asked",
            "session_id": id,
            "tool_name": "bash"
        }));
        let state = agent.state().unwrap();
        assert_eq!(
            (state.state, state.kind, state.question),
            (Phase::Waiting, Some(Kind::Permission), None),
            "opencode writes no sentence a payload carries"
        );

        send(json!({ "hook_event_name": "permission.rejected", "session_id": id }));
        let state = agent.state().unwrap();
        assert_eq!(
            state.state,
            Phase::Working,
            "the turn goes on with the refusal"
        );

        send(json!({
            "hook_event_name": "form.created",
            "session_id": id,
            "kind": "question",
            "message": "Tea or coffee?"
        }));
        let state = agent.state().unwrap();
        assert_eq!(
            (state.state, state.kind, state.question.as_deref()),
            (Phase::Waiting, Some(Kind::Question), Some("Tea or coffee?"))
        );

        send(json!({ "hook_event_name": "permission.rejected", "session_id": id }));
        send(json!({ "hook_event_name": "session.inbox.delivered", "session_id": id }));
        let state = agent.state().unwrap();
        assert_eq!(
            (state.state, state.turn_open),
            (Phase::Working, true),
            "a steered message leaves the turn open"
        );

        send(json!({
            "hook_event_name": "session.execution.ended",
            "session_id": id,
            "stop_reason": "stop",
            "last_assistant_message": "hi"
        }));
        let state = agent.state().unwrap();
        assert_eq!(
            (state.state, state.turn_open, state.result.as_deref()),
            (Phase::Idle, false, Some("hi"))
        );
    }

    #[test]
    fn hook_an_opencode_menu_is_its_question_tool_and_a_cut_turn_has_no_answer() {
        let mut meta = Meta {
            agent: Some("opencode --standalone".to_string()),
            ..meta()
        };
        let mut state = State::default();
        apply(
            &json!({
                "hook_event_name": "session.tool.called",
                "tool_name": "question",
                "tool_input": { "questions": [{
                    "question": "Tea or coffee?",
                    "header": "Drink",
                    "options": [{ "label": "Tea", "description": "A warm cup" }]
                }] }
            }),
            &mut state,
            &mut meta,
        );
        assert_eq!(
            (state.state, state.kind),
            (Phase::Waiting, Some(Kind::Question))
        );

        let mut state = State::default();
        apply(
            &json!({
                "hook_event_name": "session.execution.ended",
                "stop_reason": "aborted",
                "last_assistant_message": "half an answer"
            }),
            &mut state,
            &mut meta,
        );
        assert_eq!((state.state, state.result), (Phase::Idle, None));

        // A session the pane was resumed onto is the agent's own.
        let ours = Meta {
            session: Some("ses_ours".to_string()),
            ..meta.clone()
        };
        let opening = |source: &str| {
            json!({
                "hook_event_name": "session.selected",
                "session_id": "ses_other",
                "source": source
            })
        };
        assert!(anothers(&ours, &opening("startup")));
        assert!(!anothers(&ours, &opening("resume")));
    }

    #[test]
    fn hook_an_adopted_session_reaches_the_record_that_is_still_running() {
        // One conversation on two records: a stopped agent and the adopted
        // claude resumed from it. The payload goes to the one still running.
        let root = TempDir::new().unwrap();
        let stopped = Agent::create(
            root.path(),
            &Meta {
                parent: None,
                depth: 0,
                session: Some("abc-123".to_string()),
                created: 1,
                ..meta()
            },
        )
        .unwrap();
        stopped
            .writer()
            .unwrap()
            .update_state(|state| state.state = Phase::Stopped)
            .unwrap();
        let adopted = Agent::create(
            root.path(),
            &Meta {
                parent: None,
                depth: 0,
                id: "adopted-app-b2c".to_string(),
                session: Some("abc-123".to_string()),
                created: 2,
                ..meta()
            },
        )
        .unwrap();

        run(
            None,
            root.path(),
            &mut r#"{"session_id":"abc-123","hook_event_name":"UserPromptSubmit"}"#.as_bytes(),
            &mut std::io::sink(),
            &quiet(),
            None,
        );

        assert_eq!(adopted.state().unwrap().state, Phase::Working);
        assert_eq!(
            stopped.state().unwrap().state,
            Phase::Stopped,
            "and a record that has ended stays where it was"
        );
    }

    #[test]
    fn hook_tells_a_listening_wire_where_the_record_is() {
        // pi's extension reads what `_hook` prints, and an adopted pi has no
        // `AMX_DIR`. Every report on a record with a listening wire gets the
        // record's directory, one line.
        let root = TempDir::new().unwrap();
        let pi = Agent::create(
            root.path(),
            &Meta {
                parent: None,
                depth: 0,
                id: "their-pi-a1b".to_string(),
                agent: Some("pi".to_string()),
                model: None,
                effort: None,
                session: Some("pi-session".to_string()),
                ..meta()
            },
        )
        .unwrap();
        let where_it_is = format!("{}\n", pi.dir().display());

        let mut out = Vec::new();
        run(
            None,
            root.path(),
            &mut r#"{"session_id":"pi-session","hook_event_name":"agent_start"}"#.as_bytes(),
            &mut out,
            &quiet(),
            None,
        );
        assert_eq!(String::from_utf8(out).unwrap(), where_it_is);

        // A pane amx started gets the same answer; the extension prefers its
        // own variable.
        let mut out = Vec::new();
        run(
            Some("their-pi-a1b"),
            root.path(),
            &mut r#"{"hook_event_name":"tool_execution_start","tool_name":"bash"}"#.as_bytes(),
            &mut out,
            &quiet(),
            None,
        );
        assert_eq!(String::from_utf8(out).unwrap(), where_it_is);

        // claude shows hook output to the person (a UserPromptSubmit hook's
        // stdout joins the conversation), so a claude record gets nothing.
        Agent::create(
            root.path(),
            &Meta {
                parent: None,
                depth: 0,
                id: "their-claude-b2c".to_string(),
                agent: Some("claude".to_string()),
                model: None,
                effort: None,
                session: Some("claude-session".to_string()),
                ..meta()
            },
        )
        .unwrap();
        let mut out = Vec::new();
        run(
            None,
            root.path(),
            &mut r#"{"session_id":"claude-session","hook_event_name":"UserPromptSubmit"}"#
                .as_bytes(),
            &mut out,
            &quiet(),
            None,
        );
        assert!(out.is_empty(), "{}", String::from_utf8_lossy(&out));

        // A report belonging to no record gets nothing.
        let mut out = Vec::new();
        run(
            None,
            root.path(),
            &mut r#"{"session_id":"nobodys","hook_event_name":"agent_start"}"#.as_bytes(),
            &mut out,
            &quiet(),
            None,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn hook_takes_the_answer_from_the_transcript_when_the_payload_has_none() {
        let root = TempDir::new().unwrap();
        let agent = an_agent(root.path());
        let transcript = root.path().join("session.jsonl");
        std::fs::write(
            &transcript,
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"from the transcript\"}]}}\n",
        )
        .unwrap();

        hook(
            root.path(),
            agent.id(),
            &json!({
                "hook_event_name": "SessionStart",
                "session_id": "abc-123",
                "transcript_path": transcript,
            })
            .to_string(),
        );
        hook(
            root.path(),
            agent.id(),
            r#"{"hook_event_name":"Stop","stop_hook_active":false}"#,
        );

        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Idle);
        assert_eq!(state.result.as_deref(), Some("from the transcript"));
        assert_eq!(state.source, Some(Source::Transcript));
    }

    #[test]
    fn hook_never_takes_the_vendors_note_about_a_turn_for_its_answer() {
        // claude ends a turn that never reached the model with a synthetic
        // entry and passes its text to the Stop hook as the answer.
        let root = TempDir::new().unwrap();
        let agent = an_agent(root.path());
        let transcript = root.path().join("session.jsonl");
        std::fs::write(
            &transcript,
            concat!(
                "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"fix it\"}}\n",
                "{\"type\":\"assistant\",\"message\":{\"model\":\"<synthetic>\",\
                 \"content\":[{\"type\":\"text\",\"text\":\"API Error: 529 Overloaded\"}]}}\n",
            ),
        )
        .unwrap();
        hook(
            root.path(),
            agent.id(),
            &json!({
                "hook_event_name": "SessionStart",
                "session_id": "abc-123",
                "transcript_path": transcript,
            })
            .to_string(),
        );
        hook(
            root.path(),
            agent.id(),
            r#"{"hook_event_name":"Stop","last_assistant_message":"API Error: 529 Overloaded"}"#,
        );

        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Idle);
        assert_eq!(state.result, None);
        assert_eq!(state.source, None);
    }

    #[test]
    fn hook_never_takes_an_aborted_answer_from_the_transcript_either() {
        // With the payload's answer refused, the transcript is asked, and pi
        // wrote the same partial sentence there.
        let root = TempDir::new().unwrap();
        let agent = Agent::create(
            root.path(),
            &Meta {
                agent: Some("pi".to_string()),
                ..meta()
            },
        )
        .unwrap();
        let transcript = root.path().join("session.jsonl");
        std::fs::write(
            &transcript,
            "{\"type\":\"message\",\"id\":\"a1\",\"parentId\":null,\"message\":{\"role\":\"assistant\",\
             \"content\":[{\"type\":\"text\",\"text\":\"Let me look at the\"}],\"stopReason\":\"aborted\"}}\n",
        )
        .unwrap();
        hook(
            root.path(),
            agent.id(),
            &json!({
                "hook_event_name": "session_start",
                "session_id": "abc-123",
                "transcript_path": transcript,
            })
            .to_string(),
        );
        hook(
            root.path(),
            agent.id(),
            r#"{"hook_event_name":"agent_settled","stop_reason":"aborted","last_assistant_message":"Let me look at the"}"#,
        );

        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Idle);
        assert_eq!(state.result, None);
    }

    #[test]
    fn hook_takes_the_sessions_title_from_the_transcript() {
        // The title lives only in the transcript, and the turn-end payload
        // carries the answer, so the title must be read regardless.
        let root = TempDir::new().unwrap();
        let agent = an_agent(root.path());
        let transcript = root.path().join("session.jsonl");
        let titled = concat!(
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"done\"}]}}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"Fix the login bug\",\"sessionId\":\"abc-123\"}\n",
        );
        std::fs::write(&transcript, titled).unwrap();

        // The first event naming the transcript is enough, so the row shows the
        // title from the first tool call.
        hook(
            root.path(),
            agent.id(),
            &json!({
                "hook_event_name": "SessionStart",
                "session_id": "abc-123",
                "transcript_path": transcript,
            })
            .to_string(),
        );
        assert_eq!(
            agent.state().unwrap().session_title.as_deref(),
            Some("Fix the login bug"),
            "read before any turn has ended"
        );

        // A newer title reaches the record on the next event, mid-turn
        // included.
        std::fs::write(
            &transcript,
            concat!(
                "{\"type\":\"ai-title\",\"aiTitle\":\"Fix the login bug\",\"sessionId\":\"abc-123\"}\n",
                "{\"type\":\"custom-title\",\"customTitle\":\"auth\",\"sessionId\":\"abc-123\"}\n",
            ),
        )
        .unwrap();
        hook(
            root.path(),
            agent.id(),
            r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"cargo test"}}"#,
        );
        assert_eq!(
            agent.state().unwrap().session_title.as_deref(),
            Some("auth"),
            "the name a person typed, taken mid-turn"
        );
        // Restore the title the rest of the test expects.
        std::fs::write(&transcript, titled).unwrap();
        hook(
            root.path(),
            agent.id(),
            r#"{"hook_event_name":"Stop","last_assistant_message":"done"}"#,
        );
        assert_eq!(
            agent.state().unwrap().session_title.as_deref(),
            Some("Fix the login bug")
        );

        // A transcript with no title leaves the recorded one.
        std::fs::write(&transcript, "{\"type\":\"attachment\"}\n").unwrap();
        hook(
            root.path(),
            agent.id(),
            r#"{"hook_event_name":"Stop","last_assistant_message":"done"}"#,
        );
        assert_eq!(
            agent.state().unwrap().session_title.as_deref(),
            Some("Fix the login bug")
        );
    }

    #[test]
    fn hook_coherence_the_turn_that_ended_and_then_read_as_waiting() {
        // A recorded session, hook by hook: a turn ended with an answer, the
        // idle nudge came a minute later, and the command exited after that.
        // The record ended up `done` with "Claude is waiting for your input"
        // as its question, so `ls` showed done beside a waiting line.
        let root = TempDir::new().unwrap();
        let agent = an_agent(root.path());
        let answer = "Three that made me stop and re-read:";

        for payload in [
            json!({
                "hook_event_name": "SessionStart",
                "session_id": "34011b84-4d68-4108-a4f9-38a068bb2ae6",
                "source": "startup"
            }),
            json!({ "hook_event_name": "UserPromptSubmit", "prompt": "read README.md" }),
            json!({ "hook_event_name": "PreToolUse", "tool_name": "Read" }),
            json!({ "hook_event_name": "Stop", "last_assistant_message": answer }),
            json!({
                "hook_event_name": "Notification",
                "message": "Claude is waiting for your input",
                "notification_type": "idle_prompt"
            }),
        ] {
            hook(root.path(), agent.id(), &payload.to_string());
        }

        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Idle);
        assert_eq!(state.result.as_deref(), Some(answer));
        assert_eq!(state.source, Some(Source::Payload));
        assert_eq!(state.question, None, "and nothing is outstanding");

        exited(root.path(), agent.id(), 0, &quiet());
        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Done);
        assert_eq!(state.result.as_deref(), Some(answer));
        assert_eq!(state.question, None);
    }

    #[test]
    fn hook_coherence_a_command_that_has_ended_is_asking_nobody_anything() {
        // The pane is gone with the command, so a question left here could
        // never be answered and would stand in front of the result for good.
        let root = TempDir::new().unwrap();
        let agent = an_agent(root.path());
        agent
            .writer()
            .unwrap()
            .update_state(|state| {
                state.state = Phase::Waiting;
                state.question = Some("Which fixture should the port keep?".to_string());
                state.options = vec!["the sqlite one".to_string()];
                state.kind = Some(Kind::Question);
            })
            .unwrap();

        exited(root.path(), agent.id(), 1, &quiet());
        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Failed);
        assert_eq!(state.question, None);
        assert!(state.options.is_empty());
        assert_eq!(state.kind, None);
    }

    #[test]
    fn hook_ends_in_silence_whatever_it_is_handed() {
        let root = TempDir::new().unwrap();
        let agent = an_agent(root.path());

        // No agent named: this claude is not amx's.
        assert_eq!(
            run(
                None,
                root.path(),
                &mut "{}".as_bytes(),
                &mut std::io::sink(),
                &quiet(),
                None,
            ),
            exit::OK
        );
        // An id with no record.
        assert_eq!(hook(root.path(), "never-made-abc", "{}"), exit::OK);
        // An id with illegal characters.
        assert_eq!(hook(root.path(), "../elsewhere", "{}"), exit::OK);
        // Payloads that are not JSON objects.
        assert_eq!(hook(root.path(), agent.id(), "not json at all"), exit::OK);
        assert_eq!(hook(root.path(), agent.id(), ""), exit::OK);
        assert_eq!(hook(root.path(), agent.id(), "[1, 2, 3]"), exit::OK);

        assert_eq!(
            agent.state().unwrap().state,
            Phase::Starting,
            "and none of it moved the record"
        );
    }

    #[test]
    fn hook_exit_records_how_the_command_ended() {
        let root = TempDir::new().unwrap();
        let agent = an_agent(root.path());

        assert_eq!(exited(root.path(), agent.id(), 0, &quiet()), exit::OK);
        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Done);
        assert_eq!(state.exit, Some(0));

        let events = agent.events().unwrap();
        assert_eq!(events.last().unwrap().kind, "exit");
        assert_eq!(events.last().unwrap().payload["code"], 0);
    }

    #[test]
    fn hook_exit_with_a_code_is_a_failure() {
        let root = TempDir::new().unwrap();
        let agent = an_agent(root.path());

        exited(root.path(), agent.id(), 2, &quiet());
        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Failed);
        assert_eq!(state.exit, Some(2));
    }

    #[test]
    fn hook_exit_does_not_relabel_an_agent_somebody_stopped() {
        // `stop` signals the pane, so the command exits with a signal's code;
        // it stays stopped, not failed.
        let root = TempDir::new().unwrap();
        let agent = an_agent(root.path());
        agent
            .writer()
            .unwrap()
            .update_state(|s| s.state = Phase::Stopped)
            .unwrap();

        exited(root.path(), agent.id(), 143, &quiet());
        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Stopped);
        assert_eq!(state.exit, Some(143), "the code is still worth recording");
    }

    #[test]
    fn hook_exit_ends_in_silence_when_there_is_no_record() {
        let root = TempDir::new().unwrap();
        assert_eq!(exited(root.path(), "never-made-abc", 1, &quiet()), exit::OK);
        assert_eq!(exited(root.path(), "../elsewhere", 1, &quiet()), exit::OK);
    }
}
