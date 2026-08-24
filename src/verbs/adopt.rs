//! `amx adopt` — a claude that was already there.
//!
//! Every other agent amx has is one it started. This one was running before
//! amx was asked about it: somebody's own claude, in their own tmux, in a pane
//! amx did not open. Adopting writes the record that was missing and changes
//! nothing else. No pane is started, nothing is sent, and the agent goes on
//! with whatever it was in the middle of.
//!
//! It is typed inside the claude being adopted, and that is what makes it
//! answerable rather than a guess. The pane is `$TMUX_PANE`, which tmux puts
//! in the environment of everything running in it, and the conversation is
//! `$CLAUDE_CODE_SESSION_ID`, which the vendor puts in the environment of
//! every command it starts. Both describe the claude that ran the command and
//! no other, so amx never has to work out which of the claudes on a machine
//! was meant.
//!
//! The session is the half that keeps working afterwards. amx cannot put its
//! own id into a pane it did not start, so the events this agent fires carry
//! nothing saying whose they are, and the hook falls back to finding the
//! record whose session matches the payload's. This is where that session is
//! written down.
//!
//! What amx did not do for this agent it does not claim: no worktree, no
//! branch, no commit to measure a diff from, and no command it was launched
//! with. `stop` takes its pane and nothing else, and there is nothing for
//! `resume` or `fork` to start again — that claude was started by hand and can
//! be again.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::cli::AdoptArgs;
use crate::rules::{Claim, Ruleset};
use crate::store::{Agent, Event, Meta, Phase, State, now};
use crate::tmux::{PaneId, Server, Socket};
use crate::{exit, ids, paths, rules, spawn};

/// What amx records when it takes over a claude it did not start.
const ADOPTED: &str = "adopt";

/// Where tmux says which pane a process is running in.
const PANE_ENV: &str = "TMUX_PANE";

/// Where claude says which conversation a command it started belongs to.
///
/// Measured against claude 2.1.240 on 2026-08-24: every process the vendor
/// starts — a tool call, a hook — is handed `CLAUDE_CODE_SESSION_ID`, holding
/// the same session id its hook payloads carry. Re-measure it at every vendor
/// bump: it is the whole of how an adopted agent's events find their way home.
const SESSION_ENV: &str = "CLAUDE_CODE_SESSION_ID";

/// Run the verb against the machine.
pub fn from_env(args: &AdoptArgs) -> Result<i32> {
    let root = paths::state_root()?;
    let env: BTreeMap<String, String> = std::env::vars().collect();
    let here = std::env::current_dir().context("no working directory")?;
    let server = spawn::server()?;
    let mut out = std::io::stdout().lock();
    run(
        &root,
        rules::bundled(),
        &server,
        &env,
        &here,
        args,
        &mut out,
    )
}

