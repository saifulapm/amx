//! The conversation a vendor keeps on disk, read into what was said.
//!
//! A pane holds one screen of an agent's work and a record holds one turn's
//! answer. The transcript the vendor writes holds the whole of it — every
//! prompt, every answer, every tool call — and two surfaces want that whole:
//! the card the view opens over an agent, and `amx logs`. Both read it here,
//! so neither can disagree with the other about what a line of it means.
//!
//! Three vendors keep one, in three shapes, and the shapes are the table's to
//! name — see [`Transcript`]. What this file knows is where in each the words
//! are, and it keeps three kinds of them: what the person asked, what the
//! agent said, and which tool it called with what. Everything else in the file
//! — thinking, tool results, the vendor's bookkeeping about models and
//! compaction — is nobody's reading. A tool's result would drown the words
//! around it, and thinking is the agent's own.
//!
//! Every file is one JSON document a line. pi's is a tree — entries carry an
//! `id` and a `parentId`, and a session can branch in place — and it is read
//! here along the branch its last entry is on, which is the one pi itself
//! shows on a reload — see [`branch`]. A line that is not JSON is skipped
//! rather than fatal: a transcript is appended to while it is read.

use serde_json::Value;
use std::collections::HashMap;

use crate::vendor::Transcript;

/// One thing said in a conversation, in the order it was said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Said {
    /// What the person typed at the composer.
    Prompt(String),
    /// What the agent said, one block of it, verbatim.
    Text(String),
    /// A tool the agent called: which, and the one argument worth a row.
    Tool {
        name: String,
        detail: Option<String>,
    },
}

/// The shape a record reads its conversation by, out of the agent command
/// it was started with.
///
/// The same road `crate::rules::of` takes to a screens document: the vendor
/// the command runs, and the first in the table for a command amx has no
/// entry for — which is the wrapper-script law, and deliberate. A vendor that
/// keeps no conversation answers `None`, and then there is nothing to read.
pub fn format_of(agent: &str) -> Option<Transcript> {
    crate::registry::entry(agent)
        .or_else(|| crate::registry::entries().first())
        .and_then(|vendor| vendor.transcript)
}

/// Everything said in the conversation, in order.
pub fn read(format: Transcript, jsonl: &str) -> Vec<Said> {
    let mut said = Vec::new();
    for entry in &spoken(format, jsonl) {
        match format {
            Transcript::Claude => claude(entry, &mut said),
            Transcript::Pi => pi(entry, &mut said),
            Transcript::Codex => codex(entry, &mut said),
            // Its shape is read off captured lists, not written yet.
            Transcript::Opencode => {}
        }
    }
    said
}

/// The entries a reading walks, in order: every line of a claude transcript
/// or a codex rollout, and of a pi session the branch its last entry is on.
fn spoken(format: Transcript, jsonl: &str) -> Vec<Value> {
    match format {
        Transcript::Claude | Transcript::Codex | Transcript::Opencode => entries(jsonl).collect(),
        Transcript::Pi => branch(entries(jsonl).collect()),
    }
}

/// The branch a pi session is on: from its last entry up through `parentId`
/// to a root, read back down.
///
/// pi's own reader (`buildSessionPath`, session-manager.js at 0.84.4, and
/// unchanged at 0.85.1) takes
/// the last entry in the file as the leaf and walks to the root, and that is
/// the whole of what the file says about which branch is live. Branching
/// writes nothing by itself — the leaf pi keeps in memory moves, and the next
/// entry appended is the first the file knows of the new branch — so a
/// session navigated back to an earlier prompt and continued reads as that
/// continuation, with the path it left behind out of the reading, exactly as
/// pi would show it on a reload. A session that was never branched is one
/// path, and reads as it always did.
///
/// An entry with no `id` — the header — is on no path. A parent the file
/// does not hold ends the walk where it is, and a walk longer than the file
/// is a cycle somebody edited in, and ends too.
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
/// Both vendors write a tool's result as a message of its own after the
/// assistant's, so a conversation ending on one is a turn still running, and
/// answering with the last assistant text would serve the *previous* turn's
/// answer as this one's. That is the unrecoverable direction to be wrong in,
/// so it answers with nothing instead. The vendor's bookkeeping lines are not
/// the end of anything and are read past, and neither is a turn that never
/// reached the vendor — see [`synthetic`].
///
/// codex writes where each turn ends, and its answer is read off that — see
/// [`codex_answer`].
pub fn answer(format: Transcript, jsonl: &str) -> Option<String> {
    last_answer(format, &spoken(format, jsonl))
}

