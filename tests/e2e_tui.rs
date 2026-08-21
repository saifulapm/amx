//! The agent view: what a person sees in it, and what looking does.
//!
//! Every one of these drives the view in a real tmux pane, because a view is
//! only a view when something is drawing it on a terminal: what it puts on the
//! screen, what a keypress does to that, and what it leaves behind when it
//! closes are all questions a pty answers and nothing else does.

mod common;

use common::Harness;
use serde_json::json;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Epoch seconds, for the records a test writes as though they had just
/// happened.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock")
        .as_secs()
}

/// An agent whose command has ended: no pane, and the record is the whole
/// story. `ago` is how long since it ended, which is what orders them.
fn finished(amx: &Harness, id: &str, state: &str, ago: u64) {
    let at = now() - ago;
    amx.record(id, "%404");
    amx.set_state(
        id,
        json!({
            "state": state,
            "exit": 0,
            "since": at,
            "last_event": at,
            "result": "did what it was asked",
        }),
    );
}

/// What is on the view's screen now.
fn screen(amx: &Harness, pane: &str) -> String {
    amx.capture(pane)
}

/// The mark on an agent's row, as the view has it drawn now: past the gutter
/// its rows are indented by.
fn mark(amx: &Harness, view: &str, id: &str) -> Option<char> {
    screen(amx, view)
        .lines()
        .find(|line| line.contains(id))?
        .trim_start()
        .chars()
        .next()
}

/// The same screen with the colours the view drew it in, as the escapes tmux
/// wrote them: what a bar is made of cannot be read off the text.
fn coloured(amx: &Harness, pane: &str) -> String {
    amx.tmux(&["capture-pane", "-p", "-e", "-J", "-t", pane])
}

/// The line of the list holding `text`, escapes and all. The last of them,
/// because the header at the top says what there is in the same words the
/// headings under it do, and the list is the part the cursor walks.
fn coloured_line(amx: &Harness, view: &str, text: &str) -> String {
    let drawn = coloured(amx, view);
    drawn
        .lines()
        .rfind(|line| line.contains(text))
        .unwrap_or_else(|| panic!("no line holding {text} in:\n{drawn}"))
        .to_string()
}

/// Move a record's directory, which is what decides its project.
fn running_in(amx: &Harness, id: &str, dir: &std::path::Path) {
    let path = amx.agent_dir(id).join("meta.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("the record")).expect("the record");
    meta["dir"] = json!(dir);
    std::fs::write(&path, serde_json::to_vec(&meta).expect("the record")).expect("the record");
}

/// Every agent amx holds a record for.
fn agents(amx: &Harness) -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(amx.state_root())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    ids.sort();
    ids
}

/// Wait for a view with nothing in it, which on a terminal this size is the
/// four groups saying what would land in them. The last of the four is what
/// says the whole empty state is up.
fn until_empty(amx: &Harness, view: &str) {
    amx.until("the empty view", || {
        screen(amx, view).contains("here to read").then_some(())
    });
}

/// Type a line at the view, as a person types one.
fn types(amx: &Harness, view: &str, text: &str) {
    amx.tmux(&["send-keys", "-t", view, "-l", text]);
}

fn press(amx: &Harness, view: &str, key: &str) {
    amx.tmux(&["send-keys", "-t", view, key]);
}

/// Paste text at the view the way a terminal delivers a paste: in one
/// bracketed piece, with every newline in it left alone.
fn pastes(amx: &Harness, view: &str, text: &str) {
    amx.tmux(&["set-buffer", "--", text]);
    amx.tmux(&["paste-buffer", "-p", "-r", "-t", view]);
}

/// A pane showing exactly these rows and nothing else, where a real agent's
/// pane would be: the fixture screens the chrome cut is measured against, put
/// somewhere the view has to read them the way it reads any other pane.
fn a_pane_showing(amx: &Harness, rows: &[&str]) -> String {
    let drawn: String = rows.iter().map(|row| format!("{row}\\n")).collect();
    amx.tmux(&[
        "new-session",
        "-d",
        "-x",
        "60",
        "-y",
        "24",
        "-P",
        "-F",
        "#{pane_id}",
        "--",
        "sh",
        "-c",
        &format!("printf '{drawn}'; while :; do sleep 0.05; done"),
    ])
}

/// The five rows claude draws at the bottom of every pane it has the room
/// for, in the vendor's own order: the composer's top border with its
/// right-anchored label, whatever is staged in the box, the composer's bottom
/// border, the statusline, and the mode footer. Transcribed from a live
/// 2.1.237 on 2026-08-21.
const CHROME: [&str; 5] = [
    "──────────────────────────── execute amx-v2 ─",
    "❯ ",
    "─────────────────────────────────────────────",
    "  Opus 5 │ amx-main (main) │ xhigh",
    "  ⏵⏵ accept edits on (shift+tab to cycle)",
];

