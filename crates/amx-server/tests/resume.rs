//! V15: agent resume — a captured conversation back into a restarted pane.
//!
//! D-M2-7 keeps herdr's rigor wholesale, with the tables moved into the
//! registry, and each of its clauses is a claim somebody could get wrong:
//!
//! - **Refs are captured, not invented.** They arrive in `Core`'s mirror from
//!   `AgentHub` during normal operation and ride the ordinary capture path into
//!   the snapshot — which is exactly why the hub's shutdown has nothing to
//!   flush (`docs/08-m2-plan.md` §3).
//! - **Sources are allowlisted at every gate.** A `session.json` is a file a
//!   user can edit, so the ref that reached it under a hook's authority is
//!   checked again on the way back out, against the stanza it claims.
//! - **argv is data.** The template's one `{ref}` slot takes the ref as a whole
//!   element, and the single transformation into shell text quotes it once, at
//!   the boundary. A ref holding a semicolon is a word, never a command.
//! - **One conversation, one pane.** The dedupe reservation is taken before the
//!   spawn and released if the spawn fails, so a pane that never started does
//!   not take its conversation with it.
//! - **Launch is type-in, not exec.** The pane comes back as its shell and the
//!   invocation is typed into it after it paints, so the pane survives the
//!   agent's eventual exit and the user can see what ran.
//!
//! What is asserted here is what the *child* received and what the *user* is
//! told, never what the server intended: a resume that was planned and never
//! typed is a lost conversation, and a lost conversation is a report entry.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use amx_core::agent::{
    AgentKind, AgentState, RefKind, RefSource, SessionRef, StartSource, StatusCause,
};
use amx_core::{Config, Layout, PaneId, WorkspaceId};
use amx_proto::control::session::{RestoreEntity, RestoreSeverity};
use amx_server::actor::{AgentCall, CoreCommand};
use amx_server::agent::registry::Registry;
use amx_server::agent::resume::{self, Reservations, ResumeRefusal};
use amx_server::persist::{AgentSnapshot, PaneSnapshot, PersistError, snapshot};
use serde_json::json;
use tokio::sync::watch;

mod support;

use support::restore_rig::{
    Fixture, Running, entries, pane_row, report_of, row_of, snapshot_of, wait_until, workspace_row,
};

/// The conversation every test here resumes, spelled as Claude Code spells one
/// (V01 §3 M8: a v4-shaped UUID, and the resume ref *is* the session id).
const CONVERSATION: &str = "c9a3c73b-b184-4871-8e98-79b46b87b635";

// --------------------------------------------------------------------- builders

fn claude() -> AgentKind {
    AgentKind::new("claude").expect("a well-formed agent id")
}

fn codex() -> AgentKind {
    AgentKind::new("codex").expect("a well-formed agent id")
}

fn conversation(value: &str) -> SessionRef {
    SessionRef::new(RefKind::Id, value).expect("a well-formed session ref")
}

/// What a pane running `kind` in conversation `value` looks like on disk.
fn agent_row(kind: &AgentKind, name: &str, value: &str) -> AgentSnapshot {
    AgentSnapshot {
        kind: kind.clone(),
        name: Some(name.to_owned()),
        session_ref: Some(conversation(value)),
        source: Some(RefSource::for_agent(kind)),
        start_source: StartSource::Started,
    }
}

/// A saved pane in `cwd` whose agent was in a conversation.
fn agent_pane(
    pane: PaneId,
    short: u32,
    name: &str,
    cwd: &Path,
    agent: AgentSnapshot,
) -> PaneSnapshot {
    PaneSnapshot {
        agent: Some(agent),
        ..pane_row(pane, short, Some(name), Some(cwd.to_path_buf()))
    }
}

/// The live status `AgentHub` mirrors into `Core` for an identified agent.
fn mirrored(kind: &AgentKind, value: &str) -> amx_core::agent::AgentSnapshot {
    amx_core::agent::AgentSnapshot {
        kind: Some(kind.clone()),
        state: AgentState::Idle,
        cause: StatusCause::Hook,
        transition_seq: 1,
        attention: None,
        session_ref: Some(conversation(value)),
    }
}

/// Pin the pane shell so no test here starts the developer's login shell.
///
/// The same channel the serve path publishes configuration on, so nothing about
/// the spawn is faked — only the program at the end of it.
fn pin_shell(fx: &mut Fixture, shell: &str) -> watch::Sender<Config> {
    let mut pinned = Config::default();
    pinned.terminal.shell = Some(shell.to_owned());
    let (tx, rx) = watch::channel(pinned);
    fx.core.set_config(rx);
    tx
}

