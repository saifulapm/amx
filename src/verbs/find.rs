//! `amx find`: the agents whose record or conversation holds some text.
//!
//! Records are read newest first, live and archived alike, and their
//! transcripts are scanned as they lie: nothing is indexed on disk. One query
//! reads at most [`BUDGET`] bytes of transcript and says so when it stops
//! there, since a long session's transcript runs to hundreds of megabytes.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::Result;

use crate::derive::{self, Record};
use crate::exit;
use crate::paths;
use crate::store;

/// The most transcript one query reads.
pub const BUDGET: u64 = 128 * 1024 * 1024;

/// How much of a transcript is read at a time. A line is never read whole,
/// so one longer than this costs no more memory than a short one.
const CHUNK: usize = 1024 * 1024;

/// Bytes of the matching line kept on each side of the match.
const AROUND: usize = 80;

/// One agent whose record or conversation holds the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub id: String,
    /// The first line that holds it, cut to the text around the match.
    pub line: String,
    /// Whether the record lies in the archive.
    pub archived: bool,
}

/// What a query found, and whether it stopped at the budget.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub hits: Vec<Hit>,
    pub stopped: bool,
}

/// `amx find` against the machine's state directory.
pub fn from_env(text: &str, json: bool) -> Result<i32> {
    let root = paths::state_root()?;
    run(&root, text, json, &mut std::io::stdout().lock())
}

/// Print each agent whose record or conversation holds `text`, newest first.
/// Exits 1 when none does.
pub fn run(root: &Path, text: &str, json: bool, out: &mut impl Write) -> Result<i32> {
    let found = search(root, text, BUDGET)?;
    if json {
        let hits: Vec<_> = found
            .hits
            .iter()
            .map(|hit| {
                serde_json::json!({
                    "id": hit.id,
                    "line": hit.line,
                    "archived": hit.archived,
                })
            })
            .collect();
        let said = serde_json::json!({ "found": hits, "stopped": found.stopped });
        writeln!(out, "{}", serde_json::to_string_pretty(&said)?)?;
    } else {
        for hit in &found.hits {
            writeln!(out, "{}  {}", hit.id, hit.line)?;
        }
        if found.hits.is_empty() {
            writeln!(out, "nothing found")?;
        }
        if found.stopped {
            writeln!(out, "stopped after {} MB", BUDGET / (1024 * 1024))?;
        }
    }
    Ok(match found.hits.is_empty() {
        true => exit::FAILURE,
        false => exit::OK,
    })
}

/// The agents under `root` and its archive whose record or transcript holds
/// `text`, ignoring ASCII case, newest record first, reading at most `budget`
/// bytes of transcript.
pub fn search(root: &Path, text: &str, budget: u64) -> Result<Found> {
    let needle = text.to_ascii_lowercase().into_bytes();
    let mut found = Found::default();
    if needle.is_empty() {
        return Ok(found);
    }
    let shelf = root.join(store::ARCHIVE);
    let mut records: Vec<(Record, bool)> = derive::records(root)?
        .into_iter()
        .map(|record| (record, false))
        .chain(
            derive::records(&shelf)?
                .into_iter()
                .map(|record| (record, true)),
        )
        .collect();
    records.sort_by_key(|(record, _)| std::cmp::Reverse(record.meta.created));

    let mut left = budget;
    for (record, archived) in records {
        let said = [
            Some(record.meta.task.as_str()),
            record.state.name.as_deref(),
            record.state.session_title.as_deref(),
        ];
        let line = match said.into_iter().flatten().find_map(|words| {
            let at = position(&words.to_ascii_lowercase().into_bytes(), &needle)?;
            Some(around(words.as_bytes(), at, needle.len()))
        }) {
            Some(line) => Some(line),
            None => match &record.meta.transcript {
                Some(path) => {
                    let (line, read, stopped) = scan(path, &needle, left);
                    left -= read;
                    if stopped {
                        found.stopped = true;
                        break;
                    }
                    line
                }
                None => None,
            },
        };
        if let Some(line) = line {
            found.hits.push(Hit {
                id: record.meta.id,
                line,
                archived,
            });
        }
    }
    Ok(found)
}