/// A task of numbered lines, more of them than any composer will show at once.
fn twenty_rows() -> String {
    (1..=20)
        .map(|n| format!("row-{n:02}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A view on a terminal that can start agents of its own: the vendor's
/// stand-in as the agent command, and the scenario it plays.
fn a_view_that_dispatches(amx: &Harness, scenario: &str) -> String {
    amx.config(&format!("agent = \"{}\"\nworktrees = false\n", amx.mock()));
    let scenario = amx.scenario(scenario).to_string_lossy().into_owned();
    let transcript = amx
        .home()
        .join("composed.jsonl")
        .to_string_lossy()
        .into_owned();

    let view = amx.in_a_terminal(
        &[
            ("MOCK_CLAUDE_SCENARIO", &scenario),
            ("MOCK_CLAUDE_TRANSCRIPT", &transcript),
        ],
        &[],
    );
    until_empty(amx, &view);
    view
}

/// A view whose vendor is claude, which is the agent the registry declares
/// dials for: the stand-in under claude's name, on the path a spawn from this
/// terminal looks down.
fn a_view_that_dispatches_as_claude(amx: &Harness, config: &str) -> String {
    a_view_that_can_start_claude(amx, &format!("agent = \"claude\"\n{config}"))
}

/// The same terminal under a whole config of its own, for the tests that say
/// what the file asked for rather than taking claude as read.
fn a_view_that_can_start_claude(amx: &Harness, config: &str) -> String {
    let bin = amx.home().join("bin");
    std::fs::create_dir_all(&bin).expect("a directory for the stand-in");
    std::fs::copy(amx.mock(), bin.join("claude")).expect("the stand-in under claude's name");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    amx.config(config);
    let scenario = amx.scenario("happy-turn").to_string_lossy().into_owned();
    let transcript = amx
        .home()
        .join("composed.jsonl")
        .to_string_lossy()
        .into_owned();

    let view = amx.in_a_terminal(
        &[
            ("MOCK_CLAUDE_SCENARIO", &scenario),
            ("MOCK_CLAUDE_TRANSCRIPT", &transcript),
            ("PATH", &path),
        ],
        &[],
    );
    until_empty(amx, &view);
    view
}

/// Make a directory a git repository with one commit in it, so an agent
/// started there can be given a tree of its own.
fn a_repo_at(dir: &std::path::Path) {
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "amx tests")
            .env("GIT_AUTHOR_EMAIL", "tests@example.invalid")
            .env("GIT_COMMITTER_NAME", "amx tests")
            .env("GIT_COMMITTER_EMAIL", "tests@example.invalid")
            .output()
            .expect("running git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "amx tests"]);
    git(&["config", "user.email", "tests@example.invalid"]);
    std::fs::write(dir.join("README.md"), "before\n").expect("a file to commit");
    git(&["add", "README.md"]);
    git(&["commit", "-m", "first"]);
}

/// The one agent the view started, once its record is whole.
///
/// The directory comes before the record in it: `new` starts the pane first,
/// so that the record it writes can name the pane.
fn composed(amx: &Harness) -> String {
    amx.until("the agent to be started", || {
        let started = agents(amx);
        let id = (started.len() == 1).then(|| started[0].clone())?;
        amx.meta(&id)["pane"].as_str().map(|_| id)
    })
}

/// The next one it started, for a test that dispatches twice.
fn composed_after(amx: &Harness, first: &str) -> String {
    amx.until("the next agent to be started", || {
        let id = agents(amx).into_iter().find(|id| id != first)?;
        amx.meta(&id)["pane"].as_str().map(|_| id)
    })
}

/// The argv amx wrote for the vendor, as the pane will be handed it.
fn command_of(amx: &Harness, id: &str) -> Vec<String> {
    amx.handoff(id)["command"]
        .as_array()
        .expect("the handoff names a command")
        .iter()
        .map(|arg| arg.as_str().expect("an argument").to_string())
        .collect()
}

fn pane_field(amx: &Harness, pane: &str, format: &str) -> String {
    amx.tmux(&["display-message", "-p", "-t", pane, format])
}

#[test]
fn the_view_gathers_the_agents_under_what_they_need() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("port-importer-b2c", "works-with-a-spinner");
    amx.play("fix-login-c3d", "happy-turn");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("port-importer-b2c", "working");
    amx.until_state("fix-login-c3d", "idle");
    finished(&amx, "old-job-d4e", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    let drawn = amx.until("every group", || {
        let drawn = screen(&amx, &view);
        ["needs input", "working", "idle", "completed"]
            .iter()
            .all(|group| drawn.contains(group))
            .then_some(drawn)
    });

    for id in [
        "ask-a1b",
        "port-importer-b2c",
        "fix-login-c3d",
        "old-job-d4e",
    ] {
        assert!(drawn.contains(id), "{id} is missing from:\n{drawn}");
    }
    // A row says what the agent is up to: what it is asking, else what it is
    // doing, else what it answered.
    assert!(drawn.contains("Claude needs your permission"), "{drawn}");
    assert!(drawn.contains("Running Bash"), "{drawn}");
    assert!(drawn.contains("did what it was asked"), "{drawn}");

    // Twice over, in two vocabularies: the heading says what the group means,
    // and the counter at the top says the word the list can be narrowed by.
    for (group, counted) in [("needs input", "waiting"), ("completed", "done")] {
        assert_eq!(
            drawn.matches(group).count(),
            1,
            "{group} stands over its rows and nowhere else:\n{drawn}"
        );
        assert!(
            drawn.contains(&format!("1 {counted}")),
            "and the header counts it as {counted}:\n{drawn}"
        );
    }
}

#[test]
fn header_says_what_the_next_agent_will_be_started_with() {
    let amx = Harness::new();
    amx.config("agent = \"claude\"\nmax_agents = 3\n");
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    let drawn = amx.until("the header", || {
        let drawn = screen(&amx, &view);
        drawn.contains("worktree:").then_some(drawn)
    });

    assert!(
        drawn.contains(&format!("amx v{}", env!("CARGO_PKG_VERSION"))),
        "the build's own version:\n{drawn}"
    );
    assert!(
        drawn.contains("claude (default) · ~"),
        "the vendor, the model it will be given, and where it will run:\n{drawn}"
    );
    assert!(
        drawn.contains("1 waiting · 1/3 running"),
        "what the fleet is, and the gate the next one meets:\n{drawn}"
    );
    assert!(
        drawn.contains("worktree: on"),
        "and whether the next one is cut a tree of its own:\n{drawn}"
    );
}

#[test]
fn header_dials_turn_from_the_keys_and_leave_the_agents_alone() {
    let amx = Harness::new();
    amx.config("agent = \"claude\"\n");
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the header", || {
        screen(&amx, &view)
            .contains("claude (default)")
            .then_some(())
    });

    press(&amx, &view, "M-m");
    amx.until("the model dial to turn", || {
        screen(&amx, &view).contains("claude (fable)").then_some(())
    });

    press(&amx, &view, "M-w");
    let drawn = amx.until("the worktree dial to turn", || {
        let drawn = screen(&amx, &view);
        drawn.contains("worktree: off").then_some(drawn)
    });

    assert!(
        drawn.contains("ask-a1b"),
        "and the agent that was already running is untouched:\n{drawn}"
    );
    assert_eq!(
        amx.state("ask-a1b")["state"],
        "waiting",
        "a dial says what the next one will be, and nothing about this one"
    );
}

#[test]
fn glyphs_say_the_states_apart_and_the_working_one_breathes() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("port-importer-b2c", "works-with-a-spinner");
    amx.play("fix-login-c3d", "happy-turn");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("port-importer-b2c", "working");
    amx.until_state("fix-login-c3d", "idle");
    finished(&amx, "old-job-d4e", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    for (id, want) in [
        ("ask-a1b", '?'),
        ("fix-login-c3d", '○'),
        ("old-job-d4e", '●'),
    ] {
        amx.until(&format!("{id} to be marked {want}"), || {
            (mark(&amx, &view, id) == Some(want)).then_some(())
        });
    }

    // The working row is drawn a frame at a time, so watching it for a moment
    // shows more than one of them — and every one is the vendor's own, from the
    // set tmux hands its panes rather than the one ghostty asks for.
    let mut frames = std::collections::BTreeSet::new();
    amx.until("the working row to breathe", || {
        frames.extend(mark(&amx, &view, "port-importer-b2c"));
        (frames.len() > 1).then_some(())
    });
    for frame in &frames {
        assert!(
            "·✢*✶✻✽".contains(*frame),
            "{frame} is not a frame of the pulse: {frames:?}"
        );
    }
}

#[test]
fn ctrl_s_turns_the_axis_onto_the_project_each_agent_runs_in() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("fix-login-b2c", "happy-turn");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("fix-login-b2c", "idle");
    // One in the repository itself and one in a subdirectory of it, which is
    // the case the walk up the ancestors exists for. The third stays where the
    // harness put it, outside any repository at all.
    running_in(&amx, "ask-a1b", &repo);
    running_in(&amx, "fix-login-b2c", &repo.join("src"));
    finished(&amx, "old-job-c3d", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the agents", || {
        screen(&amx, &view).contains("needs input").then_some(())
    });

    press(&amx, &view, "C-s");
    let drawn = amx.until("the project headings", || {
        let drawn = screen(&amx, &view);
        drawn.contains("~/repo").then_some(drawn)
    });

    let row = |id: &str| {
        drawn
            .lines()
            .find(|line| line.contains(id))
            .unwrap_or_else(|| panic!("no row for {id} in:\n{drawn}"))
            .to_string()
    };
    let at = |text: &str| {
        drawn
            .lines()
            .position(|line| line.trim_end() == text)
            .unwrap_or_else(|| panic!("no {text} heading in:\n{drawn}"))
    };
    assert!(
        at("~/repo") < at("~"),
        "the project with the question in it comes first:\n{drawn}"
    );
    assert!(
        drawn
            .lines()
            .filter(|line| line.trim_end() == "~/repo")
            .count()
            == 1,
        "one heading for the repository, subdirectory and all:\n{drawn}"
    );
    // The heading no longer says the state, so every row carries it.
    assert!(row("ask-a1b").contains("waiting"), "{drawn}");
    assert!(row("fix-login-b2c").contains("idle"), "{drawn}");
    assert!(row("old-job-c3d").contains("done"), "{drawn}");
    assert!(
        !drawn.lines().any(|line| line.trim_end() == "needs input"),
        "and the state headings are gone with the axis:\n{drawn}"
    );

    // And back, on the same key.
    press(&amx, &view, "C-s");
    amx.until("what they need again", || {
        screen(&amx, &view).contains("needs input").then_some(())
    });
}