/// A "shell" that goes raw, paints its prompt, and records what is typed at it.
///
/// One script serves every pane in a test, so it records into `typed.<pid>`:
/// a suite that restores two panes can then ask *how many* of them were typed
/// into, which is the whole question the dedupe rule answers.
///
/// Raw mode is what makes the recording exact — with echo and CR/LF translation
/// off, the bytes in the file are the bytes amx queued — and the `ready` it
/// paints before recording is the first damage the resume waits for, so the
/// wait is on a condition the script guarantees rather than on a race.
fn recorder_shell(fx: &Fixture, bytes: usize) -> PathBuf {
    let shell = fx.dir.path().join("shell.sh");
    std::fs::write(
        &shell,
        format!(
            "#!/bin/sh\nstty raw -echo\nprintf 'ready\\r\\n'\n\
             dd bs=1 count={bytes} of={}/{RECORDING_PREFIX}$$ 2>/dev/null\n",
            fx.dir.path().display(),
        ),
    )
    .expect("write the recording shell");
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755))
        .expect("make it executable");
    shell
}

/// The prefix every recorded pane writes its input under.
const RECORDING_PREFIX: &str = "typed.";

/// Every recording that holds a whole invocation, in no particular order.
///
/// `at_least` and not "non-empty": the child records a byte at a time, so a
/// file that exists is a write in progress and a file of the expected length is
/// a write that finished. Waiting on this predicate is what makes the assertion
/// after it about the bytes rather than about the timing.
fn recordings(dir: &Path, at_least: usize) -> Vec<String> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read the fixture directory") {
        let path = entry.expect("a directory entry").path();
        let named = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(RECORDING_PREFIX));
        if !named {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap_or_default();
        if bytes.len() >= at_least {
            found.push(String::from_utf8_lossy(&bytes).into_owned());
        }
    }
    found
}

/// What a pane resuming [`CONVERSATION`] is expected to receive: the planned
/// argv as one command line, plus the byte `pane.run` submits with.
fn expected_invocation() -> String {
    format!("claude --resume {CONVERSATION}\r")
}

/// Every degraded-pane entry's reason, for a report that should name a loss.
async fn degraded_reasons(running: &Running) -> Vec<String> {
    report_of(running)
        .await
        .entries
        .iter()
        .filter(|entry| {
            entry.severity == RestoreSeverity::Degraded && entry.entity == RestoreEntity::Pane
        })
        .map(|entry| entry.reason.clone())
        .collect()
}

// ------------------------------------------------------------------ the capture

/// The mirror is the capture's only source for a ref: `AgentHub` fills it with
/// `try_send` during normal operation, and the snapshot reads it there.
#[tokio::test]
async fn hook_reported_refs_survive_into_the_snapshot_via_the_core_mirror() {
    let mut fx = Fixture::new("mirr");
    let _shell = pin_shell(&mut fx, "/bin/sh");
    let (ws, pane) = (WorkspaceId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");

    let opts = fx.opts();
    fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                1,
                None,
                Layout::with_root(pane),
                Some(pane),
            )],
            vec![pane_row(pane, 1, Some("dev"), Some(work))],
        ),
        &opts,
    );
    let running = fx.start();

    // Exactly what the hub sends: one status, mirrored fire-and-forget.
    running
        .tx
        .send(CoreCommand::Agent(AgentCall::Status {
            pane,
            status: Some(Box::new(mirrored(&claude(), CONVERSATION))),
            attention: Vec::new(),
        }))
        .await
        .expect("core mailbox is open");

    let capture = running.capture().await;
    let saved = &capture.snapshot.panes[0];
    assert_eq!(
        saved.argv.as_deref(),
        Some(&["/bin/sh".to_owned()][..]),
        "the spawn argv V07 records reaches the file",
    );
    let agent = saved.agent.as_ref().expect("the pane's agent was captured");
    assert_eq!(agent.kind, claude());
    assert_eq!(agent.name.as_deref(), Some("dev"), "the name is the label");
    assert_eq!(
        agent.session_ref.as_ref(),
        Some(&conversation(CONVERSATION))
    );
    assert_eq!(
        agent.source,
        Some(RefSource::for_agent(&claude())),
        "a ref reached the mirror under `claude`, so it came from `claude`'s hook path",
    );
    assert_eq!(
        agent.start_source,
        StartSource::Detected,
        "this pane runs a shell with an agent identified inside it",
    );

    // The live status itself is *not* written: it is derived and dies with the
    // process, and restoring "this agent was idle yesterday" would be a lie.
    let text = serde_json::to_string(&capture.snapshot).expect("the snapshot serializes");
    assert!(
        !text.contains("idle"),
        "no status crossed into the file: {text}"
    );

    running.into_core().await;
}

