//! `amx _hook` and `amx _exit` — the two commands amx runs against itself.
//!
//! `_hook` is wired into the vendor's settings and fires on the events amx
//! listens to. It reads one payload on stdin, appends it to the agent's event
//! log, folds it into the agent's state, and **always exits 0**. Every way it
//! can fail — nothing that says whose the payload is, a record that is not
//! there, a payload that is not JSON — ends in silence, because a hook that
//! fails is a hook that interrupts somebody's agent to tell them about amx.
//!
//! Whose the payload is has two answers. A pane amx started carries the id in
//! its environment, and that is the whole of it. A claude that was already
//! there when amx arrived carries nothing, so the payload's own session id is
//! what finds the record — see [`by_session`].
//!
//! It touches nothing but the agent's own directory. In particular it makes no
//! tmux calls: this runs on every prompt and every tool call, and the pane it
//! would ask about is the one waiting for it to return.
//!
//! `_exit` runs after the vendor's command in the same pane, and records how
//! it ended before the pane closes.

use anyhow::Result;
use serde_json::Value;
use std::io::Read;
use std::path::Path;

use crate::config::Config;
use crate::exit;
use crate::notify::{self, Notice};
use crate::store::{Agent, Ask, Choice, Kind, Meta, Phase, Source, State};

/// How the hook learns which agent it belongs to. `_boot` puts it in the
/// pane's environment, so every process the vendor starts inherits it.
pub const ID_ENV: &str = "AMX_ID";

/// Record one hook payload. Answers with the process's exit code, which is
/// always `OK`.
pub fn from_env(stdin: &mut impl Read, config: &Config) -> i32 {
    let id = std::env::var(ID_ENV).ok();
    let Ok(root) = crate::paths::state_root() else {
        return exit::OK;
    };
    run(id.as_deref(), &root, stdin, config)
}

/// The same, with everything it touches named.
pub fn run(id: Option<&str>, root: &Path, stdin: &mut impl Read, config: &Config) -> i32 {
    // Every early return here is a hook that is not amx's business, or a
    // record amx cannot reach. Both end quietly: this process is standing
    // between the vendor and its next token.
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

    let _ = record(&agent, &payload, config);
    exit::OK
}

/// Which agent a payload belongs to.
///
/// The environment first, because a pane amx started says so itself and says
/// it without reading anything. An id that names no record is not a reason to
/// go looking: the pane answered, and the answer was an agent that is gone.
fn whose(id: Option<&str>, root: &Path, payload: &Value) -> Option<Agent> {
    match id {
        Some(id) => Agent::open(root, id).ok(),
        None => by_session(root, payload["session_id"].as_str()?),
    }
}

/// The record for a session, for the payloads that arrive with nothing else
/// saying whose they are.
///
/// A claude `amx adopt` took over was launched by somebody else, so amx never
/// put its id in that pane's environment and never will — the session the
/// vendor stamps on every payload is the only thing tying the two together,
/// and it is on the record because adopting is what wrote it there.
///
/// One conversation can be on two records: an agent amx started and stopped,
/// and the claude somebody resumed it in by hand and adopted. A payload is
/// about a session that is running, so a record that has ended is not it, and
/// the newest of what is left answers for the rest.
///
/// This reads every record on the machine, and it runs for every claude amx
/// did not start, adopted or not — a hook fired by a pane amx opened never
/// arrives here at all. What it costs is a directory listing and two small
/// files per agent, against the several milliseconds this process took to
/// start; an index of sessions would be quicker and would be one more thing
/// that can disagree with the records it was built from.
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

/// The same, with the state root named.
pub fn exited(root: &Path, id: &str, code: i32, config: &Config) -> i32 {
    let Ok(agent) = Agent::open(root, id) else {
        return exit::OK;
    };
    let _ = record_exit(&agent, code, config);
    exit::OK
}

