//! `amx adopt`: write a record for an agent amx did not start.
//!
//! The command is typed inside the agent's own pane. Nothing is started or
//! sent; the agent carries on. amx records no worktree, branch, base or launch
//! command for it, so `stop` takes only the pane and there is nothing for
//! `resume` or `fork` to restart.
//!
//! - The pane comes from `$TMUX_PANE`.
//! - The vendor comes from the program tmux says the pane is running. A
//!   session variable can be inherited from an outer agent, so it only names
//!   which of that vendor's conversations this is. A program with no registry
//!   entry falls back to the first session variable present.
//! - The recorded session is how later hook events, which carry no amx id,
//!   are matched to this record.
//! - The id is stamped on the pane as `@amx-id`; every later reading depends on
//!   it (see [`crate::tmux::Server::pane_owners`]). If the stamp cannot be
//!   written the adoption fails. A pane adopted by an older amx has no stamp and
//!   reads as gone until adopted again.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::cli::AdoptArgs;
use crate::rules::{Claim, Ruleset};
use crate::store::{Agent, Event, Meta, Phase, State, now};
use crate::tmux::{PaneId, PaneOwners, Server, Socket};
use crate::vendor::{Capability, Vendor};
use crate::verbs::new;
use crate::{exit, paths, registry, rules, spawn};

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
    run(&root, &server, &env, &here, args, &mut out)
}

/// The verb, with everything it reads named.
pub fn run(
    root: &Path,
    server: &Server,
    env: &BTreeMap<String, String>,
    here: &Path,
    args: &AdoptArgs,
    out: &mut impl Write,
) -> Result<i32> {
    // Every refusal comes before anything is written. An amx id in the
    // environment only counts if its record still exists.
    if let Some(id) = env.get(crate::hook::ID_ENV)
        && Agent::open(root, id).is_ok()
    {
        bail!("this pane is agent `{id}` already, which amx started");
    }

    let pane = this_pane(env)?;
    if !server.pane_alive(&pane) {
        bail!("{pane} is not a pane on the tmux server this is running on");
    }
    let (vendor, session) = this_session(server, &pane, env)?;
    // Checking for an existing record and writing one happen under one lock,
    // so two concurrent adopts cannot both succeed.
    let _held = hold(root)?;
    let owners = server.pane_owners()?;
    if let Some(refusal) = spoken_for(root, server.socket(), &owners, &pane, &session)? {
        bail!(refusal);
    }

    let dir = pane_dir(server, &pane, here);
    let task = match &args.task {
        Some(task) => task.clone(),
        None => label(&dir, vendor),
    };
    // Captured before claiming an id, so an unreadable pane leaves the state
    // root untouched.
    let screen = server
        .capture(&pane)
        .with_context(|| format!("reading what is on {pane}"))?;

    let (id, claimed) = new::claim(root, args.name.as_deref(), &task)?;
    let meta = Meta {
        role: None,
        parent: None,
        depth: 0,
        id: id.clone(),
        task,
        // The vendor in the pane, not the configured one.
        agent: Some(vendor.name.to_string()),
        // Launch dials are unknown for an agent amx did not start.
        model: None,
        effort: None,
        dir,
        // Never claim the person's tree: `amx stop` would remove it.
        worktree: None,
        branch: None,
        base: None,
        socket: server.socket().clone(),
        pane: pane.clone(),
        bg: false,
        session: Some(session.clone()),
        // Filled in from the next hook payload, which names the transcript.
        transcript: None,
        created: now(),
    };
    // A failed record or stamp removes the claimed directory.
    let agent = match record(root, server, &meta) {
        Ok(agent) => agent,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&claimed);
            return Err(e);
        }
    };

    // Failures from here on are not undone: the record already names a live
    // pane, and the next reading repeats the seed.
    let writer = agent.writer()?;
    writer.append(&Event::new(
        ADOPTED,
        serde_json::json!({
            "pane": pane.as_str(),
            "session": session,
            "vendor": vendor.name,
        }),
    ))?;
    // Seeded with the ruleset of the vendor in this pane.
    writer.update_state(|state| seed(state, rules::of(vendor.name), &screen))?;
    drop(writer);

    writeln!(out, "{id}")?;
    Ok(exit::OK)
}