#[test]
fn a_filter_line_narrows_the_axis_instead_of_starting_an_agent() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("fix-login-b2c", "happy-turn");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("fix-login-b2c", "idle");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both agents", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("ask-a1b") && drawn.contains("fix-login-b2c")).then_some(())
    });

    // The same line a task is typed on, which is where somebody's hands
    // already are.
    types(&amx, &view, "n");
    types(&amx, &view, "s:waiting");
    amx.until(
        "the line to say it will narrow rather than start anything",
        || screen(&amx, &view).contains("narrow ▸").then_some(()),
    );
    press(&amx, &view, "Enter");

    // The line goes in the same frame the narrowing lands in, and it goes
    // last: waiting on the words alone would match the screen that is already
    // there, where they are still on the line somebody typed them on.
    let drawn = amx.until("the narrowed list", || {
        let drawn = screen(&amx, &view);
        (!drawn.contains("narrow ▸") && drawn.contains("s:waiting")).then_some(drawn)
    });
    assert!(drawn.contains("ask-a1b"), "{drawn}");
    assert!(
        !drawn.contains("fix-login-b2c"),
        "the rest of the fleet is held back:\n{drawn}"
    );
    assert_eq!(
        agents(&amx).len(),
        2,
        "and nothing was started with the line"
    );

    // A token with nothing after it gives them back.
    types(&amx, &view, "n");
    types(&amx, &view, "s:");
    press(&amx, &view, "Enter");
    amx.until("the whole fleet again", || {
        screen(&amx, &view).contains("fix-login-b2c").then_some(())
    });
}

