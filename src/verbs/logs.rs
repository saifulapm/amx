//! `amx logs` — what the agent has been up to, without taking this terminal
//! for it.
//!
//! `attach` hands the terminal over and keeps it until you leave. This is a
//! look, and it reads the best account there is:
//!
//! * **The conversation**, where the vendor keeps one. The record holds the
//!   transcript's path from the session's own announcement, and its tail is
//!   the agent's recent history whole — every prompt, answer and tool call —
//!   where a pane could only ever hold one screen of it. A full-screen vendor
//!   scrolls nothing into tmux's history, so the pane is a keyhole and the
//!   transcript is the room.
//! * **The screen**, when there is no conversation to read: a command row, an
//!   agent adopted mid-session, a vendor whose hooks never announced a
//!   transcript. The picture comes with the vendor's own furniture — composer,
//!   statusline, mode footer — cut off the bottom, the same walk the card
//!   takes, because none of it is the agent's work.
//! * **The recorded answer**, once the pane is gone and the record is what is
//!   left. Whether an agent is still running is not something a caller should
//!   have to know before it can ask.
//!
//! `amx result` is still the one that hands back a turn's answer alone,
//! verbatim, and blocks for it. This is the other question: what has been
//! going on over there?

use anyhow::Result;
use std::io::Write;
use std::path::Path;

use crate::store::Agent;
use crate::tmux::{PaneId, Server};
use crate::verbs::send;
use crate::{complain, exit, furniture, paths, tmux, warn};

/// How much of the pane a reading shows when nobody says otherwise. A screenful
/// and then some: enough to see what led to what is on the screen now, and
/// little enough to read.
pub const LINES: u32 = 100;

/// Run the verb against the machine.
pub fn from_env(id: &str, lines: u32) -> Result<i32> {
    let root = paths::state_root()?;
    let to_terminal = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let mut out = std::io::stdout().lock();
    run(&root, id, lines, to_terminal, &mut out)
}

/// The verb, with the state directory named.
pub fn run(
    root: &Path,
    id: &str,
    lines: u32,
    to_terminal: bool,
    out: &mut impl Write,
) -> Result<i32> {
    let agent = Agent::open(root, id)?;
    let meta = agent.meta()?;
    let server = Server::from_socket(meta.socket.clone());

    // Whether there is a screen to read is a question for the pane list and not
    // for the record: the phase says what amx was last told, and this verb is
    // asking what has been going on over there right now.
    match server.pane_alive(&meta.pane) {
        true => match meta.transcript.as_deref().and_then(conversation) {
            Some(said) => {
                let tail = last_lines(&said, lines as usize);
                send::line(&send::rendered(&tail, to_terminal), out)?;
                Ok(exit::OK)
            }
            None => screen(&server, &meta.pane, id, lines, out),
        },
        false => recorded(&agent, id, to_terminal, out),
    }
}