/// Fold one payload into an agent's record, under the writer's lock.
pub fn record(agent: &Agent, payload: &Value, config: &Config) -> Result<()> {
    let writer = agent.writer()?;
    writer.append(&crate::store::Event::new(
        kind(payload).unwrap_or("unknown"),
        payload.clone(),
    ))?;

    let mut meta = agent.meta()?;
    let mut state = writer.state()?;
    let before = meta.clone();
    let notice = apply(payload, &mut state, &mut meta);

    // The transcript is the second place an answer can be, and it is read only
    // when the payload had none. Reading a file is all this costs; asking the
    // pane would mean a tmux call on the hook path, which is not this
    // command's to make.
    if state.state == Phase::Idle
        && state.result.is_none()
        && let Some(path) = &meta.transcript
        && let Ok(text) = std::fs::read_to_string(path)
        && let Some(answer) = transcript_answer(&text)
    {
        state.result = Some(answer);
        state.source = Some(Source::Transcript);
    }

    writer.update_state(|current| *current = state)?;
    if meta != before {
        writer.update_meta(|current| *current = meta)?;
    }
    drop(writer);

    if let Some(notice) = notice
        && config.notifications
    {
        notify::post(&notice);
    }
    Ok(())
}

/// Record how the command ended, and tell somebody if it is worth telling.
fn record_exit(agent: &Agent, code: i32, config: &Config) -> Result<()> {
    let writer = agent.writer()?;
    writer.append(&crate::store::Event::new(
        "exit",
        serde_json::json!({ "code": code }),
    ))?;

    let state = writer.update_state(|state| {
        state.exit = Some(code);
        // The pane goes with the command, so a question left on the record
        // here is one nobody can answer and nothing can deliver an answer to.
        // It would also be the last thing every reader said about this agent,
        // in front of the answer it did give.
        state.asks(None);
        // An agent somebody stopped exits with a signal's code moments later.
        // That is not how it ended; being stopped is.
        if state.state != Phase::Stopped {
            state.state = if code == 0 {
                Phase::Done
            } else {
                Phase::Failed
            };
        }
    })?;
    drop(writer);

    if config.notifications
        && let Some(notice) = Notice::finished(agent.id(), state.state, state.exit)
    {
        notify::post(&notice);
    }
    Ok(())
}

/// The event a payload is about.
fn kind(payload: &Value) -> Option<&str> {
    payload["hook_event_name"].as_str()
}