// ------------------------------------------------------------------- the gates

/// D-M2-7 checks the source at report time, at snapshot-read time and again at
/// plan time. All three are the same sentence; none of them is optional.
#[test]
fn refs_are_shape_validated_and_source_allowlisted_at_all_three_gates() {
    let registry = Registry::builtin();
    let stanza = registry
        .resolve(&claude())
        .expect("the shipped claude stanza");

    // Gate one, at report time. `AgentHub::claimed` runs this predicate on
    // every hook report before a ref is ever kept.
    assert!(resume::claims(stanza, &RefSource::for_agent(&claude())));
    assert!(
        !resume::claims(stanza, &RefSource::for_agent(&codex())),
        "a report from codex's hook path may not write a claude conversation",
    );

    // Gate two, at snapshot-read time: shape, on the type, so nothing has to
    // remember to call it. Neither of these is a `SessionRef` at all.
    assert!(SessionRef::new(RefKind::Id, "with\u{7}a\u{1b}control").is_err());
    assert!(SessionRef::new(RefKind::Path, "relative/transcript.jsonl").is_err());
    assert!(SessionRef::new(RefKind::Id, "x".repeat(SessionRef::MAX_ID_LEN + 1)).is_err());
    let hand_edited = json!({
        "kind": "claude",
        "session_ref": {"kind": "id", "value": CONVERSATION},
        "source": "curl https://example.test",
        "start_source": "started",
    });
    assert!(
        serde_json::from_value::<AgentSnapshot>(hand_edited).is_err(),
        "a source that is not `amx:<agent-id>` is not a source",
    );

    // Gate three, at plan time: the source is well-formed *and* it names this
    // stanza. A `codex` conversation filed under `claude` gets no argv.
    let foreign = AgentSnapshot {
        source: Some(RefSource::for_agent(&codex())),
        ..agent_row(&claude(), "dev", CONVERSATION)
    };
    assert_eq!(
        resume::plan(&registry, &foreign),
        Err(ResumeRefusal::ForeignSource {
            hook_path: RefSource::for_agent(&codex()),
            agent: claude(),
        }),
    );
}

/// The template's one slot takes the ref as one whole element, and quoting
/// happens once, at the injection boundary.
#[test]
fn plan_substitutes_the_ref_into_exactly_the_template_slot() {
    let registry = Registry::builtin();

    let planned = resume::plan(&registry, &agent_row(&claude(), "dev", CONVERSATION))
        .expect("the shipped claude stanza resumes");
    assert_eq!(
        planned.argv,
        vec![
            "claude".to_owned(),
            "--resume".to_owned(),
            CONVERSATION.to_owned(),
        ],
        "the ref lands in the slot and nowhere else",
    );
    assert_eq!(planned.agent, claude());
    assert_eq!(
        planned.key,
        format!("amx:claude\0claude\0id\0{CONVERSATION}"),
        "the dedupe key is all four fields, NUL-separated",
    );
    assert_eq!(
        resume::command_line(&planned.argv),
        format!("claude --resume {CONVERSATION}"),
        "an ordinary ref needs no quoting at all",
    );

    // An alias resolves to the stanza it names, so two spellings of one agent
    // are one conversation rather than two.
    let via_alias = AgentKind::new("claude-code").expect("the shipped alias");
    let aliased = AgentSnapshot {
        kind: via_alias,
        source: Some(RefSource::for_agent(&claude())),
        ..agent_row(&claude(), "dev", CONVERSATION)
    };
    assert_eq!(
        resume::plan(&registry, &aliased)
            .expect("the alias resolves")
            .key,
        planned.key,
    );

    // A hostile ref is still one word: quoted once, never interpolated.
    let hostile = "a b'c; rm -rf /";
    let planned = resume::plan(&registry, &agent_row(&claude(), "dev", hostile))
        .expect("the shape rules admit spaces and quotes");
    assert_eq!(planned.argv[2], hostile, "argv is data, all the way down");
    assert_eq!(
        resume::command_line(&planned.argv),
        r"claude --resume 'a b'\''c; rm -rf /'",
    );
}

