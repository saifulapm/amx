//! Starting an agent, and what that leaves behind.

mod common;

use common::{AMX, Harness};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Output;

/// `amx new`, with the vendor pointed at a scenario.
fn new(amx: &Harness, scenario: &str, args: &[&str]) -> Output {
    amx.amx_command(&[&["new"], args].concat())
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario(scenario))
        .output()
        .expect("running amx new")
}

/// `amx new`, with the task typed at its stdin rather than on its command
/// line.
fn new_typed_at(amx: &Harness, scenario: &str, args: &[&str], typed: &str) -> Output {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = amx
        .amx_command(&[&["new"], args].concat())
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario(scenario))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("running amx new");
    child
        .stdin
        .take()
        .expect("stdin was asked for")
        .write_all(typed.as_bytes())
        .expect("typing the task at amx");
    child.wait_with_output().expect("waiting for amx new")
}

/// `amx new`, with the vendor's stand-in installed under the name the dial
/// table knows.
///
/// The table is keyed by the program an agent command runs, and the program it
/// has an entry for is claude. A spawn that wants a dial turned has to be
/// launching something by that name, so the stand-in is copied under it into a
/// directory of this harness's own and put in front of PATH. The pane resolves
/// the command through the environment `new` was run with, which is how the
/// copy is the one that runs.
fn new_as_claude(amx: &Harness, scenario: &str, args: &[&str]) -> Output {
    let bin = amx.home().join("bin");
    std::fs::create_dir_all(&bin).expect("a directory for the stand-in");
    std::fs::copy(amx.mock(), bin.join("claude")).expect("the stand-in under claude's name");

    amx.amx_command(&[&["new"], args].concat())
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario(scenario))
        .env("PATH", path_with_the_stand_in(amx))
        .output()
        .expect("running amx new")
}

/// The PATH the stand-in is found on, for a command that starts an agent
/// `new_as_claude` started once already: a resume and a fork launch what the
/// record names, and what it names is claude.
fn path_with_the_stand_in(amx: &Harness) -> String {
    format!(
        "{}:{}",
        amx.home().join("bin").display(),
        std::env::var("PATH").unwrap_or_default()
    )
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

/// What the vendor's own process says it was called with.
fn argv_of(amx: &Harness, id: &str) -> String {
    let pane = amx.pane_of(id);
    amx.until("the vendor to say how it was called", || {
        amx.capture(&pane)
            .lines()
            .find(|line| line.starts_with("argv:"))
            .map(str::to_string)
    })
}

/// A process's real environment, read from the kernel rather than from
/// anything amx wrote down -- the only way to see what a pane started with
/// underneath whatever amx laid over it.
fn pane_environ(pid: &str) -> std::collections::BTreeMap<String, String> {
    let raw = std::fs::read(format!("/proc/{pid}/environ"))
        .unwrap_or_else(|e| panic!("reading /proc/{pid}/environ: {e}"));
    raw.split(|&byte| byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let text = String::from_utf8_lossy(entry);
            let (name, value) = text.split_once('=').expect("NAME=VALUE");
            (name.to_string(), value.to_string())
        })
        .collect()
}

/// The environment of the pane this agent is in, once the vendor is the
/// process in it.
///
/// Waiting for the stand-in to say how it was called is waiting for `_boot` to
/// have read the boot file and exec'd the vendor, which is the moment the
/// pane's own environment is the one amx handed over.
fn pane_env(amx: &Harness, id: &str) -> std::collections::BTreeMap<String, String> {
    argv_of(amx, id);
    let pid = amx.tmux(&[
        "display-message",
        "-p",
        "-t",
        &amx.pane_of(id),
        "#{pane_pid}",
    ]);
    pane_environ(&pid)
}

/// What a `ls --json` row prints under a key. A field the record has nothing
/// for is printed null; one the shape does not carry at all is missing, and a
/// caller reading it off the row cannot tell those apart -- so this panics on
/// the second rather than handing back the first.
fn printed<'a>(row: &'a Value, key: &str) -> &'a Value {
    row.as_object()
        .expect("a row is an object")
        .get(key)
        .unwrap_or_else(|| panic!("the listing prints no `{key}`: {row}"))
}

/// The row `amx ls --json` prints for this agent, where it has one.
fn listed(amx: &Harness, id: &str) -> Option<Value> {
    let out = amx.amx(&["ls", "--json"]);
    assert!(
        out.status.success(),
        "amx ls: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rows: Vec<Value> = serde_json::from_slice(&out.stdout).expect("the listing is json");
    rows.into_iter().find(|row| row["id"] == id)
}

fn id_of(out: &Output) -> String {
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout);
    let id = printed.trim().to_string();
    assert!(!id.is_empty(), "new prints the id and nothing else");
    assert_eq!(printed.lines().count(), 1, "one line: {printed:?}");
    id
}

#[test]
fn new_starts_an_agent_and_prints_its_id() {
    let amx = Harness::new();
    let mock = amx.mock();
    let out = new(
        &amx,
        "happy-turn",
        &["--no-worktree", "--agent", &mock, "fix the login bug"],
    );

    let id = id_of(&out);
    assert!(id.starts_with("fix-the-login-bug-"), "{id}");

    let state = amx.until_state(&id, "idle");
    assert_eq!(state["result"], "the tests pass now");

    let meta = amx.meta(&id);
    assert_eq!(meta["task"], "fix the login bug");
    assert_eq!(meta["socket"]["name"], amx.socket());
    assert!(
        amx.pane_alive(meta["pane"].as_str().unwrap()),
        "the pane is on the server the record names"
    );
}

#[test]
fn new_leaves_the_session_for_a_hook_to_report_from_a_vendor_with_no_start_flag() {
    // claude declares no start flag of its own -- its SessionStart hook is
    // the one thing that ever learns which session it opened, so the record
    // waits on it rather than guessing a session amx never told the vendor to
    // use. The stable property is that nothing amx minted ever lands in
    // meta.session; waiting for the hook's own report and checking what it
    // wrote proves that without racing it for an empty field.
    let amx = Harness::new();
    let id = id_of(&new_as_claude(
        &amx,
        "a-dispatched-worker",
        &["--no-worktree", "--agent", "claude", "fix the login bug"],
    ));

    let session = amx.until("the hook to report a session", || {
        amx.meta(&id)["session"].as_str().map(str::to_string)
    });
    assert_ne!(session, id, "nothing amx minted ever reaches meta.session");
}

#[test]
fn new_records_the_command_it_launched_the_agent_with() {
    // Which vendor is in the pane is settled at the spawn, from the flag, the
    // config and the vendor amx falls back to. Nothing after the spawn can
    // work it out again, so the record keeps it.
    let amx = Harness::new();
    let id = id_of(&new_as_claude(
        &amx,
        "happy-turn",
        &["--no-worktree", "--agent", "claude", "fix the login bug"],
    ));
    assert_eq!(amx.meta(&id)["agent"], "claude");

    // A shell command runs no vendor, and a record saying it ran one would be
    // read as an agent to resume or fork.
    let ran = id_of(
        &amx.amx_command(&["new", "--exec", "true"])
            .output()
            .expect("running amx new --exec"),
    );
    assert!(
        amx.meta(&ran)["agent"].is_null(),
        "{}",
        amx.meta(&ran)["agent"]
    );
}

#[test]
fn new_takes_the_task_from_a_file() {
    // A brief worth writing down is one nobody wants to quote into a shell,
    // and what the row used to say was "Read /tmp/x and execute it exactly".
    let amx = Harness::new();
    let mock = amx.mock();
    let brief = amx.home().join("brief.md");
    std::fs::write(&brief, "fix the login bug\n").expect("a brief to read");
    let named = brief.to_string_lossy().into_owned();

    let id = id_of(&new(
        &amx,
        "happy-turn",
        &["--no-worktree", "--agent", &mock, "--file", &named],
    ));

    // The file's text is the task everywhere a typed one would have been: the
    // id cut from it, the row, the handoff, and the argv the vendor is handed.
    // The newline the editor wrote is not part of it.
    assert!(id.starts_with("fix-the-login-bug-"), "{id}");
    assert_eq!(amx.meta(&id)["task"], "fix the login bug");
    assert_eq!(amx.handoff(&id)["task"], "fix the login bug");
    assert_eq!(
        command_of(&amx, &id).last().map(String::as_str),
        Some("fix the login bug")
    );
    assert_eq!(amx.until_state(&id, "idle")["result"], "the tests pass now");
}

#[test]
fn new_takes_the_task_from_stdin_for_a_bare_dash() {
    // The brief a coordinator has in hand rather than on disk: a heredoc or a
    // pipe is the whole of what it takes.
    let amx = Harness::new();
    let mock = amx.mock();

    let id = id_of(&new_typed_at(
        &amx,
        "happy-turn",
        &["--no-worktree", "--agent", &mock, "--file", "-"],
        "fix the login bug\n\nthe test is in tests/login.rs\n",
    ));

    assert_eq!(
        amx.meta(&id)["task"],
        "fix the login bug\n\nthe test is in tests/login.rs",
        "read whole, with the last newline off and everything inside it kept"
    );
}

