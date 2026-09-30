//! `amx new`: start an agent on a task.
//!
//! - A spawn always runs in this order: claim an id, cut a worktree, write the
//!   handoff, place the pane, write the record. The pane waits for the record
//!   before it starts the vendor, so the first hook always has a record to land
//!   in.
//! - stdout carries the id and nothing else, so a caller can pipe it into the
//!   next command.

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::cli::NewArgs;
use crate::config::Config;
use crate::role::{self, Role};
use crate::spawn::{self, Dials, Handoff};
use crate::store::{Meta, now};
use crate::vendor::{Models, Vendor};
use crate::{Severity, exit, ids, models, paths, registry, said, trust, worktree};

/// The vendor command a spawn launches and the dials it launches with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Launch {
    pub(crate) agent: String,
    pub(crate) dials: Dials,
}

impl Launch {
    /// Resolves each dial from the flag, else the config, else the vendor's
    /// default.
    ///
    /// A flag value the vendor would not accept is an error. A config value it
    /// would not accept is dropped, since config files outlive vendor versions
    /// and config loading already warned about it. The vendor is resolved first
    /// because a typed model can pick the harness.
    pub(crate) fn resolve(config: &Config, args: &NewArgs) -> Result<Launch, String> {
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
                    // Only a closed dial refuses a value, so its cycle lists
                    // every accepted value.
                    Some(spec) => {
                        return Err(format!(
                            "--{key} {value:?}: {} accepts {}",
                            registry::program(&agent),
                            spec.cycle.join(", ")
                        ));
                    }
                    None => {
                        return Err(format!(
                            "--{key} {value:?}: amx cannot set {key} for {}",
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

/// A spawn's parent and depth in the agent tree.
///
/// Only `amx sub` sets `NewArgs::parent` (from `AMX_ID` or `--parent`); `amx
/// new` always starts a root, whatever pane it runs in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Lineage {
    parent: Option<String>,
    depth: u32,
}

impl Lineage {
    /// Reads the parent's depth. A parent with no record counts as no parent.
    fn of(root: &Path, args: &NewArgs) -> Lineage {
        let Some(parent) = &args.parent else {
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

/// The command for a spawn that named no agent: the harness that lists the
/// typed model, else the configured agent.
///
/// The model picks a harness only when amx has an entry for the configured
/// agent. [`registry::DEFAULT`] means "pass no model" and picks nothing.
fn picked(config: &Config, model: Option<&str>) -> Result<String, String> {
    let Some(model) = model.filter(|model| *model != registry::DEFAULT) else {
        return Ok(config.agent.clone());
    };
    if registry::entry(&config.agent).is_none() {
        return Ok(config.agent.clone());
    }
    let vendor = harness_for(config, model)?;
    // Keep the configured command, with its arguments, when it runs the chosen
    // harness; otherwise the bare harness name.
    Ok(match registry::program(&config.agent) == vendor.name {
        true => config.agent.clone(),
        false => vendor.name.to_string(),
    })
}

/// The first harness, in [`asked`] order, whose model list holds `model`.
///
/// Asking the configured harness first breaks ties in its favour and avoids
/// running a vendor's model listing when an earlier list already answered.
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

/// Every harness, the configured one first and the rest in table order.
fn asked(config: &Config) -> impl Iterator<Item = &'static Vendor> {
    let configured = registry::entry(&config.agent);
    let already = configured.map(|vendor| vendor.name);
    configured.into_iter().chain(
        registry::entries()
            .iter()
            .filter(move |vendor| Some(vendor.name) != already),
    )
}

/// The models a harness accepts, as a refusal names them.
///
/// A list the vendor prints can run to hundreds of models, so it is named by
/// count and by the command that prints it.
fn takes(vendor: &Vendor, config: &Config, list: &[String]) -> String {
    match vendor.models {
        Models::Printed(argv) | Models::Json(argv)
            if config.harness(vendor.name).models.is_empty() =>
        {
            format!(
                "{} lists {} models (`{} {}`)",
                vendor.name,
                list.len(),
                vendor.name,
                argv.join(" ")
            )
        }
        _ => format!("{} accepts {}", vendor.name, list.join(", ")),
    }
}

/// `agent` with the harness's configured `args` appended, skipping any word
/// the command already carries.
///
/// The args join the command itself so that dials, the record and the pane
/// argv all see one command line.
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

/// Runs the verb from the command line.
pub fn from_env(args: &NewArgs) -> Result<i32> {
    let root = paths::state_root()?;
    // Absolute before anything reads it, so the record and the cap count
    // match records started from inside that directory.
    let dir = match &args.dir {
        Some(dir) => paths::anchored(dir)?,
        None => std::env::current_dir().context("no working directory")?,
    };
    // The config of the directory the agent runs in, which with `--dir` is
    // not the directory the command was typed in.
    let (config, warnings) = crate::config::for_dir(&dir);
    let config = &config;
    // main already reported warnings about the user's config; only the
    // project file's warnings (such as a file nobody allowed) are new here.
    if let Some(file) = crate::paths::project_config(&dir) {
        let file = file.display().to_string();
        for warning in warnings.iter().filter(|warning| warning.contains(&file)) {
            crate::warn!("amx: {warning}");
        }
    }
    let env = spawn::env_snapshot(std::env::vars());
    let mut out = std::io::stdout().lock();
    let to_terminal = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let mut problems = std::io::stderr().lock();

    // Read the task first: without one there is nothing to start.
    let task = match task_of(args) {
        Ok(task) => task,
        Err(no_task) => {
            warned(&mut problems, &no_task.said, to_terminal)?;
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

/// Writes `what` to `problems` as a warning from `amx new`.
fn warned(problems: &mut impl Write, what: &str, to_terminal: bool) -> std::io::Result<()> {
    writeln!(
        problems,
        "{}",
        said(Severity::Warned, &format!("amx new: {what}"), to_terminal)
    )
}

/// Why there is no task, and the exit code for it.
///
/// A command line that names no usable task exits `USAGE`. An editor that ran
/// and produced no task exits `FAILURE`: the command line was valid.
struct NoTask {
    said: String,
    code: i32,
}

/// The task from `--edit`, `--file` or the command line. clap allows only one.
fn task_of(args: &NewArgs) -> Result<String, NoTask> {
    if args.edit {
        return written_in_an_editor();
    }
    match &args.file {
        Some(path) => crate::cli::text_of(path).map_err(malformed),
        None => Ok(args.task.clone().unwrap_or_default()),
    }
}

/// The task written in `$EDITOR`, read back the way `--file` text is read.
///
/// An editor that fails to run and one that exits unhappily both yield
/// `FAILURE`.
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

fn malformed(said: String) -> NoTask {
    NoTask {
        said,
        code: exit::USAGE,
    }
}

/// The verb with its inputs passed in and refusals written uncoloured.
///
/// The view and `amx sub` spawn through here; the view shows a refusal in its
/// own notice line, so it must carry no terminal colour.
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
    let task = args.task.clone().unwrap_or_default();
    run_aloud(root, dir, env, config, args, &task, out, problems, false)
}

/// [`run`] with the task already read, colouring refusals when `to_terminal`.
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
    // tmux silently opens a pane whose directory is missing in the home
    // directory, so the agent would run, and ask for trust, somewhere else.
    if !dir.is_dir() {
        bail!("{} is not a directory to run in", dir.display());
    }
    // Resolve the role before anything is made: it sets default dials and a
    // brief, and an unknown name is a usage error.
    let mut args_with_role = args.clone();
    let mut brief = String::new();
    if let Some(name) = args.role.clone() {
        let (personal, project) = role_places(dir)?;
        let (found, warnings) = role::for_name(&personal, &project, &name);
        for warning in warnings {
            warned(problems, &warning, to_terminal)?;
        }
        let Some(role) = found else {
            let known = role::names_under(&personal, &project);
            warned(
                problems,
                &format!("no role `{name}`: {}", known.join(", ")),
                to_terminal,
            )?;
            return Ok(exit::USAGE);
        };
        brief = role.brief.clone();
        fill_from_role(&role, &mut args_with_role);
    }
    // A subagent's digest of its parent follows the role brief. The vendor
    // sees it; the record does not keep it.
    if let Some(context) = &args.context_brief {
        if !brief.is_empty() {
            brief.push_str("\n\n");
        }
        brief.push_str(context);
    }

    let args = &args_with_role;
    // The vendor gets brief and task; the record keeps the task alone.
    let lined = match brief.is_empty() {
        true => task.to_string(),
        false => format!("{brief}\n\n{task}"),
    };

    // Refuse bad dials before anything is made, so there is nothing to undo.
    let launch = match Launch::resolve(config, args) {
        Ok(launch) => launch,
        Err(refusal) => {
            warned(problems, &refusal, to_terminal)?;
            return Ok(exit::USAGE);
        }
    };

    // Refuse a spawn past `subagent_depth` before any id or tree exists.
    let lineage = Lineage::of(root, args);
    if lineage.depth as usize > config.subagent_depth {
        warned(
            problems,
            &format!(
                "subagent_depth is {} and this spawn would be at depth {}",
                config.subagent_depth, lineage.depth
            ),
            to_terminal,
        )?;
        return Ok(exit::BLOCKED);
    }

    // `--with-changes` on a clean checkout is refused before anything is made.
    if args.with_changes && !worktree::has_changes_to_carry(dir)? {
        bail!(
            "--with-changes: {} has no uncommitted changes to move",
            dir.display()
        );
    }

    std::fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;

    // Count and claim in one step so two concurrent spawns cannot both take
    // the last place. The place is held until `start` has written the record.
    let taken = take_a_place(root, dir, || {
        let (id, agent_dir) = claim(root, args.name.as_deref(), task)?;
        Ok(((id, agent_dir.clone()), agent_dir))
    })?;
    let ((id, agent_dir), _place) = match taken {
        Ok(taken) => taken,
        Err(full) => {
            warned(problems, &full, to_terminal)?;
            return Ok(exit::BLOCKED);
        }
    };

    // On failure remove the claimed directory. The claim created it, so it
    // cannot hold another spawn's record.
    match start(
        root,
        &agent_dir,
        dir,
        env,
        config,
        args,
        task,
        &lined,
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

/// Counts `dir`'s project against its caps and, where there is room, claims a
/// place for the agent `claim` makes, held until the claim is dropped.
pub(crate) fn take_a_place<T>(
    root: &Path,
    dir: &Path,
    claim: impl FnOnce() -> Result<(T, PathBuf)>,
) -> Result<Result<(T, crate::store::Claim), String>> {
    let (theirs, _) = crate::config::for_dir_in(dir, root);
    spawn::take_a_place(
        root,
        &spawn::project_of(dir),
        theirs.max_agents,
        theirs.max_total,
        claim,
    )
}

/// Starts `amx _boot <id>` in a pane of its own in `cwd`.
pub(crate) fn place_boot(
    id: &str,
    cwd: &Path,
) -> Result<(crate::tmux::Server, crate::tmux::PaneId)> {
    let server = spawn::server()?;
    let boot = [
        std::env::current_exe()?.to_string_lossy().into_owned(),
        "_boot".to_string(),
        id.to_string(),
    ];
    let pane = spawn::place(&server, id, cwd, &boot)?;
    Ok((server, pane))
}

/// The user's role directory, beside the config file, and the project's,
/// under its `.amx`.
fn role_places(dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let config = paths::config_file()?;
    let beside = config
        .parent()
        .context("the config file has no directory")?;
    Ok((
        role::agents_under(beside),
        role::project_dir(&spawn::project_of(dir)),
    ))
}

/// Fills the dials `args` left empty from `role`. Typed flags win.
fn fill_from_role(role: &Role, args: &mut NewArgs) {
    let named = args.agent.get_or_insert_with(Default::default);
    if named.command.is_none() {
        named.command = role.agent.clone();
    }
    if named.model.is_none() {
        named.model = role.model.clone();
    }
    if named.effort.is_none() {
        named.effort = role.effort.clone();
    }
    if role.worktree == Some(false) && !args.no_worktree {
        args.no_worktree = true;
    }
}

/// Fills `args` from the role it names, silently.
///
/// `amx sub` calls this before inheriting from the parent, so the precedence is
/// flag, then role, then parent. Unknown roles and unreadable files are
/// reported later by `run_aloud`.
pub(crate) fn fill_role(dir: &Path, args: &mut NewArgs) -> Option<Role> {
    let name = args.role.clone()?;
    let (personal, project) = role_places(dir).ok()?;
    let (found, _) = role::for_name(&personal, &project, &name);
    let role = found?;
    fill_from_role(&role, args);
    Some(role)
}

/// How many generated ids to try before giving up.
const MAX_CLAIMS: usize = 8;

/// Claims an id by creating its directory, returning the id and the directory.
///
/// The non-recursive mkdir is the uniqueness check: of two concurrent spawns
/// only one can create it, and the loser has nothing to clean up.
pub(crate) fn claim(root: &Path, name: Option<&str>, task: &str) -> Result<(String, PathBuf)> {
    if let Some(name) = name {
        ids::validate_name(name, root)?;
        let dir = paths::agent_dir_in(root, name)?;
        if !make_dir(&dir)? {
            bail!("name {name:?} is already taken");
        }
        return Ok((name.to_string(), dir));
    }
    // `generate` skips existing directories, so a lost race is rare.
    for _ in 0..MAX_CLAIMS {
        let id = ids::generate(task, root)?;
        let dir = paths::agent_dir_in(root, &id)?;
        if make_dir(&dir)? {
            return Ok((id, dir));
        }
    }
    bail!(
        "could not claim an id for {task:?} under {} after {MAX_CLAIMS} tries",
        root.display()
    )
}

/// Cuts the tree and sets the spawn up, undoing every step on failure.
#[allow(clippy::too_many_arguments)]
fn start(
    root: &Path,
    agent_dir: &Path,
    dir: &Path,
    mut env: std::collections::BTreeMap<String, String>,
    config: &Config,
    args: &NewArgs,
    task: &str,
    lined: &str,
    launch: &Launch,
    lineage: &Lineage,
    id: &str,
    problems: &mut impl Write,
    to_terminal: bool,
) -> Result<()> {
    // Apply the harness env first: the trust store is located through it, and
    // a harness pointing claude at another config directory must seed that one.
    spawn::harness_env(&mut env, config, &launch.agent);

    let cut = cut_worktree(dir, id, config, args)?;
    let mut taken = Undo::default();
    let placed = set_up(
        root,
        agent_dir,
        dir,
        env,
        config,
        args,
        task,
        lined,
        launch,
        lineage,
        id,
        cut.as_ref(),
        &mut taken,
        problems,
        to_terminal,
    );
    if placed.is_err() {
        give_everything_back(dir, id, cut.as_ref(), taken, problems, to_terminal);
    }
    placed
}

/// Steps a spawn took after cutting its tree, for undo on a later failure.
#[derive(Default)]
struct Undo {
    /// The stash that carried the checkout's changes into the tree.
    carried: Option<String>,
    /// The vendor trust store a key was written into for the tree.
    trusted: Option<PathBuf>,
    placed: Option<(crate::tmux::Server, crate::tmux::PaneId)>,
}

/// Undoes a failed spawn, newest step first: the pane, the carried changes,
/// the trust key, then the tree and amx's branch.
///
/// If the carried changes cannot be applied back to the checkout, the tree is
/// kept, since it is the only copy, and the warning names it.
fn give_everything_back(
    dir: &Path,
    id: &str,
    cut: Option<&(PathBuf, worktree::Worktree)>,
    taken: Undo,
    problems: &mut impl Write,
    to_terminal: bool,
) {
    let mut say = |what: String| {
        let _ = warned(problems, &what, to_terminal);
    };
    if let Some((server, pane)) = &taken.placed
        && let Err(e) = server.kill_pane(pane)
    {
        say(format!("{e:#}"));
    }
    let Some((repo, tree)) = cut else {
        return;
    };
    if let Some(stash) = &taken.carried
        && let Err(e) = worktree::give_back(dir, stash)
    {
        say(format!(
            "{e:#}; your uncommitted changes are still in {}",
            tree.path.display()
        ));
        return;
    }
    if let Some(store) = &taken.trusted
        && let Err(e) = trust::forget_tree(store, &tree.path, now())
    {
        say(format!("{e:#}"));
    }
    if tree.path.exists() {
        take_back(repo, id, tree, problems, to_terminal);
    }
}

/// Everything from a cut tree to the written record, noting in `taken` what a
/// later failure must undo.
#[allow(clippy::too_many_arguments)]
fn set_up(
    root: &Path,
    agent_dir: &Path,
    dir: &Path,
    mut env: std::collections::BTreeMap<String, String>,
    config: &Config,
    args: &NewArgs,
    task: &str,
    lined: &str,
    launch: &Launch,
    lineage: &Lineage,
    id: &str,
    cut: Option<&(PathBuf, worktree::Worktree)>,
    taken: &mut Undo,
    problems: &mut impl Write,
    to_terminal: bool,
) -> Result<()> {
    let tree = cut.map(|(_, tree)| tree);
    let cwd = tree
        .map(|tree| tree.path.clone())
        .unwrap_or_else(|| dir.to_path_buf());
    if let Some((repo, tree)) = cut {
        furnish_the_tree(config, agent_dir, id, repo, tree, problems, to_terminal)?;
        // After furnishing, so a failed setup command discards a tree that
        // holds none of the user's changes.
        if args.with_changes {
            taken.carried = worktree::carry_changes(dir, &tree.path)?;
        }
    }
    if let Some(tree) = tree_to_trust(tree.map(|tree| tree.path.as_path()), dir, args.exec) {
        taken.trusted = trust_the_tree(config, &env, &launch.agent, tree, problems, to_terminal);
    }

    if !args.exec {
        dial_the_env(
            &mut env,
            registry::entry(&launch.agent),
            &launch.dials.model,
        );
    }
    // Last, so a harness env that sets the id cannot override it.
    env.insert(crate::hook::ID_ENV.to_string(), id.to_string());
    spawn::write_boot_env(agent_dir, &env)?;
    spawn::write_handoff(
        agent_dir,
        &Handoff {
            task: task.to_string(),
            command: launched(args, lined, launch, id, config.trust),
        },
    )?;

    let (server, pane) = place_boot(id, &cwd)?;
    taken.placed = Some((server.clone(), pane.clone()));

    let session = session_written(
        args.exec,
        spawn::opens_under_id(&launch.agent, &args.vendor_args),
        id,
    );

    spawn::record(
        root,
        &Meta {
            role: args.role.clone(),
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
            // A cut tree diffs against the commit it was cut from; an agent in
            // an existing directory against HEAD at spawn time. Commands have
            // no base.
            base: match tree {
                Some(tree) => Some(tree.base.clone()),
                None if !args.exec => worktree::head_commit(dir)?,
                None => None,
            },
            socket: server.socket().clone(),
            pane,
            bg: false,
            session,
            transcript: None,
            created: now(),
        },
    )?;
    Ok(())
}

/// The recorded `Meta::session`: the minted id when the vendor was started
/// under it, so vendors without hooks still have a session on record.
///
/// `None` for a command spawn and for a vendor with no start flag.
fn session_written(exec: bool, opens_under_id: bool, id: &str) -> Option<String> {
    (!exec && opens_under_id).then(|| id.to_string())
}

/// The recorded `Meta::agent`: the resolved vendor command, or `None` for a
/// command spawn, which runs no vendor.
fn vendor_written(exec: bool, agent: &str) -> Option<String> {
    (!exec).then(|| agent.to_string())
}

/// The recorded `Meta::model` or `Meta::effort`: the dial's value, or `None`
/// when it was left at [`registry::DEFAULT`] or the spawn is a command.
fn dial_written(exec: bool, dial: &str) -> Option<String> {
    (!exec && dial != registry::DEFAULT).then(|| dial.to_string())
}

/// The pane's argv: the `--exec` command, or the vendor command with the task.
///
/// `id` is the session a vendor with a start flag opens under. `trust` is the
/// config key; vendors that answer folder trust with a flag get it here, since
/// `spawn` has no config.
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

/// Adds the env vars through which `vendor` takes its model, if any. See
/// [`crate::vendor::env_dials`]. Only `new` sets these; a resumed session keeps
/// its model.
fn dial_the_env(
    env: &mut std::collections::BTreeMap<String, String>,
    vendor: Option<&Vendor>,
    model: &str,
) {
    if let Some(vendor) = vendor {
        let dials = crate::vendor::env_dials(vendor, model, env);
        env.extend(dials);
    }
}

/// The ref to cut the tree from: `--base`, else the config's `base`, else
/// `None` for the current HEAD.
fn cut_from<'a>(config: &'a Config, args: &'a NewArgs) -> Option<&'a str> {
    args.base.as_deref().or(config.base.as_deref())
}

/// Cuts the agent a worktree when one is wanted, returning the repository it
/// was cut from and the tree.
///
/// Never for `--exec`: a command runs against the checkout as it is. The repo
/// is returned because furnishing copies from it, and `git` asked from inside
/// the new tree would name the tree instead.
fn cut_worktree(
    dir: &Path,
    id: &str,
    config: &Config,
    args: &NewArgs,
) -> Result<Option<(PathBuf, worktree::Worktree)>> {
    // `--pr` and `--branch` need a checkout of their branch, so they cut a
    // tree even when the `worktrees` key is off.
    let asked_for_a_branch = args.pr.is_some() || args.branch.is_some();
    if args.exec || args.no_worktree || (!config.worktrees && !asked_for_a_branch) {
        return Ok(None);
    }
    if !dir.is_dir() {
        bail!("{} is not a directory to run in", dir.display());
    }
    // Outside a repository, run in place unless a PR or branch was asked for.
    let Some(repo) = worktree::repo_root(dir)? else {
        match (args.pr, args.branch.as_deref()) {
            (Some(number), _) => bail!(
                "--pr {number}: {} is not in a git repository",
                dir.display()
            ),
            (_, Some(name)) => bail!(
                "--branch {name}: {} is not in a git repository",
                dir.display()
            ),
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

/// A tree on the head branch of pull request `number`, fetched from origin.
fn cut_on_request(repo: &Path, id: &str, number: u64) -> Result<worktree::Worktree> {
    let head = crate::pr::request_head(repo, number)?;
    let name = request_branch(repo, &head, number);
    worktree::create_on(repo, id, &name, &format!("refs/pull/{number}/head"))
}

/// The local branch for a PR's tree.
///
/// The head branch's own name, which the PR column reads back, unless the PR
/// comes from a fork or a local branch already has that name. Then `pr-<N>`,
/// so the fetch never moves a local branch; see [`worktree::create_on`].
fn request_branch(repo: &Path, head: &crate::pr::PrHead, number: u64) -> String {
    match head.cross || here_already(repo, &head.branch) {
        true => format!("pr-{number}"),
        false => head.branch.clone(),
    }
}

/// A tree on branch `name`, local or fetched from origin.
///
/// A local branch is checked out as is, since fetching would move unpushed
/// commits. An `origin/` prefix is dropped. Fails if another tree already has
/// the branch checked out, or if neither side has it.
fn cut_on_branch(repo: &Path, id: &str, name: &str) -> Result<worktree::Worktree> {
    let name = name.strip_prefix("origin/").unwrap_or(name);
    if worktree::checked_out(repo, name)? {
        bail!("{name} is already checked out in another worktree");
    }
    if here_already(repo, name) {
        return worktree::create_on_local(repo, id, name);
    }
    match worktree::create_on(repo, id, name, name) {
        Ok(tree) => Ok(tree),
        Err(_) => bail!("{name} is not a branch here or on origin"),
    }
}

/// Whether the repository has a local branch `name`.
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

/// Copies and links the configured paths into a new tree, then runs the
/// configured setup commands.
///
/// A missing path is a warning. A failing setup command discards the tree and
/// fails the spawn, since a half-installed tree is worse than none.
fn furnish_the_tree(
    config: &Config,
    agent_dir: &Path,
    id: &str,
    repo: &Path,
    tree: &worktree::Worktree,
    problems: &mut impl Write,
    to_terminal: bool,
) -> Result<()> {
    // What setup commands cannot work out for themselves.
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
                warned(problems, &path, to_terminal)?;
            }
            Ok(())
        }
        Err(e) => {
            take_back(repo, id, tree, problems, to_terminal);
            Err(e)
        }
    }
}

/// Discards a tree the spawn will not use, with its branch when amx named it.
///
/// A user's branch checked out with `--branch` is kept. Failures are warnings;
/// the spawn has already failed.
fn take_back(
    repo: &Path,
    id: &str,
    tree: &worktree::Worktree,
    problems: &mut impl Write,
    to_terminal: bool,
) {
    let branch = worktree::named_by_amx(id, &tree.branch).then_some(tree.branch.as_str());
    if let Err(undone) = worktree::discard(repo, &tree.path, branch) {
        let _ = warned(problems, &format!("{undone:#}"), to_terminal);
    }
}

/// The tree whose folder-trust prompt amx may answer: the tree it cut, else
/// `dir` when it is a linked worktree.
///
/// A linked worktree covers `--no-worktree --dir <tree>` spawns, which is how
/// `workflow run` dispatches. A main checkout or plain directory is the user's
/// to trust, and a command meets no prompt.
fn tree_to_trust<'a>(cut: Option<&'a Path>, dir: &'a Path, exec: bool) -> Option<&'a Path> {
    if exec {
        return None;
    }
    cut.or_else(|| worktree::is_linked(dir).then_some(dir))
}

/// Marks `tree` trusted in the vendor's trust store so the agent skips the
/// folder-trust prompt.
///
/// Only for vendors that keep a store; flag-answered vendors are handled in
/// [`launched`]. A failed write is a warning, since the prompt can still be
/// answered by hand. Returns the store written, for undo.
fn trust_the_tree(
    config: &Config,
    env: &std::collections::BTreeMap<String, String>,
    agent: &str,
    tree: &Path,
    problems: &mut impl Write,
    to_terminal: bool,
) -> Option<PathBuf> {
    // The store is the user's file: write only with `trust = true`.
    if !config.trust {
        return None;
    }
    if !trust::writes_a_store(agent) {
        return None;
    }
    let store = trust::store_in(env)?;
    // The vendor resolves a tree to its repository, whose entry may cover it.
    let inherits = worktree::main_repo(tree).ok();
    match trust::seed(&store, tree, inherits.as_deref(), now()) {
        Ok(true) => Some(store),
        Ok(false) => None,
        Err(e) => {
            let _ = warned(problems, &format!("{e:#}"), to_terminal);
            None
        }
    }
}

/// Creates an agent directory, owner-only, returning false if it exists.
///
/// Not recursive: creating it is the id claim.
fn make_dir(dir: &Path) -> Result<bool> {
    use std::os::unix::fs::DirBuilderExt;
    match std::fs::DirBuilder::new().mode(paths::DIR_MODE).create(dir) {
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
            role: None,
            dir: None,
            no_worktree: false,
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
            context_brief: None,
            parent: None,
        }
    }

    /// An `--exec` spawn, which names no vendor.
    fn a_command(command: &str) -> NewArgs {
        NewArgs {
            task: Some(command.to_string()),
            file: None,
            edit: false,
            name: None,
            role: None,
            dir: None,
            no_worktree: false,
            base: None,
            branch: None,
            pr: None,
            with_changes: false,
            exec: true,
            agent: None,
            vendor_args: Vec::new(),
            context_brief: None,
            parent: None,
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
        // The vendor is resolved only here, so the record must keep it.
        let launch = Launch::resolve(&Config::default(), &spawn(None, [None; 3])).unwrap();
        assert_eq!(
            vendor_written(false, &launch.agent).as_deref(),
            Some("claude")
        );

        // A command spawn resolves a launch it never uses and records no vendor.
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
        // `default` overrides a configured dial for one spawn.
        let config = configured(Some("fable"), None, Some("max"));
        let launch = Launch::resolve(&config, &spawn(None, [Some(DEFAULT), None, None])).unwrap();

        assert_eq!(launch.dials.model, DEFAULT);
        assert_eq!(launch.dials.effort, "max", "and only that dial");
    }

    #[test]
    fn dials_the_vendor_would_refuse_are_refused_here_first() {
        // claude rejects unknown modes too; amx rejects them before any id or
        // pane exists.
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
    fn new_refuses_a_directory_that_is_gone() {
        // tmux silently opens a pane whose directory is missing in the home
        // directory. The refusal comes before anything is created.
        let root = tempfile::TempDir::new().unwrap();
        let gone = root.path().join("worktrees").join("t1");
        let (mut out, mut problems) = (Vec::new(), Vec::new());
        let refused = run_aloud(
            root.path(),
            &gone,
            std::collections::BTreeMap::new(),
            &Config::default(),
            &spawn(None, [None, None, None]),
            "tell me about the last commit",
            &mut out,
            &mut problems,
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(
            refused.contains("is not a directory to run in") && refused.contains("t1"),
            "{refused}"
        );
        assert!(
            std::fs::read_dir(root.path()).unwrap().count() == 0,
            "nothing was written under the root"
        );
    }

    #[test]
    fn a_refusal_is_yellow_on_a_terminal_and_plain_down_a_pipe() {
        // A bad dial is refused before any id or directory exists, on the
        // writer the verb was given.
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

        // A refusal is a warning (yellow), not a failure (red).
        let painted = refused(true);
        assert!(painted.starts_with("\u{1b}[33mamx new: "), "{painted:?}");
        assert!(painted.trim_end().ends_with("\u{1b}[39m"), "{painted:?}");
        assert!(painted.contains("acceptEdits"), "{painted:?}");
    }

    #[test]
    fn dials_a_full_model_name_is_taken_because_that_dial_is_open() {
        // The agent is named because a model no harness lists is refused. The
        // open model dial accepts a value outside its cycle.
        let launch = Launch::resolve(
            &Config::default(),
            &spawn(Some("claude"), [Some("claude-fable-5"), None, None]),
        )
        .unwrap();
        assert_eq!(launch.dials.model, "claude-fable-5");
    }

    /// A config listing each harness's models. pi gets an explicit list so no
    /// test runs the installed pi to list its models.
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

    /// A config giving `harness` extra arguments.
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
        // Without `--agent`, the harness that lists the model is launched.
        let config = listing(&[("pi", &["openai/gpt-5"])]);

        let launch = Launch::resolve(&config, &spawn(None, [Some("gpt-5"), None, None])).unwrap();
        assert_eq!(launch.agent, "pi", "claude lists no such word");
        assert_eq!(launch.dials.model, "gpt-5");

        let launch = Launch::resolve(&config, &spawn(None, [Some("haiku"), None, None])).unwrap();
        assert_eq!(launch.agent, "claude", "and this one is claude's alone");
    }

    #[test]
    fn a_model_several_harnesses_list_stays_with_the_configured_one() {
        // When both list it, the configured harness wins.
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
        // Configured harness first, then table order.
        let named = |config: &Config| {
            asked(config)
                .map(|vendor| vendor.name)
                .collect::<Vec<&str>>()
        };
        assert_eq!(
            named(&Config::default()),
            ["claude", "pi", "codex", "opencode"]
        );
        assert_eq!(
            named(&Config {
                agent: "pi --approve".to_string(),
                ..Config::default()
            }),
            ["pi", "claude", "codex", "opencode"],
            "the configured command, read as the harness it runs"
        );
        assert_eq!(
            named(&Config {
                agent: "mock-claude".to_string(),
                ..Config::default()
            }),
            ["claude", "pi", "codex", "opencode"],
            "an agent amx has no entry for is nobody in the table"
        );
    }

    #[test]
    fn a_model_no_harness_lists_is_refused_naming_what_each_takes() {
        // The refusal names what each harness accepts.
        let config = listing(&[("pi", &["openai/gpt-5"])]);

        let refusal =
            Launch::resolve(&config, &spawn(None, [Some("gpt-4"), None, None])).unwrap_err();

        assert!(refusal.contains("--model \"gpt-4\""), "{refusal}");
        assert!(
            refusal.contains("claude accepts fable, opus, sonnet, haiku"),
            "{refusal}"
        );
        assert!(refusal.contains("pi accepts openai/gpt-5"), "{refusal}");
    }

    #[test]
    fn a_harness_that_prints_its_models_is_named_by_how_many_and_what_prints_them() {
        // A printed listing is named by count and command; a configured list
        // is named in full.
        let printed: Vec<String> = (0..490).map(|n| format!("openai/model-{n}")).collect();
        let pi = registry::entry("pi").expect("an entry for pi");
        assert_eq!(
            takes(pi, &Config::default(), &printed),
            "pi lists 490 models (`pi --list-models`)"
        );

        let told = listing(&[("pi", &["openai/gpt-5"])]);
        assert_eq!(
            takes(pi, &told, &models::models_of(pi, &told)),
            "pi accepts openai/gpt-5"
        );

        let json = crate::vendor::second::BRANCHING;
        assert_eq!(
            takes(&json, &Config::default(), &printed[..3]),
            "second lists 3 models (`second list models`)"
        );
    }

    #[test]
    fn a_model_picks_no_harness_where_the_agent_was_named() {
        // `--agent` fixes the harness; the model does not change it.
        let config = listing(&[("pi", &["openai/gpt-5"])]);
        let launch =
            Launch::resolve(&config, &spawn(Some("pi"), [Some("haiku"), None, None])).unwrap();

        assert_eq!(launch.agent, "pi", "claude lists haiku and is not asked");
        assert_eq!(launch.dials.model, "haiku");
    }

    #[test]
    fn a_model_picks_no_harness_for_a_configured_agent_amx_knows_nothing_about() {
        // An unknown agent has no model list and no model dial, so the flag is
        // refused without asking claude.
        let config = Config {
            agent: "mock-claude".to_string(),
            ..Config::default()
        };

        let refusal =
            Launch::resolve(&config, &spawn(None, [Some("opus"), None, None])).unwrap_err();

        assert!(refusal.contains("mock-claude"), "{refusal}");
        assert!(
            refusal.contains("cannot set model"),
            "claude was never asked: {refusal}"
        );
    }

    #[test]
    fn the_sentinel_names_no_model_and_so_picks_no_harness() {
        // `--model default` passes no model, so it must not trigger a harness
        // lookup, which would refuse it.
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
        // Configured args apply however the harness was picked, once each.
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
        // A flag already in the harness args wins: the dial adds nothing.
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
        // A command runs against the checkout and its build, so it never gets a
        // tree, even where an agent spawn would fail to cut one.
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

    /// Runs git with no user or system config and a fixed identity.
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

    /// A repository with one commit on `main` and no origin.
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
    fn a_failed_place_takes_the_carried_work_back() {
        // `--with-changes` carried the work and a trust key was written, then
        // placing the pane failed. The undo restores the checkout and removes
        // the tree, branch and key.
        let dir = tempfile::TempDir::new().unwrap();
        let repo = a_repo(&dir);
        std::fs::write(repo.join("README.md"), "half an hour of work\n").unwrap();
        std::fs::write(repo.join("started.rs"), "fn started() {}\n").unwrap();
        let tree = worktree::create(&repo, "fix-login-a1b", None).unwrap();
        let carried = worktree::carry_changes(&repo, &tree.path).unwrap();
        assert!(carried.is_some());
        assert_eq!(setup(&repo, &["status", "--porcelain"]), "", "moved out");

        let store = dir.path().join("claude.json");
        std::fs::write(&store, "{}").unwrap();
        assert!(trust::seed(&store, &tree.path, None, 1_000).unwrap());

        let cut = (repo.clone(), tree.clone());
        let mut problems = Vec::new();
        give_everything_back(
            &repo,
            "fix-login-a1b",
            Some(&cut),
            Undo {
                carried,
                trusted: Some(store.clone()),
                placed: None,
            },
            &mut problems,
            false,
        );

        assert_eq!(
            std::fs::read_to_string(repo.join("README.md")).unwrap(),
            "half an hour of work\n"
        );
        assert!(repo.join("started.rs").exists(), "the new file too");
        assert!(!tree.path.exists(), "no tree");
        assert_eq!(
            setup(&repo, &["branch", "--list", &tree.branch]),
            "",
            "no branch"
        );
        assert!(
            !std::fs::read_to_string(&store)
                .unwrap()
                .contains(&*tree.path.to_string_lossy()),
            "and no key for it"
        );
        assert!(
            problems.is_empty(),
            "{}",
            String::from_utf8_lossy(&problems)
        );
    }

    #[test]
    fn a_failed_place_keeps_the_tree_when_the_work_will_not_go_back() {
        // The checkout changed the same line meanwhile, so the stash cannot be
        // applied back and the tree is kept.
        let dir = tempfile::TempDir::new().unwrap();
        let repo = a_repo(&dir);
        std::fs::write(repo.join("README.md"), "the carried work\n").unwrap();
        let tree = worktree::create(&repo, "fix-login-a1b", None).unwrap();
        let carried = worktree::carry_changes(&repo, &tree.path).unwrap();
        std::fs::write(repo.join("README.md"), "typed since\n").unwrap();

        let cut = (repo.clone(), tree.clone());
        let mut problems = Vec::new();
        give_everything_back(
            &repo,
            "fix-login-a1b",
            Some(&cut),
            Undo {
                carried,
                ..Undo::default()
            },
            &mut problems,
            false,
        );

        assert!(tree.path.exists(), "the tree holding the work stays");
        let said = String::from_utf8_lossy(&problems);
        assert!(
            said.contains(&format!(
                "your uncommitted changes are still in {}",
                tree.path.display()
            )),
            "{said}"
        );
    }

    #[test]
    fn a_request_never_moves_a_local_branch() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = a_repo(&dir);
        let origin = dir.path().join("origin.git");
        setup(
            dir.path(),
            &[
                "init",
                "--bare",
                "-q",
                "-b",
                "main",
                &origin.to_string_lossy(),
            ],
        );
        setup(
            &repo,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );
        setup(&repo, &["push", "-q", "origin", "main"]);

        // A PR opened from a branch named `perf-index`.
        setup(&repo, &["checkout", "-q", "-b", "theirs"]);
        std::fs::write(repo.join("theirs.rs"), "theirs\n").unwrap();
        setup(&repo, &["add", "theirs.rs"]);
        setup(&repo, &["commit", "-m", "their work"]);
        let commit = setup(&repo, &["rev-parse", "HEAD"]);
        setup(&repo, &["push", "-q", "origin", "HEAD:refs/pull/12/head"]);
        setup(&repo, &["checkout", "-q", "main"]);
        setup(&repo, &["branch", "-D", "theirs"]);

        // A local, unpushed `perf-index`.
        setup(&repo, &["checkout", "-q", "-b", "perf-index"]);
        std::fs::write(repo.join("mine.rs"), "mine\n").unwrap();
        setup(&repo, &["add", "mine.rs"]);
        setup(&repo, &["commit", "-m", "my unpushed work"]);
        let mine = setup(&repo, &["rev-parse", "HEAD"]);
        setup(&repo, &["checkout", "-q", "main"]);

        let head = crate::pr::PrHead {
            branch: "perf-index".to_string(),
            commit: commit.clone(),
            cross: false,
        };
        let name = request_branch(&repo, &head, 12);
        assert_eq!(name, "pr-12", "a name this checkout has is not the forge's");
        let tree = worktree::create_on(&repo, "review-a1b", &name, "refs/pull/12/head").unwrap();
        assert_eq!(setup(&tree.path, &["rev-parse", "HEAD"]), commit);
        assert_eq!(
            setup(&repo, &["rev-parse", "refs/heads/perf-index"]),
            mine,
            "and the person's branch is where they left it"
        );

        // A first agent commits on pr-12 and is stopped; a second request
        // spawn must not fetch the forge's tip over that commit.
        std::fs::write(tree.path.join("agent.rs"), "agent\n").unwrap();
        setup(&tree.path, &["add", "agent.rs"]);
        setup(&tree.path, &["commit", "-m", "the first agent's work"]);
        let first = setup(&tree.path, &["rev-parse", "HEAD"]);
        setup(&repo, &["worktree", "remove", &tree.path.to_string_lossy()]);

        assert_eq!(request_branch(&repo, &head, 12), "pr-12");
        let refused =
            worktree::create_on(&repo, "again-b2c", "pr-12", "refs/pull/12/head").unwrap_err();
        assert!(
            format!("{refused:#}").starts_with("pr-12 has commits that refs/pull/12/head does not"),
            "{refused:#}"
        );
        assert_eq!(
            setup(&repo, &["rev-parse", "refs/heads/pr-12"]),
            first,
            "the first agent's commit is still on pr-12"
        );
    }

    #[test]
    fn a_failed_setup_on_a_persons_branch_keeps_the_branch_and_its_commits() {
        // `--branch feature` on an existing local branch with an unpushed
        // commit. Setup fails; the tree goes and the branch stays.
        let dir = tempfile::TempDir::new().unwrap();
        let repo = a_repo(&dir);
        setup(&repo, &["checkout", "-b", "feature"]);
        std::fs::write(repo.join("wip.rs"), "wip\n").unwrap();
        setup(&repo, &["add", "wip.rs"]);
        setup(&repo, &["commit", "-m", "the person's unpushed work"]);
        let tip = setup(&repo, &["rev-parse", "HEAD"]);
        setup(&repo, &["checkout", "main"]);

        let tree = cut_on_branch(&repo, "fix-login-a1b", "feature").unwrap();
        let config = Config {
            setup: vec!["false".to_string()],
            ..Config::default()
        };
        let agent_dir = dir.path().join("agent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let mut problems = Vec::new();
        assert!(
            furnish_the_tree(
                &config,
                &agent_dir,
                "fix-login-a1b",
                &repo,
                &tree,
                &mut problems,
                false
            )
            .is_err()
        );

        assert!(!tree.path.exists(), "the tree is taken back");
        assert_eq!(
            setup(&repo, &["rev-parse", "refs/heads/feature"]),
            tip,
            "and the branch is still where the person left it"
        );
    }

    #[test]
    fn a_branch_another_tree_already_holds_starts_no_agent() {
        // git allows one worktree per branch, and the main checkout counts.
        let dir = tempfile::TempDir::new().unwrap();
        let repo = a_repo(&dir);

        let refusal = cut_on_branch(&repo, "fix-login-a1b", "main").unwrap_err();

        assert!(
            refusal.to_string().contains("main is already checked out"),
            "{refusal:#}"
        );
        assert_eq!(trees_in(&repo), 1, "and no tree was cut for it");
    }

    #[test]
    fn a_branch_that_is_neither_here_nor_on_the_origin_starts_no_agent() {
        // Neither a local branch nor a fetch from origin finds it.
        let dir = tempfile::TempDir::new().unwrap();
        let repo = a_repo(&dir);

        let refusal = cut_on_branch(&repo, "fix-login-a1b", "spike").unwrap_err();

        assert_eq!(
            refusal.to_string(),
            "spike is not a branch here or on origin",
            "{refusal:#}"
        );
        assert_eq!(trees_in(&repo), 1, "and no tree was cut for it");
    }

    #[test]
    fn a_branch_is_a_tree_whatever_the_key_says_and_nowhere_to_cut_one_is_refused() {
        // `--branch` cuts a tree even with `worktrees = false`, and outside a
        // repository it is refused.
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
        // The mkdir settles a race for one name: an existing directory is
        // another spawn's claim.
        let root = tempfile::TempDir::new().unwrap();
        let dir = root.path().join("fix-login-a1b");

        assert!(make_dir(&dir).unwrap(), "a free name is claimed");
        assert!(
            !make_dir(&dir).unwrap(),
            "a name somebody holds is not claimed again"
        );
    }

    /// A tree path inside a repository, and an env whose home holds the
    /// vendor's trust store.
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
    fn new_writes_a_model_carried_in_the_env_into_the_boot_env() {
        use crate::vendor::second::ELSEWHERE;

        let mut env = spawn::env_snapshot([]);
        dial_the_env(&mut env, Some(&ELSEWHERE), "large");
        assert_eq!(env.get("SECOND_CONFIG").unwrap(), r#"{"size":"large"}"#);

        // An existing value wins, and the default model writes nothing.
        let mut env = spawn::env_snapshot([("SECOND_CONFIG".to_string(), "{}".to_string())]);
        dial_the_env(&mut env, Some(&ELSEWHERE), "large");
        assert_eq!(env.get("SECOND_CONFIG").unwrap(), "{}");
        let mut env = spawn::env_snapshot([]);
        dial_the_env(&mut env, Some(&ELSEWHERE), registry::DEFAULT);
        assert!(env.is_empty());

        for vendor in [registry::entry("claude"), None] {
            let mut env = spawn::env_snapshot([]);
            dial_the_env(&mut env, vendor, "opus");
            assert!(env.is_empty());
        }
    }

    /// A config with `trust = true`.
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
        // pi answers folder trust too, but with a flag; it must not get a
        // `~/.claude.json` entry.
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
        // pi takes the trust answer as a flag on its argv.
        let args = spawn(Some("pi"), [None; 3]);
        let launch = Launch::resolve(&agreed(), &args).unwrap();

        assert_eq!(
            launched(&args, "port the importer", &launch, "port-it-b2c", true),
            [
                "pi",
                "--session-id",
                "port-it-b2c",
                "--approve",
                "--",
                "port the importer"
            ]
        );
        assert_eq!(
            launched(&args, "port the importer", &launch, "port-it-b2c", false),
            [
                "pi",
                "--session-id",
                "port-it-b2c",
                "--",
                "port the importer"
            ],
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

        // An unwritable store is one warning; the spawn continues.
        let told = String::from_utf8(problems).unwrap();
        assert!(told.contains("trust store amx can read"), "{told}");
        assert_eq!(told.lines().count(), 1, "{told}");
        assert!(told.starts_with("\u{1b}[33mamx new: "), "{told:?}");
    }

    #[test]
    fn trust_is_answered_for_the_linked_worktree_a_no_worktree_spawn_was_pointed_at() {
        // `workflow run` dispatches with `--no-worktree --dir <tree>`. Without
        // trust for that linked worktree, a claude agent stops at the prompt
        // with no hook to report it. A main checkout, a plain directory and a
        // command get no trust.
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
        // Recorded at spawn, since a vendor without hooks never reports one.
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
        // A config value the vendor cannot use is dropped, and the spawn goes
        // ahead.
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
