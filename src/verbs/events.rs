//! `amx events`: every agent's event log merged into one stream, one line per
//! event in time order, each labelled with its agent.
//!
//! `--follow` polls each log for what it grew since the last look; there is no
//! watcher or daemon. The default output is a sanitized table for a person;
//! `--json` prints one object per event with the payload whole.

use anyhow::Result;
use serde_json::Value;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;

use crate::store::{Agent, Event};
use crate::vendor::{Hooks, Moment};
use crate::verbs::send::{COMMAND, SEND, UNSENT};
use crate::{exit, paths, store};

/// How often `--follow` polls: the resolution of the recorded timestamps.
const POLL: Duration = Duration::from_secs(1);

/// Width of the kind column: the longest vendor event name.
const KIND: usize = 16;

/// Bytes read per step when a log is read from its end backwards.
const CHUNK: usize = 64 * 1024;

/// Which events to print: the newest `newest` of them, those of the last
/// `since` seconds, both, or (by default) all.
#[derive(Clone, Copy, Default)]
pub struct Reading {
    pub newest: Option<usize>,
    pub since: Option<u64>,
}

/// Run the verb against the machine.
pub fn from_env(ids: &[String], reading: Reading, follow: bool, as_json: bool) -> Result<i32> {
    let root = paths::state_root()?;
    let mut out = std::io::stdout().lock();
    run(&root, ids, reading, follow, as_json, store::now(), &mut out)
}

/// The verb, with the state directory and the clock named.
pub fn run(
    root: &Path,
    ids: &[String],
    reading: Reading,
    follow: bool,
    as_json: bool,
    now: u64,
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
    let everything = reading.newest.is_none() && reading.since.is_none() && !follow;
    if everything {
        let merge = tails.merge(root, &named);
        for (id, event) in merge {
            // A closed pipe (`amx events | head`) ends the stream quietly.
            if !show(&tails, &id, &event, as_json, out) {
                return Ok(exit::OK);
            }
        }
        return Ok(exit::OK);
    }

    let mut batch = tails.latest(root, &named, reading, now);
    loop {
        for (id, event) in batch {
            if !show(&tails, &id, &event, as_json, out) {
                return Ok(exit::OK);
            }
        }
        if !follow {
            return Ok(exit::OK);
        }
        std::thread::sleep(POLL);
        batch = tails.appended(root, &named);
    }
}

