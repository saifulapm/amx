//! End-to-end tests for the wall view: which agents it lists, how it groups
//! and orders them, and what keys and clicks on a row do.
//!
//! Each test runs the view in a real tmux pane and reads the screen back with
//! `capture-pane`, since drawing, key handling and cleanup on exit need a real
//! terminal.

mod common;

use common::{
    Harness, a_merged_request, a_pane_showing, agents, an_ended_agent, bar, card_on, click,
    clients_on, coloured, coloured_line, finished, foreground, git, merged_by_hand, mouse, now,
    pane_field, press, rgb, sgr_at, text_in, types, until_empty, watching, work_on_the_branch,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Give agent `id` the branch `amx/<id>` and a pr.json with one request,
/// `number`, in `standing`.
///
/// `asked` is now, so the view trusts the cache and never queries the forge.
fn a_request(amx: &Harness, id: &str, number: u64, standing: &str) {
    let branch = format!("amx/{id}");
    amx.set_meta(id, json!({ "branch": branch }));
    let written = json!({
        "asked": now(),
        "branch": branch,
        "prs": [{ "number": number, "standing": standing }],
    });
    std::fs::write(
        amx.agent_dir(id).join("pr.json"),
        written.to_string().as_bytes(),
    )
    .expect("what the last look wrote");
}

/// The 0-based index of the first line of `drawn` that contains `text`.
fn line_of(drawn: &str, text: &str) -> usize {
    drawn
        .lines()
        .position(|line| line.contains(text))
        .unwrap_or_else(|| panic!("no line holding {text} in:\n{drawn}"))
}

/// The status glyph on the agent's row now, after the row's one-cell indent.
fn mark(amx: &Harness, view: &str, id: &str) -> Option<char> {
    row_of(amx, view, id)?.chars().nth(1)
}

/// Mark the agent as seen now, as opening its card does.
///
/// The wall draws nothing for it; it only changes where the row sorts against
/// the Completed fold.
fn read(amx: &Harness, id: &str) {
    let mut state = amx.state(id);
    state["seen"] = json!(now());
    amx.set_state(id, state);
}

/// The first line of the view's screen that contains `id`.
fn row_of(amx: &Harness, view: &str, id: &str) -> Option<String> {
    amx.capture(view)
        .lines()
        .find(|line| line.contains(id))
        .map(str::to_string)
}

/// How many lines of `drawn` contain `prefix`, i.e. how many of a group's rows
/// the fold leaves visible.
fn rows_of(drawn: &str, prefix: &str) -> usize {
    drawn.lines().filter(|line| line.contains(prefix)).count()
}

/// Write theme `name` into the user's themes directory, where the config's
/// `theme` key looks it up.
fn theme(amx: &Harness, name: &str, text: &str) {
    let dir = amx.home().join(".config/amx/themes");
    std::fs::create_dir_all(&dir).expect("the themes directory");
    std::fs::write(dir.join(format!("{name}.toml")), text).expect("writing the theme");
}

/// Set the agent's `session_title` in state.json, as claude's end-of-turn hook
/// does.
fn titled(amx: &Harness, id: &str, title: &str) {
    let mut state = amx.state(id);
    state["session_title"] = json!(title);
    amx.set_state(id, state);
}

/// Set the agent's `name` in state.json, as renaming it from the wall does.
fn renamed(amx: &Harness, id: &str, name: &str) {
    let mut state = amx.state(id);
    state["name"] = json!(name);
    amx.set_state(id, state);
}

/// Set the agent's `dir` in meta.json, which decides its project.
fn running_in(amx: &Harness, id: &str, dir: &std::path::Path) {
    let path = amx.agent_dir(id).join("meta.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("the record")).expect("the record");
    meta["dir"] = json!(dir);
    std::fs::write(&path, serde_json::to_vec(&meta).expect("the record")).expect("the record");
}

/// Send `key` twice in one tmux command, so the second press lands within any
/// time window the view opens after the first.
fn twice(amx: &Harness, view: &str, key: &str) {
    amx.tmux(&["send-keys", "-t", view, key, key]);
}

/// The 1-based screen row of the agent's row, for mouse events.
fn screen_row_of(amx: &Harness, view: &str, id: &str) -> u16 {
    let drawn = amx.capture(view);
    let at = drawn
        .lines()
        .position(|line| line.contains(id))
        .unwrap_or_else(|| panic!("no row for {id} in:\n{drawn}"));
    at as u16 + 1
}

/// The five rows claude draws at the bottom of its pane, top to bottom: the
/// composer's top border with its right-aligned label, the composer line, the
/// bottom border, the statusline and the mode footer. Copied from claude
/// 2.1.237.
const CHROME: [&str; 5] = [
    "──────────────────────────── execute amx-v2 ─",
    "❯ ",
    "─────────────────────────────────────────────",
    "  Opus 5 │ amx-main (main) │ xhigh",
    "  ⏵⏵ accept edits on (shift+tab to cycle)",
];

/// Run bare amx in a pane with `TMUX` and `TMUX_PANE` unset, which is how amx
/// decides it is outside tmux.
fn outside_tmux(amx: &Harness) -> String {
    amx.in_a_terminal(&[("TMUX", ""), ("TMUX_PANE", "")], &[])
}

/// The names of the sessions on this harness's server.
fn sessions(amx: &Harness) -> Vec<String> {
    amx.tmux(&["list-sessions", "-F", "#{session_name}"])
        .lines()
        .map(str::to_string)
        .collect()
}

/// Record agent `id` in a detached `amx-<id>` session, as amx names its own,
/// showing "the agent at work".
fn an_agent_session(amx: &Harness, id: &str) -> String {
    let pane = amx.tmux(&[
        "new-session",
        "-d",
        "-s",
        &format!("amx-{id}"),
        "-P",
        "-F",
        "#{pane_id}",
        "--",
        "sh",
        "-c",
        "printf 'the agent at work\\n'; while :; do sleep 0.05; done",
    ]);
    amx.record(id, &pane);
    pane
}

#[test]
fn bare_amx_draws_the_list_in_the_terminal_it_was_typed_in() {
    let amx = Harness::new();

    // Once inside tmux and once in a terminal amx sees as outside it.
    let inside = amx.in_a_terminal(&[], &[]);
    let outside = outside_tmux(&amx);

    for view in [&inside, &outside] {
        until_empty(&amx, view);
        assert_eq!(
            pane_field(&amx, view, "#{pane_current_command}"),
            "amx",
            "the view is the terminal's own program, not a client attached to \
             one somewhere else"
        );
    }

    let named = sessions(&amx);
    assert!(
        !named.iter().any(|name| name == "amx"),
        "and nothing was built to draw it in: {named:?}"
    );
}

#[test]
fn the_view_gathers_the_agents_under_what_they_need() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("port-import-b2c", "works-with-a-spinner");
    amx.play("fix-login-c3d", "happy-turn");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("port-import-b2c", "working");
    amx.until_state("fix-login-c3d", "idle");
    finished(&amx, "old-job-d4e", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    let drawn = amx.until("every group", || {
        let drawn = amx.capture(&view);
        ["Needs input", "Working", "Completed"]
            .iter()
            .all(|group| drawn.contains(group))
            .then_some(drawn)
    });

    for id in ["ask-a1b", "port-import-b2c", "fix-login-c3d", "old-job-d4e"] {
        assert!(drawn.contains(id), "{id} is missing from:\n{drawn}");
    }
    // Idle and exited agents share Completed since both turns are over; only
    // the row shows whether the process is still alive.
    assert!(
        !drawn.contains("Idle"),
        "an ended turn is completed, so there is no group between:\n{drawn}"
    );
    assert!(
        line_of(&drawn, "Completed") < line_of(&drawn, "fix-login-c3d"),
        "the idle agent stands under Completed:\n{drawn}"
    );
    // A row's detail is the pending question, else the running tool, else the
    // last answer.
    assert!(drawn.contains("Claude needs your permission"), "{drawn}");
    assert!(drawn.contains("Running Bash"), "{drawn}");
    assert!(drawn.contains("did what it was asked"), "{drawn}");

    // Group names appear only as headings; the header counts use the filter
    // words (WAITING, done).
    for group in ["Needs input", "Completed"] {
        assert_eq!(
            drawn.matches(group).count(),
            1,
            "{group} stands over its rows and nowhere else:\n{drawn}"
        );
    }
    assert!(
        drawn.contains("1 WAITING"),
        "the one group that wants a person is counted in the badge:\n{drawn}"
    );
    assert!(
        drawn.contains("2 done"),
        "and the rest are counted beside it:\n{drawn}"
    );
}

/// Record an ended agent `id` as a depth-1 child of `parent`.
fn a_child(amx: &Harness, id: &str, parent: &str, ago: u64, created: u64) {
    finished(amx, id, "done", ago);
    amx.set_meta(
        id,
        json!({ "parent": parent, "depth": 1, "created": created }),
    );
}

#[test]
fn the_wall_draws_a_child_under_its_parent_on_a_connector() {
    let amx = Harness::new();
    finished(&amx, "parent-a1b", "done", 60);
    a_child(&amx, "scout-b2c", "parent-a1b", 30, 10);
    a_child(&amx, "review-c3d", "parent-a1b", 20, 20);

    let view = amx.in_a_terminal(&[], &[]);
    let drawn = amx.until("the family", || {
        let drawn = amx.capture(&view);
        (drawn.contains("scout-b2c") && drawn.contains("review-c3d")).then_some(drawn)
    });

    let parent = line_of(&drawn, "parent-a1b");
    let scout = line_of(&drawn, "scout-b2c");
    let review = line_of(&drawn, "review-c3d");
    assert!(
        parent < review && review < scout,
        "the parent comes first and its children under it, newest first:\n{drawn}"
    );

    let scout_line = row_of(&amx, &view, "scout-b2c").unwrap();
    let review_line = row_of(&amx, &view, "review-c3d").unwrap();
    let parent_line = row_of(&amx, &view, "parent-a1b").unwrap();
    assert!(
        review_line.contains("├─"),
        "the newest child opens the pair: {review_line:?}"
    );
    assert!(
        scout_line.contains("└─"),
        "and the oldest closes it: {scout_line:?}"
    );
    assert!(
        !parent_line.contains("├─"),
        "the parent wears no connector of its own"
    );

    // A root keeps the wall's base indent even with children, and a child's
    // connector starts in its parent's glyph column.
    let indent = |line: &str| line.len() - line.trim_start().len();
    assert_eq!(
        indent(&parent_line),
        1,
        "the root is padded nothing for its family: {parent_line:?}"
    );
    assert_eq!(
        indent(&review_line),
        indent(&parent_line),
        "and the child's connector starts under its parent's glyph: \
         {review_line:?} against {parent_line:?}"
    );

    // Counts include top-level agents only.
    assert!(
        drawn.contains("1 done"),
        "one top-level agent has ended:\n{drawn}"
    );
    assert!(!drawn.contains("3 done"), "{drawn}");
}

#[test]
fn ctrl_t_pins_the_row_under_the_cursor_over_every_group_and_lets_it_go() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("port-import-b2c", "works-with-a-spinner");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("port-import-b2c", "working");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the two groups", || {
        let drawn = amx.capture(&view);
        (drawn.contains("Needs input") && drawn.contains("Working")).then_some(())
    });

    // The cursor starts on the first row, and the Working heading takes one
    // step on the way down to port-import-b2c.
    press(&amx, &view, "Down");
    press(&amx, &view, "Down");
    press(&amx, &view, "C-t");
    let drawn = amx.until("the pinned group", || {
        let drawn = amx.capture(&view);
        drawn.contains("Pinned").then_some(drawn)
    });

    assert!(
        line_of(&drawn, "Pinned") < line_of(&drawn, "Needs input"),
        "what somebody pinned stands over the agent that is asking:\n{drawn}"
    );
    assert_eq!(
        line_of(&drawn, "port-import-b2c"),
        line_of(&drawn, "Pinned") + 1,
        "and it is the row under the heading, whatever it is doing:\n{drawn}"
    );
    assert!(
        !drawn.contains("Working"),
        "the group it came out of was the last of it:\n{drawn}"
    );

    press(&amx, &view, "C-t");
    let back = amx.until("the working group again", || {
        let drawn = amx.capture(&view);
        drawn.contains("Working").then_some(drawn)
    });
    assert!(!back.contains("Pinned"), "{back}");
    assert!(
        line_of(&back, "Working") < line_of(&back, "port-import-b2c"),
        "{back}"
    );
}

