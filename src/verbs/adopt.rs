//! `amx adopt` — an agent that was already there.
//!
//! Every other agent amx has is one it started. This one was running before
//! amx was asked about it: somebody's own agent, in their own tmux, in a pane
//! amx did not open. Adopting writes the record that was missing and changes
//! nothing else. No pane is started, nothing is sent, and the agent goes on
//! with whatever it was in the middle of.
//!
//! It is typed inside the agent being adopted, and that is what makes it
//! answerable rather than a guess. The pane is `$TMUX_PANE`, which tmux puts
//! in the environment of everything running in it, and the conversation is
//! whichever variable the vendor names its session in, which it puts in the
//! environment of every command it starts. Both describe the agent that ran
//! the command and no other, so amx never has to work out which of the agents
//! on a machine was meant.
//!
//! Which vendor that is comes from the environment too, and not from the
//! config: what is in this pane is what somebody started themselves, which
//! need not be what `amx new` would spawn. So the table is read for the
//! vendors that can be taken over, and the one whose variable is here is the
//! one that is here.
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
//! `resume` or `fork` to start again — that agent was started by hand and can
//! be again.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::cli::AdoptArgs;
use crate::rules::{Claim, Ruleset};
use crate::store::{Agent, Event, Meta, Phase, State, now};
use crate::tmux::{PaneId, Server, Socket};
use crate::vendor::{Capability, Vendor};
use crate::{exit, ids, paths, registry, rules, spawn};

/// What amx records when it takes over an agent it did not start.
const ADOPTED: &str = "adopt";

/// Where tmux says which pane a process is running in.
const PANE_ENV: &str = "TMUX_PANE";

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
    // written: whether this pane is amx's own, which pane it is, whose
    // conversation is in it, and whether amx is looking at it already.
    //
    // A pane amx started says so in its own environment, and that is the
    // cheapest answer there is. An id with no record behind it is not one: the
    // agent it named has been forgotten, and what is in the pane now is an
    // agent like any other.
    if let Some(id) = env.get(crate::hook::ID_ENV)
        && Agent::open(root, id).is_ok()
    {
        bail!("this pane is agent `{id}` already, which amx started");
    }

    let pane = this_pane(env)?;
    let (vendor, session) = this_session(env)?;
    if !server.pane_alive(&pane) {
        bail!("{pane} is not a pane on the tmux server this is running on");
    }
    if let Some(refusal) = spoken_for(root, server.socket(), &pane, &session)? {
        bail!(refusal);
    }

    let dir = pane_dir(server, &pane, here);
    let task = match &args.task {
        Some(task) => task.clone(),
        None => label(&dir, vendor),
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
            // Which vendor is in the pane is the environment's word, not the
            // config's, and it is the one thing about this agent amx learns
            // here that outlives the reading.
            agent: Some(vendor.name.to_string()),
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

    // Nothing after the record is written is undone if it fails. The record
    // names a live pane, which is the whole of what an agent is, so what a
    // failure here costs is the opening line of the log and one reading that
    // the next look at the pane makes again.
    let writer = agent.writer()?;
    writer.append(&Event::new(
        ADOPTED,
        // The vendor goes with them, as it does on the record: what was in
        // that pane is the question somebody asks a week later, and the log is
        // where they read what happened rather than what is so now.
        serde_json::json!({
            "pane": pane.as_str(),
            "session": session,
            "vendor": vendor.name,
        }),
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
            "no ${PANE_ENV} here: `amx adopt` needs the claude to be running \
             inside a tmux pane, because a pane is the only thing amx can \
             watch and type at"
        );
    };
    PaneId::new(pane.clone()).with_context(|| format!("${PANE_ENV} holds {pane:?}"))
}

/// The vendor this command was typed inside, and the conversation it names.
fn this_session(env: &BTreeMap<String, String>) -> Result<(&'static Vendor, String)> {
    session_in(registry::entries(), env)
}

/// The same, against a table named rather than the one amx ships.
///
/// A vendor puts the session a command belongs to in that command's
/// environment, under a name of its own, so the environment says both which
/// vendor is in this pane and which conversation is in it. The first vendor
/// whose variable is here is the answer, and two of them cannot be: a pane
/// runs one agent.
fn session_in<'v>(
    vendors: &'v [Vendor],
    env: &BTreeMap<String, String>,
) -> Result<(&'v Vendor, String)> {
    for vendor in vendors {
        let Some(named) = vendor.session_env else {
            continue;
        };
        let Some(session) = env.get(named).filter(|id| !id.is_empty()) else {
            continue;
        };
        // A vendor that names a session amx cannot take over: the record would
        // be written, and nothing would ever reach it, because being taken
        // over is the whole of what the id in that variable is for here.
        if !vendor.can(Capability::Adopt) {
            bail!(
                "${named} says this is a {} session, and a {} cannot be taken \
                 over: amx would have a record here and no way to hear from it",
                vendor.name,
                vendor.name
            );
        }
        return Ok((vendor, session.clone()));
    }

    let names: Vec<&str> = adoptable(vendors).map(|vendor| vendor.name).collect();
    if names.is_empty() {
        bail!(
            "amx has an entry for no vendor it can take over, so there is \
             nothing here for `amx adopt` to write a record about"
        );
    }
    let looked_for: Vec<String> = adoptable(vendors)
        .filter_map(|vendor| vendor.session_env)
        .map(|named| format!("${named}"))
        .collect();
    bail!(
        "no {} here, so no {} started this command. `amx adopt` is run inside \
         the agent it adopts, and that session id is how its events are \
         recognised afterwards",
        either(&looked_for),
        either(&names)
    )
}

