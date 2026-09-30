//! End-to-end tests for the view's input line and header: the composer that
//! starts agents (tokens, completion, pastes, editing), the header's dials,
//! and the find, fork, rename and reply lines.

mod common;

use common::{
    Harness, a_repo_at, agents, bar, coloured, coloured_line, command_of, finished, foreground,
    git, in_force, pane_field, press, sgr_at, starts_at, types, until_empty,
};
use serde_json::{Value, json};

/// Whether `line` is a wall row showing `name` as its agent's name.
///
/// The name starts after the gutter, the mark and a space. A notice quoting
/// the same word starts at the left edge and does not match.
fn a_row_called(line: &str, name: &str) -> bool {
    line.chars().skip(3).collect::<String>().starts_with(name)
}

/// The SGR attributes in force on the cell just after `word`, where the block
/// cursor sits at the end of a typed line.
///
/// Escapes written right after `word` paint that cell, so they are included.
fn sgr_past(line: &str, word: &str) -> Vec<u16> {
    let mut end = starts_at(line, word) + word.len();
    while let Some(rest) = line[end..].strip_prefix("\u{1b}[") {
        let Some(over) = rest.find('m') else { break };
        end += "\u{1b}[".len() + over + 1;
    }
    in_force(&line[..end])
}

/// Paste `text` into the view as one bracketed paste, with LF left as LF.
fn pastes(amx: &Harness, view: &str, text: &str) {
    amx.tmux(&["set-buffer", "--", text]);
    amx.tmux(&["paste-buffer", "-p", "-r", "-t", view]);
}

/// Twenty numbered lines, more than the composer shows at once.
fn twenty_rows() -> String {
    (1..=20)
        .map(|n| format!("row-{n:02}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Open a view whose spawns run mock-claude playing `scenario`, with
/// worktrees off.
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

/// Open a view with `agent = "claude"` and `config`, where `claude` on PATH
/// is mock-claude.
///
/// The vendor registry declares dials for claude, which an unregistered
/// command does not get.
fn a_view_that_dispatches_as_claude(amx: &Harness, config: &str) -> String {
    a_view_that_can_start_claude(amx, &format!("agent = \"claude\"\n{config}"))
}

/// Open a view with `config` as the whole config file, where `claude` on PATH
/// is mock-claude playing `happy-turn`.
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

/// Wait for the one agent the view started to have a pane in its meta, and
/// answer with its id.
///
/// The agent directory appears earlier: `new` starts the pane first so the
/// record it writes can name it.
fn composed(amx: &Harness) -> String {
    amx.until("the agent to be started", || {
        let started = agents(amx);
        let id = (started.len() == 1).then(|| started[0].clone())?;
        amx.meta(&id)["pane"].as_str().map(|_| id)
    })
}

/// Like [`composed`], for an agent other than `first`.
fn composed_after(amx: &Harness, first: &str) -> String {
    amx.until("the next agent to be started", || {
        let id = agents(amx).into_iter().find(|id| id != first)?;
        amx.meta(&id)["pane"].as_str().map(|_| id)
    })
}

/// Write an `$EDITOR` script that prints a line and blocks until
/// `$HOME/let-it-go` exists, and answer with its path.
///
/// It sets no terminal modes, so the pane keeps whatever state the view left
/// the terminal in when it handed it over.
fn an_editor_that_waits(amx: &Harness) -> String {
    let path = amx.home().join("editor");
    std::fs::write(
        &path,
        "#!/bin/sh\n\
         printf 'the editor has the screen\\n'\n\
         while [ ! -e \"$HOME/let-it-go\" ]; do sleep 0.05; done\n",
    )
    .expect("an editor to lend the terminal to");
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("an editor that runs");
    path.to_string_lossy().into_owned()
}

#[test]
fn header_says_what_the_next_agent_will_be_started_with() {
    let amx = Harness::new();
    amx.config("agent = \"claude\"\nmax_agents = 3\n");
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    let drawn = amx.until("the header", || {
        let drawn = amx.capture(&view);
        drawn.contains("└ next").then_some(drawn)
    });

    assert!(
        drawn.contains("AMX  ~"),
        "whose screen this is and where it was opened:\n{drawn}"
    );
    assert!(
        !drawn.contains(env!("CARGO_PKG_VERSION")),
        "and not which build it is, which says nothing about the fleet:\n{drawn}"
    );
    assert!(
        drawn.contains("1 running    1 WAITING"),
        "what is running, and the count that wants a person set apart from \
         it:\n{drawn}"
    );
    assert!(
        !drawn.contains("1/3 running"),
        "max_agents is one project's cap, and this view is about every agent \
         on the machine:\n{drawn}"
    );
    assert!(
        drawn.contains("└ next  claude   model  default   permission  default   worktree  new"),
        "and under it the vendor, the dials it will be given, and whether it \
         is cut a tree of its own:\n{drawn}"
    );
}

#[test]
fn header_counts_the_machine_against_max_total_where_somebody_set_one() {
    let amx = Harness::new();
    amx.config("agent = \"claude\"\nmax_agents = 3\nmax_total = 2\n");
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    let drawn = amx.until("the header", || {
        let drawn = amx.capture(&view);
        drawn.contains("└ next").then_some(drawn)
    });

    assert!(
        drawn.contains("1/2 running"),
        "the one ceiling a view about every agent can be read against:\n{drawn}"
    );
}

#[test]
fn header_opens_a_view_on_a_project_under_that_projects_own_file() {
    let amx = Harness::new();
    amx.config("agent = \"claude\"\nmodel = \"opus\"\nmax_agents = 9\n");

    let repo = amx.home().join("elsewhere");
    std::fs::create_dir_all(repo.join(".amx")).expect("the project");
    a_repo_at(&repo);
    std::fs::write(
        repo.join(".amx/config.toml"),
        "model = \"fable\"\nworktrees = false\nmax_agents = 3\n",
    )
    .expect("the project's own config");
    let allowed = amx.amx(&["allow", "--dir", &repo.to_string_lossy()]);
    assert!(allowed.status.success(), "amx allow: {:?}", allowed);

    // One agent in the project and one outside it, so the header's count
    // shows whether it counts only the project.
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");
    amx.set_meta("ask-a1b", json!({ "dir": repo }));
    amx.play("busy-b2c", "works-without-end");
    amx.until_state("busy-b2c", "working");

    let view = amx.in_a_terminal(&[], &["--dir", &repo.to_string_lossy()]);
    let drawn = amx.until("the header", || {
        let drawn = amx.capture(&view);
        drawn.contains("└ next").then_some(drawn)
    });

    assert!(
        drawn.contains("└ next  claude   model  fable   permission  default   worktree  none"),
        "the dials are the project's, laid over the person's:\n{drawn}"
    );
    assert!(
        drawn.contains("1/3 running"),
        "and this project's agents are counted against this project's own \
         cap:\n{drawn}"
    );
}

#[test]
fn a_finished_turn_is_summarised_by_the_command_its_own_project_names() {
    let amx = Harness::new();
    // `summary_command` is an ordinary config key, so a project config can
    // override it. Deterministic `sed` commands stand in for a model call.
    amx.config("summary_command = \"sed s/^/mine:/\"\n");

    let repo = amx.home().join("elsewhere");
    std::fs::create_dir_all(repo.join(".amx")).expect("the project");
    a_repo_at(&repo);
    std::fs::write(
        repo.join(".amx/config.toml"),
        "summary_command = \"sed s/^/theirs:/\"\n",
    )
    .expect("the project's own config");
    let allowed = amx.amx(&["allow", "--dir", &repo.to_string_lossy()]);
    assert!(allowed.status.success(), "amx allow: {:?}", allowed);

    // One finished agent in the project and one outside it, so each summary
    // must come from the config its own agent ran under.
    finished(&amx, "port-cli-b2c", "done", 60);
    amx.set_meta("port-cli-b2c", json!({ "dir": repo }));
    finished(&amx, "old-job-c3d", "done", 30);

    let _view = amx.in_a_terminal(&[], &[]);
    let summary = |id: &str| {
        amx.until(&format!("the line {id} is worth"), || {
            amx.state(id)["summary"].as_str().map(str::to_string)
        })
    };

    assert_eq!(
        summary("port-cli-b2c"),
        "theirs:did what it was asked",
        "the command is the one the project the turn ran in names"
    );
    assert_eq!(
        summary("old-job-c3d"),
        "mine:did what it was asked",
        "and a turn in no project of its own is still the person's"
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
        amx.capture(&view)
            .contains("└ next  claude   model  default")
            .then_some(())
    });

    press(&amx, &view, "M-m");
    amx.until("the model dial to turn", || {
        amx.capture(&view)
            .contains("└ next  claude   model  fable")
            .then_some(())
    });

    press(&amx, &view, "M-w");
    let drawn = amx.until("the worktree dial to turn", || {
        let drawn = amx.capture(&view);
        drawn.contains("worktree  none").then_some(drawn)
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
fn a_filter_line_narrows_the_axis_instead_of_starting_an_agent() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("fix-login-b2c", "happy-turn");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("fix-login-b2c", "idle");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both agents", || {
        let drawn = amx.capture(&view);
        (drawn.contains("ask-a1b") && drawn.contains("fix-login-b2c")).then_some(())
    });

    // Only the find line filters, and it applies tokens on each keystroke.
    types(&amx, &view, "/");
    types(&amx, &view, "s:waiting");
    let drawn = amx.until("the narrowed list", || {
        let drawn = amx.capture(&view);
        (drawn.contains("ask-a1b") && !drawn.contains("fix-login-b2c")).then_some(drawn)
    });
    assert_eq!(
        agents(&amx).len(),
        2,
        "and nothing was started with the line:\n{drawn}"
    );

    // Enter closes the line and keeps the filter.
    press(&amx, &view, "Enter");
    let kept = amx.until("the line to go", || {
        let drawn = amx.capture(&view);
        drawn.contains("space card").then_some(drawn)
    });
    assert!(
        kept.contains("s:waiting") && !kept.contains("fix-login-b2c"),
        "with the header saying what the wall is narrowed to:\n{kept}"
    );

    // Esc on the list clears a filter left by a closed find line.
    press(&amx, &view, "Escape");
    amx.until("the whole fleet again", || {
        amx.capture(&view).contains("fix-login-b2c").then_some(())
    });
}

#[test]
fn a_filter_line_of_two_words_keeps_the_groups_both_of_them_name() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("busy-b2c", "works-without-end");
    amx.play("fix-login-c3d", "happy-turn");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("busy-b2c", "working");
    amx.until_state("fix-login-c3d", "idle");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("all three agents", || {
        let drawn = amx.capture(&view);
        (drawn.contains("ask-a1b") && drawn.contains("busy-b2c") && drawn.contains("fix-login-c3d"))
            .then_some(())
    });

    types(&amx, &view, "/");
    types(&amx, &view, "s:waiting s:working");
    let drawn = amx.until("the narrowed list", || {
        let drawn = amx.capture(&view);
        (drawn.contains("ask-a1b")
            && drawn.contains("busy-b2c")
            && !drawn.contains("fix-login-c3d"))
        .then_some(drawn)
    });
    assert!(
        drawn.contains("Needs input") && drawn.contains("Working"),
        "with both groups still headed over their agents:\n{drawn}"
    );

    // After Enter the header shows the whole filter as typed.
    press(&amx, &view, "Enter");
    let kept = amx.until("the line to go", || {
        let drawn = amx.capture(&view);
        drawn.contains("space card").then_some(drawn)
    });
    assert!(
        kept.contains("s:waiting s:working") && !kept.contains("fix-login-c3d"),
        "read back word for word as it was typed:\n{kept}"
    );
}

#[test]
fn the_composer_starts_an_agent_on_a_line_of_state_tokens() {
    let amx = Harness::new();
    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");

    // `s:` tokens filter only on the `/` line. On the task line they are
    // plain task text.
    types(&amx, &view, "n");
    types(&amx, &view, "s:waiting");
    let drawn = amx.until("the task on the screen", || {
        let drawn = amx.capture(&view);
        drawn.contains("❯ s:waiting").then_some(drawn)
    });
    assert!(
        drawn.contains("TASK ·") && !drawn.contains("NARROW"),
        "the rule over the line says what enter is about to do:\n{drawn}"
    );
    assert!(
        drawn.contains("enter starts it"),
        "and so does the row under it:\n{drawn}"
    );

    press(&amx, &view, "Enter");
    let id = composed(&amx);
    assert_eq!(
        command_of(&amx, &id).last().map(String::as_str),
        Some("s:waiting"),
        "and the vendor is handed the line whole: {:?}",
        command_of(&amx, &id)
    );
}

#[test]
fn header_reads_as_chrome_with_its_one_colour_on_what_wants_a_person() {
    let amx = Harness::new();
    amx.config("agent = \"claude\"\n");
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the header", || {
        amx.capture(&view).contains("└ next").then_some(())
    });

    let drawn = coloured(&amx, &view);
    let lines: Vec<&str> = drawn.lines().collect();
    assert!(
        lines[0].contains(&foreground("waiting")),
        "the count that wants somebody is painted for it:\n{:?}",
        lines[0]
    );
    let badge = sgr_at(lines[0], " 1 WAITING");
    assert!(
        badge.contains(&7) && badge.contains(&1),
        "and set in reverse video, which nothing else on the screen is: \
         {badge:?} in {:?}",
        lines[0]
    );
    assert!(
        sgr_at(lines[0], "AMX").contains(&1),
        "the tool's name is the other thing up here carrying weight:\n{:?}",
        lines[0]
    );
    assert!(
        sgr_at(lines[0], "~").contains(&2),
        "and where the view is is chrome:\n{:?}",
        lines[0]
    );

    // On the dials row the labels are dim chrome; only the values carry
    // colour. Read attributes across both rows: tmux writes an escape only
    // where the style changes, so a row can inherit it from the one above.
    let header = lines[..2].concat();
    assert!(
        sgr_at(&header, "next").contains(&2) && !sgr_at(&header, "next").contains(&1),
        "the label is dim, and carries no weight of its own:\n{header:?}"
    );
}

