//! `amx agents` against a real session, over the real binary.
//!
//! `agents.rs` is the other half: it drives the renderer with replies a live
//! session takes hours to produce. This half proves the parts a renderer cannot
//! — that the verb reaches a session with no client attached, that `--json` is
//! the reply and not a re-serialization of it, that a `--workspace` label
//! resolves client-side (X02's decision, since the wire takes an id), and that
//! `--watch` takes a terminal, gives it back, and rides a server going away.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::process::Child;

use amx_server::session::probe::probe;
use serde_json::Value;

mod support;

use support::stand_in::{BLOCKED, BLOCKED_LAST_LINE, IDLE, StandIns, WORKING, WORKING_LAST_LINE};
use support::{ALT_ENTER, ALT_LEAVE, Env, Terminal, wait_until};

/// The pty every `--watch` test runs on: the 45-column window §5's acceptance
/// names, and short enough that the footer is somewhere a test can find it.
const ROWS: u16 = 20;
const COLS: u16 = 45;

/// A session with three stand-in agents across two workspaces.
struct Fixture {
    env: Env,
    server: Child,
    _stand_ins: StandIns,
}

impl Fixture {
    /// Install the stand-ins, start a server, and wait until every agent's
    /// status has reached the table.
    fn start(tag: &str) -> Self {
        let env = Env::new(tag);
        // Before the server, always: the hub parses the registry override once,
        // at assembly.
        let stand_ins = StandIns::install(&env);
        let server = env.spawn(&["server", "--session", &env.session]);
        wait_until("the server binds", || {
            probe(&env.socket()).expect("probe").is_running()
        });

        let fixture = Self {
            env,
            server,
            _stand_ins: stand_ins,
        };
        fixture.workspace("api");
        fixture.agent(BLOCKED, "backend");
        fixture.agent(WORKING, "tests");
        fixture.workspace("docs");
        fixture.agent(IDLE, "notes");
        fixture.settle();
        fixture
    }

    /// Create a workspace and land in it.
    fn workspace(&self, label: &str) {
        self.env
            .run(&[
                "workspace",
                "create",
                "--params",
                &format!(r#"{{"label":"{label}","focus":true}}"#),
            ])
            .ok();
    }

    /// Start one stand-in in the focused workspace.
    fn agent(&self, kind: &str, name: &str) {
        self.env
            .run(&[
                "agent",
                "start",
                "--params",
                &format!(r#"{{"kind":"{kind}","name":"{name}"}}"#),
            ])
            .ok();
    }

    /// Wait until every stand-in's screen has been read and committed.
    ///
    /// A condition and a deadline, never a nap: the statuses arrive when the
    /// scripts have painted and the hub has evaluated, which is a different
    /// number of milliseconds on every machine this runs on.
    fn settle(&self) {
        wait_until("every stand-in reaches its state", || {
            let reply = self.json();
            let states: Vec<&str> = reply["agents"]
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .filter_map(|row| row["status"].as_str())
                        .collect()
                })
                .unwrap_or_default();
            states.contains(&"blocked") && states.contains(&"working") && states.contains(&"idle")
        });
    }

    /// `amx agents --json`, decoded.
    fn json(&self) -> Value {
        serde_json::from_str(self.env.run(&["agents", "--json"]).ok()).expect("a JSON reply")
    }

    /// `amx agents`, as a person sees it.
    fn table(&self) -> String {
        self.env.run(&["agents"]).ok().to_owned()
    }

    /// `amx agents --watch` on a 45-column terminal.
    fn watch(&self, args: &[&str]) -> Terminal {
        let mut argv = vec!["agents", "--watch"];
        argv.extend_from_slice(args);
        self.env.spawn_on_tty(&argv, ROWS, COLS)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
    }
}

// ------------------------------------------------------------- the one-shot