#[test]
fn new_refuses_a_file_with_nothing_in_it_the_way_it_refuses_an_empty_task() {
    let amx = Harness::new();
    let mock = amx.mock();
    let empty = amx.home().join("empty.md");
    std::fs::write(&empty, "\n").expect("a file with nothing in it");
    let named = empty.to_string_lossy().into_owned();

    let refused = new(
        &amx,
        "happy-turn",
        &["--no-worktree", "--agent", &mock, "--file", &named],
    );
    assert_eq!(
        refused.status.code(),
        Some(64),
        "a malformed command line, the same as an empty argument"
    );
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("something to do"), "{said}");

    // A file amx cannot read is named, because the name is what was mistyped.
    let missing = amx.home().join("nowhere.md").to_string_lossy().into_owned();
    let mistyped = new(
        &amx,
        "happy-turn",
        &["--no-worktree", "--agent", &mock, "--file", &missing],
    );
    assert_eq!(mistyped.status.code(), Some(64));
    assert!(
        String::from_utf8_lossy(&mistyped.stderr).contains("nowhere.md"),
        "{}",
        String::from_utf8_lossy(&mistyped.stderr)
    );

    // And a task typed beside a file is two tasks, which is none.
    let both = new(
        &amx,
        "happy-turn",
        &[
            "--no-worktree",
            "--agent",
            &mock,
            "--file",
            &named,
            "fix the login bug",
        ],
    );
    assert_eq!(both.status.code(), Some(64));

    assert!(
        !amx.state_root().exists() || amx.state_root().read_dir().unwrap().next().is_none(),
        "and none of the three minted an id"
    );
}

/// `amx new`, with `$VISUAL` pointed at a script standing in for the editor
/// somebody would have written the task in.
///
/// A script rather than an editor: what `$VISUAL` names is run with the file
/// behind it, so a script that writes the file is exactly what closing an
/// editor on a task looks like from amx's side, and it is the only editor a
/// test can be sure of.
fn new_edited_by(amx: &Harness, scenario: &str, args: &[&str], script: &str) -> Output {
    use std::os::unix::fs::PermissionsExt;

    let editor = amx.home().join("editor.sh");
    std::fs::write(&editor, script).expect("an editor for the task");
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755))
        .expect("an editor that runs");

    amx.amx_command(&[&["new"], args].concat())
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario(scenario))
        .env("VISUAL", &editor)
        .output()
        .expect("running amx new")
}

/// Every agent amx has a record of, which after a refusal is none.
fn every_row(amx: &Harness) -> Vec<Value> {
    let out = amx.amx(&["ls", "--json"]);
    assert!(
        out.status.success(),
        "amx ls: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("the listing is json")
}

#[test]
fn new_takes_the_task_from_the_editor() {
    // The brief nobody has written yet: `--file` for the file that does not
    // exist, opened the way the view's own `ctrl+g` opens one.
    let amx = Harness::new();
    let mock = amx.mock();

    let id = id_of(&new_edited_by(
        &amx,
        "happy-turn",
        &["--no-worktree", "--agent", &mock, "--edit"],
        "#!/bin/sh\nprintf 'fix the login bug\\n' > \"$1\"\n",
    ));

    // What was left in the file is the task everywhere a typed one would have
    // been, with the newline the editor wrote taken off.
    assert!(id.starts_with("fix-the-login-bug-"), "{id}");
    assert_eq!(amx.meta(&id)["task"], "fix the login bug");
    assert_eq!(amx.handoff(&id)["task"], "fix the login bug");
    assert_eq!(
        command_of(&amx, &id).last().map(String::as_str),
        Some("fix the login bug")
    );
    assert_eq!(amx.until_state(&id, "idle")["result"], "the tests pass now");
}

#[test]
fn new_starts_nothing_where_the_editor_would_have_none_of_it() {
    // An editor that exits on you is somebody saying no to the spawn, which is
    // a spawn that did not happen rather than a command line nobody could
    // read: it exits 1 and leaves no id behind.
    let amx = Harness::new();
    let mock = amx.mock();

    let refused = new_edited_by(
        &amx,
        "happy-turn",
        &["--no-worktree", "--agent", &mock, "--edit"],
        "#!/bin/sh\nexit 1\n",
    );

    assert_eq!(refused.status.code(), Some(1));
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.starts_with("amx new: "), "{said}");
    assert!(said.contains("left the line as it was"), "{said}");
    assert!(every_row(&amx).is_empty(), "and nothing was minted for it");
}

#[test]
fn new_refuses_an_editor_closed_on_nothing_the_way_it_refuses_an_empty_task() {
    // The file amx opened is empty, and an editor closed without writing
    // anything into it is an empty task: a malformed command line, wherever
    // the emptiness was typed.
    let amx = Harness::new();
    let mock = amx.mock();

    let refused = new_edited_by(
        &amx,
        "happy-turn",
        &["--no-worktree", "--agent", &mock, "--edit"],
        "#!/bin/sh\nexit 0\n",
    );

    assert_eq!(refused.status.code(), Some(64));
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("something to do"), "{said}");
    assert!(every_row(&amx).is_empty(), "and nothing was minted for it");
}

#[test]
fn a_running_command_says_what_it_last_printed() {
    // A command has no vendor: nothing reports on it, and no document amx
    // holds describes a screen of somebody else's program. What is true of it
    // is what tmux can say -- the pane is still there, so the command is still
    // running -- and the line the row shows is the last one it printed.
    let amx = Harness::new();
    let id = "print-two-a1b";
    let out = amx
        .amx_command(&[
            "new",
            "--name",
            id,
            "--exec",
            r#"printf "one\ntwo\n"; sleep 30"#,
        ])
        .output()
        .expect("running amx new --exec");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let row = amx.until("the row to say what the command printed", || {
        listed(&amx, id).filter(|row| !row["summary"].is_null())
    });
    assert_eq!(row["state"], "working", "{row}");
    assert_eq!(row["evidence"], "screen", "{row}");
    assert_eq!(
        row["summary"], "two",
        "the last line it printed rather than the first: {row}"
    );
}

#[test]
fn a_command_that_has_exited_ends_by_its_exit_code() {
    // The pane is where a command is read from only while it is in it. How the
    // command ended is the record's, and nothing read off a screen stands in
    // front of that.
    let amx = Harness::new();
    let id = "run-tests-a1b";
    let out = amx
        .amx_command(&["new", "--name", id, "--exec", "exit 3"])
        .output()
        .expect("running amx new --exec");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let ended = amx.until_state(id, "failed");
    assert_eq!(ended["exit"], 3);

    let row = amx.until("the row to say how the command ended", || {
        listed(&amx, id).filter(|row| row["state"] == "failed")
    });
    assert_eq!(row["exit"], 3, "{row}");
    assert_eq!(row["evidence"], "record", "{row}");
}

#[test]
fn a_commands_output_is_kept_beside_its_record() {
    // Nothing reports on a command: it has no vendor and no hooks, so what it
    // printed is on its screen and nowhere else, and a screen is the first
    // thing a pane throws away. Its boot pipes the pane into a file of the
    // record's before the command starts, so the first line is in it as well
    // as the last.
    let amx = Harness::new();
    let id = "print-two-b2c";
    let out = amx
        .amx_command(&["new", "--name", id, "--exec", r#"printf "one\ntwo\n""#])
        .output()
        .expect("running amx new --exec");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    amx.until_state(id, "done");
    let printed = amx.until("what the command printed to reach the file", || {
        let text = std::fs::read_to_string(amx.agent_dir(id).join("output")).ok()?;
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        (lines.len() >= 2).then_some(lines)
    });
    assert_eq!(printed, ["one", "two"]);
}

#[test]
fn an_agents_pane_is_piped_into_its_record() {
    // A vendor's pane is a full-screen drawing, so what is kept of it is
    // bounded -- but kept it is, because the one moment it matters is the
    // vendor that dies before it draws anything and says why on the way out.
    let amx = Harness::new();
    let mock = amx.mock();
    let id = id_of(&new(
        &amx,
        "a-dispatched-worker",
        &["--no-worktree", "--agent", &mock, "fix the login bug"],
    ));

    // The vendor saying how it was called is the boot already past the point
    // where it would have attached a pipe.
    argv_of(&amx, &id);
    let piped = amx.tmux(&[
        "display-message",
        "-p",
        "-t",
        &amx.pane_of(&id),
        "#{pane_pipe}",
    ]);
    assert_eq!(piped, "1", "the agent's pane is piped into its record");
    assert!(
        amx.agent_dir(&id).join("output").exists(),
        "and the file is there to catch its dying words"
    );
}

#[test]
fn a_vendor_that_dies_before_it_speaks_leaves_its_words_on_the_record() {
    // Nothing reports on a vendor that exits before its first hook: no session
    // and no transcript reach the record, and its pane is gone. The bytes its
    // boot kept are the only account of why it went, and `amx logs` hands them
    // back where it used to say it captured no answer.
    let amx = Harness::new();
    let mock = amx.mock();
    let id = id_of(&new(
        &amx,
        "dies-before-its-first-hook",
        &["--no-worktree", "--agent", &mock, "fix the login bug"],
    ));

    amx.until_state(&id, "failed");
    assert!(
        amx.meta(&id)["session"].is_null(),
        "the vendor never spoke: {}",
        amx.meta(&id)
    );

    // `head` flushes when the pane closes, which is a moment after `_exit`
    // writes the state this waited for.
    amx.until("the dying words to reach the record", || {
        let out = amx.amx(&["logs", &id]);
        String::from_utf8_lossy(&out.stdout)
            .contains("could not read the state file")
            .then_some(())
    });
}

#[test]
fn the_task_never_rides_the_tmux_command_line() {
    // A task is arbitrary text and a tmux command line is not a place for it.
    // It travels in a file only its owner can read, and the pane is started
    // with nothing but an id.
    let amx = Harness::new();
    let mock = amx.mock();
    let out = new(
        &amx,
        "happy-turn",
        &[
            "--no-worktree",
            "--agent",
            &mock,
            "fix $(whoami); rm -rf \"everything\"",
        ],
    );
    let id = id_of(&out);
    let pane = amx.meta(&id)["pane"].as_str().unwrap().to_string();

    let started = amx.tmux(&[
        "display-message",
        "-p",
        "-t",
        &pane,
        "#{pane_start_command}",
    ]);
    assert!(started.contains("_boot"), "{started}");
    assert!(started.contains(&id), "{started}");
    assert!(
        !started.contains("$(") && !started.contains("rm -rf"),
        "nothing of the task's own syntax is on the command line: {started}"
    );

    let handoff = amx.handoff(&id);
    assert_eq!(handoff["task"], "fix $(whoami); rm -rf \"everything\"");
    assert_eq!(
        mode(&amx.agent_dir(&id).join("handoff.json")) & 0o777,
        0o600,
        "what a command was launched with is not everyone's to read"
    );
}

#[test]
fn the_agent_gets_the_environment_new_was_run_with() {
    // A tmux server started an hour ago has an hour-old environment. The
    // agent's comes from the command that asked for it, not from the server.
    // The environment no longer rides the handoff, so this reads the pane's
    // real environment off the kernel, the same way `boot_strips_a_marker...`
    // does below.
    let amx = Harness::new();
    let mock = amx.mock();
    let out = amx
        .amx_command(&[
            "new",
            "--no-worktree",
            "--agent",
            &mock,
            "fix the login bug",
        ])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("a-dispatched-worker"))
        .env("ANTHROPIC_MODEL", "opus")
        .env("TMUX_PANE", "%404")
        .output()
        .expect("running amx new");

    let id = id_of(&out);

    // Waiting for the vendor to say how it was called is waiting for `_boot`
    // to have already read the boot file, unlinked it and exec'd the vendor
    // with what it held.
    argv_of(&amx, &id);
    let pid = amx.tmux(&[
        "display-message",
        "-p",
        "-t",
        &amx.pane_of(&id),
        "#{pane_pid}",
    ]);
    let env = pane_environ(&pid);

    assert_eq!(
        env.get("ANTHROPIC_MODEL").map(String::as_str),
        Some("opus"),
        "a variable exported at `new` reaches the agent: {env:?}"
    );
    assert_ne!(
        env.get("TMUX_PANE").map(String::as_str),
        Some("%404"),
        "tmux's own variables belong to the pane it makes, not to the one it left: {env:?}"
    );
    assert_eq!(
        env.get("AMX_ID").map(String::as_str),
        Some(id.as_str()),
        "and the agent knows who it is"
    );
    assert!(
        !amx.agent_dir(&id).join("boot-env.json").exists(),
        "no file under the agent's directory holds the spawner's environment \
         once the pane is up"
    );
}

/// `amx new` with an agent's own id already in the environment, the way a
/// pane amx started carries it.
fn spawned_inside(amx: &Harness, scenario: &str, parent: &str, args: &[&str]) -> Output {
    amx.amx_command(&[&["new"], args].concat())
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario(scenario))
        .env("AMX_ID", parent)
        .output()
        .expect("running amx new")
}