#[test]
fn the_composer_starts_an_agent_where_the_view_is() {
    let amx = Harness::new();
    let view = a_view_that_dispatches(&amx, "happy-turn");

    types(&amx, &view, "n");
    types(&amx, &view, "port the importer");
    amx.until("the task on the screen", || {
        amx.capture(&view)
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
        pane_field(&amx, &pane, "#{session_name}"),
        format!("amx-{id}"),
        "in a session of its own, leaving the view where it was"
    );

    amx.until_state(&id, "idle");
    amx.until("the agent's own row", || {
        amx.capture(&view).contains(&id).then_some(())
    });
}

#[test]
fn the_task_line_brings_back_the_tasks_sent_before_on_the_arrows_in_this_view_and_the_next() {
    let amx = Harness::new();
    let view = a_view_that_dispatches(&amx, "happy-turn");

    types(&amx, &view, "n");
    types(&amx, &view, "port the importer");
    press(&amx, &view, "Enter");
    let id = composed(&amx);
    assert!(id.starts_with("port-the-importer"), "{id}");
    amx.until("the agent's own row", || {
        amx.capture(&view).contains("port-the-import").then_some(())
    });

    // Match on the `❯` prompt, which the agent's row above does not have.
    types(&amx, &view, "n");
    press(&amx, &view, "Up");
    amx.until("the task sent before, back on the line", || {
        amx.capture(&view)
            .contains("❯ port the importer")
            .then_some(())
    });
    // Down past the newest entry returns to the empty line, still open.
    press(&amx, &view, "Down");
    amx.until("the empty line again", || {
        let drawn = amx.capture(&view);
        (drawn.contains("TASK") && !drawn.contains("❯ port the importer")).then_some(())
    });

    // History is saved with the wall's layout, so a new view has it too.
    // Wait for the line to close before q, or Esc and q arrive as alt+q.
    press(&amx, &view, "Escape");
    amx.until("the line to close", || {
        (!amx.capture(&view).contains("TASK")).then_some(())
    });
    // Keep the pane after the view exits so the test can wait for it to die.
    amx.tmux(&["set-option", "-w", "-t", &view, "remain-on-exit", "on"]);
    press(&amx, &view, "q");
    amx.until("the view to close", || {
        (pane_field(&amx, &view, "#{pane_dead}") == "1").then_some(())
    });
    let next = amx.in_a_terminal(&[], &[]);
    // Match the prefix that fits the name column. The wall elides the rest of
    // the id, and only the view that started the agent has a notice naming it
    // in full.
    amx.until("the row in the next view", || {
        amx.capture(&next).contains("port-the-import").then_some(())
    });
    types(&amx, &next, "n");
    press(&amx, &next, "Up");
    amx.until("the task sent before, in the next view", || {
        amx.capture(&next)
            .contains("❯ port the importer")
            .then_some(())
    });
}

#[test]
fn the_cursor_lands_on_the_agent_the_line_started() {
    let amx = Harness::new();
    let view = a_view_that_dispatches(&amx, "happy-turn");

    // An existing agent for the cursor to start on, so the test sees it move.
    finished(&amx, "fix-login-a1b", "done", 60);
    amx.until("the cursor on the agent already there", || {
        coloured(&amx, &view)
            .lines()
            .any(|line| line.contains("fix-login-a1b") && line.contains(&bar()))
            .then_some(())
    });

    types(&amx, &view, "n");
    types(&amx, &view, "port it");
    amx.until("the task on the screen", || {
        amx.capture(&view).contains("❯ port it").then_some(())
    });
    press(&amx, &view, "Enter");

    // The row appears on the next read of the records, with the cursor on it.
    let id = composed_after(&amx, "fix-login-a1b");
    amx.until("the bar on the row of the agent the line started", || {
        coloured(&amx, &view)
            .lines()
            .any(|line| line.contains(&id) && line.contains(&bar()))
            .then_some(())
    });
    assert!(
        !coloured_line(&amx, &view, "fix-login-a1b").contains(&bar()),
        "one cursor, and it is on the agent that was just started"
    );
}

#[test]
fn f_starts_a_copy_of_the_agent_under_the_cursor_on_the_task_the_line_says() {
    let amx = Harness::new();
    let view = a_view_that_dispatches(&amx, "happy-turn");

    // Start the origin from the view: a fork needs the session id the vendor
    // announced, which a hand-written record lacks.
    types(&amx, &view, "n");
    types(&amx, &view, "port it");
    press(&amx, &view, "Enter");
    let origin = composed(&amx);
    amx.until("the session the vendor announced", || {
        amx.meta(&origin)["session"].as_str().map(str::to_string)
    });

    // The fork line names the agent it copies, whatever the cursor does while
    // the task is typed.
    press(&amx, &view, "f");
    types(&amx, &view, "use sqlite");
    let drawn = amx.until("the fork line", || {
        let drawn = amx.capture(&view);
        drawn.contains("❯ use sqlite").then_some(drawn)
    });
    assert!(
        drawn.contains(&format!("FORK · {origin}")),
        "the rule over the line names the agent the copy is of:\n{drawn}"
    );

    press(&amx, &view, "Enter");
    let copy = composed_after(&amx, &origin);
    let argv = command_of(&amx, &copy);
    assert!(
        argv.contains(&"--fork-session".to_string()),
        "the copy is asked of the vendor as a branch of the session: {argv:?}"
    );
    assert_eq!(
        argv.last().map(String::as_str),
        Some("use sqlite"),
        "with what was typed as its first turn: {argv:?}"
    );
    // The record is written before the frame with the notice, and CI can
    // read in between.
    amx.until("the view saying both agents", || {
        amx.capture(&view)
            .contains(&format!("forked {origin} as {copy}"))
            .then_some(())
    });

    // The cursor moves to the copy's row once the row appears.
    amx.until("the cursor on the row of the copy", || {
        coloured(&amx, &view)
            .lines()
            .any(|line| line.contains(&copy) && line.contains(&bar()))
            .then_some(())
    });

    // Fork the copy on an empty line: the new agent gets the conversation and
    // no first prompt.
    amx.until("the copy's own session", || {
        amx.meta(&copy)["session"].as_str().map(str::to_string)
    });
    press(&amx, &view, "f");
    press(&amx, &view, "Enter");
    let waiting = amx.until("the copy of the copy", || {
        let id = agents(&amx)
            .into_iter()
            .find(|id| id != &origin && id != &copy)?;
        amx.meta(&id)["pane"].as_str().map(|_| id)
    });
    assert_eq!(
        amx.meta(&waiting)["task"],
        amx.meta(&copy)["task"],
        "a copy given nothing is about what the agent it came from was about"
    );
    let argv = command_of(&amx, &waiting);
    assert_eq!(
        argv.last().map(String::as_str),
        Some("--fork-session"),
        "and nothing is put to it: {argv:?}"
    );
}

#[test]
fn f_on_a_command_row_says_there_is_no_conversation_to_copy() {
    let amx = Harness::new();
    let view = a_view_that_dispatches(&amx, "happy-turn");

    // A `!` command row. The cursor moves to it once it starts.
    types(&amx, &view, "n");
    types(&amx, &view, "!echo hi");
    press(&amx, &view, "Enter");
    let id = composed(&amx);
    amx.until("the cursor on the command row", || {
        coloured(&amx, &view)
            .lines()
            .any(|line| line.contains(&id) && line.contains(&bar()))
            .then_some(())
    });

    // A command row has no vendor session to fork, so f opens no line.
    press(&amx, &view, "f");
    let drawn = amx.until("what the key said about a command row", || {
        let drawn = amx.capture(&view);
        drawn
            .contains(&format!("{id} is a command, not an agent"))
            .then_some(drawn)
    });
    assert!(
        !drawn.contains("FORK ·"),
        "and nothing was opened to type at:\n{drawn}"
    );
}

#[test]
fn the_composer_starts_an_agent_in_the_project_the_cursor_is_under() {
    let amx = Harness::new();
    let view = a_view_that_dispatches(&amx, "happy-turn");

    // Two projects, each with one agent: the view's directory and `~/api`.
    let api = amx.home().join("api");
    std::fs::create_dir_all(&api).expect("the second project");
    finished(&amx, "here-a1b", "done", 30);
    finished(&amx, "api-b2c", "done", 60);
    amx.set_meta("api-b2c", json!({ "dir": api }));
    amx.until("both agents", || {
        let drawn = amx.capture(&view);
        (drawn.contains("here-a1b") && drawn.contains("api-b2c")).then_some(())
    });

    // Group by project, then put the cursor on the `~/api` heading: G goes to
    // its agent on the last row, and k steps up to the heading.
    press(&amx, &view, "C-s");
    amx.until("the project headings", || {
        amx.capture(&view).contains("~/api").then_some(())
    });
    press(&amx, &view, "G");
    press(&amx, &view, "k");

    types(&amx, &view, "n");
    types(&amx, &view, "port the importer");
    let drawn = amx.until("the task on the screen", || {
        let drawn = amx.capture(&view);
        drawn.contains("port the importer").then_some(drawn)
    });
    assert!(
        drawn.contains("TASK · in ~/api"),
        "the rule says where the line will run:\n{drawn}"
    );
    press(&amx, &view, "Enter");

    let id = amx.until("the agent to be started", || {
        let id = agents(&amx)
            .into_iter()
            .find(|id| id.starts_with("port-the-importer"))?;
        amx.meta(&id)["pane"].as_str().map(|_| id)
    });
    assert_eq!(
        amx.meta(&id)["dir"],
        api.to_string_lossy().as_ref(),
        "an agent starts in the project its line was opened under"
    );
}

#[test]
fn a_view_opened_about_a_directory_stands_in_it() {
    let amx = Harness::new();
    amx.config(&format!("agent = \"{}\"\nworktrees = false\n", amx.mock()));
    let repo = amx.home().join("elsewhere");
    std::fs::create_dir_all(&repo).expect("the project");
    let scenario = amx.scenario("happy-turn").to_string_lossy().into_owned();
    let transcript = amx
        .home()
        .join("composed.jsonl")
        .to_string_lossy()
        .into_owned();

    // The pane starts in home; `--dir` points the view at a project under it.
    let view = amx.in_a_terminal(
        &[
            ("MOCK_CLAUDE_SCENARIO", &scenario),
            ("MOCK_CLAUDE_TRANSCRIPT", &transcript),
        ],
        &["--dir", &repo.to_string_lossy()],
    );
    until_empty(&amx, &view);
    let drawn = amx.capture(&view);
    assert!(
        drawn.contains("AMX  ~/elsewhere"),
        "the header names the directory the view was opened about:\n{drawn}"
    );

    types(&amx, &view, "n");
    types(&amx, &view, "port the importer");
    press(&amx, &view, "Enter");

    let id = composed(&amx);
    assert_eq!(
        amx.meta(&id)["dir"],
        repo.to_string_lossy().as_ref(),
        "and a task typed at it starts there"
    );
}

#[test]
fn the_composer_folds_a_long_paste_and_starts_the_task_it_stands_for() {
    let amx = Harness::new();
    let view = a_view_that_dispatches(&amx, "happy-turn");

    // Pasted at the list. Without bracketed paste every character would be
    // read as a key and the first newline would dispatch.
    let pasted = format!("{}\n", twenty_rows());
    pastes(&amx, &view, &pasted);

    let drawn = amx.until("the marker the paste folded into", || {
        let drawn = amx.capture(&view);
        drawn.contains("[Pasted text #1]").then_some(drawn)
    });
    assert!(
        drawn.contains("❯ [Pasted text #1]"),
        "twenty rows stand on the line as the one row that names them:\n{drawn}"
    );
    assert!(
        !drawn.contains("row-01") && !drawn.contains("row-20"),
        "and none of what the marker is holding is on the screen:\n{drawn}"
    );
    assert!(
        agents(&amx).is_empty(),
        "a paste is one edit, its own last newline included:\n{drawn}"
    );

    // Enter dispatches once, with the pasted text in place of the marker.
    press(&amx, &view, "Enter");
    let id = composed(&amx);
    assert_eq!(
        amx.meta(&id)["task"].as_str(),
        Some(pasted.as_str()),
        "one task, with every line of the paste in it"
    );
}

#[test]
fn the_composer_grows_to_its_cap_as_a_line_is_broken() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);

    // Build the rows with ctrl+j, since a paste would fold into a marker.
    types(&amx, &view, "n");
    for (n, row) in twenty_rows().lines().enumerate() {
        if n > 0 {
            press(&amx, &view, "C-j");
        }
        types(&amx, &view, row);
    }
    let drawn = amx.until("the last row of the line", || {
        let drawn = amx.capture(&view);
        drawn.contains("row-20").then_some(drawn)
    });

    // The cap is ten rows or a third of the pane height, whichever is less.
    // The cursor is on the last row, so the top rows scroll off.
    let height: usize = pane_field(&amx, &view, "#{pane_height}")
        .parse()
        .expect("a pane height");
    let cap = 10.min(height / 3);
    let top = format!("❯ row-{:02}", 21 - cap);
    assert!(
        drawn.contains(&top),
        "the composer stops at {cap} rows and scrolls to {top}:\n{drawn}"
    );
    assert!(
        !drawn.contains("row-01"),
        "and what scrolled past is off the screen:\n{drawn}"
    );
}

