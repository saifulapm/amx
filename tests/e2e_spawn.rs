//! Starting an agent, and what that leaves behind.

mod common;

use common::{AMX, Harness};
use std::path::Path;
use std::process::Output;

/// `amx new`, with the vendor pointed at a scenario.
fn new(amx: &Harness, scenario: &str, args: &[&str]) -> Output {
    amx.amx_command(&[&["new"], args].concat())
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario(scenario))
        .output()
        .expect("running amx new")
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

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    amx.amx_command(&[&["new"], args].concat())
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario(scenario))
        .env("PATH", path)
        .output()
        .expect("running amx new")
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
        "happy-turn",
        &["--no-worktree", "--agent", &mock, "the first"],
    ));
    amx.until_state(&first, "idle");

    let refused = new(
        &amx,
        "happy-turn",
        &["--no-worktree", "--agent", &mock, "the second"],
    );
    assert_eq!(refused.status.code(), Some(2), "blocked, not failed");
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("max_agents") || said.contains('1'), "{said}");
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