/// The first line of the file at `path` that holds `needle` (lowercase), how
/// many bytes were read, and whether the budget ran out first. A file that
/// cannot be read holds nothing.
fn scan(path: &Path, needle: &[u8], budget: u64) -> (Option<String>, u64, bool) {
    let Ok(mut file) = File::open(path) else {
        return (None, 0, false);
    };
    let mut read: u64 = 0;
    // The end of the last chunk, so a match across the seam is found and a
    // match near the start of a chunk keeps the text before it.
    let mut carried: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; CHUNK];
    loop {
        let room = (budget - read).min(CHUNK as u64) as usize;
        if room == 0 {
            // Out of budget, unless the file ended exactly there.
            let more = file.read(&mut chunk[..1]).is_ok_and(|n| n > 0);
            return (None, read, more);
        }
        let n = match file.read(&mut chunk[..room]) {
            Ok(0) | Err(_) => return (None, read, false),
            Ok(n) => n,
        };
        read += n as u64;
        let mut window = std::mem::take(&mut carried);
        window.extend_from_slice(&chunk[..n]);
        let lower = window.to_ascii_lowercase();
        if let Some(at) = position(&lower, needle) {
            return (Some(around(&window, at, needle.len())), read, false);
        }
        let keep = needle.len().saturating_sub(1) + AROUND;
        carried = window[window.len().saturating_sub(keep)..].to_vec();
    }
}