#[test]
fn the_composer_takes_a_newline_from_ctrl_j_where_alt_enter_puts_one() {
    let amx = Harness::new();
    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");

    types(&amx, &view, "n");
    types(&amx, &view, "port the importer");
    amx.until("the first row of the task", || {
        amx.capture(&view)
            .contains("❯ port the importer")
            .then_some(())
    });

    // ctrl+j is the newline chord for terminals that cannot send alt+enter.
    // It arrives as 0x0A, which crossterm reads as ctrl+j in raw mode.
    press(&amx, &view, "C-j");
    types(&amx, &view, "and its tests");
    let drawn = amx.until("the second row of the task", || {
        let drawn = amx.capture(&view);
        drawn.contains("and its tests").then_some(drawn)
    });
    assert!(
        !drawn.contains("importerand"),
        "ctrl+j breaks the line rather than landing on the end of it:\n{drawn}"
    );
    assert!(
        agents(&amx).is_empty(),
        "and it grows the line rather than sending it:\n{drawn}"
    );

    // alt+enter also inserts a newline.
    press(&amx, &view, "M-Enter");
    types(&amx, &view, "in one go");
    amx.until("the third row of the task", || {
        amx.capture(&view).contains("in one go").then_some(())
    });

    press(&amx, &view, "Enter");
    let id = composed(&amx);
    assert_eq!(
        command_of(&amx, &id).last().map(String::as_str),
        Some("port the importer\nand its tests\nin one go"),
        "and the task the vendor is handed is the three rows as one line: {:?}",
        command_of(&amx, &id)
    );
}