/// The answer at the end of a walk already read, which is what
/// [`answer`] and [`context_and_last_words`] both ask of it.
fn last_answer(format: Transcript, entries: &[Value]) -> Option<String> {
    if format == Transcript::Codex {
        return codex_answer(entries);
    }
    let last = entries
        .iter()
        .rev()
        .find(|entry| voice(format, entry).is_some())?;
    if voice(format, last) != Some(Voice::Assistant) || synthetic(format, last) || cut_off(last) {
        return None;
    }
    answer_text(last)
}

/// Whether an assistant entry stopped before its words were an answer: pi's
/// `aborted`, a turn somebody cut short, and `error`, one its provider failed.
fn cut_off(entry: &Value) -> bool {
    matches!(
        entry["message"]["stopReason"].as_str(),
        Some("aborted" | "error")
    )
}

/// What every synthetic entry in a transcript says — see [`synthetic`].
///
/// claude hands the words of one to the hook that ends the turn as though the
/// agent had said them, and this is the only place that tells the two apart.
pub fn synthetic_words(format: Transcript, jsonl: &str) -> Vec<String> {
    spoken(format, jsonl)
        .iter()
        .filter(|entry| synthetic(format, entry))
        .filter_map(answer_text)
        .collect()
}

/// Why the last turn ended with nothing to show for it, where the vendor
/// wrote a reason worth repeating.
///
/// A turn that captured no answer is a failure a caller has to act on, and
/// what it should do next depends on why: a reply cut off at the model's token
/// limit is asked again shorter, a provider that failed is asked again at all,
/// and an aborted turn is nobody's to retry. The vendor writes all three in
/// the transcript and nowhere else — the pane has scrolled and the hooks say
/// only that the turn ended — so this is the one place they can be read.
///
/// claude writes none of those when the account stops the turn — a weekly
/// limit, credits run out — and says why in a synthetic entry marked as the
/// API's error instead, so its words are the reason, repeated as they are.
///
/// An ordinary ending answers `None`: `stop` and `toolUse`, and claude's
/// `end_turn` and `tool_use`, say nothing about why there are no words, and a
/// reason neither vendor's table names is repeated as the vendor spelled it
/// rather than guessed at.
pub fn why_it_stopped(format: Transcript, jsonl: &str) -> Option<String> {
    let entries = spoken(format, jsonl);
    if format == Transcript::Codex {
        return codex_why(&entries);
    }
    let last = entries
        .iter()
        .rev()
        .find(|entry| voice(format, entry).is_some())?;
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

/// A vendor's error, as much of it as a sentence of amx's own has room for:
/// its first line, and no more of that than reads at a glance.
fn one_line(said: &str) -> String {
    let line = said.lines().next().unwrap_or_default().trim();
    match line.char_indices().nth(160) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_string(),
    }
}

