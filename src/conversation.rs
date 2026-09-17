//! The conversation a vendor keeps on disk, read into what was said.
//!
//! A pane holds one screen of an agent's work and a record holds one turn's
//! answer. The transcript the vendor writes holds the whole of it — every
//! prompt, every answer, every tool call — and two surfaces want that whole:
//! the card the view opens over an agent, and `amx logs`. Both read it here,
//! so neither can disagree with the other about what a line of it means.
//!
//! Two vendors keep one, in two shapes, and the shapes are the table's to
//! name — see [`Transcript`]. What this file knows is where in each the words
//! are, and it keeps three kinds of them: what the person asked, what the
//! agent said, and which tool it called with what. Everything else in the file
//! — thinking, tool results, the vendor's bookkeeping about models and
//! compaction — is nobody's reading. A tool's result would drown the words
//! around it, and thinking is the agent's own.
//!
//! Both files are one JSON document a line. pi's is a tree — entries carry an
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
        }
    }
    said
}

/// The entries a reading walks, in order: every line of a claude transcript,
/// and of a pi session the branch its last entry is on.
fn spoken(format: Transcript, jsonl: &str) -> Vec<Value> {
    match format {
        Transcript::Claude => entries(jsonl).collect(),
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
pub fn answer(format: Transcript, jsonl: &str) -> Option<String> {
    last_answer(format, &spoken(format, jsonl))
}

/// The answer at the end of a walk already read, which is what
/// [`answer`] and [`context_and_last_words`] both ask of it.
fn last_answer(format: Transcript, entries: &[Value]) -> Option<String> {
    let last = entries
        .iter()
        .rev()
        .find(|entry| voice(format, entry).is_some())?;
    if voice(format, last) != Some(Voice::Assistant) || synthetic(format, last) {
        return None;
    }
    answer_text(last)
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
fn context_of(format: Transcript, entries: &[Value]) -> Option<u64> {
    entries
        .iter()
        .rev()
        .filter(|entry| voice(format, entry) == Some(Voice::Assistant))
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
        Transcript::Pi => None,
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
            // A tool's result is a `user` line whose content is blocks; a
            // prompt's is a string.
            "user" => Some(match entry["message"]["content"].is_string() {
                true => Voice::User,
                false => Voice::Result,
            }),
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
    }
}

/// One claude entry, into what it said.
fn claude(entry: &Value, said: &mut Vec<Said>) {
    match voice(Transcript::Claude, entry) {
        Some(Voice::User) => prompt(entry["message"]["content"].as_str(), said),
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
                None => {
                    let typed: Vec<&str> = blocks(entry)
                        .iter()
                        .filter(|block| block["type"] == "text")
                        .filter_map(|block| block["text"].as_str())
                        .collect();
                    prompt(Some(&typed.join("\n")), said);
                }
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
