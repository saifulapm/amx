//! `amx events`: every agent's event log merged into one stream, one line per
//! event in time order, each labelled with its agent.
//!
//! `--follow` polls each log for what it grew since the last look; there is no
//! watcher or daemon. The default output is a sanitized table for a person;
//! `--json` prints one object per event with the payload whole.

use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;

use crate::store::{Agent, Event};
use crate::vendor::{Hooks, Moment};
use crate::verbs::send::SEND;
use crate::{exit, paths, store};

/// How often `--follow` polls: the resolution of the recorded timestamps.
const POLL: Duration = Duration::from_secs(1);

/// Width of the kind column: the longest vendor event name.
const KIND: usize = 16;

/// Run the verb against the machine.
pub fn from_env(ids: &[String], follow: bool, as_json: bool) -> Result<i32> {
    let root = paths::state_root()?;
    let mut out = std::io::stdout().lock();
    run(&root, ids, follow, as_json, &mut out)
}

/// The verb, with the state directory named.
pub fn run(
    root: &Path,
    ids: &[String],
    follow: bool,
    as_json: bool,
    out: &mut impl Write,
) -> Result<i32> {
    // Refuse an unknown id up front rather than silently watching nothing.
    let mut named = ids.to_vec();
    named.sort();
    named.dedup();
    for id in &named {
        Agent::open(root, id)?;
    }

    let mut tails = Tails::default();
    loop {
        let batch = tails.appended(root, &named);
        for (id, event) in batch {
            let printed = match as_json {
                true => json(&id, &event),
                false => line(&id, tails.hooks(&id), &event, tails.widest),
            };
            // A closed pipe (`amx events | head`) ends the stream quietly.
            if writeln!(out, "{printed}").is_err() {
                return Ok(exit::OK);
            }
        }
        if !follow {
            return Ok(exit::OK);
        }
        std::thread::sleep(POLL);
    }
}

/// How far each agent's log has been read.
#[derive(Default)]
struct Tails {
    read: BTreeMap<String, u64>,
    /// Width of the id column: the longest id seen so far. It never shrinks,
    /// so columns do not shift as agents come and go.
    widest: usize,
    /// Each agent's vendor hooks, looked up once: the vendor is fixed at spawn.
    vendors: BTreeMap<String, Option<Hooks>>,
}

impl Tails {
    /// Everything appended since the last look, merged across agents by time.
    fn appended(&mut self, root: &Path, named: &[String]) -> Vec<(String, Event)> {
        let mut batch: Vec<(String, Event)> = Vec::new();
        for id in watching(root, named) {
            self.widest = self.widest.max(id.len());
            let Ok(agent) = Agent::open(root, &id) else {
                continue; // deleted since the last look
            };
            self.vendors.entry(id.clone()).or_insert_with(|| {
                let meta = agent.meta().ok();
                crate::vendor::hooks_for(
                    meta.and_then(|meta| meta.agent)
                        .as_deref()
                        .unwrap_or_default(),
                )
            });
            let read = self.read.entry(id.clone()).or_default();
            let Some((fresh, next)) = grown(&agent.events_path(), *read) else {
                continue; // no log yet, or nothing whole to read
            };
            *read = next;
            batch.extend(
                fresh
                    .lines()
                    .filter_map(|line| serde_json::from_str::<Event>(line).ok())
                    .map(|event| (id.clone(), event)),
            );
        }

        // Timestamps are whole seconds, so ties are common. The sort is stable
        // and agents are read in id order, so ties keep each log's own order.
        batch.sort_by_key(|(_, event)| event.at);
        batch
    }

    fn hooks(&self, id: &str) -> Option<&Hooks> {
        self.vendors.get(id).and_then(Option::as_ref)
    }
}

/// The ids whose logs this look reads: the named ones, or every agent, listed
/// again each look so agents started mid-stream join it.
fn watching(root: &Path, named: &[String]) -> Vec<String> {
    if !named.is_empty() {
        return named.to_vec();
    }
    let mut ids = store::list(root).unwrap_or_default();
    ids.sort();
    ids
}

/// The whole lines `path` grew past offset `read`, and the offset to read from
/// next.
///
/// Stops at the last newline, so a line still being written is picked up by a
/// later call. `None` when there is no file or no new whole line.
pub(crate) fn grown(path: &Path, read: u64) -> Option<(String, u64)> {
    let mut file = std::fs::File::open(path).ok()?;

    // A file shorter than what was read is a new log (the record was swept and
    // recreated under the same id), so read it from the start.
    let read = match file.metadata().ok()?.len() < read {
        true => 0,
        false => read,
    };
    file.seek(SeekFrom::Start(read)).ok()?;

    let mut fresh = String::new();
    file.read_to_string(&mut fresh).ok()?;
    let whole = fresh.rfind('\n')? + 1;
    fresh.truncate(whole);

    Some((fresh, read + whole as u64))
}