/// Print one event. False when the output is closed.
fn show(tails: &Tails, id: &str, event: &Event, as_json: bool, out: &mut impl Write) -> bool {
    let printed = match as_json {
        true => json(id, event),
        false => line(id, tails.hooks(id), event, tails.widest),
    };
    writeln!(out, "{printed}").is_ok()
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
    /// Open agent `id`, noting its column width and vendor on the way. `None`
    /// when it was deleted since it was listed.
    fn enroll(&mut self, root: &Path, id: &str) -> Option<Agent> {
        self.widest = self.widest.max(id.len());
        let agent = Agent::open(root, id).ok()?;
        self.vendors.entry(id.to_string()).or_insert_with(|| {
            let meta = agent.meta().ok();
            crate::vendor::hooks_for(
                meta.and_then(|meta| meta.agent)
                    .as_deref()
                    .unwrap_or_default(),
            )
        });
        Some(agent)
    }

    /// Every event of every log, oldest first, one line at a time.
    fn merge(&mut self, root: &Path, named: &[String]) -> Merge {
        let logs = watching(root, named)
            .into_iter()
            .filter_map(|id| {
                let path = self.enroll(root, &id)?.events_path();
                Some((id, path))
            })
            .collect();
        Merge::open(logs)
    }

    /// The first look. With a selection, the events it picks out, oldest
    /// first; with none, nothing, since following starts at each log's end.
    /// Either way the next `appended` reads on from where this one stopped.
    ///
    /// Each log is read from its end backwards, only as far as the selection
    /// reaches: at most `newest` events, and none older than `since`.
    fn latest(
        &mut self,
        root: &Path,
        named: &[String],
        reading: Reading,
        now: u64,
    ) -> Vec<(String, Event)> {
        let nothing = reading.newest.is_none() && reading.since.is_none();
        let newest = match nothing {
            true => Some(0),
            false => reading.newest,
        };
        let cutoff = reading.since.map_or(0, |span| now.saturating_sub(span));

        let mut batch: Vec<(String, Event)> = Vec::new();
        for id in watching(root, named) {
            let Some(agent) = self.enroll(root, &id) else {
                continue;
            };
            let Some((found, end)) = tail(&agent.events_path(), newest, cutoff, CHUNK) else {
                continue; // no log yet
            };
            self.read.insert(id.clone(), end);
            batch.extend(found.into_iter().map(|event| (id.clone(), event)));
        }

        // The same tie order as the merge: id order, then each log's own.
        batch.sort_by_key(|(_, event)| event.at);
        if let Some(newest) = newest {
            batch.drain(..batch.len().saturating_sub(newest));
        }
        batch
    }

    /// Everything appended since the last look, merged across agents by time.
    fn appended(&mut self, root: &Path, named: &[String]) -> Vec<(String, Event)> {
        let mut batch: Vec<(String, Event)> = Vec::new();
        for id in watching(root, named) {
            let Some(agent) = self.enroll(root, &id) else {
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

/// A log's lines from its end backwards, a `chunk` of bytes at a time.
struct BackLines {
    file: std::fs::File,
    /// Where the bytes not yet read end: everything before it is unread.
    pos: u64,
    /// Bytes read but not yet handed out; empty or ending in a newline.
    buf: Vec<u8>,
    chunk: usize,
}

impl BackLines {
    /// The next line up, without its newline. A line cut by a chunk boundary
    /// is completed from the chunk before it, so lines (and the characters in
    /// them) come out whole.
    fn next_line(&mut self) -> Option<Vec<u8>> {
        loop {
            if let Some(body) = self.buf.len().checked_sub(1) {
                if let Some(at) = self.buf[..body].iter().rposition(|byte| *byte == b'\n') {
                    let line = self.buf[at + 1..body].to_vec();
                    self.buf.truncate(at + 1);
                    return Some(line);
                }
                if self.pos == 0 {
                    let line = self.buf[..body].to_vec();
                    self.buf.clear();
                    return Some(line);
                }
            } else if self.pos == 0 {
                return None;
            }

            let size = (self.chunk as u64).min(self.pos) as usize;
            self.pos -= size as u64;
            let mut more = vec![0; size];
            self.file.seek(SeekFrom::Start(self.pos)).ok()?;
            self.file.read_exact(&mut more).ok()?;
            more.extend_from_slice(&self.buf);
            self.buf = more;
        }
    }
}

/// Where the last whole line of `file` ends: just past its last newline.
fn whole_end(file: &mut std::fs::File, len: u64, chunk: usize) -> Option<u64> {
    let mut pos = len;
    while pos > 0 {
        let size = (chunk as u64).min(pos) as usize;
        pos -= size as u64;
        let mut bytes = vec![0; size];
        file.seek(SeekFrom::Start(pos)).ok()?;
        file.read_exact(&mut bytes).ok()?;
        if let Some(at) = bytes.iter().rposition(|byte| *byte == b'\n') {
            return Some(pos + at as u64 + 1);
        }
    }
    Some(0)
}

/// The newest events of the log at `path` with `at` of `cutoff` or later, at
/// most `newest` of them, oldest first, and the offset where its last whole
/// line ends. `None` when there is no log.
///
/// Reads from the end and stops at the first event too old or once it has
/// enough, so a long log is never read whole.
fn tail(
    path: &Path,
    newest: Option<usize>,
    cutoff: u64,
    chunk: usize,
) -> Option<(Vec<Event>, u64)> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let end = whole_end(&mut file, len, chunk)?;

    let mut back = BackLines {
        file,
        pos: end,
        buf: Vec::new(),
        chunk,
    };
    let mut found = Vec::new();
    while newest.is_none_or(|most| found.len() < most) {
        let Some(line) = back.next_line() else {
            break;
        };
        let Ok(event) = serde_json::from_slice::<Event>(&line) else {
            continue;
        };
        if event.at < cutoff {
            break;
        }
        found.push(event);
    }
    found.reverse();
    Some((found, end))
}

/// Every log's events in time order, read a line at a time: a k-way merge.
///
/// Equal times keep the logs in the order given, and each log's own order.
struct Merge {
    ids: Vec<String>,
    readers: Vec<Option<BufReader<std::fs::File>>>,
    /// The next event of each log, not yet handed out.
    heads: Vec<Option<Event>>,
    /// `(at, log)` of each head, so the smallest is the next event.
    next: BinaryHeap<Reverse<(u64, usize)>>,
}

impl Merge {
    fn open(logs: Vec<(String, std::path::PathBuf)>) -> Self {
        let mut merge = Merge {
            ids: Vec::new(),
            readers: Vec::new(),
            heads: Vec::new(),
            next: BinaryHeap::new(),
        };
        for (id, path) in logs {
            merge.ids.push(id);
            merge
                .readers
                .push(std::fs::File::open(path).ok().map(BufReader::new));
            merge.heads.push(None);
            merge.pull(merge.ids.len() - 1);
        }
        merge
    }

    /// Read log `at` on to its next event, or to its end.
    fn pull(&mut self, at: usize) {
        let Some(reader) = self.readers[at].as_mut() else {
            return;
        };
        let mut bytes = Vec::new();
        loop {
            bytes.clear();
            // A line without its newline is still being written.
            if !matches!(reader.read_until(b'\n', &mut bytes), Ok(read) if read > 0)
                || !bytes.ends_with(b"\n")
            {
                self.readers[at] = None;
                return;
            }
            if let Ok(event) = serde_json::from_slice::<Event>(&bytes) {
                self.next.push(Reverse((event.at, at)));
                self.heads[at] = Some(event);
                return;
            }
        }
    }
}

impl Iterator for Merge {
    type Item = (String, Event);

    fn next(&mut self) -> Option<Self::Item> {
        let Reverse((_, at)) = self.next.pop()?;
        let event = self.heads[at].take()?;
        self.pull(at);
        Some((self.ids[at].clone(), event))
    }
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
        SEND | COMMAND | UNSENT => text(&payload["text"]),
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

    /// What `run` prints as JSON for `reading`, as (id, kind) pairs.
    fn printed(root: &Path, reading: Reading, now: u64) -> Vec<(String, String)> {
        let mut out = Vec::new();
        run(root, &[], reading, false, true, now, &mut out).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|line| {
                let value: Value = serde_json::from_str(line).unwrap();
                (
                    value["id"].as_str().unwrap().to_string(),
                    value["kind"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    fn pairs(wanted: &[(&str, &str)]) -> Vec<(String, String)> {
        wanted
            .iter()
            .map(|(id, kind)| (id.to_string(), kind.to_string()))
            .collect()
    }

    /// Three agents with six events between them, at 100 to 105.
    fn three(root: &Path) {
        let one = record(root, "a-one-a1b");
        let two = record(root, "b-two-c3d");
        let three = record(root, "c-three-e5f");
        happened(&one, 100, "e100");
        happened(&two, 101, "e101");
        happened(&three, 102, "e102");
        happened(&one, 103, "e103");
        happened(&three, 104, "e104");
        happened(&two, 105, "e105");
    }

    #[test]
    fn events_the_newest_n_come_from_across_every_agent_oldest_first() {
        let root = TempDir::new().unwrap();
        three(root.path());

        let reading = Reading {
            newest: Some(5),
            since: None,
        };
        assert_eq!(
            printed(root.path(), reading, 200),
            pairs(&[
                ("b-two-c3d", "e101"),
                ("c-three-e5f", "e102"),
                ("a-one-a1b", "e103"),
                ("c-three-e5f", "e104"),
                ("b-two-c3d", "e105"),
            ])
        );
    }

    #[test]
    fn events_since_keeps_only_the_events_of_that_span() {
        let root = TempDir::new().unwrap();
        three(root.path());

        // At 110 a 7 second span reaches back to 103, and 103 is in it.
        let reading = Reading {
            newest: None,
            since: Some(7),
        };
        assert_eq!(
            printed(root.path(), reading, 110),
            pairs(&[
                ("a-one-a1b", "e103"),
                ("c-three-e5f", "e104"),
                ("b-two-c3d", "e105"),
            ])
        );

        let none = Reading {
            newest: None,
            since: Some(1),
        };
        assert!(printed(root.path(), none, 500).is_empty());
    }

    #[test]
    fn events_a_count_and_a_span_together_give_the_newest_of_the_span() {
        let root = TempDir::new().unwrap();
        three(root.path());

        let reading = Reading {
            newest: Some(2),
            since: Some(7),
        };
        assert_eq!(
            printed(root.path(), reading, 110),
            pairs(&[("c-three-e5f", "e104"), ("b-two-c3d", "e105")])
        );
    }

    #[test]
    fn events_no_flags_prints_every_event_with_ties_in_id_then_log_order() {
        let root = TempDir::new().unwrap();
        let late = record(root.path(), "b-late-c3d");
        let early = record(root.path(), "a-early-a1b");
        happened(&late, 99, "late-0");
        happened(&late, 100, "late-1");
        happened(&late, 100, "late-2");
        happened(&early, 100, "early-1");
        happened(&early, 100, "early-2");
        happened(&early, 101, "early-3");

        assert_eq!(
            printed(root.path(), Reading::default(), 200),
            pairs(&[
                ("b-late-c3d", "late-0"),
                ("a-early-a1b", "early-1"),
                ("a-early-a1b", "early-2"),
                ("b-late-c3d", "late-1"),
                ("b-late-c3d", "late-2"),
                ("a-early-a1b", "early-3"),
            ])
        );
    }

    #[test]
    fn events_follow_alone_starts_at_the_end_of_each_log() {
        let root = TempDir::new().unwrap();
        three(root.path());
        let two = Agent::open(root.path(), "b-two-c3d").unwrap();

        let mut tails = Tails::default();
        assert!(
            tails
                .latest(root.path(), &[], Reading::default(), 200)
                .is_empty(),
            "what is already logged is not replayed"
        );

        happened(&two, 106, "fresh");
        assert_eq!(
            merged(&tails.appended(root.path(), &[])),
            [("b-two-c3d", "fresh")]
        );
    }

    #[test]
    fn events_follow_with_a_selection_prints_it_first_then_only_what_is_new() {
        let root = TempDir::new().unwrap();
        three(root.path());
        let one = Agent::open(root.path(), "a-one-a1b").unwrap();

        let mut tails = Tails::default();
        let reading = Reading {
            newest: Some(2),
            since: None,
        };
        assert_eq!(
            merged(&tails.latest(root.path(), &[], reading, 200)),
            [("c-three-e5f", "e104"), ("b-two-c3d", "e105")]
        );

        happened(&one, 106, "fresh");
        assert_eq!(
            merged(&tails.appended(root.path(), &[])),
            [("a-one-a1b", "fresh")]
        );
    }

    #[test]
    fn events_an_agent_that_starts_mid_stream_is_read_from_its_start() {
        let root = TempDir::new().unwrap();
        three(root.path());

        let mut tails = Tails::default();
        tails.latest(root.path(), &[], Reading::default(), 200);

        let newcomer = record(root.path(), "d-new-0a1");
        happened(&newcomer, 106, "first");
        happened(&newcomer, 107, "second");
        assert_eq!(
            merged(&tails.appended(root.path(), &[])),
            [("d-new-0a1", "first"), ("d-new-0a1", "second")]
        );
    }

    /// A log of `lines`, written to a fresh file.
    fn log_of(lines: &[String]) -> (TempDir, std::fs::File, u64) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        let mut text = lines.join("\n");
        text.push('\n');
        std::fs::write(&path, &text).unwrap();
        (dir, std::fs::File::open(&path).unwrap(), text.len() as u64)
    }

    #[test]
    fn events_backwards_reading_returns_lines_whole_across_chunk_boundaries() {
        // Multi-byte text, and a chunk of 7 bytes that cuts lines and
        // characters in every place.
        let lines: Vec<String> = [
            "plain",
            "caf\u{e9} \u{20ac} \u{1f600}",
            "",
            "x",
            "\u{1f600}\u{1f600}\u{1f600}",
        ]
        .iter()
        .map(|text| text.to_string())
        .collect();
        for chunk in [1, 2, 3, 7, 64] {
            let (_dir, file, len) = log_of(&lines);
            let mut back = BackLines {
                file,
                pos: len,
                buf: Vec::new(),
                chunk,
            };
            let mut got = Vec::new();
            while let Some(line) = back.next_line() {
                got.push(String::from_utf8(line).unwrap());
            }
            got.reverse();
            assert_eq!(got, lines, "chunk {chunk}");
        }
    }

    #[test]
    fn events_backwards_reading_stops_short_of_the_start_when_it_has_enough() {
        let lines: Vec<String> = (0..1_000).map(|n| format!("line number {n:04}")).collect();
        let (_dir, file, len) = log_of(&lines);
        let mut back = BackLines {
            file,
            pos: len,
            buf: Vec::new(),
            chunk: 64,
        };
        assert_eq!(back.next_line().unwrap(), b"line number 0999");
        assert_eq!(back.next_line().unwrap(), b"line number 0998");
        assert!(
            back.pos > len - 256,
            "read {} of {len} bytes",
            len - back.pos
        );
    }

    #[test]
    fn events_a_tail_read_with_small_chunks_keeps_a_multi_byte_payload_whole() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        let event = |at: u64, text: &str| {
            format!("{{\"at\":{at},\"kind\":\"Stop\",\"payload\":{{\"text\":\"{text}\"}}}}\n")
        };
        std::fs::write(
            &path,
            format!(
                "{}{}{}{{\"at\":9,\"ki",
                event(1, "old"),
                event(2, "caf\u{e9} \u{1f600} \u{20ac}"),
                event(3, "\u{1f600}\u{1f600}")
            ),
        )
        .unwrap();

        let (found, end) = tail(&path, Some(2), 0, 5).unwrap();
        let texts: Vec<&str> = found
            .iter()
            .map(|event| event.payload["text"].as_str().unwrap())
            .collect();
        assert_eq!(
            texts,
            ["caf\u{e9} \u{1f600} \u{20ac}", "\u{1f600}\u{1f600}"]
        );
        assert_eq!(
            end,
            (event(1, "old").len()
                + event(2, "caf\u{e9} \u{1f600} \u{20ac}").len()
                + event(3, "\u{1f600}\u{1f600}").len()) as u64,
            "a half-written last line is left for later"
        );
    }
}
