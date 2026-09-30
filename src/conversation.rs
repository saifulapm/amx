//! Vendor transcripts, read into what was said.
//!
//! The view's card and `amx logs` both read conversations through this module,
//! so they agree on what each line means. Each vendor writes its own JSONL
//! shape (see [`Transcript`]); this module keeps prompts, assistant text and
//! tool calls, and skips thinking, tool results and bookkeeping.
//!
//! - A pi session is a tree of `id`/`parentId` entries and is read along the
//!   branch its last entry is on, as pi shows it on reload (see [`branch`]).
//! - A line that is not JSON is skipped, since transcripts are read while the
//!   vendor appends to them.

use serde_json::Value;
use std::borrow::Borrow;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::vendor::Transcript;

/// One thing said in a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Said {
    /// What the person typed at the composer.
    Prompt(String),
    /// One block of what the agent said, verbatim.
    Text(String),
    /// A tool call: its name and the one argument worth showing.
    Tool {
        name: String,
        detail: Option<String>,
    },
}

/// The transcript format for an agent command.
///
/// Resolved like `crate::rules::of`: the command's vendor, or the first table
/// entry for a command amx does not know (a wrapper script, say). `None` for a
/// vendor that keeps no transcript.
pub fn format_of(agent: &str) -> Option<Transcript> {
    crate::registry::read_as(agent).and_then(|vendor| vendor.transcript)
}

/// Everything said in the conversation, in order.
pub fn read(format: Transcript, jsonl: &str) -> Vec<Said> {
    let mut said = Vec::new();
    for entry in spoken(format, jsonl) {
        said_by(format, &entry, &mut said);
    }
    said
}

/// Push what one entry said onto `said`.
fn said_by(format: Transcript, entry: &Value, said: &mut Vec<Said>) {
    match format {
        Transcript::Claude => claude(entry, said),
        Transcript::Pi => pi(entry, said),
        Transcript::Codex => codex(entry, said),
        Transcript::Opencode => opencode(entry, said),
    }
}

/// The entries a reading walks, oldest first: every line, or for pi the
/// branch its last entry is on.
///
/// Lines are parsed lazily so a large transcript is never held as one tree of
/// values; reversed, only the end is parsed. A pi branch needs the whole file.
fn spoken(format: Transcript, jsonl: &str) -> Box<dyn DoubleEndedIterator<Item = Value> + '_> {
    match format {
        Transcript::Claude | Transcript::Codex | Transcript::Opencode => Box::new(entries(jsonl)),
        Transcript::Pi => Box::new(branch(entries(jsonl).collect()).into_iter()),
    }
}

/// The branch of a pi session: from the last entry up through `parentId` to a
/// root, returned root first.
///
/// This matches pi's own reader (`buildSessionPath` in session-manager.js,
/// 0.84.4 and 0.85.1), which takes the file's last entry as the leaf. The file
/// does not record branching: after navigating back and continuing, the next
/// appended entry is the first on the new branch.
///
/// The header has no `id` and is on no path. A parent missing from the file
/// ends the walk, and so does a walk longer than the file (an edited-in
/// cycle).
fn branch(entries: Vec<Value>) -> Vec<Value> {
    let index: HashMap<&str, usize> = entries
        .iter()
        .enumerate()
        .filter_map(|(at, entry)| entry["id"].as_str().map(|id| (id, at)))
        .collect();
    let mut path = Vec::new();
    let mut at = entries.iter().rposition(|entry| entry["id"].is_string());
    while let Some(here) = at
        && path.len() <= entries.len()
    {
        path.push(here);
        at = entries[here]["parentId"]
            .as_str()
            .and_then(|parent| index.get(parent).copied());
    }
    path.reverse();
    let mut entries: Vec<Option<Value>> = entries.into_iter().map(Some).collect();
    path.into_iter()
        .filter_map(|at| entries[at].take())
        .collect()
}

/// The answer at the end of the conversation, if the turn has ended.
///
/// A transcript ending on a tool result is a turn still running whose last
/// assistant text belongs to the previous turn, so this answers `None`.
/// Bookkeeping lines and synthetic entries (see [`synthetic`]) are skipped.
/// codex answers from its turn-end event (see [`codex_answer`]).
pub fn answer(format: Transcript, jsonl: &str) -> Option<String> {
    last_answer(format, spoken(format, jsonl).rev())
}

/// The answer at the end of a walk given newest entry first. Stops at the
/// first entry that settles it, so a lazy walk parses only the end.
fn last_answer<V: Borrow<Value>>(
    format: Transcript,
    mut newest: impl Iterator<Item = V>,
) -> Option<String> {
    match format {
        Transcript::Codex => return codex_answer(newest),
        Transcript::Opencode => return opencode_answer(newest),
        _ => {}
    }
    let last = newest.find(|entry| voice(format, entry.borrow()).is_some())?;
    let last = last.borrow();
    if voice(format, last) != Some(Voice::Assistant) || synthetic(format, last) || cut_off(last) {
        return None;
    }
    answer_text(last)
}

/// Whether an assistant entry stopped short of an answer: `aborted` (cut
/// short) or `error` (the provider failed).
fn cut_off(entry: &Value) -> bool {
    matches!(
        entry["message"]["stopReason"].as_str(),
        Some("aborted" | "error")
    )
}

/// The words of every synthetic entry (see [`synthetic`]).
///
/// claude hands these to its turn-end hook as if the agent said them; this is
/// how the hook tells them apart.
pub fn synthetic_words(format: Transcript, jsonl: &str) -> Vec<String> {
    spoken(format, jsonl)
        .filter(|entry| synthetic(format, entry))
        .filter_map(|entry| answer_text(&entry))
        .collect()
}

/// Why the last turn ended without an answer, where the vendor recorded a
/// reason worth repeating.
///
/// The reason decides what a caller does next: retry shorter after a token
/// limit, retry after a provider error, or stop after an abort. Only the
/// transcript records it. When the account stops a claude turn (a usage
/// limit, no credits), claude writes a synthetic API-error entry instead, and
/// its text is the reason.
///
/// Ordinary endings (`stop`, `toolUse`, `end_turn`, `tool_use`) answer `None`.
/// An unknown reason is repeated as the vendor spelled it.
pub fn why_it_stopped(format: Transcript, jsonl: &str) -> Option<String> {
    let mut newest = spoken(format, jsonl).rev();
    match format {
        Transcript::Codex => return codex_why(newest),
        Transcript::Opencode => return opencode_why(newest),
        _ => {}
    }
    let last = newest.find(|entry| voice(format, entry).is_some())?;
    let last = &last;
    if voice(format, last) != Some(Voice::Assistant) {
        return None;
    }
    if synthetic(format, last) && last["isApiErrorMessage"] == true {
        return answer_text(last).map(|said| format!("the vendor said: {}", one_line(&said)));
    }
    let message = &last["message"];
    let reason = message["stopReason"]
        .as_str()
        .or_else(|| message["stop_reason"].as_str())?;
    match reason {
        "stop" | "toolUse" | "end_turn" | "tool_use" => None,
        "length" | "max_tokens" => Some("it stopped at the model's token limit".to_string()),
        "aborted" => Some("the turn was aborted".to_string()),
        "error" => Some(match message["errorMessage"].as_str() {
            Some(said) => format!("the provider failed: {}", one_line(said)),
            None => "the provider failed".to_string(),
        }),
        other => Some(format!("the vendor stopped on `{other}`")),
    }
}

/// The first line of a vendor's error, capped at 160 characters.
fn one_line(said: &str) -> String {
    let line = said.lines().next().unwrap_or_default().trim();
    match line.char_indices().nth(160) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_string(),
    }
}

/// An assistant entry's text blocks joined, or `None` if it has no text.
fn answer_text(entry: &Value) -> Option<String> {
    trimmed(&text_of(&entry["message"]["content"]))
}

