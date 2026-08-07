#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
//! V07 acceptance: spawn identity and the process-tree walk.
//!
//! Two halves of one chain, and they meet in a pane:
//!
//! - **Spawn side.** What a pane's child is told about amx — the recorded argv,
//!   the six `AMX_*` variables, and the per-spawn hook token. V01 §3 M6
//!   measured that this environment reaches hook processes through an
//!   interactive shell (all 185 invocations in the spike's run carried all six
//!   verbatim), which is the entire basis of D-M2-4's identity scheme, so the
//!   acceptance here is that the child really receives them rather than that
//!   the server really sent them.
//! - **Probe side.** What amx can tell about a pane's child by looking: the
//!   foreground job's argv, wrapper unwrapping, and the negative case that
//!   keeps unwrapping honest — an agent's name inside an eval flag's operand is
//!   text, not a program.
//!
//! The judgement half runs over a written-down process tree, because "which of
//! this job's processes answers" is not something a real `/proc` can be asked
//! to demonstrate on command. The reader half runs against a real pty, because
//! that is the only way to find out whether `/proc/<pid>/cmdline` and
//! `KERN_PROCARGS2` say what this code thinks they say.

use amx_core::agent::{Activity, AgentState};
use amx_core::platform::{ProcessId, ProcessTree, Pty, PtyCommand, PtySession, WinSize, pane_env};
use amx_server::agent::identity::{
    Evidence, PROBE_SPACING, ProbeGate, ProbeTrigger, identify, program_named,
};
use amx_server::agent::registry::Registry;
use amx_server::platform::{UnixProcessTree, UnixPty};

// A test crate root's module directory is `tests/`, so the harness names its
// path to live beside its one suite rather than next to everybody's.
#[path = "identity/tree.rs"]
mod tree;

use tree::{FakeTree, PATIENCE, Panes, TICK, TempDir, read_when_written};

/// A command that stays alive without painting anything.
fn quiet() -> Vec<String> {
    vec!["/bin/sh".into(), "-c".into(), "exec cat".into()]
}

/// `argv` as the owned vector the walk takes.
fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

// ---------------------------------------------------------------- spawn side

/// The field `persist/mod.rs` reserved, filled: a pane remembers what it was
/// started as, so restore can reason about it and identity has a starting
/// guess (`SpawnedIdentity`, `docs/08-m2-plan.md` §3).
///
/// The *effective* argv is what is recorded, which is why the second pane
/// matters: a split that asked for no command still ran something, and a pane
/// that remembered `None` would be a pane nothing could reason about.
#[tokio::test]
async fn spawned_pane_records_its_argv_in_state() {
    let dir = TempDir::new("argv");
    let panes = Panes::start(dir.path());
    let root = panes.root_pane().await;
    let split = panes.split(root, quiet()).await;

    let core = panes.finish().await;
    let recorded = core
        .state()
        .pane(split)
        .expect("the pane is in state")
        .argv()
        .expect("a spawned pane records its argv");
    assert_eq!(recorded, quiet(), "the argv the split asked for, as data");

    // The workspace's root pane asked for nothing and still ran a shell.
    let shell = core
        .state()
        .pane(root)
        .expect("the root pane is in state")
        .argv()
        .expect("a pane spawned without a command records its shell");
    assert_eq!(
        shell.len(),
        1,
        "a bare shell is one argv element: {shell:?}"
    );
    assert!(
        !shell[0].is_empty(),
        "the recorded shell must name something"
    );
}