/// One event as a JSON line: the logged event with an `id` key added.
///
/// The key set is a contract: keys may be added, never renamed or removed. The
/// payload is passed whole; JSON escaping already keeps control characters
/// out of the terminal.
fn json(id: &str, event: &Event) -> String {
    let mut value = serde_json::to_value(event).expect("an event is plain data");
    if let Some(object) = value.as_object_mut() {
        object.insert("id".to_string(), Value::String(id.to_string()));
    }
    value.to_string()
}

/// One event as a table row: time, id, kind and a short detail, with the kind
/// read in the words of the agent's own vendor `hooks`.
fn line(id: &str, hooks: Option<&Hooks>, event: &Event, widest: usize) -> String {
    format!(
        "{}  {id:<widest$}  {:<KIND$}  {}",
        clock(event.at),
        super::inert_line(&event.kind),
        detail(hooks, event)
    )
    .trim_end()
    .to_string()
}

/// The UTC time of day of an epoch second, as `HH:MM:SSZ`.
///
/// UTC because local time needs a timezone database the binary does not carry.
fn clock(at: u64) -> String {
    let day = at % 86_400;
    format!("{:02}:{:02}:{:02}Z", day / 3600, day % 3600 / 60, day % 60)
}

/// The one payload field worth showing for an event, or nothing.
///
/// amx's own kinds are matched by name; vendor kinds by the moment `hooks`
/// maps them to. A kind with no known field shows nothing.
fn detail(hooks: Option<&Hooks>, event: &Event) -> String {
    let payload = &event.payload;
    let about = match event.kind.as_str() {
        SEND => text(&payload["text"]),
        "answer" => text(&payload["key"]),
        "exit" => match payload["code"].as_i64() {
            Some(code) => format!("code {code}"),
            None => String::new(),
        },
        kind => match hooks.and_then(|hooks| hooks.moment(kind)) {
            Some(Moment::Started) => text(&payload["source"]),
            Some(Moment::Prompted) => text(&payload["prompt"]),
            Some(Moment::Calling) => text(&payload["tool_name"]),
            Some(Moment::Notified) => text(&payload["message"]),
            Some(Moment::Ended) => text(&payload["last_assistant_message"]),
            _ => String::new(),
        },
    };

    // Vendors raise the same events for a subagent's work. Without the label a
    // subagent's `Stop` reads as the agent's turn ending twice.
    match payload["agent_id"].is_null() {
        true => about,
        false => format!("subagent {about}").trim_end().to_string(),
    }
}