/// `amx sub --bg` typed in `parent`'s pane: the one verb that records a
/// parent. The id is the line it leaves on stderr.
fn sub_inside(amx: &Harness, scenario: &str, parent: &str, args: &[&str]) -> Output {
    amx.amx_command(&[&["sub", "--bg"], args].concat())
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario(scenario))
        .env("AMX_ID", parent)
        .output()
        .expect("running amx sub")
}

fn id_on(out: &Output) -> String {
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "amx sub: {said}");
    assert_eq!(said.lines().count(), 1, "one line on stderr: {said:?}");
    said.trim().to_string()
}

#[test]
fn new_inside_a_pane_is_still_a_root() {
    // A child is asked for with `amx sub`, never inherited: a pane amx started
    // carries its own id in the environment, and `amx new` typed inside it
    // records no parent all the same.
    let amx = Harness::new();
    let mock = amx.mock();
    let parent = id_of(&new(
        &amx,
        "a-dispatched-worker",
        &["--no-worktree", "--agent", &mock, "the parent"],
    ));

    let child = id_of(&spawned_inside(
        &amx,
        "a-dispatched-worker",
        &parent,
        &["--no-worktree", "--agent", &mock, "the child"],
    ));

    let theirs = amx.meta(&child);
    assert_eq!(
        theirs["parent"],
        Value::Null,
        "the pane's id is not a parent"
    );
    assert_eq!(theirs["depth"], 0, "a root beside the one it was typed in");
    let ours = amx.meta(&parent);
    assert_eq!(
        ours["parent"],
        Value::Null,
        "a person's shell is nobody's child"
    );
    assert_eq!(ours["depth"], 0);
}

#[test]
fn a_childs_pane_is_told_its_parent_and_its_depth() {
    let amx = Harness::new();
    let mock = amx.mock();
    let parent = id_of(&new(
        &amx,
        "a-dispatched-worker",
        &["--no-worktree", "--agent", &mock, "the parent"],
    ));
    let child = id_on(&sub_inside(
        &amx,
        "a-dispatched-worker",
        &parent,
        &["--no-worktree", "--agent", &mock, "the child"],
    ));

    let env = pane_env(&amx, &child);
    assert_eq!(
        env.get("AMX_PARENT").map(String::as_str),
        Some(parent.as_str()),
        "a child can name its parent: {env:?}"
    );
    assert_eq!(
        env.get("AMX_PARENT_DIR").map(String::as_str),
        Some(amx.agent_dir(&parent).to_string_lossy().as_ref()),
        "and read the record itself: {env:?}"
    );
    assert_eq!(env.get("AMX_DEPTH").map(String::as_str), Some("1"));
}

#[test]
fn a_spawn_past_the_depth_is_refused_before_anything_is_claimed() {
    let amx = Harness::new();
    let mock = amx.mock();
    let root = id_of(&new(
        &amx,
        "a-dispatched-worker",
        &["--no-worktree", "--agent", &mock, "the root"],
    ));
    let child = id_on(&sub_inside(
        &amx,
        "a-dispatched-worker",
        &root,
        &["--no-worktree", "--agent", &mock, "the child"],
    ));

    let grandchild = sub_inside(
        &amx,
        "a-dispatched-worker",
        &child,
        &["--no-worktree", "--agent", &mock, "the grandchild"],
    );
    assert_eq!(
        grandchild.status.code(),
        Some(2),
        "the default subagent_depth is 1: {}",
        String::from_utf8_lossy(&grandchild.stderr)
    );
    let said = String::from_utf8_lossy(&grandchild.stderr);
    assert!(said.contains("subagent_depth"), "names the key: {said:?}");
    assert_eq!(
        std::fs::read_dir(amx.state_root()).unwrap().count(),
        2,
        "and nothing was claimed"
    );

    // `amx new` typed in the same pane is a root, and a root is never bounded.
    let peer = id_of(&spawned_inside(
        &amx,
        "a-dispatched-worker",
        &child,
        &["--no-worktree", "--agent", &mock, "a peer"],
    ));
    assert_eq!(amx.meta(&peer)["parent"], Value::Null);
    assert_eq!(amx.meta(&peer)["depth"], 0);
}

#[test]
fn every_pane_a_harness_starts_carries_what_its_table_sets() {
    // A second account of one vendor, or a proxy in front of it, is written
    // down once in that harness's table instead of in a wrapper script in
    // front of every spawn -- and it reaches every pane amx opens on that
    // harness, whichever verb opened it. amx's own variables stand over it:
    // an agent whose AMX_ID a file changed would file its events under
    // somebody else.
    let amx = Harness::new();
    amx.config("[claude.env]\nAMX_HARNESS_PROOF = \"~/proof\"\nAMX_ID = \"somebody-else\"\n");
    let home = amx.home().to_path_buf();
    let proof = home.join("proof").to_string_lossy().into_owned();
    let carries = |env: &std::collections::BTreeMap<String, String>, id: &str| {
        assert_eq!(
            env.get("AMX_HARNESS_PROOF").map(String::as_str),
            Some(proof.as_str()),
            "the table's pair reaches the pane, with the ~ spelled out: {env:?}"
        );
        assert_eq!(
            env.get("AMX_ID").map(String::as_str),
            Some(id),
            "and the agent is still the one amx started: {env:?}"
        );
    };

    let id = id_of(&new_as_claude(
        &amx,
        "a-dispatched-worker",
        &[
            "--no-worktree",
            "--dir",
            &home.to_string_lossy(),
            "--agent",
            "claude",
            "fix the login bug",
        ],
    ));
    carries(&pane_env(&amx, &id), &id);

    // A resume and a fork read the config of the project the agent ran in,
    // and neither is given the vendor's session until the hook reports one.
    amx.until("the hook to report a session", || {
        amx.meta(&id)["session"].as_str().map(str::to_string)
    });
    let stopped = amx.amx(&["stop", &id, "--force"]);
    assert!(
        stopped.status.success(),
        "amx stop: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );

    let again = amx
        .amx_command(&["resume", &id])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("continues-a-session"))
        .env("PATH", path_with_the_stand_in(&amx))
        .output()
        .expect("running amx resume");
    assert!(
        again.status.success(),
        "amx resume: {}",
        String::from_utf8_lossy(&again.stderr)
    );
    carries(&pane_env(&amx, &id), &id);

    let forked = amx
        .amx_command(&["fork", &id])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("continues-a-session"))
        .env("PATH", path_with_the_stand_in(&amx))
        .output()
        .expect("running amx fork");
    assert!(
        forked.status.success(),
        "amx fork: {}",
        String::from_utf8_lossy(&forked.stderr)
    );
    let copy = String::from_utf8_lossy(&forked.stdout).trim().to_string();
    carries(&pane_env(&amx, &copy), &copy);

    // The table is the harness's, and a command that is a path is no harness
    // the table has heard of.
    let other = id_of(&new(
        &amx,
        "a-dispatched-worker",
        &[
            "--no-worktree",
            "--dir",
            &home.to_string_lossy(),
            "--agent",
            &amx.mock(),
            "fix the login bug",
        ],
    ));
    let env = pane_env(&amx, &other);
    assert!(
        !env.contains_key("AMX_HARNESS_PROOF"),
        "claude's table reached a pane running something else: {env:?}"
    );
}