#[test]
fn the_table_names_every_agent_with_its_state_its_reason_and_its_last_line() {
    let rig = Fixture::start("ag1");
    let table = rig.table();

    // The count line, then a row per agent — and the blocked one at the top,
    // because the top row of this table is always who needs the user most.
    let lines: Vec<&str> = table.lines().collect();
    assert_eq!(lines[0], "3 agents · 1 blocked", "{table}");
    assert!(lines[2].starts_with("api/backend"), "{table}");
    assert!(lines[2].contains("blocked"), "{table}");

    // The reason is a shipped detector's own name and nothing translates it:
    // these stand-ins emit no hooks, so tier 2 answers and the string is the
    // manifest rule's `name` from `crates/amx-server/assets/manifests/
    // claude.toml`.
    assert!(table.contains("permission_dialog"), "{table}");
    assert!(table.contains("footer_interrupt_hint_working"), "{table}");

    // The last line is the literal bottom row of each pane's screen.
    assert!(table.contains(BLOCKED_LAST_LINE), "{table}");
    assert!(table.contains(WORKING_LAST_LINE), "{table}");

    // Every agent, with its workspace, and no row for a pane that is not one.
    for agent in ["api/backend", "api/tests", "docs/notes"] {
        assert!(table.contains(agent), "{agent} missing from:\n{table}");
    }
}

#[test]
fn json_is_the_reply_and_the_table_is_the_same_reply_rendered() {
    let rig = Fixture::start("ag2");
    let reply = rig.json();

    // `--json` is the wire's own shape: the fields D15 names, spelled the way
    // the method table spells them, so a consumer never has to know a human
    // form exists.
    assert!(reply["seq"].is_number(), "{reply}");
    assert!(reply["now"].is_number(), "{reply}");
    assert_eq!(reply["attention"].as_array().expect("a queue").len(), 1);
    let rows = reply["agents"].as_array().expect("rows");
    assert_eq!(rows.len(), 3, "{reply}");
    let blocked = rows
        .iter()
        .find(|row| row["status"] == "blocked")
        .expect("a blocked agent");
    assert_eq!(blocked["workspace"]["name"], "api");
    assert_eq!(blocked["name"], "backend");
    assert_eq!(blocked["reason"], "permission_dialog");
    assert_eq!(blocked["last_line"], BLOCKED_LAST_LINE);
    assert!(blocked["since"].is_number(), "{blocked}");

    // And the two forms describe one session: every name the reply carries is
    // on the table, which is what makes `--json` a spelling rather than a
    // different answer.
    let table = rig.table();
    for row in rows {
        let name = row["name"].as_str().expect("a name");
        assert!(table.contains(name), "{name} missing from:\n{table}");
    }
}

#[test]
fn a_workspace_label_resolves_client_side_and_an_unknown_one_is_refused() {
    let rig = Fixture::start("ag3");

    // The wire takes an id (X02), so the label was resolved here, against
    // `session.state`, before the call was made.
    let scoped = rig.env.run(&["agents", "--workspace", "docs"]);
    let scoped = scoped.ok();
    assert!(scoped.contains("docs/notes"), "{scoped}");
    assert!(!scoped.contains("api/backend"), "{scoped}");
    // And the count line is about the table it is over. The one blocked agent
    // in this session is in `api`, and a scoped table saying "1 agent · 1
    // blocked" about a workspace with no blocked agent in it would be two
    // scopes in one sentence.
    assert_eq!(scoped.lines().next(), Some("1 agent"), "{scoped}");

    // The queue itself is *not* narrowed — a filtered queue would answer a
    // different question than the one `agent.next` acts on.
    let json: Value = serde_json::from_str(
        rig.env
            .run(&["agents", "--workspace", "docs", "--json"])
            .ok(),
    )
    .expect("a JSON reply");
    assert_eq!(json["agents"].as_array().expect("rows").len(), 1);
    assert_eq!(json["attention"].as_array().expect("a queue").len(), 1);

    // An id is taken as an id and never looked up.
    let id = json["agents"][0]["workspace"]["id"]
        .as_str()
        .expect("a workspace id")
        .to_owned();
    assert!(
        rig.env
            .run(&["agents", "--workspace", &id])
            .ok()
            .contains("docs/notes"),
    );

    // A label nobody has says so, and says what the session does have — the
    // one thing a user who mistyped it needs.
    let refused = rig.env.run(&["agents", "--workspace", "nope"]);
    let message = refused.failed();
    assert!(message.contains("nope"), "{message}");
    assert!(message.contains("api"), "{message}");
    assert!(message.contains("docs"), "{message}");
}

#[test]
fn watch_and_json_together_are_refused_with_the_spelling_that_works() {
    let env = Env::new("ag4");
    let refused = env.run(&["agents", "--watch", "--json"]);
    let message = refused.failed();
    assert!(message.contains("amx events --json"), "{message}");
    // Refused before the session is even probed: the two flags contradict each
    // other whether or not a server is running.
    assert!(!message.contains("not running"), "{message}");
}