/// The vendors amx can be asked to take one of over.
fn adoptable(vendors: &[Vendor]) -> impl Iterator<Item = &Vendor> {
    vendors
        .iter()
        .filter(|vendor| vendor.can(Capability::Adopt))
}

/// A list of things to name in a sentence, however many of them there turn out
/// to be: `a`, or `a or b`, or `a, b or c`.
fn either(each: &[impl AsRef<str>]) -> String {
    let each: Vec<&str> = each.iter().map(AsRef::as_ref).collect();
    match each.split_last() {
        Some((last, [])) => (*last).to_string(),
        Some((last, before)) => format!("{} or {last}", before.join(", ")),
        // A table with nothing adoptable in it is refused before this is
        // reached: a sentence naming none of them would have a hole in it.
        None => "nothing".to_string(),
    }
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
/// The task is the one thing about an agent amx did not start that amx cannot
/// know: it was given at a prompt hours ago, in a conversation amx has never
/// read. The directory is what is true and worth reading on a row, and the
/// word in front of it says why this row is not like the others. A directory
/// with no name to it leaves the vendor, which is the other true thing.
fn label(dir: &Path, vendor: &Vendor) -> String {
    match dir.file_name().and_then(|name| name.to_str()) {
        Some(name) => format!("adopted {name}"),
        None => format!("adopted {}", vendor.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::Spawn;
    use crate::vendor::second::SECOND;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    /// The vendor these tests take over, read out of the table rather than
    /// named: which vendor amx can be asked to adopt, and what it calls its
    /// session, are the table's to say.
    fn a_vendor() -> &'static Vendor {
        registry::entries()
            .iter()
            .find(|vendor| vendor.can(Capability::Adopt))
            .expect("a vendor amx can take over")
    }

    /// What that vendor names its session in.
    fn session_env() -> &'static str {
        a_vendor()
            .session_env
            .expect("a vendor that can be adopted names its session")
    }

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
                (session_env().to_string(), session.to_string()),
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
        assert_eq!(
            meta.agent.as_deref(),
            Some(a_vendor().name),
            "the vendor whose variable is in this pane, and not the one the \
             config would have spawned"
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
            label(&agent.meta().unwrap().dir, a_vendor()),
            "the task is the one thing about an agent amx did not start that \
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
    fn adopt_spells_no_vendors_variable_of_its_own() {
        // Which variable names a session is the vendor's word, and a copy of
        // it here is the one that goes stale the day the vendor renames it.
        let ships = include_str!("adopt.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap_or_default();
        for vendor in registry::entries() {
            if let Some(session) = vendor.session_env {
                assert!(
                    !ships.contains(session),
                    "adopt keeps its own copy of {}'s {session}",
                    vendor.name
                );
            }
        }
    }

    #[test]
    fn adopt_takes_the_session_from_whichever_vendor_named_it() {
        // The vendor being taken over is the one that started this command,
        // not the one the config would spawn, so the table is read rather than
        // a name being asked after. Every vendor that can be taken over names
        // the variable that makes it possible.
        for vendor in registry::entries() {
            if !vendor.can(Capability::Adopt) {
                continue;
            }
            let named = vendor.session_env.expect("a vendor that can be adopted");
            let env = BTreeMap::from([(named.to_string(), "abc-123".to_string())]);
            let (found, session) = session_in(registry::entries(), &env).expect("the session");
            assert_eq!(found.name, vendor.name);
            assert_eq!(session, "abc-123");
        }

        // A vendor that names a session amx cannot take over is refused, in
        // words saying which of the two is missing.
        let cannot = Vendor {
            capabilities: &[Capability::Resume],
            ..SECOND
        };
        let env = BTreeMap::from([(
            cannot.session_env.unwrap().to_string(),
            "abc-123".to_string(),
        )]);
        let said = format!("{:#}", session_in(&[cannot], &env).unwrap_err());
        assert!(said.contains(SECOND.name), "{said}");
        assert!(said.contains("cannot be taken over"), "{said}");
    }

    #[test]
    fn adopt_needs_the_pane_and_the_conversation_it_is_typed_in() {
        let root = TempDir::new().unwrap();
        let pane = APane::showing(&A_PERMISSION_BOX);

        // Not in tmux at all, and in tmux with no claude around it.
        let outside = BTreeMap::from([(session_env().to_string(), "abc-123".to_string())]);
        let said = format!(
            "{:#}",
            adopt(root.path(), &pane, &outside, &AdoptArgs::default()).unwrap_err()
        );
        assert!(said.contains(PANE_ENV), "{said}");
        assert!(
            said.contains("inside a tmux pane"),
            "the refusal names the limitation rather than the variable alone: {said}"
        );
        assert!(
            said.contains("watch"),
            "and why: a pane is the only thing amx can watch and type at: {said}"
        );

        let shell = BTreeMap::from([(PANE_ENV.to_string(), pane.pane.to_string())]);
        let said = format!(
            "{:#}",
            adopt(root.path(), &pane, &shell, &AdoptArgs::default()).unwrap_err()
        );
        assert!(said.contains(session_env()), "{said}");
        assert!(
            said.contains(a_vendor().name),
            "the refusal names the vendor amx looked for: {said}"
        );

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
        assert_eq!(label(Path::new("/srv/app"), a_vendor()), "adopted app");
        assert_eq!(
            ids::stem_from_task(&label(Path::new("/srv/app"), a_vendor())),
            "adopted-app"
        );

        // A directory with no name to it leaves the vendor to say what the row
        // is, and that is the vendor's own word.
        assert_eq!(label(Path::new("/"), &SECOND), "adopted second");
    }
}