/// What one payload means for the record.
///
/// The mapping, in full:
///
/// * `SessionStart` records the vendor's session id and the transcript it
///   writes, and moves nothing. Only this event may set the session id —
///   every payload carries one, and a subagent's is not the agent's.
/// * `UserPromptSubmit` and `PreToolUse` mean the agent is working, and the
///   tool call says what it is doing. The one tool that is not work is
///   `AskUserQuestion`: it draws a menu and waits, and its payload carries
///   every question the menu will ask, whole.
/// * `PermissionRequest` is the permission box the instant it goes up, tool
///   and all; `PermissionDenied` is the only thing that says it closed with
///   the tool refused, after which the turn is working again.
/// * `Notification` means it has stopped on a question, and carries its words.
///   The choices under it are on the pane, which this command does not read;
///   a reader fills them in later. Its `notification_type` is the vendor's own
///   word for what the notice is about: it is the only thing here that names a
///   permission prompt for what it is, and the only thing that tells the
///   nudge about an idle session apart from a question, which it is not.
/// * `Stop` ends the turn, and its payload is the freshest place the answer
///   ever exists — the transcript is written asynchronously and lags it.
/// * Anything carrying an `agent_id` is a subagent's, and a subagent's work is
///   not the agent's state.
/// * A record that has already ended stays ended. A late hook is a hook about
///   a turn that is over.
///
/// What comes back is the one notice a stop is worth, and one for every stop.
/// Somebody is told when a screen goes up that nothing has told them about,
/// with the question if the event that put it there carried one, and anything
/// repeating a screen already up is folded in silently. Three of the events
/// above end in waiting and one `AskUserQuestion` fires all three — the tool
/// call that draws the menu, the permission box over that same tool, and the
/// notification that repeats the box. They are three different sentences, so
/// nothing but the record's own phase tells one stop from three.
///
/// The phase alone would then fold two stops into one. Nothing amx installs
/// fires when a box is approved or a menu answered — the vendor says so with
/// `PostToolUse`, which is not one of amx's events — so the record still reads
/// waiting when the next screen goes up, and the person is told about a box
/// they have answered and not about the menu in front of them. A menu says for
/// itself that it is a stop of its own.
pub fn apply(payload: &Value, state: &mut State, meta: &mut Meta) -> Option<Notice> {
    if !payload["agent_id"].is_null() || state.state.is_terminal() {
        return None;
    }
    let was_waiting = state.state == Phase::Waiting;

    let screen = match kind(payload)? {
        "SessionStart" => {
            if let Some(session) = payload["session_id"].as_str() {
                meta.session = Some(session.to_string());
            }
            if let Some(transcript) = payload["transcript_path"].as_str() {
                meta.transcript = Some(transcript.into());
            }
            Screen::Clear
        }

        "UserPromptSubmit" => {
            state.state = Phase::Working;
            state.summary = None;
            state.asks(None);
            // A new turn retires the last one's answer. A turn that ends
            // without one would otherwise leave the previous answer on the
            // record, and `result` would hand it to a caller as this turn's.
            state.result = None;
            state.source = None;
            Screen::Clear
        }

        "PreToolUse" if payload["tool_name"] == "AskUserQuestion" => {
            state.state = Phase::Waiting;
            // Not running anything: the menu is what it is doing.
            state.summary = None;
            state.asks_all(asked(&payload["tool_input"]));
            state.kind = Some(Kind::Question);
            // Nothing else will say this agent is waiting. The vendor notifies
            // about an idle session only when nothing is open on it, and a
            // menu is open on this one.
            Screen::Fresh
        }

        "PreToolUse" => {
            state.state = Phase::Working;
            if let Some(tool) = payload["tool_name"].as_str() {
                state.summary = Some(format!("Running {tool}"));
            }
            state.asks(None);
            Screen::Clear
        }

        // Fired as the permission box goes up — six seconds before the
        // notification that repeats it, which was the whole of what said so
        // before this event was wired. The payload carries the tool and not
        // the vendor's sentence, so the sentence is written here the way the
        // vendor will write it, and a box with no tool named waits for a
        // reader to quote the pane.
        "PermissionRequest" => {
            state.state = Phase::Waiting;
            state.summary = None;
            state.asks(
                payload["tool_name"]
                    .as_str()
                    .map(|tool| format!("Claude needs your permission to use {}", rendered(tool))),
            );
            state.kind = Some(Kind::Permission);
            Screen::Waiting
        }

        // The one hook that says the box closed without the tool running: no
        // PostToolUse follows a tool that never ran. The turn goes on with
        // the refusal in it, and the next tool call will say what the agent
        // is doing now.
        "PermissionDenied" => {
            state.state = Phase::Working;
            state.summary = None;
            state.asks(None);
            Screen::Clear
        }

        // The vendor nudges about an idle session only when nothing is open on
        // it: no permission box, no menu, nobody being asked for anything. It
        // is the turn being over, said a minute late, and the record has often
        // said so already with the answer on it. Its words are about the
        // session rather than about anything to answer, so they are not the
        // question, and whatever amx thought was outstanding is not on that
        // screen either.
        "Notification" if payload["notification_type"] == "idle_prompt" => {
            state.state = Phase::Idle;
            state.summary = None;
            state.asks(None);
            Screen::Clear
        }

        "Notification" => {
            // An untyped notification about a turn that already ended with an
            // answer can only be an older vendor's idle nudge wearing no
            // name: a vendor notifies about an idle session when nothing is
            // open on it, and an answered turn has nothing open. Its words
            // are not a question, and taking them as one would put the record
            // back to waiting in front of the answer it holds.
            if state.state == Phase::Idle
                && state.result.is_some()
                && payload["notification_type"].is_null()
            {
                return None;
            }
            state.state = Phase::Waiting;
            state.asks(payload["message"].as_str().map(str::to_string));
            if payload["notification_type"] == "permission_prompt" {
                state.kind = Some(Kind::Permission);
            }
            Screen::Waiting
        }

        "Stop" => {
            state.state = Phase::Idle;
            state.summary = None;
            state.asks(None);
            if let Some(answer) = payload["last_assistant_message"].as_str() {
                state.result = Some(answer.to_string());
                state.source = Some(Source::Payload);
            }
            Screen::Clear
        }

        _ => Screen::Clear,
    };

    // One stop, one interruption. An agent that was already waiting when a box
    // or a notification arrived is one somebody has been told about, whatever
    // this event calls the thing it is waiting on — and a menu is a stop
    // nobody has been told about however the record reads.
    let told = match screen {
        Screen::Fresh => true,
        Screen::Waiting => !was_waiting,
        Screen::Clear => false,
    };
    told.then(|| Notice::waiting(&meta.id, state.question.as_deref()))
}