#[test]
fn a_wall_with_nothing_on_it_gets_the_headings_and_what_lands_under_each() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);

    let drawn = screen(&amx, &view);
    for (group, said) in [
        ("needs input", "stopped on a question"),
        ("working", "starting or mid-turn"),
        ("idle", "amx cannot account for"),
        ("completed", "here to read"),
    ] {
        let at = drawn
            .lines()
            .position(|line| line.trim_end() == group)
            .unwrap_or_else(|| panic!("no {group} heading in:\n{drawn}"));
        assert!(
            drawn.lines().nth(at + 1).is_some_and(|l| l.contains(said)),
            "{group} has nothing under it saying what lands there:\n{drawn}"
        );
    }
    assert!(
        !drawn.contains("no agents"),
        "the four of them are said instead of the one sentence:\n{drawn}"
    );

    // And they go the moment there is anything to read off a row.
    amx.play("ask-a1b", "asks-a-question");
    amx.until("the agent's own row", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("ask-a1b") && !drawn.contains("here to read")).then_some(())
    });
}

#[test]
fn completed_agents_fold_into_a_count_until_they_are_opened() {
    let amx = Harness::new();
    for (n, id) in ["one-a1b", "two-b2c", "three-c3d", "four-d4e", "five-e5f"]
        .iter()
        .enumerate()
    {
        finished(&amx, id, "done", n as u64 * 60);
    }

    let view = amx.in_a_terminal(&[], &[]);
    let folded = amx.until("the fold", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("one-a1b") && drawn.contains("2 more")).then_some(drawn)
    });
    assert!(
        !folded.contains("five-e5f"),
        "the oldest are behind the count:\n{folded}"
    );

    // Down onto the fold — three agents are shown, so it is the fourth row —
    // and open it.
    amx.tmux(&["send-keys", "-t", &view, "Down", "Down", "Down", "Enter"]);
    amx.until("the rest of them", || {
        screen(&amx, &view).contains("five-e5f").then_some(())
    });
}

#[test]
fn the_cursor_is_a_bar_over_rows_and_headings_alike() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("ask-a1b").then_some(())
    });

    // The bar is a background colour, so it is in the escapes rather than in
    // the text: 55,55,55, which is the vendor's own for a selected line.
    const BAR: &str = "48;2;55;55;55";
    amx.until("the bar under the cursor", || {
        coloured_line(&amx, &view, "ask-a1b")
            .contains(BAR)
            .then_some(())
    });
    assert!(
        !coloured_line(&amx, &view, "needs input").contains(BAR),
        "and not under the heading the cursor is not on"
    );

    press(&amx, &view, "Up");
    amx.until("the bar to move up onto the heading", || {
        coloured_line(&amx, &view, "needs input")
            .contains(BAR)
            .then_some(())
    });
    assert!(
        !coloured_line(&amx, &view, "ask-a1b").contains(BAR),
        "one line at a time, whatever kind of line it is"
    );
}

#[test]
fn enter_shuts_the_group_its_headings_stand_over_and_opens_it_again() {
    let amx = Harness::new();
    finished(&amx, "one-a1b", "done", 60);
    finished(&amx, "two-b2c", "failed", 120);

    let view = amx.in_a_terminal(&[], &[]);
    let drawn = amx.until("both agents", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("one-a1b") && drawn.contains("two-b2c")).then_some(drawn)
    });
    assert!(
        drawn.contains("completed · 1 failed"),
        "a heading says how many failed with the rows still under it:\n{drawn}"
    );

    // Up off the first agent onto the heading over it, and shut the group.
    press(&amx, &view, "Up");
    press(&amx, &view, "Enter");
    let shut = amx.until("the group to be put away", || {
        let drawn = screen(&amx, &view);
        drawn.contains("completed 2 · 1 failed").then_some(drawn)
    });
    assert!(
        !shut.contains("one-a1b") && !shut.contains("two-b2c"),
        "the rows are behind the count now:\n{shut}"
    );

    press(&amx, &view, "Enter");
    amx.until("the agents back", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("one-a1b") && !drawn.contains("completed 2")).then_some(())
    });
}

#[test]
fn card_floats_over_the_list_with_the_question_and_the_pane() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("ask-a1b").then_some(())
    });

    press(&amx, &view, "Space");
    let carded = amx.until("the card", || {
        let drawn = screen(&amx, &view);
        drawn.contains("rm -rf build").then_some(drawn)
    });

    // The screen the agent is sitting on, and the question it is sitting on it
    // for — which is on its row as well, so the card is the second of the two.
    assert!(carded.contains("Do you want to proceed?"), "{carded}");
    assert_eq!(
        carded.matches("Claude needs your permission").count(),
        2,
        "the row asks it and the card asks it again:\n{carded}"
    );

    // A box, with the list it was opened from still drawn above it.
    let top = carded
        .lines()
        .find(|line| line.trim_start().starts_with('╭'))
        .unwrap_or_else(|| panic!("no card in:\n{carded}"));
    assert!(top.contains("ask-a1b · waiting"), "{top}");
    assert!(top.trim_end().ends_with('╮'), "{top}");
    assert!(
        carded.lines().any(|line| line.trim_end().ends_with('╯')),
        "{carded}"
    );
    assert!(
        carded.lines().next().unwrap_or_default().contains("amx v"),
        "the header is where it was:\n{carded}"
    );
    assert!(
        carded.contains("needs input"),
        "and so is the group the row is under:\n{carded}"
    );

    // Esc puts it away and leaves the wall as it was.
    press(&amx, &view, "Escape");
    amx.until("the card to go", || {
        (!screen(&amx, &view).contains("rm -rf build")).then_some(())
    });
}

