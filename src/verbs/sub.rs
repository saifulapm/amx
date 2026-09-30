//! `amx sub`: start a subagent and wait for its answer.
//!
//! `amx new` plus `amx result` in one call, for an agent whose record names a
//! parent. The id goes to stderr and the answer to stdout, with `result`'s exit
//! codes; `--bg` prints the id on stdout and returns without waiting. A child
//! runs in its parent's directory by default and inherits the parent's vendor,
//! and its model and effort when the vendor matches. `max_children` caps live
//! children per parent, and `--permission` needs `subagents_may_escalate`.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use crate::cli::{AgentArgs, Context as StartContext, NewArgs, SubArgs};
use crate::config::Config;
use crate::store::{Agent, Meta};
use crate::verbs::{new, result, wait};
use crate::{Severity, derive, exit, paths, registry, said, spawn, store};

/// Run the verb against the machine.
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

    let mut env = spawn::env_snapshot(std::env::vars());
    // `new` reads the lineage off `AMX_ID`, so a named parent goes there too.
    if let Some(id) = &args.parent {
        if Agent::open(root, id)
            .and_then(|agent| agent.meta())
            .is_err()
        {
            writeln!(
                err,
                "{}",
                said(
                    Severity::Warned,
                    &format!("amx sub: no agent `{id}` to use as the parent"),
                    colours
                )
            )?;
            return Ok(exit::USAGE);
        }
        env.insert(crate::hook::ID_ENV.to_string(), id.clone());
    }
    let parent = parent_of(root, &env, args.no_parent);

    if args.context == Some(StartContext::Digest) && parent.is_none() {
        writeln!(
            err,
            "{}",
            said(
                Severity::Warned,
                "amx sub: --context digest needs a parent agent, and this is not running in one",
                colours
            )
        )?;
        return Ok(exit::USAGE);
    }

    let dir = match &args.dir {
        Some(dir) => paths::anchored(dir)?,
        None => parent
            .as_ref()
            .map(|meta| meta.dir.clone())
            .unwrap_or(std::env::current_dir().context("no working directory")?),
    };
    // A stopped parent's worktree may be gone. Refused here so the message
    // names the parent, which is what the caller typed.
    if args.dir.is_none()
        && let Some(parent) = &parent
        && !dir.is_dir()
    {
        writeln!(
            err,
            "{}",
            said(
                Severity::Warned,
                &format!(
                    "amx sub: {} ran in {}, which no longer exists; pass a directory with --dir",
                    parent.id,
                    dir.display()
                ),
                colours
            )
        )?;
        return Ok(exit::USAGE);
    }
    let (config, _) = crate::config::for_dir(&dir);
    let config = &config;

    let mut spawn_args = as_new(args, parent.as_ref());

    // Role before inheritance, so a role beats the parent and a typed flag
    // beats both. The role's `worktree` replaces the default, not a typed flag.
    if let Some(role) = new::fill_role(&dir, &mut spawn_args)
        && !args.worktree
        && !args.no_worktree
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
                "amx sub: max_children is {} and {} already has that many children",
                config.max_children, parent.id
            ),
        );
    }

    // `new` prints the id; catch it rather than pass it through.
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
        return report(root, &id, None, args.json, out);
    }

    // The id first, so a caller that gets a question knows who to answer.
    // `--json` carries it in the object instead.
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
        report(root, &id, answer, true, out)?;
    } else {
        out.write_all(&answer)?;
    }
    Ok(code)
}

/// The `new` arguments for this spawn.
///
/// The only place a parent is handed to `new`. A child shares its parent's
/// directory unless `--worktree` is given; a parentless spawn cuts a tree
/// unless `--no-worktree` is given, as `amx new` does.
fn as_new(args: &SubArgs, parent: Option<&Meta>) -> NewArgs {
    let has_parent = parent.is_some();
    NewArgs {
        task: Some(args.task.clone()),
        file: None,
        edit: false,
        name: args.name.clone(),
        role: args.role.clone(),
        dir: None,
        no_worktree: args.no_worktree || (has_parent && !args.worktree),
        base: None,
        branch: None,
        pr: None,
        with_changes: false,
        exec: false,
        agent: args.agent.clone(),
        vendor_args: args.vendor_args.clone(),
        context_brief: None,
        parent: parent.map(|parent| parent.id.clone()),
    }
}

/// The brief a `--context digest` child gets: the parent's task and, where its
/// transcript has one, its latest answer.
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
        digest.push_str(&format!("\n\nIts latest answer:\n\n{words}"));
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