#[test]
fn z_puts_the_row_under_the_cursor_below_every_group_and_wakes_it_again() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("port-import-b2c", "works-with-a-spinner");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("port-import-b2c", "working");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the two groups", || {
        let drawn = amx.capture(&view);
        (drawn.contains("Needs input") && drawn.contains("Working")).then_some(())
    });

    // The cursor starts on ask-a1b, the waiting agent.
    press(&amx, &view, "z");
    let drawn = amx.until("the sleeping group", || {
        let drawn = amx.capture(&view);
        drawn.contains("Asleep").then_some(drawn)
    });

    assert!(
        line_of(&drawn, "Working") < line_of(&drawn, "Asleep"),
        "what somebody put away stands under the work that is running:\n{drawn}"
    );
    assert_eq!(
        line_of(&drawn, "ask-a1b"),
        line_of(&drawn, "Asleep") + 1,
        "and it is the row under the heading, though it is the one asking:\n{drawn}"
    );
    assert!(
        !drawn.contains("Needs input"),
        "the group it came out of was the last of it:\n{drawn}"
    );
    assert!(
        drawn.contains("1 WAITING"),
        "and the badge still counts it: where a row is drawn is no answer to \
         its question:\n{drawn}"
    );

    // The sleep mark is persisted, so a second view shows it too.
    let again = amx.in_a_terminal(&[], &[]);
    let opened = amx.until("the second view", || {
        let drawn = amx.capture(&again);
        (drawn.contains("Asleep") && drawn.contains("ask-a1b")).then_some(drawn)
    });
    assert!(
        line_of(&opened, "Asleep") < line_of(&opened, "ask-a1b"),
        "the next view opens on the wall the last one was left on:\n{opened}"
    );

    // The header's word for the group also works as a find filter.
    types(&amx, &view, "/");
    types(&amx, &view, "s:asleep");
    amx.until("the wall narrowed to the sleeping agent", || {
        let drawn = amx.capture(&view);
        (drawn.contains("ask-a1b") && !drawn.contains("port-import-b2c")).then_some(())
    });
    press(&amx, &view, "Escape");
    amx.until("the whole fleet again", || {
        amx.capture(&view).contains("port-import-b2c").then_some(())
    });

    // The sleeping row is last on the wall, so G reaches it.
    press(&amx, &view, "G");
    press(&amx, &view, "z");
    let back = amx.until("the asking group again", || {
        let drawn = amx.capture(&view);
        drawn.contains("Needs input").then_some(drawn)
    });
    assert!(!back.contains("Asleep"), "{back}");
    assert!(
        line_of(&back, "Needs input") < line_of(&back, "ask-a1b"),
        "{back}"
    );
}

#[test]
fn ready_for_review_takes_an_ended_agent_whose_request_is_still_open() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);
    a_request(&amx, "fix-login-a1b", 12, "open");
    finished(&amx, "port-import-b2c", "done", 30);
    a_request(&amx, "port-import-b2c", 9, "merged");
    finished(&amx, "old-job-c3d", "done", 90);

    let view = amx.in_a_terminal(&[], &[]);
    let drawn = amx.until("both groups", || {
        let drawn = amx.capture(&view);
        (drawn.contains("Ready for review") && drawn.contains("Completed")).then_some(drawn)
    });

    assert!(
        line_of(&drawn, "Ready for review") < line_of(&drawn, "Completed"),
        "work waiting on a reviewer stands over the work that is over:\n{drawn}"
    );
    assert_eq!(
        line_of(&drawn, "fix-login-a1b"),
        line_of(&drawn, "Ready for review") + 1,
        "the agent whose request is still asking for something:\n{drawn}"
    );
    assert!(
        row_of(&amx, &view, "fix-login-a1b").is_some_and(|row| row.contains("#12")),
        "with the number the review is happening under:\n{drawn}"
    );
    for id in ["port-import-b2c", "old-job-c3d"] {
        assert!(
            line_of(&drawn, id) > line_of(&drawn, "Completed"),
            "a merged request and a branch nobody opened one for are both \
             over, so {id} is completed:\n{drawn}"
        );
    }
}

