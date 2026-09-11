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
use crate::vendor::{Models, Vendor};
use crate::{Severity, exit, ids, models, paths, registry, said, trust, worktree};

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
    ///
    /// Which vendor runs is settled first, because a typed model picks the
    /// harness that offers it and the dials are read against whichever one
    /// that turns out to be.
    fn resolve(config: &Config, args: &NewArgs) -> Result<Launch, String> {
        let named = args.agent.as_ref();
        let agent = match named.and_then(|named| named.command.clone()) {
            Some(command) => command,
            None => picked(config, named.and_then(|named| named.model.as_deref()))?,
        };
        let agent = carrying(config, agent);
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

/// The command a spawn that named no agent runs: the harness the typed model
/// belongs to, else the configured agent as it stands.
///
/// A model is asked about only where amx has an entry for the configured
/// agent, because the answer is a comparison against what each harness offers
/// and an agent amx knows nothing about offers nothing. The sentinel is nobody's
/// model — it is the word for passing no model at all — so it picks nothing and
/// leaves the configured agent where it was.
fn picked(config: &Config, model: Option<&str>) -> Result<String, String> {
    let Some(model) = model.filter(|model| *model != registry::DEFAULT) else {
        return Ok(config.agent.clone());
    };
    if registry::entry(&config.agent).is_none() {
        return Ok(config.agent.clone());
    }
    let vendor = harness_for(config, model)?;
    // The configured command where the harness is the one it runs, because
    // `agent = "claude --add-dir .."` is how somebody says how claude is run
    // here. The bare name where it is another, because nothing in the file
    // says how to run that one.
    Ok(match registry::program(&config.agent) == vendor.name {
        true => config.agent.clone(),
        false => vendor.name.to_string(),
    })
}

/// The harness a typed model belongs to: the first whose list holds the word.
///
/// The order is the rule. A word one harness alone lists picks that harness
/// wherever it is asked; a word several list goes to the configured one where
/// it is among them, and to the first in the table otherwise — which is what
/// asking the configured agent first and stopping at the first answer says. It
/// also spends the least: a listing a vendor has to be run for is never started
/// once a list amx already holds has answered.
fn harness_for(config: &Config, model: &str) -> Result<&'static Vendor, String> {
    let mut said = Vec::new();
    for vendor in asked(config) {
        let list = models::models_of(vendor, config);
        if models::lists_model(&list, model) {
            return Ok(vendor);
        }
        said.push(takes(vendor, config, &list));
    }
    Err(format!("--model {model:?}: {}", said.join("; ")))
}

/// Every harness there is, the configured one first and the rest in the order
/// the table holds them.
fn asked(config: &Config) -> impl Iterator<Item = &'static Vendor> {
    let configured = registry::entry(&config.agent);
    let already = configured.map(|vendor| vendor.name);
    configured.into_iter().chain(
        registry::entries()
            .iter()
            .filter(move |vendor| Some(vendor.name) != already),
    )
}

/// What a harness takes, for a refusal to name.
///
/// The words themselves where amx holds the list, and how many there are and
/// what prints them where the vendor is the one holding it: a listing of
/// several hundred models is not a sentence, and the command that prints it is
/// what somebody would run to read it.
fn takes(vendor: &Vendor, config: &Config, list: &[String]) -> String {
    match vendor.models {
        Models::Printed(argv) if config.harness(vendor.name).models.is_empty() => format!(
            "{} takes {} models ({} {})",
            vendor.name,
            list.len(),
            vendor.name,
            argv.join(" ")
        ),
        _ => format!("{} takes {}", vendor.name, list.join(", ")),
    }
}

/// The command with whatever the file says this harness always carries.
///
/// Whenever it runs and however it was picked: a harness's own arguments are
/// what every agent on it is started with. They go into the command rather than
/// beside it so that everything downstream reads one command line — the dial
/// that stands down for a flag already written, the record of what was
/// launched, the pane's own argv.
fn carrying(config: &Config, agent: String) -> String {
    let args = config.harness(registry::program(&agent)).args;
    match args.is_empty() {
        true => agent,
        false => format!("{agent} {}", args.join(" ")),
    }
}