#[test]
fn trust_is_seeded_in_the_store_the_harness_table_points_the_agent_at() {
    // A table that moves claude's config directory moves the store the agent
    // reads its trust from, so that is the store the seeding has to write:
    // one written under the home would answer a screen the agent never reads.
    let amx = Harness::new();
    let work = amx.home().join("work");
    std::fs::create_dir_all(&work).expect("the other config directory");
    amx.config("trust = true\n[claude.env]\nCLAUDE_CONFIG_DIR = \"~/work\"\n");
    let repo = amx.a_repo();

    let id = id_of(&new_as_claude(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--agent",
            "claude",
            "fix the login bug",
        ],
    ));

    let tree = amx.meta(&id)["worktree"]
        .as_str()
        .expect("a worktree")
        .to_string();
    let key = std::fs::canonicalize(&tree)
        .expect("the tree")
        .to_string_lossy()
        .into_owned();
    let store: Value = serde_json::from_str(
        &std::fs::read_to_string(work.join(".claude.json")).expect("the store the agent reads"),
    )
    .expect("json");
    assert_eq!(
        store["projects"][&key]["hasTrustDialogAccepted"],
        serde_json::json!(true),
        "the tree is trusted where the agent will look: {store}"
    );
    assert!(
        !amx.home().join(".claude.json").exists(),
        "and nothing was written into a store the agent never reads"
    );
}

#[test]
fn boot_strips_a_marker_sitting_in_the_tmux_servers_own_environment() {
    // A snapshot taken when `new` runs only ever strips a vendor's markers
    // from what travels in the handoff. It says nothing about what the pane
    // starts with before that snapshot is laid over it, and a server first
    // started inside a claude session carries that session's markers as its
    // own baseline -- set here on the command that starts this harness's
    // server, before amx ever touches the socket.
    let amx = Harness::new();
    let state_dir = amx
        .state_root()
        .parent()
        .expect("the state root has a parent")
        .to_path_buf();
    let status = std::process::Command::new("tmux")
        .args([
            "-L",
            amx.socket(),
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "--",
            "sh",
            "-c",
            "while :; do sleep 0.05; done",
        ])
        .env("AMX_STATE_DIR", &state_dir)
        .env("HOME", amx.home())
        .env("CLAUDE_CODE_CHILD_SESSION", "1")
        .status()
        .expect("running tmux");
    assert!(status.success(), "starting the harness's server");

    let mock = amx.mock();
    let id = id_of(&new(
        &amx,
        "a-dispatched-worker",
        &["--no-worktree", "--agent", &mock, "fix the login bug"],
    ));

    // Waiting for the vendor to say how it was called is waiting for `_boot`
    // to have already exec'd it: only then is the pane's own environment the
    // one this test needs to read.
    argv_of(&amx, &id);
    let pid = amx.tmux(&[
        "display-message",
        "-p",
        "-t",
        &amx.pane_of(&id),
        "#{pane_pid}",
    ]);
    let env = pane_environ(&pid);

    assert!(
        !env.contains_key("CLAUDE_CODE_CHILD_SESSION"),
        "a marker sitting in the server's own environment reached the agent: {env:?}"
    );
    for kept in ["TMUX", "TMUX_PANE", "PATH", "HOME"] {
        assert!(
            env.contains_key(kept),
            "{kept} belongs to the pane amx made and stands: {env:?}"
        );
    }
}

#[test]
fn agents_started_from_inside_tmux_get_a_session_each() {
    let amx = Harness::new();
    let mock = amx.mock();
    let inside = amx.inside_tmux();

    let first = id_of(
        &amx.amx_command(&["new", "--no-worktree", "--agent", &mock, "the first"])
            .env("MOCK_CLAUDE_SCENARIO", amx.scenario("happy-turn"))
            .envs(inside.clone())
            .output()
            .unwrap(),
    );
    let second = id_of(
        &amx.amx_command(&["new", "--no-worktree", "--agent", &mock, "the second"])
            .env("MOCK_CLAUDE_SCENARIO", amx.scenario("happy-turn"))
            .envs(inside)
            .output()
            .unwrap(),
    );

    let session_of = |id: &str| {
        let pane = amx.meta(id)["pane"].as_str().unwrap().to_string();
        amx.tmux(&["display-message", "-p", "-t", &pane, "#{session_name}"])
    };
    assert_eq!(session_of(&first), format!("amx-{first}"));
    assert_eq!(session_of(&second), format!("amx-{second}"));
    assert_eq!(
        amx.tmux(&[
            "show-options",
            "-t",
            &format!("amx-{first}"),
            "-v",
            "destroy-unattached"
        ]),
        "off",
        "and each of them outlives whoever is not watching it"
    );
    amx.until_state(&first, "idle");
    amx.until_state(&second, "idle");
}

#[test]
fn new_gives_an_agent_its_own_worktree_in_a_repository() {
    let amx = Harness::new();
    let mock = amx.mock();
    let repo = amx.a_repo();

    let id = id_of(&new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--agent",
            &mock,
            "fix the login bug",
        ],
    ));

    let meta = amx.meta(&id);
    let worktree = Path::new(meta["worktree"].as_str().expect("a worktree"));
    assert_eq!(worktree, repo.join(".amx/worktrees").join(&id));
    assert!(
        worktree.join("README.md").exists(),
        "the repository's work is in it"
    );
    assert_eq!(meta["branch"], format!("amx/{id}"));
    assert!(
        meta["base"].as_str().unwrap().len() >= 7,
        "the commit it was cut from"
    );
    assert_eq!(meta["dir"], worktree.to_string_lossy().as_ref());
}

/// git in a repository the harness made, with none of the developer's own
/// configuration behind it.
fn git(repo: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(repo)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("running git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

#[test]
fn new_cuts_the_tree_from_the_ref_it_was_given() {
    let amx = Harness::new();
    let mock = amx.mock();
    let repo = amx.a_repo();
    let first = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["branch", "release"]);
    std::fs::write(repo.join("README.md"), "after\n").expect("a second version");
    git(&repo, &["commit", "-am", "second"]);

    let typed = id_of(&new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--base",
            "release",
            "--agent",
            &mock,
            "fix the login bug",
        ],
    ));

    let meta = amx.meta(&typed);
    assert_eq!(meta["base"], first, "the commit the ref resolved to");
    let worktree = Path::new(meta["worktree"].as_str().expect("a worktree"));
    assert_eq!(
        std::fs::read_to_string(worktree.join("README.md")).unwrap(),
        "before\n",
        "the work of the ref, and not what HEAD has since become"
    );

    // And the key, which says it for every spawn instead of for one.
    amx.config("base = \"release\"\n");
    let held = id_of(&new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--agent",
            &mock,
            "port the importer",
        ],
    ));
    assert_eq!(amx.meta(&held)["base"], first);
}

#[test]
fn new_refuses_a_base_that_names_no_commit_before_anything_is_made() {
    let amx = Harness::new();
    let mock = amx.mock();
    let repo = amx.a_repo();

    let refused = new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--base",
            "release",
            "--agent",
            &mock,
            "fix the login bug",
        ],
    );

    assert_eq!(refused.status.code(), Some(1));
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("release"), "the ref that was typed: {said}");
    assert!(!repo.join(".amx").exists(), "and no tree was cut");
    assert!(
        !amx.state_root().exists() || amx.state_root().read_dir().unwrap().next().is_none(),
        "nor an id minted for it"
    );
}

#[test]
fn new_furnishes_the_tree_before_the_pane_starts() {
    let amx = Harness::new();
    let mock = amx.mock();
    let repo = amx.a_repo();
    std::fs::write(repo.join(".env"), "TOKEN=hunter2\n").expect("what git is right not to carry");
    std::fs::create_dir(repo.join("node_modules")).expect("an install to share");
    std::fs::write(repo.join("node_modules/left-pad"), "installed\n").expect("something in it");
    amx.config(
        r#"copy = [".env"]
link = ["node_modules"]
setup = ["printf '%s\\n' \"$AMX_ID\" \"$AMX_WORKTREE\" \"$AMX_REPO\" \"$AMX_AGENT_DIR\" > furnished"]
"#,
    );

    let id = id_of(&new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--agent",
            &mock,
            "fix the login bug",
        ],
    ));

    let worktree = repo.join(".amx/worktrees").join(&id);
    assert_eq!(
        std::fs::read_to_string(worktree.join(".env")).expect("the file was copied in"),
        "TOKEN=hunter2\n"
    );
    assert!(
        std::fs::symlink_metadata(worktree.join("node_modules"))
            .expect("the directory was linked in")
            .file_type()
            .is_symlink(),
        "an install shared rather than made again"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("node_modules/left-pad")).unwrap(),
        "installed\n"
    );

    let furnished =
        std::fs::read_to_string(worktree.join("furnished")).expect("the setup command ran");
    assert_eq!(
        furnished.lines().collect::<Vec<&str>>(),
        [
            id.as_str(),
            worktree.to_str().unwrap(),
            repo.to_str().unwrap(),
            amx.agent_dir(&id).join("scratch").to_str().unwrap(),
        ],
        "under the four variables, in the tree"
    );
}

