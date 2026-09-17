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

/// Where a spawn stands in a family: the agent whose pane it was typed in,
/// where `$AMX_ID` names a record in this state root, and the depth that puts
/// it at — one more than its parent's.
///
/// Parentage is a rule rather than a flag: `_boot` puts an agent's own id in
/// its pane, every process in the pane inherits it, and `amx new` typed there
/// is a child. `--no-parent` is the escape for an agent that wants a peer,
/// and a person's own shell has no `AMX_ID` and records neither.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Lineage {
    parent: Option<String>,
    depth: u32,
}

impl Lineage {
    /// What the environment this spawn was typed in says about its family.
    ///
    /// An id that names no record here — an `AMX_ID` somebody exported by
    /// hand, or a parent since removed — records nothing, the way a person's
    /// own shell does.
    fn of(
        root: &Path,
        env: &std::collections::BTreeMap<String, String>,
        args: &NewArgs,
    ) -> Lineage {
        if args.no_parent {
            return Lineage::default();
        }
        let Some(parent) = env.get(crate::hook::ID_ENV) else {
            return Lineage::default();
        };
        let Ok(meta) = crate::store::Agent::open(root, parent).and_then(|agent| agent.meta())
        else {
            return Lineage::default();
        };
        Lineage {
            parent: Some(parent.clone()),
            depth: meta.depth + 1,
        }
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
/// launched, the pane's own argv. A word the command already carries is not
/// written twice: `agent = "claude --add-dir .."` beside the same words in the
/// table is one person saying one thing in two places, not a vendor to be
/// handed the flag twice.
fn carrying(config: &Config, agent: String) -> String {
    let carried: Vec<&str> = agent.split_whitespace().collect();
    let args: Vec<String> = config
        .harness(registry::program(&agent))
        .args
        .into_iter()
        .filter(|arg| !carried.contains(&arg.as_str()))
        .collect();
    match args.is_empty() {
        true => agent,
        false => format!("{agent} {}", args.join(" ")),
    }
}

/// Run the verb against the machine.
pub fn from_env(_config: &Config, args: &NewArgs) -> Result<i32> {
    let root = paths::state_root()?;
    // Spelled out from the root before anything reads it: the record holds
    // it, the cap is counted by it, and `--dir ../scratch` is a spelling no
    // record started from inside that directory would ever match.
    let dir = match &args.dir {
        Some(dir) => paths::anchored(dir)?,
        None => std::env::current_dir().context("no working directory")?,
    };
    // The project the agent will run in has the last word on every key, the
    // way the view's own spawns read it: which agent a project is written
    // for, what furnishes its trees and what they are cut from are its own
    // file's to say, laid over the person's a key at a time. `--dir` sends an
    // agent into another project, so it is that project's file and not the
    // one beside wherever the command was typed. A project file amx cannot
    // use leaves the person's standing under it, as everywhere else.
    let (config, _) = crate::config::for_dir(&dir);
    let config = &config;
    let env = spawn::env_snapshot(std::env::vars());
    let mut out = std::io::stdout().lock();
    let to_terminal = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let mut problems = std::io::stderr().lock();

    // The file is read here, in front of everything: it is the task, and a
    // task nobody can read is a command line that never started an agent.
    let task = match task_of(args) {
        Ok(task) => task,
        Err(no_task) => {
            writeln!(
                problems,
                "{}",
                said(
                    Severity::Warned,
                    &format!("amx new: {}", no_task.said),
                    to_terminal
                )
            )?;
            return Ok(no_task.code);
        }
    };

    run_aloud(
        &root,
        &dir,
        env,
        config,
        args,
        &task,
        &mut out,
        &mut problems,
        to_terminal,
    )
}

/// Why there is no task to start an agent on, and what amx exits with for it.
///
/// Two codes, because there are two kinds of nothing. A command line that
/// names no task is malformed and exits `USAGE`, wherever the emptiness was
/// typed. An editor that was opened and would have none of it ran and
/// answered: the command line was well formed, and what happened is a spawn
/// that did not happen, which is `FAILURE`.
struct NoTask {
    said: String,
    code: i32,
}

/// What this spawn is on: the text left in an editor where `--edit` opened one,
/// the file's text where `--file` named one, else what was typed.
///
/// One of the three and never two — the command line refuses a task typed
/// beside a file or an editor — so this is the whole of the question, and
/// everything downstream is handed the answer rather than the places it could
/// have come from.
fn task_of(args: &NewArgs) -> Result<String, NoTask> {
    if args.edit {
        return written_in_an_editor();
    }
    match &args.file {
        Some(path) => crate::cli::text_of(path).map_err(malformed),
        None => Ok(args.task.clone().unwrap_or_default()),
    }
}

/// The task somebody wrote in their editor, opened on nothing and read back
/// the way a file handed to `--file` is read.
///
/// An editor that could not be run at all is the same answer as one that
/// exited unhappily: it was asked for the task and there is none, and the
/// sentence saying so is the whole of what amx knows about it.
fn written_in_an_editor() -> Result<String, NoTask> {
    match crate::tui::edited("") {
        Ok(crate::tui::Edited::Line(text)) => crate::cli::a_text(&text).map_err(malformed),
        Ok(crate::tui::Edited::No(why)) => Err(NoTask {
            said: why,
            code: exit::FAILURE,
        }),
        Err(e) => Err(NoTask {
            said: format!("{e:#}"),
            code: exit::FAILURE,
        }),
    }
}

/// A command line that could not name a task.
fn malformed(said: String) -> NoTask {
    NoTask {
        said,
        code: exit::USAGE,
    }
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
    // The view types its task on the line at the foot of the screen; there is
    // no file behind that door.
    let task = args.task.clone().unwrap_or_default();
    run_aloud(root, dir, env, config, args, &task, out, problems, false)
}

/// The same, told to a stderr that is a terminal and wants the colour.
#[allow(clippy::too_many_arguments)]
fn run_aloud(
    root: &Path,
    dir: &Path,
    env: std::collections::BTreeMap<String, String>,
    config: &Config,
    args: &NewArgs,
    task: &str,
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

    // Where this spawn stands in a family, read off the environment it was
    // typed in. A spawn past the configured depth is refused here, before a
    // tree is cut or an id is claimed, the way a cap is: there is nothing to
    // clean up and nobody has to answer for a pane that should not exist.
    let lineage = Lineage::of(root, &env, args);
    if lineage.depth as usize > config.subagent_depth {
        writeln!(
            problems,
            "{}",
            said(
                Severity::Warned,
                &format!(
                    "amx new: subagent_depth is {} and this spawn would be at depth {}",
                    config.subagent_depth, lineage.depth
                ),
                to_terminal
            )
        )?;
        return Ok(exit::BLOCKED);
    }

    // Asked before anything is made, the same as a base git cannot resolve:
    // `--with-changes` where the last commit already holds all of it is a
    // command nobody meant, and answering it here leaves no id, no tree and no
    // install standing behind it.
    if args.with_changes && !worktree::has_changes_to_carry(dir)? {
        bail!("--with-changes: nothing in {} to move", dir.display());
    }

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

    let (id, agent_dir) = claim(root, args, task)?;

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
        task,
        &launch,
        &lineage,
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
fn claim(root: &Path, args: &NewArgs, task: &str) -> Result<(String, PathBuf)> {
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
        let id = ids::generate(task, root)?;
        let dir = paths::agent_dir_in(root, &id)?;
        if make_dir(&dir)? {
            return Ok((id, dir));
        }
    }
    bail!(
        "no id for {task:?} could be claimed under {} after {MAX_CLAIMS} draws",
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
    task: &str,
    launch: &Launch,
    lineage: &Lineage,
    id: &str,
    problems: &mut impl Write,
    to_terminal: bool,
) -> Result<()> {
    // What the file says this harness runs with, before anything reads the
    // environment on the agent's behalf: the trust store is looked up in it,
    // and a table pointing claude at another config directory has to point
    // the seeding at the same one, or the agent meets the screen the seeding
    // was meant to answer.
    spawn::harness_env(&mut env, config, &launch.agent);

    let cut = cut_worktree(dir, id, config, args)?;
    let tree = cut.as_ref().map(|(_, tree)| tree);
    let cwd = tree
        .map(|tree| tree.path.clone())
        .unwrap_or_else(|| dir.to_path_buf());
    if let Some((repo, tree)) = &cut {
        furnish_the_tree(config, agent_dir, id, repo, tree, problems, to_terminal)?;
        // After the furnishing, so that a setup command that fails takes back
        // a tree with nothing of yours in it. Whether there was anything to
        // move was settled before the id was minted, and a directory somebody
        // has committed in since is no longer a spawn to refuse.
        if args.with_changes
            && let Err(e) = worktree::carry_changes(dir, &tree.path)
        {
            // The same undo a failed setup gets. The work that would not
            // apply is still where it was typed, so the tree holds nothing of
            // yours, and a tree standing under an id nothing records would
            // refuse the next spawn under that name.
            take_back(repo, tree, problems, to_terminal);
            return Err(e);
        }
    }
    if let Some(tree) = tree_to_trust(tree.map(|tree| tree.path.as_path()), dir, args.exec) {
        trust_the_tree(config, &env, &launch.agent, tree, problems, to_terminal);
    }

    // amx's own id over the top of the harness's pairs laid above: a table
    // that set the id would have this agent reporting under somebody else's
    // name.
    env.insert(crate::hook::ID_ENV.to_string(), id.to_string());
    spawn::write_boot_env(agent_dir, &env)?;
    spawn::write_handoff(
        agent_dir,
        &Handoff {
            task: task.to_string(),
            command: launched(args, task, launch, id, config.trust),
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
            task: task.to_string(),
            agent: vendor_written(args.exec, &launch.agent),
            model: dial_written(args.exec, &launch.dials.model),
            effort: dial_written(args.exec, &launch.dials.effort),
            parent: lineage.parent.clone(),
            depth: lineage.depth,
            dir: cwd,
            worktree: tree.map(|tree| tree.path.clone()),
            branch: tree.map(|tree| tree.branch.clone()),
            base: tree.map(|tree| tree.base.clone()),
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

/// What `Meta::model` and `Meta::effort` are recorded as: the value the dial
/// was turned to, by the command line or by the config, and `None` for a dial
/// left where the vendor's own configuration puts it.
///
/// [`registry::DEFAULT`] is amx saying it sent no flag, which is not a value
/// the vendor was asked for — a record repeating it would have the wall
/// claiming to know a word only the vendor knows. `None` from a command spawn
/// for the same reason `vendor_written` gives: the dials are refused beside
/// `--exec` and nothing was resolved for it.
fn dial_written(exec: bool, dial: &str) -> Option<String> {
    (!exec && dial != registry::DEFAULT).then(|| dial.to_string())
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
fn launched(args: &NewArgs, task: &str, launch: &Launch, id: &str, trust: bool) -> Vec<String> {
    match args.exec {
        true => spawn::exec_command(task),
        false => spawn::vendor_command(
            &launch.agent,
            &launch.dials,
            &args.vendor_args,
            task,
            Some(id),
            trust,
        ),
    }
}

/// What the tree is cut from: the ref this spawn was given, else the one the
/// config holds, else nothing, which is the commit checked out where `new` was
/// typed.
///
/// The same order every other dial is read in, and for the same reason: the
/// flag is somebody standing there saying it about this one agent, and the key
/// is the answer they wrote down once for all of them.
fn cut_from<'a>(config: &'a Config, args: &'a NewArgs) -> Option<&'a str> {
    args.base.as_deref().or(config.base.as_deref())
}

/// A worktree of its own, when the agent is being sent into a repository and
/// nobody has said not to, with the repository it was cut from beside it.
///
/// Never for a command. A worktree is there to keep one conversation's work
/// apart from another's, and a command has no conversation: it was typed to
/// run *here*, against this checkout and whatever is already built in it.
///
/// The repository is answered with rather than asked for again, because
/// furnishing copies out of it and the question has two answers: `git` asked
/// from inside a linked worktree names that worktree, and the tree was cut
/// from whichever one `new` was typed in.
fn cut_worktree(
    dir: &Path,
    id: &str,
    config: &Config,
    args: &NewArgs,
) -> Result<Option<(PathBuf, worktree::Worktree)>> {
    // The key says what happens when nobody asked for a tree. A request and a
    // branch are somebody asking: there is no working on either without the
    // branch it is on checked out somewhere, so both are a tree whatever the
    // key says.
    let asked_for_a_branch = args.pr.is_some() || args.branch.is_some();
    if args.exec || args.no_worktree || (!config.worktrees && !asked_for_a_branch) {
        return Ok(None);
    }
    if !dir.is_dir() {
        bail!("{} is not a directory to run in", dir.display());
    }
    // Somewhere that is not a repository is somewhere to work in as it is —
    // unless a request or a branch was named, which are a repository's own
    // things to have and nothing a directory outside one could be started on.
    let Some(repo) = worktree::repo_root(dir)? else {
        match (args.pr, args.branch.as_deref()) {
            (Some(number), _) => bail!("--pr {number}: {} is in no repository", dir.display()),
            (_, Some(name)) => bail!("--branch {name}: {} is in no repository", dir.display()),
            _ => return Ok(None),
        }
    };
    let tree = match (args.pr, args.branch.as_deref()) {
        (Some(number), _) => cut_on_request(&repo, id, number)?,
        (_, Some(name)) => cut_on_branch(&repo, id, name)?,
        _ => worktree::create(&repo, id, cut_from(config, args))?,
    };
    Ok(Some((repo, tree)))
}

/// A tree on the head branch of request `number`, fetched from the origin.
///
/// The name is the head ref's own wherever it can be, because that is what the
/// PR column reads a row's request back off. It cannot be when the work is in
/// somebody's fork, where the same name means another branch, or when a tree
/// in this repository already holds it — git keeps one tree to a branch, and a
/// second agent on the same request is a thing to allow rather than refuse.
fn cut_on_request(repo: &Path, id: &str, number: u64) -> Result<worktree::Worktree> {
    let head = crate::pr::request_head(repo, number)?;
    let name = match head.cross || worktree::checked_out(repo, &head.branch)? {
        true => format!("pr-{number}"),
        false => head.branch,
    };
    worktree::create_on(repo, id, &name, &format!("refs/pull/{number}/head"))
}

/// A tree on `name`, a branch this checkout already has or the origin does.
///
/// [`cut_on_request`]'s other half: the same tree on a branch somebody already
/// made, without a forge to ask where it is. Which of the two ways it is cut
/// turns on whether the ref is here — a branch with commits nobody has pushed
/// would be moved to whatever the origin holds if it were fetched, and a name
/// only the origin has is no branch at all until it is.
///
/// `origin/x` is how a branch on the forge is usually read out, and the tree
/// goes on `x` either way, so the prefix comes off rather than being hunted for
/// under a name nothing has.
///
/// The refusals are both about the name: git keeps one tree to a branch, so one
/// another tree holds cannot be checked out again — and unlike a request, which
/// can be started twice under a name of amx's own, there is no second name for
/// the branch somebody typed.
fn cut_on_branch(repo: &Path, id: &str, name: &str) -> Result<worktree::Worktree> {
    let name = name.strip_prefix("origin/").unwrap_or(name);
    if worktree::checked_out(repo, name)? {
        bail!("{name} is checked out in another tree already");
    }
    if here_already(repo, name) {
        return worktree::create_on_local(repo, id, name);
    }
    match worktree::create_on(repo, id, name, name) {
        Ok(tree) => Ok(tree),
        // Whatever git said about the fetch, what happened is that the name
        // was in neither place, which is the sentence to answer with.
        Err(_) => bail!("{name} is no branch here or on origin"),
    }
}

/// Whether this checkout already has a branch called `name`.
///
/// Asked of the refs alone, because the answer decides which way the tree is
/// cut rather than anything about the tree itself.
fn here_already(repo: &Path, name: &str) -> bool {
    std::process::Command::new("git")
        .current_dir(repo)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Furnish the tree amx has just cut: the files and directories the config
/// names, and then its setup commands, before the pane is placed.
///
/// The other half of a tree being worth working in, and the opposite kind of
/// answer to [`trust_the_tree`]'s. A path the config names and the repository
/// does not have is said and stepped over — a config file outlives the project
/// it was written for. A setup command that fails takes the tree with it and
/// refuses the spawn, because what is left of a tree whose install did not
/// finish is worse than no tree at all: the agent would spend its first turn
/// working that out.
fn furnish_the_tree(
    config: &Config,
    agent_dir: &Path,
    id: &str,
    repo: &Path,
    tree: &worktree::Worktree,
    problems: &mut impl Write,
    to_terminal: bool,
) -> Result<()> {
    // The two a setup command cannot work out for itself. The tree it runs in
    // and the repository behind it are the furnishing's own to say.
    let env = [
        (crate::hook::ID_ENV.to_string(), id.to_string()),
        (
            spawn::AGENT_DIR_ENV.to_string(),
            spawn::scratch(agent_dir)?.to_string_lossy().into_owned(),
        ),
    ];

    match worktree::furnish(
        repo,
        &tree.path,
        &config.copy,
        &config.link,
        &config.setup,
        &env,
    ) {
        Ok(missing) => {
            for path in missing {
                writeln!(
                    problems,
                    "{}",
                    said(Severity::Warned, &format!("amx new: {path}"), to_terminal)
                )?;
            }
            Ok(())
        }
        Err(e) => {
            // Nothing half furnished stands.
            take_back(repo, tree, problems, to_terminal);
            Err(e)
        }
    }
}

/// Take back a tree the spawn is not going to use: it was cut a moment ago
/// and holds nothing of the person's, so it goes with its branch. What the
/// undo cannot do is said, and the refusal that brought it here is still the
/// answer: the spawn is off either way.
fn take_back(repo: &Path, tree: &worktree::Worktree, problems: &mut impl Write, to_terminal: bool) {
    if let Err(undone) = worktree::discard(repo, &tree.path, &tree.branch) {
        let _ = writeln!(
            problems,
            "{}",
            said(
                Severity::Warned,
                &format!("amx new: {undone:#}"),
                to_terminal
            )
        );
    }
}

/// The tree whose folder-trust screen is amx's to answer on this spawn: the
/// one it cut, or the linked worktree it was pointed at when it cut none.
///
/// The second is how `workflow run` dispatches every worker and reader, with
/// `--no-worktree --dir <a tree it cut itself>`, and a linked worktree is
/// derived from a repository whoever cut it, which is the same provenance a
/// tree amx cut has. A checkout is the person's own to answer, a plain
/// directory is derived from nothing, and a command asks no vendor anything,
/// so none of those is answered for.
fn tree_to_trust<'a>(cut: Option<&'a Path>, dir: &'a Path, exec: bool) -> Option<&'a Path> {
    if exec {
        return None;
    }
    cut.or_else(|| worktree::is_linked(dir).then_some(dir))
}

/// Write the vendor's own trust store for the tree the agent is about to
/// start in, so that it starts on the task instead of on a question nobody
/// has to think about.
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
            task: Some("port the importer".to_string()),
            file: None,
            edit: false,
            name: None,
            dir: None,
            no_worktree: false,
            no_parent: false,
            base: None,
            branch: None,
            pr: None,
            with_changes: false,
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
            task: Some(command.to_string()),
            file: None,
            edit: false,
            name: None,
            dir: None,
            no_worktree: false,
            no_parent: false,
            base: None,
            branch: None,
            pr: None,
            with_changes: false,
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
                "port the importer",
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
                            env: BTreeMap::new(),
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
                    env: BTreeMap::new(),
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
                    env: BTreeMap::new(),
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
            launched(&args, "port the importer", &launch, "port-it-b2c", false),
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
            launched(
                &a_command("cargo test"),
                "cargo test",
                &launch,
                "port-it-b2c",
                false
            ),
            ["sh", "-c", "cargo test"],
            "no vendor, no dials, and no task appended after it"
        );
        assert_eq!(
            launched(
                &spawn(Some("claude"), [None; 3]),
                "port the importer",
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
    fn the_tree_is_cut_from_the_flag_then_the_key_then_what_is_checked_out() {
        let held = Config {
            base: Some("main".to_string()),
            ..Config::default()
        };
        let mut args = spawn(None, [None; 3]);

        assert_eq!(
            cut_from(&Config::default(), &args),
            None,
            "nothing said is the commit the command was typed on"
        );
        assert_eq!(
            cut_from(&held, &args),
            Some("main"),
            "the key, for all of them"
        );

        args.base = Some("release-2".to_string());
        assert_eq!(
            cut_from(&held, &args),
            Some("release-2"),
            "and the flag, for this one"
        );
    }

    /// git as these tests run it: none of the developer's own configuration
    /// and an identity of its own.
    fn setup(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "amx tests")
            .env("GIT_AUTHOR_EMAIL", "tests@example.invalid")
            .env("GIT_COMMITTER_NAME", "amx tests")
            .env("GIT_COMMITTER_EMAIL", "tests@example.invalid")
            .output()
            .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    }

    /// A repository with one commit on `main` and no origin at all.
    fn a_repo(dir: &tempfile::TempDir) -> PathBuf {
        let repo = dir.path().join("app");
        std::fs::create_dir_all(&repo).unwrap();
        setup(&repo, &["init", "-b", "main"]);
        std::fs::write(repo.join("README.md"), "the work\n").unwrap();
        setup(&repo, &["add", "README.md"]);
        setup(&repo, &["commit", "-m", "the first commit"]);
        repo
    }

    /// How many trees this repository has, the checkout itself included.
    fn trees_in(repo: &Path) -> usize {
        setup(repo, &["worktree", "list", "--porcelain"])
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count()
    }

    #[test]
    fn a_branch_another_tree_already_holds_starts_no_agent() {
        // git keeps one tree to a branch, and the checkout itself is a tree:
        // `--branch main` in a repository standing on main is the everyday
        // version of it. Asked before anything is made, so there is nothing
        // to take back.
        let dir = tempfile::TempDir::new().unwrap();
        let repo = a_repo(&dir);

        let refusal = cut_on_branch(&repo, "fix-login-a1b", "main").unwrap_err();

        assert!(
            refusal.to_string().contains("main is checked out"),
            "{refusal:#}"
        );
        assert_eq!(trees_in(&repo), 1, "and no tree was cut for it");
    }

    #[test]
    fn a_branch_that_is_neither_here_nor_on_the_origin_starts_no_agent() {
        // Both halves have been tried by the time this is said: there is no
        // such ref in this checkout, and the fetch that would have brought one
        // came back with nothing.
        let dir = tempfile::TempDir::new().unwrap();
        let repo = a_repo(&dir);

        let refusal = cut_on_branch(&repo, "fix-login-a1b", "spike").unwrap_err();

        assert_eq!(
            refusal.to_string(),
            "spike is no branch here or on origin",
            "{refusal:#}"
        );
        assert_eq!(trees_in(&repo), 1, "and no tree was cut for it");
    }

    #[test]
    fn a_branch_is_a_tree_whatever_the_key_says_and_nowhere_to_cut_one_is_refused() {
        // The key answers for the spawns nobody said anything about. Naming a
        // branch is somebody saying it, and outside a repository there is no
        // branch of that name to say it about.
        let dir = tempfile::TempDir::new().unwrap();
        let mut args = spawn(None, [None; 3]);
        args.branch = Some("spike".to_string());
        let no_trees = Config {
            worktrees: false,
            ..Config::default()
        };

        let refusal = cut_worktree(dir.path(), "fix-login-a1b", &no_trees, &args).unwrap_err();

        assert!(
            refusal.to_string().contains("--branch spike"),
            "{refusal:#}"
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
            launched(&args, "port the importer", &launch, "port-it-b2c", true),
            [
                "pi",
                "--session-id",
                "port-it-b2c",
                "--approve",
                "port the importer"
            ]
        );
        assert_eq!(
            launched(&args, "port the importer", &launch, "port-it-b2c", false),
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
    fn trust_is_answered_for_the_linked_worktree_a_no_worktree_spawn_was_pointed_at() {
        // `workflow run` cuts its own trees and dispatches every worker with
        // `--no-worktree --dir <tree>`, so nothing here cut anything, and
        // until this the store was seeded for nothing: every claude reader in
        // a repository nobody had trusted stopped at the screen and died
        // there, with no event to say so. A linked worktree is derived from a
        // repository whoever cut it, which is the same provenance a tree amx
        // cut has. A checkout, a plain directory and a command are not.
        let dir = tempfile::TempDir::new().unwrap();
        let repo = a_repo(&dir);
        let theirs = dir.path().join("workflow/plan/t1");
        setup(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                &theirs.to_string_lossy(),
                "-b",
                "t1",
            ],
        );
        let cut = repo.join(".amx/worktrees/fix-login-a1b");

        assert_eq!(tree_to_trust(Some(&cut), &repo, false), Some(cut.as_path()));
        assert_eq!(tree_to_trust(None, &theirs, false), Some(theirs.as_path()));
        assert_eq!(tree_to_trust(None, &repo, false), None, "the checkout");
        assert_eq!(
            tree_to_trust(None, dir.path(), false),
            None,
            "a plain directory"
        );
        assert_eq!(
            tree_to_trust(None, &theirs, true),
            None,
            "a command asks no vendor anything"
        );
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

    #[test]
    fn a_harness_argument_the_agent_command_already_carries_is_not_written_twice() {
        let mut config = carrying_args("claude", &["--add-dir", "/srv/shared", "--verbose"]);
        config.agent = "claude --add-dir /srv/shared".to_string();
        let launch = Launch::resolve(&config, &spawn(None, [None, None, None])).unwrap();
        assert_eq!(
            launch.agent, "claude --add-dir /srv/shared --verbose",
            "the words the command carries stand once, and the rest follow"
        );
    }
}