#[test]
fn glyphs_say_a_live_agent_from_an_ended_one_and_the_working_one_breathes() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("port-import-b2c", "works-with-a-spinner");
    amx.play("fix-login-c3d", "happy-turn");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("port-import-b2c", "working");
    amx.until_state("fix-login-c3d", "idle");
    finished(&amx, "old-job-d4e", "done", 60);

    // ✻ while the process is alive and ∙ once it has gone, whatever the state.
    let view = amx.in_a_terminal(&[], &[]);
    for (id, want) in [
        ("ask-a1b", '✻'),
        ("fix-login-c3d", '✻'),
        ("old-job-d4e", '∙'),
    ] {
        amx.until(&format!("{id} to be marked {want}"), || {
            (mark(&amx, &view, id) == Some(want)).then_some(())
        });
    }

    // Only the colour tells the two live glyphs apart.
    let asking = coloured_line(&amx, &view, "ask-a1b");
    assert!(
        asking.contains(&foreground("waiting")),
        "the waiting glyph is painted for what the row wants:\n{asking:?}"
    );
    let resting = coloured_line(&amx, &view, "fix-login-c3d");
    assert!(
        resting.contains(&foreground("done")),
        "and the one whose turn is over is painted done, at its prompt or \
         not:\n{resting:?}"
    );

    // The working glyph animates, so polling sees several frames. Each must be
    // from claude's spinner set under tmux; claude uses another set in ghostty.
    let mut frames = std::collections::BTreeSet::new();
    amx.until("the working row to breathe", || {
        frames.extend(mark(&amx, &view, "port-import-b2c"));
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
fn glyphs_wear_a_dollar_on_the_rows_running_a_command() {
    let amx = Harness::new();
    // A shell row is a record with `agent: null`, as `!cmd` and
    // `amx new --exec` write. A live and an ended shell row, next to an agent
    // in each of those states.
    let pane = a_pane_showing(&amx, &["cargo build"]);
    amx.record("build-a1b", &pane);
    amx.set_meta("build-a1b", json!({ "agent": null }));
    finished(&amx, "sweep-b2c", "done", 30);
    amx.set_meta("sweep-b2c", json!({ "agent": null }));
    amx.play("port-import-c3d", "works-with-a-spinner");
    amx.until_state("port-import-c3d", "working");
    finished(&amx, "old-job-d4e", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    for id in ["build-a1b", "sweep-b2c"] {
        amx.until(&format!("{id} to be marked $"), || {
            (mark(&amx, &view, id) == Some('$')).then_some(())
        });
    }

    // Agent rows keep their usual glyphs.
    amx.until("old-job-d4e to rest on the dot", || {
        (mark(&amx, &view, "old-job-d4e") == Some('∙')).then_some(())
    });
    let mut frames = std::collections::BTreeSet::new();
    amx.until("the working agent to breathe", || {
        frames.extend(mark(&amx, &view, "port-import-c3d"));
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
fn keys_v_says_which_vendor_model_and_effort_each_row_runs() {
    let amx = Harness::new();
    // An agent with model and effort set, one with neither, and a shell row,
    // which has no vendor.
    finished(&amx, "fix-login-a1b", "done", 30);
    amx.set_meta(
        "fix-login-a1b",
        json!({ "model": "opus", "effort": "high" }),
    );
    finished(&amx, "port-b2c", "done", 60);
    let pane = a_pane_showing(&amx, &["cargo build"]);
    amx.record("build-c3d", &pane);
    amx.set_meta("build-c3d", json!({ "agent": null }));

    let ids = ["fix-login-a1b", "port-b2c", "build-c3d"];
    // All three rows or None. The view has no synchronized output, so a
    // capture can land mid-frame; a partial screen means poll again.
    let rows = |drawn: &str| -> Option<Vec<String>> {
        ids.iter()
            .map(|id| {
                drawn
                    .lines()
                    .find(|line| line.contains(id))
                    .map(str::to_string)
            })
            .collect()
    };
    let rows_of =
        |drawn: &str| rows(drawn).unwrap_or_else(|| panic!("the three rows in:\n{drawn}"));

    let view = amx.in_a_terminal(&[], &[]);
    let quiet = amx.until("the three rows", || {
        let drawn = amx.capture(&view);
        rows(&drawn).map(|_| drawn)
    });
    for row in rows_of(&quiet) {
        assert!(
            !row.contains("claude"),
            "the column is not on the wall until somebody asks: {row:?}"
        );
    }

    // Wait on all three rows: a capture that lands mid-frame would look like
    // a column that never appeared.
    press(&amx, &view, "v");
    let loud = amx.until("what each row runs", || {
        let drawn = amx.capture(&view);
        let said = rows(&drawn)?;
        (said[0].contains("claude opus high")
            && said[1].contains("claude")
            && said[2].contains("sh"))
        .then_some(drawn)
    });
    assert!(
        !rows_of(&loud)[1].contains("opus"),
        "a dial nobody turned is the vendor's own, and amx does not guess it: {:?}",
        rows_of(&loud)[1]
    );
    for (before, after) in rows_of(&quiet).iter().zip(rows_of(&loud)) {
        assert_eq!(
            before.find(char::is_alphanumeric),
            after.find(char::is_alphanumeric),
            "the name column does not move for it:\n{before:?}\n{after:?}"
        );
    }

    press(&amx, &view, "v");
    amx.until("the wall without it", || {
        let drawn = amx.capture(&view);
        rows(&drawn)?
            .iter()
            .all(|row| !row.contains("claude"))
            .then_some(())
    });

    // The setting persists, so a new view opens with the column on.
    press(&amx, &view, "v");
    amx.until("the column again", || {
        amx.capture(&view)
            .contains("claude opus high")
            .then_some(())
    });
    let again = amx.in_a_terminal(&[], &[]);
    amx.until("the second view to open on the same wall", || {
        let drawn = amx.capture(&again);
        let said = rows(&drawn)?;
        (said[0].contains("claude opus high") && said[2].contains("sh")).then_some(())
    });
}

#[test]
fn a_working_row_says_what_the_line_over_the_composer_says() {
    let amx = Harness::new();
    let mut pane_rows = vec![
        "● Read(src/importer.rs)",
        "  ⎿  Read 210 lines",
        "",
        "✽ Nesting… (15s · ↓ 1.3k tokens)",
    ];
    pane_rows.extend_from_slice(&CHROME);
    let pane = a_pane_showing(&amx, &pane_rows);
    amx.record("port-cli-b2c", &pane);
    // Hooks silent for ten minutes mid-turn: the record still names the last
    // tool call, which may be long over.
    let quiet_since = now() - 600;
    amx.set_state(
        "port-cli-b2c",
        json!({
            "state": "working",
            "summary": "Running Read",
            "since": quiet_since,
            "last_event": quiet_since,
        }),
    );

    let view = amx.in_a_terminal(&[], &[]);
    let row = amx.until("the row", || {
        row_of(&amx, &view, "port-cli-b2c").filter(|row| row.contains("Running Read"))
    });
    assert!(
        !row.contains("Nesting"),
        "the vendor's spinner line is its own chrome — a gerund it picked and \
         a clock the row keeps in its own column — so the row goes on saying \
         what the hooks said:\n{row}"
    );

    // Unlike the row, the card's rule line shows claude's spinner line for
    // the whole turn.
    let carded = card_on(&amx, &view, "port-cli-b2c");
    let ruled = carded
        .lines()
        .find(|line| line.contains("port-cli-b2c") && line.contains('┈'))
        .unwrap_or_else(|| panic!("no rule in:\n{carded}"));
    assert!(
        ruled.contains("Nesting… (15s · ↓ 1.3k tokens)"),
        "the vendor's line whole, less the glyph it pulses in front of it:\n{ruled}"
    );

    // The spinner line is read live and never stored: in the record it would
    // go stale and later readers would show it as current.
    assert_eq!(
        amx.state("port-cli-b2c")["summary"],
        "Running Read",
        "the record says what the hooks said"
    );
}

#[test]
fn a_working_row_says_the_newest_line_of_its_transcript() {
    let amx = Harness::new();
    let mut pane_rows = vec!["✽ Nesting… (15s · ↓ 1.3k tokens)"];
    pane_rows.extend_from_slice(&CHROME);
    let pane = a_pane_showing(&amx, &pane_rows);
    amx.record("port-cli-b2c", &pane);

    // Mid-turn transcript: one text message and no tool call after it.
    let transcript = amx.agent_dir("port-cli-b2c").join("session.jsonl");
    let said = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\
                \"text\":\"The importer keeps its own clock.\"}]}}\n";
    std::fs::write(&transcript, said).expect("the transcript");
    amx.set_meta("port-cli-b2c", json!({ "transcript": transcript }));

    // Hooks silent for ten minutes since the call the record names.
    let quiet_since = now() - 600;
    amx.set_state(
        "port-cli-b2c",
        json!({
            "state": "working",
            "summary": "Running Read",
            "since": quiet_since,
            "last_event": quiet_since,
        }),
    );

    let view = amx.in_a_terminal(&[], &[]);
    let row = amx.until("the row to say what the transcript says", || {
        row_of(&amx, &view, "port-cli-b2c").filter(|row| row.contains("The importer"))
    });
    assert!(
        row.contains("The importer keeps its own clock."),
        "the newest line of the conversation, between two calls:\n{row}"
    );
    assert!(
        !row.contains("Nesting"),
        "which is newer than the line the vendor spins:\n{row}"
    );

    // A newer tool call: the row shows the tool and its path.
    let call = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\
                \"name\":\"Read\",\"input\":{\"file_path\":\"src/importer.rs\"}}]}}\n";
    std::fs::write(&transcript, format!("{said}{call}")).expect("the transcript");

    let row = amx.until("the row to say the call the transcript names", || {
        row_of(&amx, &view, "port-cli-b2c").filter(|row| row.contains("Read src/importer.rs"))
    });
    assert!(
        !row.contains("Running Read"),
        "the tool and its path, not the record's word for the same call:\n{row}"
    );

    // Transcript lines are not stored either; each reader reads the
    // transcript itself.
    assert_eq!(
        amx.state("port-cli-b2c")["summary"],
        "Running Read",
        "the record says what the hooks said"
    );
}

/// Record agent `id` as done a minute ago, with a multi-paragraph answer that
/// has no summary yet.
fn ended_with_an_answer(amx: &Harness, id: &str) {
    let at = now() - 60;
    amx.record(id, "%404");
    amx.set_state(
        id,
        json!({
            "state": "done",
            "exit": 0,
            "since": at,
            "last_event": at,
            "result": "ported the importer\n\nthe fixtures moved with it, and the suite is green.",
        }),
    );
}

#[test]
fn summary_command_writes_the_line_a_finished_row_shows() {
    let amx = Harness::new();
    // In real use this is usually a model call; `tr` gives a deterministic
    // stand-in.
    amx.config("summary_command = \"tr a-z A-Z\"\n");
    ended_with_an_answer(&amx, "port-cli-b2c");

    let view = amx.in_a_terminal(&[], &[]);
    let row = amx.until("the row to say what the command made of the answer", || {
        row_of(&amx, &view, "port-cli-b2c").filter(|row| row.contains("PORTED THE IMPORTER"))
    });
    assert!(
        !row.contains("ported the importer"),
        "the line stands where the answer's first line stood:\n{row}"
    );

    // The summary is stored so later readers skip the command, and
    // summary.asked marks the ask as finished.
    assert_eq!(
        amx.state("port-cli-b2c")["summary"],
        "PORTED THE IMPORTER",
        "the whole answer went in, and the first line it printed came out"
    );
    let asked: Value = serde_json::from_slice(
        &std::fs::read(amx.agent_dir("port-cli-b2c").join("summary.asked")).expect("the ask"),
    )
    .expect("the ask");
    assert_eq!(asked["over"], true, "one ask per turn, whatever came back");
}

#[test]
fn a_finished_row_without_a_summary_command_costs_nothing_and_keeps_the_answer() {
    let amx = Harness::new();
    ended_with_an_answer(&amx, "port-cli-b2c");

    let view = amx.in_a_terminal(&[], &[]);
    let row = amx.until("the row", || {
        row_of(&amx, &view, "port-cli-b2c").filter(|row| row.contains("ported the importer"))
    });
    assert!(
        !row.contains("the fixtures moved with it"),
        "the answer's first line, which is what a row has room for:\n{row}"
    );

    assert!(
        !amx.agent_dir("port-cli-b2c").join("summary.asked").exists(),
        "no command is no question"
    );
    assert_eq!(amx.state("port-cli-b2c")["summary"], Value::Null);
}

#[test]
fn ctrl_s_turns_the_axis_onto_the_project_each_agent_runs_in() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("fix-login-b2c", "happy-turn");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("fix-login-b2c", "idle");
    // One agent at the repo root and one in a subdirectory, which takes the
    // walk up to the root. old-job-c3d stays outside any repository.
    running_in(&amx, "ask-a1b", &repo);
    running_in(&amx, "fix-login-b2c", &repo.join("src"));
    finished(&amx, "old-job-c3d", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the agents", || {
        amx.capture(&view).contains("Needs input").then_some(())
    });

    press(&amx, &view, "C-s");
    let drawn = amx.until("the project headings", || {
        let drawn = amx.capture(&view);
        drawn.contains("~/repo").then_some(drawn)
    });

    let row = |id: &str| {
        drawn
            .lines()
            .find(|line| line.contains(id))
            .unwrap_or_else(|| panic!("no row for {id} in:\n{drawn}"))
            .to_string()
    };
    // A heading is the project followed by a state summary, so match its first
    // word exactly: `~` must not match the `~/repo` heading.
    let headings = |text: &str| {
        let heading = text.to_string();
        drawn
            .lines()
            .enumerate()
            .filter(move |(_, line)| line.split_whitespace().next() == Some(heading.as_str()))
    };
    let at = |text: &str| match headings(text).next() {
        Some((at, _)) => at,
        None => panic!("no {text} heading in:\n{drawn}"),
    };
    assert!(
        at("~/repo") < at("~"),
        "the project with the question in it comes first:\n{drawn}"
    );
    assert!(
        headings("~/repo").count() == 1,
        "one heading for the repository, subdirectory and all:\n{drawn}"
    );
    // Project headings drop the state, so each row shows its own.
    assert!(row("ask-a1b").contains("waiting"), "{drawn}");
    assert!(row("fix-login-b2c").contains("done"), "{drawn}");
    assert!(row("old-job-c3d").contains("done"), "{drawn}");
    assert!(
        !drawn.contains("Needs input"),
        "and the state headings are gone with the axis:\n{drawn}"
    );

    // Further presses go to state, repository, then state again: the state
    // axis sits between the two path axes.
    press(&amx, &view, "C-s");
    amx.until("what they need in between", || {
        amx.capture(&view).contains("Needs input").then_some(())
    });
    press(&amx, &view, "C-s");
    amx.until("the repository headings", || {
        let drawn = amx.capture(&view);
        // A repository heading names its root's branch. Match at line start,
        // since the header also shows `~/repo` as where the next agent runs.
        let whole = drawn.lines().any(|line| line.starts_with("~/repo (main)"));
        (whole && !drawn.contains("Needs input")).then_some(drawn)
    });
    press(&amx, &view, "C-s");
    amx.until("what they need again", || {
        amx.capture(&view).contains("Needs input").then_some(())
    });
}

#[test]
fn a_pin_in_one_view_is_on_the_other_by_its_next_reading() {
    // Two views share one state file: a pin made in one shows in the other on
    // its next refresh, without reopening either.
    let amx = Harness::new();
    amx.play("one-a1b", "asks-a-question");
    amx.play("two-b2c", "happy-turn");
    amx.until_state("one-a1b", "waiting");
    amx.until_state("two-b2c", "idle");

    let left = amx.in_a_terminal(&[], &[]);
    let right = amx.in_a_terminal(&[], &[]);
    amx.until("both walls", || {
        (amx.capture(&left).contains("one-a1b") && amx.capture(&right).contains("one-a1b"))
            .then_some(())
    });
    assert!(
        !amx.capture(&right).contains("Pinned"),
        "nothing is pinned yet:\n{}",
        amx.capture(&right)
    );

    // w moves the cursor to the waiting agent.
    press(&amx, &left, "w");
    press(&amx, &left, "C-t");
    amx.until("the pin on the left", || {
        amx.capture(&left).contains("Pinned").then_some(())
    });

    amx.until("the pin on the right", || {
        amx.capture(&right).contains("Pinned").then_some(())
    });
}

#[test]
fn a_wall_with_nothing_on_it_says_so_in_one_line_of_amxs_own() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);

    let drawn = amx.capture(&view);
    // No group headings, only amx's empty-wall line. How many lines the empty
    // wall takes is not pinned.
    for group in [
        "Pinned",
        "Ready for review",
        "Needs input",
        "Working",
        "Completed",
    ] {
        assert!(
            !drawn.contains(group),
            "{group} is a heading over rows, and there are none:\n{drawn}"
        );
    }
    assert!(
        drawn.contains("no agents yet"),
        "and the wall says what it is in its own words:\n{drawn}"
    );

    amx.play("ask-a1b", "asks-a-question");
    amx.until("the agent's own row", || {
        let drawn = amx.capture(&view);
        (drawn.contains("ask-a1b") && !drawn.contains("no agents yet")).then_some(())
    });
}

#[test]
fn a_blank_line_stands_the_list_off_from_the_header() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    let drawn = amx.until("the first heading", || {
        let drawn = amx.capture(&view);
        drawn.contains("Needs input").then_some(drawn)
    });

    let lines: Vec<&str> = drawn.lines().map(str::trim_end).collect();
    let at = lines
        .iter()
        .position(|line| *line == "Needs input")
        .unwrap_or_else(|| panic!("no Needs input heading in:\n{drawn}"));
    assert!(
        lines[at - 1].is_empty(),
        "the first heading is stood off from the header the way the next one \
         is stood off from it:\n{drawn}"
    );
    assert!(
        lines[at - 2].starts_with("└ next") && lines[at - 3].contains("running"),
        "and what the space is under is the header, both rows of it:\n{drawn}"
    );
}

#[test]
fn every_group_folds_past_thirty_rows_and_enter_opens_the_one_it_is_on() {
    let amx = Harness::new();
    // 64 ended agents: 32 with an open request and 32 without, each set in
    // its own directory. The state and project axes both make two groups of
    // 32, so the folds are the same on either.
    let reviews = amx.home().join("reviews");
    let shipped = amx.home().join("shipped");
    for at in [&reviews, &shipped] {
        std::fs::create_dir_all(at).expect("a place for them to have run");
    }
    for n in 0..32u64 {
        let id = format!("review-{n:02}");
        finished(&amx, &id, "done", (n + 1) * 60);
        a_request(&amx, &id, 100 + n, "open");
        running_in(&amx, &id, &reviews);
        let id = format!("over-{n:02}");
        finished(&amx, &id, "done", (n + 1) * 60);
        running_in(&amx, &id, &shipped);
    }

    // Tall enough that screen height never hides a row.
    let view = amx.in_a_terminal(&[], &[]);
    amx.tmux(&["resize-window", "-t", &view, "-x", "80", "-y", "80"]);
    let folded = amx.until("both groups folded", || {
        let drawn = amx.capture(&view);
        (drawn.matches("2 more").count() == 2
            && drawn.contains("Ready for review")
            && drawn.contains("Completed"))
        .then_some(drawn)
    });
    assert_eq!(
        rows_of(&folded, "review-"),
        30,
        "thirty of thirty-two:\n{folded}"
    );
    assert_eq!(
        rows_of(&folded, "over-"),
        30,
        "and thirty of thirty-two:\n{folded}"
    );

    // Thirty Downs put the cursor on the first group's fold line.
    for _ in 0..30 {
        press(&amx, &view, "Down");
    }
    press(&amx, &view, "Enter");
    let opened = amx.until("the group that was opened", || {
        let drawn = amx.capture(&view);
        (rows_of(&drawn, "review-") == 32 && drawn.matches("2 more").count() == 1).then_some(drawn)
    });
    assert_eq!(rows_of(&opened, "over-"), 30, "{opened}");

    // By project the groups fold the same way, and a fold opened under a state
    // heading is shut again under a path heading.
    press(&amx, &view, "C-s");
    let by_project = amx.until("both projects folded", || {
        let drawn = amx.capture(&view);
        (drawn.contains("~/reviews") && drawn.matches("2 more").count() == 2).then_some(drawn)
    });
    assert_eq!(rows_of(&by_project, "review-"), 30, "{by_project}");
    assert_eq!(rows_of(&by_project, "over-"), 30, "{by_project}");
}

#[test]
fn a_row_brings_up_the_name_under_the_cursor_and_leaves_the_wall_quiet() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");
    finished(&amx, "fix-login-b2c", "done", 60);
    read(&amx, "fix-login-b2c");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = amx.capture(&view);
        (drawn.contains("ask-a1b") && drawn.contains("fix-login-b2c")).then_some(())
    });

    // Off the cursor, the name and summary are both dim and neither is bold.
    let quiet = coloured_line(&amx, &view, "fix-login-b2c");
    let name = sgr_at(&quiet, "fix-login-b2c");
    assert!(
        name.contains(&2) && !name.contains(&1),
        "the name is dim and carries no weight:\n{quiet:?}"
    );
    assert!(
        sgr_at(&quiet, "did what it was asked").contains(&2),
        "and what it said is drawn at the same strength:\n{quiet:?}"
    );
    assert!(
        quiet.contains(&foreground("done")),
        "the glyph alone carries the state's colour:\n{quiet:?}"
    );

    // The cursor starts on ask-a1b: its name is normal intensity in the
    // waiting colour, and its question is not dimmed.
    let asking = coloured_line(&amx, &view, "ask-a1b");
    let name = sgr_at(&asking, "ask-a1b");
    assert!(
        !name.contains(&2) && !name.contains(&1),
        "the name under the cursor comes up without weight:\n{asking:?}"
    );
    assert!(
        asking.contains(&foreground("waiting")),
        "and it is painted for what it wants:\n{asking:?}"
    );
    assert!(
        !sgr_at(&asking, "Claude needs your permission").contains(&2),
        "the question is not dimmed the way a finished row's line is:\n{asking:?}"
    );
}

