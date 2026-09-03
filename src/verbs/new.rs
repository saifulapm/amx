//! `amx new` — start an agent on a task.
//!
//! The order matters and is the same every time: mint an id, cut a worktree,
//! write the handoff, start the pane, then write the record. The pane waits
//! for the record before it starts the vendor, so the vendor's first hook
//! always has somewhere to go, and nothing but the id is ever printed — a
//! caller reads that id straight into its next command.

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::cli::NewArgs;
use crate::config::Config;
use crate::spawn::{self, Dials, Handoff};
use crate::store::{Meta, now};
use crate::{Severity, exit, ids, paths, registry, said, trust, worktree};

/// What this spawn launches: the vendor's command, and where its dials are
/// pointed for this one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Launch {
    agent: String,
    dials: Dials,
}

impl Launch {
    /// What the caller typed, else what the config holds, else the vendor's
    /// own behaviour, dial by dial.
    ///
    /// A value the vendor would not take is refused rather than passed on.
    /// The config is treated more gently and drops such a value instead: a
    /// file outlives the versions that wrote it, and it has already said so
    /// on its own terms when it was read. A flag was typed for this spawn,
    /// with the person who typed it still standing there, so telling them
    /// beats starting an agent at a setting nobody asked for.
    fn resolve(config: &Config, args: &NewArgs) -> Result<Launch, String> {
        let named = args.agent.as_ref();
        let agent = named
            .and_then(|named| named.command.clone())
            .unwrap_or_else(|| config.agent.clone());
        let entry = registry::entry(&agent);
        let mut dials = Dials::default();

        for (key, spec, typed, held, resolved) in [
            (
                "model",
                entry.and_then(|entry| entry.model),
                named.and_then(|named| named.model.as_deref()),
                config.model.as_deref(),
                &mut dials.model,
            ),
            (
                "permission",
                entry.and_then(|entry| entry.permission),
                named.and_then(|named| named.permission.as_deref()),
                config.permission.as_deref(),
                &mut dials.permission,
            ),
            (
                "effort",
                entry.and_then(|entry| entry.effort),
                named.and_then(|named| named.effort.as_deref()),
                config.effort.as_deref(),
                &mut dials.effort,
            ),
        ] {
            if let Some(value) = typed {
                match spec {
                    Some(spec) if registry::accepts(&spec, value) => *resolved = value.to_string(),
                    // Only a closed dial ever refuses a value, so its cycle is
                    // the whole of what the vendor takes and worth printing.
                    Some(spec) => {
                        return Err(format!(
                            "--{key} {value:?}: {} takes {}",
                            registry::program(&agent),
                            spec.cycle.join(", ")
                        ));
                    }
                    None => {
                        return Err(format!(
                            "--{key} {value:?}: amx knows no {key} dial for {}",
                            registry::program(&agent)
                        ));
                    }
                }
                continue;
            }
            if let Some(value) = held
                && spec.is_some_and(|spec| registry::accepts(&spec, value))
            {
                *resolved = value.to_string();
            }
        }

        Ok(Launch { agent, dials })
    }
}

/// Run the verb against the machine.
pub fn from_env(config: &Config, args: &NewArgs) -> Result<i32> {
    let root = paths::state_root()?;
    let dir = match &args.dir {
        Some(dir) => dir.clone(),
        None => std::env::current_dir().context("no working directory")?,
    };
    let env = spawn::env_snapshot(std::env::vars());
    let mut out = std::io::stdout().lock();
    let to_terminal = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let mut problems = std::io::stderr().lock();

    run_aloud(
        &root,
        &dir,
        env,
        config,
        args,
        &mut out,
        &mut problems,
        to_terminal,
    )
}

/// The verb, with everything it reads named and its refusals in the words
/// alone.
///
/// The view spawns through here, and what it does with a refusal is put it in
/// the notice at the foot of its own screen, in the colour that band paints its
/// notices. Paint amx wrote would be paint the view has to take back out.
#[allow(clippy::too_many_arguments)]
pub fn run(
    root: &Path,
    dir: &Path,
    env: std::collections::BTreeMap<String, String>,
    config: &Config,
    args: &NewArgs,
    out: &mut impl Write,
    problems: &mut impl Write,
) -> Result<i32> {
    run_aloud(root, dir, env, config, args, out, problems, false)
}

