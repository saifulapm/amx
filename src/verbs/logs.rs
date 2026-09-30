//! `amx logs`: the tail of what an agent has been doing, without attaching.
//!
//! Reads the best source available, in this order:
//!
//! - The transcript, where the vendor keeps one. It holds every prompt, answer
//!   and tool call; a full-screen vendor scrolls nothing into tmux's history,
//!   so the pane only ever shows one screen.
//! - The pane, when there is no transcript (a command, an adopted agent, a
//!   vendor that never announced one), with the vendor's chrome cut off the
//!   bottom the way the card cuts it.
//! - Once the pane is gone or belongs to another agent: the recorded answer,
//!   else what the pane printed (kept by its boot for commands and for vendors
//!   that died before their first hook). With neither, a failure naming what
//!   was missing.
//!
//! `amx result` is the verb for one turn's answer.

use anyhow::Result;
use std::io::Write;
use std::path::Path;

use crate::furniture::Furniture;
use crate::store::Agent;
use crate::tmux::{PaneId, Server};
use crate::vendor::{Capability, Vendor};
use crate::verbs::send;
use crate::{complain, exit, furniture, paths, spawn, tmux, warn};

/// Default number of lines printed: a screenful and some context above it.
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

    // The vendor from the record first (the only source for an adopted agent,
    // which has no handoff), else from the handoff's command. A command amx
    // has no entry for comes back as no vendor.
    let started = spawn::read_handoff(agent.dir()).ok();
    let vendor = meta
        .agent
        .as_deref()
        .and_then(crate::vendor::find)
        .or_else(|| started.as_ref().and_then(spawn::vendor_of));
    let told = keeps_a_conversation(vendor)
        .then(|| {
            meta.transcript
                .as_deref()
                .and_then(|path| conversation(path, meta.agent.as_deref().unwrap_or_default()))
        })
        .flatten();

    // The transcript wins with or without a pane: a parked agent's session is
    // still on disk, and its recorded answer is only the last turn.
    if let Some(said) = told {
        let tail = last_lines(&said, lines as usize);
        send::line(&send::rendered(&tail, to_terminal), out)?;
        return Ok(exit::OK);
    }

    // Ask tmux, not the recorded phase, whether there is a pane. It must answer
    // for this agent: pane numbers are reused, and another agent's screen must
    // not be printed under this id.
    match server.pane_answers_for(&meta.pane, &meta.id) {
        true => screen(&server, &meta.pane, id, lines, chrome(vendor), out),
        false => recorded(&agent, id, vendor, lines, to_terminal, out),
    }
}

/// Whether this vendor keeps a transcript amx can read back.
///
/// Checked against the vendor table before any path on the record is opened.
/// With no known vendor, the record's transcript path is trusted.
fn keeps_a_conversation(vendor: Option<&Vendor>) -> bool {
    vendor.is_none_or(|vendor| vendor.can(Capability::Transcript))
}

/// The chrome this vendor draws at the bottom of its pane, to cut off a
/// capture.
///
/// Each vendor's anchors are its own, so the wrong vendor's furniture finds
/// nothing to cut. An unknown vendor uses the default vendor's rules.
fn chrome(vendor: Option<&Vendor>) -> &'static Furniture {
    crate::rules::of(vendor.map_or("", |vendor| vendor.name)).furniture()
}

/// The transcript at `path` rendered as plain text, in the format of vendor
/// `agent` (see [`crate::conversation::plain`]).
///
/// `None` when the file cannot be read or renders to nothing, so the caller
/// falls back to the pane.
fn conversation(path: &Path, agent: &str) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let format = crate::conversation::format_of(agent)?;
    let said = crate::conversation::read(format, &raw);
    (!said.is_empty()).then(|| crate::conversation::plain(&said))
}