/// Run the verb against the machine.
pub fn from_env(config: &Config, args: &NewArgs) -> Result<i32> {
    let root = paths::state_root()?;
    // Spelled out from the root before anything reads it: the record holds
    // it, the cap is counted by it, and `--dir ../scratch` is a spelling no
    // record started from inside that directory would ever match.
    let dir = match &args.dir {
        Some(dir) => paths::anchored(dir)?,
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

    // The cap is the project's own, and the project is the one the agent will
    // run in rather than the one the command was typed in: `--dir` sends an
    // agent into another repository, and what that one runs at once is its own
    // file to answer. What is wrong with that file is not this spawn's to say —
    // one key of it is being asked about, and the answer to that is a pane or a
    // refusal.
    let (theirs, _) = crate::config::for_dir(dir);
    let project = spawn::project_of(dir);
    if let Some(full) = spawn::at_capacity(root, &project, theirs.max_agents, theirs.max_total)? {
        writeln!(
            problems,
            "{}",
            said(Severity::Warned, &format!("amx new: {full}"), to_terminal)
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
            command: launched(args, launch, id, config.trust),
        },
    )?;

    let server = spawn::server()?;
    let boot = vec![
        std::env::current_exe()?.to_string_lossy().into_owned(),
        "_boot".to_string(),
        id.to_string(),
    ];
    let pane = spawn::place(&server, id, &cwd, &boot)?;

    let session = session_written(
        args.exec,
        spawn::opens_under_id(&launch.agent, &args.vendor_args),
        id,
    );

    spawn::record(
        root,
        &Meta {
            id: id.to_string(),
            task: args.task.clone(),
            agent: vendor_written(args.exec, &launch.agent),
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

/// What `Meta::session` is recorded as: the id amx minted, the moment a
/// vendor that declares a start flag opens under it, rather than left `None`
/// for a Started hook that a vendor with no hooks at all could never send.
///
/// `None` from a command spawn, which opens no session of its own, and from
/// a vendor `opens_under_id` says was never offered one.
fn session_written(exec: bool, opens_under_id: bool, id: &str) -> Option<String> {
    (!exec && opens_under_id).then(|| id.to_string())
}

/// What `Meta::agent` is recorded as: the command this spawn resolved, which
/// is what was typed, else what the config holds, else the vendor amx falls
/// back to.
///
/// `None` from a command spawn. A shell command runs no vendor — the dials are
/// refused beside `--exec` and nothing about the launch is resolved for it —
/// and a row claiming one would be a row somebody went looking for a session
/// on.
fn vendor_written(exec: bool, agent: &str) -> Option<String> {
    (!exec).then(|| agent.to_string())
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
///
/// `trust` is the same config key [`trust_the_tree`] stands behind, carried
/// here because a vendor whose folder-trust answer is a flag is answered on
/// this argv rather than in a file. The key is read here, where the config is,
/// because `spawn` is handed no config of its own.
fn launched(args: &NewArgs, launch: &Launch, id: &str, trust: bool) -> Vec<String> {
    match args.exec {
        true => spawn::exec_command(&args.task),
        false => spawn::vendor_command(
            &launch.agent,
            &launch.dials,
            &args.vendor_args,
            &args.task,
            Some(id),
            trust,
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

/// Write the vendor's own trust store for the tree amx has just cut, so that
/// the agent starts on the task instead of on a question nobody has to think
/// about.
///
/// The half of the answer that is a file. A vendor answered with a flag
/// instead is answered in [`launched`], on the argv of the pane it is about,
/// and nothing here runs for it — a store amx wrote for a pi agent would be
/// another vendor's file touched over a screen pi never draws.
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
    // Which vendors amx can answer for is the wider question, and `doctor`
    // asks it. The one this write turns on is narrower: whose file it is.
    if !trust::writes_a_store(agent) {
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
    use crate::config::HarnessConfig;
    use crate::registry::DEFAULT;
    use std::collections::BTreeMap;

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
    fn the_record_names_the_vendor_this_spawn_resolved() {
        // Which vendor an agent runs is settled here, out of the flag, the
        // config and amx's own fallback, and nothing that reads the record
        // afterwards can work it out again.
        let launch = Launch::resolve(&Config::default(), &spawn(None, [None; 3])).unwrap();
        assert_eq!(
            vendor_written(false, &launch.agent).as_deref(),
            Some("claude")
        );

        // A command spawn resolves a launch it never uses — what was typed is
        // what runs — and a row claiming a vendor is one somebody goes looking
        // for a conversation on.
        assert_eq!(
            vendor_written(a_command("make test").exec, &launch.agent),
            None
        );
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
        // The agent is named here, because a word no harness lists picks none
        // and is refused. What this is about is the dial, which takes a value
        // its own cycle never names.
        let launch = Launch::resolve(
            &Config::default(),
            &spawn(Some("claude"), [Some("claude-fable-5"), None, None]),
        )
        .unwrap();
        assert_eq!(launch.dials.model, "claude-fable-5");
    }

    /// A config whose file says which models each harness named runs. pi is
    /// given a list wherever a test could reach it: the list that vendor holds
    /// is one it prints when it is run, and a unit test that started a process
    /// would be reading whatever is installed on the machine.
    fn listing(tables: &[(&str, &[&str])]) -> Config {
        Config {
            harnesses: tables
                .iter()
                .map(|(harness, models)| {
                    (
                        harness.to_string(),
                        HarnessConfig {
                            models: models.iter().map(|model| model.to_string()).collect(),
                            args: Vec::new(),
                        },
                    )
                })
                .collect(),
            ..Config::default()
        }
    }

    /// The same, for a harness that carries arguments rather than models.
    fn carrying_args(harness: &str, args: &[&str]) -> Config {
        Config {
            harnesses: BTreeMap::from([(
                harness.to_string(),
                HarnessConfig {
                    models: Vec::new(),
                    args: args.iter().map(|arg| arg.to_string()).collect(),
                },
            )]),
            ..Config::default()
        }
    }

    #[test]
    fn a_typed_model_runs_the_one_harness_that_lists_it() {
        // Nobody types --agent: the word names the harness, and the harness
        // amx is about to launch is whichever one offers it.
        let config = listing(&[("pi", &["openai/gpt-5"])]);

        let launch = Launch::resolve(&config, &spawn(None, [Some("gpt-5"), None, None])).unwrap();
        assert_eq!(launch.agent, "pi", "claude lists no such word");
        assert_eq!(launch.dials.model, "gpt-5");

        let launch = Launch::resolve(&config, &spawn(None, [Some("haiku"), None, None])).unwrap();
        assert_eq!(launch.agent, "claude", "and this one is claude's alone");
    }

    #[test]
    fn a_model_several_harnesses_list_stays_with_the_configured_one() {
        // Both list it, so the tie goes to the harness the file already names,
        // whichever of them that is.
        let claudes = listing(&[("pi", &["opus"])]);
        let launch = Launch::resolve(&claudes, &spawn(None, [Some("opus"), None, None])).unwrap();
        assert_eq!(launch.agent, "claude");

        let pis = Config {
            agent: "pi".to_string(),
            ..listing(&[("pi", &["opus"])])
        };
        let launch = Launch::resolve(&pis, &spawn(None, [Some("opus"), None, None])).unwrap();
        assert_eq!(launch.agent, "pi");
    }

    #[test]
    fn the_harnesses_are_asked_configured_first_and_then_in_the_tables_order() {
        // The order is the whole rule: the configured harness where it lists
        // the word, the first in the table where it does not, and never a
        // listing run for a harness an earlier answer has already settled.
        let named = |config: &Config| {
            asked(config)
                .map(|vendor| vendor.name)
                .collect::<Vec<&str>>()
        };
        assert_eq!(named(&Config::default()), ["claude", "pi"]);
        assert_eq!(
            named(&Config {
                agent: "pi --approve".to_string(),
                ..Config::default()
            }),
            ["pi", "claude"],
            "the configured command, read as the harness it runs"
        );
        assert_eq!(
            named(&Config {
                agent: "mock-claude".to_string(),
                ..Config::default()
            }),
            ["claude", "pi"],
            "an agent amx has no entry for is nobody in the table"
        );
    }

    #[test]
    fn a_model_no_harness_lists_is_refused_naming_what_each_takes() {
        // Refused while the command is still on screen, with enough in the
        // line to type the next one: nothing is minted and no pane is opened.
        let config = listing(&[("pi", &["openai/gpt-5"])]);

        let refusal =
            Launch::resolve(&config, &spawn(None, [Some("gpt-4"), None, None])).unwrap_err();

        assert!(refusal.contains("--model \"gpt-4\""), "{refusal}");
        assert!(
            refusal.contains("claude takes fable, opus, sonnet, haiku"),
            "{refusal}"
        );
        assert!(refusal.contains("pi takes openai/gpt-5"), "{refusal}");
    }

    #[test]
    fn a_harness_that_prints_its_models_is_named_by_how_many_and_what_prints_them() {
        // Several hundred models is not a sentence, so the refusal says how
        // many there are and what to run to read them. A list amx holds itself
        // is short and is named in full.
        let printed: Vec<String> = (0..490).map(|n| format!("openai/model-{n}")).collect();
        let pi = registry::entry("pi").expect("an entry for pi");
        assert_eq!(
            takes(pi, &Config::default(), &printed),
            "pi takes 490 models (pi --list-models)"
        );

        // And the same harness, once the file has said which models are its.
        let told = listing(&[("pi", &["openai/gpt-5"])]);
        assert_eq!(
            takes(pi, &told, &models::models_of(pi, &told)),
            "pi takes openai/gpt-5"
        );
    }

    #[test]
    fn a_model_picks_no_harness_where_the_agent_was_named() {
        // `--agent` is somebody saying which harness runs, and a model is not
        // an argument with it.
        let config = listing(&[("pi", &["openai/gpt-5"])]);
        let launch =
            Launch::resolve(&config, &spawn(Some("pi"), [Some("haiku"), None, None])).unwrap();

        assert_eq!(launch.agent, "pi", "claude lists haiku and is not asked");
        assert_eq!(launch.dials.model, "haiku");
    }

    #[test]
    fn a_model_picks_no_harness_for_a_configured_agent_amx_knows_nothing_about() {
        // An agent with no entry has no list to be asked for, so a spawn under
        // one is left exactly as it was: the model is the dial's business, and
        // the dial does not exist.
        let config = Config {
            agent: "mock-claude".to_string(),
            ..Config::default()
        };

        let refusal =
            Launch::resolve(&config, &spawn(None, [Some("opus"), None, None])).unwrap_err();

        assert!(refusal.contains("mock-claude"), "{refusal}");
        assert!(
            refusal.contains("no model dial"),
            "claude was never asked: {refusal}"
        );
    }

    #[test]
    fn the_sentinel_names_no_model_and_so_picks_no_harness() {
        // `--model default` is the word for passing no model at all, and no
        // harness lists it. Asking which one takes it would refuse every spawn
        // that turned a dial back to the vendor's own behaviour — and would
        // run a listing to do it.
        let launch = Launch::resolve(
            &Config::default(),
            &spawn(None, [Some(DEFAULT), None, None]),
        )
        .unwrap();

        assert_eq!(launch.agent, "claude");
        assert_eq!(launch.dials.model, DEFAULT);
    }

    #[test]
    fn a_harness_carries_the_arguments_its_own_table_gives_it() {
        // Whenever it runs and however it was picked, once each.
        let config = carrying_args("claude", &["--add-dir", "/tmp"]);
        let launch = Launch::resolve(&config, &spawn(None, [None; 3])).unwrap();
        assert_eq!(launch.agent, "claude --add-dir /tmp");

        let launch = Launch::resolve(&config, &spawn(Some("claude"), [None; 3])).unwrap();
        assert_eq!(
            launch.agent, "claude --add-dir /tmp",
            "the agent somebody named is still that harness"
        );

        let picked = Config {
            harnesses: BTreeMap::from([(
                "pi".to_string(),
                HarnessConfig {
                    models: vec!["openai/gpt-5".to_string()],
                    args: vec!["--approve".to_string()],
                },
            )]),
            ..Config::default()
        };
        let launch = Launch::resolve(&picked, &spawn(None, [Some("gpt-5"), None, None])).unwrap();
        assert_eq!(launch.agent, "pi --approve");
        assert_eq!(
            launch
                .agent
                .split_whitespace()
                .filter(|word| *word == "--approve")
                .count(),
            1,
            "{}",
            launch.agent
        );
    }

    #[test]
    fn a_dial_stands_down_from_a_flag_the_harnesss_own_arguments_carry() {
        // The same law the agent command's own words are under: both halves
        // end up in one argv, and a flag written there wins by the dial saying
        // nothing rather than by anybody deciding between them.
        let config = carrying_args("claude", &["--model", "opus"]);
        let args = spawn(None, [Some("haiku"), None, None]);
        let launch = Launch::resolve(&config, &args).unwrap();

        assert_eq!(
            launched(&args, &launch, "port-it-b2c", false),
            ["claude", "--model", "opus", "port the importer"]
        );
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
            launched(&a_command("cargo test"), &launch, "port-it-b2c", false),
            ["sh", "-c", "cargo test"],
            "no vendor, no dials, and no task appended after it"
        );
        assert_eq!(
            launched(
                &spawn(Some("claude"), [None; 3]),
                &launch,
                "port-it-b2c",
                false
            ),
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
    fn trust_writes_no_other_vendors_store_for_an_agent_answered_on_its_argv() {
        // The store this write is about is claude's own file. pi claims the
        // folder-trust capability too, and a guard that asked the capability
        // question would have written `~/.claude.json` for a tree no claude
        // will ever open.
        let dir = tempfile::TempDir::new().unwrap();
        let (tree, env) = a_tree(&dir);
        let mut problems = Vec::new();

        trust_the_tree(&agreed(), &env, "pi", &tree, &mut problems, false);

        assert!(
            !trust::store_in(&env).unwrap().exists(),
            "another vendor's file, touched over a screen that vendor never draws"
        );
        assert!(problems.is_empty());
    }

    #[test]
    fn trust_is_answered_on_the_argv_for_a_vendor_whose_answer_is_a_flag() {
        // pi's half of the same key, and the reason the config is read here:
        // `spawn` is handed none, and the flag has to reach the argv of the
        // pane this spawn is about to start.
        let args = spawn(Some("pi"), [None; 3]);
        let launch = Launch::resolve(&agreed(), &args).unwrap();

        assert_eq!(
            launched(&args, &launch, "port-it-b2c", true),
            [
                "pi",
                "--session-id",
                "port-it-b2c",
                "--approve",
                "port the importer"
            ]
        );
        assert_eq!(
            launched(&args, &launch, "port-it-b2c", false),
            ["pi", "--session-id", "port-it-b2c", "port the importer"],
            "and nothing at all without the key"
        );
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
    fn spawn_writes_the_minted_id_into_metasession_the_moment_a_vendor_opens_under_it() {
        // Recorded at the moment the pane is started, rather than left None
        // for a Started hook that a vendor with no hooks at all could never
        // send.
        assert_eq!(
            session_written(false, true, "fix-login-a1b"),
            Some("fix-login-a1b".to_string())
        );
        assert_eq!(
            session_written(false, false, "fix-login-a1b"),
            None,
            "no start flag was offered, so nothing was minted to record"
        );
        assert_eq!(
            session_written(true, true, "fix-login-a1b"),
            None,
            "a command spawn opens no session of its own"
        );
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