/// What one event leaves on the agent's pane.
enum Screen {
    /// Nothing to answer: the agent is working, or the event moved nothing.
    Clear,
    /// Something to answer, which may well be what was already there — the box
    /// over a call amx has seen, or the notification that repeats the box.
    /// Whether this is the first anybody has heard of it is the record's own
    /// phase to say.
    Waiting,
    /// Something to answer that was not there a moment ago. A menu is this
    /// wherever it lands: `PreToolUse` fires before the vendor has asked
    /// anybody whether the call may run at all, so the call it names has not
    /// been on the pane before, and whatever the record was waiting on is
    /// behind it — answered, approved, or gone with the tool that ran.
    Fresh,
}

/// A tool's name the way the vendor writes it into the permission sentence,
/// measured at 2.1.237: the last `__` segment — an MCP tool arrives as
/// `mcp__<server>__<tool>` — with underscores as spaces and a letter raised
/// wherever a word starts, which is after anything that is not a letter or a
/// digit (the vendor's `\b\w`), not only after an underscore. That carries a
/// kebab-case name past its dashes, leaves a built-in like `Bash` as it
/// stands, and keeps a digit's word one word. The sentence written at
/// `PermissionRequest` has to be the one the notification will repeat: it is
/// what every reader quotes for the six seconds until the echo lands, and the
/// echo writes the vendor's own words over it.
fn rendered(tool: &str) -> String {
    let mut boundary = true;
    tool.rsplit("__")
        .next()
        .unwrap_or(tool)
        .chars()
        .map(|letter| {
            let letter = if letter == '_' { ' ' } else { letter };
            let raised = if boundary {
                letter.to_ascii_uppercase()
            } else {
                letter
            };
            boundary = !letter.is_ascii_alphanumeric();
            raised
        })
        .collect()
}

/// Every question an `AskUserQuestion` call is about to put on the pane.
///
/// The tool takes up to four and draws them as tabs on one screen, asking them
/// one at a time, so the first is the one showing and the rest are a keystroke
/// behind it. All of them are taken: what they are called, the sentences under
/// their choices, the previews that turn the notes field on and the flag that
/// says how many choices each takes are in the payload and nowhere else, and a
/// pane narrow enough to elide the tab strip does not even say how many there
/// are.
///
/// A question with no words is not one anybody can be asked, so it is left
/// out rather than written down empty.
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

/// The choices under one question. A choice is its label: the sentence beside
/// it explains it to whoever is reading, but the label is what an answer names
/// and what comes back when it is chosen.
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

/// The answer at the end of a transcript, if the turn has ended.
///
/// Tool results are `user` lines — there are ten of them for every real turn —
/// so a trailing `user` line means the turn is still running, and answering
/// with the last assistant line would serve the *previous* turn's answer as
/// this one's. That is the unrecoverable direction to be wrong in, so it
/// answers with nothing instead. `attachment` lines are bookkeeping; anything
/// else is turn content.
pub fn transcript_answer(text: &str) -> Option<String> {
    let lines: Vec<Value> = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["type"] != "attachment")
        .collect();

    if lines.last()?["type"] == "user" {
        return None;
    }
    lines
        .iter()
        .rev()
        .find(|line| line["type"] == "assistant")
        .and_then(assistant_text)
}