#[test]
fn the_composer_takes_a_newline_from_shift_enter_where_the_terminal_sends_one() {
    let amx = Harness::new();

    // tmux answers modifyOtherKeys, not the kitty keyboard protocol the view
    // requests, so force extended keys in the CSI u format crossterm parses.
    // Both options apply when a pane is created and need a running server, so
    // start a placeholder session, set them, then open the view.
    amx.tmux(&["new-session", "-d", "--", "sh", "-c", "sleep 600"]);
    amx.tmux(&["set", "-s", "extended-keys", "always"]);
    amx.tmux(&["set", "-s", "extended-keys-format", "csi-u"]);

    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");

    types(&amx, &view, "n");
    types(&amx, &view, "port the importer");
    amx.until("the first row of the task", || {
        amx.capture(&view)
            .contains("❯ port the importer")
            .then_some(())
    });

    press(&amx, &view, "S-Enter");
    types(&amx, &view, "and its tests");
    let drawn = amx.until("the second row of the task", || {
        let drawn = amx.capture(&view);
        drawn.contains("and its tests").then_some(drawn)
    });
    assert!(
        !drawn.contains("importerand"),
        "shift+enter breaks the line rather than landing on the end of it:\n{drawn}"
    );
    assert!(
        agents(&amx).is_empty(),
        "and it grows the line rather than sending it:\n{drawn}"
    );
}

#[test]
fn the_composer_types_where_the_cursor_stands_rather_than_at_the_end() {
    let amx = Harness::new();
    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");

    // A typo mid-line, fixed by moving the cursor back to it.
    types(&amx, &view, "n");
    types(&amx, &view, "port the imprter");
    amx.until("the task on the screen", || {
        amx.capture(&view)
            .contains("❯ port the imprter")
            .then_some(())
    });

    // Four lefts put the block on the `r` the `o` goes before, just after
    // `port the imp`.
    for _ in 0..4 {
        press(&amx, &view, "Left");
    }
    let drawn = amx.until("the block to walk back into the line", || {
        sgr_past(&coloured(&amx, &view), "port the imp")
            .contains(&7)
            .then(|| amx.capture(&view))
    });
    assert!(
        drawn.contains("❯ port the imprter"),
        "the character under the block keeps its cell rather than being hidden \
         by it:\n{drawn}"
    );

    types(&amx, &view, "o");
    let drawn = amx.until("the letter where the cursor was", || {
        let drawn = amx.capture(&view);
        drawn.contains("❯ port the importer").then_some(drawn)
    });
    assert!(
        !drawn.contains("imprtero"),
        "a character typed mid-line lands where the block is:\n{drawn}"
    );

    press(&amx, &view, "Enter");
    let id = composed(&amx);
    assert_eq!(
        command_of(&amx, &id).last().map(String::as_str),
        Some("port the importer"),
        "and the task the vendor is handed is the line as it was mended: {:?}",
        command_of(&amx, &id)
    );
}

#[test]
fn the_composer_takes_back_a_character_and_a_word_where_the_cursor_stands() {
    let amx = Harness::new();
    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");

    // A doubled letter and an extra word mid-line, both fixed in place.
    types(&amx, &view, "n");
    types(&amx, &view, "port thee legacy importer");
    amx.until("the task on the screen", || {
        amx.capture(&view)
            .contains("❯ port thee legacy importer")
            .then_some(())
    });

    // Sixteen lefts put the cursor on the space after `thee`; backspace
    // removes the extra `e`.
    let mut keys = vec!["send-keys", "-t", &view];
    keys.extend(std::iter::repeat_n("Left", 16));
    amx.tmux(&keys);
    press(&amx, &view, "BSpace");
    let drawn = amx.until("the doubled letter to go", || {
        let drawn = amx.capture(&view);
        drawn
            .contains("❯ port the legacy importer")
            .then_some(drawn)
    });
    assert!(
        !drawn.contains("importe "),
        "backspace takes the character behind the cursor and not the last one \
         on the line:\n{drawn}"
    );

    // Eight rights put the cursor at the start of `importer`; ctrl+w deletes
    // the word behind it.
    let mut keys = vec!["send-keys", "-t", &view];
    keys.extend(std::iter::repeat_n("Right", 8));
    amx.tmux(&keys);
    press(&amx, &view, "C-w");
    let drawn = amx.until("the word to go", || {
        let drawn = amx.capture(&view);
        drawn.contains("❯ port the importer").then_some(drawn)
    });
    assert!(
        !drawn.contains("legacy"),
        "the word behind the cursor goes whole:\n{drawn}"
    );

    press(&amx, &view, "Enter");
    let id = composed(&amx);
    assert_eq!(
        command_of(&amx, &id).last().map(String::as_str),
        Some("port the importer"),
        "and the task the vendor is handed is the line as it was mended: {:?}",
        command_of(&amx, &id)
    );
}