/// D-M2-4's identity injection, proved from the far side of the pty.
///
/// The child dumps its own environment and the test reads it back, because the
/// claim is about *inheritance* — the server writing six variables onto a
/// `PtyCommand` proves nothing that a child seeing them does not prove better.
///
/// The token assertion is the one that guards misattribution: the value the
/// child carries has to be the value the pane recorded, or a hook report would
/// arrive with a token `AgentHub` could not match to anything.
#[tokio::test]
async fn pane_child_env_carries_amx_ids_socket_and_token() {
    let dir = TempDir::new("env");
    let dumped = dir.path().join("env");
    let panes = Panes::start(dir.path());
    let root = panes.root_pane().await;
    // Written to a temporary name and renamed into place, so the file
    // appearing is the file being complete.
    let script = format!(
        "printenv > {tmp}; mv {tmp} {out}; exec cat",
        tmp = dumped.with_extension("part").display(),
        out = dumped.display(),
    );
    let pane = panes
        .split(root, vec!["/bin/sh".into(), "-c".into(), script])
        .await;

    let text = read_when_written(&dumped).await;
    let seen: std::collections::HashMap<&str, &str> = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();

    let socket = panes.ctx().socket.display().to_string();
    let core = panes.finish().await;
    let state = core.state();
    let workspace = state
        .workspaces()
        .find(|ws| ws.layout().contains(pane))
        .expect("the pane is in a workspace")
        .id();
    let token = state
        .pane(pane)
        .expect("the pane is in state")
        .hook_token()
        .expect("a spawned pane records its hook token")
        .as_str()
        .to_owned();

    for name in pane_env::ALL {
        assert!(
            seen.contains_key(name),
            "the child never saw {name}; it saw {:?}",
            seen.keys()
                .filter(|k| k.starts_with("AMX"))
                .collect::<Vec<_>>(),
        );
    }
    assert_eq!(seen[pane_env::MARKER], pane_env::MARKER_VALUE);
    assert_eq!(seen[pane_env::SESSION], "identity");
    assert_eq!(seen[pane_env::SOCKET], socket);
    assert_eq!(seen[pane_env::PANE], pane.to_string());
    assert_eq!(seen[pane_env::WORKSPACE], workspace.to_string());
    assert_eq!(
        seen[pane_env::TOKEN],
        token,
        "the token the child carries must be the token the pane recorded"
    );
    assert!(
        !token.is_empty() && token.len() >= 16,
        "a hook token has to be worth comparing: {token:?}"
    );
}

// ---------------------------------------------------------------- probe side

/// The real reader, against a real terminal: `/proc/<pid>/cmdline` on Linux,
/// `sysctl(KERN_PROCARGS2)` on darwin, behind one seam.
///
/// The shell `exec`s, so the foreground group leader keeps its pid and changes
/// its argv. That is deliberate — it is what makes this a test of the *live
/// foreground job* rather than of the command amx happened to spawn, which is
/// the whole distinction tier 3 exists for (a `claude` typed by hand into a
/// pane's shell is attributed exactly like one `agent start` launched).
#[tokio::test]
async fn process_tree_argv_reads_the_foreground_job_on_linux_and_darwin() {
    let mut session = UnixPty
        .spawn(&PtyCommand {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "exec cat".into()],
            env: Vec::new(),
            cwd: Some("/".into()),
            size: WinSize { rows: 24, cols: 80 },
        })
        .expect("a pty and a child on it");

    let tree = UnixProcessTree;
    let deadline = tokio::time::Instant::now() + PATIENCE;
    let (leader, argv) = loop {
        // The child sets its controlling terminal in `pre_exec` and then
        // `exec`s, so both facts arrive after the parent returns from spawn.
        if let Ok(leader) = session.foreground_group()
            && let Ok(argv) = tree.argv(leader)
            && argv.first().is_some_and(|word| word == "cat")
        {
            break (leader, argv);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the foreground job never became the exec'd program"
        );
        tokio::time::sleep(TICK).await;
    };

    assert_eq!(argv, vec!["cat"], "the live argv, as the kernel holds it");
    assert_eq!(
        leader,
        ProcessId(session.child().0),
        "the pty child leads its own group"
    );
    let exe = tree.exe(leader).expect("the mapped executable");
    assert_eq!(
        exe.file_name().and_then(|name| name.to_str()),
        Some("cat"),
        "exe corroborates argv[0]: {}",
        exe.display()
    );

    // And a process that is gone answers `NotFound`, never a stale argv.
    session.kill().expect("hang the child up");
    let gone = loop {
        if !tree.is_alive(leader) {
            break tree.argv(leader);
        }
        let _ = session.try_wait();
        assert!(
            tokio::time::Instant::now() < deadline,
            "the child never exited"
        );
        tokio::time::sleep(TICK).await;
    };
    assert!(gone.is_err(), "a dead process has no argv: {gone:?}");
}