/// What one assistant entry said, as the answer it would be: its text blocks
/// joined, or nothing where it said nothing.
fn answer_text(entry: &Value) -> Option<String> {
    let text: Vec<&str> = blocks(entry)
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect();
    let text = text.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Whether an assistant entry is the vendor's note that the turn never reached
/// it, rather than something the agent said.
///
/// claude writes `"model":"<synthetic>"` for "No response requested.", an API
/// error, a session limit: bookkeeping about a turn that did not happen, and
/// neither an answer nor a last word.
fn synthetic(format: Transcript, entry: &Value) -> bool {
    matches!(format, Transcript::Claude) && entry["message"]["model"] == "<synthetic>"
}

/// The newest thing said, as the one line a row has room for.
///
/// Where [`answer`] waits for the turn to end, this does not: a row says what
/// an agent is doing now, and a call whose result has not come back yet is
/// exactly that. A tool call is its name and the one argument worth a row —
/// `Bash cargo test --all`, `Read src/importer.rs`, a name on its own where
/// the call spells none of them. What the agent said is its first line,
/// because the rest of a paragraph is not a row's to carry.
///
/// A prompt answers nothing. It is what the person typed, and whoever is
/// reading the row typed it.
pub fn latest(format: Transcript, jsonl: &str) -> Option<String> {
    match read(format, jsonl).pop()? {
        Said::Prompt(_) => None,
        Said::Text(words) => words.lines().next().map(str::to_string),
        Said::Tool { name, detail } => Some(match detail {
            Some(detail) => format!("{name} {detail}"),
            None => name,
        }),
    }
}

/// The input side of the conversation's usage, as of the last assistant entry
/// that sent anything: what the next turn would send back to the vendor, in
/// tokens.
///
/// Both vendors report usage per message rather than accumulating it
/// themselves, so the last entry that carries it is the whole of what the
/// conversation has cost so far: claude `input_tokens +
/// cache_creation_input_tokens + cache_read_input_tokens`, pi `input +
/// cacheRead + cacheWrite`. A field the entry does not carry counts as 0.
///
/// A turn that ended without reaching the vendor — claude writes
/// `"model":"<synthetic>"` for "No response requested.", for an API error, for
/// a session limit — carries a usage object of nothing but zeros, and it is
/// written last, so reading it would report a conversation of hundreds of
/// thousands of tokens as costing 0 for the whole window a caller polls. A
/// real turn never sends 0 tokens, so the sum itself tells the two apart for
/// either vendor, and a tail holding no turn that sent anything answers
/// `None`.
///
/// codex keeps usage off its messages, in `token_count` events, and the
/// `input_tokens` of their `last_token_usage` is the last request's whole
/// input, the cached part of it included. A turn that never reached the model
/// writes none.
fn context_of(format: Transcript, entries: &[Value]) -> Option<u64> {
    entries
        .iter()
        .rev()
        .filter(|entry| match format {
            Transcript::Codex => codex_event(entry) == Some("token_count"),
            _ => voice(format, entry) == Some(Voice::Assistant),
        })
        .map(|entry| usage_sum(format, entry))
        .find(|total| *total > 0)
}

/// The input side of one entry's usage, a field it does not carry at 0.
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
        Transcript::Opencode => 0,
    }
}

/// What the next turn would send back to the vendor and the reader's own words
/// at the end of the last one, answered from one walk of the transcript.
///
/// `View::json()` asks both questions of the same 64 KiB tail, and asking each
/// of [`context_of`] and [`last_answer`] separately walks that tail twice —
/// which a program polling a wall of agents pays twice a second. This reads it
/// once and answers both.
pub fn context_and_last_words(format: Transcript, jsonl: &str) -> (Option<u64>, Option<String>) {
    let entries = spoken(format, jsonl);
    (context_of(format, &entries), last_answer(format, &entries))
}

/// The name the session goes under, where something has given it one.
///
/// claude writes the title on a line of its own and writes the whole line
/// again every time it changes, so the file holds every name the session has
/// had and the last of them is the one it goes under now. The vendor's own
/// name for it is `aiTitle` and the one a person typed is `customTitle`; a
/// session somebody has named is one the vendor stops naming — measured at
/// 2.1.263 on 2026-09-08, no transcript holds both — so the last of either
/// answers, and a name with nothing in it is no name at all.
///
/// pi keeps no title in its session file, and there is nothing to read.
pub fn session_title(format: Transcript, jsonl: &str) -> Option<String> {
    match format {
        Transcript::Pi | Transcript::Codex | Transcript::Opencode => None,
        Transcript::Claude => entries(jsonl)
            .filter_map(|entry| {
                let title = match entry["type"].as_str()? {
                    "custom-title" => entry["customTitle"].as_str()?,
                    "ai-title" => entry["aiTitle"].as_str()?,
                    _ => return None,
                }
                .trim();
                (!title.is_empty()).then(|| title.to_string())
            })
            .last(),
    }
}