/// The last `lines` lines of `text`, after dropping trailing blank lines.
///
/// Does not sanitize: callers pass the result to [`send::rendered`], which
/// decides between verbatim (pipe) and inert (terminal).
fn last_lines(text: &str, lines: usize) -> String {
    let mut kept: Vec<&str> = text.lines().collect();
    while kept.last().is_some_and(|line| line.trim().is_empty()) {
        kept.pop();
    }
    kept[kept.len().saturating_sub(lines)..].join("\n")
}

/// Print the tail of the live pane.
fn screen(
    server: &Server,
    pane: &PaneId,
    id: &str,
    lines: u32,
    chrome: &Furniture,
    out: &mut impl Write,
) -> Result<i32> {
    // Cut the vendor's composer, statusline and footer before taking the tail.
    let sanitized = tmux::sanitize(&capture(server, pane, lines)?);
    let rows: Vec<&str> = sanitized.lines().collect();
    let tail = tail_of(&furniture::cut(chrome, &rows).join("\n"), lines as usize);
    if tail.is_empty() {
        // Say so: an empty stdout alone looks like amx failed to read.
        warn!("amx: {id} has a pane, and it has printed nothing yet");
        return Ok(exit::OK);
    }
    send::line(&tail, out)?;
    Ok(exit::OK)
}

/// Print what is left of an agent whose pane is gone: the recorded answer,
/// else what the pane printed (see [`Agent::output`]), cut to `lines`.
///
/// Rendered like `result`: verbatim down a pipe, inert on a terminal. With
/// neither, or an empty output file, exits `FAILURE`.
fn recorded(
    agent: &Agent,
    id: &str,
    vendor: Option<&Vendor>,
    lines: u32,
    to_terminal: bool,
    out: &mut impl Write,
) -> Result<i32> {
    let left = match agent.state()?.result {
        Some(answer) => Some(last_lines(&answer, lines as usize)),
        None => agent
            .output()
            .map(|printed| last_lines(&printed, lines as usize))
            .filter(|printed| !printed.trim().is_empty()),
    };
    let Some(left) = left else {
        complain!("{}", nothing_left(id, vendor));
        return Ok(exit::FAILURE);
    };
    send::line(&send::rendered(&left, to_terminal), out)?;
    Ok(exit::OK)
}

/// The error for an agent with no pane and nothing recorded. Names a vendor
/// that keeps no transcript, since that is where someone would look next.
fn nothing_left(id: &str, vendor: Option<&Vendor>) -> String {
    let gap = match vendor.filter(|vendor| !vendor.can(Capability::Transcript)) {
        Some(vendor) => format!(", {} keeps no conversation to read back", vendor.name),
        None => String::new(),
    };
    format!("amx: {id} has no pane any more{gap}, and amx captured no answer from it")
}

/// The visible pane plus `lines` lines of scrollback (`-S -<n>`), joined.
///
/// The screen height is unknown here, so the caller cuts it to length.
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