#[test]
fn card_tail_cuts_the_chrome_claude_draws_under_its_pane() {
    let amx = Harness::new();
    let mut pane_rows = vec![
        "i ported the importer",
        "",
        "✻ Nesting… (15s · still thinking)",
        "",
    ];
    pane_rows.extend_from_slice(&CHROME);
    let pane = a_pane_showing(&amx, &pane_rows);
    amx.record("port-cli-b2c", &pane);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("port-cli-b2c").then_some(())
    });

    press(&amx, &view, "Space");
    let carded = amx.until("the card", || {
        let drawn = screen(&amx, &view);
        drawn.contains("i ported the importer").then_some(drawn)
    });
    for furniture in [
        "accept edits on",
        "execute amx-v2",
        "amx-main (main)",
        "still thinking",
    ] {
        assert!(
            !carded.contains(furniture),
            "{furniture} is claude's, not the agent's:\n{carded}"
        );
    }
}

#[test]
fn card_tail_cuts_a_composer_a_message_was_typed_into() {
    let amx = Harness::new();
    // Wrapped over three rows, because a composer holding one row of text is
    // the state a walk that cut exactly one input row would pass.
    let mut pane_rows = vec!["i ported the importer", CHROME[0]];
    pane_rows.extend_from_slice(&[
        "❯ and now check every call site that",
        "  used to take the old shape, then",
        "  run the suite",
    ]);
    pane_rows.extend_from_slice(&CHROME[2..]);
    let pane = a_pane_showing(&amx, &pane_rows);
    amx.record("port-cli-b2c", &pane);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("port-cli-b2c").then_some(())
    });

    press(&amx, &view, "Space");
    let carded = amx.until("the card", || {
        let drawn = screen(&amx, &view);
        drawn.contains("i ported the importer").then_some(drawn)
    });
    for staged in ["check every call site", "the old shape", "run the suite"] {
        assert!(
            !carded.contains(staged),
            "a line somebody is part way through typing is not the tail:\n{carded}"
        );
    }
    assert!(!carded.contains("accept edits on"), "{carded}");
}

#[test]
fn card_tail_leaves_a_prompt_whole_where_the_vendor_drew_no_footer() {
    let amx = Harness::new();
    let pane = a_pane_showing(
        &amx,
        &[
            "─────────────────────────────────────────────",
            " Bash command",
            "   rm -rf build",
            " Permission rule Bash requires confirmation.",
            " Do you want to proceed?",
            " ❯ 1. Yes",
            "   2. No",
        ],
    );
    amx.record("port-cli-b2c", &pane);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("port-cli-b2c").then_some(())
    });

    press(&amx, &view, "Space");
    let carded = amx.until("the card", || {
        let drawn = screen(&amx, &view);
        drawn.contains("rm -rf build").then_some(drawn)
    });
    // Every row of it, options included: a screen with a real prompt on it does
    // not end in a footer, and cutting one that has none would take the answer
    // the card was opened to give.
    assert!(carded.contains("2. No"), "{carded}");
}

#[test]
fn enter_puts_the_agent_in_front_of_the_terminal() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    let session = amx.tmux(&["display-message", "-p", "-t", &view, "#{session_id}"]);

    // An agent in a window beside the view's, which nobody is looking at.
    let pane = amx.tmux(&[
        "new-window",
        "-d",
        "-t",
        &session,
        "-P",
        "-F",
        "#{pane_id}",
        "--",
        "sh",
        "-c",
        "printf 'the agent at work\\n'; while :; do sleep 0.05; done",
    ]);
    amx.record("fix-login-a1b", &pane);
    amx.until("the row", || {
        screen(&amx, &view).contains("fix-login-a1b").then_some(())
    });

    amx.tmux(&["send-keys", "-t", &view, "Enter"]);
    amx.until("the agent's window to come forward", || {
        let active = amx.tmux(&["display-message", "-p", "-t", &pane, "#{window_active}"]);
        (active == "1").then_some(())
    });
    assert_eq!(
        amx.tmux(&["display-message", "-p", "-t", &pane, "#{pane_active}"]),
        "1",
        "and the agent's own pane within it"
    );
    assert!(
        amx.pane_alive(&view),
        "the view is still there to come back to"
    );
}

#[test]
fn the_composer_starts_an_agent_where_the_view_is() {
    let amx = Harness::new();
    let view = a_view_that_dispatches(&amx, "happy-turn");

    types(&amx, &view, "n");
    types(&amx, &view, "port the importer");
    amx.until("the task on the screen", || {
        screen(&amx, &view)
            .contains("port the importer")
            .then_some(())
    });
    press(&amx, &view, "Enter");

    let id = composed(&amx);
    assert!(id.starts_with("port-the-importer"), "{id}");

    let meta = amx.meta(&id);
    assert_eq!(meta["task"], "port the importer");
    assert_eq!(
        meta["dir"],
        amx.home().to_string_lossy().as_ref(),
        "an agent starts where the view was opened"
    );

    let pane = meta["pane"].as_str().expect("a pane").to_string();
    assert_eq!(
        pane_field(&amx, &pane, "#{window_name}"),
        "amx-wall",
        "and tiles into the wall of the session the view is in"
    );

    // It is a real agent: it runs, and it appears in the list the composer was
    // opened from.
    amx.until_state(&id, "idle");
    amx.until("the agent's own row", || {
        screen(&amx, &view).contains(&id).then_some(())
    });
}