#[test]
fn a_row_lands_its_name_summary_and_age_in_the_columns_the_grid_fixes() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    let row = amx.until("the row, drawn to the edge", || {
        row_of(&amx, &view, "fix-login-a1b").filter(|row| row.chars().count() == 80)
    });
    let cells: Vec<char> = row.chars().collect();
    assert_eq!(cells.len(), 80, "a row is drawn to the edge:\n{row:?}");

    // One cell of indent, the glyph and a space, then the name column, which
    // is 16 cells on a screen under 100 columns.
    let column = |from: usize, to: usize| cells[from..to].iter().collect::<String>();
    assert_eq!(column(3, 19), "fix-login-a1b   ", "{row:?}");
    assert_eq!(column(19, 21), "  ", "two cells stand the columns apart");

    // The summary takes whatever width is left.
    assert_eq!(
        column(21, 74),
        format!("{:<53}", "did what it was asked"),
        "{row:?}"
    );

    // The age is right-aligned in the last four cells.
    assert_eq!(column(74, 76), "  ", "{row:?}");
    let age = column(76, 80);
    assert!(
        !age.trim().is_empty() && !age.ends_with(' '),
        "the age is right-aligned in its own column:\n{row:?}"
    );
}

#[test]
fn a_row_goes_under_the_title_its_session_was_given_until_somebody_renames_it() {
    let amx = Harness::new();
    finished(&amx, "port-import-b2c", "done", 60);
    titled(&amx, "port-import-b2c", "Importer clock");
    // Title and name both set, to check which one wins.
    finished(&amx, "fix-login-a1b", "done", 120);
    titled(&amx, "fix-login-a1b", "Login timeout");
    renamed(&amx, "fix-login-a1b", "auth");

    let view = amx.in_a_terminal(&[], &[]);
    let row = amx.until("the row under the title claude gave the session", || {
        row_of(&amx, &view, "Importer clock")
    });
    assert!(
        !row.contains("port-import-b2c"),
        "the title stands where the id stood:\n{row}"
    );
    let both = row_of(&amx, &view, "auth").expect("the renamed row");
    assert!(
        !both.contains("Login timeout"),
        "and a name somebody typed here outranks the one the session goes \
         under:\n{both}"
    );

    // Find matches the title as it matches a name or an id.
    types(&amx, &view, "/");
    types(&amx, &view, "clock");
    let drawn = amx.until("the wall narrowed to the title", || {
        let drawn = amx.capture(&view);
        (drawn.contains("Importer clock") && !drawn.contains("auth")).then_some(drawn)
    });
    assert!(
        !drawn.contains("fix-login-a1b"),
        "the row the word misses is gone, id and all:\n{drawn}"
    );
}

#[test]
fn a_group_heading_is_the_groups_own_words_and_stops_there() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");
    finished(&amx, "one-b2c", "done", 60);
    finished(&amx, "two-c3d", "done", 120);

    let view = amx.in_a_terminal(&[], &[]);
    // Wait for both headings complete: the view has no synchronized output,
    // so a capture can land partway through a line.
    let drawn = amx.until("both headings, whole", || {
        let drawn = amx.capture(&view);
        let whole = ["Needs input", "Completed"]
            .iter()
            .all(|label| drawn.lines().any(|line| line.trim_end() == *label));
        whole.then_some(drawn)
    });

    // A heading is just its label: no rule to the edge and no count, since
    // the rows under it are on screen.
    assert!(
        !drawn.contains('┈'),
        "nothing carries the eye out to the edge of the wall:\n{drawn}"
    );

    // Headings are dim like row summaries, except Needs input, which takes the
    // waiting colour. Read the whole screen: the heading's dim is switched on
    // by the row above it and left in force.
    let painted = coloured(&amx, &view);
    let label = sgr_at(&painted, "Completed");
    assert!(
        label.contains(&2) && !label.contains(&1),
        "the label is dim and carries no weight:\n{painted:?}"
    );
    let waiting = coloured_line(&amx, &view, "Needs input");
    assert!(
        waiting.contains(&foreground("waiting")),
        "and the group that wants a person is painted for it:\n{waiting:?}"
    );
}

#[test]
fn a_path_heading_reads_the_way_a_group_heading_does() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");
    running_in(&amx, "ask-a1b", &repo);
    finished(&amx, "old-job-c3d", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the agents", || {
        amx.capture(&view).contains("Needs input").then_some(())
    });
    press(&amx, &view, "C-s");
    let drawn = amx.until("the heading over the repository, whole", || {
        let drawn = amx.capture(&view);
        let whole = drawn
            .lines()
            .any(|line| line.split_whitespace().next() == Some("~/repo"));
        whole.then_some(drawn)
    });

    // Same as a group heading: the path alone, with no rule or count.
    assert!(
        !drawn.contains('┈'),
        "no rule over a project either:\n{drawn}"
    );

    // The whole path is dim, last segment included. Each segment is looked up
    // on its own since the two used to differ by an escape between them, and
    // on the whole screen since the dim carries over from the rows above.
    let painted = coloured(&amx, &view);
    for segment in ["~/", "repo"] {
        let cells = sgr_at(&painted, segment);
        assert!(
            cells.contains(&2) && !cells.contains(&1),
            "{segment} carries no more weight than the rest of the path:\n{painted:?}"
        );
    }
}

#[test]
fn a_path_too_long_for_its_heading_loses_its_middle_and_not_its_end() {
    let amx = Harness::new();
    let deep =
        PathBuf::from("/srv/monorepo/services/ingest/packages/importer-worker/vendor/legacy-shim");
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");
    running_in(&amx, "ask-a1b", &deep);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the agent", || {
        amx.capture(&view).contains("Needs input").then_some(())
    });
    press(&amx, &view, "C-s");
    let line = amx.until("the heading over the deep path, whole", || {
        amx.capture(&view)
            .lines()
            .find(|line| line.starts_with("/srv/") && line.contains("legacy-shim"))
            .map(str::to_string)
    });

    assert!(
        line.chars().count() <= 80,
        "a path this long does not push the heading off the edge:\n{line:?}"
    );

    let path = line.split_whitespace().next().expect("the path");
    assert!(
        path.starts_with("/srv/…/") && path.ends_with("/vendor/legacy-shim"),
        "what goes is the middle: the end is the segment that says \
         which worktree of a project this is:\n{line:?}"
    );
}

#[test]
fn a_row_under_a_path_grows_a_state_word_and_moves_no_other_column() {
    let amx = Harness::new();
    amx.play("port-import-b2c", "works-with-a-spinner");
    amx.until_state("port-import-b2c", "working");
    finished(&amx, "fix-login-a1b", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = amx.capture(&view);
        (drawn.contains("fix-login-a1b") && drawn.contains("port-import-b2c")).then_some(())
    });
    let before = row_of(&amx, &view, "fix-login-a1b").expect("a row under its state");

    press(&amx, &view, "C-s");
    let row = amx.until("the state word on the row, drawn to the edge", || {
        row_of(&amx, &view, "fix-login-a1b")
            .filter(|row| row.contains("done") && row.chars().count() == 80)
    });
    let cells: Vec<char> = row.chars().collect();
    let column = |from: usize, to: usize| cells[from..to].iter().collect::<String>();
    assert_eq!(cells.len(), 80, "a row is drawn to the edge:\n{row:?}");
    assert_eq!(column(3, 19), "fix-login-a1b   ", "the name has not moved");
    assert_eq!(column(19, 21), "  ", "two cells stand the columns apart");

    // Under a path heading the row shows its state in an 8-cell column, the
    // width of the longest state word.
    assert_eq!(column(21, 29), "done    ", "{row:?}");
    assert_eq!(column(29, 31), "  ", "{row:?}");

    // Only the summary shrinks, by the ten cells of state word and gap.
    assert_eq!(
        column(31, 74),
        format!("{:<43}", "did what it was asked"),
        "{row:?}"
    );
    assert_eq!(column(74, 76), "  ", "{row:?}");
    let was: Vec<char> = before.chars().collect();
    assert_eq!(
        column(76, 80),
        was[76..80].iter().collect::<String>(),
        "the age is in the cells it was in under a state heading:\n{row:?}\n{before:?}"
    );

    // The state word is dim while the agent works, and in the state's colour
    // once it has ended.
    let working = coloured_line(&amx, &view, "port-import-b2c");
    assert!(
        sgr_at(&working, "working").contains(&2),
        "a row still at it says so under its breath:\n{working:?}"
    );
    let done = coloured_line(&amx, &view, "fix-login-a1b");
    assert!(
        !sgr_at(&done, "done").contains(&2),
        "and a row that has ended says how it went:\n{done:?}"
    );
    assert!(
        done.contains(&foreground("done")),
        "in the colour that says so:\n{done:?}"
    );
}

#[test]
fn the_view_paints_in_the_theme_the_config_names() {
    let amx = Harness::new();
    amx.config("theme = \"terminal\"\n");
    finished(&amx, "fix-login-a1b", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        amx.capture(&view).contains("fix-login-a1b").then_some(())
    });

    // The terminal theme uses named colours only, so the row is painted from
    // the terminal's palette. tmux writes a named colour as an index
    // (`38;5;n`).
    let row = coloured_line(&amx, &view, "fix-login-a1b");
    assert!(
        row.contains("38;5;"),
        "nothing on the row is painted in a colour the palette names:\n{row:?}"
    );

    // Neither the default theme's done green nor its cursor bar is on the row.
    assert!(
        !row.contains(&foreground("done")) && !row.contains(&bar()),
        "the default theme is what is being painted:\n{row:?}"
    );
}

