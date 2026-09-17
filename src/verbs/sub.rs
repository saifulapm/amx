//! `amx sub` — start a subagent and wait for its answer.
//!
//! A subagent is an ordinary amx agent whose record names a parent, and this
//! verb is `amx new` plus `amx result` in one call: a parent asks a question
//! and is handed the answer, without a second command and without knowing an
//! id it has not been told yet. The id goes to stderr on one line, the answer
//! to stdout, and the exit code is `result`'s — 0 an answer, 1 failed or
//! stopped, 2 the child is asking a question, 3 the caller's own deadline.
//!
//! What it adds over the two verbs is what makes a child a child. The parent's
//! directory is where it runs by default, so a scout sees the uncommitted work
//! the parent is asking about and needs no branch to answer one question. A
//! model and an effort the parent already chose are handed down when the child
//! runs the same vendor, since a claude model means nothing to a codex child.
//! The number of live children one parent may have is its own key, and a
//! child's `--permission` is an escalation the config has to allow.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use crate::cli::{AgentArgs, Context as StartContext, NewArgs, SubArgs};
use crate::config::Config;
use crate::store::{Agent, Meta, Phase};
use crate::verbs::{new, result};
use crate::{Severity, derive, exit, paths, registry, said, spawn, store};

/// The verb, against the machine's own state directory.
pub fn from_env(args: &SubArgs) -> Result<i32> {
    let root = paths::state_root()?;
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    run(&root, args, &mut out, &mut err)
}

/// The verb, with the state directory named.
pub fn run(root: &Path, args: &SubArgs, out: &mut impl Write, err: &mut impl Write) -> Result<i32> {
    let to_terminal = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let colours = std::io::IsTerminal::is_terminal(&std::io::stderr());

    let env = spawn::env_snapshot(std::env::vars());
    let parent = parent_of(root, &env, args.no_parent);

    if args.context == Some(StartContext::Digest) && parent.is_none() {
        writeln!(
            err,
            "{}",
            said(
                Severity::Warned,
                "amx sub: --context digest needs a parent, and there is none here",
                colours
            )
        )?;
        return Ok(exit::USAGE);
    }

    // The parent's directory unless the caller named one: a child is an
    // extension of the parent's work and shares the checkout it is about.
    let dir = match &args.dir {
        Some(dir) => paths::anchored(dir)?,
        None => parent
            .as_ref()
            .map(|meta| meta.dir.clone())
            .unwrap_or(std::env::current_dir().context("no working directory")?),
    };
    let (config, _) = crate::config::for_dir(&dir);
    let config = &config;

    let mut spawn_args = as_new(args, parent.is_some());

    // The role's dials before the parent's, so a role beats an inheritance and
    // a typed flag beats both. Its `worktree` stands where the verb's own
    // default would, unless the caller typed one.
    if let Some(role) = new::fill_role(&dir, &mut spawn_args)
        && !args.worktree
        && let Some(worktree) = role.worktree
    {
        spawn_args.no_worktree = !worktree;
    }
    match spawn_args
        .agent
        .as_ref()
        .and_then(|named| named.permission.as_deref())
    {
        Some(permission) if !config.subagents_may_escalate => {
            return refuse(
                err,
                colours,
                format!(
                    "amx sub: subagents_may_escalate is false, so --permission {permission:?} is refused"
                ),
            );
        }
        _ => {}
    }

    match inherit(config, parent.as_ref(), &mut spawn_args) {
        Ok(()) => {}
        Err(refusal) => {
            writeln!(
                err,
                "{}",
                said(Severity::Warned, &format!("amx sub: {refusal}"), colours)
            )?;
            return Ok(exit::USAGE);
        }
    }

    if args.context == Some(StartContext::Digest)
        && let Some(parent) = &parent
    {
        spawn_args.context_brief = Some(digest_of(parent));
    }

    if let Some(parent) = &parent
        && config.max_children != 0
        && live_children(root, &parent.id)? >= config.max_children
    {
        return refuse(
            err,
            colours,
            format!(
                "amx sub: max_children is {} and {} already has that many",
                config.max_children, parent.id
            ),
        );
    }

    // The same claim and start path `amx new` runs, with its id caught on the
    // way past rather than printed to the caller.
    let mut printed = Vec::new();
    let code = new::run(root, &dir, env, config, &spawn_args, &mut printed, err)?;
    if code != exit::OK {
        return Ok(code);
    }
    let id = String::from_utf8(printed)
        .context("the id amx printed is not text")?
        .trim()
        .to_string();

    if args.bg {
        return report(root, &id, None, args.json, out, err);
    }

    // The id first, so a caller reading a question on stdout knows who to
    // answer. `--json` says it in the object instead.
    if !args.json {
        writeln!(err, "{id}")?;
    }

    let mut answer = Vec::new();
    let code = result::run(
        root,
        &id,
        args.timeout.map(Duration::from_secs),
        to_terminal && !args.json,
        &mut answer,
    )?;

    if args.json {
        let answer = match code {
            exit::OK => Some(String::from_utf8_lossy(&answer).trim_end().to_string()),
            _ => None,
        };
        report(root, &id, answer, true, out, err)?;
    } else {
        out.write_all(&answer)?;
    }
    Ok(code)
}