/// The last `lines` non-padding lines of a capture, sanitized.
///
/// tmux pads a capture to the pane height with blank rows, so trailing blanks
/// are dropped before counting. Blank lines inside the output are kept.
fn tail_of(capture: &str, lines: usize) -> String {
    last_lines(&tmux::sanitize(capture), lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Meta, Phase, now};
    use crate::tmux::{Socket, Spawn};
    use crate::vendor::second::SECOND;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    /// A private tmux server, killed on drop.
    struct TestServer(Server);

    impl TestServer {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let name = format!(
                "amx-test-logs-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            // An empty conf keeps the developer's ~/.tmux.conf out of the test.
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

    /// Poll until `f` returns true, panicking after ten seconds.
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

    /// A pane running `command` in the session amx names after `id`, so the
    /// pane answers for that agent (see [`Server::pane_answers_for`]).
    fn a_pane_for(server: &Server, id: &str, command: &[&str]) -> PaneId {
        let session = format!("{}{id}", tmux::SESSION_PREFIX);
        let (_, pane) = server
            .new_session(&Spawn {
                name: Some(&session),
                command,
                ..Spawn::default()
            })
            .expect("a pane to read");
        pane
    }

    /// A record of an agent in `pane` on `socket`.
    fn record(root: &Path, id: &str, socket: Socket, pane: PaneId) -> Agent {
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
        let pane = a_pane_for(
            &server,
            "fix-login-a1b",
            &[
                "sh",
                "-c",
                "for i in 1 2 3; do echo line $i; done; while :; do sleep 0.05; done",
            ],
        );

        let root = TempDir::new().unwrap();
        let agent = record(
            root.path(),
            "fix-login-a1b",
            server.socket().clone(),
            pane.clone(),
        );
        // A recorded answer too, which the live pane must win over.
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

        // `--lines` keeps the end.
        let (_, said) = printed(root.path(), "fix-login-a1b", 1);
        assert_eq!(said, "line 3\n");
    }

    #[test]
    fn logs_cut_the_chrome_of_the_vendor_the_record_names() {
        // An adopted agent has no handoff, only the vendor adopt recorded.
        // Its capture must lose pi's box, working directory and stats line.
        let box_rule = "─".repeat(40);
        let screen = format!(
            "the work\n\n{box_rule}\n\n{box_rule}\n~/srv/app (main)\n↑1.9k ↓1.7k R1.9k 0.3%/1.0M (auto)\n"
        );
        let server = TestServer::new();
        let pane = a_pane_for(
            &server,
            "adopted-a1b",
            &[
                "sh",
                "-c",
                "printf '%s' \"$0\"; while :; do sleep 0.05; done",
                &screen,
            ],
        );
        let root = TempDir::new().unwrap();
        let agent = record(
            root.path(),
            "adopted-a1b",
            server.socket().clone(),
            pane.clone(),
        );
        agent
            .writer()
            .unwrap()
            .update_meta(|meta| meta.agent = Some("pi".to_string()))
            .unwrap();
        until("the pane to draw pi's chrome", || {
            server
                .capture(&pane)
                .is_ok_and(|drawn| drawn.contains("0.3%/1.0M"))
        });

        let (code, said) = printed(root.path(), "adopted-a1b", LINES);
        assert_eq!(code, exit::OK);
        assert_eq!(said, "the work\n", "pi's chrome is pi's, not the agent's");
    }

    #[test]
    fn logs_prefer_the_conversation_the_vendor_keeps() {
        // Shapes from a claude 2.1.240 transcript: a user entry's content is
        // a string, an assistant's is an array of typed blocks, and the other
        // entries are bookkeeping.
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
        let pane = a_pane_for(
            &server,
            "fix-login-a1b",
            &["sh", "-c", "echo the pane; while :; do sleep 0.05; done"],
        );

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
        assert!(said.contains("› Bash"), "{said:?}");
        assert!(said.contains("1\n2\n3"), "{said:?}");
        assert!(
            !said.contains("the pane") && !said.contains("thinking"),
            "the conversation, not a picture of it: {said:?}"
        );

        let (_, said) = printed(root.path(), "fix-login-a1b", 2);
        assert_eq!(said, "2\n3\n");

        // A transcript that renders to nothing falls back to the pane.
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
        // claude's bottom chrome (composer box, statusline, mode footer) in
        // the shapes `furniture::cut` recognises.
        let server = TestServer::new();
        let pane = a_pane_for(
            &server,
            "fix-login-a1b",
            &[
                "sh",
                "-c",
                "printf 'the work itself\\n\\n\\342\\224\\200\\342\\224\\200\\342\\224\\200\\342\\224\\200\\n\\342\\235\\257 try\\n\\342\\224\\200\\342\\224\\200\\342\\224\\200\\342\\224\\200\\n  statusline here\\n  \\342\\217\\270 manual mode on\\n'; while :; do sleep 0.05; done",
            ],
        );

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
        let pane = a_pane_for(
            &server,
            "fix-login-a1b",
            &["sh", "-c", "while :; do sleep 0.05; done"],
        );

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
    fn logs_cut_the_recorded_answer_to_the_lines_asked_for() {
        let root = TempDir::new().unwrap();
        let agent = without_a_pane(root.path(), "fix-login-a1b");
        agent
            .writer()
            .unwrap()
            .update_state(|s| {
                s.state = Phase::Done;
                s.result = Some("one\ntwo\nthree".to_string());
            })
            .unwrap();

        let (code, said) = printed(root.path(), "fix-login-a1b", 2);
        assert_eq!(code, exit::OK);
        assert_eq!(said, "two\nthree\n");
    }

    #[test]
    fn logs_of_a_parked_agent_are_its_conversation() {
        // Parking takes the pane and keeps the session, so the transcript
        // still wins over the recorded answer.
        let transcript = TempDir::new().unwrap();
        let kept = transcript.path().join("session.jsonl");
        std::fs::write(
            &kept,
            concat!(
                "{\"type\":\"user\",\"message\":{\"content\":\"print the numbers\"}}\n",
                "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"1\\n2\\n3\"}]}}\n",
            ),
        )
        .unwrap();

        let root = TempDir::new().unwrap();
        let agent = without_a_pane(root.path(), "fix-login-a1b");
        let writer = agent.writer().unwrap();
        writer
            .update_meta(|meta| meta.transcript = Some(kept.clone()))
            .unwrap();
        writer
            .update_state(|s| {
                s.state = Phase::Idle;
                s.parked_at = now();
                s.result = Some("3".to_string());
            })
            .unwrap();
        drop(writer);

        let (code, said) = printed(root.path(), "fix-login-a1b", LINES);
        assert_eq!(code, exit::OK);
        assert!(said.contains("❯ print the numbers"), "{said:?}");
        assert!(said.contains("1\n2\n3"), "{said:?}");

        let (_, said) = printed(root.path(), "fix-login-a1b", 2);
        assert_eq!(said, "2\n3\n");
    }

    #[test]
    fn logs_of_a_command_that_has_ended_are_what_it_printed() {
        // A command records no answer; its boot pipes the pane into the
        // output file, which is read once the pane is gone.
        let root = TempDir::new().unwrap();
        let agent = without_a_pane(root.path(), "build-a1b");
        agent
            .writer()
            .unwrap()
            .update_state(|s| {
                s.state = Phase::Done;
                s.exit = Some(0);
            })
            .unwrap();
        std::fs::write(agent.dir().join(crate::store::OUTPUT), "one\ntwo\n").unwrap();

        let (code, said) = printed(root.path(), "build-a1b", LINES);
        assert_eq!(code, exit::OK);
        assert_eq!(said, "one\ntwo\n");

        let (_, said) = printed(root.path(), "build-a1b", 1);
        assert_eq!(said, "two\n");

        // An empty output file is a failure, so stdout is never silently empty.
        std::fs::write(agent.dir().join(crate::store::OUTPUT), "").unwrap();
        let (code, said) = printed(root.path(), "build-a1b", LINES);
        assert_eq!(code, exit::FAILURE);
        assert!(said.is_empty(), "{said:?}");
    }

    #[test]
    fn logs_of_a_record_that_has_spoken_do_not_read_its_boot_bytes() {
        // Once a vendor has announced a transcript, the boot output is just a
        // picture of its screen and must not be printed as the answer.
        let root = TempDir::new().unwrap();
        let agent = without_a_pane(root.path(), "fix-login-a1b");
        agent
            .writer()
            .unwrap()
            .update_meta(|meta| meta.transcript = Some(PathBuf::from("/srv/transcript.jsonl")))
            .unwrap();
        std::fs::write(
            agent.dir().join(crate::store::OUTPUT),
            "could not read the state file\n",
        )
        .unwrap();

        let (code, said) = printed(root.path(), "fix-login-a1b", LINES);
        assert_eq!(code, exit::FAILURE);
        assert!(said.is_empty(), "{said:?}");
    }

    #[test]
    fn logs_of_a_command_still_in_its_pane_are_that_pane() {
        // While the command runs, its live pane wins over the output file.
        let server = TestServer::new();
        let pane = a_pane_for(
            &server,
            "build-a1b",
            &["sh", "-c", "echo on the pane; while :; do sleep 0.05; done"],
        );

        let root = TempDir::new().unwrap();
        let agent = record(
            root.path(),
            "build-a1b",
            server.socket().clone(),
            pane.clone(),
        );
        std::fs::write(agent.dir().join(crate::store::OUTPUT), "in the file\n").unwrap();
        until("the pane to say its piece", || {
            server
                .capture(&pane)
                .is_ok_and(|screen| screen.contains("on the pane"))
        });

        let (code, said) = printed(root.path(), "build-a1b", LINES);
        assert_eq!(code, exit::OK);
        assert!(said.contains("on the pane"), "{said:?}");
        assert!(!said.contains("in the file"), "{said:?}");
    }

    #[test]
    fn logs_of_an_agent_that_left_nothing_behind_say_so() {
        // No pane and no answer exits FAILURE with nothing on stdout.
        let root = TempDir::new().unwrap();
        without_a_pane(root.path(), "fix-login-a1b");

        let (code, said) = printed(root.path(), "fix-login-a1b", LINES);
        assert_eq!(code, exit::FAILURE);
        assert!(said.is_empty(), "{said:?}");
    }

    #[test]
    fn logs_look_for_a_conversation_only_where_the_vendor_keeps_one() {
        assert!(keeps_a_conversation(crate::registry::entry("claude")));
        assert!(!keeps_a_conversation(Some(&SECOND)));
        assert!(
            keeps_a_conversation(None),
            "a command amx has no entry for is one amx has measured nothing \
             about, and nothing measured is not a measurement"
        );
    }

    #[test]
    fn logs_of_an_agent_that_left_nothing_behind_name_the_gap() {
        // Only a vendor that keeps no transcript is named in the error.
        let said = nothing_left("fix-login-a1b", Some(&SECOND));
        assert!(said.contains("fix-login-a1b"), "{said}");
        assert!(said.contains(SECOND.name), "{said}");
        assert!(said.contains("no conversation"), "{said}");

        for measured in [crate::registry::entry("claude"), None] {
            let said = nothing_left("fix-login-a1b", measured);
            assert!(said.contains("no pane"), "{said}");
            assert!(
                !said.contains("no conversation"),
                "a vendor that keeps one has nothing to answer for here: {said}"
            );
        }
    }

    #[test]
    fn logs_end_at_the_last_line_with_anything_on_it() {
        // The blank rows tmux pads a capture with are not output.
        let screen = "first\nsecond\nthird\n\n   \n\n";
        assert_eq!(tail_of(screen, 100), "first\nsecond\nthird");
        assert_eq!(tail_of(screen, 2), "second\nthird");

        // Blank lines inside the output are kept.
        assert_eq!(tail_of("first\n\nthird\n", 100), "first\n\nthird");

        assert_eq!(tail_of("", 100), "");
        assert_eq!(tail_of("\n\n   \n", 100), "");
    }

    #[test]
    fn logs_a_screen_cannot_drive_the_terminal_it_is_printed_into() {
        // Control and format characters are replaced with spaces, never
        // deleted, so the halves of a word cannot join up.
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
        // An id shaped like a path would otherwise reach a record, and a pane,
        // outside the state directory.
        let root = TempDir::new().unwrap();
        let mut out = Vec::new();
        for not_one in ["../elsewhere", "never-made-abc"] {
            let refused = run(root.path(), not_one, LINES, false, &mut out).unwrap_err();
            assert!(format!("{refused:#}").contains("no agent"), "{not_one}");
        }
    }
}
