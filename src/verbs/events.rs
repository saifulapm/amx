//! `amx events` — every agent's log, merged into one stream.
//!
//! Each agent appends to its own log, so watching four of them means watching
//! four files. This verb is the merge: one line per event, in the order they
//! happened, each saying whose it is — the one story all of them are part of.
//!
//! `--follow` keeps that merge running by re-reading what each log grew since
//! the last look. No watcher, no daemon and nothing to leak if the person
//! reading walks away, which is the same bargain the rest of amx makes: nothing
//! amx runs stays resident.
//!
//! There are two streams, and which one a reader wants depends on who is
//! reading. The table is drawn for a person: columns, a phrase per event, and
//! nothing in it that can drive their terminal. `--json` is the same merge for
//! a program, one object per line, payloads whole.

use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;

use crate::store::{Agent, Event};
use crate::verbs::send::SEND;
use crate::{exit, paths, store};

/// How often `--follow` looks again. A second is the resolution of the
/// timestamps amx records, so nothing can be reordered by having waited.
const POLL: Duration = Duration::from_secs(1);

/// The width of the kind column: the longest of the vendor's own event names.
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
    // A name that is not an agent's is said now, once. A stream that quietly
    // watches nothing is the worst possible answer to a typed id.
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
                false => line(&id, &event, tails.widest),
            };
            // A reader that walked away — `amx events | head` — ends the
            // stream, and that is nobody's failure to report.
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
    /// The longest name seen so far, which is what the id column is drawn to.
    /// It only ever grows: a stream whose columns moved back and forth as
    /// agents came and went would be harder to read than a ragged one.
    widest: usize,
}

