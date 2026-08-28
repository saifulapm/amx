//! The line somebody types on: the task it starts, the dials the header holds
//! over it, and what else the same line takes.
//!
//! Driven in a real tmux pane like the rest of the view, because what a line
//! grows to on the screen and what it starts when it is sent are questions a
//! pty answers and nothing else does.

mod common;

use common::Harness;
use serde_json::json;
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

/// Whether this line of the screen is the row of an agent the view is calling
/// `name`: past the gutter, the mark and the space after it, which is where a
/// row writes what it calls its agent. A notice quoting the same word is prose
/// at the left edge and does not answer to this.
fn a_row_called(line: &str, name: &str) -> bool {
    line.chars().skip(4).collect::<String>().starts_with(name)
}

/// The same screen with the colours the view drew it in, as the escapes tmux
/// wrote them: what a bar is made of cannot be read off the text.
fn coloured(amx: &Harness, pane: &str) -> String {
    amx.tmux(&["capture-pane", "-p", "-e", "-J", "-t", pane])
}

/// The SGR attributes in force where `word` starts on this captured line:
/// every escape before it walked, resets honoured, and the colour
/// introducers' arguments consumed — the `2` of `38;2;r;g;b` is a
/// colourspace, never the dim attribute.
fn sgr_at(line: &str, word: &str) -> Vec<u16> {
    let at = line
        .find(word)
        .unwrap_or_else(|| panic!("{word:?} is not on {line:?}"));
    let mut on: Vec<u16> = Vec::new();
    let mut rest = &line[..at];
    while let Some(start) = rest.find("\u{1b}[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('m') else { break };
        let params: Vec<u16> = after[..end]
            .split(';')
            .map(|param| param.parse().unwrap_or(0))
            .collect();
        let mut n = 0;
        while n < params.len() {
            match params[n] {
                0 => on.clear(),
                22 => on.retain(|param| *param != 1 && *param != 2),
                38 | 48 => {
                    n += match params.get(n + 1) {
                        Some(2) => 4,
                        Some(5) => 2,
                        _ => 0,
                    };
                }
                param => on.push(param),
            }
            n += 1;
        }
        rest = &after[end + 1..];
    }
    on
}

/// What the default theme paints a role in, out of the file that states it.
///
/// The escapes below are what tmux wrote for a colour, and a colour typed out
/// here as well would part company with the palette the day somebody edited
/// one. `assets/themes/default.toml` is held to the struct default by a test
/// of its own, so reading it here reaches both.
fn default_theme(role: &str) -> (u8, u8, u8) {
    let said = include_str!("../assets/themes/default.toml")
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{role} = ")))
        .unwrap_or_else(|| panic!("the default theme names {role}"))
        .trim()
        .trim_matches('"');
    rgb(said)
}

/// A colour as a theme file spells it, in the three bytes tmux writes.
fn rgb(said: &str) -> (u8, u8, u8) {
    let hex = said
        .strip_prefix('#')
        .unwrap_or_else(|| panic!("a hex colour: {said}"));
    let byte = |at: usize| {
        u8::from_str_radix(&hex[at..at + 2], 16).unwrap_or_else(|_| panic!("a hex colour: {said}"))
    };
    (byte(0), byte(2), byte(4))
}

/// A colour as the escape tmux writes for text painted in it.
fn text_in((r, g, b): (u8, u8, u8)) -> String {
    format!("38;2;{r};{g};{b}")
}