/// Lock the agents root against other adopts until the lock is dropped.
fn hold(root: &Path) -> Result<nix::fcntl::Flock<std::fs::File>> {
    std::fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
    let dir = std::fs::File::open(root).with_context(|| format!("opening {}", root.display()))?;
    nix::fcntl::Flock::lock(dir, nix::fcntl::FlockArg::LockExclusive)
        .map_err(|(_, errno)| errno)
        .with_context(|| format!("locking {}", root.display()))
}

/// Write `meta`, then stamp its id on the pane.
///
/// An adopted pane sits in the person's own session, so the stamp is the only
/// thing that says whose it is. Without it every reader calls the agent gone,
/// so a failed stamp fails the adoption.
fn record(root: &Path, server: &Server, meta: &Meta) -> Result<Agent> {
    let agent = Agent::create(root, meta)?;
    server
        .set_pane_option(&meta.pane, crate::tmux::ID_OPTION, &meta.id)
        .with_context(|| format!("writing `{}` on {}", meta.id, meta.pane))?;
    Ok(agent)
}

/// Seed the record's phase and question from the pane's screen.
///
/// An adopted agent may be mid-turn, and readers trust a fresh record, so
/// `starting` would be believed. The screen is read the way a reader would
/// read it.
fn seed(state: &mut State, rules: &Ruleset, screen: &str) {
    let (phase, asking) = match rules.claim(screen, Phase::Starting, 1) {
        Claim::Ruled(rule) => (rule.state, rule.question(screen)),
        // No rule claims the screen.
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
            "no ${PANE_ENV} here: `amx adopt` needs the agent to be running \
             inside a tmux pane, because a pane is the only thing amx can \
             watch and type at"
        );
    };
    PaneId::new(pane.clone()).with_context(|| format!("${PANE_ENV} holds {pane:?}"))
}

/// The vendor running in `pane` and the session it names in `env`.
fn this_session(
    server: &Server,
    pane: &PaneId,
    env: &BTreeMap<String, String>,
) -> Result<(&'static Vendor, String)> {
    session_in(
        registry::entries(),
        running_in(server, pane).as_deref(),
        env,
    )
}

/// [`this_session`] against a given vendor table and pane program.
///
/// A known program picks the vendor, and only that vendor's session variable
/// is read. An unknown program (a shell, an unlisted tool, or no answer from
/// tmux) falls back to the first vendor whose variable is set.
fn session_in<'v>(
    vendors: &'v [Vendor],
    program: Option<&str>,
    env: &BTreeMap<String, String>,
) -> Result<(&'v Vendor, String)> {
    match program.and_then(|program| vendors.iter().find(|vendor| vendor.name == program)) {
        Some(vendor) => in_this_pane(vendor, env),
        None => in_the_environment(vendors, env),
    }
}

/// The session `vendor` names in `env`, refusing a vendor that cannot be
/// adopted.
fn in_this_pane<'v>(
    vendor: &'v Vendor,
    env: &BTreeMap<String, String>,
) -> Result<(&'v Vendor, String)> {
    // Without the capability or a session variable, no hook could ever be
    // matched to the record. The table guarantees every adoptable vendor
    // names a session variable.
    let Some(named) = vendor.session_env.filter(|_| vendor.can(Capability::Adopt)) else {
        bail!(
            "tmux says a {} is running in this pane, and a {} cannot be taken \
             over: amx would have a record here and no way to hear from it",
            vendor.name,
            vendor.name
        );
    };
    let Some(session) = env.get(named).filter(|id| !id.is_empty()) else {
        // No fallback to another vendor's variable: that session id would
        // never match this agent's hooks.
        bail!(
            "tmux says a {} is running in this pane and there is no ${named} \
             here, so amx cannot tell which {} conversation this is. `amx \
             adopt` is run inside the agent it adopts, and that session id is \
             how its events are recognised afterwards",
            vendor.name,
            vendor.name
        );
    };
    Ok((vendor, session.clone()))
}

/// The first vendor whose session variable is set, for a pane running an
/// unknown program.
fn in_the_environment<'v>(
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

/// The vendors with the `Adopt` capability.
fn adoptable(vendors: &[Vendor]) -> impl Iterator<Item = &Vendor> {
    vendors
        .iter()
        .filter(|vendor| vendor.can(Capability::Adopt))
}