/// The same, told to a stderr that is a terminal and wants the colour.
#[allow(clippy::too_many_arguments)]
fn run_aloud(
    root: &Path,
    dir: &Path,
    env: std::collections::BTreeMap<String, String>,
    config: &Config,
    args: &NewArgs,
    out: &mut impl Write,
    problems: &mut impl Write,
    to_terminal: bool,
) -> Result<i32> {
    // Before anything is made: a dial the vendor would not take is a
    // malformed command line, and there is nothing to clean up if it is
    // answered here.
    let launch = match Launch::resolve(config, args) {
        Ok(launch) => launch,
        Err(refusal) => {
            writeln!(
                problems,
                "{}",
                said(
                    Severity::Warned,
                    &format!("amx new: {refusal}"),
                    to_terminal
                )
            )?;
            return Ok(exit::USAGE);
        }
    };

    std::fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;

    // The cap is about agents that are still going. One that has finished is a
    // record, not a running program.
    let live = spawn::live(root)?;
    if live.len() >= config.max_agents {
        writeln!(
            problems,
            "{}",
            said(
                Severity::Warned,
                &format!(
                    "amx new: {} agents already running, and max_agents is {}",
                    live.len(),
                    config.max_agents
                ),
                to_terminal
            )
        )?;
        return Ok(exit::BLOCKED);
    }

    let (id, agent_dir) = claim(root, args)?;

    // From here on a failure leaves nothing behind: an id that half exists is
    // worse than one that does not. The directory is this spawn's own — the
    // claim made it — so removing it can never take another spawn's record.
    match start(
        root,
        &agent_dir,
        dir,
        env,
        config,
        args,
        &launch,
        &id,
        problems,
        to_terminal,
    ) {
        Ok(()) => {
            writeln!(out, "{id}")?;
            Ok(exit::OK)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&agent_dir);
            Err(e)
        }
    }
}

/// How many minted ids to try to claim before giving up.
const MAX_CLAIMS: usize = 8;

/// Claim an id by making its directory. The mkdir is the uniqueness check:
/// two spawns in flight can both believe a name is free, but the directory
/// can only be made by one of them, and nothing the loser has to clean up
/// exists yet.
fn claim(root: &Path, args: &NewArgs) -> Result<(String, PathBuf)> {
    if let Some(name) = &args.name {
        ids::validate_name(name, root)?;
        let dir = paths::agent_dir_in(root, name)?;
        // A typed name that loses the claim was taken, however recently.
        if !make_dir(&dir)? {
            bail!("name {name:?} is already taken");
        }
        return Ok((name.clone(), dir));
    }
    // generate already avoids every directory that exists, so losing a draw
    // to a spawn in flight is next to never — and answered with another draw.
    for _ in 0..MAX_CLAIMS {
        let id = ids::generate(&args.task, root)?;
        let dir = paths::agent_dir_in(root, &id)?;
        if make_dir(&dir)? {
            return Ok((id, dir));
        }
    }
    bail!(
        "no id for {:?} could be claimed under {} after {MAX_CLAIMS} draws",
        args.task,
        root.display()
    )
}