/// A payload string field as one sanitized line, or empty.
fn text(value: &Value) -> String {
    value.as_str().map(super::inert_line).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Meta, now};
    use crate::tmux::{PaneId, Socket};
    use serde_json::json;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn record(root: &Path, id: &str) -> Agent {
        Agent::create(
            root,
            &Meta {
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
                pane: PaneId::new("%1").unwrap(),
                bg: false,
                session: None,
                transcript: None,
                created: now(),
            },
        )
        .expect("the record")
    }

    /// Append an event of `kind` at epoch second `at`.
    fn happened(agent: &Agent, at: u64, kind: &str) {
        agent
            .writer()
            .unwrap()
            .append(&Event {
                at,
                kind: kind.to_string(),
                payload: json!({}),
            })
            .unwrap();
    }

    fn merged(batch: &[(String, Event)]) -> Vec<(&str, &str)> {
        batch
            .iter()
            .map(|(id, event)| (id.as_str(), event.kind.as_str()))
            .collect()
    }

    fn shown(kind: &str, payload: Value) -> String {
        line(
            "fix-login-a1b",
            crate::vendor::hooks_for("").as_ref(),
            &Event {
                at: 1,
                kind: kind.to_string(),
                payload,
            },
            "fix-login-a1b".len(),
        )
    }

    #[test]
    fn events_a_line_says_when_whose_and_what() {
        let shown = shown(
            "Stop",
            json!({ "last_assistant_message": "the tests pass now" }),
        );
        assert!(
            shown.starts_with("00:00:01Z  fix-login-a1b  Stop"),
            "{shown}"
        );
        assert!(shown.ends_with("the tests pass now"), "{shown}");
        assert_eq!(shown.lines().count(), 1);
    }

    #[test]
    fn events_the_clock_is_the_time_of_day_it_happened() {
        assert_eq!(clock(0), "00:00:00Z");
        assert_eq!(clock(86_399), "23:59:59Z");
        assert_eq!(clock(1_800_000_061), "08:01:01Z", "and the day is dropped");
    }

    #[test]
    fn events_every_kind_shows_the_one_thing_it_is_about() {
        let claude = crate::vendor::hooks_for("");
        let about = |kind, payload| detail(claude.as_ref(), &Event::new(kind, payload));
        assert_eq!(
            about("SessionStart", json!({ "source": "resume" })),
            "resume"
        );
        assert_eq!(
            about("UserPromptSubmit", json!({ "prompt": "run the tests" })),
            "run the tests"
        );
        assert_eq!(about("PreToolUse", json!({ "tool_name": "Bash" })), "Bash");
        assert_eq!(
            about(
                "Notification",
                json!({ "message": "permission to use Bash" })
            ),
            "permission to use Bash"
        );
        assert_eq!(
            about("Stop", json!({ "last_assistant_message": "done" })),
            "done"
        );
        assert_eq!(
            about(SEND, json!({ "text": "and now the linter" })),
            "and now the linter"
        );
        assert_eq!(about("answer", json!({ "key": "y" })), "y");
        assert_eq!(about("exit", json!({ "code": 2 })), "code 2");
        assert_eq!(
            about("PostCompact", json!({ "trigger": "auto" })),
            "",
            "a payload amx has no phrase for shows nothing rather than all of itself"
        );
    }

    #[test]
    fn events_a_pi_row_shows_the_one_thing_it_is_about() {
        // pi's own event names, read through the vendor the record names.
        let pi = crate::vendor::hooks_for("pi");
        let about = |kind, payload| detail(pi.as_ref(), &Event::new(kind, payload));
        assert_eq!(
            about("tool_execution_start", json!({ "tool_name": "bash" })),
            "bash"
        );
        assert_eq!(
            about(
                "ui_prompt_start",
                json!({ "kind": "confirm", "message": "Trust this folder?" })
            ),
            "Trust this folder?"
        );
        assert_eq!(
            about(
                "agent_settled",
                json!({ "last_assistant_message": "the tests pass" })
            ),
            "the tests pass"
        );
        assert_eq!(
            about("Stop", json!({ "last_assistant_message": "done" })),
            "",
            "claude's word is nothing to a pi"
        );
    }

    #[test]
    fn events_a_subagents_event_says_whose_it_is() {
        let about = detail(
            crate::vendor::hooks_for("").as_ref(),
            &Event::new(
                "Stop",
                json!({ "agent_id": "sub-1", "last_assistant_message": "the linter is clean" }),
            ),
        );
        assert_eq!(about, "subagent the linter is clean");
    }

    #[test]
    fn events_nothing_in_a_payload_can_add_a_line_to_the_stream() {
        // Anything running as this user can write the log.
        let shown = shown(
            "Stop\u{1b}]0;PWNED\u{7}",
            json!({ "last_assistant_message": "done\u{1b}[2Jand more\nfaked  line" }),
        );
        assert_eq!(shown.lines().count(), 1, "{shown:?}");
        assert!(!shown.contains("faked"), "{shown:?}");
        assert_eq!(
            shown.chars().filter(|c| c.is_control()).count(),
            0,
            "{shown:?}"
        );
    }

    #[test]
    fn events_the_json_line_is_the_record_with_whose_it_is_added() {
        // The key set is a contract: keys may be added, never renamed or
        // dropped.
        let printed = json(
            "fix-login-a1b",
            &Event {
                at: 1_800_000_061,
                kind: "Stop".to_string(),
                payload: json!({ "last_assistant_message": "the tests pass now" }),
            },
        );

        let parsed: Value = serde_json::from_str(&printed).expect("a line of JSON");
        let object = parsed.as_object().expect("an object");
        let keys: Vec<&str> = object.keys().map(String::as_str).collect();
        assert_eq!(keys, ["at", "id", "kind", "payload"]);
        assert_eq!(object["id"], "fix-login-a1b");
        assert_eq!(object["at"], 1_800_000_061u64);
        assert_eq!(object["kind"], "Stop");
        assert_eq!(
            object["payload"]["last_assistant_message"],
            "the tests pass now"
        );
    }

    #[test]
    fn events_the_json_line_hands_over_a_payload_whole() {
        // Unlike the table, the payload is not cut down. JSON escaping keeps it
        // on one line and free of raw control characters.
        let printed = json(
            "fix-login-a1b",
            &Event {
                at: 1,
                kind: "Stop".to_string(),
                payload: json!({ "last_assistant_message": "done\u{1b}[2J\nand more" }),
            },
        );

        assert_eq!(printed.lines().count(), 1, "{printed:?}");
        assert_eq!(
            printed.chars().filter(|c| c.is_control()).count(),
            0,
            "{printed:?}"
        );
        let parsed: Value = serde_json::from_str(&printed).expect("a line of JSON");
        assert_eq!(
            parsed["payload"]["last_assistant_message"], "done\u{1b}[2J\nand more",
            "and it reads back as what the vendor said"
        );
    }

    #[test]
    fn events_merges_by_time_and_leaves_each_log_in_its_own_order() {
        let root = TempDir::new().unwrap();
        let one = record(root.path(), "fix-login-a1b");
        let two = record(root.path(), "port-importer-c3d");

        // Two events in the same second keep the log's order.
        happened(&one, 100, "SessionStart");
        happened(&one, 100, "UserPromptSubmit");
        happened(&one, 102, "Stop");
        happened(&two, 101, "SessionStart");

        let batch = Tails::default().appended(root.path(), &[]);
        assert_eq!(
            merged(&batch),
            [
                ("fix-login-a1b", "SessionStart"),
                ("fix-login-a1b", "UserPromptSubmit"),
                ("port-importer-c3d", "SessionStart"),
                ("fix-login-a1b", "Stop"),
            ]
        );
    }

    #[test]
    fn events_draws_every_line_to_the_same_columns() {
        let root = TempDir::new().unwrap();
        let short = record(root.path(), "ask-a1b");
        let long = record(root.path(), "port-importer-c3d");
        happened(&short, 100, "Stop");
        happened(&long, 100, "Stop");

        let mut tails = Tails::default();
        let drawn: Vec<String> = tails
            .appended(root.path(), &[])
            .iter()
            .map(|(id, event)| line(id, tails.hooks(id), event, tails.widest))
            .collect();
        assert_eq!(tails.widest, "port-importer-c3d".len());
        assert_eq!(
            drawn[0].find("Stop"),
            drawn[1].find("Stop"),
            "a short name does not pull the columns in: {drawn:?}"
        );

        // The column keeps its width after the widest agent is gone.
        long.remove().unwrap();
        happened(&short, 101, "exit");
        tails.appended(root.path(), &[]);
        assert_eq!(tails.widest, "port-importer-c3d".len());
    }

    #[test]
    fn events_reads_only_the_agents_it_was_named() {
        let root = TempDir::new().unwrap();
        let one = record(root.path(), "fix-login-a1b");
        let two = record(root.path(), "port-importer-c3d");
        happened(&one, 100, "Stop");
        happened(&two, 100, "Stop");

        let batch = Tails::default().appended(root.path(), &["fix-login-a1b".to_string()]);
        assert_eq!(merged(&batch), [("fix-login-a1b", "Stop")]);
    }

    #[test]
    fn events_a_second_look_brings_only_what_is_new() {
        let root = TempDir::new().unwrap();
        let agent = record(root.path(), "fix-login-a1b");
        happened(&agent, 100, "SessionStart");

        let mut tails = Tails::default();
        assert_eq!(tails.appended(root.path(), &[]).len(), 1);
        assert!(
            tails.appended(root.path(), &[]).is_empty(),
            "nothing has happened since"
        );

        happened(&agent, 101, "Stop");
        assert_eq!(
            merged(&tails.appended(root.path(), &[])),
            [("fix-login-a1b", "Stop")]
        );
    }

    #[test]
    fn events_reads_a_line_only_once_it_is_whole() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        let written = "{\"at\":1,\"kind\":\"Stop\",\"payload\":{}}\n";
        std::fs::write(&path, format!("{written}{{\"at\":2,\"ki")).unwrap();

        let (fresh, next) = grown(&path, 0).expect("the whole line");
        assert_eq!(fresh, written, "the half-written line is left for later");
        assert_eq!(next, written.len() as u64);

        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"nd\":\"exit\",\"payload\":{}}\n")
            .unwrap();

        let (fresh, _) = grown(&path, next).expect("the rest");
        assert_eq!(fresh.lines().count(), 1);
        assert!(fresh.contains("exit"), "{fresh}");
    }

    #[test]
    fn events_starts_a_log_over_when_it_is_not_the_log_it_was_reading() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        let line = "{\"at\":1,\"kind\":\"Stop\",\"payload\":{}}\n";
        std::fs::write(&path, format!("{line}{line}")).unwrap();

        let (_, next) = grown(&path, 0).expect("both lines");
        std::fs::write(&path, line).unwrap();

        let (fresh, read) = grown(&path, next).expect("the log that is there now");
        assert_eq!(fresh, line);
        assert_eq!(read, line.len() as u64);
    }

    #[test]
    fn events_has_nothing_to_read_until_a_log_exists() {
        let dir = TempDir::new().unwrap();
        assert_eq!(grown(&dir.path().join("events.jsonl"), 0), None);
    }
}