/// Join items for a sentence: `a`, `a or b`, `a, b or c`.
fn either(each: &[impl AsRef<str>]) -> String {
    let each: Vec<&str> = each.iter().map(AsRef::as_ref).collect();
    match each.split_last() {
        Some((last, [])) => (*last).to_string(),
        Some((last, before)) => format!("{} or {last}", before.join(", ")),
        // Unreachable: an empty adoptable list is refused earlier.
        None => "nothing".to_string(),
    }
}

/// The refusal when a live record already owns this pane or this session.
///
/// Ended records are ignored. So is a live record naming this pane number that
/// the pane does not answer for: a dead tmux server leaves such records behind,
/// and pane numbers are reused.
fn spoken_for(
    root: &Path,
    socket: &Socket,
    owners: &PaneOwners,
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
        if &meta.pane == pane && &meta.socket == socket && owners.pane_answers_for(pane, &id) {
            return Ok(Some(format!("{pane} is agent `{id}` already")));
        }
        if meta.session.as_deref() == Some(session) {
            return Ok(Some(format!("this conversation is agent `{id}` already")));
        }
    }
    Ok(None)
}

/// The program tmux says the pane is running, or `None` if it will not say.
fn running_in(server: &Server, pane: &PaneId) -> Option<String> {
    server
        .pane_field(pane, "#{pane_current_command}")
        .ok()
        .map(|program| program.trim().to_string())
        .filter(|program| !program.is_empty())
}

/// The pane's current directory, falling back to `here` (where this command
/// runs, inside the same pane).
fn pane_dir(server: &Server, pane: &PaneId, here: &Path) -> PathBuf {
    server
        .pane_field(pane, "#{pane_current_path}")
        .ok()
        .map(|path| PathBuf::from(path.trim()))
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| here.to_path_buf())
}