/// A role of the default theme as the escape tmux writes for text in it.
fn foreground(role: &str) -> String {
    text_in(default_theme(role))
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

/// Wait for a view with nothing in it, which is the one line amx has for a
/// wall nobody has put anything on.
fn until_empty(amx: &Harness, view: &str) {
    amx.until("the empty view", || {
        screen(amx, view).contains("nobody asking").then_some(())
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
fn header_says_what_the_next_agent_will_be_started_with() {
    let amx = Harness::new();
    amx.config("agent = \"claude\"\nmax_agents = 3\n");
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    let drawn = amx.until("the header", || {
        let drawn = screen(&amx, &view);
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
        drawn.contains("1/3 running    1 WAITING"),
        "the gate the next one meets, and the count that wants a person set \
         apart from it:\n{drawn}"
    );
    assert!(
        drawn.contains("└ next  claude   model  default   permission  default   worktree  new"),
        "and under it the vendor, the dials it will be given, and whether it \
         is cut a tree of its own:\n{drawn}"
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
            .contains("└ next  claude   model  default")
            .then_some(())
    });

    press(&amx, &view, "M-m");
    amx.until("the model dial to turn", || {
        screen(&amx, &view)
            .contains("└ next  claude   model  fable")
            .then_some(())
    });

    press(&amx, &view, "M-w");
    let drawn = amx.until("the worktree dial to turn", || {
        let drawn = screen(&amx, &view);
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
        let drawn = screen(&amx, &view);
        (drawn.contains("ask-a1b") && drawn.contains("fix-login-b2c")).then_some(())
    });

    // The same line a task is typed on, which is where somebody's hands
    // already are.
    types(&amx, &view, "n");
    types(&amx, &view, "s:waiting");
    amx.until(
        "the line to say it will narrow rather than start anything",
        || screen(&amx, &view).contains("NARROW").then_some(()),
    );
    press(&amx, &view, "Enter");

    // The line goes in the same frame the narrowing lands in, and it goes
    // last: waiting on the words alone would match the screen that is already
    // there, where they are still on the line somebody typed them on.
    let drawn = amx.until("the narrowed list", || {
        let drawn = screen(&amx, &view);
        (!drawn.contains("❯") && drawn.contains("s:waiting")).then_some(drawn)
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
fn header_reads_as_chrome_with_its_one_colour_on_what_wants_a_person() {
    let amx = Harness::new();
    amx.config("agent = \"claude\"\n");
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the header", || {
        screen(&amx, &view).contains("└ next").then_some(())
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

    // The dials row names its dials as quietly as the rest of the chrome up
    // here: what carries the colour on it is the value each one is set to. The
    // attributes are read across both rows, because a terminal writes an
    // escape where the paint changes rather than where a line begins.
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
        pane_field(&amx, &pane, "#{session_name}"),
        format!("amx-{id}"),
        "in a session of its own, leaving the view where it was"
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
    let top = format!("❯ row-{:02}", 22 - cap);
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
fn header_puts_what_the_next_agent_may_do_over_the_line_that_starts_it() {
    let amx = Harness::new();
    amx.config("agent = \"claude\"\n");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the header", || {
        screen(&amx, &view)
            .contains("└ next  claude   model  default")
            .then_some(())
    });

    press(&amx, &view, "n");
    let drawn = amx.until("the rule over the line", || {
        let drawn = screen(&amx, &view);
        drawn.contains("vendor default").then_some(drawn)
    });
    assert!(
        drawn.contains("shift+tab permission"),
        "the dial on the rule wears no label, so the keys under the line name \
         what turns it:\n{drawn}"
    );

    press(&amx, &view, "BTab");
    let drawn = amx.until("the permission dial to turn", || {
        let drawn = screen(&amx, &view);
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
        screen(&amx, &view).contains("ask-a1b").then_some(())
    });

    types(&amx, &view, "n");
    types(&amx, &view, "port the importer");
    let drawn = amx.until("the rule over the line somebody is typing", || {
        let drawn = screen(&amx, &view);
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
        drawn.contains("❯ port the importer█"),
        "under it the line itself, with a block where the next letter lands:\n{drawn}"
    );

    // The attributes are read from the top of the capture, because a terminal
    // writes an escape where the paint changes rather than where a row begins.
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
        sgr_at(&painted, "port the importer").contains(&1),
        "the line somebody is typing is the one bold thing left:\n{painted:?}"
    );

    // And the wall it was opened from is still there, saying so quietly.
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
fn header_dials_are_the_argv_the_next_agent_is_started_with() {
    let amx = Harness::new();
    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");

    press(&amx, &view, "M-m");
    amx.until("the model dial to turn", || {
        screen(&amx, &view)
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
        screen(&amx, &view)
            .contains("└ next  claude   model  fable")
            .then_some(())
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
        drawn.contains("worktree  none").then_some(drawn)
    });
    assert!(
        !drawn.contains("model"),
        "an unregistered command declares no model dial, so there is no dial \
         on the row to name:\n{drawn}"
    );

    press(&amx, &view, "M-v");
    amx.until("the vendor dial to turn", || {
        screen(&amx, &view)
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
fn header_worktree_dial_gives_the_next_agent_a_tree_the_file_would_not() {
    let amx = Harness::new();
    a_repo_at(amx.home());
    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");

    press(&amx, &view, "M-w");
    amx.until("the worktree dial to turn", || {
        screen(&amx, &view).contains("worktree  new").then_some(())
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
        screen(&amx, &view).contains("fix-login-a1b").then_some(())
    });

    types(&amx, &view, "r");
    amx.until("the line to be addressed to the agent", || {
        screen(&amx, &view)
            .contains("MESSAGE · to fix-login-a1b")
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
fn acts_ctrl_r_calls_the_agent_what_a_person_typed() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("fix-login-a1b").then_some(())
    });

    press(&amx, &view, "C-r");
    amx.until("the line to open on what the row is called", || {
        // Which agent is on the rule; what it is called already is on the
        // line, because a rename is an edit of the name rather than a name
        // typed again from nothing.
        let drawn = screen(&amx, &view);
        (drawn.contains("RENAME · fix-login-a1b") && drawn.contains("❯ fix-login-a1b"))
            .then_some(())
    });

    // Edited rather than typed again: the name it had, back to nothing, and a
    // word of somebody's own in its place.
    let mut keys = vec!["send-keys", "-t", &view];
    keys.extend(std::iter::repeat_n("BSpace", "fix-login-a1b".len()));
    amx.tmux(&keys);
    types(&amx, &view, "auth");
    press(&amx, &view, "Enter");

    let wall = amx.until("the wall to call it auth", || {
        let drawn = screen(&amx, &view);
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
        let drawn = screen(&amx, &view);
        (drawn.contains("port-a1b") && drawn.contains("login-b2c")).then_some(())
    });

    // The line opens on the row the keys were on, and says what it takes.
    types(&amx, &view, "/");
    amx.until("the find line", || {
        screen(&amx, &view)
            .contains("/a name, or s:state")
            .then_some(())
    });

    // Narrowed on the keystroke, not on an enter afterwards: the wall answers
    // while the word is still being typed.
    types(&amx, &view, "port");
    let narrowed = amx.until("the wall to narrow under it", || {
        let drawn = screen(&amx, &view);
        // Both, in one frame: the wall narrows on every keystroke, so a frame
        // caught part way through the word has already dropped the row that
        // does not match.
        (drawn.contains("/port") && !drawn.contains("login-b2c")).then_some(drawn)
    });
    assert!(
        narrowed.contains("port-a1b"),
        "the one that matches is still there:\n{narrowed}"
    );

    // Enter closes the line and leaves the narrowing standing.
    press(&amx, &view, "Enter");
    let kept = amx.until("the line to go", || {
        // The keys are back on the row the line had. Not the absence of
        // `/port`: the header reads the narrowing back in the words it was
        // typed in, so that string is still on the screen and should be.
        let drawn = screen(&amx, &view);
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

    // And esc drops it from the list itself, with no line open: a narrowing
    // outlives the line it was typed on, so the key that clears it has to.
    press(&amx, &view, "Escape");
    amx.until("the whole fleet back", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("port-a1b") && drawn.contains("login-b2c")).then_some(())
    });
}