/// Write a user skill named `review` under `~/.claude/skills`.
fn a_skill_called_review(amx: &Harness) {
    a_file_saying(
        &amx.home().join(".claude/skills/review/SKILL.md"),
        "Read the diff.",
    );
}

/// Write a user agent named `scout` under `~/.claude/agents`.
fn an_agent_called_scout(amx: &Harness) {
    a_file_saying(
        &amx.home().join(".claude/agents/scout.md"),
        "Goes and looks.",
    );
}

/// Write a markdown file at `path` whose frontmatter description is `about`.
fn a_file_saying(path: &std::path::Path, about: &str) {
    std::fs::create_dir_all(path.parent().expect("a directory")).expect("the directory");
    std::fs::write(path, format!("---\ndescription: {about}\n---\n\nwords\n"))
        .expect("the file the vendor loads");
}

#[test]
fn the_composer_completes_the_word_under_the_cursor_out_of_the_vendors_files() {
    let amx = Harness::new();
    a_skill_called_review(&amx);
    an_agent_called_scout(&amx);

    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");
    types(&amx, &view, "n");
    types(&amx, &view, "/rev");
    amx.until("the word on the line", || {
        amx.capture(&view).contains("❯ /rev").then_some(())
    });

    press(&amx, &view, "Tab");
    let drawn = amx.until("the word completed", || {
        let drawn = amx.capture(&view);
        drawn.contains("❯ /review").then_some(drawn)
    });
    assert!(
        agents(&amx).is_empty(),
        "tab finishes the word rather than the line:\n{drawn}"
    );

    // `/` completes commands and skills; `@` completes agents.
    types(&amx, &view, "@sco");
    press(&amx, &view, "Tab");
    amx.until("the second word completed", || {
        amx.capture(&view)
            .contains("❯ /review @scout")
            .then_some(())
    });

    press(&amx, &view, "Enter");
    let id = composed(&amx);
    assert_eq!(
        command_of(&amx, &id).last().map(String::as_str),
        Some("/review @scout "),
        "and what the vendor is handed is the words it answers to, each with \
         the space tab left for the next one: {:?}",
        command_of(&amx, &id)
    );
}

#[test]
fn the_composer_offers_the_agents_when_tab_is_pressed_on_nothing() {
    let amx = Harness::new();
    an_agent_called_scout(&amx);

    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");
    types(&amx, &view, "n");
    amx.until("the line", || {
        amx.capture(&view).contains("TASK").then_some(())
    });

    // Tab on an empty line inserts `@` and opens the suggestions for it.
    press(&amx, &view, "Tab");
    let drawn = amx.until("the agents under the line", || {
        let drawn = amx.capture(&view);
        (drawn.contains("❯ @") && drawn.contains("@scout")).then_some(drawn)
    });
    assert!(
        drawn.contains("Goes and looks."),
        "each of them saying what it is for:\n{drawn}"
    );
    assert!(
        agents(&amx).is_empty(),
        "and the key that opened them started nothing:\n{drawn}"
    );

    // It is the same list a typed `@` opens, so a second tab accepts the
    // selected entry.
    press(&amx, &view, "Tab");
    amx.until("the agent on the line", || {
        amx.capture(&view).contains("❯ @scout").then_some(())
    });
}

#[test]
fn the_composer_offers_the_projects_files_where_the_vendor_has_no_agents() {
    let amx = Harness::new();
    // With no claude agents defined, `@` suggests files from the directory the
    // line will run in.
    a_file_saying(&amx.home().join("notes/plan.md"), "What to do first.");

    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");
    types(&amx, &view, "n");
    amx.until("the line", || {
        amx.capture(&view).contains("TASK").then_some(())
    });

    press(&amx, &view, "Tab");
    amx.until("the files under the line", || {
        amx.capture(&view).contains("@notes/").then_some(())
    });
}

#[test]
fn the_composer_completes_a_file_of_the_project_the_agent_will_run_in() {
    let amx = Harness::new();
    // No claude agent matches `@not`, so it completes as a path under the
    // view's directory.
    a_file_saying(&amx.home().join("notes/plan.md"), "What to do first.");

    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");
    types(&amx, &view, "n");
    types(&amx, &view, "read @not");
    press(&amx, &view, "Tab");
    // A directory completes with a trailing `/` and no space, so typing
    // continues the same word.
    amx.until("the directory completed", || {
        amx.capture(&view).contains("❯ read @notes/").then_some(())
    });

    types(&amx, &view, "pl");
    press(&amx, &view, "Tab");
    amx.until("the file completed", || {
        amx.capture(&view)
            .contains("❯ read @notes/plan.md")
            .then_some(())
    });

    press(&amx, &view, "Enter");
    let id = composed(&amx);
    assert_eq!(
        command_of(&amx, &id).last().map(String::as_str),
        Some("read @notes/plan.md "),
        "and the vendor is handed the path the way it reads one: {:?}",
        command_of(&amx, &id)
    );

    // A `~/` path completes against the home directory. No file is named `~`,
    // so a completion shows the `~` was expanded.
    types(&amx, &view, "n");
    types(&amx, &view, "read @~/notes/pl");
    press(&amx, &view, "Tab");
    amx.until("the file under the home directory", || {
        amx.capture(&view)
            .contains("❯ read @~/notes/plan.md")
            .then_some(())
    });

    press(&amx, &view, "Enter");
    let next = composed_after(&amx, &id);
    assert_eq!(
        command_of(&amx, &next).last().map(String::as_str),
        Some("read @~/notes/plan.md "),
        "{:?}",
        command_of(&amx, &next)
    );
}

#[test]
fn the_composer_stands_what_the_word_could_be_in_a_band_under_the_line() {
    let amx = Harness::new();
    // Two skills match `/rev`: one selected entry and one unselected.
    a_skill_called_review(&amx);
    a_file_saying(
        &amx.home().join(".claude/skills/revise/SKILL.md"),
        "Say it again.",
    );

    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");
    types(&amx, &view, "n");
    types(&amx, &view, "/rev");
    // Wait for the whole word: the list refilters on each letter, and the
    // list for `/r` also has two entries.
    let drawn = amx.until("the band under the line", || {
        let drawn = amx.capture(&view);
        (drawn.contains("❯ /rev") && drawn.contains("Read the diff.")).then_some(drawn)
    });

    let rows: Vec<&str> = drawn.lines().collect();
    let at = |what: &str| {
        rows.iter()
            .position(|row| row.contains(what))
            .unwrap_or_else(|| panic!("{what} is on none of:\n{drawn}"))
    };
    assert!(
        at("❯ /rev") < at("/review") && at("/review") < at("/revise"),
        "the words stand under the line they would go on:\n{drawn}"
    );
    assert!(
        at("/revise") < at("enter starts it"),
        "and over the keys, which are the foot of the screen:\n{drawn}"
    );
    assert!(
        rows[at("/review")].contains("Read the diff."),
        "each of them saying what it is for:\n{drawn}"
    );

    let painted = coloured(&amx, &view);
    assert!(
        sgr_at(&painted, "/review").contains(&1),
        "the one the choice is on carries the weight:\n{painted:?}"
    );
    assert!(
        !sgr_at(&painted, "/revise").contains(&1),
        "and the ones under it do not:\n{painted:?}"
    );
    assert!(
        sgr_at(&painted, "Read the diff.").contains(&2),
        "what a word is for stands behind the word:\n{painted:?}"
    );
}

#[test]
fn the_composer_offers_the_words_the_vendor_answers_out_of_itself() {
    let amx = Harness::new();
    // A user skill with the same name as claude's built-in `/review`, which
    // must be offered once.
    a_skill_called_review(&amx);

    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");
    types(&amx, &view, "n");
    types(&amx, &view, "/sec");

    // `/security-review` is a claude built-in with no file behind it. The list
    // offers it alongside the file-based entries.
    let drawn = amx.until("the vendor's own word under the line", || {
        let drawn = amx.capture(&view);
        (drawn.contains("❯ /sec") && drawn.contains("/security-review")).then_some(drawn)
    });
    assert!(
        agents(&amx).is_empty(),
        "and reading it started nothing:\n{drawn}"
    );

    // `/review` is listed once, as the user skill: user directories are read
    // before amx's list of the vendor's built-ins, and the row shows the
    // skill's description.
    for _ in 0.."sec".len() {
        press(&amx, &view, "BSpace");
    }
    types(&amx, &view, "rev");
    let drawn = amx.until("the band on the word they share", || {
        let drawn = amx.capture(&view);
        (drawn.contains("❯ /rev") && drawn.contains("Read the diff.")).then_some(drawn)
    });
    let rows: Vec<&str> = drawn
        .lines()
        .filter(|row| row.contains("/review"))
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "one row, rather than one for the file and one for the vendor:\n{drawn}"
    );
    assert!(
        rows[0].contains("Read the diff."),
        "and it is the person's, which is the one with anything to say about \
         itself:\n{drawn}"
    );
}