impl Tails {
    /// Everything appended since the last look, merged across the agents.
    fn appended(&mut self, root: &Path, named: &[String]) -> Vec<(String, Event)> {
        let mut batch: Vec<(String, Event)> = Vec::new();
        for id in watching(root, named) {
            self.widest = self.widest.max(id.len());
            let Ok(agent) = Agent::open(root, &id) else {
                continue; // deleted since the last look
            };
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

        // Timestamps are whole seconds, so ties are the rule rather than the
        // exception. The sort is stable and the agents were read in name
        // order, so a tie leaves each log in the order it was written — the
        // only order any of it means anything in.
        batch.sort_by_key(|(_, event)| event.at);
        batch
    }
}

/// Whose logs this look reads.
///
/// With nobody named it is every agent, read again each time: an agent started
/// while the stream is running joins it.
fn watching(root: &Path, named: &[String]) -> Vec<String> {
    if !named.is_empty() {
        return named.to_vec();
    }
    let mut ids = store::list(root).unwrap_or_default();
    ids.sort();
    ids
}

/// Whatever `path` grew past `read`, and where the next look starts.
///
/// The tail stops at the last newline: a line without one is a write still in
/// progress, and the next look finds it whole.
fn grown(path: &Path, read: u64) -> Option<(String, u64)> {
    let mut file = std::fs::File::open(path).ok()?;

    // A log with less in it than has already been read is not the log that was
    // read: the record was swept and an agent of the same name made another.
    // Starting it over beats watching a file that can never grow past itself.
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

/// One event as a program reads it: the line the record holds, with the agent
/// it came from added, because a merged stream nothing is labelled in cannot
/// be read at all.
///
/// This is a contract. A key may be added here; renaming or dropping one
/// breaks every script written against it, so the key set is spelled out in
/// the tests rather than left to be discovered in somebody's `jq` weeks from
/// now.
///
/// The payload goes out whole, which the stream a person reads cannot do. It
/// is one line and there is nothing in it a terminal acts on for a different
/// reason than over there: JSON escapes control characters, so the escaping is
/// the sanitising.
fn json(id: &str, event: &Event) -> String {
    let mut value = serde_json::to_value(event).expect("an event is plain data");
    if let Some(object) = value.as_object_mut() {
        object.insert("id".to_string(), Value::String(id.to_string()));
    }
    value.to_string()
}

/// One event as a person reads it: when, whose, what, and the one thing worth
/// knowing about it.
fn line(id: &str, event: &Event, widest: usize) -> String {
    format!(
        "{}  {id:<widest$}  {:<KIND$}  {}",
        clock(event.at),
        inert(&event.kind),
        detail(event)
    )
    .trim_end()
    .to_string()
}

/// The time of day an event was recorded, as UTC.
///
/// amx records whole epoch seconds, and turning one into the reader's own local
/// time needs a timezone database this binary does not carry. The `Z` says
/// which clock it is rather than leaving a person to work that out.
fn clock(at: u64) -> String {
    let day = at % 86_400;
    format!("{:02}:{:02}:{:02}Z", day / 3600, day % 3600 / 60, day % 60)
}

/// The one thing worth knowing about an event, in a phrase.
///
/// Every kind keeps what it is about somewhere different, and a payload amx has
/// no phrase for shows nothing rather than a page of JSON.
fn detail(event: &Event) -> String {
    let payload = &event.payload;
    let about = match event.kind.as_str() {
        "SessionStart" => text(&payload["source"]),
        "UserPromptSubmit" => text(&payload["prompt"]),
        "PreToolUse" => text(&payload["tool_name"]),
        "Notification" => text(&payload["message"]),
        "Stop" => text(&payload["last_assistant_message"]),
        SEND => text(&payload["text"]),
        "answer" => text(&payload["key"]),
        "exit" => match payload["code"].as_i64() {
            Some(code) => format!("code {code}"),
            None => String::new(),
        },
        _ => String::new(),
    };

    // The vendor raises the same events for a subagent's work, and amx's own
    // law is that those never move the agent's state. A second `Stop` a moment
    // after the first, with nothing beside it, reads as the turn ending twice.
    match payload["agent_id"].is_null() {
        true => about,
        false => format!("subagent {about}").trim_end().to_string(),
    }
}

/// One field of a payload, as a line of a stream can hold it.
fn text(value: &Value) -> String {
    value.as_str().map(inert).unwrap_or_default()
}

/// A string amx did not author, made safe to print: its first line, with
/// nothing in it that can drive the terminal it prints into.
///
/// Both halves matter. The log is a document anything running as this user can
/// write, and an answer with a newline in it would otherwise put a second,
/// unlabelled line into a stream whose whole shape is one line per event.
fn inert(text: &str) -> String {
    crate::tmux::sanitize(text.lines().next().unwrap_or(""))
        .trim()
        .to_string()
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
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                agent: None,
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

    /// One event on an agent's log, at a second of the test's choosing.
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
        let about = |kind, payload| detail(&Event::new(kind, payload));
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
    fn events_a_subagents_event_says_whose_it_is() {
        let about = detail(&Event::new(
            "Stop",
            json!({ "agent_id": "sub-1", "last_assistant_message": "the linter is clean" }),
        ));
        assert_eq!(about, "subagent the linter is clean");
    }

    #[test]
    fn events_nothing_in_a_payload_can_add_a_line_to_the_stream() {
        // The log is a document anything at this uid can write, and every line
        // of this stream is meant to be one line.
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
    fn clibatch_the_json_line_is_the_record_with_whose_it_is_added() {
        // The key set is the contract every script reading this stream is
        // written against: a key may be added, never renamed or dropped.
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
    fn clibatch_the_json_line_hands_over_a_payload_whole() {
        // The stream a person reads takes the first line of a phrase and
        // strips it; this one is read by programs, and a payload cut down is a
        // payload that lied. It is one line and inert for a different reason:
        // JSON escapes the control characters a terminal acts on.
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

        // Two events in the same second: their order is the log's, not a
        // clock's, because the clock cannot tell them apart.
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
            .map(|(id, event)| line(id, event, tails.widest))
            .collect();
        assert_eq!(tails.widest, "port-importer-c3d".len());
        assert_eq!(
            drawn[0].find("Stop"),
            drawn[1].find("Stop"),
            "a short name does not pull the columns in: {drawn:?}"
        );

        // The widest name is gone; the columns stay where the eye left them.
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

        // The rest of it arrives, and the next look finds it whole.
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