/// The verb, with everything it reads named.
pub fn run(
    root: &Path,
    rules: &Ruleset,
    server: &Server,
    env: &BTreeMap<String, String>,
    here: &Path,
    args: &AdoptArgs,
    out: &mut impl Write,
) -> Result<i32> {
    // Everything that would stop an adoption is asked before anything is
    // written: which pane, whose conversation, and whether amx is looking at
    // that pane already.
    let pane = this_pane(env)?;
    let session = this_session(env)?;
    if let Some(id) = env.get(crate::hook::ID_ENV)
        && Agent::open(root, id).is_ok()
    {
        bail!("this pane is agent `{id}` already, which amx started");
    }
    if !server.pane_alive(&pane) {
        bail!("{pane} is not a pane on the tmux server this is running on");
    }
    if let Some(refusal) = spoken_for(root, server.socket(), &pane, &session)? {
        bail!(refusal);
    }

    let dir = pane_dir(server, &pane, here);
    let task = match &args.task {
        Some(task) => task.clone(),
        None => label(&dir),
    };
    let id = match &args.name {
        Some(name) => {
            ids::validate_name(name, root)?;
            name.clone()
        }
        None => ids::generate(&task, root)?,
    };

    // The screen before the record, because the screen is what the record is
    // about to say. A pane amx cannot read is one there is nothing to adopt
    // from, and saying so leaves the state root as it was found.
    let screen = server
        .capture(&pane)
        .with_context(|| format!("reading what is on {pane}"))?;

    let agent = Agent::create(
        root,
        &Meta {
            id: id.clone(),
            task,
            dir,
            // amx cut nothing and started nothing here. A record claiming this
            // person's tree as an agent's worktree would be one `amx stop`
            // away from removing the work they are doing in it.
            worktree: None,
            branch: None,
            base: None,
            socket: server.socket().clone(),
            pane: pane.clone(),
            bg: false,
            session: Some(session.clone()),
            // The transcript arrives with the next SessionStart, if one ever
            // comes. What the agent says at the end of a turn is on the Stop
            // payload, which is the fresher of the two anyway.
            transcript: None,
            created: now(),
        },
    )?;

    let writer = agent.writer()?;
    writer.append(&Event::new(
        ADOPTED,
        serde_json::json!({ "pane": pane.as_str(), "session": session }),
    ))?;
    writer.update_state(|state| seed(state, rules, &screen))?;
    drop(writer);

    writeln!(out, "{id}")?;
    Ok(exit::OK)
}

/// Write down what the pane is showing, as the record's first word on what
/// this agent is doing.
///
/// An agent amx started is `starting` until its first hook, and that is
/// honest: nothing has happened yet. An adopted one has been working for an
/// hour, and a record that said `starting` about it would be believed —
/// readers take a record at its word while it is fresh, and the first reading
/// after an adoption is the freshest there is. So the screen answers for it
/// here, the same reading a reader would make of the same pane a minute later.
fn seed(state: &mut State, rules: &Ruleset, screen: &str) {
    let (phase, asking) = match rules.claim(screen, Phase::Starting, 1) {
        Claim::Ruled(rule) => (rule.state, rule.question(screen)),
        // Nothing amx knows accounts for the screen, which is what `unknown`
        // says everywhere else. An adoption is not the moment to guess.
        Claim::Unsettled(_) | Claim::Unclaimed => (Phase::Unknown, None),
    };
    state.state = phase;
    if let Some(asking) = asking {
        state.learn(&asking);
    }
}

/// The pane this command was typed in.
fn this_pane(env: &BTreeMap<String, String>) -> Result<PaneId> {
    let Some(pane) = env.get(PANE_ENV).filter(|pane| !pane.is_empty()) else {
        bail!(
            "no ${PANE_ENV} here, so this is not a tmux pane. \
             `amx adopt` is run inside the claude it adopts"
        );
    };
    PaneId::new(pane.clone()).with_context(|| format!("${PANE_ENV} holds {pane:?}"))
}

/// The conversation this command was typed in.
fn this_session(env: &BTreeMap<String, String>) -> Result<String> {
    let Some(session) = env.get(SESSION_ENV).filter(|id| !id.is_empty()) else {
        bail!(
            "no ${SESSION_ENV} here, so no claude started this command. \
             `amx adopt` is run inside the claude it adopts, and that session \
             id is how its events are recognised afterwards"
        );
    };
    Ok(session.clone())
}

/// Why amx will not adopt: it has an agent for this pane, or for this
/// conversation, already.
///
/// Only agents that are still going are in the way. A record that has ended is
/// history — the pane it names may have been somebody else's for a week — and
/// standing on a finished agent's toes is what an id is for.
fn spoken_for(
    root: &Path,
    socket: &Socket,
    pane: &PaneId,
    session: &str,
) -> Result<Option<String>> {
    let mut ids = crate::store::list(root)?;
    ids.sort();
    for id in ids {
        let agent = Agent::open(root, &id)?;
        let Ok(meta) = agent.meta() else { continue };
        if agent.state()?.state.is_terminal() {
            continue;
        }
        if &meta.pane == pane && &meta.socket == socket {
            return Ok(Some(format!("{pane} is agent `{id}` already")));
        }
        if meta.session.as_deref() == Some(session) {
            return Ok(Some(format!("this conversation is agent `{id}` already")));
        }
    }
    Ok(None)
}