#[test]
fn editing_the_theme_recolours_the_view_that_is_open_on_it() {
    let amx = Harness::new();
    amx.config("theme = \"mine\"\n");
    theme(&amx, "mine", "done = \"#ff00ff\"\n");
    finished(&amx, "fix-login-a1b", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    let before = text_in(rgb("#ff00ff"));
    amx.until("the row in the colour the theme says", || {
        coloured(&amx, &view).contains(&before).then_some(())
    });

    // Edit the file with no restart or keypress; the view must pick it up.
    theme(&amx, "mine", "done = \"#00ffff\"\n");

    let after = text_in(rgb("#00ffff"));
    let drawn = amx.until("the row in the colour the file now says", || {
        let drawn = coloured(&amx, &view);
        drawn.contains(&after).then_some(drawn)
    });
    assert!(
        !drawn.contains(&before),
        "the colour that was edited away is still on the screen:\n{drawn:?}"
    );
}

#[test]
fn the_list_takes_the_mouse_and_a_click_is_the_cursor() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);
    finished(&amx, "port-import-b2c", "done", 120);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = amx.capture(&view);
        (drawn.contains("fix-login-a1b") && drawn.contains("port-import-b2c")).then_some(())
    });
    assert_eq!(
        pane_field(&amx, &view, "#{mouse_any_flag}"),
        "1",
        "the view asked the terminal for the mouse"
    );

    // The id also appears in the footer once selected, so find the row by its
    // summary too.
    click(
        &amx,
        &view,
        5,
        screen_row_of(&amx, &view, "port-import-b2c"),
    );
    amx.until("the bar under the clicked row", || {
        coloured(&amx, &view)
            .lines()
            .find(|line| line.contains("port-import-b2c") && line.contains("did what"))
            .filter(|line| line.contains(&bar()))
            .map(|_| ())
    });
    assert!(
        !coloured_line(&amx, &view, "fix-login-a1b").contains(&bar()),
        "one cursor, and the click is where it is"
    );

    // The click also tries to switch to the agent, as enter does. This agent
    // has no recorded session, so the refusal shows it got that far.
    amx.until("the refusal", || {
        amx.capture(&view)
            .contains("no session was recorded")
            .then_some(())
    });

    // Clicking a heading toggles its group.
    let heading = amx
        .capture(&view)
        .lines()
        .position(|line| line.trim_end() == "Completed")
        .expect("the heading") as u16
        + 1;
    click(&amx, &view, 5, heading);
    amx.until("the group shut", || {
        let drawn = amx.capture(&view);
        (drawn.contains("Completed") && !drawn.contains("port-import-b2c")).then_some(())
    });
    click(&amx, &view, 5, heading);
    amx.until("the group open again", || {
        amx.capture(&view).contains("port-import-b2c").then_some(())
    });

    // Quitting releases mouse capture.
    amx.tmux(&["set-option", "-w", "-t", &view, "remain-on-exit", "on"]);
    press(&amx, &view, "q");
    amx.until("the view to close", || {
        let dead = amx.tmux(&["display-message", "-p", "-t", &view, "#{pane_dead}"]);
        (dead == "1").then_some(())
    });
    assert_eq!(
        pane_field(&amx, &view, "#{mouse_any_flag}"),
        "0",
        "the capture was released on the way out"
    );
}

/// The 1-based screen column where `word` starts on the agent's row.
///
/// Counted in chars, not bytes: the row's glyph is a multi-byte char.
fn column_of(amx: &Harness, view: &str, id: &str, word: &str) -> u16 {
    let row = row_of(amx, view, id).unwrap_or_else(|| panic!("no row for {id}"));
    let at = row
        .find(word)
        .unwrap_or_else(|| panic!("no {word} on {row:?}"));
    row[..at].chars().count() as u16 + 1
}

#[test]
fn a_left_drag_reverses_the_cells_it_covers_and_copies_them_on_release() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);
    let view = amx.in_a_terminal(&[], &[]);
    // By default tmux passes OSC 52 to the outer terminal and keeps no
    // buffer; `set-clipboard on` makes it keep one the test can read. Set
    // after the view starts, since that is what starts the server.
    amx.tmux(&["set-option", "-s", "set-clipboard", "on"]);
    amx.until("the row", || {
        amx.capture(&view).contains("fix-login-a1b").then_some(())
    });

    // Drag from the first cell of the id to its last; the covered cells show
    // reversed while the button is held.
    let row = screen_row_of(&amx, &view, "fix-login-a1b");
    let from = column_of(&amx, &view, "fix-login-a1b", "fix-login-a1b");
    let to = from + "fix-login-a1b".len() as u16 - 1;
    mouse(&amx, &view, 0, from, row, true);
    mouse(&amx, &view, 32, to, row, true);
    amx.until("the dragged cells reversed", || {
        let line = coloured_line(&amx, &view, "fix-login-a1b");
        sgr_at(&line, "fix-login-a1b").contains(&7).then_some(())
    });

    // Release copies them by OSC 52, which lands in a tmux buffer here, and
    // the view reports the copy.
    mouse(&amx, &view, 0, to, row, false);
    amx.until("the view to say it copied", || {
        amx.capture(&view).contains("copied").then_some(())
    });
    assert_eq!(
        amx.tmux(&["show-buffer"]),
        "fix-login-a1b",
        "the id the drag covered and nothing either side of it"
    );
    assert!(
        !sgr_at(
            &coloured_line(&amx, &view, "fix-login-a1b"),
            "fix-login-a1b"
        )
        .contains(&7),
        "and the reverse went with the button"
    );
}

#[test]
fn hovering_a_row_tints_its_name_and_moves_no_cursor() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);
    finished(&amx, "port-import-b2c", "done", 120);
    // Mark both seen so the hover tint is the only styling difference.
    read(&amx, "fix-login-a1b");
    read(&amx, "port-import-b2c");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = amx.capture(&view);
        (drawn.contains("fix-login-a1b") && drawn.contains("port-import-b2c")).then_some(())
    });

    // Hover over the row the cursor is not on.
    let row = screen_row_of(&amx, &view, "port-import-b2c");
    mouse(&amx, &view, 35, 5, row, true);
    amx.until("the name to take the tint", || {
        let name = sgr_at(
            &coloured_line(&amx, &view, "port-import-b2c"),
            "port-import-b2c",
        );
        (!name.contains(&2) && !name.contains(&1)).then_some(())
    });
    assert!(
        coloured_line(&amx, &view, "fix-login-a1b").contains(&bar()),
        "the bar stayed where the keyboard's cursor is"
    );
    assert!(
        !coloured_line(&amx, &view, "port-import-b2c").contains(&bar()),
        "a hover is a tint, not a selection"
    );
}

/// The agent id on the open card's rule line, or None with no card open.
///
/// The rule is the only line with `┈` that starts in column 0, since list rows
/// are indented. The id is its second word, after the agent's glyph.
fn card_rule(drawn: &str) -> Option<String> {
    drawn
        .lines()
        .find(|line| line.contains('┈') && !line.starts_with(' '))
        .and_then(|line| line.split_whitespace().nth(1))
        .map(str::to_string)
}

/// Whether the cursor bar is on the agent's list row.
///
/// The card's rule also names the agent, so match on the summary only a row
/// carries.
fn barred(amx: &Harness, view: &str, id: &str) -> bool {
    coloured(amx, view)
        .lines()
        .any(|line| line.contains(id) && line.contains("did what") && line.contains(&bar()))
}

#[test]
fn space_opens_the_card_on_the_row_under_the_pointer() {
    let amx = Harness::new();
    finished(&amx, "first-a1b", "done", 60);
    finished(&amx, "second-b2c", "done", 120);
    finished(&amx, "third-c3d", "done", 180);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("three rows with the bar on the first", || {
        let drawn = amx.capture(&view);
        (drawn.contains("second-b2c")
            && drawn.contains("third-c3d")
            && barred(&amx, &view, "first-a1b"))
        .then_some(())
    });

    // With the pointer resting on a row, space opens that row's card and
    // moves the cursor there.
    mouse(
        &amx,
        &view,
        35,
        5,
        screen_row_of(&amx, &view, "third-c3d"),
        true,
    );
    press(&amx, &view, "Space");
    amx.until("the third row's card", || {
        (card_rule(&amx.capture(&view)).as_deref() == Some("third-c3d")).then_some(())
    });
    assert!(
        barred(&amx, &view, "third-c3d"),
        "the cursor landed where the pointer was"
    );

    // Again with the pointer on the first row.
    press(&amx, &view, "Escape");
    amx.until("the card away", || {
        card_rule(&amx.capture(&view)).is_none().then_some(())
    });
    mouse(
        &amx,
        &view,
        35,
        5,
        screen_row_of(&amx, &view, "first-a1b"),
        true,
    );
    press(&amx, &view, "Space");
    amx.until("the first row's card", || {
        (card_rule(&amx.capture(&view)).as_deref() == Some("first-a1b")).then_some(())
    });
    assert!(
        barred(&amx, &view, "first-a1b"),
        "the cursor landed where the pointer was"
    );

    // With the pointer off the list, space opens the cursor's card, not the
    // row the pointer last rested on.
    press(&amx, &view, "Escape");
    amx.until("the card away", || {
        card_rule(&amx.capture(&view)).is_none().then_some(())
    });
    mouse(&amx, &view, 35, 5, 1, true);
    press(&amx, &view, "j");
    press(&amx, &view, "Space");
    amx.until("the second row's card", || {
        (card_rule(&amx.capture(&view)).as_deref() == Some("second-b2c")).then_some(())
    });
}

#[test]
fn the_wheel_scrolls_the_wall_and_the_keys_move_the_cursor() {
    let amx = Harness::new();
    // More agents than the list fits, each a minute older than the last so
    // they draw in name order.
    for n in 0..25u64 {
        finished(&amx, &format!("row-{n:02}-a1b"), "done", 60 + n * 60);
    }

    let view = amx.in_a_terminal(&[], &[]);
    let first = line_of(
        &amx.until("a wall taller than the band", || {
            let drawn = amx.capture(&view);
            drawn.contains("row-00-a1b").then_some(drawn)
        }),
        "row-00-a1b",
    );

    // Three wheel-downs scroll three lines: the heading and the first two rows
    // go off the top, and row-02 takes the heading's line.
    for _ in 0..3 {
        mouse(&amx, &view, 65, 5, first as u16 + 1, true);
    }
    let scrolled = amx.until("three rows of wall scrolled away", || {
        let drawn = amx.capture(&view);
        (drawn.lines().position(|line| line.contains("row-02-a1b")) == Some(first - 1))
            .then_some(drawn)
    });
    assert!(
        !scrolled.contains("row-00-a1b"),
        "the row the cursor is on is off the top:\n{scrolled}"
    );

    // The wheel left the cursor on row-00, so `j` moves it to row-01 and the
    // window scrolls back just far enough to show it.
    press(&amx, &view, "j");
    let walked = amx.until("the cursor's row back on the screen", || {
        let drawn = amx.capture(&view);
        drawn.contains("row-01-a1b").then_some(drawn)
    });
    assert_eq!(
        line_of(&walked, "row-01-a1b"),
        first - 1,
        "on the first drawn line:\n{walked}"
    );
    assert!(
        coloured_line(&amx, &view, "row-01-a1b").contains(&bar()),
        "and the cursor is on it"
    );
}

#[test]
fn the_wheel_pages_the_card_under_the_pointer_and_leaves_the_cursor_alone() {
    let amx = Harness::new();
    amx.record("tall-b2c", "%404");
    let at = now() - 100;
    amx.set_state(
        "tall-b2c",
        json!({
            "state": "done",
            "exit": 0,
            "since": at,
            "last_event": at,
            "result": (0..40).map(|n| format!("said {n}\n")).collect::<String>(),
        }),
    );
    finished(&amx, "short-a1b", "done", 200);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = amx.capture(&view);
        (drawn.contains("tall-b2c") && drawn.contains("short-a1b")).then_some(())
    });

    // The wheel over the open card pages it. The row's summary is also the
    // answer's first line, so count "said 0" to tell the card's copy apart.
    let carded = card_on(&amx, &view, "tall-b2c");
    assert_eq!(
        carded.matches("said 0").count(),
        2,
        "the top of the answer, over the row saying the same:\n{carded}"
    );
    let inside = carded
        .lines()
        .position(|line| line.contains("said 2"))
        .expect("a row of the card's body") as u16
        + 1;
    mouse(&amx, &view, 65, 5, inside, true);
    let paged = amx.until("the paged card", || {
        let drawn = amx.capture(&view);
        drawn.contains("more").then_some(drawn)
    });
    assert_eq!(
        paged.matches("said 0").count(),
        1,
        "the top is behind, and only the row still says it:\n{paged}"
    );
    mouse(&amx, &view, 64, 5, inside, true);
    amx.until("the edge again", || {
        (amx.capture(&view).matches("said 0").count() == 2).then_some(())
    });

    // A wheel over the rows scrolls the wall, which has nowhere to go with two
    // rows. The card wheel sent after it shows when both are handled; neither
    // may move the cursor or change the card.
    let over_rows = screen_row_of(&amx, &view, "short-a1b");
    mouse(&amx, &view, 65, 5, over_rows, true);
    mouse(&amx, &view, 65, 5, inside, true);
    amx.until("the card paged past its top again", || {
        (amx.capture(&view).matches("said 0").count() == 1).then_some(())
    });
    // Match the row, not the card's rule, by the summary only a row carries.
    let rows = coloured(&amx, &view);
    assert!(
        rows.lines().any(|line| line.contains("tall-b2c")
            && line.contains("said 0")
            && line.contains(&bar())),
        "the wheel over the rows moved no cursor:\n{rows}"
    );
    assert_eq!(
        card_rule(&amx.capture(&view)).as_deref(),
        Some("tall-b2c"),
        "and took the card nowhere"
    );
}