#[test]
fn the_composer_drops_the_suggestions_before_the_line_they_stand_under() {
    let amx = Harness::new();
    a_skill_called_review(&amx);

    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");
    types(&amx, &view, "n");
    types(&amx, &view, "/rev");
    // Wait for the list as well as the word, or Esc can arrive before the
    // list is up.
    amx.until("the band under the line", || {
        let drawn = amx.capture(&view);
        (drawn.contains("❯ /rev") && drawn.contains("Read the diff.")).then_some(())
    });

    // Esc closes the list and leaves the line open, so Enter sends the word
    // as typed, not the selected suggestion. Wait for the list to go: an Esc
    // and an Enter read together are alt+enter.
    press(&amx, &view, "Escape");
    amx.until("the list closed over the line", || {
        let drawn = amx.capture(&view);
        (drawn.contains("❯ /rev") && !drawn.contains("Read the diff.")).then_some(())
    });
    press(&amx, &view, "Enter");

    let id = composed(&amx);
    assert_eq!(
        command_of(&amx, &id).last().map(String::as_str),
        Some("/rev"),
        "esc took the suggestions and left the line where it was typed: {:?}",
        command_of(&amx, &id)
    );
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

    // Tokens apply to one spawn. The next line without tokens uses the config
    // for every dial.
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
fn the_composer_moves_the_work_no_commit_holds_into_the_tree_it_asks_for() {
    let amx = Harness::new();
    a_repo_at(amx.home());
    std::fs::write(amx.home().join("README.md"), "after\n").expect("the work already in hand");
    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");

    types(&amx, &view, "n");
    types(&amx, &view, "w:changes fix the login bug");
    press(&amx, &view, "Enter");

    let id = composed(&amx);
    let meta = amx.meta(&id);
    let worktree = std::path::Path::new(meta["worktree"].as_str().expect("a tree of its own"));
    assert_eq!(
        std::fs::read_to_string(worktree.join("README.md")).expect("the tree has the work"),
        "after\n",
        "the agent starts on what was in hand rather than on the last commit"
    );
    assert_eq!(
        std::fs::read_to_string(amx.home().join("README.md")).unwrap(),
        "before\n",
        "and the directory the line was typed in is left as that commit had it"
    );
}

#[test]
fn the_composer_cuts_the_tree_from_the_ref_its_line_names() {
    let amx = Harness::new();
    a_repo_at(amx.home());
    let release = git(amx.home(), &["rev-parse", "HEAD"]);
    git(amx.home(), &["branch", "release"]);
    std::fs::write(amx.home().join("README.md"), "after\n").expect("a second version");
    git(amx.home(), &["commit", "-am", "second"]);

    // `b:` only applies to a worktree and this config turns worktrees off, so
    // the line also has `w:on`.
    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");
    types(&amx, &view, "n");
    types(&amx, &view, "b:release w:on port the importer");
    press(&amx, &view, "Enter");

    let id = composed(&amx);
    let meta = amx.meta(&id);
    assert_eq!(meta["base"], release, "the commit the ref resolved to");
    let worktree = std::path::Path::new(meta["worktree"].as_str().expect("a tree of its own"));
    assert_eq!(
        std::fs::read_to_string(worktree.join("README.md")).unwrap(),
        "before\n",
        "cut from the ref the line named rather than from what HEAD has become"
    );
}

#[test]
fn the_composer_starts_the_agent_on_the_branch_its_line_names() {
    let amx = Harness::new();
    a_repo_at(amx.home());
    // An existing branch with work on it. `on:` checks it out in the worktree
    // at its tip, so the agent's commits land on it.
    git(amx.home(), &["checkout", "-q", "-b", "spike"]);
    std::fs::write(amx.home().join("README.md"), "after\n").expect("work on the branch");
    git(amx.home(), &["commit", "-qam", "a spike"]);
    let left = git(amx.home(), &["rev-parse", "HEAD"]);
    git(amx.home(), &["checkout", "-q", "main"]);

    // `on:` always makes a worktree, so it overrides `worktrees = false`
    // without a `w:on`.
    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");
    types(&amx, &view, "n");
    types(&amx, &view, "on:spike carry on with it");
    press(&amx, &view, "Enter");

    let id = composed(&amx);
    let meta = amx.meta(&id);
    assert_eq!(meta["branch"], "spike", "the branch the record keeps");
    let worktree = std::path::Path::new(meta["worktree"].as_str().expect("a tree of its own"));
    assert_eq!(
        git(worktree, &["rev-parse", "HEAD"]),
        left,
        "cut on the branch as it stands rather than on what HEAD is"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("README.md")).unwrap(),
        "after\n",
        "so the agent starts on the work that is already there"
    );

    let command = command_of(&amx, &id);
    assert_eq!(
        command.last().map(String::as_str),
        Some("carry on with it"),
        "and the word is off the task the vendor is handed: {command:?}"
    );
}

#[test]
fn the_composer_runs_the_session_as_the_agent_the_line_is_led_with() {
    let amx = Harness::new();
    // A leading `@word` becomes `--agent` only when a claude agent of that
    // name exists.
    an_agent_called_scout(&amx);
    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");

    types(&amx, &view, "n");
    types(&amx, &view, "@scout port the importer");
    press(&amx, &view, "Enter");

    let id = composed(&amx);
    let command = command_of(&amx, &id);
    assert!(
        command.windows(2).any(|pair| pair == ["--agent", "scout"]),
        "the word the line is led with is who the vendor is asked to be: \
         {command:?}"
    );
    assert_eq!(
        command.last().map(String::as_str),
        Some("port the importer"),
        "and it is off the task the vendor is handed: {command:?}"
    );

    // A leading `@` that names no agent stays in the task: it is a file
    // reference, which claude resolves itself.
    types(&amx, &view, "n");
    types(&amx, &view, "@notes.md port the importer");
    press(&amx, &view, "Enter");

    let next = composed_after(&amx, &id);
    let command = command_of(&amx, &next);
    assert!(!command.iter().any(|arg| arg == "--agent"), "{command:?}");
    assert_eq!(
        command.last().map(String::as_str),
        Some("@notes.md port the importer"),
        "the whole line is the task: {command:?}"
    );
}

#[test]
fn the_composer_runs_a_line_led_with_a_bang_as_a_command() {
    let amx = Harness::new();
    a_repo_at(amx.home());
    // Worktrees are on by default, but a `!` command always runs in the
    // checkout it was typed in.
    let view = a_view_that_dispatches_as_claude(&amx, "");

    types(&amx, &view, "n");
    types(&amx, &view, "!echo one two");
    let drawn = amx.until("the command on the screen", || {
        let drawn = amx.capture(&view);
        drawn.contains("❯ !echo one two").then_some(drawn)
    });
    assert!(
        drawn.contains("COMMAND ·") && !drawn.contains("TASK ·"),
        "the rule over the line says what enter is about to do:\n{drawn}"
    );
    assert!(
        !drawn.contains("vendor default"),
        "and the dial the rule carries for an agent is off a row that runs \
         none:\n{drawn}"
    );

    press(&amx, &view, "Enter");
    let id = composed(&amx);
    assert_eq!(
        command_of(&amx, &id),
        ["sh", "-c", "echo one two"],
        "the rest of the line is the command, handed to a shell whole"
    );

    let meta = amx.meta(&id);
    assert!(
        meta["agent"].is_null(),
        "a command row runs no vendor: {meta}"
    );
    assert!(
        meta["worktree"].is_null(),
        "and never in a tree of its own: {meta}"
    );
    assert_eq!(
        meta["dir"],
        amx.home().to_string_lossy().as_ref(),
        "it runs where the view is: {meta}"
    );
    amx.until_state(&id, "done");

    // `d:` is the one token a command line accepts.
    let elsewhere = amx.home().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("somewhere else to run");
    types(&amx, &view, "n");
    types(
        &amx,
        &view,
        &format!("!d:{} echo there", elsewhere.display()),
    );
    press(&amx, &view, "Enter");

    let next = composed_after(&amx, &id);
    assert_eq!(
        command_of(&amx, &next),
        ["sh", "-c", "echo there"],
        "with the token off the command: {:?}",
        command_of(&amx, &next)
    );
    assert_eq!(
        amx.meta(&next)["dir"],
        elsewhere.to_string_lossy().as_ref(),
        "{}",
        amx.meta(&next)
    );
}

