//! Tests for `amx here`: a claude started where you stand, under amx, that
//! opens idle and takes over the terminal it was typed in.

mod common;

use common::{Harness, agents, argv_of, clients_on, command_of};

/// A claude that opens a session at once and waits at its prompt.
fn opens_a_session(amx: &Harness) -> String {
    let path = amx.home().join("opens-a-session.scenario");
    std::fs::write(
        &path,
        "argv\nhook SessionStart {\"source\": \"startup\"}\nprint waiting at the prompt\nsleep 600000\n",
    )
    .expect("writing the scenario");
    path.to_string_lossy().into_owned()
}

/// Run `amx here` with `args` in a terminal outside tmux, which starts in the
/// harness's home, and answer with the one agent it started.
fn here(amx: &Harness, args: &[&str]) -> (String, String) {
    amx.config(&format!("agent = \"{}\"\n", amx.mock()));
    let scenario = opens_a_session(amx);
    let terminal = amx.in_a_terminal(
        &[
            ("TMUX", ""),
            ("TMUX_PANE", ""),
            ("MOCK_CLAUDE_SCENARIO", &scenario),
        ],
        args,
    );
    let id = amx.until("the agent to have a record", || {
        let id = agents(amx).into_iter().next()?;
        amx.meta(&id)["pane"].is_string().then_some(id)
    });
    (terminal, id)
}

#[test]
fn here_starts_an_agent_in_the_directory_with_no_tree_and_attaches_the_terminal() {
    let amx = Harness::new();
    let (terminal, id) = here(&amx, &["here"]);

    let meta = amx.meta(&id);
    assert_eq!(meta["dir"], amx.home().to_string_lossy().as_ref());
    assert!(meta["worktree"].is_null(), "{meta}");
    assert_eq!(agents(&amx).len(), 1);

    let tty = amx.tmux(&["display-message", "-p", "-t", &terminal, "#{pane_tty}"]);
    amx.until("this terminal on the agent", || {
        clients_on(&amx, &format!("amx-{id}"))
            .lines()
            .any(|on| on == tty)
            .then_some(())
    });
}

#[test]
fn here_opens_with_no_task_and_reads_idle_once_the_session_opens() {
    let amx = Harness::new();
    let (_terminal, id) = here(&amx, &["here"]);

    let argv = argv_of(&amx, &id);
    assert!(!argv.contains("''"), "an empty task word: {argv}");
    let command = command_of(&amx, &id);
    assert_eq!(command.len(), 1, "{command:?}");
    assert_eq!(command[0], amx.mock());
    assert_eq!(amx.meta(&id)["task"], "");

    amx.until_state(&id, "idle");
}

#[test]
fn here_passes_what_follows_the_dashes_to_claude() {
    let amx = Harness::new();
    let (_terminal, id) = here(&amx, &["here", "--", "--model", "haiku"]);

    let command = command_of(&amx, &id);
    assert_eq!(command, [amx.mock(), "--model".into(), "haiku".into()]);
    assert!(argv_of(&amx, &id).contains("--model haiku"));
}

#[test]
fn new_still_refuses_an_empty_task() {
    let amx = Harness::new();
    let out = amx.amx(&["new", ""]);
    assert_ne!(common::code(&out), 0);
    assert!(agents(&amx).is_empty());
}
