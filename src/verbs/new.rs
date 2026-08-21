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
use crate::spawn::{self, Dials, Handoff, Placement};
use crate::store::{Meta, now};
use crate::{exit, ids, paths, registry, trust, worktree};

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
    let mut problems = std::io::stderr().lock();

    run(&root, &dir, env, config, args, &mut out, &mut problems)
}

/// The verb, with everything it reads named.
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
    // Before anything is made: a dial the vendor would not take is a
    // malformed command line, and there is nothing to clean up if it is
    // answered here.
    let launch = match Launch::resolve(config, args) {
        Ok(launch) => launch,
        Err(refusal) => {
            writeln!(problems, "amx new: {refusal}")?;
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
            "amx new: {} agents already running, and max_agents is {}",
            live.len(),
            config.max_agents
        )?;
        return Ok(exit::BLOCKED);
    }

    let id = match &args.name {
        Some(name) => {
            ids::validate_name(name, root)?;
            name.clone()
        }
        None => ids::generate(&args.task, root)?,
    };

    let agent_dir = paths::agent_dir_in(root, &id)?;
    make_dir(&agent_dir)?;

    // From here on a failure leaves nothing behind: an id that half exists is
    // worse than one that does not.
    match start(
        root, &agent_dir, dir, env, config, args, &launch, &id, problems,
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
) -> Result<()> {
    let tree = cut_worktree(dir, id, config, args)?;
    let cwd = tree
        .as_ref()
        .map(|tree| tree.path.clone())
        .unwrap_or_else(|| dir.to_path_buf());
    if let Some(tree) = &tree {
        trust_the_tree(&env, &launch.agent, &tree.path, problems);
    }

    env.insert(crate::hook::ID_ENV.to_string(), id.to_string());
    spawn::write_handoff(
        agent_dir,
        &Handoff {
            task: args.task.clone(),
            command: spawn::vendor_command(
                &launch.agent,
                &launch.dials,
                &args.vendor_args,
                &args.task,
            ),
            env,
        },
    )?;

    let server = spawn::server(root)?;
    let placement = if args.bg {
        Placement::Background
    } else {
        Placement::Wall
    };
    let boot = vec![
        std::env::current_exe()?.to_string_lossy().into_owned(),
        "_boot".to_string(),
        id.to_string(),
    ];
    let pane = spawn::place(&server, placement, id, &cwd, &boot, &spawn::wall_lock(root))?;

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
            session: None,
            transcript: None,
            created: now(),
        },
    )?;
    Ok(())
}

/// A worktree of its own, when the agent is being sent into a repository and
/// nobody has said not to.
fn cut_worktree(
    dir: &Path,
    id: &str,
    config: &Config,
    args: &NewArgs,
) -> Result<Option<worktree::Worktree>> {
    if args.no_worktree || !config.worktrees {
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
    env: &std::collections::BTreeMap<String, String>,
    agent: &str,
    tree: &Path,
    problems: &mut impl Write,
) {
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
        let _ = writeln!(problems, "amx new: {e:#}");
    }
}

/// The agent's own directory, which nobody else has any business reading.
fn make_dir(dir: &PathBuf) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("creating {}", dir.display()))
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
            bg: false,
            name: None,
            dir: None,
            no_worktree: false,
            agent: Some(AgentArgs {
                command: agent.map(str::to_string),
                model: model.map(str::to_string),
                permission: permission.map(str::to_string),
                effort: effort.map(str::to_string),
            }),
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

    #[test]
    fn trust_is_answered_for_the_tree_a_claude_spawn_was_given() {
        let dir = tempfile::TempDir::new().unwrap();
        let (tree, env) = a_tree(&dir);
        let mut problems = Vec::new();

        trust_the_tree(&env, "claude", &tree, &mut problems);

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

        trust_the_tree(&env, "mock-claude --pane", &tree, &mut problems);

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

        trust_the_tree(&env, "claude", &tree, &mut problems);

        let said = String::from_utf8(problems).unwrap();
        assert!(said.contains("trust store amx can read"), "{said}");
        assert_eq!(said.lines().count(), 1, "{said}");
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