#[test]
fn the_composer_takes_a_paste_as_one_edit_and_grows_to_its_cap() {
    let amx = Harness::new();
    let view = a_view_that_dispatches(&amx, "happy-turn");

    // Pasted at the list, where without bracketing every line of it would be
    // read as the keys it is made of and the first newline would dispatch.
    let pasted = format!("{}\n", twenty_rows());
    pastes(&amx, &view, &pasted);

    let drawn = amx.until("the pasted task", || {
        let drawn = screen(&amx, &view);
        drawn.contains("row-20").then_some(drawn)
    });
    assert!(
        agents(&amx).is_empty(),
        "a paste is one edit, its own last newline included:\n{drawn}"
    );

    // Ten rows, or a third of the terminal where that is less. The paste's own
    // last newline leaves an empty row at the bottom, where the cursor is, so
    // the line at the top of the composer is one further back than the count.
    let height: usize = pane_field(&amx, &view, "#{pane_height}")
        .parse()
        .expect("a pane height");
    let cap = 10.min(height / 3);
    let top = format!("task ▸ row-{:02}", 22 - cap);
    assert!(
        drawn.contains(&top),
        "the composer stops at {cap} rows and scrolls to {top}:\n{drawn}"
    );
    assert!(
        !drawn.contains("row-01"),
        "and what scrolled past is off the screen:\n{drawn}"
    );

    // And the enter afterwards is what dispatches, once, with the whole of it.
    press(&amx, &view, "Enter");
    let id = composed(&amx);
    assert_eq!(
        amx.meta(&id)["task"].as_str(),
        Some(pasted.as_str()),
        "one task, with every line of the paste in it"
    );
}

#[test]
fn an_agent_can_be_composed_out_of_sight() {
    let amx = Harness::new();
    let view = a_view_that_dispatches(&amx, "happy-turn");

    types(&amx, &view, "n");
    types(&amx, &view, "watch the log");
    press(&amx, &view, "Tab");
    amx.until("the composer to say where it is going", || {
        screen(&amx, &view).contains("out of sight").then_some(())
    });
    press(&amx, &view, "Enter");

    let id = composed(&amx);
    let pane = amx.meta(&id)["pane"].as_str().expect("a pane").to_string();
    assert_ne!(
        pane_field(&amx, &pane, "#{window_name}"),
        "amx-wall",
        "an agent started out of sight is not on the wall"
    );
    assert_eq!(
        pane_field(&amx, &pane, "#{session_name}"),
        format!("amx-{id}"),
        "it has a session nobody is attached to"
    );
    amx.until_state(&id, "idle");
}

#[test]
fn the_composer_turns_the_dials_for_the_one_spawn_its_tokens_lead() {
    let amx = Harness::new();
    a_repo_at(amx.home());
    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");

    types(&amx, &view, "n");
    types(&amx, &view, "m:opus p:plan w:on port the importer");
    press(&amx, &view, "Enter");

    let id = composed(&amx);
    let command = command_of(&amx, &id);
    assert!(
        command.windows(2).any(|pair| pair == ["--model", "opus"])
            && command
                .windows(2)
                .any(|pair| pair == ["--permission-mode", "plan"]),
        "{command:?}"
    );
    assert_eq!(
        command.last().map(String::as_str),
        Some("port the importer"),
        "and the tokens are off the task the vendor is handed: {command:?}"
    );
    assert!(
        amx.meta(&id)["worktree"].is_string(),
        "w:on out-votes a config that turned worktrees off"
    );

    // One spawn and no other: the next line with no tokens on it is the
    // config's answer again, every dial of it.
    types(&amx, &view, "n");
    types(&amx, &view, "fix the login bug");
    press(&amx, &view, "Enter");

    let next = composed_after(&amx, &id);
    let command = command_of(&amx, &next);
    assert!(
        !command.iter().any(|arg| arg.starts_with("--")),
        "a token turns a dial for the line it was typed on: {command:?}"
    );
    assert!(amx.meta(&next)["worktree"].is_null(), "and no other");
}

#[test]
fn header_puts_what_the_next_agent_may_do_under_the_line_that_starts_it() {
    let amx = Harness::new();
    amx.config("agent = \"claude\"\n");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the header", || {
        screen(&amx, &view)
            .contains("claude (default)")
            .then_some(())
    });

    press(&amx, &view, "n");
    amx.until("the permission row", || {
        screen(&amx, &view)
            .contains("permission: vendor default (shift+tab to cycle)")
            .then_some(())
    });

    press(&amx, &view, "BTab");
    let drawn = amx.until("the permission dial to turn", || {
        let drawn = screen(&amx, &view);
        drawn.contains("⏵⏵ acceptEdits").then_some(drawn)
    });
    assert!(
        drawn.contains("task ▸"),
        "and the line it is under is still there to type into:\n{drawn}"
    );
}

#[test]
fn header_dials_are_the_argv_the_next_agent_is_started_with() {
    let amx = Harness::new();
    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");

    press(&amx, &view, "M-m");
    amx.until("the model dial to turn", || {
        screen(&amx, &view).contains("claude (fable)").then_some(())
    });

    types(&amx, &view, "n");
    types(&amx, &view, "port the importer");
    press(&amx, &view, "Enter");

    let id = composed(&amx);
    let command = command_of(&amx, &id);
    assert!(
        command.windows(2).any(|pair| pair == ["--model", "fable"]),
        "what the header says the next agent will be is what it is: {command:?}"
    );

    // A token on the line is about the one spawn it leads, so it beats the
    // dial the view is holding rather than turning it.
    types(&amx, &view, "n");
    types(&amx, &view, "m:opus fix the login bug");
    press(&amx, &view, "Enter");

    let next = composed_after(&amx, &id);
    assert!(
        command_of(&amx, &next)
            .windows(2)
            .any(|pair| pair == ["--model", "opus"]),
        "{:?}",
        command_of(&amx, &next)
    );
    amx.until("the header to be as it was", || {
        screen(&amx, &view).contains("claude (fable)").then_some(())
    });
}

