//! The line somebody types on: the task it starts, the dials the header holds
//! over it, and what else the same line takes.
//!
//! Driven in a real tmux pane like the rest of the view, because what a line
//! grows to on the screen and what it starts when it is sent are questions a
//! pty answers and nothing else does.

mod common;

use common::Harness;
use serde_json::{Value, json};
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

/// The SGR attributes in force where `word` starts on this captured line.
fn sgr_at(line: &str, word: &str) -> Vec<u16> {
    in_force(&line[..starts_at(line, word)])
}

/// The same, on the cell after `word` ends, which is where the block stands at
/// the end of a line being typed. The escapes the view wrote between the two
/// are what paint that cell, so they are walked with the rest.
fn sgr_past(line: &str, word: &str) -> Vec<u16> {
    let mut end = starts_at(line, word) + word.len();
    while let Some(rest) = line[end..].strip_prefix("\u{1b}[") {
        let Some(over) = rest.find('m') else { break };
        end += "\u{1b}[".len() + over + 1;
    }
    in_force(&line[..end])
}

/// Where a word begins on a captured line, escapes and all.
fn starts_at(line: &str, word: &str) -> usize {
    line.find(word)
        .unwrap_or_else(|| panic!("{word:?} is not on {line:?}"))
}

/// The SGR attributes in force at the end of this much of a capture: every
/// escape in it walked, resets honoured, and the colour introducers' arguments
/// consumed — the `2` of `38;2;r;g;b` is a colourspace, never the dim
/// attribute.
fn in_force(walked: &str) -> Vec<u16> {
    let mut on: Vec<u16> = Vec::new();
    let mut rest = walked;
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

/// The background the cursor's bar is made of, as tmux writes the escape.
fn bar() -> String {
    let (r, g, b) = default_theme("cursor");
    format!("48;2;{r};{g};{b}")
}

/// The captured line holding this text, escapes and all.
fn coloured_line(amx: &Harness, view: &str, text: &str) -> String {
    let drawn = coloured(amx, view);
    drawn
        .lines()
        .rfind(|line| line.contains(text))
        .unwrap_or_else(|| panic!("no line holding {text} in:\n{drawn}"))
        .to_string()
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

/// An `$EDITOR` that says it has the screen and holds it until the test drops
/// `let-it-go` in the home it is running under.
///
/// It writes nothing to the file it is opened on and asks the terminal for
/// nothing, which is the point: what it is sitting in front of is whatever
/// state the view left the terminal in, the way `vi` or `less` would be.
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
        let drawn = screen(&amx, &view);
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

    // One agent in the project and one outside it, so the count on the header
    // is the project's rather than the machine's.
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");
    amx.set_meta("ask-a1b", json!({ "dir": repo }));
    amx.play("busy-b2c", "works-without-end");
    amx.until_state("busy-b2c", "working");

    let view = amx.in_a_terminal(&[], &["--dir", &repo.to_string_lossy()]);
    let drawn = amx.until("the header", || {
        let drawn = screen(&amx, &view);
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
    // What a finished row says is a key like any other, so a project can have
    // its own answer to it. Both commands answer the same way every time,
    // which is the only difference from the model call somebody would really
    // configure here that matters to the reader running it.
    amx.config("summary_command = \"sed s/^/mine:/\"\n");

    let repo = amx.home().join("elsewhere");
    std::fs::create_dir_all(repo.join(".amx")).expect("the project");
    a_repo_at(&repo);
    std::fs::write(
        repo.join(".amx/config.toml"),
        "summary_command = \"sed s/^/theirs:/\"\n",
    )
    .expect("the project's own config");

    // One turn that ended in the project and one that ended outside it, so
    // each row is a reading of the file the agent that wrote it ran under.
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

    // The find line, which is the one line that narrows anything: the tokens
    // are read on the keystroke rather than on an enter after them.
    types(&amx, &view, "/");
    types(&amx, &view, "s:waiting");
    let drawn = amx.until("the narrowed list", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("ask-a1b") && !drawn.contains("fix-login-b2c")).then_some(drawn)
    });
    assert_eq!(
        agents(&amx).len(),
        2,
        "and nothing was started with the line:\n{drawn}"
    );

    // Enter closes the line and leaves the narrowing standing.
    press(&amx, &view, "Enter");
    let kept = amx.until("the line to go", || {
        let drawn = screen(&amx, &view);
        drawn.contains("space card").then_some(drawn)
    });
    assert!(
        kept.contains("s:waiting") && !kept.contains("fix-login-b2c"),
        "with the header saying what the wall is narrowed to:\n{kept}"
    );

    // And esc gives the fleet back, the way it does for any narrowing that
    // has outlived the line it was typed on.
    press(&amx, &view, "Escape");
    amx.until("the whole fleet again", || {
        screen(&amx, &view).contains("fix-login-b2c").then_some(())
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
        let drawn = screen(&amx, &view);
        (drawn.contains("ask-a1b") && drawn.contains("busy-b2c") && drawn.contains("fix-login-c3d"))
            .then_some(())
    });

    // What somebody watching a fleet asks for: what needs them and what is
    // still running, on the one screen, with everything finished out of the
    // way.
    types(&amx, &view, "/");
    types(&amx, &view, "s:waiting s:working");
    let drawn = amx.until("the narrowed list", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("ask-a1b")
            && drawn.contains("busy-b2c")
            && !drawn.contains("fix-login-c3d"))
        .then_some(drawn)
    });
    assert!(
        drawn.contains("NEEDS INPUT") && drawn.contains("WORKING"),
        "with both groups still headed over their agents:\n{drawn}"
    );

    // Enter closes the line, and the header says the whole of what was typed.
    press(&amx, &view, "Enter");
    let kept = amx.until("the line to go", || {
        let drawn = screen(&amx, &view);
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

    // The words that narrowed the wall from here once. `/` reads them now, so
    // on this line they are a task with a colon in it like any other.
    types(&amx, &view, "n");
    types(&amx, &view, "s:waiting");
    let drawn = amx.until("the task on the screen", || {
        let drawn = screen(&amx, &view);
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
fn the_cursor_lands_on_the_agent_the_line_started() {
    let amx = Harness::new();
    let view = a_view_that_dispatches(&amx, "happy-turn");

    // One agent already on the wall for the cursor to be standing on, so that
    // the bar has somewhere to move from.
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
        screen(&amx, &view).contains("❯ port it").then_some(())
    });
    press(&amx, &view, "Enter");

    // The row does not exist until the reading after the start, and the bar is
    // on it as soon as it does.
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
fn the_composer_starts_an_agent_in_the_project_the_cursor_is_under() {
    let amx = Harness::new();
    let view = a_view_that_dispatches(&amx, "happy-turn");

    // Two projects on the wall: the directory the view was opened in, and a
    // second one an agent of its own is running in.
    let api = amx.home().join("api");
    std::fs::create_dir_all(&api).expect("the second project");
    finished(&amx, "here-a1b", "done", 30);
    finished(&amx, "api-b2c", "done", 60);
    amx.set_meta("api-b2c", json!({ "dir": api }));
    amx.until("both agents", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("here-a1b") && drawn.contains("api-b2c")).then_some(())
    });

    // Gathered by project, with the cursor on the second heading: the last row
    // of the list is that project's own agent, and the heading is one step
    // back up from it.
    press(&amx, &view, "C-s");
    amx.until("the project headings", || {
        screen(&amx, &view).contains("~/api").then_some(())
    });
    press(&amx, &view, "G");
    press(&amx, &view, "k");

    types(&amx, &view, "n");
    types(&amx, &view, "port the importer");
    let drawn = amx.until("the task on the screen", || {
        let drawn = screen(&amx, &view);
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
fn the_composer_folds_a_long_paste_and_starts_the_task_it_stands_for() {
    let amx = Harness::new();
    let view = a_view_that_dispatches(&amx, "happy-turn");

    // Pasted at the list, where without bracketing every line of it would be
    // read as the keys it is made of and the first newline would dispatch.
    let pasted = format!("{}\n", twenty_rows());
    pastes(&amx, &view, &pasted);

    let drawn = amx.until("the marker the paste folded into", || {
        let drawn = screen(&amx, &view);
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

    // And the enter afterwards is what dispatches, once, with what the marker
    // stands for rather than the marker.
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

    // Twenty rows made a newline at a time, which is the way a line grows past
    // what the composer can show without a paste to fold.
    types(&amx, &view, "n");
    for (n, row) in twenty_rows().lines().enumerate() {
        if n > 0 {
            press(&amx, &view, "C-j");
        }
        types(&amx, &view, row);
    }
    let drawn = amx.until("the last row of the line", || {
        let drawn = screen(&amx, &view);
        drawn.contains("row-20").then_some(drawn)
    });

    // Ten rows, or a third of the terminal where that is less. The cursor is
    // at the end of the last row, so the rows above the cap are the ones that
    // scrolled.
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
        screen(&amx, &view)
            .contains("❯ port the importer")
            .then_some(())
    });

    // The chord a terminal that will not send alt+enter has instead: 0x0A,
    // which is ctrl+j once raw mode has stopped the tty turning it into a
    // carriage return.
    press(&amx, &view, "C-j");
    types(&amx, &view, "and its tests");
    let drawn = amx.until("the second row of the task", || {
        let drawn = screen(&amx, &view);
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

    // The chord it stands beside, on the same line, doing the same thing.
    press(&amx, &view, "M-Enter");
    types(&amx, &view, "in one go");
    amx.until("the third row of the task", || {
        screen(&amx, &view).contains("in one go").then_some(())
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

    // A terminal that can say the shift on an enter. tmux answers the
    // modifyOtherKeys request rather than the kitty one the view makes, so the
    // pane is told to send modified keys whatever the program in it asked for,
    // in the format crossterm reads them in. Both are settled when the pane is
    // made, and an option wants a server to stand on, so a session of nothing
    // goes first and the view opens after them.
    amx.tmux(&["new-session", "-d", "--", "sh", "-c", "sleep 600"]);
    amx.tmux(&["set", "-s", "extended-keys", "always"]);
    amx.tmux(&["set", "-s", "extended-keys-format", "csi-u"]);

    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");

    types(&amx, &view, "n");
    types(&amx, &view, "port the importer");
    amx.until("the first row of the task", || {
        screen(&amx, &view)
            .contains("❯ port the importer")
            .then_some(())
    });

    press(&amx, &view, "S-Enter");
    types(&amx, &view, "and its tests");
    let drawn = amx.until("the second row of the task", || {
        let drawn = screen(&amx, &view);
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

    // A word with a letter missing in the middle of it, which is what a person
    // finds by reading the line back rather than by typing the next character.
    types(&amx, &view, "n");
    types(&amx, &view, "port the imprter");
    amx.until("the task on the screen", || {
        screen(&amx, &view)
            .contains("❯ port the imprter")
            .then_some(())
    });

    // Four presses back, which lands on the r the o belongs in front of: the
    // chevron and the space after it, and twelve characters of the line.
    for _ in 0..4 {
        press(&amx, &view, "Left");
    }
    let drawn = amx.until("the block to walk back into the line", || {
        sgr_past(&coloured(&amx, &view), "port the imp")
            .contains(&7)
            .then(|| screen(&amx, &view))
    });
    assert!(
        drawn.contains("❯ port the imprter"),
        "the character under the block keeps its cell rather than being hidden \
         by it:\n{drawn}"
    );

    types(&amx, &view, "o");
    let drawn = amx.until("the letter where the cursor was", || {
        let drawn = screen(&amx, &view);
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

    // A letter typed twice in the middle of the line and a word in it that
    // does not belong: both are mended where they are, with the end of the
    // line left alone.
    types(&amx, &view, "n");
    types(&amx, &view, "port thee legacy importer");
    amx.until("the task on the screen", || {
        screen(&amx, &view)
            .contains("❯ port thee legacy importer")
            .then_some(())
    });

    // Sixteen presses back, which stands the cursor on the space after the
    // doubled letter, and one backspace to take the letter behind it.
    let mut keys = vec!["send-keys", "-t", &view];
    keys.extend(std::iter::repeat_n("Left", 16));
    amx.tmux(&keys);
    press(&amx, &view, "BSpace");
    let drawn = amx.until("the doubled letter to go", || {
        let drawn = screen(&amx, &view);
        drawn
            .contains("❯ port the legacy importer")
            .then_some(drawn)
    });
    assert!(
        !drawn.contains("importe "),
        "backspace takes the character behind the cursor and not the last one \
         on the line:\n{drawn}"
    );

    // Then forward to the front of the last word, where ctrl+w takes the whole
    // of the word behind it in one press.
    let mut keys = vec!["send-keys", "-t", &view];
    keys.extend(std::iter::repeat_n("Right", 8));
    amx.tmux(&keys);
    press(&amx, &view, "C-w");
    let drawn = amx.until("the word to go", || {
        let drawn = screen(&amx, &view);
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

/// A skill of the person's own, where claude loads them from: the one thing
/// `/rev` could mean on a line typed in this home.
fn a_skill_called_review(amx: &Harness) {
    a_file_saying(
        &amx.home().join(".claude/skills/review/SKILL.md"),
        "Read the diff.",
    );
}

/// And an agent of their own, which is what the other mark asks for.
fn an_agent_called_scout(amx: &Harness) {
    a_file_saying(
        &amx.home().join(".claude/agents/scout.md"),
        "Goes and looks.",
    );
}

/// A file saying `about` about itself, in the frontmatter a suggestion reads.
fn a_file_saying(path: &std::path::Path, about: &str) {
    std::fs::create_dir_all(path.parent().expect("a directory")).expect("the directory");
    std::fs::write(path, format!("---\ndescription: {about}\n---\n\nwords\n"))
        .expect("the file the vendor loads");
}

#[test]
fn the_composer_completes_the_word_under_the_cursor_out_of_the_vendors_files() {
    let amx = Harness::new();
    // What `/rev` could mean is whatever is in the vendor's own directories at
    // the moment somebody types it, so the word is looked up rather than
    // guessed at.
    a_skill_called_review(&amx);
    an_agent_called_scout(&amx);

    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");
    types(&amx, &view, "n");
    types(&amx, &view, "/rev");
    amx.until("the word on the line", || {
        screen(&amx, &view).contains("❯ /rev").then_some(())
    });

    press(&amx, &view, "Tab");
    let drawn = amx.until("the word completed", || {
        let drawn = screen(&amx, &view);
        drawn.contains("❯ /review").then_some(drawn)
    });
    assert!(
        agents(&amx).is_empty(),
        "tab finishes the word rather than the line:\n{drawn}"
    );

    // The other mark, on the same line: `/` is something the vendor runs and
    // `@` is one of the agents it can be told to be.
    types(&amx, &view, "@sco");
    press(&amx, &view, "Tab");
    amx.until("the second word completed", || {
        screen(&amx, &view)
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
    // The one agent claude loads on this machine, which is what a line with
    // the mark on it is answered with.
    an_agent_called_scout(&amx);

    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");
    types(&amx, &view, "n");
    amx.until("the line", || {
        screen(&amx, &view).contains("TASK").then_some(())
    });

    // Nothing is typed, so there is no word for tab to take: it writes the
    // mark that asks for one, and the band opens on what answers to it.
    press(&amx, &view, "Tab");
    let drawn = amx.until("the agents under the line", || {
        let drawn = screen(&amx, &view);
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

    // And the band is the one a typed mark opens, so the next tab takes the
    // word the choice is standing on.
    press(&amx, &view, "Tab");
    amx.until("the agent on the line", || {
        screen(&amx, &view).contains("❯ @scout").then_some(())
    });
}

#[test]
fn the_composer_offers_the_projects_files_where_the_vendor_has_no_agents() {
    let amx = Harness::new();
    // Nothing of the vendor's answers to the mark on this machine, and a path
    // is the other thing it is for: what the band holds is what is in the
    // directory the line will run in.
    a_file_saying(&amx.home().join("notes/plan.md"), "What to do first.");

    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");
    types(&amx, &view, "n");
    amx.until("the line", || {
        screen(&amx, &view).contains("TASK").then_some(())
    });

    press(&amx, &view, "Tab");
    amx.until("the files under the line", || {
        screen(&amx, &view).contains("@notes/").then_some(())
    });
}

#[test]
fn the_composer_completes_a_file_of_the_project_the_agent_will_run_in() {
    let amx = Harness::new();
    // The view is opened in the project, so what `@not` could mean is what is
    // on the disk under it at the moment somebody types the word. No agent of
    // the vendor's answers to it, and a path is the other thing the mark is
    // for.
    a_file_saying(&amx.home().join("notes/plan.md"), "What to do first.");

    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");
    types(&amx, &view, "n");
    types(&amx, &view, "read @not");
    press(&amx, &view, "Tab");
    // A directory carries the separator that says the path may go on, and
    // nothing after it: the next thing typed is more of the same word.
    amx.until("the directory completed", || {
        screen(&amx, &view).contains("❯ read @notes/").then_some(())
    });

    types(&amx, &view, "pl");
    press(&amx, &view, "Tab");
    amx.until("the file completed", || {
        screen(&amx, &view)
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

    // The same path with a `~` in front of it, which is the home directory
    // wherever the line is typed. Nothing is called `~`, so a word that
    // completed under one was read as the directory it stands for.
    types(&amx, &view, "n");
    types(&amx, &view, "read @~/notes/pl");
    press(&amx, &view, "Tab");
    amx.until("the file under the home directory", || {
        screen(&amx, &view)
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
    // Two things `/rev` could mean, so the band is a list and one of them is
    // the one the choice is standing on.
    a_skill_called_review(&amx);
    a_file_saying(
        &amx.home().join(".claude/skills/revise/SKILL.md"),
        "Say it again.",
    );

    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");
    types(&amx, &view, "n");
    types(&amx, &view, "/rev");
    // The whole word, because the band answers to every letter of it: two
    // suggestions stand under `/r` as well, and they are not what this is
    // reading.
    let drawn = amx.until("the band under the line", || {
        let drawn = screen(&amx, &view);
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
    // A skill of the person's own, called what one of claude's own commands is
    // called: the word both of them answer to is the one that has to be
    // offered once.
    a_skill_called_review(&amx);

    let view = a_view_that_dispatches_as_claude(&amx, "worktrees = false\n");
    types(&amx, &view, "n");
    types(&amx, &view, "/sec");

    // Nothing in anybody's directories answers to this one. It is a word
    // claude draws out of its own input, and the band holds it beside the
    // files all the same.
    let drawn = amx.until("the vendor's own word under the line", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("❯ /sec") && drawn.contains("/security-review")).then_some(drawn)
    });
    assert!(
        agents(&amx).is_empty(),
        "and reading it started nothing:\n{drawn}"
    );

    // The word they both answer to stands once, as the person's: their own
    // directories are read before the list amx measured off the vendor, and
    // what the row says is what their file says about itself.
    for _ in 0.."sec".len() {
        press(&amx, &view, "BSpace");
    }
    types(&amx, &view, "rev");
    let drawn = amx.until("the band on the word they share", || {
        let drawn = screen(&amx, &view);
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
    amx.until("the word on the line", || {
        screen(&amx, &view).contains("❯ /rev").then_some(())
    });

    // One key back from a list is the list gone, and the line is what the next
    // press of the same key is about. So enter after it sends the word as it
    // was typed rather than the one that was being offered.
    press(&amx, &view, "Escape");
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
fn the_composer_runs_the_session_as_the_agent_the_line_is_led_with() {
    let amx = Harness::new();
    // The one agent in the places claude reads on this machine, which is what
    // decides whether a word at the front of a line is one of them.
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

    // The same word naming none of them is the sentence it was typed in: what
    // the mark means past the vendor's own agents is a file, and the vendor
    // reads that itself.
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
    // The file leaves worktrees on, which is what it falls back to: a command
    // is not a conversation to keep apart from the next one, so it runs in the
    // checkout it was typed in whatever the file says.
    let view = a_view_that_dispatches_as_claude(&amx, "");

    types(&amx, &view, "n");
    types(&amx, &view, "!echo one two");
    let drawn = amx.until("the command on the screen", || {
        let drawn = screen(&amx, &view);
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

    // The one dial it does take, which is the other place it can run.
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
        let drawn = screen(&amx, &view);
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
        drawn.contains("❯ port the importer"),
        "under it the line itself:\n{drawn}"
    );
    assert_eq!(
        pane_field(&amx, &view, "#{cursor_flag}"),
        "0",
        "with the terminal's own cursor put away, so nothing of the \
         terminal's blinks in the cell amx is painting:\n{drawn}"
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
        !sgr_at(&painted, "port the importer").contains(&1),
        "the line somebody is typing carries no weight of its own, because what \
         says where somebody is on it is the block:\n{painted:?}"
    );
    assert!(
        sgr_past(&painted, "port the importer").contains(&7),
        "and the cell the next letter lands in is that cell reversed, which is \
         the whole of the block:\n{painted:?}"
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
fn lending_the_line_to_an_editor_hands_the_terminal_its_cursor_back() {
    let amx = Harness::new();
    amx.config("agent = \"claude\"\n");
    let editor = an_editor_that_waits(&amx);

    // `$VISUAL` goes, because it is read first and whoever is running the
    // tests has one of their own.
    let view = amx.in_a_terminal(&[("VISUAL", ""), ("EDITOR", &editor)], &[]);
    until_empty(&amx, &view);

    types(&amx, &view, "n");
    types(&amx, &view, "port the importer");
    amx.until("the line to be typed", || {
        screen(&amx, &view)
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
        let drawn = screen(&amx, &view);
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
        let drawn = screen(&amx, &view);
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

    press(&amx, &view, "M-a");
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
            .contains("/a name or task, or s:state")
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

#[test]
fn find_reaches_the_task_an_agent_was_started_on() {
    let amx = Harness::new();
    finished(&amx, "one-a1b", "done", 60);
    finished(&amx, "two-b2c", "done", 120);

    // The record the view reads, with the sentence somebody typed in it. The
    // ids say nothing about the work, which is the whole point: what a person
    // remembers an agent by is what they asked it for.
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
        let drawn = screen(&amx, &view);
        (drawn.contains("one-a1b") && drawn.contains("two-b2c")).then_some(())
    });

    // Typed in the case it was not written in, because a task is a sentence
    // and nobody remembers where its capitals were.
    types(&amx, &view, "/importer");
    amx.until("the wall to narrow to the task that says it", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("one-a1b") && !drawn.contains("two-b2c")).then_some(())
    });

    types(&amx, &view, "");
    for _ in 0.."importer".len() {
        press(&amx, &view, "BSpace");
    }
    types(&amx, &view, "LOGIN");
    amx.until("the other one, found past its capitals", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("two-b2c") && !drawn.contains("one-a1b")).then_some(())
    });
}