#[test]
fn the_cursor_is_a_bar_over_rows_and_headings_alike() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        amx.capture(&view).contains("ask-a1b").then_some(())
    });

    // The bar is a background colour, so check the escapes. It is the theme's
    // cursor colour, which matches the vendor's selected-line colour.
    amx.until("the bar under the cursor", || {
        coloured_line(&amx, &view, "ask-a1b")
            .contains(&bar())
            .then_some(())
    });
    assert!(
        !coloured_line(&amx, &view, "Needs input").contains(&bar()),
        "and not under the heading the cursor is not on"
    );

    press(&amx, &view, "Up");
    amx.until("the bar to move up onto the heading", || {
        coloured_line(&amx, &view, "Needs input")
            .contains(&bar())
            .then_some(())
    });
    assert!(
        !coloured_line(&amx, &view, "ask-a1b").contains(&bar()),
        "one line at a time, whatever kind of line it is"
    );
}

#[test]
fn w_lands_the_bar_on_the_first_agent_that_needs_you() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("port-import-b2c", "works-with-a-spinner");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("port-import-b2c", "working");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the two groups", || {
        let drawn = amx.capture(&view);
        (drawn.contains("Needs input") && drawn.contains("Working")).then_some(())
    });

    // Pin the working agent so the waiting one sits below it and w has to
    // search down the wall.
    press(&amx, &view, "Down");
    press(&amx, &view, "Down");
    press(&amx, &view, "C-t");
    let drawn = amx.until("the pinned group", || {
        let drawn = amx.capture(&view);
        drawn.contains("Pinned").then_some(drawn)
    });
    assert!(
        line_of(&drawn, "port-import-b2c") < line_of(&drawn, "ask-a1b"),
        "the agent that is asking stands under the working one:\n{drawn}"
    );

    // Start at the top, the Pinned heading: w finds the first waiting agent
    // on the wall wherever the cursor is.
    twice(&amx, &view, "g");
    amx.until("the bar at the top", || {
        coloured_line(&amx, &view, "Pinned")
            .contains(&bar())
            .then_some(())
    });

    press(&amx, &view, "w");
    amx.until("the bar on the agent that is asking", || {
        coloured_line(&amx, &view, "ask-a1b")
            .contains(&bar())
            .then_some(())
    });
    let drawn = amx.capture(&view);
    assert!(
        !coloured_line(&amx, &view, "Pinned").contains(&bar()),
        "and off the line it was pressed on:\n{drawn}"
    );
    assert!(
        !drawn
            .lines()
            .any(|line| line.starts_with("✻ ask-a1b · claude ┈")),
        "the cursor is all the key moves, so nothing is opened over the wall:\n{drawn}"
    );
}

#[test]
fn the_vim_letters_walk_the_bar_and_a_card_takes_them_as_text() {
    // A card opens with an input line at its foot, so over a card these
    // letters are text and only esc closes it. On the wall they are keys.
    let amx = Harness::new();
    finished(&amx, "done-a1b", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        amx.capture(&view).contains("done-a1b").then_some(())
    });
    amx.until("the bar under the cursor", || {
        coloured_line(&amx, &view, "done-a1b")
            .contains(&bar())
            .then_some(())
    });

    press(&amx, &view, "k");
    amx.until("the bar to move up onto the heading", || {
        coloured_line(&amx, &view, "Completed")
            .contains(&bar())
            .then_some(())
    });
    press(&amx, &view, "j");
    amx.until("the bar back on the row", || {
        coloured_line(&amx, &view, "done-a1b")
            .contains(&bar())
            .then_some(())
    });

    // Space opens the card inside the view. An attach would hand the terminal
    // to tmux and the wall would be gone.
    let carded = |drawn: &str| {
        drawn
            .lines()
            .any(|line| line.starts_with("∙ done-a1b · claude ┈"))
    };
    press(&amx, &view, "Space");
    amx.until("the card", || carded(&amx.capture(&view)).then_some(()));

    // h used to close the card; now it is typed into the card's line.
    press(&amx, &view, "h");
    let typed = amx.until("the letter on the line", || {
        let drawn = amx.capture(&view);
        drawn.contains("❯ h").then_some(drawn)
    });
    assert!(
        carded(&typed),
        "with the card still open under it:\n{typed}"
    );

    press(&amx, &view, "Escape");
    amx.until("the card put away", || {
        let drawn = amx.capture(&view);
        (!carded(&drawn) && drawn.contains("done-a1b")).then_some(())
    });
}

#[test]
fn gg_and_g_reach_the_two_ends_of_the_list_and_one_g_waits() {
    let amx = Harness::new();
    finished(&amx, "one-a1b", "done", 60);
    finished(&amx, "two-b2c", "done", 120);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both agents", || {
        let drawn = amx.capture(&view);
        (drawn.contains("one-a1b") && drawn.contains("two-b2c")).then_some(())
    });

    press(&amx, &view, "G");
    amx.until("the bar at the foot", || {
        coloured_line(&amx, &view, "two-b2c")
            .contains(&bar())
            .then_some(())
    });

    press(&amx, &view, "g");
    let waiting = amx.until("the row to say a g is waiting", || {
        let drawn = amx.capture(&view);
        drawn.contains("g again").then_some(drawn)
    });
    assert!(
        coloured_line(&amx, &view, "two-b2c").contains(&bar()),
        "and the cursor has not moved for it:\n{waiting}"
    );

    // The top of the list is the Completed heading.
    press(&amx, &view, "g");
    amx.until("the bar at the top", || {
        let drawn = amx.capture(&view);
        (coloured_line(&amx, &view, "Completed").contains(&bar()) && !drawn.contains("g again"))
            .then_some(())
    });
}

#[test]
fn enter_shuts_the_group_its_headings_stand_over_and_opens_it_again() {
    let amx = Harness::new();
    finished(&amx, "one-a1b", "done", 60);
    finished(&amx, "two-b2c", "failed", 120);

    let view = amx.in_a_terminal(&[], &[]);
    // Wait for the whole heading: the view has no synchronized output, so a
    // capture can land mid-line.
    let drawn = amx.until("both agents under a heading that says so", || {
        let drawn = amx.capture(&view);
        (drawn.contains("one-a1b")
            && drawn.contains("two-b2c")
            && drawn.contains("Completed · 1 failed"))
        .then_some(drawn)
    });
    let heading = |drawn: &str| {
        drawn
            .lines()
            .find(|line| line.trim_end().starts_with("Completed"))
            .unwrap_or_else(|| panic!("no Completed heading in:\n{drawn}"))
            .trim_end()
            .to_string()
    };
    assert_eq!(
        heading(&drawn),
        "Completed · 1 failed",
        "a heading says how many failed under it, and leaves the counting of \
         the rows to the rows:\n{drawn}"
    );

    // Up moves from the first row to the heading.
    press(&amx, &view, "Up");
    press(&amx, &view, "Enter");
    let shut = amx.until("the group to be put away", || {
        let drawn = amx.capture(&view);
        (!drawn.contains("one-a1b") && drawn.contains("Completed 2 · 1 failed")).then_some(drawn)
    });
    assert!(!shut.contains("two-b2c"), "the rows are away:\n{shut}");
    assert_eq!(
        heading(&shut),
        "Completed 2 · 1 failed",
        "the count comes up to stand for the rows that have gone, and the \
         failures keep their place after it:\n{shut}"
    );

    press(&amx, &view, "Enter");
    amx.until("the agents back", || {
        amx.capture(&view).contains("one-a1b").then_some(())
    });
}

#[test]
fn enter_puts_the_agent_in_front_of_the_terminal() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    let holding = pane_field(&amx, &view, "#{session_name}");
    until_empty(&amx, &view);

    // Enter moves the client showing the view, so attach one.
    let terminal = watching(&amx, &holding);
    let tty = amx.until("a client on the view", || {
        let clients = clients_on(&amx, &holding);
        (!clients.is_empty()).then_some(clients)
    });

    an_agent_session(&amx, "fix-login-a1b");
    // A second window, left current in the agent's session; enter must still
    // show the agent's own pane.
    amx.tmux(&[
        "new-window",
        "-t",
        "amx-fix-login-a1b",
        "--",
        "sh",
        "-c",
        "while :; do sleep 0.05; done",
    ]);
    amx.until("the row", || row_of(&amx, &view, "fix-login-a1b").map(drop));

    press(&amx, &view, "Enter");
    amx.until("the agent on their screen", || {
        amx.capture(&terminal)
            .contains("the agent at work")
            .then_some(())
    });
    assert_eq!(
        clients_on(&amx, "amx-fix-login-a1b"),
        tty,
        "the client that was on the view is the one that moved"
    );

    // The view stayed in its own session, so switching back finds it.
    amx.tmux(&["switch-client", "-c", &tty, "-t", &holding]);
    amx.until("the list again", || {
        amx.capture(&terminal).contains("? keys").then_some(())
    });
    assert!(
        row_of(&amx, &terminal, "fix-login-a1b").is_some(),
        "with the agent still on it"
    );
}

#[test]
fn backspace_puts_the_cursor_on_the_agent_the_terminal_was_last_in() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    let holding = pane_field(&amx, &view, "#{session_name}");
    until_empty(&amx, &view);

    // The trail records where a client went, so attach one to the view.
    let terminal = watching(&amx, &holding);
    let tty = amx.until("a client on the view", || {
        let clients = clients_on(&amx, &holding);
        (!clients.is_empty()).then_some(clients)
    });

    an_agent_session(&amx, "fix-login-a1b");
    an_agent_session(&amx, "port-import-b2c");
    let drawn = amx.until("both rows", || {
        let drawn = amx.capture(&view);
        (drawn.contains("fix-login-a1b") && drawn.contains("port-import-b2c")).then_some(drawn)
    });
    // The row order is not under test: the cursor starts on whichever row is
    // drawn first, and one step down is the other.
    let (first, second) =
        match line_of(&drawn, "fix-login-a1b") < line_of(&drawn, "port-import-b2c") {
            true => ("fix-login-a1b", "port-import-b2c"),
            false => ("port-import-b2c", "fix-login-a1b"),
        };

    // Enter the agent under the cursor, then switch the client back to the
    // view's session.
    let go_in_and_out = |id: &str| {
        press(&amx, &view, "Enter");
        amx.until("the client on the agent", || {
            (clients_on(&amx, &format!("amx-{id}")) == tty).then_some(())
        });
        amx.tmux(&["switch-client", "-c", &tty, "-t", &holding]);
        amx.until("the list again", || {
            amx.capture(&terminal).contains("? keys").then_some(())
        });
    };

    go_in_and_out(first);
    press(&amx, &view, "Down");
    amx.until("the cursor on the other row", || {
        coloured_line(&amx, &view, second)
            .contains(&bar())
            .then_some(())
    });
    go_in_and_out(second);

    // The cursor's row is the agent last entered, so backspace goes to the
    // one before it.
    press(&amx, &view, "BSpace");
    amx.until("the cursor on the agent before it", || {
        coloured_line(&amx, &view, first)
            .contains(&bar())
            .then_some(())
    });
    assert!(
        !coloured_line(&amx, &view, second).contains(&bar()),
        "and off the row it was pressed on:\n{}",
        amx.capture(&view)
    );
}

#[test]
fn enter_lends_the_terminal_to_a_view_that_has_it_to_itself() {
    let amx = Harness::new();
    let view = outside_tmux(&amx);
    until_empty(&amx, &view);

    an_agent_session(&amx, "fix-login-a1b");
    amx.until("the row", || row_of(&amx, &view, "fix-login-a1b").map(drop));

    // Outside tmux there is no client to move, so the view lends its own
    // terminal to an attach.
    press(&amx, &view, "Enter");
    amx.until("the agent on the screen", || {
        amx.capture(&view)
            .contains("the agent at work")
            .then_some(())
    });

    // Detaching returns to the view, which waited for the attach to end.
    amx.tmux(&["detach-client", "-s", "amx-fix-login-a1b"]);
    amx.until("the list again", || {
        amx.capture(&view).contains("? keys").then_some(())
    });
    assert!(
        row_of(&amx, &view, "fix-login-a1b").is_some(),
        "with the agent still on it"
    );
}