/// The two spawns `amx sub` makes: the child that records a parent and the
/// top-level one a person's own shell gets.
///
/// A child shares the parent's directory — `--worktree` is the child that will
/// change something — while a spawn from outside a pane is an ordinary `amx
/// new` and cuts a tree by default.
fn as_new(args: &SubArgs, has_parent: bool) -> NewArgs {
    NewArgs {
        task: Some(args.task.clone()),
        file: None,
        edit: false,
        name: None,
        role: args.role.clone(),
        dir: None,
        no_worktree: has_parent && !args.worktree,
        no_parent: args.no_parent,
        base: None,
        branch: None,
        pr: None,
        with_changes: false,
        exec: false,
        agent: args.agent.clone(),
        vendor_args: args.vendor_args.clone(),
        context_brief: None,
    }
}

/// The short read of a parent a `--context digest` child is handed: its task,
/// and its latest word where the transcript has one.
///
/// A state rather than a log. The child can still read the whole of the
/// parent's conversation with `amx logs $AMX_PARENT`, which its pane names.
fn digest_of(parent: &Meta) -> String {
    let mut digest = format!(
        "Your parent agent, {}, is working on this task:\n\n{}",
        parent.id, parent.task
    );
    if let Some(format) =
        crate::conversation::format_of(parent.agent.as_deref().unwrap_or_default())
        && let Some(tail) = Agent::transcript_tail(parent)
        && let Some(words) = crate::conversation::answer(format, &tail)
    {
        digest.push_str(&format!("\n\nIts latest word on it:\n\n{words}"));
    }
    digest
}

/// The agent whose pane this was typed in, where `$AMX_ID` names one.
fn parent_of(root: &Path, env: &BTreeMap<String, String>, no_parent: bool) -> Option<Meta> {
    if no_parent {
        return None;
    }
    let id = env.get(crate::hook::ID_ENV)?;
    Agent::open(root, id).ok()?.meta().ok()
}

/// Hand the parent's dials down, where the child runs the same vendor.
///
/// The command first: a child of a pi agent is pi unless the caller says
/// otherwise. Then a claude parent hands its `model` and `effort` to a claude
/// child and says nothing to a `--agent codex` child, where a claude model
/// name means nothing. Anything the caller typed wins, and `--permission` is
/// not here at all: widening it is a decision rather than an inheritance.
fn inherit(config: &Config, parent: Option<&Meta>, spawn_args: &mut NewArgs) -> Result<(), String> {
    let Some(parent) = parent else {
        return Ok(());
    };
    let named = spawn_args.agent.clone().unwrap_or_default();
    // The vendor is the first dial a child inherits: a subagent of a pi agent
    // is pi. Only a default — an `--agent` names the command outright, and a
    // `--model` still picks the harness the model belongs to the way it does
    // for `new` — and it is the parent's whole command that rides, so a
    // `claude --add-dir ..` parent hands the flag down with the vendor.
    if named.command.is_none()
        && named.model.is_none()
        && let Some(command) = parent.agent.as_deref().filter(|agent| !agent.is_empty())
    {
        spawn_args.agent = Some(AgentArgs {
            command: Some(command.to_string()),
            model: None,
            permission: None,
            effort: None,
        });
    }
    // What this child would have launched as now, which is the vendor the
    // comparison is about.
    let launch = new::Launch::resolve(config, spawn_args)?;
    let theirs = registry::program(parent.agent.as_deref().unwrap_or_default());
    if registry::program(&launch.agent) != theirs {
        return Ok(());
    }

    let named = spawn_args.agent.clone().unwrap_or_default();
    spawn_args.agent = Some(AgentArgs {
        // Pinned to the vendor already resolved, so an inherited model cannot
        // send the child to a different harness than the comparison allowed.
        command: Some(launch.agent.clone()),
        model: named.model.or_else(|| parent.model.clone()),
        permission: named.permission,
        effort: named.effort.or_else(|| parent.effort.clone()),
    });
    Ok(())
}

/// How many children of `parent` have not reached a terminal phase.
fn live_children(root: &Path, parent: &str) -> Result<usize> {
    let mut count = 0;
    for id in store::list(root)? {
        let Ok(agent) = Agent::open(root, &id) else {
            continue;
        };
        let Ok(meta) = agent.meta() else {
            continue;
        };
        if meta.parent.as_deref() != Some(parent) {
            continue;
        }
        let phase = derive::view(root, &id, store::now())
            .map(|view| view.phase())
            .unwrap_or(Phase::Unknown);
        if !phase.is_terminal() {
            count += 1;
        }
    }
    Ok(count)
}

/// Write the child's id, and with `--json` the one object instead.
fn report(
    root: &Path,
    id: &str,
    answer: Option<String>,
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> Result<i32> {
    if !json {
        writeln!(err, "{id}")?;
        return Ok(exit::OK);
    }
    let view = derive::view(root, id, store::now())?;
    let object = serde_json::json!({
        "id": view.id(),
        "parent": view.meta.parent,
        "phase": view.phase().as_str(),
        "answer": answer,
        "evidence": view.verdict.evidence,
    });
    writeln!(out, "{}", serde_json::to_string(&object)?)?;
    Ok(exit::OK)
}

/// A refusal that is the answer rather than a failure: exit 2, the word
/// naming what was over the line.
fn refuse(err: &mut impl Write, colours: bool, message: String) -> Result<i32> {
    writeln!(err, "{}", said(Severity::Warned, &message, colours))?;
    Ok(exit::BLOCKED)
}