#[test]
fn the_composer_refuses_a_dial_beside_the_bang_and_keeps_the_line() {
    let amx = Harness::new();
    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");

    types(&amx, &view, "n");
    types(&amx, &view, "!m:opus cargo test");
    press(&amx, &view, "Enter");

    let drawn = amx.until("the refusal", || {
        let drawn = amx.capture(&view);
        drawn.contains("m:opus:").then_some(drawn)
    });
    assert!(
        drawn.contains("d:"),
        "said in the words of the line, and naming the one dial the row does \
         take:\n{drawn}"
    );
    assert!(
        drawn.contains("❯ !m:opus cargo test"),
        "the line is still there to be fixed:\n{drawn}"
    );
    assert!(agents(&amx).is_empty(), "and nothing was made:\n{drawn}");
}

#[test]
fn header_puts_what_the_next_agent_may_do_over_the_line_that_starts_it() {
    let amx = Harness::new();
    amx.config("agent = \"claude\"\n");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the header", || {
        amx.capture(&view)
            .contains("└ next  claude   model  default")
            .then_some(())
    });

    press(&amx, &view, "n");
    // The rule and the keys under the line, which can land a frame apart.
    let drawn = amx.until("the rule over the line", || {
        let drawn = amx.capture(&view);
        (drawn.contains("vendor default") && drawn.contains("shift+tab")).then_some(drawn)
    });
    assert!(
        drawn.contains("shift+tab permission"),
        "the dial on the rule wears no label, so the keys under the line name \
         what turns it:\n{drawn}"
    );

    press(&amx, &view, "BTab");
    let drawn = amx.until("the permission dial to turn", || {
        let drawn = amx.capture(&view);
        drawn.contains("acceptEdits").then_some(drawn)
    });
    let edge = drawn
        .lines()
        .find(|line| line.contains("TASK"))
        .expect("a rule over the line");
    assert!(
        edge.contains(" acceptEdits "),
        "the mode the dial is resting on, in the vendor's own word for it:\n{drawn}"
    );
    assert!(
        drawn.contains("❯"),
        "and the line under it is still there to type into:\n{drawn}"
    );
}

#[test]
fn input_mode_hangs_the_line_off_a_labelled_rule_over_a_wall_gone_dim() {
    let amx = Harness::new();
    amx.config("agent = \"claude\"\n");
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the wall", || {
        amx.capture(&view).contains("ask-a1b").then_some(())
    });

    types(&amx, &view, "n");
    types(&amx, &view, "port the importer");
    let drawn = amx.until("the rule over the line somebody is typing", || {
        let drawn = amx.capture(&view);
        (drawn.contains("TASK") && drawn.contains("port the importer")).then_some(drawn)
    });

    let edge = drawn
        .lines()
        .find(|line| line.contains("TASK"))
        .expect("a rule over the line");
    assert!(
        edge.contains("letters are text until esc"),
        "the one rule of the mode is said on its edge:\n{drawn}"
    );
    assert!(
        edge.contains('┈'),
        "and it is a rule the width of the screen:\n{drawn}"
    );
    assert!(
        edge.trim_end().ends_with("vendor default ┈┈"),
        "with what the next agent may do without asking at the far end of it:\n{drawn}"
    );
    assert!(
        drawn.contains("❯ port the importer"),
        "under it the line itself:\n{drawn}"
    );
    assert_eq!(
        pane_field(&amx, &view, "#{cursor_flag}"),
        "0",
        "with the terminal's own cursor put away, so nothing of the \
         terminal's blinks in the cell amx is painting:\n{drawn}"
    );

    // Read attributes from the top of the capture: tmux writes an escape only
    // where the style changes, so a row can inherit it from the one above.
    let painted = coloured(&amx, &view);
    assert!(
        sgr_at(&painted, "TASK").contains(&1),
        "the mode's own word carries the weight on the rule:\n{painted:?}"
    );
    assert!(
        sgr_at(&painted, "vendor default").contains(&7),
        "and the dial is set in reverse video, the way the badge is:\n{painted:?}"
    );
    assert!(
        !sgr_at(&painted, "port the importer").contains(&1),
        "the line somebody is typing carries no weight of its own, because what \
         says where somebody is on it is the block:\n{painted:?}"
    );
    assert!(
        sgr_past(&painted, "port the importer").contains(&7),
        "and the cell the next letter lands in is that cell reversed, which is \
         the whole of the block:\n{painted:?}"
    );

    // The wall stays visible behind the line, dimmed.
    let row = sgr_at(&painted, "ask-a1b");
    assert!(
        row.contains(&2) && !row.contains(&1),
        "every row behind the line goes dim and loses its weight:\n{painted:?}"
    );
    assert!(
        !sgr_at(&painted, "WAITING").contains(&7),
        "the count that wants somebody gives up its badge with them:\n{painted:?}"
    );
}

#[test]
fn lending_the_line_to_an_editor_hands_the_terminal_its_cursor_back() {
    let amx = Harness::new();
    amx.config("agent = \"claude\"\n");
    let editor = an_editor_that_waits(&amx);

    // Unset `VISUAL`: it takes precedence over `EDITOR`, and the developer
    // may have one set.
    let view = amx.in_a_terminal(&[("VISUAL", ""), ("EDITOR", &editor)], &[]);
    until_empty(&amx, &view);

    types(&amx, &view, "n");
    types(&amx, &view, "port the importer");
    amx.until("the line to be typed", || {
        amx.capture(&view)
            .contains("❯ port the importer")
            .then_some(())
    });
    assert_eq!(
        pane_field(&amx, &view, "#{cursor_flag}"),
        "0",
        "the terminal's own cursor is away while the view has the screen"
    );

    press(&amx, &view, "C-g");
    let drawn = amx.until("the editor to have the screen", || {
        let drawn = amx.capture(&view);
        drawn.contains("the editor has the screen").then_some(drawn)
    });
    assert_eq!(
        pane_field(&amx, &view, "#{cursor_flag}"),
        "1",
        "and it goes back with the screen, so somebody typing in the editor \
         can see where they are:\n{drawn}"
    );

    std::fs::write(amx.home().join("let-it-go"), "").expect("the editor to be let go");
    let drawn = amx.until("the view to take the screen back", || {
        let drawn = amx.capture(&view);
        drawn.contains("❯ port the importer").then_some(drawn)
    });
    assert_eq!(
        pane_field(&amx, &view, "#{cursor_flag}"),
        "0",
        "the draw that takes it back puts it away again:\n{drawn}"
    );
}

#[test]
fn header_dials_are_the_argv_the_next_agent_is_started_with() {
    let amx = Harness::new();
    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");

    press(&amx, &view, "M-m");
    amx.until("the model dial to turn", || {
        amx.capture(&view)
            .contains("└ next  claude   model  fable")
            .then_some(())
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

    // A line token overrides the dial for one spawn and leaves it unchanged.
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
        amx.capture(&view)
            .contains("└ next  claude   model  fable")
            .then_some(())
    });
}

#[test]
fn header_effort_dial_says_how_hard_the_next_agent_thinks() {
    let amx = Harness::new();
    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");
    amx.until("the header", || {
        amx.capture(&view)
            .contains("└ next  claude   model  default")
            .then_some(())
    });

    // At 80 columns an unset effort dial is not shown. Once turned it
    // appears, and the labels are dropped to make room.
    press(&amx, &view, "M-e");
    let drawn = amx.until("the effort dial to turn", || {
        let drawn = amx.capture(&view);
        drawn.contains("·  low").then_some(drawn)
    });
    assert!(
        drawn.contains("└ next  claude"),
        "and the row is still the dials:\n{drawn}"
    );

    types(&amx, &view, "n");
    types(&amx, &view, "port the importer");
    press(&amx, &view, "Enter");

    let id = composed(&amx);
    let command = command_of(&amx, &id);
    assert!(
        command.windows(2).any(|pair| pair == ["--effort", "low"]),
        "what the header says the next agent thinks at is what it is: \
         {command:?}"
    );

    // An `e:` token overrides the dial for one spawn.
    types(&amx, &view, "n");
    types(&amx, &view, "e:max fix the login bug");
    press(&amx, &view, "Enter");

    let next = composed_after(&amx, &id);
    assert!(
        command_of(&amx, &next)
            .windows(2)
            .any(|pair| pair == ["--effort", "max"]),
        "{:?}",
        command_of(&amx, &next)
    );
}