#[test]
fn ctrl_z_in_the_session_gives_the_view_its_terminal_back() {
    let amx = Harness::new();
    let view = outside_tmux(&amx);
    until_empty(&amx, &view);

    an_agent_session(&amx, "fix-login-a1b");
    amx.until("the row", || row_of(&amx, &view, "fix-login-a1b").map(drop));

    press(&amx, &view, "Enter");
    amx.until("the agent on the screen", || {
        amx.capture(&view)
            .contains("the agent at work")
            .then_some(())
    });

    // The tmux client now in the view's terminal reads C-z, not the view, and
    // detaches, which hands the terminal back.
    press(&amx, &view, "C-z");
    amx.until("the list again", || {
        amx.capture(&view).contains("? keys").then_some(())
    });
    assert!(
        row_of(&amx, &view, "fix-login-a1b").is_some(),
        "with the agent still on it"
    );
}

#[test]
fn ctrl_z_in_the_session_moves_the_client_back_to_the_view_inside_tmux() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    let holding = pane_field(&amx, &view, "#{session_name}");
    until_empty(&amx, &view);

    let terminal = watching(&amx, &holding);
    let tty = amx.until("a client on the view", || {
        let clients = clients_on(&amx, &holding);
        (!clients.is_empty()).then_some(clients)
    });

    an_agent_session(&amx, "fix-login-a1b");
    amx.until("the row", || row_of(&amx, &view, "fix-login-a1b").map(drop));

    press(&amx, &view, "Enter");
    amx.until("the client on the agent", || {
        (clients_on(&amx, "amx-fix-login-a1b") == tty).then_some(())
    });

    // C-z at the client's own terminal switches it back to the view's
    // session.
    press(&amx, &terminal, "C-z");
    amx.until("the client on the view again", || {
        (clients_on(&amx, &holding) == tty).then_some(())
    });
    assert!(
        row_of(&amx, &view, "fix-login-a1b").is_some(),
        "with the agent still on it"
    );
}

#[test]
fn the_row_the_terminal_came_back_from_is_the_one_the_accent_marks() {
    // Two idle agents have identical rows, so only the accent can show which
    // one the terminal was in.
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");
    amx.play("port-import-b2c", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");
    amx.until_state("port-import-b2c", "idle");

    let view = outside_tmux(&amx);
    let drawn = amx.until("both rows", || {
        let drawn = amx.capture(&view);
        (drawn.contains("fix-login-a1b") && drawn.contains("port-import-b2c")).then_some(drawn)
    });
    // The first row is whichever turn ended last, so read it off the screen.
    let (went_into, stayed) =
        match line_of(&drawn, "fix-login-a1b") < line_of(&drawn, "port-import-b2c") {
            true => ("fix-login-a1b", "port-import-b2c"),
            false => ("port-import-b2c", "fix-login-a1b"),
        };

    // Outside tmux, enter lends the terminal to the agent's session. The
    // footer is the scenario's last line, so waiting for it waits for the
    // whole screen.
    press(&amx, &view, "Enter");
    amx.until("the agent's own screen", || {
        amx.capture(&view).contains("⏵⏵ auto mode on").then_some(())
    });

    // Detach with the tmux prefix and d.
    press(&amx, &view, "C-b");
    press(&amx, &view, "d");
    amx.until("the wall again", || {
        amx.capture(&view).contains("? keys").then_some(())
    });

    // Move the cursor away so the entered row shows the accent without the
    // bar.
    press(&amx, &view, "Down");
    amx.until("the cursor on the row nobody went into", || {
        coloured_line(&amx, &view, stayed)
            .contains(&bar())
            .then_some(())
    });

    let marked = coloured_line(&amx, &view, went_into);
    assert!(
        marked.contains(&foreground("accent")),
        "the name of the agent the terminal came back from is in the \
         accent:\n{marked:?}"
    );
    let rest = coloured_line(&amx, &view, stayed);
    assert!(
        !rest.contains(&foreground("accent")),
        "and the row nobody went into is the terminal's own:\n{rest:?}"
    );
}

#[test]
fn the_row_the_client_came_back_from_is_marked_inside_tmux_too() {
    // Inside tmux, enter moves the client to the agent's session while the
    // view keeps drawing. Switching back must mark the row as it is marked
    // outside tmux.
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");
    amx.play("port-import-b2c", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");
    amx.until_state("port-import-b2c", "idle");

    let view = amx.in_a_terminal(&[], &[]);
    let holding = pane_field(&amx, &view, "#{session_name}");
    // Enter needs a client to move.
    watching(&amx, &holding);
    let tty = amx.until("a client on the view", || {
        let clients = clients_on(&amx, &holding);
        (!clients.is_empty()).then_some(clients)
    });

    let drawn = amx.until("both rows", || {
        let drawn = amx.capture(&view);
        (drawn.contains("fix-login-a1b") && drawn.contains("port-import-b2c")).then_some(drawn)
    });
    let (went_into, stayed) =
        match line_of(&drawn, "fix-login-a1b") < line_of(&drawn, "port-import-b2c") {
            true => ("fix-login-a1b", "port-import-b2c"),
            false => ("port-import-b2c", "fix-login-a1b"),
        };

    // Harness panes are not in `amx-<id>` sessions, so ask the pane for its
    // session name.
    let into = pane_field(&amx, &amx.pane_of(went_into), "#{session_name}");
    press(&amx, &view, "Enter");
    amx.until("the client on the agent", || {
        (clients_on(&amx, &into) == tty).then_some(())
    });

    amx.tmux(&["switch-client", "-c", &tty, "-t", &holding]);
    amx.until("the client on the view again", || {
        (clients_on(&amx, &holding) == tty).then_some(())
    });

    press(&amx, &view, "Down");
    amx.until("the cursor on the row nobody went into", || {
        coloured_line(&amx, &view, stayed)
            .contains(&bar())
            .then_some(())
    });

    let marked = coloured_line(&amx, &view, went_into);
    assert!(
        marked.contains(&foreground("accent")),
        "the name of the agent the client came back from is in the \
         accent:\n{marked:?}"
    );
    let rest = coloured_line(&amx, &view, stayed);
    assert!(
        !rest.contains(&foreground("accent")),
        "and the row nobody went into is the terminal's own:\n{rest:?}"
    );
}

#[test]
fn ctrl_x_stops_the_agent_and_then_forgets_it() {
    let amx = Harness::new();
    let pane = amx.play("watch-log-e5f", "works-without-end");
    amx.until_state("watch-log-e5f", "working");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        amx.capture(&view).contains("watch-log-e5f").then_some(())
    });

    press(&amx, &view, "C-x");
    amx.until("the agent to stop", || {
        (amx.state("watch-log-e5f")["state"] == "stopped").then_some(())
    });
    // The record says stopped before the signal is sent, so the exit reads as
    // a stop and not a failure. The pane dies a moment later.
    amx.until("its pane to go with it", || {
        (!amx.pane_alive(&pane)).then_some(())
    });

    // On an ended agent ctrl+x forgets it, which takes two presses as it does
    // everywhere.
    twice(&amx, &view, "C-x");
    amx.until("the record to go", || agents(&amx).is_empty().then_some(()));
    until_empty(&amx, &view);
}

#[test]
fn i_cuts_short_the_turn_the_row_under_the_cursor_is_on() {
    let amx = Harness::new();
    let pane = amx.play("port-import-c3d", "interrupted");
    amx.until_state("port-import-c3d", "working");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || row_of(&amx, &view, "port-import-c3d"));
    // On the project axis the row shows its state as a word.
    press(&amx, &view, "C-s");
    amx.until("the working row", || {
        row_of(&amx, &view, "port-import-c3d").filter(|row| row.contains("working"))
    });

    // The cursor starts on the only row.
    press(&amx, &view, "i");
    let said = amx.until("what the view says it did", || {
        amx.capture(&view)
            .lines()
            .rfind(|line| line.contains("interrupted"))
            .map(str::to_string)
    });
    assert!(said.contains("interrupted port-import-c3d"), "{said}");

    // The key reached the vendor too: this scenario blocks on stdin until it
    // reads Escape, and only then draws its prompt.
    amx.until("the vendor to go back to its prompt", || {
        amx.capture(&pane).contains("⏵⏵").then_some(())
    });
    let row = amx.until("the row off the turn it was on", || {
        row_of(&amx, &view, "port-import-c3d").filter(|row| row.contains("done"))
    });
    assert!(!row.contains("working"), "{row}");

    // The press logs the same `interrupt` event as the verb, so a `result`
    // waiting on this turn learns no answer is coming.
    let kinds = amx.event_kinds("port-import-c3d");
    assert!(
        kinds.iter().any(|kind| kind == "interrupt"),
        "the turn the view cut short is on the log: {kinds:?}"
    );
}

#[test]
fn ctrl_x_arms_a_finished_row_and_says_so_where_its_summary_was() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row and what it did", || {
        row_of(&amx, &view, "fix-login-a1b")
            .filter(|row| row.contains("did what it was asked"))
            .map(|_| ())
    });

    press(&amx, &view, "C-x");
    let armed = amx.until("the warning where the summary was", || {
        coloured(&amx, &view)
            .lines()
            .find(|line| line.contains("ctrl+x again forgets"))
            .map(str::to_string)
    });
    assert!(
        armed.contains("fix-login-a1b"),
        "on the agent's own row rather than at the foot of the screen:\n{armed}"
    );
    assert!(
        !armed.contains("did what it was asked"),
        "in place of the summary rather than beside it:\n{armed}"
    );
    assert!(
        armed.contains(&foreground("waiting")),
        "in the colour of a thing waiting on a person:\n{armed}"
    );
    assert_eq!(
        agents(&amx),
        ["fix-login-a1b"],
        "and one press forgets nothing"
    );

    // The window times out and the summary comes back.
    amx.until("the summary to come back", || {
        row_of(&amx, &view, "fix-login-a1b")
            .filter(|row| row.contains("did what it was asked"))
            .map(|_| ())
    });
    assert_eq!(
        agents(&amx),
        ["fix-login-a1b"],
        "a window that closed forgets nothing either"
    );

    twice(&amx, &view, "C-x");
    amx.until("the record to go", || agents(&amx).is_empty().then_some(()));
    until_empty(&amx, &view);
}

#[test]
fn ctrl_x_on_a_heading_forgets_the_finished_and_keeps_the_work() {
    let amx = Harness::new();
    let repo = amx.a_repo();

    // One agent with uncommitted work in its own worktree, and one with no
    // worktree.
    let out = amx
        .amx_command(&[
            "new",
            "--name",
            "keeps-work-a1b",
            "--dir",
            &repo.to_string_lossy(),
            "--agent",
            &amx.mock(),
            "fix the login bug",
        ])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("finishes"))
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let tree = PathBuf::from(
        amx.meta("keeps-work-a1b")["worktree"]
            .as_str()
            .expect("a worktree"),
    );
    amx.until_state("keeps-work-a1b", "done");
    std::fs::write(tree.join("login.rs"), "fn login() {}\n").expect("the work in the tree");
    finished(&amx, "port-import-b2c", "done", 120);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = amx.capture(&view);
        (drawn.contains("keeps-work-a1b") && drawn.contains("port-import-b2c")).then_some(())
    });

    // Up from the first row is the group heading. One press arms every
    // finished row under it, and the warning shows on each row.
    press(&amx, &view, "Up");
    press(&amx, &view, "C-x");
    let armed = amx.until("the armed rows", || {
        let drawn = amx.capture(&view);
        drawn
            .contains("ctrl+x again stops and forgets")
            .then_some(drawn)
    });
    assert!(
        !armed.contains("forget 2 finished"),
        "the rows say it and the footer asks nothing:\n{armed}"
    );
    assert_eq!(agents(&amx).len(), 2, "and arming forgets nothing");

    // Two presses work whether or not the first window has lapsed: if open,
    // the first forgets; if lapsed, the first re-arms and the second forgets.
    twice(&amx, &view, "C-x");
    amx.until("the sweep", || {
        (agents(&amx) == ["keeps-work-a1b"]).then_some(())
    });
    assert!(
        tree.exists(),
        "the tree holding work nobody else has a copy of is still here"
    );
}