#[test]
fn new_reads_the_project_file_for_the_tree_it_furnishes() {
    // The keys are laid over the person's file a key at a time, and a project
    // says them in its own file: a spawn sent into that project furnishes the
    // tree that file asks for, whatever the person's file says. Found on
    // 2026-09-13 dogfooding, where `new` read the project's file for the cap
    // alone and cut a bare tree beside a config asking for a furnished one.
    let amx = Harness::new();
    let mock = amx.mock();
    let repo = amx.a_repo();
    std::fs::write(repo.join(".env"), "TOKEN=hunter2\n").expect("what git is right not to carry");
    std::fs::create_dir_all(repo.join(".amx")).expect("the project's own directory");
    std::fs::write(
        repo.join(".amx/config.toml"),
        "copy = [\".env\"]\nsetup = [\"touch furnished\"]\n",
    )
    .expect("the project's config");

    let id = id_of(&new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--agent",
            &mock,
            "fix the login bug",
        ],
    ));

    let worktree = repo.join(".amx/worktrees").join(&id);
    assert_eq!(
        std::fs::read_to_string(worktree.join(".env")).expect("the project's file was read"),
        "TOKEN=hunter2\n"
    );
    assert!(worktree.join("furnished").exists(), "and its setup ran");
}

#[test]
fn new_says_what_the_config_names_and_the_repository_does_not_have() {
    let amx = Harness::new();
    let mock = amx.mock();
    let repo = amx.a_repo();
    amx.config("copy = [\".env\"]\nlink = [\"node_modules\"]\n");

    let out = new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--agent",
            &mock,
            "fix the login bug",
        ],
    );

    // A config file outlives the project it was written for, so a path this
    // repository does not have is said and stepped over.
    let id = id_of(&out);
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains(".env"), "the path by name: {said}");
    assert!(said.contains("node_modules"), "{said}");
    let worktree = repo.join(".amx/worktrees").join(&id);
    assert!(
        !worktree.join(".env").exists() && !worktree.join("node_modules").exists(),
        "and nothing was made for either of them"
    );
}

#[test]
fn new_refuses_a_spawn_whose_setup_failed_and_leaves_no_tree() {
    let amx = Harness::new();
    let mock = amx.mock();
    let repo = amx.a_repo();
    amx.config("setup = [\"echo no such lockfile >&2; exit 3\", \"touch second\"]\n");

    let refused = new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--agent",
            &mock,
            "fix the login bug",
        ],
    );

    assert_eq!(refused.status.code(), Some(1));
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(
        said.contains("no such lockfile"),
        "what the command said: {said}"
    );
    assert_eq!(
        std::fs::read_dir(repo.join(".amx/worktrees"))
            .map(|trees| trees.count())
            .unwrap_or(0),
        0,
        "a tree an agent cannot work in is worse than none"
    );
    assert_eq!(
        git(&repo, &["branch", "--list", "amx/*"]),
        "",
        "nor is a branch left for it"
    );
    let listed = amx.amx(&["ls", "--json"]);
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout).trim(),
        "[]",
        "and no agent was started"
    );
}

#[test]
fn new_moves_the_uncommitted_work_into_the_tree() {
    let amx = Harness::new();
    let mock = amx.mock();
    let repo = amx.a_repo();
    std::fs::write(repo.join(".gitignore"), "/build/\n").expect("the line git draws");
    git(&repo, &["add", ".gitignore"]);
    git(&repo, &["commit", "-m", "ignore the build"]);
    std::fs::write(repo.join("README.md"), "after\n").expect("the work already in hand");
    std::fs::write(repo.join("notes.txt"), "scratch\n").expect("and a file git never heard of");
    std::fs::create_dir(repo.join("build")).expect("where the build writes");
    std::fs::write(repo.join("build/out"), "compiled\n").expect("and what it wrote");

    let id = id_of(&new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--with-changes",
            "--agent",
            &mock,
            "fix the login bug",
        ],
    ));

    let worktree = repo.join(".amx/worktrees").join(&id);
    assert_eq!(
        std::fs::read_to_string(worktree.join("README.md")).expect("the tree has the work"),
        "after\n",
        "the agent starts on what was in hand rather than on the last commit"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("notes.txt")).expect("and the new file with it"),
        "scratch\n",
        "the file no commit has ever held is most of that half hour"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "before\n",
        "and the directory it was typed in is left as that commit had it"
    );
    assert_eq!(
        git(&repo, &["status", "--porcelain"]),
        "",
        "with nothing of the work left behind it"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("build/out")).expect("the build's output stays"),
        "compiled\n",
        "since .gitignore is where that is told apart from work"
    );
    assert!(!worktree.join("build").exists());
}

#[test]
fn new_refuses_to_move_work_that_is_not_there() {
    let amx = Harness::new();
    let mock = amx.mock();
    let repo = amx.a_repo();

    let refused = new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--with-changes",
            "--agent",
            &mock,
            "fix the login bug",
        ],
    );

    assert_eq!(refused.status.code(), Some(1));
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("--with-changes"), "{said}");
    assert!(!repo.join(".amx").exists(), "and no tree was cut");
    let listed = amx.amx(&["ls", "--json"]);
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout).trim(),
        "[]",
        "nor an agent started"
    );
}

#[test]
fn new_takes_back_the_tree_when_the_work_will_not_apply_in_it() {
    // The work in hand was written against the second commit, and the tree is
    // cut from the first: the stash will not apply, and what is left must be
    // what there was before the command -- the work where it was typed, and no
    // tree standing under an id nothing records.
    let amx = Harness::new();
    let mock = amx.mock();
    let repo = amx.a_repo();
    std::fs::write(repo.join("README.md"), "second\n").unwrap();
    git(&repo, &["commit", "-am", "second"]);
    std::fs::write(repo.join("README.md"), "third\n").expect("the work in hand");

    let refused = new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--with-changes",
            "--base",
            "HEAD~1",
            "--agent",
            &mock,
            "fix the login bug",
        ],
    );

    assert_eq!(refused.status.code(), Some(1));
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("moving the work"), "{said}");
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "third\n",
        "the work is still where it was typed"
    );
    assert!(
        !repo.join(".amx/worktrees").exists()
            || std::fs::read_dir(repo.join(".amx/worktrees"))
                .unwrap()
                .next()
                .is_none(),
        "and the tree was taken back"
    );
    assert_eq!(
        git(&repo, &["branch", "--list", "amx/*"]),
        "",
        "with its branch"
    );
    let listed = amx.amx(&["ls", "--json"]);
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout).trim(),
        "[]",
        "and no agent started"
    );
}

/// A repository with a bare origin beside it, one commit pushed there on a
/// `feature` branch, and that commit set as request 7's head.
///
/// The branch is taken back out of the checkout afterwards, which is what a
/// request somebody else opened looks like from here: work that is on the
/// forge and in no local branch at all.
fn a_repo_with_a_request(amx: &Harness) -> (PathBuf, String) {
    let repo = amx.a_repo();
    let origin = amx.home().join("origin.git");
    std::fs::create_dir_all(&origin).expect("the forge's own copy");
    git(&origin, &["init", "--bare", "-b", "main"]);

    git(
        &repo,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    );
    git(&repo, &["push", "origin", "main"]);
    git(&repo, &["checkout", "-b", "feature"]);
    std::fs::write(repo.join("README.md"), "the request's work\n").expect("something to review");
    git(&repo, &["commit", "-am", "the request"]);
    let commit = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["push", "origin", "feature"]);
    git(&repo, &["checkout", "main"]);
    git(&repo, &["branch", "-D", "feature"]);
    git(&origin, &["update-ref", "refs/pull/7/head", &commit]);

    (repo, commit)
}

/// A `gh` of the suite's own under the harness's `bin`, answering about
/// request 7 and refusing every other number the way gh refuses one that is
/// not there.
///
/// Never the gh the machine running the suite has installed: it would ask a
/// forge about a repository nobody here has heard of.
fn a_gh(amx: &Harness, commit: &str) {
    use std::os::unix::fs::PermissionsExt;

    let bin = amx.home().join("bin");
    std::fs::create_dir_all(&bin).expect("a directory for it");
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
if [ "$3" = "7" ]; then
  printf '%s' '{{"headRefName":"feature","headRefOid":"{commit}","isCrossRepository":false}}'
  exit 0
fi
echo "no pull requests found" >&2
exit 1
"#
        ),
    )
    .expect("writing the fake gh");
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755))
        .expect("a gh that can be run");
}

/// `amx new`, with the harness's own `bin` in front of PATH, which is where
/// the fake gh is.
fn new_with_gh(amx: &Harness, scenario: &str, args: &[&str]) -> Output {
    let path = format!(
        "{}:{}",
        amx.home().join("bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    amx.amx_command(&[&["new"], args].concat())
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario(scenario))
        .env("PATH", path)
        .output()
        .expect("running amx new")
}

#[test]
fn new_cuts_the_tree_on_the_head_branch_of_the_request_it_was_given() {
    let amx = Harness::new();
    let mock = amx.mock();
    let (repo, commit) = a_repo_with_a_request(&amx);
    a_gh(&amx, &commit);

    let id = id_of(&new_with_gh(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--pr",
            "7",
            "--agent",
            &mock,
            "review the request",
        ],
    ));

    let meta = amx.meta(&id);
    assert_eq!(
        meta["branch"], "feature",
        "the head ref's own name, so the column finds the request by it"
    );
    assert_eq!(meta["base"], commit, "the commit the request is at");
    let worktree = repo.join(".amx/worktrees").join(&id);
    assert_eq!(
        meta["worktree"],
        worktree.to_string_lossy().as_ref(),
        "a tree where every other one goes"
    );
    assert_eq!(
        git(&worktree, &["rev-parse", "HEAD"]),
        commit,
        "with the request's work checked out in it"
    );

    // git holds one tree to a branch, and the first agent has this one.
    let second = id_of(&new_with_gh(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--pr",
            "7",
            "--agent",
            &mock,
            "review it again",
        ],
    ));
    assert_eq!(amx.meta(&second)["branch"], "pr-7");
    assert_eq!(amx.meta(&second)["base"], commit);
}

