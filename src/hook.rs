//! `amx _hook` and `amx _exit` — the two commands amx runs against itself.
//!
//! `_hook` is wired into the vendor's settings and fires on the events amx
//! listens to. It reads one payload on stdin, appends it to the agent's event
//! log, folds it into the agent's state, and **always exits 0**. Every way it
//! can fail — no agent named in the environment, a record that is not there, a
//! payload that is not JSON — ends in silence, because a hook that fails is a
//! hook that interrupts somebody's agent to tell them about amx.
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
use crate::store::{Agent, Meta, Phase, Source, State};

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
    let Some(id) = id else {
        return exit::OK;
    };
    let Ok(agent) = Agent::open(root, id) else {
        return exit::OK;
    };

    let mut text = String::new();
    if stdin.read_to_string(&mut text).is_err() {
        return exit::OK;
    }
    let Ok(payload) = serde_json::from_str::<Value>(&text) else {
        return exit::OK;
    };

    let _ = record(&agent, &payload, config);
    exit::OK
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
///   tool call says what it is doing.
/// * `Notification` means it has stopped on a question, and carries it.
/// * `Stop` ends the turn, and its payload is the freshest place the answer
///   ever exists — the transcript is written asynchronously and lags it.
/// * Anything carrying an `agent_id` is a subagent's, and a subagent's work is
///   not the agent's state.
/// * A record that has already ended stays ended. A late hook is a hook about
///   a turn that is over.
pub fn apply(payload: &Value, state: &mut State, meta: &mut Meta) -> Option<Notice> {
    if !payload["agent_id"].is_null() || state.state.is_terminal() {
        return None;
    }

    match kind(payload)? {
        "SessionStart" => {
            if let Some(session) = payload["session_id"].as_str() {
                meta.session = Some(session.to_string());
            }
            if let Some(transcript) = payload["transcript_path"].as_str() {
                meta.transcript = Some(transcript.into());
            }
            None
        }

        "UserPromptSubmit" => {
            state.state = Phase::Working;
            state.summary = None;
            state.question = None;
            None
        }

        "PreToolUse" => {
            state.state = Phase::Working;
            if let Some(tool) = payload["tool_name"].as_str() {
                state.summary = Some(format!("Running {tool}"));
            }
            state.question = None;
            None
        }

        "Notification" => {
            state.state = Phase::Waiting;
            state.question = payload["message"].as_str().map(str::to_string);
            Some(Notice::waiting(&meta.id, state.question.as_deref()))
        }

        "Stop" => {
            state.state = Phase::Idle;
            state.summary = None;
            state.question = None;
            if let Some(answer) = payload["last_assistant_message"].as_str() {
                state.result = Some(answer.to_string());
                state.source = Some(Source::Payload);
            }
            None
        }

        _ => None,
    }
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