/// An agent shipped as a script has its interpreter's name in `comm`, so the
/// walk looks past interpreters and launchers — and past path tokens, so
/// `/usr/local/bin/claude` and `claude` are one answer.
#[test]
fn wrapper_unwrapping_finds_the_agent_under_node_python_and_sh_dash_c_path() {
    let cases: &[(&[&str], &str)] = &[
        (&["claude"], "claude"),
        (&["/usr/local/bin/claude"], "claude"),
        (&["node", "/usr/local/lib/claude"], "claude"),
        (&["node", "--enable-source-maps", "/opt/claude"], "claude"),
        (&["python3", "/opt/tools/codex"], "codex"),
        (&["python3.13", "/opt/tools/codex"], "codex"),
        // `sh -c <path>` with nothing else in the operand: a program named by
        // path, which is the one shape an eval flag is allowed to name.
        (&["sh", "-c", "/usr/local/bin/claude"], "claude"),
        // `env` assignments come before the program it runs.
        (
            &["env", "NODE_ENV=production", "node", "/opt/claude"],
            "claude",
        ),
        // Two wrappers deep, and `--` ends the launcher's own options.
        (&["env", "--", "bun", "/opt/codex"], "codex"),
    ];
    for (words, want) in cases {
        assert_eq!(
            program_named(&argv(words)),
            Some(*want),
            "{words:?} names {want}"
        );
    }

    // A wrapper with nothing to run names nothing — an interactive shell is a
    // shell, not whatever it might later start.
    assert_eq!(program_named(&argv(&["/bin/zsh"])), None);
    assert_eq!(program_named(&argv(&["node", "--version"])), None);
    assert_eq!(program_named(&[]), None);
}

/// herdr's negative test, re-derived: `python -c "codex"` is not codex.
///
/// An eval flag's operand is source text. Text that *mentions* an agent is the
/// exact shape a wrapper's arguments have, so a walk that read operands as
/// programs would identify every script that imports one — and a misidentified
/// pane gets a whole agent's worth of wrong status, where an unidentified one
/// gets `busy`/`quiet`.
#[test]
fn eval_flag_arguments_never_identify_an_agent() {
    let registry = Registry::builtin();
    let eval: &[&[&str]] = &[
        &["python", "-c", "codex"],
        &["python3", "-c", "import codex; codex.main()"],
        &["node", "-e", "require('claude')"],
        &["sh", "-c", "claude --resume abc123"],
        &["bash", "-c", "cd /tmp && claude"],
        &["sh", "--eval", "claude"],
        // A path, but with shell syntax around it: still a script.
        &["sh", "-c", "exec /usr/local/bin/claude"],
        &["sh", "-c", "/usr/local/bin/claude || true"],
    ];
    for words in eval {
        assert_eq!(
            program_named(&argv(words)),
            None,
            "{words:?} names no program"
        );
    }

    // And end to end: a pane running one of these is not an agent pane.
    let tree = FakeTree::new().with(100, &["python", "-c", "codex"], &[]);
    let seen = identify(&tree, &registry, ProcessId(100));
    assert_eq!(seen.kind(), None, "an eval operand cannot identify");
    assert_eq!(seen.evidence(), Evidence::Unidentified);
}

/// The foreground group leader answers first; only when it names nothing does
/// the walk descend, and the shallowest evidence wins.
#[test]
fn identity_prefers_the_group_leader_then_unwrapped_evidence() {
    let registry = Registry::builtin();

    // The leader is the agent: nothing below it is even read.
    let tree = FakeTree::new()
        .with(100, &["/usr/local/bin/claude"], &[101])
        .with(101, &["codex"], &[]);
    let seen = identify(&tree, &registry, ProcessId(100));
    assert_eq!(
        seen.kind().map(ToString::to_string).as_deref(),
        Some("claude")
    );
    assert_eq!(seen.evidence(), Evidence::GroupLeader);
    assert_eq!(seen.process(), Some(ProcessId(100)));

    // The leader is an interactive shell — the pane's own — and the agent was
    // typed into it. That is the hand-typed case D-M2-4 promises works.
    let tree =
        FakeTree::new()
            .with(200, &["/bin/zsh"], &[201])
            .with(201, &["node", "/opt/claude"], &[]);
    let seen = identify(&tree, &registry, ProcessId(200));
    assert_eq!(
        seen.kind().map(ToString::to_string).as_deref(),
        Some("claude")
    );
    assert_eq!(seen.evidence(), Evidence::JobMember { depth: 1 });
    assert_eq!(seen.process(), Some(ProcessId(201)));
    assert_eq!(seen.argv(), argv(&["node", "/opt/claude"]));

    // A shallower member outranks a deeper one, whatever order the tree is
    // walked in: the agent's own workers are not the agent.
    let tree = FakeTree::new()
        .with(300, &["/bin/zsh"], &[301, 302])
        .with(301, &["vim", "notes.md"], &[303])
        .with(303, &["codex"], &[])
        .with(302, &["claude"], &[]);
    let seen = identify(&tree, &registry, ProcessId(300));
    assert_eq!(
        seen.kind().map(ToString::to_string).as_deref(),
        Some("claude")
    );
    assert_eq!(seen.evidence(), Evidence::JobMember { depth: 1 });

    // A process whose argv is unreadable still has the executable the kernel
    // mapped, which is what the `exe` half of the seam is for.
    let tree =
        FakeTree::new()
            .with(400, &["/bin/zsh"], &[401])
            .with_exe(401, "/usr/local/bin/codex", &[]);
    let seen = identify(&tree, &registry, ProcessId(400));
    assert_eq!(
        seen.kind().map(ToString::to_string).as_deref(),
        Some("codex")
    );
    assert!(seen.argv().is_empty(), "there was no argv to report");

    // A leader that is not in the tree at all — the job exited between the
    // trigger and the probe — is unreadable, not a panic.
    let seen = identify(&FakeTree::new(), &registry, ProcessId(999));
    assert_eq!(seen.kind(), None);
    assert_eq!(seen.process(), None);
}