#[allow(clippy::too_many_arguments)]
fn start(
    root: &Path,
    agent_dir: &Path,
    dir: &Path,
    mut env: std::collections::BTreeMap<String, String>,
    config: &Config,
    args: &NewArgs,
    launch: &Launch,
    id: &str,
    problems: &mut impl Write,
    to_terminal: bool,
) -> Result<()> {
    let tree = cut_worktree(dir, id, config, args)?;
    let cwd = tree
        .as_ref()
        .map(|tree| tree.path.clone())
        .unwrap_or_else(|| dir.to_path_buf());
    if let Some(tree) = &tree {
        trust_the_tree(
            config,
            &env,
            &launch.agent,
            &tree.path,
            problems,
            to_terminal,
        );
    }

    env.insert(crate::hook::ID_ENV.to_string(), id.to_string());
    spawn::write_boot_env(agent_dir, &env)?;
    spawn::write_handoff(
        agent_dir,
        &Handoff {
            task: args.task.clone(),
            command: launched(args, launch, id),
        },
    )?;

    let server = spawn::server()?;
    let boot = vec![
        std::env::current_exe()?.to_string_lossy().into_owned(),
        "_boot".to_string(),
        id.to_string(),
    ];
    let pane = spawn::place(&server, id, &cwd, &boot)?;

    // A vendor that opens under the id amx offered it is recorded here,
    // rather than left `None` for a Started hook that a vendor with no hooks
    // at all could never send.
    let session = (!args.exec && spawn::opens_under_id(&launch.agent, &args.vendor_args))
        .then(|| id.to_string());

    spawn::record(
        root,
        &Meta {
            id: id.to_string(),
            task: args.task.clone(),
            dir: cwd,
            worktree: tree.as_ref().map(|tree| tree.path.clone()),
            branch: tree.as_ref().map(|tree| tree.branch.clone()),
            base: tree.as_ref().map(|tree| tree.base.clone()),
            socket: server.socket().clone(),
            pane,
            // Nothing is out of sight any more: an agent is a session nobody
            // is attached to until somebody looks in on it.
            bg: false,
            session,
            transcript: None,
            created: now(),
        },
    )?;
    Ok(())
}

/// What the pane runs: a shell command when that is what was asked for, else
/// the vendor with the task after it.
///
/// A command spawn has no vendor and no dials — the command line refuses them
/// beside `--exec` — so nothing about the launch is resolved for it. What was
/// typed is what runs.
///
/// `id` rides along as the session a vendor that declares a start flag is
/// asked to open under; a vendor with no such flag is unaffected by it.
fn launched(args: &NewArgs, launch: &Launch, id: &str) -> Vec<String> {
    match args.exec {
        true => spawn::exec_command(&args.task),
        false => spawn::vendor_command(
            &launch.agent,
            &launch.dials,
            &args.vendor_args,
            &args.task,
            Some(id),
        ),
    }
}

/// A worktree of its own, when the agent is being sent into a repository and
/// nobody has said not to.
///
/// Never for a command. A worktree is there to keep one conversation's work
/// apart from another's, and a command has no conversation: it was typed to
/// run *here*, against this checkout and whatever is already built in it.
fn cut_worktree(
    dir: &Path,
    id: &str,
    config: &Config,
    args: &NewArgs,
) -> Result<Option<worktree::Worktree>> {
    if args.exec || args.no_worktree || !config.worktrees {
        return Ok(None);
    }
    if !dir.is_dir() {
        bail!("{} is not a directory to run in", dir.display());
    }
    // Somewhere that is not a repository is somewhere to work in as it is.
    let Some(repo) = worktree::repo_root(dir)? else {
        return Ok(None);
    };
    Ok(Some(worktree::create(&repo, id)?))
}

/// Answer the vendor's folder-trust screen for the tree amx has just cut, so
/// that the agent starts on the task instead of on a question nobody has to
/// think about.
///
/// Never a reason to refuse the spawn. An agent that meets the screen is an
/// agent somebody answers by hand, which is exactly where amx stood before it
/// wrote anything at all, so a store amx cannot write is said once and the
/// spawn goes on.
fn trust_the_tree(
    config: &Config,
    env: &std::collections::BTreeMap<String, String>,
    agent: &str,
    tree: &Path,
    problems: &mut impl Write,
    to_terminal: bool,
) {
    // The store is the person's own file, and nothing is written to it until
    // they have said so once — `trust = true` in the config, the same consent
    // the hooks stand behind at doctor --fix. Until then the screen is theirs
    // to answer, and doctor points at the key.
    if !config.trust {
        return;
    }
    if !trust::is_vendor(agent) {
        return;
    }
    let Some(store) = trust::store_in(env) else {
        return;
    };
    // The vendor resolves a tree to the repository it belongs to, so that is
    // what already covers it when the person has trusted the repository.
    let inherits = worktree::main_repo(tree).ok();
    if let Err(e) = trust::seed(&store, tree, inherits.as_deref(), now()) {
        let _ = writeln!(
            problems,
            "{}",
            said(Severity::Warned, &format!("amx new: {e:#}"), to_terminal)
        );
    }
}