/// Where the agent is working: the directory the pane is in.
///
/// tmux answers for the pane's own process, which is the claude being adopted.
/// A pane that will not say — a **value read**, so a blank answer is all this
/// gets — leaves the directory this command was typed in, which is inside that
/// same pane.
fn pane_dir(server: &Server, pane: &PaneId, here: &Path) -> PathBuf {
    server
        .pane_field(pane, "#{pane_current_path}")
        .ok()
        .map(|path| PathBuf::from(path.trim()))
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| here.to_path_buf())
}

/// What the row says an adopted agent is for, when nobody has said.
///
/// The task is the one thing about a claude amx did not start that amx cannot
/// know: it was given at a prompt hours ago, in a conversation amx has never
/// read. The directory is what is true and worth reading on a row, and the
/// word in front of it says why this row is not like the others.
fn label(dir: &Path) -> String {
    match dir.file_name().and_then(|name| name.to_str()) {
        Some(name) => format!("adopted {name}"),
        None => "adopted claude".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::Spawn;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    /// A socket name of this test's own.
    fn tag() -> String {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        format!(
            "amx-test-adopt-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// A claude's pane, or near enough: a real pane on a server of this test's
    /// own, with the rows a vendor would have drawn painted on it. Gone when
    /// the test is.
    struct APane {
        server: Server,
        pane: PaneId,
    }

    impl APane {
        fn showing(rows: &[&str]) -> APane {
            // An empty conf, so nothing in the developer's ~/.tmux.conf can
            // change what this measures.
            let server = Server::named(tag()).with_conf("/dev/null");
            let painted: Vec<String> = rows.iter().map(|row| format!("'{row}'")).collect();
            let script = format!(
                "printf '%s\\n' {}; while :; do sleep 0.05; done",
                painted.join(" ")
            );
            let (_, pane) = server
                .new_session(&Spawn {
                    command: &["sh", "-c", &script],
                    ..Spawn::default()
                })
                .expect("a pane to adopt");

            // The shell has to have drawn before the screen says anything.
            for _ in 0..200 {
                if server
                    .capture(&pane)
                    .is_ok_and(|screen| screen.contains(rows[rows.len() - 1]))
                {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            APane { server, pane }
        }

        /// The environment a command typed in this pane would run in.
        fn env(&self, session: &str) -> BTreeMap<String, String> {
            BTreeMap::from([
                (PANE_ENV.to_string(), self.pane.to_string()),
                (SESSION_ENV.to_string(), session.to_string()),
            ])
        }
    }

    impl Drop for APane {
        fn drop(&mut self) {
            let _ = self.server.kill();
        }
    }

    /// claude's permission box, as the ruleset's own measurement records it.
    const A_PERMISSION_BOX: [&str; 7] = [
        " Bash command",
        "   rm -f b.txt",
        " Permission rule Bash requires confirmation for this command.",
        " Do you want to proceed?",
        " ❯ 1. Yes",
        "   2. No",
        " Esc to cancel · Tab to amend · ctrl+e to explain",
    ];

    /// The verb, with nowhere for its answer to go but a buffer.
    fn adopt(
        root: &Path,
        pane: &APane,
        env: &BTreeMap<String, String>,
        args: &AdoptArgs,
    ) -> Result<(i32, String)> {
        let mut out = Vec::new();
        let code = run(
            root,
            rules::bundled(),
            &pane.server,
            env,
            Path::new("/srv/app"),
            args,
            &mut out,
        )?;
        Ok((code, String::from_utf8(out).unwrap()))
    }

    #[test]
    fn adopt_registers_the_claude_in_this_pane_and_reads_its_screen_at_once() {
        let root = TempDir::new().unwrap();
        let pane = APane::showing(&A_PERMISSION_BOX);

        let (code, printed) = adopt(
            root.path(),
            &pane,
            &pane.env("abc-123"),
            &AdoptArgs::default(),
        )
        .unwrap();
        assert_eq!(code, exit::OK);

        let id = printed.trim();
        assert!(!id.is_empty(), "the id is the whole of what it prints");
        let agent = Agent::open(root.path(), id).expect("a record for the adopted claude");

        let meta = agent.meta().unwrap();
        assert_eq!(meta.pane, pane.pane);
        assert_eq!(&meta.socket, pane.server.socket());
        assert_eq!(
            meta.session.as_deref(),
            Some("abc-123"),
            "which is the whole of how its events are recognised"
        );
        assert_eq!(
            (meta.worktree, meta.branch, meta.base),
            (None, None, None),
            "amx cut nothing here, so it claims nothing"
        );

        // The screen, read at the moment of adoption: a record saying
        // `starting` about an agent that has been going for an hour would be
        // believed by every reader for as long as it was fresh.
        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Waiting);
        assert_eq!(state.question.as_deref(), Some("Do you want to proceed?"));
        assert_eq!(state.options, ["Yes", "No"]);

        // And the log opens with where this agent came from, before the vendor
        // has said anything at all.
        let events = agent.events().unwrap();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].kind, ADOPTED);
        assert_eq!(events[0].payload["pane"], pane.pane.as_str());
        assert_eq!(events[0].payload["session"], "abc-123");
    }

    #[test]
    fn adopt_names_the_row_after_the_directory_and_takes_a_better_name() {
        let root = TempDir::new().unwrap();
        let pane = APane::showing(&["⏵⏵ auto mode on (shift+tab to cycle) · ← for agents"]);

        let (_, printed) = adopt(
            root.path(),
            &pane,
            &pane.env("abc-123"),
            &AdoptArgs::default(),
        )
        .unwrap();
        let agent = Agent::open(root.path(), printed.trim()).unwrap();
        assert_eq!(
            agent.meta().unwrap().task,
            label(&agent.meta().unwrap().dir),
            "the task is the one thing about a claude amx did not start that \
             amx cannot know"
        );
        assert_eq!(
            agent.state().unwrap().state,
            Phase::Idle,
            "the idle screen is claimed at once, with nothing outstanding to \
             hold the rule back"
        );

        // A row somebody has named says what they named it, and the id is the
        // name rather than a draw against the task.
        let second = APane::showing(&A_PERMISSION_BOX);
        let (_, printed) = adopt(
            root.path(),
            &second,
            &second.env("def-456"),
            &AdoptArgs {
                task: Some("port the importer".to_string()),
                name: Some("importer".to_string()),
            },
        )
        .unwrap();
        assert_eq!(printed.trim(), "importer");
        assert_eq!(
            Agent::open(root.path(), "importer")
                .unwrap()
                .meta()
                .unwrap()
                .task,
            "port the importer"
        );
    }

    #[test]
    fn adopt_the_row_of_a_screen_no_rule_claims_says_it_cannot_tell() {
        let root = TempDir::new().unwrap();
        let pane = APane::showing(&["a shell prompt and nothing a vendor drew"]);

        let (_, printed) = adopt(
            root.path(),
            &pane,
            &pane.env("abc-123"),
            &AdoptArgs::default(),
        )
        .unwrap();
        let agent = Agent::open(root.path(), printed.trim()).unwrap();
        assert_eq!(agent.state().unwrap().state, Phase::Unknown);
        assert_eq!(agent.state().unwrap().question, None);
    }

    #[test]
    fn adopt_needs_the_pane_and_the_conversation_it_is_typed_in() {
        let root = TempDir::new().unwrap();
        let pane = APane::showing(&A_PERMISSION_BOX);

        // Not in tmux at all, and in tmux with no claude around it.
        let outside = BTreeMap::from([(SESSION_ENV.to_string(), "abc-123".to_string())]);
        let said = format!(
            "{:#}",
            adopt(root.path(), &pane, &outside, &AdoptArgs::default()).unwrap_err()
        );
        assert!(said.contains(PANE_ENV), "{said}");

        let shell = BTreeMap::from([(PANE_ENV.to_string(), pane.pane.to_string())]);
        let said = format!(
            "{:#}",
            adopt(root.path(), &pane, &shell, &AdoptArgs::default()).unwrap_err()
        );
        assert!(said.contains(SESSION_ENV), "{said}");

        // A pane that is not on this server, and one that is not a pane.
        let mut elsewhere = pane.env("abc-123");
        elsewhere.insert(PANE_ENV.to_string(), "%404".to_string());
        let said = format!(
            "{:#}",
            adopt(root.path(), &pane, &elsewhere, &AdoptArgs::default()).unwrap_err()
        );
        assert!(said.contains("not a pane"), "{said}");

        let mut nonsense = pane.env("abc-123");
        nonsense.insert(PANE_ENV.to_string(), "the-third-one".to_string());
        assert!(adopt(root.path(), &pane, &nonsense, &AdoptArgs::default()).is_err());

        assert!(
            crate::store::list(root.path()).unwrap().is_empty(),
            "and nothing was written for any of them"
        );
    }

    #[test]
    fn adopt_refuses_a_pane_or_a_conversation_amx_is_already_looking_at() {
        let root = TempDir::new().unwrap();
        let pane = APane::showing(&A_PERMISSION_BOX);
        let env = pane.env("abc-123");

        let (_, printed) = adopt(root.path(), &pane, &env, &AdoptArgs::default()).unwrap();
        let first = printed.trim().to_string();

        // The same pane again: an agent adopted twice is two records driving
        // one claude, and answering it from either would type the answer twice.
        let said = format!(
            "{:#}",
            adopt(root.path(), &pane, &env, &AdoptArgs::default()).unwrap_err()
        );
        assert!(said.contains(&first), "{said}");
        assert!(said.contains("already"), "{said}");

        // The same conversation from somewhere else — a claude resumed by hand
        // in a pane of its own.
        let second = APane::showing(&A_PERMISSION_BOX);
        let said = format!(
            "{:#}",
            adopt(
                root.path(),
                &second,
                &second.env("abc-123"),
                &AdoptArgs::default()
            )
            .unwrap_err()
        );
        assert!(said.contains("this conversation"), "{said}");
        assert!(said.contains(&first), "{said}");

        // A pane amx started is one amx already has an id for, and its
        // environment says so without anything being read.
        let mut inside = pane.env("def-456");
        inside.insert(crate::hook::ID_ENV.to_string(), first.clone());
        let said = format!(
            "{:#}",
            adopt(root.path(), &pane, &inside, &AdoptArgs::default()).unwrap_err()
        );
        assert!(said.contains(&first), "{said}");

        assert_eq!(
            crate::store::list(root.path()).unwrap(),
            [first],
            "and none of the refusals left a record behind"
        );
    }

    #[test]
    fn adopt_stands_aside_for_a_record_that_has_ended() {
        // A stopped agent's pane is nobody's a week later, and the machine
        // hands the same pane ids out again.
        let root = TempDir::new().unwrap();
        let pane = APane::showing(&A_PERMISSION_BOX);
        let env = pane.env("abc-123");

        let (_, printed) = adopt(root.path(), &pane, &env, &AdoptArgs::default()).unwrap();
        let first = Agent::open(root.path(), printed.trim()).unwrap();
        first
            .writer()
            .unwrap()
            .update_state(|state| state.state = Phase::Stopped)
            .unwrap();

        let (code, printed) = adopt(root.path(), &pane, &env, &AdoptArgs::default()).unwrap();
        assert_eq!(code, exit::OK);
        assert_ne!(printed.trim(), first.id());
    }

    #[test]
    fn adopt_the_row_is_named_after_the_directory_the_pane_is_in() {
        assert_eq!(label(Path::new("/srv/app")), "adopted app");
        assert_eq!(label(Path::new("/")), "adopted claude");
        assert_eq!(
            ids::stem_from_task(&label(Path::new("/srv/app"))),
            "adopted-app"
        );
    }
}