/// The default task for an adopted agent: `adopted <dir name>`, or
/// `adopted <vendor>` for a directory with no name.
fn label(dir: &Path, vendor: &Vendor) -> String {
    match dir.file_name().and_then(|name| name.to_str()) {
        Some(name) => format!("adopted {name}"),
        None => format!("adopted {}", vendor.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids;
    use crate::tmux::Spawn;
    use crate::vendor::second::SECOND;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    /// The first adoptable vendor in the table.
    fn a_vendor() -> &'static Vendor {
        registry::entries()
            .iter()
            .find(|vendor| vendor.can(Capability::Adopt))
            .expect("a vendor amx can take over")
    }

    /// That vendor's session variable.
    fn session_env() -> &'static str {
        a_vendor()
            .session_env
            .expect("a vendor that can be adopted names its session")
    }

    /// A tmux socket name unique to one test.
    fn tag() -> String {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        format!(
            "amx-test-adopt-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// A real pane on a private tmux server, painted with a vendor's rows.
    /// Killed on drop.
    struct APane {
        server: Server,
        pane: PaneId,
        /// Holds the symlink the pane's program runs from.
        _program: TempDir,
    }

    impl APane {
        fn showing(rows: &[&str]) -> APane {
            APane::running("sh", rows)
        }

        /// The same pane, with its process running under `program`'s name.
        ///
        /// tmux reports a pane's program by name, so a symlink to `/bin/sh`
        /// under that name stands in for the vendor.
        fn running(program: &str, rows: &[&str]) -> APane {
            let dir = TempDir::new().unwrap();
            let started = dir.path().join(program);
            std::os::unix::fs::symlink("/bin/sh", &started).expect("a shell under that name");
            let started = started.to_string_lossy().into_owned();

            // An empty conf keeps the developer's ~/.tmux.conf out of the test.
            let server = Server::named(tag()).with_conf("/dev/null");
            let painted: Vec<String> = rows.iter().map(|row| format!("'{row}'")).collect();
            let script = format!(
                "printf '%s\\n' {}; while :; do sleep 0.05; done",
                painted.join(" ")
            );
            let (_, pane) = server
                .new_session(&Spawn {
                    command: &[&started, "-c", &script],
                    ..Spawn::default()
                })
                .expect("a pane to adopt");

            // Wait for the shell to draw the rows.
            for _ in 0..200 {
                if server
                    .capture(&pane)
                    .is_ok_and(|screen| screen.contains(rows[rows.len() - 1]))
                {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            APane {
                server,
                pane,
                _program: dir,
            }
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

    /// claude's permission box.
    const A_PERMISSION_BOX: [&str; 7] = [
        " Bash command",
        "   rm -f b.txt",
        " Permission rule Bash requires confirmation for this command.",
        " Do you want to proceed?",
        " ❯ 1. Yes",
        "   2. No",
        " Esc to cancel · Tab to amend · ctrl+e to explain",
    ];

    /// Run the verb, capturing stdout.
    fn adopt(
        root: &Path,
        pane: &APane,
        env: &BTreeMap<String, String>,
        args: &AdoptArgs,
    ) -> Result<(i32, String)> {
        let mut out = Vec::new();
        let code = run(
            root,
            &pane.server,
            env,
            Path::new("/srv/app"),
            args,
            &mut out,
        )?;
        Ok((code, String::from_utf8(out).unwrap()))
    }

    /// A session id for `vendor`.
    fn a_session(vendor: &Vendor) -> String {
        format!("{}-abc-123", vendor.name)
    }

    /// Every adoptable vendor's session variable, each set to its own session.
    fn every_session() -> BTreeMap<String, String> {
        adoptable(registry::entries())
            .map(|vendor| {
                (
                    vendor
                        .session_env
                        .expect("a vendor that can be adopted")
                        .to_string(),
                    a_session(vendor),
                )
            })
            .collect()
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

        // The screen is read at adoption: a fresh `starting` record would be
        // believed.
        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Waiting);
        assert_eq!(state.question.as_deref(), Some("Do you want to proceed?"));
        assert_eq!(state.options, ["Yes", "No"]);

        // The log opens with the adoption event.
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

        // A given task and name are used as is.
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
        // Session variable names live in the vendor table; a copy here would
        // go stale.
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
        // For a pane running an unknown program, the environment alone picks
        // the vendor.
        for vendor in registry::entries() {
            if !vendor.can(Capability::Adopt) {
                continue;
            }
            let named = vendor.session_env.expect("a vendor that can be adopted");
            let env = BTreeMap::from([(named.to_string(), "abc-123".to_string())]);
            let (found, session) =
                session_in(registry::entries(), Some("sh"), &env).expect("the session");
            assert_eq!(found.name, vendor.name);
            assert_eq!(session, "abc-123");
        }

        // A vendor without the Adopt capability is refused.
        let cannot = Vendor {
            capabilities: &[Capability::Resume],
            ..SECOND
        };
        let env = BTreeMap::from([(
            cannot.session_env.unwrap().to_string(),
            "abc-123".to_string(),
        )]);
        let said = format!("{:#}", session_in(&[cannot], Some("sh"), &env).unwrap_err());
        assert!(said.contains(SECOND.name), "{said}");
        assert!(said.contains("cannot be taken over"), "{said}");
    }

    #[test]
    fn adopt_takes_the_vendor_from_the_program_that_is_in_the_pane() {
        // An agent started inside another inherits both vendors' variables.
        // The pane's program picks the vendor.
        let both = every_session();
        for vendor in adoptable(registry::entries()) {
            let (found, session) =
                session_in(registry::entries(), Some(vendor.name), &both).expect("the session");
            assert_eq!(found.name, vendor.name);
            assert_eq!(session, a_session(vendor));
        }

        // An unknown program, or no answer from tmux, falls back to the first
        // variable set.
        let first = adoptable(registry::entries())
            .next()
            .expect("a vendor amx can take over");
        for read in [Some("sh"), None] {
            let (found, _) = session_in(registry::entries(), read, &both).expect("the session");
            assert_eq!(found.name, first.name, "read as {read:?}");
        }
    }

    #[test]
    fn adopt_refuses_a_pane_and_an_environment_that_agree_on_nothing() {
        // The pane runs one vendor and only other vendors' session variables
        // are set.
        for vendor in adoptable(registry::entries()) {
            let mut others = every_session();
            others.remove(vendor.session_env.expect("a vendor that can be adopted"));

            let said = format!(
                "{:#}",
                session_in(registry::entries(), Some(vendor.name), &others).unwrap_err()
            );
            assert!(
                said.contains(vendor.name),
                "the refusal names what was in the pane: {said}"
            );
            assert!(
                said.contains(vendor.session_env.unwrap()),
                "and the variable that would have said which conversation: {said}"
            );
        }

        // A pane running a vendor that cannot be adopted is refused on its
        // program.
        let cannot = Vendor {
            capabilities: &[Capability::Resume],
            ..SECOND
        };
        let env = BTreeMap::from([(
            cannot.session_env.unwrap().to_string(),
            "abc-123".to_string(),
        )]);
        let said = format!(
            "{:#}",
            session_in(&[cannot], Some(SECOND.name), &env).unwrap_err()
        );
        assert!(said.contains(SECOND.name), "{said}");
        assert!(said.contains("cannot be taken over"), "{said}");
    }

    #[test]
    fn adopt_writes_the_record_of_the_vendor_the_pane_was_running() {
        // The whole verb, in panes whose environment also carries other
        // vendors' session ids.
        let root = TempDir::new().unwrap();
        let both = every_session();
        for vendor in adoptable(registry::entries()) {
            let pane = APane::running(vendor.name, &A_PERMISSION_BOX);
            let mut env = both.clone();
            env.insert(PANE_ENV.to_string(), pane.pane.to_string());

            let (code, printed) = adopt(root.path(), &pane, &env, &AdoptArgs::default()).unwrap();
            assert_eq!(code, exit::OK);

            let meta = Agent::open(root.path(), printed.trim())
                .expect("a record for the adopted agent")
                .meta()
                .unwrap();
            assert_eq!(meta.agent.as_deref(), Some(vendor.name));
            assert_eq!(
                meta.session.as_deref(),
                Some(a_session(vendor).as_str()),
                "the conversation this vendor named, and not another vendor's"
            );
        }
    }

    #[test]
    fn adopt_needs_the_pane_and_the_conversation_it_is_typed_in() {
        let root = TempDir::new().unwrap();
        let pane = APane::showing(&A_PERMISSION_BOX);

        // Outside tmux, then inside tmux with no session variable.
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
        assert!(
            registry::entries()
                .iter()
                .all(|vendor| !said.contains(&format!("the {}", vendor.name))),
            "no pane, so no vendor in hand for the refusal to name: {said}"
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

        // The same pane again: two records would drive one agent.
        let said = format!(
            "{:#}",
            adopt(root.path(), &pane, &env, &AdoptArgs::default()).unwrap_err()
        );
        assert!(said.contains(&first), "{said}");
        assert!(said.contains("already"), "{said}");

        // The same session in another pane, as when resumed by hand.
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

        // A pane amx started carries its id in the environment.
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

    /// A live record naming this pane, as a dead tmux server leaves behind.
    fn a_record_naming(root: &Path, id: &str, pane: &APane) {
        Agent::create(
            root,
            &Meta {
                role: None,
                parent: None,
                depth: 0,
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                agent: Some(a_vendor().name.to_string()),
                model: None,
                effort: None,
                dir: PathBuf::from("/srv/app"),
                worktree: None,
                branch: None,
                base: None,
                socket: pane.server.socket().clone(),
                pane: pane.pane.clone(),
                bg: false,
                session: Some("some-other-conversation".to_string()),
                transcript: None,
                created: 1,
            },
        )
        .expect("a record");
    }

    #[test]
    fn adopt_writes_the_id_on_the_pane_it_takes_over() {
        // The stamp is the only link from an adopted pane to its record.
        let root = TempDir::new().unwrap();
        let pane = APane::showing(&A_PERMISSION_BOX);

        let (_, printed) = adopt(
            root.path(),
            &pane,
            &pane.env("abc-123"),
            &AdoptArgs::default(),
        )
        .unwrap();
        let id = printed.trim();

        assert_eq!(
            pane.server
                .pane_option(&pane.pane, crate::tmux::ID_OPTION)
                .unwrap()
                .as_deref(),
            Some(id)
        );
        assert!(pane.server.pane_answers_for(&pane.pane, id));
        assert!(
            !pane
                .server
                .pane_answers_for(&pane.pane, "somebody-else-b2c"),
            "and for nobody else, whatever pane number they were recorded with"
        );
    }

    #[test]
    fn adopt_stands_aside_for_a_record_that_has_lost_the_pane_it_names() {
        // A live record from a dead server names this pane number, but the
        // pane does not answer for it.
        let root = TempDir::new().unwrap();
        let pane = APane::showing(&A_PERMISSION_BOX);
        a_record_naming(root.path(), "yesterday-a1b", &pane);

        let (code, printed) = adopt(
            root.path(),
            &pane,
            &pane.env("abc-123"),
            &AdoptArgs::default(),
        )
        .unwrap();
        assert_eq!(code, exit::OK);
        let id = printed.trim();
        assert_ne!(id, "yesterday-a1b");
        assert!(pane.server.pane_answers_for(&pane.pane, id));
        assert!(
            !pane.server.pane_answers_for(&pane.pane, "yesterday-a1b"),
            "the older record holds nothing here"
        );

        // A record the pane does answer for still blocks.
        let said = format!(
            "{:#}",
            adopt(
                root.path(),
                &pane,
                &pane.env("def-456"),
                &AdoptArgs::default()
            )
            .unwrap_err()
        );
        assert!(said.contains(id), "{said}");
        assert!(said.contains("already"), "{said}");
    }

    #[test]
    fn adopt_stands_aside_for_a_record_that_has_ended() {
        // Ended records do not block; pane ids are reused.
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

        // A directory with no name falls back to the vendor's name.
        assert_eq!(label(Path::new("/"), &SECOND), "adopted second");
    }

    #[test]
    fn adopt_twice_at_once_leaves_one_record() {
        // Without the lock both would check before either wrote.
        let root = TempDir::new().unwrap();
        let pane = APane::showing(&A_PERMISSION_BOX);
        let env = pane.env("abc-123");

        let start = std::sync::Barrier::new(2);
        let taken: Vec<bool> = std::thread::scope(|s| {
            let each: Vec<_> = (0..2)
                .map(|_| {
                    s.spawn(|| {
                        start.wait();
                        adopt(root.path(), &pane, &env, &AdoptArgs::default()).is_ok()
                    })
                })
                .collect();
            each.into_iter().map(|one| one.join().unwrap()).collect()
        });

        assert_eq!(taken.iter().filter(|took| **took).count(), 1, "{taken:?}");
        let ids = crate::store::list(root.path()).unwrap();
        assert_eq!(ids.len(), 1, "{ids:?}");
        assert!(pane.server.pane_answers_for(&pane.pane, &ids[0]));
    }

    #[test]
    fn adopt_leaves_the_pane_unstamped_when_the_record_cannot_be_written() {
        let root = TempDir::new().unwrap();
        let pane = APane::showing(&A_PERMISSION_BOX);
        std::fs::set_permissions(
            root.path(),
            std::os::unix::fs::PermissionsExt::from_mode(0o500),
        )
        .unwrap();

        let refused = adopt(
            root.path(),
            &pane,
            &pane.env("abc-123"),
            &AdoptArgs::default(),
        );
        std::fs::set_permissions(
            root.path(),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();

        assert!(refused.is_err());
        assert_eq!(
            pane.server
                .pane_option(&pane.pane, crate::tmux::ID_OPTION)
                .unwrap(),
            None
        );
    }

    #[test]
    fn adopt_refuses_a_taken_name_before_it_touches_the_pane() {
        let root = TempDir::new().unwrap();
        let pane = APane::showing(&A_PERMISSION_BOX);
        let elsewhere = APane::showing(&A_PERMISSION_BOX);
        a_record_naming(root.path(), "login", &elsewhere);

        let said = format!(
            "{:#}",
            adopt(
                root.path(),
                &pane,
                &pane.env("abc-123"),
                &AdoptArgs {
                    name: Some("login".to_string()),
                    ..AdoptArgs::default()
                },
            )
            .unwrap_err()
        );

        assert!(said.contains("taken"), "{said}");
        assert_eq!(
            pane.server
                .pane_option(&pane.pane, crate::tmux::ID_OPTION)
                .unwrap(),
            None
        );
        let meta = Agent::open(root.path(), "login").unwrap().meta().unwrap();
        assert_eq!(
            meta.pane, elsewhere.pane,
            "and the record is left as it was"
        );
    }
}