#[test]
fn ctrl_x_on_a_heading_arms_rows_in_every_state_before_it_stops_any() {
    let amx = Harness::new();
    // An idle agent and a finished one, both in the harness's home, so one
    // project heading covers both states.
    let pane = amx.play("fix-login-a1b", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");
    finished(&amx, "old-job-d4e", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = amx.capture(&view);
        (drawn.contains("fix-login-a1b") && drawn.contains("old-job-d4e")).then_some(())
    });
    press(&amx, &view, "C-s");
    amx.until("the project heading over both", || {
        amx.capture(&view)
            .lines()
            .any(|line| line.starts_with("~ "))
            .then_some(())
    });

    // Up from the first row is the heading. One press arms every row under
    // it, whatever its state, and stops nothing.
    press(&amx, &view, "Up");
    press(&amx, &view, "C-x");
    let armed = amx.until("both rows to be armed", || {
        let drawn = amx.capture(&view);
        (drawn.matches("ctrl+x again stops and forgets").count() == 2).then_some(drawn)
    });
    assert!(
        !armed.contains("has finished"),
        "the refusal went with the rule:\n{armed}"
    );
    assert_eq!(agents(&amx).len(), 2, "and arming forgets nothing");
    assert_eq!(
        amx.state("fix-login-a1b")["state"],
        "idle",
        "the press that armed the group stopped nothing in it"
    );
    assert!(
        amx.pane_alive(&pane),
        "the live agent is sitting at its prompt with its pane"
    );

    // Two presses work whether or not the first window has lapsed; either way
    // the live agent is stopped and both are forgotten.
    twice(&amx, &view, "C-x");
    amx.until("the group to be stopped and forgotten", || {
        agents(&amx).is_empty().then_some(())
    });
    amx.until("the live agent's pane to go with it", || {
        (!amx.pane_alive(&pane)).then_some(())
    });
    // The project axis has no empty-wall message, just "no agents".
    amx.until("the empty wall", || {
        amx.capture(&view).contains("no agents").then_some(())
    });
}

/// Two ended agents with work on their own branches: fix-login-a1b with a
/// merged request, and tidy-b2c merged by hand. Answers their worktrees in
/// that order.
fn two_agents_whose_work_landed(amx: &Harness, repo: &Path) -> (String, String) {
    let landed = an_ended_agent(amx, "fix-login-a1b", repo);
    work_on_the_branch(&landed, "login.rs");
    a_merged_request(amx, "fix-login-a1b", 12, &landed);

    let merged = an_ended_agent(amx, "tidy-b2c", repo);
    work_on_the_branch(&merged, "search.rs");
    merged_by_hand(repo, "tidy-b2c");

    (landed, merged)
}

/// Wait until both rows armed by `c` show their reason, and answer with the
/// screen.
fn until_armed(amx: &Harness, view: &str) -> String {
    amx.until("both rows to say why their work has landed", || {
        let drawn = amx.capture(view);
        (drawn.contains("c again clears · #12 merged")
            && drawn.contains("c again clears · amx/tidy-b2c merged into main"))
        .then_some(drawn)
    })
}

#[test]
fn c_clears_the_agents_whose_work_has_landed_and_says_why_on_each_row() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let (landed, merged) = two_agents_whose_work_landed(&amx, &repo);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = amx.capture(&view);
        (drawn.contains("fix-login-a1b") && drawn.contains("tidy-b2c")).then_some(())
    });

    // One press arms every row the sweep found, wherever the cursor is, and
    // each row shows its reason in place of its summary.
    press(&amx, &view, "c");
    let armed = until_armed(&amx, &view);
    assert!(
        !armed.contains("ctrl+x again"),
        "the rows name the key that armed them and not the other one:\n{armed}"
    );
    assert_eq!(
        agents(&amx),
        ["fix-login-a1b", "tidy-b2c"],
        "and one press clears nothing:\n{armed}"
    );

    // A second press within the window removes each record, worktree and
    // branch, and the footer shows the count.
    press(&amx, &view, "c");
    amx.until("the records to go", || {
        agents(&amx).is_empty().then_some(())
    });
    amx.until("the count", || {
        amx.capture(&view).contains("cleared 2").then_some(())
    });
    for tree in [&landed, &merged] {
        assert!(!Path::new(tree).exists(), "{tree} went with its record");
    }
    let left = git(&repo, &["branch", "--list"]);
    assert!(!left.contains("amx/"), "and so did their branches: {left}");
}

#[test]
fn c_whose_window_lapses_keeps_every_agent_it_marked() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let (landed, merged) = two_agents_whose_work_landed(&amx, &repo);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = amx.capture(&view);
        (drawn.contains("fix-login-a1b") && drawn.contains("tidy-b2c")).then_some(())
    });
    press(&amx, &view, "c");
    until_armed(&amx, &view);

    // With no second press the window times out and the summaries return.
    let back = amx.until("the summaries to come back", || {
        let drawn = amx.capture(&view);
        (!drawn.contains("c again clears")).then_some(drawn)
    });
    assert!(
        back.contains("fix-login-a1b") && back.contains("tidy-b2c"),
        "both agents are still on the wall:\n{back}"
    );
    assert_eq!(agents(&amx), ["fix-login-a1b", "tidy-b2c"]);
    for tree in [&landed, &merged] {
        assert!(Path::new(tree).exists(), "{tree} stands");
    }
    let left = git(&repo, &["branch", "--list"]);
    for id in ["fix-login-a1b", "tidy-b2c"] {
        assert!(
            left.contains(&format!("amx/{id}")),
            "and its branch: {left}"
        );
    }
}

#[test]
fn c_keeps_back_the_landed_agent_whose_tree_holds_work_and_counts_it() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let (clean, holding) = two_agents_whose_work_landed(&amx, &repo);

    // Both branches are merged, but the second worktree has an uncommitted
    // file. The sweep keeps that tree and its record, and the view says so
    // before the clearing press.
    std::fs::write(Path::new(&holding).join("notes.md"), "half an idea\n")
        .expect("a file nobody committed");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = amx.capture(&view);
        (drawn.contains("fix-login-a1b") && drawn.contains("tidy-b2c")).then_some(())
    });

    // The first press checks each tree with git. The held row says why it
    // will be kept where the others say what the next press does.
    press(&amx, &view, "c");
    let armed = amx.until("the row that will be kept to say why", || {
        let drawn = amx.capture(&view);
        drawn.contains("has uncommitted changes").then_some(drawn)
    });
    let held_row = row_of(&amx, &view, "tidy-b2c").expect("the row still on the wall");
    assert!(
        held_row.contains("has uncommitted changes") && !held_row.contains("c again clears"),
        "the held row says why rather than promising a press that passes it \
         by:\n{armed}"
    );
    assert!(
        row_of(&amx, &view, "fix-login-a1b")
            .is_some_and(|row| row.contains("c again clears · #12 merged")),
        "and the row with nothing uncommitted in it says what it said:\n{armed}"
    );

    // The second press clears the clean row and skips the held one; the
    // footer counts both and says why one was kept.
    press(&amx, &view, "c");
    amx.until("the count of what went and what was kept", || {
        amx.capture(&view)
            .contains("cleared 1 · kept 1 with uncommitted changes")
            .then_some(())
    });
    assert_eq!(
        agents(&amx),
        ["tidy-b2c"],
        "the kept agent keeps its record, since the tree is where its work is"
    );
    assert!(!Path::new(&clean).exists(), "{clean} went with its record");
    assert!(
        Path::new(&holding).join("notes.md").exists(),
        "and the uncommitted file is where somebody left it"
    );
    let after = amx.capture(&view);
    assert!(
        after.contains("tidy-b2c"),
        "the kept agent is still a row on the wall:\n{after}"
    );
}

/// Add a bare `origin` remote to `repo` with `main` pushed to it, and answer
/// with its path.
fn an_origin(amx: &Harness, repo: &Path) -> PathBuf {
    let bare = amx.home().join("origin.git");
    std::fs::create_dir_all(&bare).expect("the origin");
    git(&bare, &["init", "--bare", "-b", "main"]);
    git(repo, &["remote", "add", "origin", &bare.to_string_lossy()]);
    git(repo, &["push", "-q", "origin", "main"]);
    bare
}

/// The `%(upstream:track)` of the agent's branch in `repo`.
///
/// After the remote branch is deleted this stays empty until a fetch, then
/// reads `[gone]`.
fn upstream_track(repo: &Path, id: &str) -> String {
    git(
        repo,
        &[
            "for-each-ref",
            "--format=%(upstream:track)",
            &format!("refs/heads/amx/{id}"),
        ],
    )
    .trim()
    .to_string()
}

#[test]
fn c_reads_a_branch_the_forge_deleted_because_the_view_fetched_for_it() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let origin = an_origin(&amx, &repo);
    let tree = an_ended_agent(&amx, "tidy-b2c", &repo);
    work_on_the_branch(&tree, "search.rs");
    git(
        Path::new(&tree),
        &["push", "-q", "-u", "origin", "amx/tidy-b2c"],
    );

    // As after a squash merge: the branch's commits are not in main, so it
    // does not read as merged, and the forge deleted it. Only the view's own
    // fetch can make git report the branch as gone.
    git(&origin, &["branch", "-D", "amx/tidy-b2c"]);
    assert_eq!(
        upstream_track(&repo, "tidy-b2c"),
        "",
        "until something fetches, git has the origin holding the branch"
    );

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        amx.capture(&view).contains("tidy-b2c").then_some(())
    });
    amx.until("the view's own fetch to record the delete", || {
        (upstream_track(&repo, "tidy-b2c") == "[gone]").then_some(())
    });

    // `c` does not fetch; it reads what the view's fetch recorded.
    press(&amx, &view, "c");
    let armed = amx.until("the row to say why its work has landed", || {
        let drawn = amx.capture(&view);
        drawn
            .contains("c again clears · amx/tidy-b2c gone from origin")
            .then_some(drawn)
    });
    assert_eq!(
        agents(&amx),
        ["tidy-b2c"],
        "and one press clears nothing:\n{armed}"
    );
}

#[test]
fn space_writes_the_look_on_the_record_and_leaves_the_rows_alone() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);
    finished(&amx, "port-import-b2c", "done", 120);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = amx.capture(&view);
        (drawn.contains("fix-login-a1b") && drawn.contains("port-import-b2c")).then_some(())
    });

    // The cursor starts on the most recently ended row. Being read shows
    // nowhere on the wall, so check `seen` in the record.
    press(&amx, &view, "Space");
    amx.until("the look to reach the record", || {
        (amx.state("fix-login-a1b")["seen"].as_u64().unwrap_or(0) > 0).then_some(())
    });
    let carded = |drawn: &str| {
        drawn
            .lines()
            .any(|line| line.starts_with("∙ fix-login-a1b · claude ┈"))
    };
    amx.until("the card", || carded(&amx.capture(&view)).then_some(()));

    // With a card open the whole wall is dim, cursor row included, as under a
    // task line. Read the whole screen since the dim is set once at the top;
    // each id's first match is its row, above the card's rule.
    let whole = coloured(&amx, &view);
    for id in ["fix-login-a1b", "port-import-b2c"] {
        let on = sgr_at(&whole, id);
        assert!(
            on.contains(&2) && !on.contains(&1),
            "{id} is dim under the card and carries no weight:\n{}",
            amx.capture(&view)
        );
    }

    // After esc the rows look as before the press: the cursor row at normal
    // intensity and the other dim. Having been read changes nothing on screen.
    press(&amx, &view, "Escape");
    amx.until("the card put away", || {
        (!carded(&amx.capture(&view))).then_some(())
    });
    let whole = coloured(&amx, &view);
    let opened = sgr_at(&whole, "fix-login-a1b");
    assert!(
        !opened.contains(&1) && !opened.contains(&2),
        "the row the card was on is the row the cursor is on, and it reads as \
         it did before the press:\n{}",
        amx.capture(&view)
    );
    let untouched = sgr_at(&whole, "port-import-b2c");
    assert!(
        untouched.contains(&2) && !untouched.contains(&1),
        "and the row nobody opened is as quiet as it always was:\n{}",
        amx.capture(&view)
    );
}