/// The text of one assistant line.
fn assistant_text(line: &Value) -> Option<String> {
    let content = line["message"]["content"].as_array()?;
    let text: Vec<&str> = content
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect();
    (!text.is_empty()).then(|| text.join("\n").trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Meta;
    use crate::tmux::{PaneId, Socket};
    use serde_json::json;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn meta() -> Meta {
        Meta {
            id: "fix-login-a1b".to_string(),
            task: "fix the login bug".to_string(),
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

    fn quiet() -> Config {
        Config {
            notifications: false,
            ..Config::default()
        }
    }

    /// Fold a payload into a fresh record and answer with what came of it.
    fn fold(payload: Value) -> (State, Meta, Option<Notice>) {
        let mut state = State::default();
        let mut meta = meta();
        let notice = apply(&payload, &mut state, &mut meta);
        (state, meta, notice)
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
    fn hook_a_new_turn_retires_the_last_turns_answer() {
        // A turn that ends with nothing to say leaves whatever the last one
        // said on the record, and `result` reads that record.
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
        // The options were read under one particular question. Under the next
        // one they are somebody else's answers, and offering them to a caller
        // would be offering it the wrong keys to press.
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
        // Measured against claude 2.1.237: the notification that a permission
        // box is up carries notification_type `permission_prompt` beside the
        // message. The words alone would not do. They are one sentence about
        // one tool, and every other notice the vendor sends is a sentence in
        // the same shape.
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
        // Measured at 2.1.237: PermissionRequest fires as the box goes up,
        // six seconds before the notification that repeats it. The payload
        // carries the tool and not the vendor's sentence, so the sentence is
        // written the way the vendor will write it.
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

        // A box amx cannot name is still a permission box somebody has to
        // answer, and the pane's own words reach the record from a reader.
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
    fn hook_one_stop_interrupts_once_however_many_events_say_so() {
        // Filed on 2026-08-24 off the first run of the wall: one
        // AskUserQuestion put three notifications on the desktop. The tool
        // call that draws the menu, the permission box over that same tool
        // and the notification that repeats the box a few seconds later are
        // three different sentences, so matching the words catches none of
        // them. What they have in common is that the agent had already
        // stopped when they arrived.
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
        // The payloads themselves, captured on 2026-08-24 from a claude
        // 2.1.240 in a tmux session of its own, in default permission mode,
        // with a hook on every event appending its stdin to a file. One
        // AskUserQuestion fired the tool call, the permission box over that
        // same tool and the notification that repeats it, in this order, and
        // all three end in waiting.
        //
        // The notification's message names no tool: it is "Claude needs your
        // permission" and nothing more, against the box's own sentence and
        // the menu's question. No reading of the words could have told one
        // stop from three.
        //
        // What the last two do to the question on the record is the kind
        // precedence 02BQ6442 is about, and nothing here says it is right.
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
    }

    #[test]
    fn hook_coherence_the_menu_behind_an_answered_box_is_its_own_stop() {
        // One turn, two stops, captured on 2026-08-25 from a claude 2.1.240 in
        // a tmux pane in default permission mode. The box over a Bash command
        // went up, was answered with 1, and the menu was on the pane a moment
        // later. Between them the vendor fired PostToolUse(Bash) — not one of
        // the seven events amx installs, so it is left out here as it is left
        // out there. Nothing that does reach this function said the box had
        // closed, and the record still read waiting when the menu went up.
        //
        // Two screens is two people have to be interrupted. The box's notice
        // is spent by the time the menu arrives, and the menu is what is on
        // the pane now.
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
        // The same claude and the same pane, asked for two questions in one
        // turn: menu, answer, menu. Answering the first fired PostToolUse,
        // which amx does not install, so the record reads waiting from the
        // first menu straight through to the second. Two questions is two
        // things somebody has to answer, and the second is the one on the
        // pane.
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

        // Each notice went out on the menu going up, which is the event that
        // knew what it was asking. What the box and the notification behind
        // each menu then do to the question on the record is the kind
        // precedence 02BQ6442 is about, and nothing here says it is right.
        assert_eq!(told.len(), 2, "two menus, two notices: {told:?}");
        assert_eq!(told[0].body, "Which fixture should the port keep?");
        assert_eq!(
            told[1].body, "Which port should be bound?",
            "the second question is the one still up"
        );
    }

    #[test]
    fn hook_a_stop_amx_cannot_name_is_still_one_interruption() {
        // A permission box whose payload names no tool goes up with nothing
        // amx can quote, so the notice says what it can. The words arrive six
        // seconds later with the notification and they reach the record,
        // which is where a reader looks; nobody is told a second time.
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
        // One notice per stop, not one per agent. Whatever put the agent back
        // to work — a box refused, a menu answered, a new prompt — the next
        // thing it stops on is news.
        let mut state = State::default();
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
        // The payload names the tool `mcp__<server>__<tool>`; the vendor's
        // sentence — on the box, and in the notification that repeats it —
        // renders the last `__` segment with underscores as spaces and each
        // word's first letter raised. A sentence written any other way is
        // what every reader quotes while the box is up, against a pane that
        // says something else.
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
    fn hook_a_tools_name_is_raised_at_every_word_boundary() {
        // The vendor raises a letter wherever a word starts — after anything
        // that is not a letter or a digit — not only after an underscore.
        // Raised the underscore way, a kebab-case name reads
        // 'Resolve-library-id' against the pane's 'Resolve-Library-Id', and
        // the record hands whoever is answering a sentence nothing drew.
        assert_eq!(
            rendered("mcp__context7__resolve-library-id"),
            "Resolve-Library-Id"
        );
        assert_eq!(rendered("mcp__playwright__browser_click"), "Browser Click");
        assert_eq!(rendered("mcp__acme__fs.read_file"), "Fs.Read File");
        // A digit neither opens a word nor ends one: nothing raises after it.
        assert_eq!(rendered("mcp__totp__get2fa-codes"), "Get2fa-Codes");
        assert_eq!(rendered("Bash"), "Bash");
    }

    #[test]
    fn hook_a_denied_permission_puts_the_agent_back_to_work() {
        // The one hook that says a box has closed without the tool running:
        // no PostToolUse follows a tool that never ran, so without this the
        // record would say waiting for as long as the turn went on.
        let mut state = State {
            state: Phase::Waiting,
            question: Some("Claude needs your permission to use Bash".to_string()),
            options: vec!["Yes".to_string(), "No".to_string()],
            kind: Some(Kind::Permission),
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
    fn hook_a_notification_of_no_named_kind_leaves_the_kind_where_it_was() {
        // Every notification amx saw before this carried no type at all, and
        // it still means what it always meant: this agent has stopped, and
        // here are the words it stopped on.
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
        // The vendor only nudges about an idle session when nothing is open on
        // it: no permission box, no menu. Nobody is being asked for anything;
        // the turn is over. Taking the nudge's own words as the question would
        // leave "Claude is waiting for your input" on the record as the thing
        // to answer, and every reader would say the agent was waiting.
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
    fn hook_coherence_an_untyped_nudge_never_undoes_an_answered_turn() {
        // An older vendor types nothing on its notifications, and its idle
        // nudge arrives with no name. One arriving after the turn ended with
        // an answer on the record can only be that nudge: the vendor notifies
        // about an idle session when nothing is open on it, and an answered
        // turn has nothing open. Taking its words as a question would flip
        // the record back to waiting in front of the answer it holds.
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
        // AskUserQuestion is a tool call, so the only hook it fires is the one
        // that says a tool is about to run. It never runs in the sense the
        // others do: it draws a menu and waits. The question and the choices
        // are in the payload, which is earlier and surer than the pane.
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
        // The payload measured against 2.1.240 on 2026-08-24 and recorded in
        // docs/question-shapes.md: three questions in one call, drawn as tabs,
        // with multiSelect per question rather than per call. Only the first
        // is on the screen; the rest are one keystroke behind it, and the
        // payload is the only place they are ever written down.
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

        // The question on the screen, where every reader looks for it.
        assert_eq!(
            state.question.as_deref(),
            Some("Which runtime should the service target?")
        );
        assert_eq!(state.options, ["Node", "Deno"]);

        // And the two behind it, with what no screen carries.
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
        // Nothing else does. The screen draws `[ ]` boxes and a Submit row,
        // and a reader that took the labels off it would come back with
        // `[ ] Logging` for a label and no way to tell the shape apart from a
        // plain menu at the moment the record is written.
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
        // Measured against 2.1.240: `preview` on any choice is what draws the
        // notes field, and `n` on a menu without one does nothing at all. The
        // previewed screen also drops the free-text row and the number on
        // `Chat about this`, so nothing on the pane tells the two apart.
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
        // A menu amx cannot quote is still a menu somebody has to answer.
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
        assert_eq!(notice, None, "an idle agent is on the wall already");
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

        // Every payload carries a session id; only this event may set it.
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

    #[test]
    fn hook_the_transcript_answers_only_when_the_turn_has_ended() {
        let ended = "\
{\"type\":\"user\",\"message\":{\"content\":\"fix the login bug\"}}
{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"I fixed it.\"}]}}
";
        assert_eq!(transcript_answer(ended).as_deref(), Some("I fixed it."));

        // Tool results are `user` lines, and there are ten of them for every
        // real turn. A trailing one means the turn is still going, and the
        // last assistant line belongs to the turn before it.
        let still_going = format!(
            "{ended}{}\n",
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"content\":\"ok\"}]}}"
        );
        assert_eq!(transcript_answer(&still_going), None);
    }

    #[test]
    fn hook_the_transcript_reads_past_its_own_bookkeeping() {
        let with_noise = "\
{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"the answer\"}]}}
{\"type\":\"attachment\",\"note\":\"bookkeeping\"}
";
        assert_eq!(
            transcript_answer(with_noise).as_deref(),
            Some("the answer"),
            "an attachment is not the end of a turn"
        );

        assert_eq!(transcript_answer(""), None);
        assert_eq!(transcript_answer("{not json\n"), None);
        assert_eq!(
            transcript_answer("{\"type\":\"assistant\",\"message\":{\"content\":[]}}\n"),
            None,
            "an assistant line with nothing in it is not an answer"
        );
    }

    #[test]
    fn hook_thinking_and_tool_blocks_are_not_the_answer() {
        let mixed = "{\"type\":\"assistant\",\"message\":{\"content\":[\
            {\"type\":\"thinking\",\"thinking\":\"hmm\"},\
            {\"type\":\"text\",\"text\":\"the answer\"},\
            {\"type\":\"tool_use\",\"name\":\"Bash\"}]}}\n";
        assert_eq!(transcript_answer(mixed).as_deref(), Some("the answer"));
    }

    // ── The commands themselves ──────────────────────────────────────────────

    fn an_agent(root: &Path) -> Agent {
        Agent::create(root, &meta()).unwrap()
    }

    fn hook(root: &Path, id: &str, payload: &str) -> i32 {
        run(Some(id), root, &mut payload.as_bytes(), &quiet())
    }

    /// The document on disk, as a caller that is not amx would find it.
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
    fn hook_an_adopted_claude_is_found_by_the_session_its_payload_names() {
        // A claude amx did not start has no `AMX_ID` in its pane, so the only
        // thing in the payload that says whose it is is the session — which is
        // what `adopt` wrote down.
        let root = TempDir::new().unwrap();
        let adopted = Agent::create(
            root.path(),
            &Meta {
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
                &quiet(),
            ),
            exit::OK
        );
        let state = adopted.state().unwrap();
        assert_eq!(state.state, Phase::Working);
        assert_eq!(state.summary.as_deref(), Some("Running Bash"));
        assert_eq!(adopted.events().unwrap().len(), 1, "and it is written down");

        // A session amx has no record of is a claude that is nobody's.
        assert_eq!(
            run(
                None,
                root.path(),
                &mut r#"{"session_id":"def-456","hook_event_name":"Stop","last_assistant_message":"done"}"#
                    .as_bytes(),
                &quiet(),
            ),
            exit::OK
        );
        assert_eq!(adopted.state().unwrap().result, None);
    }

    #[test]
    fn hook_an_adopted_session_reaches_the_record_that_is_still_running() {
        // One conversation can be on two records: an agent amx started and
        // stopped, and the claude somebody resumed it in by hand and adopted.
        // The payload is about the one that has not ended.
        let root = TempDir::new().unwrap();
        let stopped = Agent::create(
            root.path(),
            &Meta {
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
            &quiet(),
        );

        assert_eq!(adopted.state().unwrap().state, Phase::Working);
        assert_eq!(
            stopped.state().unwrap().state,
            Phase::Stopped,
            "and a record that has ended stays where it was"
        );
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
    fn hook_coherence_the_turn_that_ended_and_then_read_as_waiting() {
        // Recorded on 2026-08-20 from the agent read-readme-md-and-799, hook
        // for hook: a turn ended with an answer, the vendor nudged about the
        // idle session a minute later, and the command exited a minute after
        // that. What was left on disk said `done` with an answer on it and
        // "Claude is waiting for your input" as the question, so `ls` called
        // the agent done while the line beside it said it was waiting.
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
        // The pane is gone with the command, so there is nothing left to
        // answer and nowhere to send an answer. A question left here outlives
        // every turn, and `line` puts it in front of the result for good.
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

        // No agent named at all: this claude is not one of amx's.
        assert_eq!(
            run(None, root.path(), &mut "{}".as_bytes(), &quiet()),
            exit::OK
        );
        // An id with no record behind it.
        assert_eq!(hook(root.path(), "never-made-abc", "{}"), exit::OK);
        // An id that could not be one.
        assert_eq!(hook(root.path(), "../elsewhere", "{}"), exit::OK);
        // Payloads that are not payloads.
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
        // `stop` signals the pane, so the command exits with a signal's code
        // moments later. It was not a failure; it was stopped.
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