/// Hand the parent's vendor down, and its model and effort when the child runs
/// the same vendor.
///
/// Anything the caller typed wins. `--permission` is never inherited.
fn inherit(config: &Config, parent: Option<&Meta>, spawn_args: &mut NewArgs) -> Result<(), String> {
    let Some(parent) = parent else {
        return Ok(());
    };
    let named = spawn_args.agent.clone().unwrap_or_default();
    // Only when neither `--agent` nor `--model` was given, since a model picks
    // its own harness. The parent's whole command rides, flags included.
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
    let launch = new::Launch::resolve(config, spawn_args)?;
    let theirs = registry::program(parent.agent.as_deref().unwrap_or_default());
    if registry::program(&launch.agent) != theirs {
        return Ok(());
    }

    let named = spawn_args.agent.clone().unwrap_or_default();
    spawn_args.agent = Some(AgentArgs {
        // Pinned, so an inherited model cannot move the child to another
        // harness.
        command: Some(launch.agent.clone()),
        model: named.model.or_else(|| parent.model.clone()),
        permission: named.permission,
        effort: named.effort.or_else(|| parent.effort.clone()),
    });
    Ok(())
}

/// How many children of `parent` have not reached a terminal phase.
fn live_children(root: &Path, parent: &str) -> Result<usize> {
    Ok(wait::children_of(root, parent)?
        .iter()
        .filter(|id| {
            derive::view(root, id, store::now()).map_or(true, |view| !view.phase().is_terminal())
        })
        .count())
}

/// Print the child's id, or with `--json` the child as one object.
fn report(
    root: &Path,
    id: &str,
    answer: Option<String>,
    json: bool,
    out: &mut impl Write,
) -> Result<i32> {
    if !json {
        writeln!(out, "{id}")?;
        return Ok(exit::OK);
    }
    let view = derive::view(root, id, store::now())?;
    let mut object = result::answer_json(&view, answer);
    object["id"] = serde_json::json!(view.id());
    object["parent"] = serde_json::json!(view.meta.parent);
    writeln!(out, "{}", serde_json::to_string(&object)?)?;
    Ok(exit::OK)
}

/// Refuse with `BLOCKED`, naming the limit that was hit.
fn refuse(err: &mut impl Write, colours: bool, message: String) -> Result<i32> {
    writeln!(err, "{}", said(Severity::Warned, &message, colours))?;
    Ok(exit::BLOCKED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Kind, Phase};
    use crate::tmux::{PaneId, Socket};

    #[test]
    fn sub_json_on_a_question_carries_what_answer_needs() {
        // The object is the caller's only pipe, so it carries the choices
        // and the kind of question.
        let root = tempfile::TempDir::new().unwrap();
        let meta = Meta {
            role: None,
            parent: Some("lead-a1b".to_string()),
            depth: 1,
            id: "scout-c3d".to_string(),
            task: "find the flaky test".to_string(),
            agent: Some("claude".to_string()),
            model: None,
            effort: None,
            dir: std::path::PathBuf::from("/srv/app"),
            worktree: None,
            branch: None,
            base: None,
            socket: Socket::Name(format!("amx-no-such-server-{}", std::process::id())),
            pane: PaneId::new("%404").unwrap(),
            bg: false,
            session: None,
            transcript: None,
            created: 1,
        };
        let agent = Agent::create(root.path(), &meta).unwrap();
        agent
            .writer()
            .unwrap()
            .observe(|state| {
                state.state = Phase::Waiting;
                state.question = Some("Which runner?".to_string());
                state.options = vec!["Node".to_string(), "Deno".to_string()];
                state.kind = Some(Kind::Question);
                // Parked, so the reading is the record's phase.
                state.parked_at = 4_600;
            })
            .unwrap();

        let mut out = Vec::new();
        report(root.path(), "scout-c3d", None, true, &mut out).unwrap();

        // Without --json, the id alone on stdout, as `amx new` prints it.
        let mut id = Vec::new();
        report(root.path(), "scout-c3d", None, false, &mut id).unwrap();
        assert_eq!(String::from_utf8(id).unwrap(), "scout-c3d\n");

        let object: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(object["phase"], "waiting");
        assert_eq!(object["question"], "Which runner?");
        assert_eq!(object["options"], serde_json::json!(["Node", "Deno"]));
        assert_eq!(object["kind"], "question");
        assert_eq!(object["parent"], "lead-a1b");
        let keys: Vec<&str> = object
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "answer", "evidence", "id", "kind", "options", "parent", "phase", "question"
            ]
        );
    }
}