/// A `session.json` is a file a user can edit. Nothing they can write into one
/// becomes an argument to a process.
#[test]
fn a_hand_edited_snapshot_ref_cannot_reach_an_argv() {
    let registry = Registry::builtin();
    let dir = support::TempDir::new("hand");

    // Control characters and relative paths do not survive the read gate: they
    // are not `SessionRef` values, so the bytes are not a snapshot. The pane
    // cannot be restored *with* a conversation because there is no conversation
    // to restore — reported as a whole-session loss by `restore_from_disk`.
    for hostile in [
        json!({"kind": "id", "value": "9d2f\u{1b}[31m-injected"}),
        json!({"kind": "path", "value": "../../etc/passwd"}),
        json!({"kind": "id", "value": ""}),
    ] {
        std::fs::write(
            dir.path().join("session.json"),
            serde_json::to_vec(&json!({
                "version": 1,
                "workspaces": [],
                "panes": [{
                    "id": PaneId::new_v4().to_string(),
                    "short": 1,
                    "agent": {
                        "kind": "claude",
                        "session_ref": hostile,
                        "source": "amx:claude",
                        "start_source": "started",
                    },
                }],
            }))
            .expect("the hostile fixture serializes"),
        )
        .expect("plant the hand-edited snapshot");
        let err = snapshot::load(dir.path()).expect_err("a hostile ref is not a ref");
        assert!(
            matches!(err, PersistError::Malformed(_)),
            "{hostile}: {err}"
        );
    }

    // What *does* parse still has to earn an argv. Each of these is a pane that
    // comes back as a plain shell, with the reason in the restore report.
    for (edited, refusal) in [
        (
            AgentSnapshot {
                source: Some(RefSource::for_agent(&codex())),
                ..agent_row(&claude(), "dev", CONVERSATION)
            },
            ResumeRefusal::ForeignSource {
                hook_path: RefSource::for_agent(&codex()),
                agent: claude(),
            },
        ),
        (
            AgentSnapshot {
                source: None,
                ..agent_row(&claude(), "dev", CONVERSATION)
            },
            ResumeRefusal::NoSource,
        ),
        (
            AgentSnapshot {
                session_ref: Some(
                    SessionRef::new(RefKind::Path, "/tmp/somewhere.jsonl").expect("absolute"),
                ),
                ..agent_row(&claude(), "dev", CONVERSATION)
            },
            ResumeRefusal::WrongRefKind {
                agent: claude(),
                found: "path",
                wanted: "id",
            },
        ),
        (
            agent_row(
                &AgentKind::new("an-agent-this-build-dropped").expect("a well-formed id"),
                "dev",
                CONVERSATION,
            ),
            ResumeRefusal::UnknownAgent {
                agent: AgentKind::new("an-agent-this-build-dropped").expect("a well-formed id"),
            },
        ),
    ] {
        assert_eq!(resume::plan(&registry, &edited), Err(refusal));
    }
}

// -------------------------------------------------------------- the reservation

/// Two panes cannot hold one conversation: the second comes back as a shell and
/// is told why.
#[tokio::test]
async fn two_panes_claiming_one_conversation_resume_once_second_restores_a_shell() {
    let mut fx = Fixture::new("dedu");
    let fx_dir = fx.dir.path().to_path_buf();
    let expected = expected_invocation();
    let shell = recorder_shell(&fx, expected.len());
    let _shell = pin_shell(&mut fx, &shell.to_string_lossy());
    let (ws, first, second) = (WorkspaceId::new_v4(), PaneId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");

    let opts = fx.opts();
    let summary = fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                1,
                None,
                row_of(&[first, second]),
                Some(first),
            )],
            vec![
                agent_pane(
                    first,
                    1,
                    "a1",
                    &work,
                    agent_row(&claude(), "a1", CONVERSATION),
                ),
                agent_pane(
                    second,
                    2,
                    "a2",
                    &work,
                    agent_row(&claude(), "a2", CONVERSATION),
                ),
            ],
        ),
        &opts,
    );

    assert_eq!(
        summary.restored, 3,
        "the workspace and both panes came back"
    );
    assert_eq!(
        (summary.lost, summary.degraded),
        (0, 1),
        "nothing was lost; one conversation could not be handed out twice",
    );

    let running = fx.start();
    let reasons = degraded_reasons(&running).await;
    assert_eq!(reasons.len(), 1);
    assert!(
        reasons[0].contains("another pane resumed this conversation"),
        "the second pane is told what it lost: {reasons:?}",
    );
    let report = report_of(&running).await;
    let degraded = entries(&report, RestoreSeverity::Degraded, RestoreEntity::Pane);
    assert_eq!(
        degraded[0].pane,
        Some(second),
        "the pane that lost the race is the one that reports it",
    );

    // And the count that matters is not the report's but the children's: one
    // pane was typed into, the other is sitting at a plain shell.
    let (dir, want) = (fx_dir.clone(), expected.len());
    wait_until("one pane receives the resume invocation", async || {
        !recordings(&dir, want).is_empty()
    })
    .await;
    assert_eq!(recordings(&fx_dir, want), vec![expected]);

    running.into_core().await;
}