#[test]
fn new_cuts_the_tree_for_a_request_whatever_the_worktrees_key_says() {
    // The key answers for the spawns nobody said anything about. A request is
    // work that lives on a branch, and there is no starting on it without the
    // tree that branch is checked out in.
    let amx = Harness::new();
    let mock = amx.mock();
    let (repo, commit) = a_repo_with_a_request(&amx);
    a_gh(&amx, &commit);
    amx.config("worktrees = false\n");

    let id = id_of(&new_with_gh(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--pr",
            "7",
            "--agent",
            &mock,
            "review the request",
        ],
    ));

    let meta = amx.meta(&id);
    assert_eq!(meta["branch"], "feature", "{meta}");
    assert_eq!(meta["base"], commit, "{meta}");
    assert!(
        meta["worktree"].is_string(),
        "a tree was cut for it, key or no key: {meta}"
    );
}

#[test]
fn new_refuses_a_request_gh_cannot_answer() {
    let amx = Harness::new();
    let mock = amx.mock();
    let (repo, commit) = a_repo_with_a_request(&amx);
    a_gh(&amx, &commit);

    let refused = new_with_gh(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--pr",
            "9",
            "--agent",
            &mock,
            "review the request",
        ],
    );

    assert_eq!(refused.status.code(), Some(1));
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("#9"), "the number that was typed: {said}");
    let listed = amx.amx(&["ls", "--json"]);
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout).trim(),
        "[]",
        "and no agent was started for it"
    );
    assert_eq!(
        git(&repo, &["worktree", "list", "--porcelain"])
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count(),
        1,
        "nor a tree cut"
    );
}

#[test]
fn new_refuses_a_request_beside_the_flags_that_say_where_a_tree_comes_from() {
    let amx = Harness::new();
    let mock = amx.mock();
    let (repo, commit) = a_repo_with_a_request(&amx);
    a_gh(&amx, &commit);

    let refused = new_with_gh(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--pr",
            "7",
            "--base",
            "main",
            "--agent",
            &mock,
            "review the request",
        ],
    );

    assert_eq!(
        refused.status.code(),
        Some(64),
        "the request says what the tree is cut from, and so does --base"
    );
}

#[test]
fn new_cuts_the_tree_on_a_branch_this_checkout_already_has() {
    // No origin in this repository at all, which is the whole of the point: a
    // branch that is here is a branch there is nothing to fetch, and the
    // commits somebody made on it locally stay where they are.
    let amx = Harness::new();
    let mock = amx.mock();
    let repo = amx.a_repo();
    let spike = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["branch", "spike"]);
    std::fs::write(repo.join("README.md"), "main went on\n").expect("a second version");
    git(&repo, &["commit", "-am", "second"]);

    let id = id_of(&new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--branch",
            "spike",
            "--agent",
            &mock,
            "carry on with it",
        ],
    ));

    let meta = amx.meta(&id);
    assert_eq!(
        meta["branch"], "spike",
        "the branch the commits are to land on"
    );
    assert_eq!(
        meta["base"], spike,
        "the branch's own commit, not whatever main is standing on"
    );
    let worktree = repo.join(".amx/worktrees").join(&id);
    assert_eq!(meta["worktree"], worktree.to_string_lossy().as_ref());
    assert_eq!(
        git(&worktree, &["rev-parse", "HEAD"]),
        spike,
        "with the branch's work checked out in it"
    );
    assert_eq!(
        git(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "spike",
        "and standing on the branch rather than beside it"
    );
}

#[test]
fn new_cuts_the_tree_on_a_branch_only_the_origin_has() {
    // The same shape a request arrives in: `feature` is on the origin and in
    // no local branch here, so the branch has to be fetched before there is
    // anything to check out. `origin/feature` is how somebody reads that name
    // out, and the tree goes on `feature` either way.
    let amx = Harness::new();
    let mock = amx.mock();
    let (repo, commit) = a_repo_with_a_request(&amx);

    let id = id_of(&new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--branch",
            "origin/feature",
            "--agent",
            &mock,
            "carry on with it",
        ],
    ));

    let meta = amx.meta(&id);
    assert_eq!(meta["branch"], "feature", "the prefix comes off: {meta}");
    assert_eq!(meta["base"], commit, "the commit the origin has it at");
    let worktree = repo.join(".amx/worktrees").join(&id);
    assert_eq!(
        git(&worktree, &["rev-parse", "HEAD"]),
        commit,
        "with the branch's work checked out in it"
    );
}

#[test]
fn new_refuses_a_branch_another_tree_already_holds() {
    // git keeps one tree to a branch, and the checkout itself is a tree. A
    // request can be started twice under a name of amx's own; a branch
    // somebody typed has no second name.
    let amx = Harness::new();
    let mock = amx.mock();
    let repo = amx.a_repo();

    let refused = new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--branch",
            "main",
            "--agent",
            &mock,
            "carry on with it",
        ],
    );

    assert_eq!(refused.status.code(), Some(1));
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("main is checked out"), "{said}");
    let listed = amx.amx(&["ls", "--json"]);
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout).trim(),
        "[]",
        "and no agent was started for it"
    );
    assert_eq!(
        git(&repo, &["worktree", "list", "--porcelain"])
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count(),
        1,
        "nor a tree cut"
    );
}

#[test]
fn new_runs_in_the_directory_as_it_is_when_asked() {
    let amx = Harness::new();
    let mock = amx.mock();
    let repo = amx.a_repo();

    let id = id_of(&new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--no-worktree",
            "--agent",
            &mock,
            "fix the login bug",
        ],
    ));

    let meta = amx.meta(&id);
    assert!(meta["worktree"].is_null());
    assert_eq!(meta["dir"], repo.to_string_lossy().as_ref());
    assert!(!repo.join(".amx").exists(), "and nothing was cut");
}

#[test]
fn new_refuses_once_the_cap_is_reached() {
    let amx = Harness::new();
    let mock = amx.mock();
    amx.config("max_agents = 1\n");

    let first = id_of(&new(
        &amx,
        "works-without-end",
        &["--no-worktree", "--agent", &mock, "the first"],
    ));
    amx.until_state(&first, "working");

    let refused = new(
        &amx,
        "happy-turn",
        &["--no-worktree", "--agent", &mock, "the second"],
    );
    assert_eq!(refused.status.code(), Some(2), "blocked, not failed");
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("max_agents") || said.contains('1'), "{said}");
}

/// A directory with a config file of its own, which outside a repository is
/// the whole of a project.
fn a_project(amx: &Harness, name: &str, config: &str) -> std::path::PathBuf {
    let dir = amx.home().join(name);
    std::fs::create_dir_all(dir.join(".amx")).expect("the project's own directory");
    std::fs::write(dir.join(".amx/config.toml"), config).expect("the project's config");
    dir
}

#[test]
fn new_counts_the_cap_however_the_directory_was_spelled() {
    // `--dir` as somebody types it at a prompt: relative to where they are
    // standing. The record holds where they meant, spelled out from the root,
    // and the cap counts it against the same project the absolute spelling
    // names — outside a repository, where the directory is the whole of the
    // project and nothing above it would have answered in absolute terms.
    let amx = Harness::new();
    let mock = amx.mock();
    let alpha = a_project(&amx, "alpha", "max_agents = 1\n");

    let first = id_of(
        &amx.amx_command(&["new", "--dir", "alpha", "--agent", &mock, "the first"])
            .env("MOCK_CLAUDE_SCENARIO", amx.scenario("works-without-end"))
            .current_dir(amx.home())
            .output()
            .expect("running amx new"),
    );
    amx.until_state(&first, "working");
    assert_eq!(
        amx.meta(&first)["dir"],
        std::fs::canonicalize(&alpha)
            .unwrap()
            .to_string_lossy()
            .as_ref(),
        "the record says where the agent runs from the root"
    );

    let refused = new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &alpha.to_string_lossy(),
            "--agent",
            &mock,
            "the second",
        ],
    );
    assert_eq!(refused.status.code(), Some(2), "blocked, not failed");
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("max_agents is 1"), "{said}");
}

#[test]
fn new_counts_the_cap_against_the_project_the_agent_will_run_in() {
    // Two projects, each of them allowed one agent at a time. What one is
    // running is nothing the other answers for, and the refusal names the
    // project it counted so that a person knows which file said so.
    let amx = Harness::new();
    let mock = amx.mock();
    let alpha = a_project(&amx, "alpha", "max_agents = 1\n");
    let beta = a_project(&amx, "beta", "max_agents = 1\n");

    let first = id_of(&new(
        &amx,
        "works-without-end",
        &[
            "--dir",
            &alpha.to_string_lossy(),
            "--agent",
            &mock,
            "the first",
        ],
    ));
    amx.until_state(&first, "working");

    let refused = new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &alpha.to_string_lossy(),
            "--agent",
            &mock,
            "the second",
        ],
    );
    assert_eq!(refused.status.code(), Some(2), "blocked, not failed");
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("max_agents is 1"), "{said}");
    assert!(
        said.contains(&alpha.display().to_string()),
        "the project it counted: {said}"
    );

    let elsewhere = new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &beta.to_string_lossy(),
            "--agent",
            &mock,
            "in the other project",
        ],
    );
    assert!(
        elsewhere.status.success(),
        "the next project has a cap of its own: {}",
        String::from_utf8_lossy(&elsewhere.stderr)
    );
}