/// The agent's own directory, which nobody else has any business reading.
///
/// Deliberately not recursive: making the directory is the uniqueness claim,
/// so one that is already there has to answer false rather than stand in for
/// one this spawn made.
fn make_dir(dir: &Path) -> Result<bool> {
    use std::os::unix::fs::DirBuilderExt;
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e).with_context(|| format!("creating {}", dir.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::AgentArgs;
    use crate::registry::DEFAULT;

    fn spawn(agent: Option<&str>, dials: [Option<&str>; 3]) -> NewArgs {
        let [model, permission, effort] = dials;
        NewArgs {
            task: "port the importer".to_string(),
            name: None,
            dir: None,
            no_worktree: false,
            exec: false,
            agent: Some(AgentArgs {
                command: agent.map(str::to_string),
                model: model.map(str::to_string),
                permission: permission.map(str::to_string),
                effort: effort.map(str::to_string),
            }),
            vendor_args: Vec::new(),
        }
    }

    /// A spawn of a shell command, which names no vendor because it launches
    /// none.
    fn a_command(command: &str) -> NewArgs {
        NewArgs {
            task: command.to_string(),
            name: None,
            dir: None,
            no_worktree: false,
            exec: true,
            agent: None,
            vendor_args: Vec::new(),
        }
    }

    fn configured(model: Option<&str>, permission: Option<&str>, effort: Option<&str>) -> Config {
        Config {
            model: model.map(str::to_string),
            permission: permission.map(str::to_string),
            effort: effort.map(str::to_string),
            ..Config::default()
        }
    }

    #[test]
    fn dials_the_caller_beats_the_config_which_beats_the_vendors_own() {
        let config = configured(Some("fable"), Some("plan"), None);
        let launch =
            Launch::resolve(&config, &spawn(None, [Some("opus"), None, Some("high")])).unwrap();

        assert_eq!(launch.agent, "claude", "the configured vendor, unnamed");
        assert_eq!(launch.dials.model, "opus", "typed over configured");
        assert_eq!(launch.dials.permission, "plan", "configured, never typed");
        assert_eq!(launch.dials.effort, "high", "typed, never configured");
    }

    #[test]
    fn dials_untouched_by_either_are_left_to_the_vendor() {
        let launch = Launch::resolve(&Config::default(), &spawn(None, [None; 3])).unwrap();
        assert_eq!(launch.dials, Dials::default());
    }

    #[test]
    fn dials_can_be_turned_back_to_the_vendors_own_by_name() {
        // The only way to spawn once at whatever claude was going to do
        // anyway, without editing the config file first.
        let config = configured(Some("fable"), None, Some("max"));
        let launch = Launch::resolve(&config, &spawn(None, [Some(DEFAULT), None, None])).unwrap();

        assert_eq!(launch.dials.model, DEFAULT);
        assert_eq!(launch.dials.effort, "max", "and only that dial");
    }

    #[test]
    fn dials_the_vendor_would_refuse_are_refused_here_first() {
        // claude answers `--permission-mode nonsense` with an error naming the
        // modes it takes, so amx saying it is the same answer sooner, before
        // an id is minted or a pane is opened.
        let refusal = Launch::resolve(
            &Config::default(),
            &spawn(None, [None, Some("acceptedits"), None]),
        )
        .unwrap_err();
        assert!(refusal.contains("--permission"), "{refusal}");
        assert!(refusal.contains("acceptEdits"), "{refusal}");

        let refusal = Launch::resolve(&Config::default(), &spawn(None, [None, None, Some("hard")]))
            .unwrap_err();
        assert!(
            refusal.contains("--effort") && refusal.contains("xhigh"),
            "{refusal}"
        );
    }

    #[test]
    fn a_refusal_is_yellow_on_a_terminal_and_plain_down_a_pipe() {
        // A dial the vendor would refuse is answered before an id is minted or
        // a directory is made, so this reaches the writer with nothing behind
        // it — and the writer is the stderr the verb was handed.
        let dir = tempfile::TempDir::new().unwrap();
        let refused = |to_terminal| {
            let (mut out, mut problems) = (Vec::new(), Vec::new());
            let code = run_aloud(
                dir.path(),
                dir.path(),
                std::collections::BTreeMap::new(),
                &Config::default(),
                &spawn(None, [None, Some("acceptedits"), None]),
                &mut out,
                &mut problems,
                to_terminal,
            )
            .unwrap();
            assert_eq!(code, exit::USAGE);
            String::from_utf8(problems).unwrap()
        };

        let plain = refused(false);
        assert!(plain.starts_with("amx new: "), "{plain:?}");
        assert!(!plain.contains('\u{1b}'), "{plain:?}");

        // A refusal is amx working as it should, so it is yellow and not red.
        let painted = refused(true);
        assert!(painted.starts_with("\u{1b}[33mamx new: "), "{painted:?}");
        assert!(painted.trim_end().ends_with("\u{1b}[39m"), "{painted:?}");
        assert!(painted.contains("acceptEdits"), "{painted:?}");
    }

    #[test]
    fn dials_a_full_model_name_is_taken_because_that_dial_is_open() {
        let launch = Launch::resolve(
            &Config::default(),
            &spawn(None, [Some("claude-fable-5"), None, None]),
        )
        .unwrap();
        assert_eq!(launch.dials.model, "claude-fable-5");
    }

    #[test]
    fn dials_a_flag_for_a_vendor_that_has_no_such_dial_is_refused() {
        let refusal = Launch::resolve(
            &Config::default(),
            &spawn(Some("mock-claude"), [Some("opus"), None, None]),
        )
        .unwrap_err();
        assert!(refusal.contains("--model"), "{refusal}");
        assert!(refusal.contains("mock-claude"), "{refusal}");
    }

    #[test]
    fn exec_the_pane_is_handed_the_command_instead_of_a_vendor() {
        let launch = Launch::resolve(&Config::default(), &a_command("cargo test")).unwrap();

        assert_eq!(
            launched(&a_command("cargo test"), &launch, "port-it-b2c"),
            ["sh", "-c", "cargo test"],
            "no vendor, no dials, and no task appended after it"
        );
        assert_eq!(
            launched(&spawn(Some("claude"), [None; 3]), &launch, "port-it-b2c"),
            ["claude", "port the importer"],
            "and an ordinary spawn is launched the way it always was"
        );
    }

    #[test]
    fn exec_a_command_runs_where_it_was_typed_and_never_in_a_tree_of_its_own() {
        // A command is not a conversation: it has nothing to keep apart from
        // the next one, and a tree amx cut is a checkout without the build a
        // `cargo test` or an `npm test` was typed to run. So the question is
        // not asked at all — this directory is not one to work in, and a
        // command spawn never gets far enough to find out.
        let nowhere = Path::new("/nowhere/at/all");
        let config = Config::default();

        assert!(
            cut_worktree(nowhere, "cargo-test-a1b", &config, &a_command("cargo test"))
                .unwrap()
                .is_none()
        );
        assert!(
            cut_worktree(nowhere, "port-it-b2c", &config, &spawn(None, [None; 3])).is_err(),
            "where an agent is asked for one and there is nowhere to cut it"
        );
    }

    #[test]
    fn the_claim_is_the_mkdir_and_a_directory_already_there_is_not_ours() {
        // Two spawns racing one name both believe it is free; the mkdir is
        // what settles it. A directory that already exists must read as
        // somebody else's claim — never as a success to clean up later.
        let root = tempfile::TempDir::new().unwrap();
        let dir = root.path().join("fix-login-a1b");

        assert!(make_dir(&dir).unwrap(), "a free name is claimed");
        assert!(
            !make_dir(&dir).unwrap(),
            "a name somebody holds is not claimed again"
        );
    }

    /// A repository with a tree of amx's own in it, and a home to keep a
    /// vendor's trust store in.
    fn a_tree(dir: &tempfile::TempDir) -> (PathBuf, std::collections::BTreeMap<String, String>) {
        let tree = dir.path().join("app/.amx/worktrees/fix-login-a1b");
        std::fs::create_dir_all(&tree).unwrap();
        let env = spawn::env_snapshot([(
            "HOME".to_string(),
            dir.path().join("home").to_string_lossy().into_owned(),
        )]);
        (tree, env)
    }

    /// A config whose person has said yes to the trust write.
    fn agreed() -> Config {
        Config {
            trust: true,
            ..Config::default()
        }
    }

    #[test]
    fn trust_is_never_answered_until_the_config_says_yes() {
        let dir = tempfile::TempDir::new().unwrap();
        let (tree, env) = a_tree(&dir);
        let mut problems = Vec::new();

        trust_the_tree(
            &Config::default(),
            &env,
            "claude",
            &tree,
            &mut problems,
            false,
        );

        assert!(
            !trust::store_in(&env).unwrap().exists(),
            "the person's own file, and the person has not said yes"
        );
        assert!(problems.is_empty(), "declining quietly is not a problem");
    }

    #[test]
    fn trust_is_answered_for_the_tree_a_claude_spawn_was_given() {
        let dir = tempfile::TempDir::new().unwrap();
        let (tree, env) = a_tree(&dir);
        let mut problems = Vec::new();

        trust_the_tree(&agreed(), &env, "claude", &tree, &mut problems, false);

        let store = trust::store_in(&env).unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&store).unwrap()).unwrap();
        assert!(trust::trusted(&written, &tree), "{written}");
        assert!(
            problems.is_empty(),
            "{}",
            String::from_utf8_lossy(&problems)
        );
    }

    #[test]
    fn trust_is_never_answered_for_a_vendor_that_does_not_ask() {
        let dir = tempfile::TempDir::new().unwrap();
        let (tree, env) = a_tree(&dir);
        let mut problems = Vec::new();

        trust_the_tree(
            &agreed(),
            &env,
            "mock-claude --pane",
            &tree,
            &mut problems,
            false,
        );

        assert!(
            !trust::store_in(&env).unwrap().exists(),
            "a store amx invented for a vendor that keeps none"
        );
        assert!(problems.is_empty());
    }

    #[test]
    fn trust_that_cannot_be_written_is_said_once_and_the_spawn_goes_on() {
        let dir = tempfile::TempDir::new().unwrap();
        let (tree, env) = a_tree(&dir);
        let store = trust::store_in(&env).unwrap();
        std::fs::create_dir_all(store.parent().unwrap()).unwrap();
        std::fs::write(&store, "{ not json at all }").unwrap();
        let mut problems = Vec::new();

        trust_the_tree(&agreed(), &env, "claude", &tree, &mut problems, true);

        // A store amx cannot write is a warning and not a failure: the spawn
        // went ahead, and what is left is a screen somebody answers by hand.
        let told = String::from_utf8(problems).unwrap();
        assert!(told.contains("trust store amx can read"), "{told}");
        assert_eq!(told.lines().count(), 1, "{told}");
        assert!(told.starts_with("\u{1b}[33mamx new: "), "{told:?}");
    }

    #[test]
    fn dials_the_config_cannot_stop_a_spawn_the_way_a_flag_can() {
        // A config file outlives the versions that wrote it, so a value this
        // vendor cannot use is dropped and the spawn goes ahead. The file has
        // already said so on its own terms when it was read.
        let config = configured(Some("opus"), None, Some("high"));
        let launch = Launch::resolve(&config, &spawn(Some("mock-claude"), [None; 3])).unwrap();

        assert_eq!(launch.agent, "mock-claude");
        assert_eq!(launch.dials, Dials::default());
    }
}