/// The conversation as lines somebody reads down a terminal or a pipe: a
/// prompt wears the composer's own `❯` so the two voices read apart, a tool
/// call wears `›`, and what the agent said is its own words. One blank line
/// between one thing said and the next, except between one call and the call
/// after it: a run of calls is one block, the way the card draws it.
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

/// Which voice an entry is in, for the formats that have one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Voice {
    User,
    Assistant,
    /// A tool's result, which both vendors write as a message of its own.
    Result,
}

/// The voice of one entry, or `None` for the vendor's bookkeeping.
fn voice(format: Transcript, entry: &Value) -> Option<Voice> {
    match format {
        Transcript::Claude => match entry["type"].as_str()? {
            // A tool's result is a `user` line whose blocks carry it; a
            // prompt is a string, or blocks where an image was pasted into it
            // or claude noted the person cut the turn short.
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
        Transcript::Codex | Transcript::Opencode => None,
    }
}

/// What claude calls a queued message it took into the turn already running,
/// and the reason it gives for taking it off the queue.
///
/// The one case where a person's own words reach the model and are written
/// nowhere a reading can see them. claude keeps a queue of what was typed
/// while it worked, writes an `enqueue` line for each, and writes a `remove`
/// line when it takes one — and for this reason, and only this one, it writes
/// no `user` entry at all. Its own pane draws the message: measured at 2.1.278
/// on 2026-09-21, `❯ also say BRAVO` stood between the tool it interrupted and
/// the answer, with the transcript holding the two queue lines and nothing
/// else about it.
///
/// So the removal is the prompt. It sits exactly where the pane draws it —
/// after the call the message arrived during and before the answer it changed
/// — and it carries the words, which the pane's own row would have to be
/// captured and cut to get back.
///
/// This is what Saiful's `tell-me-about-this-uuz` came to: `Stop no need`,
/// absorbed 868 milliseconds after it was queued, answered by the agent, and
/// on no screen amx could draw.
const QUEUED: &str = "queue-operation";
const ABSORBED: &str = "absorbed_mid_turn";

/// The words of a queued message claude took without writing a turn for it.
fn absorbed(entry: &Value) -> Option<&str> {
    let taken =
        entry["type"] == QUEUED && entry["operation"] == "remove" && entry["reason"] == ABSORBED;
    taken.then(|| entry["content"].as_str()).flatten()
}

/// One claude entry, into what it said.
fn claude(entry: &Value, said: &mut Vec<Said>) {
    if let Some(queued) = absorbed(entry) {
        prompt(Some(queued), said);
        return;
    }
    match voice(Transcript::Claude, entry) {
        // claude's own lines in the person's voice: a skill's body, an image's
        // source, the caveat before a local command, the summary a compaction
        // starts the conversation again from. Nobody typed them.
        Some(Voice::User) if entry["isMeta"] == true || entry["isCompactSummary"] == true => {}
        Some(Voice::User) => {
            let typed = match entry["message"]["content"].as_str() {
                Some(typed) => typed.to_string(),
                None => typed_text(entry),
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

/// One pi entry, into what it said.
fn pi(entry: &Value, said: &mut Vec<Said>) {
    match voice(Transcript::Pi, entry) {
        Some(Voice::User) => {
            // A prompt is a string, or blocks where an image rode with it.
            let content = &entry["message"]["content"];
            match content.as_str() {
                Some(typed) => prompt(Some(typed), said),
                None => prompt(Some(&typed_text(entry)), said),
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

/// One codex rollout line, into what it said.
///
/// A prompt is read off codex's own record of what the person sent, never off
/// the `user` messages it hands the model: those also carry the context codex
/// writes in the person's voice — `<environment_context>`, a project's
/// AGENTS.md — which nobody typed. codex keeps that record as an
/// `item_completed` event whose item is a `UserMessage` on the paginated
/// threads its TUI starts, and as a `user_message` event on legacy ones; a
/// steered message is one of them like any other. Measured off codex 0.157.1
/// on 2026-09-28, where every typed prompt had one and the injected context
/// had none.
///
/// What the agent said and called is its `response_item`s: `output_text` of an
/// assistant message, commentary and final answer alike, and a call as a
/// `function_call` with JSON arguments or a `custom_tool_call` whose input is
/// whatever the tool takes — for `exec`, a script, which names no argument
/// worth a row.
fn codex(entry: &Value, said: &mut Vec<Said>) {
    let payload = &entry["payload"];
    match codex_event(entry) {
        Some("item_completed") if payload["item"]["type"] == "UserMessage" => {
            let typed: Vec<&str> = payload["item"]["content"]
                .as_array()
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect();
            prompt(Some(&typed.join("\n")), said);
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

/// The type of a codex `event_msg` line, and `None` for any other line.
fn codex_event(entry: &Value) -> Option<&str> {
    match entry["type"] == "event_msg" {
        true => entry["payload"]["type"].as_str(),
        false => None,
    }
}

/// The event that last opened or closed a turn in a rollout: `task_started`,
/// `task_complete` or `turn_aborted`.
///
/// A turn whose last word is `task_started` is still running, and a pane
/// killed under it leaves it so for good: codex writes nothing more for that
/// turn, and a later resume does not either (docs/codex-screens.md).
fn codex_turn_end(entries: &[Value]) -> Option<&Value> {
    entries
        .iter()
        .rev()
        .find(|entry| {
            matches!(
                codex_event(entry),
                Some("task_started" | "task_complete" | "turn_aborted")
            )
        })
        .map(|entry| &entry["payload"])
}

/// A codex turn's answer: the `last_agent_message` its `task_complete`
/// carries. An Esc'd turn has no `task_complete` — even where it had written
/// a final answer before the Esc landed — a turn that ended on a question
/// carries a null one, and an errored one carries an `error` beside it.
fn codex_answer(entries: &[Value]) -> Option<String> {
    let end = codex_turn_end(entries)?;
    if end["type"] != "task_complete" || !end["error"].is_null() {
        return None;
    }
    let said = end["last_agent_message"].as_str()?.trim();
    (!said.is_empty()).then(|| said.to_string())
}

/// Why a codex turn ended with nothing: aborted, with the reason codex gave
/// (Esc is `interrupted`), or failed, with the provider's own message.
///
/// codex writes the error it got back as the message, and where that is the
/// provider's JSON — measured with a model the account may not use — its
/// `error.message` is the sentence, and the rest is wrapping.
fn codex_why(entries: &[Value]) -> Option<String> {
    let end = codex_turn_end(entries)?;
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

/// The text blocks of a prompt written as blocks, joined: the words typed
/// beside a pasted image, which is a block of its own.
fn typed_text(entry: &Value) -> String {
    let typed: Vec<&str> = blocks(entry)
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect();
    typed.join("\n")
}

/// A slash command or a skill as the person typed it, out of the tags claude
/// writes it in: `<command-name>/amx</command-name>` beside a
/// `<command-message>` and the `<command-args>` typed after the name.
fn command(written: &str) -> Option<String> {
    let name = tagged(written, "command-name")?.trim();
    let args = tagged(written, "command-args").unwrap_or_default().trim();
    Some(match args.is_empty() {
        true => name.to_string(),
        false => format!("{name} {args}"),
    })
}

/// What stands between `<tag>` and `</tag>`.
fn tagged<'a>(written: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let from = written.find(&open)? + open.len();
    let to = written[from..].find(&format!("</{tag}>"))?;
    Some(&written[from..from + to])
}

/// The message's blocks, or none where the content is not blocks.
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

/// The one argument of a tool call worth a row beside its name: the command
/// a shell ran, the path a file tool touched, the pattern a search looked
/// for. The first line of it, because a row is one line.
///
/// Named by the arguments both vendors' tools spell, in the order a reader
/// would want them; a call spelling none of these is a name on its own.
fn detail(input: &Value) -> Option<String> {
    const WORTH_A_ROW: [&str; 8] = [
        "command",
        "file_path",
        "path",
        "pattern",
        "url",
        "query",
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

/// Every line of the file that is a JSON document.
fn entries(jsonl: &str) -> impl Iterator<Item = Value> + '_ {
    jsonl
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shapes measured from a live claude 2.1.240 transcript on 2026-08-25.
    const CLAUDE: &str = concat!(
        "{\"type\":\"mode\",\"x\":1}\n",
        "{\"type\":\"user\",\"message\":{\"content\":\"print the numbers\"}}\n",
        "{\"type\":\"attachment\"}\n",
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"thinking\",\"thinking\":\"hm\"}]}}\n",
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Bash\",\"input\":{\"command\":\"seq 3\\n# and more\"}}]}}\n",
        "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"content\":\"1\\n2\\n3\"}]}}\n",
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"1\\n2\\n3\"}]}}\n",
    );

    /// Shapes measured from a live pi 0.84.4 session on 2026-09-05: the
    /// header, two bookkeeping entries, and messages in the three voices.
    /// 0.85.1 still writes session version 3.
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
    fn conversation_reads_a_message_claude_took_off_its_queue_as_the_prompt_it_is() {
        // A message typed while claude worked reaches the model and is written
        // nowhere a reading can see it: two queue lines, and no `user` entry
        // at all. Measured at 2.1.278 on 2026-09-21, twice — Saiful's
        // `tell-me-about-this-uuz`, where `Stop no need` was absorbed 868ms
        // after it was queued and then answered, and a driven session of the
        // same shape. claude's own pane draws the row; amx drew nothing, so
        // the card went from `· queued` to no sign of it anywhere.
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

        // The enqueue is not, because a message still on the queue is one the
        // model has not seen and the row already says is waiting — and a
        // removal for any other reason is claude doing something else with it.
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
        // Shapes read off this machine's claude transcripts on 2026-09-26. A
        // prompt with an image pasted into it is blocks, not a string, and so
        // is claude's note that the person cut the turn short; a slash command
        // or a skill is a string of claude's own tags around what was typed.
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
        // Written by claude 2.1 on 2026-08-22: a turn the account's limit
        // stopped is a synthetic entry whose words are the reason.
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

        // A prompt pi writes as a string reads the same as one it writes as
        // blocks, and a call spelling none of the arguments worth a row is a
        // name on its own.
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
        // again: pi moves its leaf to `0b`, writes the summary of the path it
        // left behind as a child of it, and the new answer as a child of that.
        // The reading is the prompt and the new answer; the tool call and
        // the first answer are on the branch left behind.
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

        // Navigated to before the first prompt and asked something else: a
        // second root, and nothing of the first tree is on its path.
        let rerooted = format!(
            "{PI}{}\n",
            "{\"type\":\"message\",\"id\":\"f1\",\"parentId\":null,\"message\":{\"role\":\"user\",\"content\":\"start over\"}}",
        );
        assert_eq!(
            read(Transcript::Pi, &rerooted),
            vec![Said::Prompt("start over".to_string())]
        );

        // A parent the file does not hold ends the walk where it is, and a
        // cycle somebody edited in ends it too.
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
        // The reasons a live pi wrote over 9000 turns on this machine:
        // `toolUse` and `stop` are how a turn ends, and `error`, `aborted` and
        // `length` are the three a caller with no answer has to act on.
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

        // claude's own spelling, and a turn still running says nothing: the
        // question is only asked of a turn that ended with no answer.
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
        // Tool results are `user` lines on claude and `toolResult` messages
        // on pi, and there are ten of them for every real turn. A trailing
        // one means the last assistant text belongs to the turn before.
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

        // And so is one ending on the prompt itself.
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

        // Mid-turn, with the call's result back and nothing said since: the
        // call is the newest row there is, its command beside its name.
        let calling = concat!(
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Bash\",\"input\":{\"command\":\"cargo test --all\"}}]}}\n",
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"content\":\"ok\"}]}}\n",
        );
        assert_eq!(
            latest(Transcript::Claude, calling).as_deref(),
            Some("Bash cargo test --all")
        );

        // A call spelling none of the arguments worth a row is its name alone.
        let bare = "{\"type\":\"message\",\"id\":\"a1\",\"parentId\":null,\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"name\":\"ls\",\"arguments\":{}}]}}\n";
        assert_eq!(latest(Transcript::Pi, bare).as_deref(), Some("ls"));

        // What the person typed is not news to whoever is reading the row.
        let asked = "{\"type\":\"user\",\"message\":{\"content\":\"print the numbers\"}}\n";
        assert_eq!(latest(Transcript::Claude, asked), None);
        assert_eq!(latest(Transcript::Claude, ""), None);
        assert_eq!(
            latest(Transcript::Claude, "{\"type\":\"attachment\"}\n"),
            None,
            "and the vendor's bookkeeping is nothing said at all"
        );
    }

    /// The context half of the combined reader, which is the only reader the
    /// tests below have to ask.
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

        // claude ends a turn it could not answer with a <synthetic> entry
        // whose usage is all zeros -- "No response requested.", an API error,
        // a session limit. That is not what the conversation costs, and the
        // real turn before it is.
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
        // Shapes measured from live claude 2.1.263 transcripts on 2026-09-08.
        // The vendor writes the whole line again every time the name changes,
        // so the file holds every name the session has had.
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

        // A name a person typed arrives under a key of its own, after
        // whatever the vendor had been calling the session.
        let renamed = format!(
            "{named}{}\n",
            "{\"type\":\"custom-title\",\"customTitle\":\"foundation\",\"sessionId\":\"120567b6\"}"
        );
        assert_eq!(
            session_title(Transcript::Claude, &renamed).as_deref(),
            Some("foundation")
        );

        // A name with nothing in it is no name, and leaves the one before it
        // standing.
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

    /// Rollouts codex 0.157.1 wrote on 2026-09-28, copied out of a scratch
    /// `CODEX_HOME` — see docs/codex-screens.md, "Rollouts".
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
        // Every prompt the person typed, the steered one among them, and
        // none of codex's own context: the `<environment_context>` it writes
        // in the person's voice, the developer instructions, the
        // `<turn_aborted>` note. What the agent said is its commentary and
        // its final answers; reasoning and a call's output are nobody's.
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

        // A question turn completes with no last message, an errored one
        // with an error, and an Esc'd one with `turn_aborted` -- even where
        // the agent had written a final answer before the Esc landed.
        assert_eq!(answer(Transcript::Codex, CODEX_QUESTION), None);
        assert_eq!(answer(Transcript::Codex, CODEX_ERROR), None);
        assert_eq!(answer(Transcript::Codex, CODEX_ABORTED), None);
        assert_eq!(
            answer(Transcript::Codex, &codex_until(CODEX_TURNS, 41)),
            None
        );

        // A turn that has started and not ended is still running: one just
        // started after a turn that did answer, and one whose pane was killed
        // mid-call, which leaves it open in the file for good.
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

        // The input side of the last token_count: the last request's.
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
            format_of("opencode"),
            crate::registry::entries()
                .first()
                .and_then(|v| v.transcript),
            "a command amx has no entry for reads by the first entry, the way \
             its screens do"
        );
    }
}
