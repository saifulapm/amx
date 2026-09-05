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
//! here in the order the lines were written, which is the branch every
//! session that was never forked or navigated has. A line that is not JSON is
//! skipped rather than fatal: a transcript is appended to while it is read.

use serde_json::Value;

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
    for entry in entries(jsonl) {
        match format {
            Transcript::Claude => claude(&entry, &mut said),
            Transcript::Pi => pi(&entry, &mut said),
        }
    }
    said
}

/// The answer at the end of the conversation, if the turn has ended.
///
/// Both vendors write a tool's result as a message of its own after the
/// assistant's, so a conversation ending on one is a turn still running, and
/// answering with the last assistant text would serve the *previous* turn's
/// answer as this one's. That is the unrecoverable direction to be wrong in,
/// so it answers with nothing instead. The vendor's bookkeeping lines are not
/// the end of anything and are read past.
pub fn answer(format: Transcript, jsonl: &str) -> Option<String> {
    let entries: Vec<Value> = entries(jsonl)
        .filter(|entry| voice(format, entry).is_some())
        .collect();
    let last = entries.last()?;
    if voice(format, last) != Some(Voice::Assistant) {
        return None;
    }
    let text: Vec<&str> = blocks(last)
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect();
    let text = text.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// The conversation as lines somebody reads down a terminal or a pipe: a
/// prompt wears the composer's own `❯` so the two voices read apart, a tool
/// call wears `⚒`, and what the agent said is its own words. One blank line
/// between one thing said and the next.
pub fn plain(said: &[Said]) -> String {
    said.iter()
        .map(|one| match one {
            Said::Prompt(text) => format!("❯ {text}"),
            Said::Text(text) => text.clone(),
            Said::Tool { name, detail } => match detail {
                Some(detail) => format!("⚒ {name} {detail}"),
                None => format!("⚒ {name}"),
            },
        })
        .collect::<Vec<_>>()
        .join("\n\n")
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
            "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"  go  \"}}\n",
            "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"name\":\"ls\",\"arguments\":{}}]}}\n",
        );
        assert_eq!(
            read(Transcript::Pi, bare),
            vec![Said::Prompt("go".to_string()), tool("ls", None)]
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
            "{\"type\":\"message\",\"message\":{\"role\":\"toolResult\",\"content\":[]}}"
        );
        assert_eq!(answer(Transcript::Pi, &pi_running), None);

        // And so is one ending on the prompt itself.
        let asked = "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"go\"}}\n";
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
            "{\"type\":\"custom\",\"customType\":\"amx\",\"data\":{}}"
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
    fn conversation_prints_plain_with_a_glyph_per_voice() {
        let said = read(Transcript::Claude, CLAUDE);
        assert_eq!(
            plain(&said),
            "❯ print the numbers\n\n⚒ Bash seq 3\n\n1\n2\n3"
        );
        assert_eq!(plain(&[tool("Bash", None)]), "⚒ Bash");
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