#[test]
fn header_vendor_dial_runs_the_next_agent_under_the_vendor_it_names() {
    let amx = Harness::new();
    // The file names a command amx has no entry for, and claude is on the
    // path beside it: the vendor dial is the only thing on this screen that
    // could reach the second one.
    let view = a_view_that_can_start_claude(
        &amx,
        &format!("agent = \"{}\"\nworktrees = false\n", amx.mock()),
    );
    let drawn = amx.until("the header", || {
        let drawn = screen(&amx, &view);
        drawn.contains("worktree: off").then_some(drawn)
    });
    assert!(
        !drawn.contains("claude (default)"),
        "an unregistered command declares no model dial, so there is nothing \
         to put in parentheses:\n{drawn}"
    );

    press(&amx, &view, "M-v");
    amx.until("the vendor dial to turn", || {
        screen(&amx, &view)
            .contains("claude (default)")
            .then_some(())
    });

    types(&amx, &view, "n");
    types(&amx, &view, "port the importer");
    press(&amx, &view, "Enter");

    let id = composed(&amx);
    let command = command_of(&amx, &id);
    assert_eq!(
        command.first().map(String::as_str),
        Some("claude"),
        "the vendor the header names is the program the agent runs: {command:?}"
    );
    amx.until_state(&id, "idle");
}

#[test]
fn header_worktree_dial_gives_the_next_agent_a_tree_the_file_would_not() {
    let amx = Harness::new();
    a_repo_at(amx.home());
    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");

    press(&amx, &view, "M-w");
    amx.until("the worktree dial to turn", || {
        screen(&amx, &view).contains("worktree: on").then_some(())
    });

    types(&amx, &view, "n");
    types(&amx, &view, "port the importer");
    press(&amx, &view, "Enter");

    let id = composed(&amx);
    assert!(
        amx.meta(&id)["worktree"].is_string(),
        "the dial is what this view spawns at, whatever the file says: {:?}",
        amx.meta(&id)
    );
}

#[test]
fn the_composer_keeps_a_line_the_vendor_would_refuse_and_says_what_it_takes() {
    let amx = Harness::new();
    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");

    types(&amx, &view, "n");
    types(&amx, &view, "p:nonsense port the importer");
    press(&amx, &view, "Enter");

    let drawn = amx.until("the refusal", || {
        let drawn = screen(&amx, &view);
        drawn.contains("claude takes").then_some(drawn)
    });
    assert!(
        drawn.contains("acceptEdits"),
        "and the modes it does take:\n{drawn}"
    );
    assert!(
        drawn.contains("task ▸ p:nonsense port the importer"),
        "the line is still there to be fixed:\n{drawn}"
    );
    assert!(agents(&amx).is_empty(), "and nothing was made:\n{drawn}");
}

#[test]
fn card_answers_the_question_the_agent_stopped_on() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("ask-a1b").then_some(())
    });

    // One key: the card is where somebody decides what to answer and where
    // they type it.
    press(&amx, &view, "Space");
    amx.until("the card, with a line to answer on", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("rm -rf build") && drawn.contains('❯')).then_some(())
    });

    // A key of the grammar that is nowhere on the agent's own screen, so what
    // reaches the pane is what was typed here and not what was already drawn.
    types(&amx, &view, "9");
    amx.until("the key on the line", || {
        screen(&amx, &view).contains("❯ 9").then_some(())
    });
    press(&amx, &view, "Enter");

    amx.until("the key to reach the agent's pane", || {
        amx.capture(&amx.pane_of("ask-a1b"))
            .contains('9')
            .then_some(())
    });
    amx.until("the question to stop being pending", || {
        (amx.state("ask-a1b")["state"] != "waiting").then_some(())
    });
}

/// Park an agent on a menu of the vendor's own: the one question that takes
/// words as well as a key, with the choices it drew recorded beside it.
fn parked_on_a_menu(amx: &Harness, id: &str) {
    amx.play(id, "works-without-end");
    amx.until_state(id, "working");
    amx.set_state(
        id,
        json!({
            "state": "waiting",
            "question": {
                "text": "Which fixture should the port keep?",
                "options": ["the sqlite one", "the docker one"],
                "kind": "question",
            },
            "since": now(),
            "last_event": now(),
        }),
    );
}

#[test]
fn card_takes_words_where_the_question_asks_for_them() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);
    parked_on_a_menu(&amx, "pick-a1b");
    amx.until("the question on its row", || {
        screen(&amx, &view)
            .contains("Which fixture should the port keep?")
            .then_some(())
    });

    press(&amx, &view, "Space");
    let carded = amx.until("the card, with the choices numbered", || {
        let drawn = screen(&amx, &view);
        drawn.contains("1. the sqlite one").then_some(drawn)
    });
    assert!(carded.contains("2. the docker one"), "{carded}");
    assert_eq!(
        carded
            .matches("Which fixture should the port keep?")
            .count(),
        2,
        "the row asks it and the card asks it again:\n{carded}"
    );

    // Words of somebody's own, which is what this one question takes.
    types(&amx, &view, "neither, keep both");
    amx.until("the answer on the line", || {
        screen(&amx, &view)
            .contains("❯ neither, keep both")
            .then_some(())
    });
    press(&amx, &view, "Enter");

    amx.until("the words to reach the agent's pane", || {
        amx.capture(&amx.pane_of("pick-a1b"))
            .contains("neither, keep both")
            .then_some(())
    });
    amx.until("the question to be answered rather than repeated", || {
        (amx.state("pick-a1b")["question"] == json!(null)).then_some(())
    });
}