#[test]
fn new_refuses_at_the_ceiling_over_every_project() {
    // The ceiling is the machine's rather than any project's: two projects
    // with room to spare between them still stop at what the person allowed
    // in total.
    let amx = Harness::new();
    let mock = amx.mock();
    amx.config("max_total = 1\n");
    let alpha = a_project(&amx, "alpha", "max_agents = 5\n");
    let beta = a_project(&amx, "beta", "max_agents = 5\n");

    let first = id_of(&new(
        &amx,
        "works-without-end",
        &[
            "--dir",
            &alpha.to_string_lossy(),
            "--agent",
            &mock,
            "the first",
        ],
    ));
    amx.until_state(&first, "working");

    let refused = new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &beta.to_string_lossy(),
            "--agent",
            &mock,
            "the second",
        ],
    );
    assert_eq!(refused.status.code(), Some(2), "blocked, not failed");
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("max_total is 1"), "{said}");
}

#[test]
fn an_agent_that_has_ended_does_not_hold_a_place() {
    let amx = Harness::new();
    let mock = amx.mock();
    amx.config("max_agents = 1\n");

    let first = id_of(&new(
        &amx,
        "finishes",
        &["--no-worktree", "--agent", &mock, "the first"],
    ));
    amx.until_state(&first, "done");

    let second = new(
        &amx,
        "happy-turn",
        &["--no-worktree", "--agent", &mock, "the second"],
    );
    assert!(
        second.status.success(),
        "an agent that is over is not one of the five: {}",
        String::from_utf8_lossy(&second.stderr)
    );
}

#[test]
fn the_cap_counts_the_vendor_agents_that_are_running() {
    // friction #JX6B7GWF: a project at max_agents 1 refused a spawn while the
    // only records against it were a shell command and an agent sitting at its
    // prompt. Neither is running a turn -- a command has no vendor at all, and
    // an idle agent is a pane waiting to be spoken to -- so neither fills the
    // place the cap is counting.
    let amx = Harness::new();
    let mock = amx.mock();
    let alpha = a_project(&amx, "alpha", "max_agents = 1\n");
    let dir = alpha.to_string_lossy().into_owned();

    let command = amx
        .amx_command(&[
            "new",
            "--dir",
            &dir,
            "--name",
            "watch-log-a1b",
            "--exec",
            "sleep 600",
        ])
        .output()
        .expect("running amx new --exec");
    assert!(
        command.status.success(),
        "amx new --exec: {}",
        String::from_utf8_lossy(&command.stderr)
    );

    let sitting = id_of(&new(
        &amx,
        "happy-turn",
        &["--dir", &dir, "--agent", &mock, "the first"],
    ));
    amx.until_state(&sitting, "idle");

    let started = new(
        &amx,
        "works-without-end",
        &["--dir", &dir, "--agent", &mock, "the second"],
    );
    assert!(
        started.status.success(),
        "a command and an idle agent leave the cap its place: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    let working = id_of(&started);
    amx.until_state(&working, "working");

    let refused = new(
        &amx,
        "happy-turn",
        &["--dir", &dir, "--agent", &mock, "the third"],
    );
    assert_eq!(refused.status.code(), Some(2), "blocked, not failed");
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(
        said.contains("max_agents is 1"),
        "and an agent working does fill it: {said}"
    );
}

#[test]
fn new_takes_the_name_it_is_given_and_refuses_one_twice() {
    let amx = Harness::new();
    let mock = amx.mock();

    let id = id_of(&new(
        &amx,
        "happy-turn",
        &[
            "--no-worktree",
            "--name",
            "importer",
            "--agent",
            &mock,
            "port it",
        ],
    ));
    assert_eq!(id, "importer");

    let again = new(
        &amx,
        "happy-turn",
        &[
            "--no-worktree",
            "--name",
            "importer",
            "--agent",
            &mock,
            "port it again",
        ],
    );
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("importer"));

    let wrong = new(
        &amx,
        "happy-turn",
        &[
            "--no-worktree",
            "--name",
            "Not An Id",
            "--agent",
            &mock,
            "port it",
        ],
    );
    assert!(!wrong.status.success());
}

#[test]
fn vendor_arguments_reach_the_vendor_untouched() {
    let amx = Harness::new();
    let mock = amx.mock();
    let id = id_of(&new(
        &amx,
        "happy-turn",
        &[
            "--no-worktree",
            "--agent",
            &mock,
            "fix the login bug",
            "--",
            "--session-id",
            "abc-123",
        ],
    ));

    let handoff = amx.handoff(&id);
    let command: Vec<&str> = handoff["command"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect();
    assert_eq!(command[0], mock);
    assert!(
        command
            .windows(2)
            .any(|pair| pair == ["--session-id", "abc-123"]),
        "{command:?}"
    );
    assert_eq!(
        command.last(),
        Some(&"fix the login bug"),
        "the task is the last word, the way a prompt is"
    );
}

#[test]
fn dials_the_flags_amx_was_given_reach_the_vendor() {
    let amx = Harness::new();
    let id = id_of(&new_as_claude(
        &amx,
        "a-dispatched-worker",
        &[
            "--no-worktree",
            "--agent",
            "claude",
            "--model",
            "opus",
            "--effort",
            "high",
            "fix the login bug",
        ],
    ));

    let command = command_of(&amx, &id);
    assert_eq!(command[0], "claude");
    assert!(
        command.windows(2).any(|pair| pair == ["--model", "opus"])
            && command.windows(2).any(|pair| pair == ["--effort", "high"]),
        "{command:?}"
    );
    assert!(
        !command.iter().any(|arg| arg == "--permission-mode"),
        "the dial nobody turned sends no flag at all: {command:?}"
    );
    assert_eq!(
        command.last().map(String::as_str),
        Some("fix the login bug"),
        "and the task is still the last word: {command:?}"
    );

    // Not only on the record: the process in the pane was called that way.
    let argv = argv_of(&amx, &id);
    assert!(argv.contains("--model opus"), "{argv}");
    assert!(argv.contains("--effort high"), "{argv}");
}

#[test]
fn dials_stand_down_from_a_flag_the_caller_wrote_out_by_hand() {
    // Both spellings of the same thing are on this command line: amx's dial
    // and claude's own flag. The vendor is handed one of them, the one that
    // was written out, and the dial nobody wrote is still injected.
    let amx = Harness::new();
    let id = id_of(&new_as_claude(
        &amx,
        "a-dispatched-worker",
        &[
            "--no-worktree",
            "--agent",
            "claude",
            "--model",
            "opus",
            "--effort",
            "high",
            "fix the login bug",
            "--",
            "--model",
            "sonnet",
        ],
    ));

    let command = command_of(&amx, &id);
    assert_eq!(
        command.iter().filter(|arg| *arg == "--model").count(),
        1,
        "one --model, never two with the winner left to the vendor: {command:?}"
    );
    assert!(
        command.windows(2).any(|pair| pair == ["--model", "sonnet"]),
        "and it is the caller's own: {command:?}"
    );
    assert!(
        command.windows(2).any(|pair| pair == ["--effort", "high"]),
        "{command:?}"
    );

    let argv = argv_of(&amx, &id);
    assert!(
        argv.contains("--model sonnet") && !argv.contains("opus"),
        "{argv}"
    );
}

#[test]
fn dials_are_turned_by_the_config_for_every_spawn_that_says_nothing() {
    let amx = Harness::new();
    amx.config("agent = \"claude\"\nmodel = \"fable\"\npermission = \"plan\"\n");

    let id = id_of(&new_as_claude(
        &amx,
        "a-dispatched-worker",
        &["--no-worktree", "fix the login bug"],
    ));

    let command = command_of(&amx, &id);
    assert_eq!(command[0], "claude");
    assert!(
        command.windows(2).any(|pair| pair == ["--model", "fable"])
            && command
                .windows(2)
                .any(|pair| pair == ["--permission-mode", "plan"]),
        "{command:?}"
    );
}

#[test]
fn dials_the_record_keeps_the_model_and_effort_that_were_turned() {
    // Which vendor ran is on the record already; which model and how hard it
    // was told to think lived in the pane's argv alone, where nothing reading
    // a wall could get at them.
    let amx = Harness::new();
    let id = id_of(&new_as_claude(
        &amx,
        "a-dispatched-worker",
        &[
            "--no-worktree",
            "--agent",
            "claude",
            "--model",
            "opus",
            "--effort",
            "high",
            "fix the login bug",
        ],
    ));

    let meta = amx.meta(&id);
    assert_eq!(meta["model"], "opus", "{meta}");
    assert_eq!(meta["effort"], "high", "{meta}");

    let row = listed(&amx, &id).expect("a row for the agent just started");
    assert_eq!(row["agent"], "claude", "{row}");
    assert_eq!(row["model"], "opus", "{row}");
    assert_eq!(row["effort"], "high", "{row}");
}

#[test]
fn dials_a_spawn_that_turned_neither_records_neither() {
    // A dial nobody turned sends no flag, and the record says the same thing
    // the argv does: nothing. A reader that wants the word the vendor would
    // have chosen has to ask the vendor.
    let amx = Harness::new();
    let id = id_of(&new_as_claude(
        &amx,
        "a-dispatched-worker",
        &["--no-worktree", "--agent", "claude", "fix the login bug"],
    ));

    let row = listed(&amx, &id).expect("a row for the agent just started");
    assert_eq!(row["agent"], "claude", "{row}");
    assert!(printed(&row, "model").is_null(), "{row}");
    assert!(printed(&row, "effort").is_null(), "{row}");
}

#[test]
fn dials_a_command_spawn_records_no_vendor_and_no_dials() {
    // The dials are refused beside --exec, so nothing about a launch is
    // resolved for a shell row. All three read null, the way `agent` does.
    let amx = Harness::new();
    let ran = id_of(
        &amx.amx_command(&["new", "--exec", "true"])
            .output()
            .expect("running amx new --exec"),
    );

    let row = listed(&amx, &ran).expect("a row for the command just run");
    assert!(printed(&row, "agent").is_null(), "{row}");
    assert!(printed(&row, "model").is_null(), "{row}");
    assert!(printed(&row, "effort").is_null(), "{row}");
}

#[test]
fn dials_a_fork_carries_the_ones_its_origin_was_started_with() {
    // A copy runs the conversation it was made from, launched the same way,
    // so it is running the same model at the same effort. Nothing on the
    // command line of a fork can say otherwise.
    let amx = Harness::new();
    let id = id_of(&new_as_claude(
        &amx,
        "a-dispatched-worker",
        &[
            "--no-worktree",
            "--agent",
            "claude",
            "--model",
            "opus",
            "--effort",
            "high",
            "fix the login bug",
        ],
    ));
    amx.until("the hook to report a session", || {
        amx.meta(&id)["session"].as_str().map(str::to_string)
    });

    let forked = amx
        .amx_command(&["fork", &id])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("continues-a-session"))
        .env("PATH", path_with_the_stand_in(&amx))
        .output()
        .expect("running amx fork");
    assert!(
        forked.status.success(),
        "amx fork: {}",
        String::from_utf8_lossy(&forked.stderr)
    );
    let copy = String::from_utf8_lossy(&forked.stdout).trim().to_string();

    let meta = amx.meta(&copy);
    assert_eq!(meta["model"], "opus", "{meta}");
    assert_eq!(meta["effort"], "high", "{meta}");
}