/// `text` trimmed, or `None` if nothing is left.
fn trimmed(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Whether an entry is claude's own note about a turn that never reached the
/// model.
///
/// claude marks these `"model":"<synthetic>"`: "No response requested.", API
/// errors, usage limits.
fn synthetic(format: Transcript, entry: &Value) -> bool {
    matches!(format, Transcript::Claude) && entry["message"]["model"] == "<synthetic>"
}

/// The newest thing said, as a single row.
///
/// Unlike [`answer`], this does not wait for the turn to end: a call whose
/// result is pending is what the agent is doing now. A tool call reads as its
/// name and main argument (`Bash cargo test --all`), text as its first line.
/// A prompt answers `None`, since the reader typed it.
pub fn latest(format: Transcript, jsonl: &str) -> Option<String> {
    let newest = spoken(format, jsonl).rev().find_map(|entry| {
        let mut said = Vec::new();
        said_by(format, &entry, &mut said);
        said.pop()
    })?;
    match newest {
        Said::Prompt(_) => None,
        Said::Text(words) => words.lines().next().map(str::to_string),
        Said::Tool { name, detail } => Some(match detail {
            Some(detail) => format!("{name} {detail}"),
            None => name,
        }),
    }
}

/// Input tokens as of the last assistant entry that sent any: what the next
/// turn would send.
///
/// Vendors report usage per message, so the last entry that has it covers the
/// whole conversation. claude sums `input_tokens`,
/// `cache_creation_input_tokens` and `cache_read_input_tokens`; pi sums
/// `input`, `cacheRead` and `cacheWrite`. Missing fields count as 0.
///
/// Entries summing to 0 are skipped: claude's synthetic entries carry all-zero
/// usage and are written last, and a real turn never sends 0 tokens.
///
/// codex reports usage in `token_count` events; `last_token_usage.input_tokens`
/// is the last request's whole input, cached part included.
fn context_of<V: Borrow<Value>>(
    format: Transcript,
    newest: impl Iterator<Item = V>,
) -> Option<u64> {
    newest
        .filter(|entry| match format {
            Transcript::Codex => codex_event(entry.borrow()) == Some("token_count"),
            _ => voice(format, entry.borrow()) == Some(Voice::Assistant),
        })
        .map(|entry| usage_sum(format, entry.borrow()))
        .find(|total| *total > 0)
}

/// The input tokens of one entry's usage, missing fields as 0.
fn usage_sum(format: Transcript, entry: &Value) -> u64 {
    let usage = &entry["message"]["usage"];
    let field = |key: &str| usage[key].as_u64().unwrap_or(0);
    match format {
        Transcript::Claude => {
            field("input_tokens")
                + field("cache_creation_input_tokens")
                + field("cache_read_input_tokens")
        }
        Transcript::Pi => field("input") + field("cacheRead") + field("cacheWrite"),
        Transcript::Codex => entry["payload"]["info"]["last_token_usage"]["input_tokens"]
            .as_u64()
            .unwrap_or(0),
        Transcript::Opencode => {
            let tokens = &entry["tokens"];
            [
                &tokens["input"],
                &tokens["cache"]["read"],
                &tokens["cache"]["write"],
            ]
            .iter()
            .map(|count| count.as_u64().unwrap_or(0))
            .sum()
        }
    }
}

/// The context size and the last answer, both read off the end.
///
/// `View::json()` asks both of the same tail on every poll. Each walk parses
/// from the end only as far as it needs; a pi branch needs the whole tail
/// parsed, so it is parsed once for both.
pub fn context_and_last_words(format: Transcript, jsonl: &str) -> (Option<u64>, Option<String>) {
    match format {
        // Find the pi branch once for both walks.
        Transcript::Pi => {
            let entries: Vec<Value> = spoken(format, jsonl).collect();
            (
                context_of(format, entries.iter().rev()),
                last_answer(format, entries.iter().rev()),
            )
        }
        _ => (
            context_of(format, spoken(format, jsonl).rev()),
            last_answer(format, spoken(format, jsonl).rev()),
        ),
    }
}

/// The session's title, if it has one.
///
/// claude rewrites the title line whenever the title changes, so the last one
/// wins. `aiTitle` is claude's own, `customTitle` one a person typed; claude
/// 2.1.263 stops writing `aiTitle` once a session is renamed, so the last of
/// either answers. A blank title is no title. Other vendors keep no title.
pub fn session_title(format: Transcript, jsonl: &str) -> Option<String> {
    match format {
        Transcript::Pi | Transcript::Codex | Transcript::Opencode => None,
        // Called by the hook on every event; title lines are rare, so skip
        // parsing lines that cannot be one.
        Transcript::Claude => jsonl
            .lines()
            .filter(|line| line.contains("-title"))
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|entry| {
                let title = match entry["type"].as_str()? {
                    "custom-title" => entry["customTitle"].as_str()?,
                    "ai-title" => entry["aiTitle"].as_str()?,
                    _ => return None,
                }
                .trim();
                (!title.is_empty()).then(|| title.to_string())
            })
            .next_back(),
    }
}

/// The conversation as plain text for a terminal or pipe.
///
/// Prompts are prefixed `❯`, tool calls `›`, and agent text is bare. Items are
/// separated by a blank line, except consecutive tool calls, which form one
/// block as on the card.
pub fn plain(said: &[Said]) -> String {
    let mut out = String::new();
    let mut after_call = false;
    for one in said {
        let call = matches!(one, Said::Tool { .. });
        if !out.is_empty() {
            out.push_str(if after_call && call { "\n" } else { "\n\n" });
        }
        out.push_str(&match one {
            Said::Prompt(text) => format!("❯ {text}"),
            Said::Text(text) => text.clone(),
            Said::Tool { name, detail } => match detail {
                Some(detail) => format!("› {name} {detail}"),
                None => format!("› {name}"),
            },
        });
        after_call = call;
    }
    out
}

/// Which voice an entry is in, for formats that have one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Voice {
    User,
    Assistant,
    /// A tool result, which vendors write as a message of its own.
    Result,
}

/// The voice of one entry, or `None` for bookkeeping.
fn voice(format: Transcript, entry: &Value) -> Option<Voice> {
    match format {
        Transcript::Claude => match entry["type"].as_str()? {
            // A tool result is a `user` line with a `tool_result` block. A
            // prompt is a string, or blocks when an image was pasted or the
            // turn was interrupted.
            "user" => Some(
                match blocks(entry)
                    .iter()
                    .any(|block| block["type"] == "tool_result")
                {
                    true => Voice::Result,
                    false => Voice::User,
                },
            ),
            "assistant" => Some(Voice::Assistant),
            _ => None,
        },
        Transcript::Pi => {
            if entry["type"] != "message" {
                return None;
            }
            match entry["message"]["role"].as_str()? {
                "user" => Some(Voice::User),
                "assistant" => Some(Voice::Assistant),
                "toolResult" => Some(Voice::Result),
                _ => None,
            }
        }
        Transcript::Opencode => match entry["type"].as_str()? {
            "user" => Some(Voice::User),
            "assistant" => Some(Voice::Assistant),
            _ => None,
        },
        Transcript::Codex => None,
    }
}

/// claude's queue lines, and the removal reason for a queued message it folds
/// into the running turn.
///
/// claude writes an `enqueue` line for a message typed mid-turn and a `remove`
/// line when it takes it. For this reason alone it writes no `user` entry, so
/// the removal is the only record of the prompt. It sits where claude's pane
/// draws the message: after the tool call it arrived during, before the answer
/// it changed. Seen in claude 2.1.278.
const QUEUED: &str = "queue-operation";
const ABSORBED: &str = "absorbed_mid_turn";

/// Every message claude took off its queue, with the second it was taken.
/// Other formats have no queue and answer empty.
pub fn unqueued(format: Transcript, jsonl: &str) -> Vec<(u64, String)> {
    if format != Transcript::Claude {
        return Vec::new();
    }
    jsonl
        .lines()
        .filter(|line| line.contains(QUEUED))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| entry["type"] == QUEUED && entry["operation"] == "remove")
        .filter_map(|entry| {
            let at = epoch(entry["timestamp"].as_str()?)?;
            Some((at, entry["content"].as_str()?.to_string()))
        })
        .collect()
}

/// [`unqueued`] for the transcript at `path`, reading back from its end only
/// as far as the first whole line stamped before `since`.
///
/// A queued message is taken after it was sent, so nothing older matters, and
/// claude transcripts run to tens of megabytes.
pub fn unqueued_since(format: Transcript, path: &Path, since: u64) -> Vec<(u64, String)> {
    if format != Transcript::Claude {
        return Vec::new();
    }
    since_stamp(path, since)
        .map(|jsonl| unqueued(format, &jsonl))
        .unwrap_or_default()
}