#[test]
fn card_refuses_words_at_a_prompt_that_reads_one_key() {
    let amx = Harness::new();
    let pane = amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("ask-a1b").then_some(())
    });

    press(&amx, &view, "Space");
    amx.until("the card", || {
        screen(&amx, &view).contains("rm -rf build").then_some(())
    });
    types(&amx, &view, "neither, keep both");
    press(&amx, &view, "Enter");

    let refused = amx.until("the refusal", || {
        let drawn = screen(&amx, &view);
        drawn.contains("ask-a1b is asking").then_some(drawn)
    });
    assert!(
        refused.contains("❯ neither, keep both"),
        "a line the question would not take is a line somebody is still \
         writing:\n{refused}"
    );
    assert!(
        !amx.capture(&pane).contains("neither, keep both"),
        "and a permission box reads one key, so words typed at it would \
         answer it by accident"
    );
    assert_eq!(
        amx.state("ask-a1b")["state"],
        "waiting",
        "with the question still pending"
    );
}

#[test]
fn a_reply_to_an_agent_between_turns_is_a_message() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "takes-a-message");
    amx.until_state("fix-login-a1b", "idle");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("fix-login-a1b").then_some(())
    });

    types(&amx, &view, "r");
    amx.until("the line to be addressed to the agent", || {
        screen(&amx, &view)
            .contains("message to fix-login-a1b")
            .then_some(())
    });
    types(&amx, &view, "and now the linter");
    press(&amx, &view, "Enter");

    amx.until("the message to reach the agent's pane", || {
        amx.capture(&amx.pane_of("fix-login-a1b"))
            .contains("and now the linter")
            .then_some(())
    });
    assert!(
        amx.state("fix-login-a1b")["seq"].as_u64().unwrap_or(0) > 0,
        "and the send is on the record, the way the verb records one"
    );
}

#[test]
fn ctrl_x_stops_the_agent_and_then_forgets_it() {
    let amx = Harness::new();
    let pane = amx.play("watch-log-e5f", "works-without-end");
    amx.until_state("watch-log-e5f", "working");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("watch-log-e5f").then_some(())
    });

    press(&amx, &view, "C-x");
    amx.until("the agent to stop", || {
        (amx.state("watch-log-e5f")["state"] == "stopped").then_some(())
    });
    // The record says stopped before the signal is sent, so that the exit it
    // causes reads as a stop rather than a failure. The pane going is the
    // stopping itself, and it happens a moment after.
    amx.until("its pane to go with it", || {
        (!amx.pane_alive(&pane)).then_some(())
    });

    // Again on the same row: an agent that has already ended is forgotten.
    press(&amx, &view, "C-x");
    amx.until("the record to go", || agents(&amx).is_empty().then_some(()));
    until_empty(&amx, &view);
}

#[test]
fn d_shows_what_the_agent_has_changed() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let out = amx
        .amx_command(&[
            "new",
            "--name",
            "fix-login-a1b",
            "--dir",
            &repo.to_string_lossy(),
            "--agent",
            &amx.mock(),
            "fix the login bug",
        ])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("works-without-end"))
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let tree = PathBuf::from(
        amx.meta("fix-login-a1b")["worktree"]
            .as_str()
            .expect("a worktree"),
    );
    std::fs::write(tree.join("README.md"), "after\n").expect("the changed file");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("fix-login-a1b").then_some(())
    });

    types(&amx, &view, "d");
    let shown = amx.until("the diff", || {
        let drawn = screen(&amx, &view);
        drawn.contains("+after").then_some(drawn)
    });
    assert!(shown.contains("-before"), "{shown}");
    assert!(
        shown.contains("what it has changed"),
        "and the panel says what it is showing: {shown}"
    );
}

#[test]
fn the_keys_are_on_the_screen_for_the_asking() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);

    types(&amx, &view, "?");
    let keys = amx.until("the keys", || {
        let drawn = screen(&amx, &view);
        drawn.contains("ctrl+x").then_some(drawn)
    });
    for does in [
        "start an agent",
        "reply",
        "what it has changed",
        "stop it",
        "close the view",
    ] {
        assert!(keys.contains(does), "{does} is not among the keys:\n{keys}");
    }

    // And back to the agents, which is what the view is for.
    press(&amx, &view, "Escape");
    until_empty(&amx, &view);
}

#[test]
fn q_closes_the_view_and_gives_the_screen_back() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);

    // Keep the pane after its command ends, so what it ended with can be read.
    amx.tmux(&["set-option", "-w", "-t", &view, "remain-on-exit", "on"]);
    amx.tmux(&["send-keys", "-t", &view, "q"]);

    amx.until("the view to close", || {
        let dead = amx.tmux(&["display-message", "-p", "-t", &view, "#{pane_dead}"]);
        (dead == "1").then_some(())
    });
    assert_eq!(
        amx.tmux(&["display-message", "-p", "-t", &view, "#{pane_dead_status}"]),
        "0",
        "closing a view is not a failure"
    );
    assert!(
        !screen(&amx, &view).contains("here to read"),
        "the screen the view borrowed is handed back"
    );
}