#[test]
fn agents_against_a_session_nobody_is_running_says_so_and_starts_nothing() {
    let env = Env::new("ag5");
    let refused = env.run(&["agents"]);
    assert!(refused.failed().contains("is not running"), "{refused:?}");
    assert!(
        !probe(&env.socket()).expect("probe").is_running(),
        "a monitor is not one of the two commands that mean `make this session exist`"
    );
}

// ----------------------------------------------------------------- --watch

#[test]
fn watch_paints_the_table_at_forty_five_columns_and_q_gives_the_terminal_back() {
    let rig = Fixture::start("ag6");
    let mut screen = rig.watch(&[]);

    screen.wait_for(ALT_ENTER);
    screen.wait_for(b"api/backend");
    screen.wait_for(b"q quits");

    // Nothing the watch painted is wider than the window it was painting into.
    // The narrow client D14 exists for is a phone, and a table that wrapped
    // would turn one row into two and move every row below it.
    for line in painted(screen.output()) {
        assert!(
            line.chars().count() <= usize::from(COLS),
            "{} columns: {line:?}",
            line.chars().count()
        );
    }

    screen.send(b"q");
    assert_eq!(screen.wait(), Some(0), "q quits");
    assert!(
        support::window(screen.output(), ALT_LEAVE),
        "the alternate screen was given back"
    );
    assert_eq!(
        screen.termios(),
        screen.initial_termios(),
        "the terminal is how it was found"
    );
}

#[test]
fn watch_survives_the_server_going_away_and_coming_back() {
    let mut rig = Fixture::start("ag7");
    let mut screen = rig.watch(&[]);
    screen.wait_for(b"api/backend");

    // A stop and a fresh start is the harsher half of what a handoff does to a
    // consumer: the socket goes, and what binds it back is a server that has
    // never heard of this connection. A watch that ended there would make an
    // upgrade look like the end of the stream.
    rig.env.run(&["session", "stop"]).ok();
    let _ = rig.server.wait();
    rig.server = rig.env.spawn(&["server", "--session", &rig.env.session]);
    wait_until("the successor binds", || {
        probe(&rig.env.socket()).expect("probe").is_running()
    });

    // It came back, and it says so on the screen rather than leaving a reader
    // to infer it from a table that stopped moving. X00's wave-1 boundary
    // handed this question here: `amx events --json` announces a cold restart
    // on *stderr* while stdout goes straight from one sequence to a lower one
    // with nothing between, and a full-screen watch has somewhere better to
    // put it.
    screen.wait_for(b"the session restarted");
    let seen = String::from_utf8_lossy(screen.output()).into_owned();
    assert!(
        seen.contains("reconnecting…"),
        "the gap between the two servers is said out loud too:\n{seen}"
    );
    screen.send(b"q");
    assert_eq!(screen.wait(), Some(0), "still a live watch afterwards");
}

#[test]
fn watch_refreshes_when_an_agent_moves() {
    let rig = Fixture::start("ag8");
    let mut screen = rig.watch(&["--workspace", "docs"]);
    screen.wait_for(b"docs/notes");
    assert!(
        !support::window(screen.output(), b"docs/second"),
        "the second agent does not exist yet"
    );

    // Nothing tells the watch to look: a delivery lands on the subscription it
    // opened, and one `agent.list` per refresh window answers for every row.
    rig.env
        .run(&["workspace", "switch", "--params", &focus_docs(&rig)])
        .ok();
    rig.agent(WORKING, "second");
    screen.wait_for(b"docs/second");
    screen.send(b"q");
    assert_eq!(screen.wait(), Some(0));
}

/// The `workspace.switch` parameters that land in `docs`.
fn focus_docs(rig: &Fixture) -> String {
    let json: Value = serde_json::from_str(
        rig.env
            .run(&["agents", "--workspace", "docs", "--json"])
            .ok(),
    )
    .expect("a JSON reply");
    let id = json["agents"][0]["workspace"]["id"]
        .as_str()
        .expect("a workspace id");
    format!(r#"{{"workspace":"{id}"}}"#)
}

/// The text a watch put on screen, one line per cursor move, with the escapes
/// taken out.
///
/// Enough to measure a row's width, which is all the assertion above needs; it
/// is not a rasterizer and does not pretend to be one.
fn painted(output: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(output)
        .split("\u{1b}[")
        .filter_map(|chunk| chunk.split_once('H'))
        .map(|(_, text)| text.trim_end_matches('\u{1b}').to_owned())
        .filter(|line| !line.is_empty())
        .collect()
}