/// The end of the file at `path`, read back in chunks until its first whole
/// line carries a timestamp before `since`, or the whole file.
fn since_stamp(path: &Path, since: u64) -> Option<String> {
    const CHUNK: u64 = 64 * 1024;
    let mut file = File::open(path).ok()?;
    let mut start = file.metadata().ok()?.len();
    let mut bytes = Vec::new();
    while start > 0 {
        let from = start.saturating_sub(CHUNK);
        let mut chunk = vec![0; (start - from) as usize];
        file.seek(SeekFrom::Start(from)).ok()?;
        file.read_exact(&mut chunk).ok()?;
        chunk.extend_from_slice(&bytes);
        bytes = chunk;
        start = from;
        // The first line is cut unless the read reached the start.
        let whole = match start {
            0 => &bytes[..],
            _ => bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(&[][..], |at| &bytes[at + 1..]),
        };
        let earliest = whole
            .split(|byte| *byte == b'\n')
            .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
            .find_map(|entry| epoch(entry["timestamp"].as_str()?));
        if earliest.is_some_and(|at| at < since) {
            break;
        }
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Epoch seconds for a UTC stamp like `2026-09-29T20:32:04.768Z`.
fn epoch(stamp: &str) -> Option<u64> {
    let (date, time) = stamp.split_once('T')?;
    let mut date = date.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let mut time = time.trim_end_matches('Z').splitn(3, ':');
    let hours: i64 = time.next()?.parse().ok()?;
    let minutes: i64 = time.next()?.parse().ok()?;
    let seconds: f64 = time.next()?.parse().ok()?;
    // Days since 1970-01-01, per Howard Hinnant's `days_from_civil`.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    u64::try_from(days * 86_400 + hours * 3_600 + minutes * 60 + seconds as i64).ok()
}

/// The words of a queued message claude folded into the running turn.
fn absorbed(entry: &Value) -> Option<&str> {
    let taken =
        entry["type"] == QUEUED && entry["operation"] == "remove" && entry["reason"] == ABSORBED;
    taken.then(|| entry["content"].as_str()).flatten()
}

/// What one claude entry said.
fn claude(entry: &Value, said: &mut Vec<Said>) {
    if let Some(queued) = absorbed(entry) {
        prompt(Some(queued), said);
        return;
    }
    match voice(Transcript::Claude, entry) {
        // claude's own lines in the user's voice: a skill's body, an image's
        // source, the local-command caveat, a compaction summary.
        Some(Voice::User) if entry["isMeta"] == true || entry["isCompactSummary"] == true => {}
        Some(Voice::User) => {
            let typed = match entry["message"]["content"].as_str() {
                Some(typed) => typed.to_string(),
                None => text_of(&entry["message"]["content"]),
            };
            prompt(Some(&command(&typed).unwrap_or(typed)), said);
        }
        Some(Voice::Assistant) => {
            for block in blocks(entry) {
                match block["type"].as_str() {
                    Some("text") => text(block["text"].as_str(), said),
                    Some("tool_use") => tool(block["name"].as_str(), &block["input"], said),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// What one pi entry said.
fn pi(entry: &Value, said: &mut Vec<Said>) {
    match voice(Transcript::Pi, entry) {
        Some(Voice::User) => {
            // A string, or blocks when an image was attached.
            let content = &entry["message"]["content"];
            match content.as_str() {
                Some(typed) => prompt(Some(typed), said),
                None => prompt(Some(&text_of(&entry["message"]["content"])), said),
            }
        }
        Some(Voice::Assistant) => {
            for block in blocks(entry) {
                match block["type"].as_str() {
                    Some("text") => text(block["text"].as_str(), said),
                    Some("toolCall") => tool(block["name"].as_str(), &block["arguments"], said),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// What one codex rollout line said.
///
/// Prompts come from codex's record of what the person sent, never from the
/// `user` messages sent to the model, which also carry injected context
/// (`<environment_context>`, AGENTS.md). That record is an `item_completed`
/// event with a `UserMessage` item on threads the TUI starts, and a
/// `user_message` event on legacy ones; steered messages appear the same way
/// (codex 0.157.1).
///
/// Assistant text is the `output_text` of assistant messages. A call is a
/// `function_call` with JSON arguments or a `custom_tool_call` with free-form
/// input (a script, for `exec`).
fn codex(entry: &Value, said: &mut Vec<Said>) {
    let payload = &entry["payload"];
    match codex_event(entry) {
        Some("item_completed") if payload["item"]["type"] == "UserMessage" => {
            prompt(Some(&text_of(&payload["item"]["content"])), said);
        }
        Some("user_message") => prompt(payload["message"].as_str(), said),
        _ if entry["type"] != "response_item" => {}
        _ => match payload["type"].as_str() {
            Some("message") if payload["role"] == "assistant" => {
                for block in payload["content"].as_array().into_iter().flatten() {
                    if block["type"] == "output_text" {
                        text(block["text"].as_str(), said);
                    }
                }
            }
            Some("function_call" | "custom_tool_call") => {
                let input = payload["arguments"]
                    .as_str()
                    .or_else(|| payload["input"].as_str())
                    .and_then(|input| serde_json::from_str(input).ok())
                    .unwrap_or(Value::Null);
                tool(payload["name"].as_str(), &input, said);
            }
            _ => {}
        },
    }
}

/// The type of a codex `event_msg` line, or `None` for other lines.
fn codex_event(entry: &Value) -> Option<&str> {
    match entry["type"] == "event_msg" {
        true => entry["payload"]["type"].as_str(),
        false => None,
    }
}

/// The last turn edge in a rollout: `task_started`, `task_complete` or
/// `turn_aborted`.
///
/// A turn whose last edge is `task_started` is still running. A pane killed
/// mid-turn leaves it that way for good; codex never closes it, even on resume
/// (docs/codex-screens.md).
fn codex_turn_end<V: Borrow<Value>>(mut newest: impl Iterator<Item = V>) -> Option<V> {
    newest.find(|entry| {
        matches!(
            codex_event(entry.borrow()),
            Some("task_started" | "task_complete" | "turn_aborted")
        )
    })
}

/// A codex turn's answer: `last_agent_message` from its `task_complete`.
///
/// An Esc'd turn has no `task_complete` even if it wrote a final answer, a
/// turn that ended on a question has a null message, and a failed one carries
/// an `error`.
fn codex_answer<V: Borrow<Value>>(newest: impl Iterator<Item = V>) -> Option<String> {
    let end = codex_turn_end(newest)?;
    let end = &end.borrow()["payload"];
    if end["type"] != "task_complete" || !end["error"].is_null() {
        return None;
    }
    trimmed(end["last_agent_message"].as_str()?)
}

/// Why a codex turn ended with nothing: aborted (Esc is `interrupted`), or
/// failed with the provider's message.
///
/// When the error message is the provider's JSON, its `error.message` is the
/// sentence and the rest is wrapping.
fn codex_why<V: Borrow<Value>>(newest: impl Iterator<Item = V>) -> Option<String> {
    let end = codex_turn_end(newest)?;
    let end = &end.borrow()["payload"];
    match end["type"].as_str()? {
        "turn_aborted" => Some(match end["reason"].as_str() {
            Some("interrupted") | None => "the turn was aborted".to_string(),
            Some(other) => format!("the vendor stopped on `{other}`"),
        }),
        "task_complete" if !end["error"].is_null() => {
            let written = end["error"]["message"].as_str();
            let said = written
                .and_then(|written| serde_json::from_str::<Value>(written).ok())
                .and_then(|json| json["error"]["message"].as_str().map(str::to_string))
                .or(written.map(str::to_string));
            Some(match said {
                Some(said) => format!("the provider failed: {}", one_line(&said)),
                None => "the provider failed".to_string(),
            })
        }
        _ => None,
    }
}

/// What one opencode message said.
///
/// A prompt is a `user` message's `text`. Assistant text and calls are the
/// `text` and `tool` items of an `assistant` message's `content`, with call
/// arguments under `state.input`. Reasoning items, `idle` rows and synthetic,
/// system and compaction messages are skipped.
fn opencode(entry: &Value, said: &mut Vec<Said>) {
    match voice(Transcript::Opencode, entry) {
        Some(Voice::User) => prompt(entry["text"].as_str(), said),
        Some(Voice::Assistant) => {
            for item in entry["content"].as_array().into_iter().flatten() {
                match item["type"].as_str() {
                    Some("text") => text(item["text"].as_str(), said),
                    Some("tool") => tool(item["name"].as_str(), &item["state"]["input"], said),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// How the last opencode turn ended, and its last assistant step.
///
/// The `idle` row closing a turn carries the outcome: `succeeded`, `failed`
/// or `interrupted`. A turn ended by a rejected permission or a dismissed
/// question writes no `idle` row; its last step has an `aborted` error and
/// reads as interrupted (docs/opencode-screens.md, "The message lists"). A
/// list ending on a prompt, or on a step without an error, is still running.
fn opencode_end<V: Borrow<Value>>(
    mut newest: impl Iterator<Item = V>,
) -> Option<(String, Option<V>)> {
    let last = newest.find(|entry| {
        matches!(
            entry.borrow()["type"].as_str(),
            Some("user" | "assistant" | "idle")
        )
    })?;
    let ended = last.borrow();
    let outcome = match ended["type"].as_str()? {
        "idle" => ended["outcome"].as_str()?,
        "assistant" if ended["error"]["type"] == "aborted" => "interrupted",
        "assistant" if ended["error"].is_object() => "failed",
        _ => return None,
    }
    .to_string();
    let step = std::iter::once(last)
        .chain(newest)
        .take_while(|entry| entry.borrow()["type"] != "user")
        .find(|entry| entry.borrow()["type"] == "assistant");
    Some((outcome, step))
}

/// An opencode turn's answer: its last step's text, if the turn succeeded.
fn opencode_answer<V: Borrow<Value>>(newest: impl Iterator<Item = V>) -> Option<String> {
    match opencode_end(newest)? {
        (outcome, Some(step)) if outcome == "succeeded" => {
            trimmed(&text_of(&step.borrow()["content"]))
        }
        _ => None,
    }
}

/// Why an opencode turn ended with nothing: interrupted, or failed with its
/// last step's error message.
fn opencode_why<V: Borrow<Value>>(newest: impl Iterator<Item = V>) -> Option<String> {
    let (outcome, step) = opencode_end(newest)?;
    let step = step.as_ref().map(Borrow::borrow);
    match outcome.as_str() {
        "succeeded" => None,
        "interrupted" => Some("the turn was aborted".to_string()),
        "failed" => Some(
            match step.and_then(|step| step["error"]["message"].as_str()) {
                Some(said) => format!("the provider failed: {}", one_line(said)),
                None => "the provider failed".to_string(),
            },
        ),
        other => Some(format!("the vendor stopped on `{other}`")),
    }
}

/// The `text` blocks of a content array joined by newlines. Other blocks
/// (images, thinking) are skipped.
fn text_of(content: &Value) -> String {
    let text: Vec<&str> = content
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect();
    text.join("\n")
}

/// A slash command or skill as typed, from claude's tags: `<command-name>`
/// followed by any `<command-args>`.
fn command(written: &str) -> Option<String> {
    let name = tagged(written, "command-name")?.trim();
    let args = tagged(written, "command-args").unwrap_or_default().trim();
    Some(match args.is_empty() {
        true => name.to_string(),
        false => format!("{name} {args}"),
    })
}

/// The text between `<tag>` and `</tag>`.
fn tagged<'a>(written: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let from = written.find(&open)? + open.len();
    let to = written[from..].find(&format!("</{tag}>"))?;
    Some(&written[from..from + to])
}

/// A message's content blocks, or none if the content is not an array.
fn blocks(entry: &Value) -> &[Value] {
    entry["message"]["content"]
        .as_array()
        .map_or(&[], Vec::as_slice)
}

fn prompt(typed: Option<&str>, said: &mut Vec<Said>) {
    if let Some(typed) = typed.map(str::trim).filter(|typed| !typed.is_empty()) {
        said.push(Said::Prompt(typed.to_string()));
    }
}

fn text(words: Option<&str>, said: &mut Vec<Said>) {
    if let Some(words) = words.map(str::trim).filter(|words| !words.is_empty()) {
        said.push(Said::Text(words.to_string()));
    }
}

fn tool(name: Option<&str>, input: &Value, said: &mut Vec<Said>) {
    if let Some(name) = name.filter(|name| !name.is_empty()) {
        said.push(Said::Tool {
            name: name.to_string(),
            detail: detail(input),
        });
    }
}

/// The one tool argument worth showing beside its name, first line only:
/// a shell command, a file path, a search pattern and so on.
///
/// Keys are tried in order of usefulness; a call with none of them shows its
/// name alone.
fn detail(input: &Value) -> Option<String> {
    const WORTH_A_ROW: [&str; 9] = [
        "command",
        "file_path",
        "path",
        "pattern",
        "url",
        "query",
        "skill",
        "description",
        "prompt",
    ];
    WORTH_A_ROW
        .iter()
        .find_map(|key| input[key].as_str())
        .and_then(|value| value.lines().next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Every line that parses as JSON.
fn entries(jsonl: &str) -> impl DoubleEndedIterator<Item = Value> + '_ {
    jsonl
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shapes from a live claude 2.1.240 transcript.
    const CLAUDE: &str = concat!(
        "{\"type\":\"mode\",\"x\":1}\n",
        "{\"type\":\"user\",\"message\":{\"content\":\"print the numbers\"}}\n",
        "{\"type\":\"attachment\"}\n",
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"thinking\",\"thinking\":\"hm\"}]}}\n",
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Bash\",\"input\":{\"command\":\"seq 3\\n# and more\"}}]}}\n",
        "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"content\":\"1\\n2\\n3\"}]}}\n",
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"1\\n2\\n3\"}]}}\n",
    );

    /// Shapes from a live pi 0.84.4 session (0.85.1 writes the same version
    /// 3): the header, two bookkeeping entries, and messages in all three
    /// voices.
    const PI: &str = concat!(
        "{\"type\":\"session\",\"version\":3,\"id\":\"hi-c4g\",\"cwd\":\"/srv/app\"}\n",
        "{\"type\":\"model_change\",\"id\":\"0a\",\"parentId\":null,\"provider\":\"opencode\",\"modelId\":\"m\"}\n",
        "{\"type\":\"thinking_level_change\",\"id\":\"18\",\"parentId\":\"0a\",\"thinkingLevel\":\"high\"}\n",
        "{\"type\":\"message\",\"id\":\"0b\",\"parentId\":\"18\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Hi\"}],\"timestamp\":1}}\n",
        "{\"type\":\"message\",\"id\":\"a8\",\"parentId\":\"0b\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"thinking\",\"thinking\":\"\"},{\"type\":\"toolCall\",\"id\":\"c1\",\"name\":\"bash\",\"arguments\":{\"command\":\"pwd\"}}],\"stopReason\":\"toolUse\"}}\n",
        "{\"type\":\"message\",\"id\":\"55\",\"parentId\":\"a8\",\"message\":{\"role\":\"toolResult\",\"toolCallId\":\"c1\",\"toolName\":\"bash\",\"content\":[{\"type\":\"text\",\"text\":\"/srv/app\"}],\"isError\":false}}\n",
        "{\"type\":\"message\",\"id\":\"c9\",\"parentId\":\"55\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Hello. You are in /srv/app.\"}],\"stopReason\":\"stop\"}}\n",
    );

    fn tool(name: &str, detail: Option<&str>) -> Said {
        Said::Tool {
            name: name.to_string(),
            detail: detail.map(str::to_string),
        }
    }

    #[test]
    fn a_skill_call_names_the_skill() {
        let jsonl = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Skill\",\"input\":{\"skill\":\"desktop\",\"args\":\"screenshot the bar\"}}]}}\n";
        assert_eq!(
            read(Transcript::Claude, jsonl),
            vec![tool("Skill", Some("desktop"))]
        );
    }

    #[test]
    fn conversation_reads_a_claude_transcript_in_order() {
        assert_eq!(
            read(Transcript::Claude, CLAUDE),
            vec![
                Said::Prompt("print the numbers".to_string()),
                tool("Bash", Some("seq 3")),
                Said::Text("1\n2\n3".to_string()),
            ],
            "a prompt, the call with the first line of its command, the words; \
             thinking, the tool's result and the bookkeeping are nobody's reading"
        );
    }

    #[test]
    fn a_taken_message_is_found_however_far_back_the_read_must_go() {
        // Older lines, the removal, then more than one chunk of newer lines.
        let line = |stamp: &str, text: &str| {
            format!(
                "{{\"type\":\"user\",\"timestamp\":\"{stamp}\",\"message\":{{\"content\":\"{text}\"}}}}\n"
            )
        };
        let mut jsonl = String::new();
        for _ in 0..2000 {
            jsonl += &line("2026-09-29T20:00:00.000Z", "old");
        }
        jsonl += "{\"type\":\"queue-operation\",\"operation\":\"remove\",\"timestamp\":\"2026-09-29T20:32:04.768Z\",\"content\":\"and the linter\",\"reason\":\"absorbed_mid_turn\"}\n";
        let newer = "x".repeat(500);
        for _ in 0..400 {
            jsonl += &line("2026-09-29T20:33:00.000Z", &newer);
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(&path, &jsonl).unwrap();

        // Sent at 20:27:34.
        assert_eq!(
            unqueued_since(Transcript::Claude, &path, 1_790_713_654),
            vec![(1_790_713_924, "and the linter".to_string())]
        );
        assert!(unqueued_since(Transcript::Pi, &path, 0).is_empty());
    }

    #[test]
    fn a_claude_timestamp_reads_as_epoch_seconds() {
        assert_eq!(epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch("2000-03-01T00:00:00.000Z"), Some(951_868_800));
        assert_eq!(epoch("2026-09-29T20:27:34.348Z"), Some(1_790_713_654));
        assert_eq!(epoch("yesterday"), None);
    }

    #[test]
    fn conversation_reads_a_message_claude_took_off_its_queue_as_the_prompt_it_is() {
        // A message typed while claude works reaches the model with no `user`
        // entry, only two queue lines (claude 2.1.278). The removal must read
        // as a prompt, or the card shows no trace of the message.
        let absorbed = concat!(
            "{\"type\":\"user\",\"message\":{\"content\":\"run the tests\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Bash\",\"input\":{\"command\":\"cargo test\"}}]}}\n",
            "{\"type\":\"queue-operation\",\"operation\":\"enqueue\",\"content\":\"also the linter\"}\n",
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\"}]}}\n",
            "{\"type\":\"queue-operation\",\"operation\":\"remove\",\"content\":\"also the linter\",\"reason\":\"absorbed_mid_turn\"}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Both are green.\"}]}}\n",
        );
        assert_eq!(
            read(Transcript::Claude, absorbed),
            vec![
                Said::Prompt("run the tests".to_string()),
                tool("Bash", Some("cargo test")),
                Said::Prompt("also the linter".to_string()),
                Said::Text("Both are green.".to_string()),
            ],
            "the removal is the prompt, and it stands where the pane draws it: \
             after the call the message arrived during, before the answer it \
             changed"
        );

        // An enqueue is not a prompt: the model has not seen it yet. A removal
        // for any other reason is not one either.
        let waiting = concat!(
            "{\"type\":\"user\",\"message\":{\"content\":\"run the tests\"}}\n",
            "{\"type\":\"queue-operation\",\"operation\":\"enqueue\",\"content\":\"also the linter\"}\n",
        );
        assert_eq!(
            read(Transcript::Claude, waiting),
            vec![Said::Prompt("run the tests".to_string())]
        );
        let dropped = waiting.to_string()
            + "{\"type\":\"queue-operation\",\"operation\":\"remove\",\"content\":\"also the linter\",\"reason\":\"cancelled\"}\n";
        assert_eq!(
            read(Transcript::Claude, &dropped),
            vec![Said::Prompt("run the tests".to_string())]
        );
    }

    #[test]
    fn conversation_reads_pasted_image_skill_and_interrupt_prompts_as_they_were_typed() {
        // Shapes from claude transcripts. A prompt with a pasted image is
        // blocks, as is claude's interrupt note; a slash command or skill is a
        // string wrapped in claude's tags.
        let typed = concat!(
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"[Image #3] why is the name the store's?\"},{\"type\":\"image\",\"source\":{\"type\":\"base64\",\"data\":\"iVBO\"}}]}}\n",
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"[Image: source: /tmp/images/3.png]\"}]},\"isMeta\":true}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Bash\",\"input\":{\"command\":\"cargo test\"}}]}}\n",
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"[Request interrupted by user]\"}]}}\n",
            "{\"type\":\"user\",\"message\":{\"content\":\"<command-message>amx</command-message>\\n<command-name>/amx</command-name>\\n<command-args>spawn two agents\\nfor the importer</command-args>\"}}\n",
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Base directory for this skill: /srv/.claude/skills/amx\\n\\n# amx\"}]},\"isMeta\":true,\"sourceToolUseID\":\"toolu_1\"}\n",
            "{\"type\":\"user\",\"message\":{\"content\":\"<command-name>/exit</command-name>\\n            <command-message>exit</command-message>\\n            <command-args></command-args>\"}}\n",
        );
        assert_eq!(
            read(Transcript::Claude, typed),
            vec![
                Said::Prompt("[Image #3] why is the name the store's?".to_string()),
                tool("Bash", Some("cargo test")),
                Said::Prompt("[Request interrupted by user]".to_string()),
                Said::Prompt("/amx spawn two agents\nfor the importer".to_string()),
                Said::Prompt("/exit".to_string()),
            ],
            "the words of the pasted prompt, the interrupt, and each command as \
             the person typed it; the image's source note and the skill's body \
             are claude's"
        );
    }

    #[test]
    fn conversation_meta_and_compaction_lines_are_never_prompts() {
        let written = concat!(
            "{\"type\":\"user\",\"message\":{\"content\":\"<local-command-caveat>Caveat: The messages below were generated by the user while running local commands.</local-command-caveat>\"},\"isMeta\":true}\n",
            "{\"type\":\"user\",\"message\":{\"content\":\"go on\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Going on.\"}]}}\n",
            "{\"type\":\"user\",\"message\":{\"content\":\"This session is being continued from a previous conversation that ran out of context.\\n\\nSummary:\\n1. Primary Request\"},\"isCompactSummary\":true,\"isVisibleInTranscriptOnly\":true}\n",
        );
        assert_eq!(
            read(Transcript::Claude, written),
            vec![
                Said::Prompt("go on".to_string()),
                Said::Text("Going on.".to_string()),
            ]
        );
        assert_eq!(
            answer(Transcript::Claude, written),
            None,
            "a summary after the last answer is a turn going on past it, not an end"
        );
    }

    #[test]
    fn conversation_says_what_limit_stopped_a_claude_turn() {
        // A turn stopped by an account limit is a synthetic entry whose text is
        // the reason (claude 2.1).
        let limited = format!(
            "{CLAUDE}{}\n",
            "{\"type\":\"assistant\",\"message\":{\"model\":\"<synthetic>\",\"role\":\"assistant\",\"stop_reason\":\"stop_sequence\",\"content\":[{\"type\":\"text\",\"text\":\"You've hit your weekly limit · resets Aug 24, 10am (Asia/Dhaka)\"}]},\"error\":\"rate_limit\",\"isApiErrorMessage\":true}"
        );
        assert_eq!(answer(Transcript::Claude, &limited), None);
        assert_eq!(
            why_it_stopped(Transcript::Claude, &limited).as_deref(),
            Some(
                "the vendor said: You've hit your weekly limit · resets Aug 24, 10am (Asia/Dhaka)"
            )
        );
    }

    #[test]
    fn conversation_reads_a_pi_transcript_in_order() {
        assert_eq!(
            read(Transcript::Pi, PI),
            vec![
                Said::Prompt("Hi".to_string()),
                tool("bash", Some("pwd")),
                Said::Text("Hello. You are in /srv/app.".to_string()),
            ]
        );

        // A string prompt reads like a blocks prompt, and a call with no
        // argument worth showing is its name alone.
        let bare = concat!(
            "{\"type\":\"message\",\"id\":\"a1\",\"parentId\":null,\"message\":{\"role\":\"user\",\"content\":\"  go  \"}}\n",
            "{\"type\":\"message\",\"id\":\"a2\",\"parentId\":\"a1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"name\":\"ls\",\"arguments\":{}}]}}\n",
        );
        assert_eq!(
            read(Transcript::Pi, bare),
            vec![Said::Prompt("go".to_string()), tool("ls", None)]
        );
    }

    #[test]
    fn conversation_follows_the_branch_a_pi_session_is_on() {
        // The session above, navigated back to its first prompt and answered
        // again: pi moves its leaf to `0b`, writes a branch summary as its
        // child, and the new answer under that. The tool call and first answer
        // are on the abandoned branch.
        let branched = format!(
            "{PI}{}\n{}\n",
            "{\"type\":\"branch_summary\",\"id\":\"e1\",\"parentId\":\"0b\",\"fromId\":\"c9\",\"summary\":\"was in /srv/app\"}",
            "{\"type\":\"message\",\"id\":\"e2\",\"parentId\":\"e1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Hello again.\"}],\"stopReason\":\"stop\"}}",
        );
        assert_eq!(
            read(Transcript::Pi, &branched),
            vec![
                Said::Prompt("Hi".to_string()),
                Said::Text("Hello again.".to_string()),
            ]
        );
        assert_eq!(
            answer(Transcript::Pi, &branched).as_deref(),
            Some("Hello again."),
            "and the answer is the branch's, not the file's last assistant line"
        );

        // Navigated to before the first prompt: a second root, sharing
        // nothing with the first tree.
        let rerooted = format!(
            "{PI}{}\n",
            "{\"type\":\"message\",\"id\":\"f1\",\"parentId\":null,\"message\":{\"role\":\"user\",\"content\":\"start over\"}}",
        );
        assert_eq!(
            read(Transcript::Pi, &rerooted),
            vec![Said::Prompt("start over".to_string())]
        );

        // A missing parent ends the walk, and so does a cycle.
        let orphaned = "{\"type\":\"message\",\"id\":\"g1\",\"parentId\":\"gone\",\"message\":{\"role\":\"user\",\"content\":\"hm\"}}\n";
        assert_eq!(
            read(Transcript::Pi, orphaned),
            vec![Said::Prompt("hm".to_string())]
        );
        let cyclic = concat!(
            "{\"type\":\"message\",\"id\":\"h1\",\"parentId\":\"h2\",\"message\":{\"role\":\"user\",\"content\":\"one\"}}\n",
            "{\"type\":\"message\",\"id\":\"h2\",\"parentId\":\"h1\",\"message\":{\"role\":\"user\",\"content\":\"two\"}}\n",
        );
        assert_eq!(
            read(Transcript::Pi, cyclic).len(),
            2,
            "each entry once, and the walk ends"
        );
    }

    #[test]
    fn conversation_answers_with_the_last_assistant_text_once_the_turn_has_ended() {
        assert_eq!(
            answer(Transcript::Claude, CLAUDE).as_deref(),
            Some("1\n2\n3")
        );
        assert_eq!(
            answer(Transcript::Pi, PI).as_deref(),
            Some("Hello. You are in /srv/app.")
        );
    }

    #[test]
    fn conversation_says_why_a_turn_that_said_nothing_ended() {
        // `toolUse` and `stop` are ordinary endings; `error`, `aborted` and
        // `length` are the ones a caller has to act on.
        fn pi_ended(on: &str) -> Option<String> {
            why_it_stopped(
                Transcript::Pi,
                &format!(
                    "{}{}\n",
                    PI,
                    format_args!(
                        "{{\"type\":\"message\",\"id\":\"ff\",\"parentId\":\"c9\",\"message\":{{\"role\":\"assistant\",\"content\":[],{on}}}}}"
                    )
                ),
            )
        }

        assert_eq!(why_it_stopped(Transcript::Pi, PI), None, "an ordinary end");
        assert_eq!(why_it_stopped(Transcript::Claude, CLAUDE), None);
        assert_eq!(
            pi_ended("\"stopReason\":\"length\"").as_deref(),
            Some("it stopped at the model's token limit")
        );
        assert_eq!(
            pi_ended("\"stopReason\":\"aborted\"").as_deref(),
            Some("the turn was aborted")
        );
        assert_eq!(
            pi_ended("\"stopReason\":\"error\",\"errorMessage\":\"400: upstream request failed\\nand a second line nobody needs\"").as_deref(),
            Some("the provider failed: 400: upstream request failed"),
            "the first line of the vendor's own error, and not the rest of it"
        );
        assert_eq!(
            pi_ended("\"stopReason\":\"error\"").as_deref(),
            Some("the provider failed"),
            "the reason alone, where the vendor wrote no message with it"
        );
        assert_eq!(
            pi_ended("\"stopReason\":\"refusal\"").as_deref(),
            Some("the vendor stopped on `refusal`"),
            "a reason amx has no word for is repeated as the vendor spelled it"
        );

        // claude's own spelling. A turn still running has no reason.
        let cut_short = format!(
            "{CLAUDE}{}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[],\"stop_reason\":\"max_tokens\"}}"
        );
        assert_eq!(
            why_it_stopped(Transcript::Claude, &cut_short).as_deref(),
            Some("it stopped at the model's token limit")
        );
        let running = format!(
            "{CLAUDE}{}\n",
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"content\":\"1\"}]}}"
        );
        assert_eq!(why_it_stopped(Transcript::Claude, &running), None);
    }

    #[test]
    fn conversation_ending_on_a_tools_result_is_a_turn_still_running() {
        // A trailing tool result (a `user` line on claude, `toolResult` on
        // pi) means the last assistant text belongs to the previous turn.
        let claude_running = format!(
            "{CLAUDE}{}\n",
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"content\":\"ok\"}]}}"
        );
        assert_eq!(answer(Transcript::Claude, &claude_running), None);

        let pi_running = format!(
            "{PI}{}\n",
            "{\"type\":\"message\",\"id\":\"d1\",\"parentId\":\"c9\",\"message\":{\"role\":\"toolResult\",\"content\":[]}}"
        );
        assert_eq!(answer(Transcript::Pi, &pi_running), None);

        // As does one ending on the prompt.
        let asked = "{\"type\":\"message\",\"id\":\"d2\",\"parentId\":null,\"message\":{\"role\":\"user\",\"content\":\"go\"}}\n";
        assert_eq!(answer(Transcript::Pi, asked), None);
    }

    #[test]
    fn conversation_answers_past_the_vendors_bookkeeping() {
        let with_noise = "\
{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"the answer\"}]}}
{\"type\":\"attachment\",\"note\":\"bookkeeping\"}
";
        assert_eq!(
            answer(Transcript::Claude, with_noise).as_deref(),
            Some("the answer"),
            "an attachment is not the end of a turn"
        );
        let pi_noise = format!(
            "{PI}{}\n",
            "{\"type\":\"custom\",\"id\":\"d3\",\"parentId\":\"c9\",\"customType\":\"amx\",\"data\":{}}"
        );
        assert_eq!(
            answer(Transcript::Pi, &pi_noise).as_deref(),
            Some("Hello. You are in /srv/app.")
        );

        assert_eq!(answer(Transcript::Claude, ""), None);
        assert_eq!(answer(Transcript::Claude, "{not json\n"), None);
        assert_eq!(
            answer(
                Transcript::Claude,
                "{\"type\":\"assistant\",\"message\":{\"content\":[]}}\n"
            ),
            None,
            "an assistant line with nothing in it is not an answer"
        );
    }

    #[test]
    fn conversation_thinking_and_tool_blocks_are_not_the_answer() {
        let mixed = "{\"type\":\"assistant\",\"message\":{\"content\":[\
            {\"type\":\"thinking\",\"thinking\":\"hmm\"},\
            {\"type\":\"text\",\"text\":\"the answer\"},\
            {\"type\":\"tool_use\",\"name\":\"Bash\"}]}}\n";
        assert_eq!(
            answer(Transcript::Claude, mixed).as_deref(),
            Some("the answer")
        );
    }

    #[test]
    fn conversation_latest_is_the_newest_thing_said_as_one_row() {
        assert_eq!(
            latest(Transcript::Claude, CLAUDE).as_deref(),
            Some("1"),
            "the words, down to the one line a row has room for"
        );
        assert_eq!(
            latest(Transcript::Pi, PI).as_deref(),
            Some("Hello. You are in /srv/app.")
        );

        // Mid-turn with the result back and nothing said since: the call is
        // the newest row.
        let calling = concat!(
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Bash\",\"input\":{\"command\":\"cargo test --all\"}}]}}\n",
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"content\":\"ok\"}]}}\n",
        );
        assert_eq!(
            latest(Transcript::Claude, calling).as_deref(),
            Some("Bash cargo test --all")
        );

        // A call with no argument worth showing is its name alone.
        let bare = "{\"type\":\"message\",\"id\":\"a1\",\"parentId\":null,\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"name\":\"ls\",\"arguments\":{}}]}}\n";
        assert_eq!(latest(Transcript::Pi, bare).as_deref(), Some("ls"));

        // A prompt is not shown as the newest row.
        let asked = "{\"type\":\"user\",\"message\":{\"content\":\"print the numbers\"}}\n";
        assert_eq!(latest(Transcript::Claude, asked), None);
        assert_eq!(latest(Transcript::Claude, ""), None);
        assert_eq!(
            latest(Transcript::Claude, "{\"type\":\"attachment\"}\n"),
            None,
            "and the vendor's bookkeeping is nothing said at all"
        );
    }

    /// The context half of `context_and_last_words`.
    fn usage_context(format: Transcript, jsonl: &str) -> Option<u64> {
        context_and_last_words(format, jsonl).0
    }

    #[test]
    fn conversation_usage_context_sums_the_last_assistants_usage() {
        let claude = concat!(
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"a\"}],\
             \"usage\":{\"input_tokens\":10,\"cache_creation_input_tokens\":5,\
             \"cache_read_input_tokens\":2}}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"b\"}],\
             \"usage\":{\"input_tokens\":100}}}\n",
        );
        assert_eq!(
            usage_context(Transcript::Claude, claude),
            Some(100),
            "the last entry that carries usage, absent fields at 0"
        );

        let pi = "{\"type\":\"message\",\"id\":\"a1\",\"parentId\":null,\
                   \"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"a\"}],\
                   \"usage\":{\"input\":10,\"cacheRead\":5,\"cacheWrite\":2}}}\n";
        assert_eq!(usage_context(Transcript::Pi, pi), Some(17));

        assert_eq!(
            usage_context(Transcript::Claude, CLAUDE),
            None,
            "no entry carries usage"
        );
        assert_eq!(usage_context(Transcript::Pi, PI), None);

        // claude ends a turn it could not answer with a synthetic entry whose
        // usage is all zeros; the real turn before it is the context.
        let synthetic = concat!(
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"a\"}],\
             \"usage\":{\"input_tokens\":3,\"cache_creation_input_tokens\":140,\
             \"cache_read_input_tokens\":822189}}}\n",
            "{\"type\":\"assistant\",\"message\":{\"model\":\"<synthetic>\",\
             \"content\":[{\"type\":\"text\",\"text\":\"No response requested.\"}],\
             \"usage\":{\"input_tokens\":0,\"cache_creation_input_tokens\":0,\
             \"cache_read_input_tokens\":0}}}\n",
        );
        assert_eq!(
            usage_context(Transcript::Claude, synthetic),
            Some(822332),
            "the last turn that sent tokens, not the zero-sum entry after it"
        );
        assert_eq!(
            answer(Transcript::Claude, synthetic),
            None,
            "and the vendor's no-response note is not the agent's last words"
        );

        let only_synthetic = "{\"type\":\"assistant\",\"message\":{\"model\":\"<synthetic>\",\
             \"content\":[{\"type\":\"text\",\"text\":\"API Error: 529 Overloaded\"}],\
             \"usage\":{\"input_tokens\":0,\"cache_creation_input_tokens\":0,\
             \"cache_read_input_tokens\":0}}}\n";
        assert_eq!(
            usage_context(Transcript::Claude, only_synthetic),
            None,
            "a tail holding nothing else has no context to report"
        );
        assert_eq!(answer(Transcript::Claude, only_synthetic), None);
    }

    #[test]
    fn conversation_answers_context_and_last_words_from_one_read() {
        let claude = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"the importer is ported\"}],\
             \"usage\":{\"input_tokens\":100,\"cache_read_input_tokens\":5}}}\n";
        assert_eq!(
            context_and_last_words(Transcript::Claude, claude),
            (Some(105), Some("the importer is ported".to_string())),
            "the two questions View::json() asks, from the one walk"
        );
        assert_eq!(
            context_and_last_words(Transcript::Claude, CLAUDE).0,
            usage_context(Transcript::Claude, CLAUDE),
            "and the context agrees with the reader that answers it alone"
        );
    }

    #[test]
    fn conversation_title_is_the_last_name_the_session_was_given() {
        // Shapes from claude 2.1.263 transcripts. The title line is rewritten
        // on every change, so the file holds every title the session had.
        let named = concat!(
            "{\"type\":\"ai-title\",\"aiTitle\":\"Audio panel work\",\"sessionId\":\"120567b6\"}\n",
            "{\"type\":\"user\",\"message\":{\"content\":\"and the mixer too\"}}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"Audio panel and mixer work\",\"sessionId\":\"120567b6\"}\n",
        );
        assert_eq!(
            session_title(Transcript::Claude, named).as_deref(),
            Some("Audio panel and mixer work"),
            "the newest of them is what the session goes under now"
        );

        // A title a person typed arrives under its own key.
        let renamed = format!(
            "{named}{}\n",
            "{\"type\":\"custom-title\",\"customTitle\":\"foundation\",\"sessionId\":\"120567b6\"}"
        );
        assert_eq!(
            session_title(Transcript::Claude, &renamed).as_deref(),
            Some("foundation")
        );

        // A blank title leaves the previous one standing.
        let blanked = format!(
            "{renamed}{}\n",
            "{\"type\":\"custom-title\",\"customTitle\":\"  \",\"sessionId\":\"120567b6\"}"
        );
        assert_eq!(
            session_title(Transcript::Claude, &blanked).as_deref(),
            Some("foundation")
        );

        assert_eq!(
            session_title(Transcript::Claude, CLAUDE),
            None,
            "a session nothing has named yet"
        );
        assert_eq!(
            session_title(Transcript::Pi, PI),
            None,
            "and pi keeps no title in its session file"
        );
    }

    #[test]
    fn conversation_prints_plain_with_a_glyph_per_voice() {
        let said = read(Transcript::Claude, CLAUDE);
        assert_eq!(
            plain(&said),
            "❯ print the numbers\n\n› Bash seq 3\n\n1\n2\n3"
        );
        assert_eq!(plain(&[tool("Bash", None)]), "› Bash");
        assert_eq!(
            plain(&[
                tool("Read", Some("a.rs")),
                tool("ls", None),
                Said::Text("ok".into())
            ]),
            "› Read a.rs\n› ls\n\nok",
            "a run of calls is one block"
        );
        assert_eq!(plain(&[]), "");
    }

    /// Rollouts written by codex 0.157.1 (see docs/codex-screens.md,
    /// "Rollouts").
    const CODEX_TURNS: &str =
        include_str!("../tests/codex/rollouts/turn-steer-abort-kill-resume.jsonl");
    const CODEX_ABORTED: &str = include_str!("../tests/codex/rollouts/aborted-first-turn.jsonl");
    const CODEX_APPROVAL: &str = include_str!("../tests/codex/rollouts/approval.jsonl");
    const CODEX_ERROR: &str = include_str!("../tests/codex/rollouts/error.jsonl");
    const CODEX_QUESTION: &str = include_str!("../tests/codex/rollouts/question.jsonl");

    /// The first `lines` lines of a rollout: the file as it stood then.
    fn codex_until(rollout: &str, lines: usize) -> String {
        rollout.split_inclusive('\n').take(lines).collect()
    }

    #[test]
    fn conversation_reads_a_codex_rollout_in_order() {
        // Every typed prompt, including the steered one, and none of codex's
        // injected context (`<environment_context>`, developer instructions,
        // the `<turn_aborted>` note). Reasoning and call output are skipped.
        assert_eq!(
            read(Transcript::Codex, CODEX_TURNS),
            vec![
                Said::Prompt(
                    "Run this shell command and reply with its output only: sleep 45; echo \"$CODEX_SESSION_ID $CODEX_THREAD_ID\"".to_string()
                ),
                tool("exec", None),
                Said::Prompt("Also end your reply with the word steered.".to_string()),
                tool("exec", None),
                Said::Text(
                    "01a0e495-b6aa-7022-ba93-84d2107c2d0e 01a0e495-b6aa-7022-ba93-84d2107c2d0e steered".to_string()
                ),
                Said::Prompt("Run this shell command: sleep 60".to_string()),
                Said::Text("Running it now.".to_string()),
                tool("exec", None),
                Said::Prompt("Run this shell command: sleep 30".to_string()),
                tool("exec", None),
                Said::Prompt("Reply with the single word pong.".to_string()),
                Said::Text("pong".to_string()),
            ]
        );
        assert_eq!(
            read(Transcript::Codex, CODEX_QUESTION),
            vec![
                Said::Prompt(
                    "Use the request_user_input tool to ask me one question: tea or coffee, with those two options. Do nothing else.".to_string()
                ),
                tool("request_user_input", None),
            ],
            "and the empty final answer after the question is nothing said"
        );
    }

    #[test]
    fn conversation_answers_a_codex_turn_with_its_last_agent_message() {
        assert_eq!(
            answer(Transcript::Codex, CODEX_TURNS).as_deref(),
            Some("pong")
        );
        assert_eq!(
            answer(Transcript::Codex, CODEX_APPROVAL).as_deref(),
            Some("Created `hello.txt` successfully.")
        );
        assert_eq!(
            answer(Transcript::Codex, &codex_until(CODEX_TURNS, 26)).as_deref(),
            Some(
                "01a0e495-b6aa-7022-ba93-84d2107c2d0e 01a0e495-b6aa-7022-ba93-84d2107c2d0e steered"
            ),
            "the first turn, as the file stood when it completed"
        );

        // A question turn completes with no message, an errored one with an
        // error, and an Esc'd one with `turn_aborted`, even if a final answer
        // was already written.
        assert_eq!(answer(Transcript::Codex, CODEX_QUESTION), None);
        assert_eq!(answer(Transcript::Codex, CODEX_ERROR), None);
        assert_eq!(answer(Transcript::Codex, CODEX_ABORTED), None);
        assert_eq!(
            answer(Transcript::Codex, &codex_until(CODEX_TURNS, 41)),
            None
        );

        // A turn that started and has not ended is still running, including
        // one whose pane was killed mid-call.
        assert_eq!(
            answer(Transcript::Codex, &codex_until(CODEX_TURNS, 28)),
            None
        );
        assert_eq!(
            answer(Transcript::Codex, &codex_until(CODEX_TURNS, 50)),
            None
        );
    }

    #[test]
    fn conversation_says_why_a_codex_turn_ended_with_nothing() {
        assert_eq!(
            why_it_stopped(Transcript::Codex, CODEX_ABORTED).as_deref(),
            Some("the turn was aborted")
        );
        assert_eq!(
            why_it_stopped(Transcript::Codex, &codex_until(CODEX_TURNS, 41)).as_deref(),
            Some("the turn was aborted")
        );
        assert_eq!(
            why_it_stopped(Transcript::Codex, CODEX_ERROR).as_deref(),
            Some(
                "the provider failed: The 'no-such-model-xyz' model is not supported when using Codex with a ChatGPT account."
            ),
            "the provider's own message, out of the JSON codex wraps it in"
        );
        assert_eq!(why_it_stopped(Transcript::Codex, CODEX_TURNS), None);
        assert_eq!(why_it_stopped(Transcript::Codex, CODEX_QUESTION), None);
        assert_eq!(
            why_it_stopped(Transcript::Codex, &codex_until(CODEX_TURNS, 50)),
            None,
            "a killed pane's turn never ended, as far as the file says"
        );
    }

    #[test]
    fn conversation_latest_and_context_of_a_codex_rollout() {
        assert_eq!(
            latest(Transcript::Codex, CODEX_TURNS).as_deref(),
            Some("pong")
        );
        assert_eq!(
            latest(Transcript::Codex, CODEX_QUESTION).as_deref(),
            Some("request_user_input")
        );
        assert_eq!(
            latest(Transcript::Codex, &codex_until(CODEX_TURNS, 50)).as_deref(),
            Some("exec"),
            "the call the killed pane was in"
        );
        assert_eq!(latest(Transcript::Codex, CODEX_ERROR), None);

        // The input tokens of the last `token_count`.
        assert_eq!(usage_context(Transcript::Codex, CODEX_TURNS), Some(14066));
        assert_eq!(
            usage_context(Transcript::Codex, CODEX_APPROVAL),
            Some(13572)
        );
        assert_eq!(
            usage_context(Transcript::Codex, CODEX_QUESTION),
            Some(13542)
        );
        assert_eq!(usage_context(Transcript::Codex, CODEX_ABORTED), Some(13433));
        assert_eq!(
            usage_context(Transcript::Codex, &codex_until(CODEX_TURNS, 50)),
            Some(13832)
        );
        assert_eq!(
            usage_context(Transcript::Codex, CODEX_ERROR),
            None,
            "a turn that never reached the model counted nothing"
        );
        assert_eq!(
            context_and_last_words(Transcript::Codex, CODEX_TURNS),
            (Some(14066), Some("pong".to_string()))
        );

        assert_eq!(session_title(Transcript::Codex, CODEX_TURNS), None);
        assert!(synthetic_words(Transcript::Codex, CODEX_TURNS).is_empty());
    }

    #[test]
    fn conversation_format_is_the_vendors_and_the_first_entrys_for_a_stranger() {
        assert_eq!(format_of("claude"), Some(Transcript::Claude));
        assert_eq!(format_of("pi --offline"), Some(Transcript::Pi));
        assert_eq!(
            format_of("aider"),
            crate::registry::entries()
                .first()
                .and_then(|v| v.transcript),
            "a command amx has no entry for reads by the first entry, the way \
             its screens do"
        );
    }

    /// Message lists from opencode 2.0.16, as the plugin writes them at the
    /// end of a turn (see docs/opencode-screens.md, "The message lists").
    const OPENCODE_TURN: &str = include_str!("../tests/opencode/messages/turn.jsonl");
    const OPENCODE_STEER: &str = include_str!("../tests/opencode/messages/steer.jsonl");
    const OPENCODE_INTERRUPT: &str = include_str!("../tests/opencode/messages/interrupt.jsonl");
    const OPENCODE_PERMISSION: &str = include_str!("../tests/opencode/messages/permission.jsonl");
    const OPENCODE_QUESTION: &str = include_str!("../tests/opencode/messages/question.jsonl");
    const OPENCODE_FAILURE: &str = include_str!("../tests/opencode/messages/failure.jsonl");

    #[test]
    fn conversation_reads_an_opencode_list_in_order() {
        // Reasoning, tool output and `idle` rows are skipped.
        assert_eq!(
            read(Transcript::Opencode, OPENCODE_STEER),
            vec![
                Said::Prompt(
                    "Run the shell command `sleep 30` with your shell tool, then reply with the single word done.".to_string()
                ),
                tool("shell", Some("sleep 30")),
                Said::Prompt("After that, also say the word banana.".to_string()),
                Said::Text("done\n\nbanana".to_string()),
            ]
        );
        assert_eq!(
            read(Transcript::Opencode, OPENCODE_QUESTION),
            vec![
                Said::Prompt(
                    "Use your question tool to ask me one question: tea or coffee? Offer the two options. Do nothing else.".to_string()
                ),
                tool("question", None),
                Said::Text("Great choice — tea it is! 🍵".to_string()),
                Said::Prompt("Use your question tool again: milk or no milk? Do nothing else.".to_string()),
                tool("question", None),
            ]
        );
    }

    #[test]
    fn conversation_answers_an_opencode_turn_its_idle_row_says_succeeded() {
        assert_eq!(
            answer(Transcript::Opencode, OPENCODE_TURN).as_deref(),
            Some("done")
        );
        assert_eq!(
            answer(Transcript::Opencode, OPENCODE_STEER).as_deref(),
            Some("done\n\nbanana")
        );
        assert_eq!(
            answer(Transcript::Opencode, &codex_until(OPENCODE_PERMISSION, 4)).as_deref(),
            Some("done"),
            "the first turn, as the list stood when it ended"
        );

        // Interrupted and failed turns have no answer, nor do turns ended by a
        // rejected permission or dismissed question (no `idle` row). A turn
        // with a prompt or step after its last `idle` is still running.
        assert_eq!(answer(Transcript::Opencode, OPENCODE_INTERRUPT), None);
        assert_eq!(answer(Transcript::Opencode, OPENCODE_FAILURE), None);
        assert_eq!(answer(Transcript::Opencode, OPENCODE_PERMISSION), None);
        assert_eq!(answer(Transcript::Opencode, OPENCODE_QUESTION), None);
        assert_eq!(
            answer(Transcript::Opencode, &codex_until(OPENCODE_TURN, 2)),
            None
        );
        assert_eq!(
            answer(Transcript::Opencode, &codex_until(OPENCODE_PERMISSION, 5)),
            None
        );
        assert_eq!(
            answer(Transcript::Opencode, &codex_until(OPENCODE_STEER, 3)),
            None
        );
    }

    #[test]
    fn conversation_says_why_an_opencode_turn_ended_with_nothing() {
        assert_eq!(
            why_it_stopped(Transcript::Opencode, OPENCODE_INTERRUPT).as_deref(),
            Some("the turn was aborted")
        );
        assert_eq!(
            why_it_stopped(Transcript::Opencode, OPENCODE_FAILURE).as_deref(),
            Some("the provider failed: measurement: bad request")
        );
        assert_eq!(
            why_it_stopped(Transcript::Opencode, OPENCODE_PERMISSION).as_deref(),
            Some("the turn was aborted"),
            "a rejection ends the turn in an aborted step and no `idle` row"
        );
        assert_eq!(
            why_it_stopped(Transcript::Opencode, OPENCODE_QUESTION).as_deref(),
            Some("the turn was aborted")
        );
        assert_eq!(why_it_stopped(Transcript::Opencode, OPENCODE_TURN), None);
        assert_eq!(
            why_it_stopped(Transcript::Opencode, &codex_until(OPENCODE_TURN, 2)),
            None,
            "a turn still running has not stopped"
        );
    }

    #[test]
    fn conversation_latest_and_context_of_an_opencode_list() {
        assert_eq!(
            latest(Transcript::Opencode, OPENCODE_TURN).as_deref(),
            Some("done")
        );
        assert_eq!(
            latest(Transcript::Opencode, OPENCODE_INTERRUPT).as_deref(),
            Some("shell sleep 60"),
            "the call the Esc cut short"
        );
        assert_eq!(latest(Transcript::Opencode, OPENCODE_FAILURE), None);

        // The last step's input plus cache reads and writes; `input` excludes
        // the cache.
        assert_eq!(
            usage_context(Transcript::Opencode, OPENCODE_TURN),
            Some(7065)
        );
        assert_eq!(
            usage_context(Transcript::Opencode, OPENCODE_PERMISSION),
            Some(7086)
        );
        assert_eq!(
            usage_context(Transcript::Opencode, OPENCODE_FAILURE),
            None,
            "a step that never reached the model counted nothing"
        );
        assert_eq!(
            context_and_last_words(Transcript::Opencode, OPENCODE_STEER),
            (Some(7051), Some("done\n\nbanana".to_string()))
        );

        assert_eq!(session_title(Transcript::Opencode, OPENCODE_TURN), None);
        assert!(synthetic_words(Transcript::Opencode, OPENCODE_TURN).is_empty());
    }
}