/// Where `needle` first starts in `haystack`.
fn position(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let first = *needle.first()?;
    let mut from = 0;
    while let Some(skip) = haystack.get(from..)?.iter().position(|byte| *byte == first) {
        let at = from + skip;
        if haystack.get(at..at + needle.len())? == needle {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

/// The line around a match at `at` of `len` bytes: at most [`AROUND`] bytes
/// either side, within the line, marked `…` where it was cut, with control
/// characters (a terminal's escapes among them) made spaces.
fn around(bytes: &[u8], at: usize, len: usize) -> String {
    let floor = at.saturating_sub(AROUND);
    let start = bytes[floor..at]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(floor, |newline| floor + newline + 1);
    let ceiling = (at + len + AROUND).min(bytes.len());
    let end = bytes[at + len..ceiling]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(ceiling, |newline| at + len + newline);
    let cut_before = start > 0 && bytes[start - 1] != b'\n';
    let cut_after = end < bytes.len() && bytes[end] != b'\n';
    let said: String = String::from_utf8_lossy(&bytes[start..end])
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let said = said.trim();
    format!(
        "{}{said}{}",
        if cut_before { "…" } else { "" },
        if cut_after { "…" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Agent;
    use serde_json::json;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// A record under `root` started at `created`, whose transcript holds
    /// `said`.
    fn record(root: &Path, id: &str, created: u64, said: &str) -> PathBuf {
        let transcript = root.parent().unwrap().join(format!("{id}.jsonl"));
        std::fs::write(&transcript, said).unwrap();
        let meta = serde_json::from_value(json!({
            "id": id,
            "task": "a task",
            "dir": "/srv/app",
            "socket": {"name": "amx"},
            "pane": "%1",
            "bg": false,
            "depth": 0,
            "created": created,
            "transcript": transcript,
        }))
        .unwrap();
        std::fs::create_dir_all(root).unwrap();
        Agent::create(root, &meta).unwrap();
        transcript
    }

    fn agents(home: &TempDir) -> PathBuf {
        home.path().join("agents")
    }

    #[test]
    fn find_names_the_agents_whose_conversation_holds_the_text_newest_first() {
        let home = TempDir::new().unwrap();
        let root = agents(&home);
        record(
            &root,
            "old-a1b",
            10,
            "{\"text\":\"port the Importer to v2\"}\n",
        );
        record(&root, "other-b2c", 20, "{\"text\":\"fix the login\"}\n");
        record(
            &root,
            "new-c3d",
            30,
            "{\"text\":\"the importer is done\"}\n",
        );

        let found = search(&root, "importer", BUDGET).unwrap();
        let ids: Vec<&str> = found.hits.iter().map(|hit| hit.id.as_str()).collect();
        assert_eq!(ids, ["new-c3d", "old-a1b"], "case aside, newest first");
        assert_eq!(found.hits[1].line, "{\"text\":\"port the Importer to v2\"}");
        assert!(!found.stopped);

        let mut out = Vec::new();
        assert_eq!(run(&root, "importer", false, &mut out).unwrap(), exit::OK);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "new-c3d  {\"text\":\"the importer is done\"}\n\
             old-a1b  {\"text\":\"port the Importer to v2\"}\n"
        );

        let mut out = Vec::new();
        assert_eq!(
            run(&root, "nothing-matches", false, &mut out).unwrap(),
            exit::FAILURE
        );
        assert_eq!(String::from_utf8(out).unwrap(), "nothing found\n");
    }

    #[test]
    fn find_reads_the_archive_too_in_the_same_newest_first_order() {
        let home = TempDir::new().unwrap();
        let root = agents(&home);
        record(&root, "live-a1b", 10, "the importer\n");
        record(&root.join(store::ARCHIVE), "kept-b2c", 20, "the importer\n");

        let found = search(&root, "importer", BUDGET).unwrap();
        assert_eq!(
            found
                .hits
                .iter()
                .map(|hit| (hit.id.as_str(), hit.archived))
                .collect::<Vec<_>>(),
            [("kept-b2c", true), ("live-a1b", false)]
        );
    }

    #[test]
    fn find_matches_the_task_and_name_without_reading_the_transcript() {
        let home = TempDir::new().unwrap();
        let root = agents(&home);
        record(&root, "port-a1b", 10, "nothing here\n");
        let agent = Agent::open(&root, "port-a1b").unwrap();
        agent
            .writer()
            .unwrap()
            .update_state(|state| state.name = Some("importer work".to_string()))
            .unwrap();
        let found = search(&root, "importer", BUDGET).unwrap();
        assert_eq!(found.hits.len(), 1);
        assert_eq!(found.hits[0].line, "importer work");
    }

    #[test]
    fn find_stops_at_its_budget_and_says_so() {
        let home = TempDir::new().unwrap();
        let root = agents(&home);
        // Two sparse transcripts of 100 MB each: no disk, all zeros, with the
        // word only past the budget.
        for (id, created) in [("one-a1b", 20), ("two-b2c", 10)] {
            let transcript = record(&root, id, created, "");
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(&transcript)
                .unwrap();
            file.set_len(100 * 1024 * 1024).unwrap();
            let mut file = file;
            std::io::Seek::seek(&mut file, std::io::SeekFrom::End(0)).unwrap();
            file.write_all(b"\nthe importer\n").unwrap();
        }

        let found = search(&root, "importer", BUDGET).unwrap();
        assert!(found.stopped);
        assert_eq!(
            found
                .hits
                .iter()
                .map(|hit| hit.id.as_str())
                .collect::<Vec<_>>(),
            ["one-a1b"],
            "the first is read whole; the second runs past 128 MB"
        );
        let mut out = Vec::new();
        run(&root, "importer", false, &mut out).unwrap();
        assert!(
            String::from_utf8(out)
                .unwrap()
                .ends_with("stopped after 128 MB\n")
        );
    }

    #[test]
    fn find_prints_a_cut_line_for_a_match_in_a_line_longer_than_its_chunk() {
        let home = TempDir::new().unwrap();
        let root = agents(&home);
        let mut long = "x".repeat(3 * CHUNK);
        long.push_str("the importer");
        long.push_str(&"y".repeat(2 * CHUNK));
        long.push('\n');
        record(&root, "long-a1b", 10, &long);

        let found = search(&root, "importer", BUDGET).unwrap();
        let line = &found.hits[0].line;
        assert!(line.len() < 4 * AROUND, "{} bytes", line.len());
        assert!(line.starts_with('…') && line.ends_with('…'), "{line:?}");
        assert!(line.contains("the importer"), "{line:?}");
    }

    #[test]
    fn find_finds_a_match_across_two_chunks_and_keeps_what_came_before() {
        let mut said = "a".repeat(CHUNK - 4);
        said.push_str(" the importer here\n");
        let home = TempDir::new().unwrap();
        let root = agents(&home);
        record(&root, "seam-a1b", 10, &said);
        let found = search(&root, "importer", BUDGET).unwrap();
        assert!(
            found.hits[0].line.ends_with("aaa the importer here"),
            "{:?}",
            found.hits[0].line
        );
    }

    #[test]
    fn find_makes_a_terminals_escapes_inert() {
        let home = TempDir::new().unwrap();
        let root = agents(&home);
        record(&root, "esc-a1b", 10, "\u{1b}]0;PWNED\u{7} importer\r\n");
        let found = search(&root, "importer", BUDGET).unwrap();
        assert!(
            !found.hits[0].line.chars().any(char::is_control),
            "{:?}",
            found.hits[0].line
        );
    }
}