/// The agent's recent conversation, read from the transcript the vendor keeps.
///
/// One JSON document a line, and three kinds worth a reader's time — shapes
/// measured from a live claude 2.1.240 transcript on 2026-08-25. A prompt is a
/// `user` entry whose content is a string, and wears the composer's own `❯` so
/// the two voices read apart. An `assistant` entry's content is typed blocks:
/// `text` is the agent's words, verbatim; `tool_use` is a line naming the tool,
/// because what ran matters and its output would drown the words around it.
/// Everything else — thinking, attachments, tool results, the vendor's
/// bookkeeping — is nobody's reading.
///
/// `None` when the file cannot be read or renders to nothing: a transcript
/// with nothing in it to say is no transcript, and the screen is the fallback.
fn conversation(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let mut told: Vec<String> = Vec::new();
    for line in raw.lines() {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match entry["type"].as_str() {
            Some("user") => {
                if let Some(text) = entry["message"]["content"].as_str()
                    && !text.trim().is_empty()
                {
                    told.push(format!("❯ {}", text.trim()));
                }
            }
            Some("assistant") => {
                for block in entry["message"]["content"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                {
                    match block["type"].as_str() {
                        Some("text") => {
                            let text = block["text"].as_str().unwrap_or("").trim();
                            if !text.is_empty() {
                                told.push(text.to_string());
                            }
                        }
                        Some("tool_use") => {
                            if let Some(name) = block["name"].as_str() {
                                told.push(format!("⚒ {name}"));
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    (!told.is_empty()).then(|| told.join("\n\n"))
}

/// The last `lines` lines of a text, trailing blanks dropped first.
///
/// The sanitizing that [`tail_of`] folds in does not belong here: what this
/// cuts goes on to [`send::rendered`], which is the one place that decides
/// verbatim-down-a-pipe from inert-on-a-terminal.
fn last_lines(text: &str, lines: usize) -> String {
    let mut kept: Vec<&str> = text.lines().collect();
    while kept.last().is_some_and(|line| line.trim().is_empty()) {
        kept.pop();
    }
    kept[kept.len().saturating_sub(lines)..].join("\n")
}

/// What the pane has been saying.
fn screen(
    server: &Server,
    pane: &PaneId,
    id: &str,
    lines: u32,
    out: &mut impl Write,
) -> Result<i32> {
    // The vendor's furniture comes off the bottom before the tail is cut:
    // composer, statusline and mode footer are the vendor's, not the agent's,
    // and the walk is the same one the card takes.
    let sanitized = tmux::sanitize(&capture(server, pane, lines)?);
    let rows: Vec<&str> = sanitized.lines().collect();
    let tail = tail_of(&furniture::cut(&rows).join("\n"), lines as usize);
    if tail.is_empty() {
        // A live pane with nothing on it is an answer, and an empty stdout on
        // its own reads as amx having failed to look.
        warn!("amx: {id} has a pane, and it has printed nothing yet");
        return Ok(exit::OK);
    }
    send::line(&tail, out)?;
    Ok(exit::OK)
}

/// What is left of an agent whose pane has gone.
///
/// The answer on the record, which is the agent's own words rather than a
/// picture of them, so it goes out the way `result` sends it: verbatim down a
/// pipe, inert on a terminal.
fn recorded(agent: &Agent, id: &str, to_terminal: bool, out: &mut impl Write) -> Result<i32> {
    let Some(answer) = agent.state()?.result else {
        complain!("amx: {id} has no pane any more, and amx captured no answer from it");
        return Ok(exit::FAILURE);
    };
    send::line(&send::rendered(&answer, to_terminal), out)?;
    Ok(exit::OK)
}

/// Ask tmux for the pane's recent output.
///
/// `-S -<n>` starts the capture that many lines above the top of the screen, so
/// what comes back is the screen and that much of what has scrolled off it. How
/// tall the screen is is not something the caller asked about, so the reading is
/// cut to length afterwards rather than here.
fn capture(server: &Server, pane: &PaneId, lines: u32) -> Result<String> {
    let start = format!("-{lines}");
    server.run(&[
        "capture-pane",
        "-p",
        "-J",
        "-S",
        &start,
        "-t",
        pane.as_str(),
    ])
}

/// The last `lines` lines of a capture, made inert.
///
/// A screen is padded out to its height with blank rows, so an agent that has
/// printed three lines into a fifty-row pane has forty-seven of them under its
/// output. Nobody asked to read those, and a tail measured through them is a
/// tail of nothing, so the reading ends at the last line with anything on it.
/// Blank lines inside the output are the pane's own and stay where they are.
///
/// Sanitized, like every other capture amx takes and unlike the one the view
/// walks the paint of: this is going to somebody's terminal, and a terminal is
/// an interpreter. What the pane looks like painted is what `attach` is for.
fn tail_of(capture: &str, lines: usize) -> String {
    let sanitized = tmux::sanitize(capture);
    let mut kept: Vec<&str> = sanitized.lines().collect();
    while kept.last().is_some_and(|line| line.trim().is_empty()) {
        kept.pop();
    }
    kept[kept.len().saturating_sub(lines)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Meta, Phase, now};
    use crate::tmux::{Socket, Spawn};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    /// A server of this test's own, gone when the test is.
    struct TestServer(Server);

    impl TestServer {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let name = format!(
                "amx-test-logs-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            // An empty conf, so nothing in the developer's ~/.tmux.conf can
            // change what these tests measure.
            Self(Server::named(name).with_conf("/dev/null"))
        }
    }

    impl std::ops::Deref for TestServer {
        type Target = Server;
        fn deref(&self) -> &Server {
            &self.0
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            let _ = self.0.kill();
        }
    }

    /// Poll until `f` is happy, rather than sleeping and hoping.
    fn until(what: &str, mut f: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {what}");
    }

    /// A record of an agent, pointed at whichever pane the test has.
    fn record(root: &Path, id: &str, socket: Socket, pane: PaneId) -> Agent {
        Agent::create(
            root,
            &Meta {
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                dir: PathBuf::from("/srv/app"),
                worktree: None,
                branch: None,
                base: None,
                socket,
                pane,
                bg: false,
                session: None,
                transcript: None,
                created: now(),
            },
        )
        .expect("the record")
    }

    /// A record naming a pane on a server nothing is listening on.
    fn without_a_pane(root: &Path, id: &str) -> Agent {
        record(
            root,
            id,
            Socket::Name(format!("amx-test-logs-gone-{}", std::process::id())),
            PaneId::new("%404").unwrap(),
        )
    }

    fn printed(root: &Path, id: &str, lines: u32) -> (i32, String) {
        let mut out = Vec::new();
        let code = run(root, id, lines, false, &mut out).expect("a reading");
        (code, String::from_utf8(out).expect("what was printed"))
    }

    #[test]
    fn logs_are_what_the_pane_has_been_saying() {
        let server = TestServer::new();
        let (_, pane) = server
            .new_session(&Spawn {
                command: &[
                    "sh",
                    "-c",
                    "for i in 1 2 3; do echo line $i; done; while :; do sleep 0.05; done",
                ],
                ..Spawn::default()
            })
            .unwrap();

        let root = TempDir::new().unwrap();
        let agent = record(
            root.path(),
            "fix-login-a1b",
            server.socket().clone(),
            pane.clone(),
        );
        // An answer on the record as well, so which of the two this prints is
        // the question the test is asking.
        agent
            .writer()
            .unwrap()
            .update_state(|s| s.result = Some("wrote the parser".to_string()))
            .unwrap();

        until("the pane to say its piece", || {
            server
                .capture(&pane)
                .is_ok_and(|screen| screen.contains("line 3"))
        });

        let (code, said) = printed(root.path(), "fix-login-a1b", LINES);
        assert_eq!(code, exit::OK);
        assert!(
            said.contains("line 1") && said.contains("line 3"),
            "{said:?}"
        );
        assert!(
            !said.contains("wrote the parser"),
            "while there is a pane, the pane is what there is to read: {said:?}"
        );

        // A shorter reading is the last of it and not the first of it.
        let (_, said) = printed(root.path(), "fix-login-a1b", 1);
        assert_eq!(said, "line 3\n");
    }

    #[test]
    fn logs_prefer_the_conversation_the_vendor_keeps() {
        // Shapes measured from a live claude 2.1.240 transcript on
        // 2026-08-25: a user entry's content is a string, an assistant's is
        // an array of typed blocks, and the rest of the file is bookkeeping.
        let transcript = TempDir::new().unwrap();
        let kept = transcript.path().join("session.jsonl");
        std::fs::write(
            &kept,
            concat!(
                "{\"type\":\"mode\",\"x\":1}\n",
                "{\"type\":\"user\",\"message\":{\"content\":\"print the numbers\"}}\n",
                "{\"type\":\"attachment\"}\n",
                "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"thinking\",\"thinking\":\"hm\"}]}}\n",
                "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Bash\",\"input\":{}}]}}\n",
                "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\"}]}}\n",
                "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"1\\n2\\n3\"}]}}\n",
            ),
        )
        .unwrap();

        let server = TestServer::new();
        let (_, pane) = server
            .new_session(&Spawn {
                command: &["sh", "-c", "echo the pane; while :; do sleep 0.05; done"],
                ..Spawn::default()
            })
            .unwrap();

        let root = TempDir::new().unwrap();
        let agent = record(
            root.path(),
            "fix-login-a1b",
            server.socket().clone(),
            pane.clone(),
        );
        agent
            .writer()
            .unwrap()
            .update_meta(|meta| meta.transcript = Some(kept.clone()))
            .unwrap();

        let (code, said) = printed(root.path(), "fix-login-a1b", LINES);
        assert_eq!(code, exit::OK);
        assert!(said.contains("❯ print the numbers"), "{said:?}");
        assert!(said.contains("⚒ Bash"), "{said:?}");
        assert!(said.contains("1\n2\n3"), "{said:?}");
        assert!(
            !said.contains("the pane") && !said.contains("thinking"),
            "the conversation, not a picture of it: {said:?}"
        );

        // A shorter reading is the tail of the conversation.
        let (_, said) = printed(root.path(), "fix-login-a1b", 2);
        assert_eq!(said, "2\n3\n");

        // A transcript that renders to nothing is no transcript: the pane is
        // what there is to read.
        std::fs::write(&kept, "{\"type\":\"mode\"}\n").unwrap();
        until("the pane to say its piece", || {
            server
                .capture(&pane)
                .is_ok_and(|screen| screen.contains("the pane"))
        });
        let (_, said) = printed(root.path(), "fix-login-a1b", LINES);
        assert!(said.contains("the pane"), "{said:?}");
    }

    #[test]
    fn logs_cut_the_vendors_furniture_off_the_screen() {
        // A pane wearing claude's own bottom: composer box, statusline, mode
        // footer. The rows are the measured shapes furniture::cut walks; what
        // the agent printed above them is what a reading is for.
        let server = TestServer::new();
        let (_, pane) = server
            .new_session(&Spawn {
                command: &[
                    "sh",
                    "-c",
                    "printf 'the work itself\\n\\n\\342\\224\\200\\342\\224\\200\\342\\224\\200\\342\\224\\200\\n\\342\\235\\257 try\\n\\342\\224\\200\\342\\224\\200\\342\\224\\200\\342\\224\\200\\n  statusline here\\n  \\342\\217\\270 manual mode on\\n'; while :; do sleep 0.05; done",
                ],
                ..Spawn::default()
            })
            .unwrap();

        let root = TempDir::new().unwrap();
        record(
            root.path(),
            "fix-login-a1b",
            server.socket().clone(),
            pane.clone(),
        );
        until("the footer to be drawn", || {
            server
                .capture(&pane)
                .is_ok_and(|screen| screen.contains("manual mode"))
        });

        let (code, said) = printed(root.path(), "fix-login-a1b", LINES);
        assert_eq!(code, exit::OK);
        assert!(said.contains("the work itself"), "{said:?}");
        assert!(
            !said.contains("manual mode") && !said.contains("statusline here"),
            "the vendor's furniture is not the agent's work: {said:?}"
        );
    }

    #[test]
    fn logs_of_a_pane_that_has_printed_nothing_are_not_a_failure() {
        let server = TestServer::new();
        let (_, pane) = server
            .new_session(&Spawn {
                command: &["sh", "-c", "while :; do sleep 0.05; done"],
                ..Spawn::default()
            })
            .unwrap();

        let root = TempDir::new().unwrap();
        record(
            root.path(),
            "fix-login-a1b",
            server.socket().clone(),
            pane.clone(),
        );

        let (code, said) = printed(root.path(), "fix-login-a1b", LINES);
        assert_eq!(code, exit::OK);
        assert!(said.is_empty(), "{said:?}");
    }

    #[test]
    fn logs_hand_back_the_recorded_answer_once_the_pane_is_gone() {
        let root = TempDir::new().unwrap();
        let agent = without_a_pane(root.path(), "fix-login-a1b");
        agent
            .writer()
            .unwrap()
            .update_state(|s| {
                s.state = Phase::Done;
                s.result = Some("wrote the parser\nand the tests with it".to_string());
            })
            .unwrap();

        let (code, said) = printed(root.path(), "fix-login-a1b", LINES);
        assert_eq!(code, exit::OK);
        assert_eq!(said, "wrote the parser\nand the tests with it\n");
    }

    #[test]
    fn logs_of_an_agent_that_left_nothing_behind_say_so() {
        // No pane and no answer: there is nothing to print, and printing
        // nothing while exiting 0 would read as an agent that said nothing.
        let root = TempDir::new().unwrap();
        without_a_pane(root.path(), "fix-login-a1b");

        let (code, said) = printed(root.path(), "fix-login-a1b", LINES);
        assert_eq!(code, exit::FAILURE);
        assert!(said.is_empty(), "{said:?}");
    }

    #[test]
    fn logs_end_at_the_last_line_with_anything_on_it() {
        // The blank rows a screen is padded out to its height with are not
        // output, and a tail measured through them is a tail of nothing.
        let screen = "first\nsecond\nthird\n\n   \n\n";
        assert_eq!(tail_of(screen, 100), "first\nsecond\nthird");
        assert_eq!(tail_of(screen, 2), "second\nthird");

        // Blank lines inside the output are the pane's own.
        assert_eq!(tail_of("first\n\nthird\n", 100), "first\n\nthird");

        // A pane with nothing on it has nothing to show.
        assert_eq!(tail_of("", 100), "");
        assert_eq!(tail_of("\n\n   \n", 100), "");
    }

    #[test]
    fn hardening_a_screen_cannot_drive_the_terminal_it_is_printed_into() {
        // Every byte here was written by something that is not amx, and a
        // terminal is an interpreter. The same sieve a captured pane goes
        // through everywhere else in amx: replaced, never deleted, so the
        // halves of a spelling cannot close up.
        let painted = "done\u{1b}]0;PWNED\u{7}\n\u{9b}2J ad\u{200b}min\n";
        let shown = tail_of(painted, 100);

        assert!(shown.contains("]0;PWNED"), "still readable: {shown:?}");
        assert!(shown.contains("ad min"), "{shown:?}");
        assert_eq!(
            shown
                .chars()
                .filter(|c| c.is_control() && *c != '\n')
                .count(),
            0,
            "and inert: {shown:?}"
        );
    }

    #[test]
    fn logs_are_never_taken_through_something_that_is_not_an_id() {
        // `root.join(id)` is not a lookup: an id shaped like a path would name
        // a record anywhere on the machine, and then a pane to read.
        let root = TempDir::new().unwrap();
        let mut out = Vec::new();
        for not_one in ["../elsewhere", "never-made-abc"] {
            let refused = run(root.path(), not_one, LINES, false, &mut out).unwrap_err();
            assert!(format!("{refused:#}").contains("no agent"), "{not_one}");
        }
    }
}