#[test]
fn dials_a_value_the_vendor_would_refuse_is_refused_before_anything_is_made() {
    let amx = Harness::new();
    let refused = new(
        &amx,
        "happy-turn",
        &[
            "--no-worktree",
            "--agent",
            "claude",
            "--effort",
            "hard",
            "fix the login bug",
        ],
    );

    assert_eq!(
        refused.status.code(),
        Some(64),
        "a malformed command line, not a state a caller branches on"
    );
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(
        said.contains("--effort") && said.contains("xhigh"),
        "{said}"
    );
    assert!(
        !amx.state_root().exists() || amx.state_root().read_dir().unwrap().next().is_none(),
        "and no id was minted for it"
    );
}

#[test]
fn a_directory_that_is_not_there_is_said_so_before_anything_is_made() {
    let amx = Harness::new();
    let mock = amx.mock();
    let refused = new(
        &amx,
        "happy-turn",
        &["--dir", "/nowhere/at/all", "--agent", &mock, "fix it"],
    );

    assert!(!refused.status.success());
    assert!(
        !amx.state_root().exists() || amx.state_root().read_dir().unwrap().next().is_none(),
        "a dispatch that failed leaves no half-made agent behind"
    );
}

#[test]
fn new_two_racers_for_one_name_leave_the_winners_record_standing() {
    // Two `amx new --name <same>` in flight at once. The name has one owner:
    // whichever spawn claims the directory keeps it, and the id it printed is
    // still a record afterwards — the loser tidying up after itself must
    // never take the winner's meta.json with it.
    let amx = Harness::new();
    let mock = amx.mock();
    // Ten attempts of racers, and the finished ones still hold panes: the
    // default cap would start refusing spawns halfway through the race.
    amx.config("max_agents = 40\n");
    let state_dir = amx
        .state_root()
        .parent()
        .expect("the state root has a parent")
        .to_path_buf();

    for attempt in 0..10 {
        let name = format!("race-{attempt}");
        // Both racers are up and spinning before the starting gun fires, so
        // they reach the uniqueness check together instead of one whole run
        // apart.
        let go = state_dir.join(format!("go-{attempt}"));
        let racers: Vec<_> = (0..2)
            .map(|_| {
                std::process::Command::new("sh")
                    .arg("-c")
                    .arg(format!(
                        "until [ -e '{go}' ]; do :; done; \
                         exec '{AMX}' new --no-worktree --agent '{mock}' \
                         --name '{name}' 'fix the login bug'",
                        go = go.display(),
                    ))
                    .env("AMX_STATE_DIR", &state_dir)
                    .env("HOME", amx.home())
                    .env("XDG_CONFIG_HOME", amx.home().join(".config"))
                    .env("AMX_TMUX_SOCKET", amx.socket())
                    .env("MOCK_CLAUDE_SCENARIO", amx.scenario("finishes"))
                    .env_remove("TMUX")
                    .env_remove("TMUX_PANE")
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .expect("starting a racer")
            })
            .collect();
        std::fs::write(&go, b"").expect("the starting gun");
        let done: Vec<Output> = racers
            .into_iter()
            .map(|racer| racer.wait_with_output().expect("waiting for a racer"))
            .collect();

        let winners: Vec<&Output> = done.iter().filter(|out| out.status.success()).collect();
        assert_eq!(
            winners.len(),
            1,
            "attempt {attempt}: one name, one owner: {}",
            done.iter()
                .map(|out| format!(
                    "[exit {:?} out {:?} err {:?}]",
                    out.status.code(),
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                ))
                .collect::<Vec<_>>()
                .join(" ")
        );
        for winner in winners {
            let id = String::from_utf8_lossy(&winner.stdout).trim().to_string();
            assert_eq!(id, name, "the winner prints the name it was given");
            assert!(
                amx.agent_dir(&id).join("meta.json").exists(),
                "attempt {attempt}: {id} was printed with exit 0 but its record is gone"
            );
        }
    }
}

#[test]
fn attach_says_so_when_there_is_no_such_agent() {
    let amx = Harness::new();
    let out = amx.amx(&["attach", "never-made-abc"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("never-made-abc"));
}

fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .expect("the file")
        .permissions()
        .mode()
}

/// A role file written under this harness's own config, where the person's
/// roles stand.
fn a_role(amx: &Harness, name: &str, text: &str) {
    let dir = amx.home().join(".config/amx/agents");
    std::fs::create_dir_all(&dir).expect("a directory for roles");
    std::fs::write(dir.join(format!("{name}.md")), text).expect("writing the role");
}

#[test]
fn new_spawns_on_a_roles_dials_and_hands_the_brief_before_the_task() {
    // A role is a named recipe: its frontmatter is a default for the dials and
    // its body is a brief. The vendor is handed the brief and then the task;
    // the record keeps the task alone, because what somebody asked for is the
    // task and the role is how they asked.
    let amx = Harness::new();
    a_role(
        &amx,
        "scout",
        "---\ndescription: fast recon\nagent: claude\nmodel: fable\neffort: low\n---\nYou are a scout.\n",
    );

    let out = new_as_claude(
        &amx,
        "happy-turn",
        &[
            "--no-worktree",
            "--role",
            "scout",
            "find the auth middleware",
        ],
    );
    let id = id_of(&out);

    let meta = amx.meta(&id);
    assert_eq!(
        meta["task"], "find the auth middleware",
        "the record keeps the task"
    );
    assert_eq!(meta["agent"], "claude", "the role's agent");
    assert_eq!(meta["model"], "fable", "and its model");
    assert_eq!(meta["effort"], "low", "and its effort");
    assert_eq!(meta["role"], "scout", "and the role is named on the record");

    // And the reading says so, both ways.
    let json: Value =
        serde_json::from_slice(&amx.amx(&["status", &id, "--json"]).stdout).expect("one object");
    assert_eq!(json["role"], "scout");
    let printed = String::from_utf8_lossy(&amx.amx(&["status", &id]).stdout).into_owned();
    assert!(
        printed
            .lines()
            .any(|line| line.contains("role") && line.contains("scout")),
        "the reading names the role: {printed}"
    );

    let command = command_of(&amx, &id);
    assert_eq!(
        command.last().map(String::as_str),
        Some("You are a scout.\n\nfind the auth middleware"),
        "the brief rides in front of the task: {command:?}"
    );
}

#[test]
fn a_typed_dial_beats_the_roles_and_an_unknown_role_names_the_ones_it_knows() {
    let amx = Harness::new();
    a_role(
        &amx,
        "scout",
        "---\ndescription: fast recon\nagent: claude\nmodel: fable\n---\nYou are a scout.\n",
    );

    // The role is a default: what the caller typed stands.
    let out = new_as_claude(
        &amx,
        "happy-turn",
        &[
            "--no-worktree",
            "--role",
            "scout",
            "--model",
            "opus",
            "find the auth middleware",
        ],
    );
    assert_eq!(amx.meta(&id_of(&out))["model"], "opus");

    // A name amx does not know is a command line to fix, and the roles it does
    // know are named so the next try is the right one.
    let out = new_as_claude(
        &amx,
        "happy-turn",
        &[
            "--no-worktree",
            "--role",
            "nobody",
            "find the auth middleware",
        ],
    );
    let said = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(64), "{said}");
    assert!(said.contains("no role `nobody`"), "{said}");
    assert!(said.contains("scout"), "the ones it knows: {said}");
}

#[test]
fn a_role_is_refused_beside_a_shell_command() {
    // A role is a vendor recipe: model, effort, a brief for an agent. A shell
    // command has none of those, so the pair is a command line nobody meant.
    let amx = Harness::new();

    let out = amx.amx(&["new", "--exec", "--role", "scout", "echo hi"]);

    assert_eq!(out.status.code(), Some(64));
}