/// A pane whose shell will not start must not take its conversation with it.
#[tokio::test]
async fn failed_spawn_releases_the_reservation_for_a_later_pane() {
    let mut fx = Fixture::new("roll");
    let fx_dir = fx.dir.path().to_path_buf();
    let expected = expected_invocation();
    let shell = recorder_shell(&fx, expected.len());
    let _shell = pin_shell(&mut fx, &shell.to_string_lossy());
    let (ws, doomed, later) = (WorkspaceId::new_v4(), PaneId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");
    let gone = fx.missing("a-directory-that-was-deleted");

    // The home it would fall back to does not exist either, so the doomed
    // pane's respawn fails in `chdir` — a real spawn failure, not a simulated
    // one — *after* it has reserved the conversation.
    let opts = fx.opts_without_home();
    let summary = fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                1,
                None,
                row_of(&[doomed, later]),
                Some(later),
            )],
            vec![
                agent_pane(
                    doomed,
                    1,
                    "a1",
                    &gone,
                    agent_row(&claude(), "a1", CONVERSATION),
                ),
                agent_pane(
                    later,
                    2,
                    "a2",
                    &work,
                    agent_row(&claude(), "a2", CONVERSATION),
                ),
            ],
        ),
        &opts,
    );

    assert_eq!(summary.restored, 2, "the workspace and the surviving pane");
    assert_eq!(
        (summary.lost, summary.degraded),
        (1, 0),
        "the doomed pane is lost; the survivor's conversation is not",
    );

    let running = fx.start();
    assert!(
        degraded_reasons(&running).await.is_empty(),
        "the second pane claimed the conversation the first one gave back",
    );
    let (dir, want) = (fx_dir.clone(), expected.len());
    wait_until(
        "the surviving pane receives the resume invocation",
        async || !recordings(&dir, want).is_empty(),
    )
    .await;
    assert_eq!(
        recordings(&fx_dir, want),
        vec![expected],
        "the conversation the doomed pane reserved was handed to the next one",
    );

    running.into_core().await;
}

// ------------------------------------------------------------------ the launch

/// The pane comes back as its shell, and the invocation is *typed* into it once
/// it has painted — so the child really receives it, and the pane survives the
/// agent's eventual exit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_types_the_command_after_first_damage_and_the_child_receives_it() {
    let mut fx = Fixture::new("type");
    let fx_dir = fx.dir.path().to_path_buf();
    let work = fx.dir_named("work");
    let expected = expected_invocation();
    let shell = recorder_shell(&fx, expected.len());
    let _shell = pin_shell(&mut fx, &shell.to_string_lossy());

    let (ws, pane) = (WorkspaceId::new_v4(), PaneId::new_v4());
    let opts = fx.opts();
    let summary = fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                1,
                None,
                Layout::with_root(pane),
                Some(pane),
            )],
            vec![agent_pane(
                pane,
                1,
                "dev",
                &work,
                agent_row(&claude(), "dev", CONVERSATION),
            )],
        ),
        &opts,
    );
    assert!(summary.is_clean(), "the pane came back whole");

    let running = fx.start();
    let (dir, want) = (fx_dir.clone(), expected.len());
    wait_until("the child receives the resume invocation", async || {
        !recordings(&dir, want).is_empty()
    })
    .await;
    assert_eq!(
        recordings(&fx_dir, want),
        vec![expected],
        "the child received the planned argv, quoted once, and a submit",
    );

    // And the pane is still its shell: what was typed is an invocation in the
    // user's own history, not the pane's process.
    let capture = running.capture().await;
    assert_eq!(
        capture.snapshot.panes[0].argv.as_deref(),
        Some(&[shell.to_string_lossy().into_owned()][..]),
        "restore respawns the saved shell and never execs the agent's argv",
    );

    running.into_core().await;
}

/// The reservation table on its own: claim, refuse, release, re-claim.
#[test]
fn reservations_hand_a_conversation_to_one_holder_at_a_time() {
    let mut held = Reservations::new();
    let key = "amx:claude\0claude\0id\0one";

    assert!(held.is_empty());
    assert!(held.reserve(key), "the first caller gets it");
    assert!(!held.reserve(key), "the second does not");
    assert!(held.holds(key));
    assert_eq!(held.len(), 1);

    held.release(key);
    assert!(!held.holds(key));
    assert!(held.reserve(key), "released is claimable again");
}