/// 04 §5: "Process-tree foreground job + prompt detection for unknown
/// programs: `busy/quiet`, **never fake `blocked`**."
///
/// The rule holds by typing rather than by care — tier 3 speaks `Activity`, and
/// `Activity` has no block to spell — so the test walks every input there is
/// and shows that none of them produces one.
#[test]
fn unknown_foreground_program_reports_busy_or_quiet_never_blocked() {
    let registry = Registry::builtin();
    let tree = FakeTree::new()
        .with(100, &["vim", "notes.md"], &[])
        .with(200, &["claude"], &[]);

    let unknown = identify(&tree, &registry, ProcessId(100));
    assert_eq!(unknown.kind(), None);
    assert_eq!(
        unknown.program(),
        Some("vim"),
        "an unidentified pane still says what it is looking at"
    );
    assert_eq!(unknown.assert_state(Activity::Busy), Some(AgentState::Busy));
    assert_eq!(
        unknown.assert_state(Activity::Quiet),
        Some(AgentState::Quiet)
    );
    for activity in [Activity::Busy, Activity::Quiet] {
        assert_ne!(
            unknown.assert_state(activity),
            Some(AgentState::Blocked),
            "tier 3 never asserts a block"
        );
    }

    // Once an agent is identified, tier 3 stops asserting state at all: the
    // pane's status is the fusion machine's, from hook edges and the screen.
    let known = identify(&tree, &registry, ProcessId(200));
    assert!(known.kind().is_some());
    for activity in [Activity::Busy, Activity::Quiet] {
        assert_eq!(known.assert_state(activity), None);
    }
}

/// "Probes are triggered, rate-limited work … never a free-running scan loop."
///
/// Damage arrives in bursts and gets a floor; a spawn and an `agent start`
/// readiness check are events that just changed what is running, and delaying
/// them would delay the answer they exist to produce.
#[test]
fn damage_probes_are_rate_limited_and_spawn_and_readiness_are_not() {
    let start = std::time::Instant::now();
    let mut gate = ProbeGate::new();

    assert!(gate.admit(ProbeTrigger::Spawn, start));
    assert!(
        !gate.admit(ProbeTrigger::Damage, start),
        "a damage probe on the heels of the spawn probe adds nothing"
    );
    assert!(
        !gate.admit(ProbeTrigger::Damage, start + PROBE_SPACING / 2),
        "still inside the floor"
    );
    assert!(
        gate.admit(ProbeTrigger::Readiness, start + PROBE_SPACING / 2),
        "a caller blocked on the answer is never made to wait"
    );
    assert!(
        gate.admit(ProbeTrigger::Damage, start + PROBE_SPACING * 2),
        "past the floor, damage probes again"
    );
    // A refused probe does not push the next one further away: the floor is
    // measured from the last probe that ran, not from the last one asked for.
    assert!(!gate.admit(ProbeTrigger::Damage, start + PROBE_SPACING * 2));
    assert!(gate.admit(ProbeTrigger::Damage, start + PROBE_SPACING * 3));
}