#[test]
fn header_vendor_dial_runs_the_next_agent_under_the_vendor_it_names() {
    let amx = Harness::new();
    // The config names mock-claude by path, which has no registry entry.
    // claude is also on PATH, and only the vendor dial can select it.
    let view = a_view_that_can_start_claude(
        &amx,
        &format!("agent = \"{}\"\nworktrees = false\n", amx.mock()),
    );
    let drawn = amx.until("the header", || {
        let drawn = amx.capture(&view);
        drawn.contains("worktree  none").then_some(drawn)
    });
    // Match the label with its two trailing spaces: the agent command on this
    // row is a path, which could contain "model".
    assert!(
        !drawn.contains("model  "),
        "an unregistered command declares no model dial, so there is no dial \
         on the row to name:\n{drawn}"
    );

    press(&amx, &view, "M-a");
    amx.until("the vendor dial to turn", || {
        amx.capture(&view)
            .contains("└ next  claude   model  default")
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
fn header_model_dial_keeps_the_harness_the_row_names() {
    let amx = Harness::new();
    // The config picks pi and lists its models. The model dial cycles those
    // and never switches the vendor, though claude is on PATH.
    let view = a_view_that_can_start_claude(
        &amx,
        "agent = \"pi\"\nworktrees = false\n\n[pi]\nmodels = [\"openai/gpt-5\", \"anthropic/claude-opus-4-1\"]\n",
    );
    amx.until("the header", || {
        amx.capture(&view)
            .contains("└ next  pi   model  default")
            .then_some(())
    });

    press(&amx, &view, "M-m");
    amx.until("the model dial to turn", || {
        amx.capture(&view)
            .contains("└ next  pi   model  openai/gpt-5")
            .then_some(())
    });

    press(&amx, &view, "M-m");
    amx.until("the next model, and still pi", || {
        amx.capture(&view)
            .contains("└ next  pi   model  anthropic/claude-opus-4-1")
            .then_some(())
    });

    press(&amx, &view, "M-m");
    amx.until("the dial back at the sentinel", || {
        amx.capture(&view)
            .contains("└ next  pi   model  default")
            .then_some(())
    });
}

#[test]
fn header_worktree_dial_gives_the_next_agent_a_tree_the_file_would_not() {
    let amx = Harness::new();
    a_repo_at(amx.home());
    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");

    press(&amx, &view, "M-w");
    amx.until("the worktree dial to turn", || {
        amx.capture(&view).contains("worktree  new").then_some(())
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
        let drawn = amx.capture(&view);
        drawn.contains("claude accepts").then_some(drawn)
    });
    assert!(
        drawn.contains("acceptEdits"),
        "and the modes it does take:\n{drawn}"
    );
    assert!(
        drawn.contains("❯ p:nonsense port the importer"),
        "the line is still there to be fixed:\n{drawn}"
    );
    assert!(agents(&amx).is_empty(), "and nothing was made:\n{drawn}");
}

#[test]
fn a_reply_to_an_agent_between_turns_is_a_message() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "takes-a-message");
    amx.until_state("fix-login-a1b", "idle");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        amx.capture(&view).contains("fix-login-a1b").then_some(())
    });

    // Space opens the card. The idle agent is asking nothing, so the empty
    // line's placeholder is `reply`.
    press(&amx, &view, "Space");
    amx.until("the card and its line", || {
        let drawn = amx.capture(&view);
        (drawn
            .lines()
            .any(|line| line.starts_with("✻ fix-login-a1b · claude ┈"))
            && drawn.contains("❯ reply"))
        .then_some(())
    });
    types(&amx, &view, "and now the linter");
    amx.until("the words on the line", || {
        amx.capture(&view)
            .contains("❯ and now the linter")
            .then_some(())
    });
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
fn ctrl_r_calls_the_agent_what_a_person_typed() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        amx.capture(&view).contains("fix-login-a1b").then_some(())
    });

    press(&amx, &view, "C-r");
    amx.until("the line to open on what the row is called", || {
        // The rule names the agent, and the line starts with its current name
        // for editing.
        let drawn = amx.capture(&view);
        (drawn.contains("RENAME · fix-login-a1b") && drawn.contains("❯ fix-login-a1b"))
            .then_some(())
    });

    // Erase the prefilled name and type the new one.
    let mut keys = vec!["send-keys", "-t", &view];
    keys.extend(std::iter::repeat_n("BSpace", "fix-login-a1b".len()));
    amx.tmux(&keys);
    types(&amx, &view, "auth");
    press(&amx, &view, "Enter");

    let wall = amx.until("the wall to call it auth", || {
        let drawn = amx.capture(&view);
        drawn
            .lines()
            .any(|line| a_row_called(line, "auth"))
            .then_some(drawn)
    });
    assert!(
        !wall.lines().any(|line| a_row_called(line, "fix-login-a1b")),
        "the row carries the name and the id is off it:\n{wall}"
    );
    assert_eq!(
        amx.state("fix-login-a1b")["name"],
        "auth",
        "and the record is filed under the id it always was"
    );
}

#[test]
fn find_narrows_the_wall_as_it_is_typed_and_esc_puts_it_back() {
    let amx = Harness::new();
    finished(&amx, "port-a1b", "done", 60);
    finished(&amx, "login-b2c", "done", 120);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both agents", || {
        let drawn = amx.capture(&view);
        (drawn.contains("port-a1b") && drawn.contains("login-b2c")).then_some(())
    });

    // The find line opens in place of the hint row, with a placeholder.
    types(&amx, &view, "/");
    amx.until("the find line", || {
        amx.capture(&view)
            .contains("/a name or task, or s:state")
            .then_some(())
    });

    // The filter applies on each keystroke, before Enter.
    types(&amx, &view, "port");
    let narrowed = amx.until("the wall to narrow under it", || {
        let drawn = amx.capture(&view);
        // Check both in one frame: a frame from partway through the word has
        // already dropped the other row.
        (drawn.contains("/port") && !drawn.contains("login-b2c")).then_some(drawn)
    });
    assert!(
        narrowed.contains("port-a1b"),
        "the one that matches is still there:\n{narrowed}"
    );

    // Enter closes the line and keeps the filter.
    press(&amx, &view, "Enter");
    let kept = amx.until("the line to go", || {
        // Wait for the hint row, not for `/port` to go: the header still
        // shows the filter as typed.
        let drawn = amx.capture(&view);
        drawn.contains("space card").then_some(drawn)
    });
    assert!(
        kept.contains("/port"),
        "with the header saying what the wall is narrowed to:\n{kept}"
    );
    assert!(
        !kept.contains("login-b2c"),
        "the wall stays narrowed:\n{kept}"
    );

    // Esc on the list, with no line open, clears the filter.
    press(&amx, &view, "Escape");
    amx.until("the whole fleet back", || {
        let drawn = amx.capture(&view);
        (drawn.contains("port-a1b") && drawn.contains("login-b2c")).then_some(())
    });
}

#[test]
fn find_reaches_the_task_an_agent_was_started_on() {
    let amx = Harness::new();
    finished(&amx, "one-a1b", "done", 60);
    finished(&amx, "two-b2c", "done", 120);

    // Tasks unrelated to the ids, so a match can only come from the task.
    for (id, task) in [
        ("one-a1b", "Port the importer to the new shape"),
        ("two-b2c", "fix the login bug"),
    ] {
        let meta = amx.agent_dir(id).join("meta.json");
        let mut held: Value =
            serde_json::from_str(&std::fs::read_to_string(&meta).expect("the record"))
                .expect("the record is json");
        held["task"] = json!(task);
        std::fs::write(&meta, held.to_string()).expect("the record back");
    }

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both agents", || {
        let drawn = amx.capture(&view);
        (drawn.contains("one-a1b") && drawn.contains("two-b2c")).then_some(())
    });

    types(&amx, &view, "/importer");
    amx.until("the wall to narrow to the task that says it", || {
        let drawn = amx.capture(&view);
        (drawn.contains("one-a1b") && !drawn.contains("two-b2c")).then_some(())
    });

    types(&amx, &view, "");
    for _ in 0.."importer".len() {
        press(&amx, &view, "BSpace");
    }
    // Matching ignores case.
    types(&amx, &view, "LOGIN");
    amx.until("the other one, found past its capitals", || {
        let drawn = amx.capture(&view);
        (drawn.contains("two-b2c") && !drawn.contains("one-a1b")).then_some(())
    });
}
