//! The hook block both agents keep, and the surgery amx performs on it.
//!
//! Claude Code and Codex CLI arrived at the same configuration shape from
//! different directions, and V01 measured both of them running from it:
//!
//! ```json
//! { "hooks": { "PreToolUse": [ { "matcher": "*",
//!                                "hooks": [ { "type": "command",
//!                                             "command": "…" } ] } ] } }
//! ```
//!
//! Claude Code keeps it inside `settings.json` beside `permissions` and the
//! rest; Codex keeps it as the whole of `$CODEX_HOME/hooks.json`. The event
//! keys are the same PascalCase names, and the entry shape is the same, so the
//! editing lives here once and the per-agent modules contribute only a path, a
//! matcher policy, and the caveats their agent earns.
//!
//! Which of the entries in that block are amx's is [`super::command`]'s
//! question; this module only walks the JSON and asks it.
//!
//! # Everything else in the file is untouchable
//!
//! Every walk below is conservative in the same direction: a `hooks` block that
//! is not an object, an event whose value is not an array, a group that is not
//! an object — each is an error that leaves the file exactly as it was found.
//! amx would rather refuse to install than reshape a configuration it does not
//! understand, because the thing it does not understand is the thing the user
//! wrote by hand.

use std::path::PathBuf;

use amx_core::agent::AgentKind;
use anyhow::{Context as _, bail};

use super::command;
use super::edit::{Json, Object};

/// The key both agents keep their subscriptions under.
const HOOKS: &str = "hooks";

/// The optional tool-name filter on a group of entries.
const MATCHER: &str = "matcher";

/// The discriminator on a handler: amx only ever writes `command` handlers.
const TYPE: &str = "type";

/// A `command` handler's command line.
const COMMAND: &str = "command";

/// One event amx wants an entry for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Subscription {
    /// The event name, exactly as the agent spells it.
    pub event: String,
    /// The tool-name matcher this event's entry takes, where the agent's
    /// schema has one. `None` means the entry is written without the key,
    /// which is how V01's measured configuration subscribed every event that
    /// is not scoped to a tool.
    pub matcher: Option<String>,
}

/// One amx entry found in a configuration file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Installed {
    /// The event it is subscribed to.
    pub event: String,
    /// The command string, verbatim.
    pub command: String,
    /// The program the command runs.
    pub program: PathBuf,
    /// The version marker it carries, if it carries one.
    pub marker: Option<u32>,
}

/// Every amx entry for `agent` in `document`, in document order.
#[must_use]
pub fn installed(document: &Json, agent: &AgentKind) -> Vec<Installed> {
    let mut found = Vec::new();
    let Some(block) = document.as_object().and_then(|root| root.get(HOOKS)) else {
        return found;
    };
    let Some(block) = block.as_object() else {
        return found;
    };
    for (event, groups) in block.iter() {
        for group in groups.as_array().unwrap_or_default() {
            for handler in handlers(group) {
                let Some(line) = handler.as_object().and_then(|h| h.get(COMMAND)) else {
                    continue;
                };
                let Some(line) = line.as_str() else {
                    continue;
                };
                if let Some((program, marker)) = command::parse(line, agent) {
                    found.push(Installed {
                        event: event.to_owned(),
                        command: line.to_owned(),
                        program,
                        marker,
                    });
                }
            }
        }
    }
    found
}

/// Make `document` subscribe exactly `subs` to `command`, and nothing else.
///
/// Idempotent by construction: a group that holds amx's entries and only amx's
/// entries is rewritten *in place*, so a second install over a current one
/// produces the identical document and the caller writes no file at all. Entries
/// for events that are no longer wanted — the registry's list is measured data
/// and will change when the measurement does — are removed by the same pass.
pub fn apply(
    document: &mut Json,
    agent: &AgentKind,
    command: &str,
    subs: &[Subscription],
) -> anyhow::Result<()> {
    let root = document
        .as_object_mut()
        .context("the configuration is not a JSON object")?;
    let block = root.entry(HOOKS, Json::object());
    rewrite(block, agent, Some((command, subs)))?;
    Ok(())
}

/// Remove every amx entry for `agent`, and nothing else.
///
/// Returns how many entries went. Containers amx emptied go with them — an
/// event key whose only group was amx's, and the `hooks` block itself if amx
/// wrote the whole of it — while a container that was already empty is left
/// alone: uninstall removes what install wrote, not what it found.
pub fn strip(document: &mut Json, agent: &AgentKind) -> anyhow::Result<usize> {
    let root = document
        .as_object_mut()
        .context("the configuration is not a JSON object")?;
    // No block at all is not an error and not an edit: there is nothing of
    // amx's in a file that has never had hooks in it.
    let Some(block) = root.get_mut(HOOKS) else {
        return Ok(0);
    };
    let removed = rewrite(block, agent, None)?;
    if removed > 0 && block.as_object().is_some_and(Object::is_empty) {
        root.remove(HOOKS);
    }
    Ok(removed)
}

/// The one walk `apply` and `strip` are both spellings of.
///
/// `wanted` is `None` for an uninstall and `Some` for an install; the only
/// difference between the two is whether an all-amx group is replaced with a
/// fresh entry or dropped.
fn rewrite(
    block: &mut Json,
    agent: &AgentKind,
    wanted: Option<(&str, &[Subscription])>,
) -> anyhow::Result<usize> {
    let block = block
        .as_object_mut()
        .context("`hooks` is not a JSON object")?;
    let events: Vec<String> = block.keys().map(str::to_owned).collect();
    let mut removed = 0;
    let mut placed: Vec<&str> = Vec::new();

    for event in &events {
        let desired = wanted.and_then(|(command, subs)| {
            subs.iter()
                .find(|sub| &sub.event == event)
                .map(|sub| (command, sub))
        });
        let Some(value) = block.get_mut(event) else {
            continue;
        };
        let groups = value
            .as_array_mut()
            .with_context(|| format!("`hooks.{event}` is not a JSON array"))?;

        let mut kept: Vec<Json> = Vec::with_capacity(groups.len());
        let mut dropped = 0;
        let mut done = false;
        for group in std::mem::take(groups) {
            let (ours, theirs) = count(&group, agent)?;
            if ours == 0 {
                kept.push(group);
                continue;
            }
            dropped += ours;
            if theirs > 0 {
                // A group the user merged amx's entry into: their handlers stay
                // exactly where they are, and only amx's leave.
                kept.push(without_ours(group, agent)?);
                continue;
            }
            // An all-amx group is amx's slot: the first one becomes the entry
            // this build writes, in place, so a reinstall keeps its position
            // and produces a byte-identical document. A second one is a
            // leftover from an older install and goes — one entry per event is
            // the whole design.
            if let (Some((command, sub)), false) = (desired, done) {
                kept.push(entry(sub, command));
                done = true;
            }
        }

        removed += dropped;
        if kept.is_empty() && dropped > 0 {
            block.remove(event);
        } else if let Some(value) = block.get_mut(event) {
            *value = Json::Array(kept);
        }
        if done {
            placed.push(event.as_str());
        }
    }

    if let Some((command, subs)) = wanted {
        for sub in subs {
            if placed.contains(&sub.event.as_str()) {
                continue;
            }
            let groups = block.entry(&sub.event, Json::Array(Vec::new()));
            let groups = groups
                .as_array_mut()
                .with_context(|| format!("`hooks.{}` is not a JSON array", sub.event))?;
            groups.push(entry(sub, command));
        }
    }
    Ok(removed)
}

/// How many of a group's handlers are amx's, and how many are not.
fn count(group: &Json, agent: &AgentKind) -> anyhow::Result<(usize, usize)> {
    if group.as_object().is_none() {
        bail!("a hook entry is not a JSON object");
    }
    let mut ours = 0;
    let mut theirs = 0;
    for handler in handlers(group) {
        if is_ours(handler, agent) {
            ours += 1;
        } else {
            theirs += 1;
        }
    }
    Ok((ours, theirs))
}

/// `group` with amx's handlers removed and everything else where it was.
fn without_ours(group: Json, agent: &AgentKind) -> anyhow::Result<Json> {
    let Json::Object(object) = group else {
        bail!("a hook entry is not a JSON object");
    };
    let members = object
        .iter()
        .map(|(key, value)| {
            let value = match (key, value.as_array()) {
                (HOOKS, Some(items)) => Json::Array(
                    items
                        .iter()
                        .filter(|handler| !is_ours(handler, agent))
                        .cloned()
                        .collect(),
                ),
                _ => value.clone(),
            };
            (key.to_owned(), value)
        })
        .collect();
    Ok(Json::Object(members))
}

/// Whether one handler is an amx invocation for `agent`.
fn is_ours(handler: &Json, agent: &AgentKind) -> bool {
    handler
        .as_object()
        .and_then(|object| object.get(COMMAND))
        .and_then(Json::as_str)
        .is_some_and(|text| command::parse(text, agent).is_some())
}

/// A group's handler list, or nothing if it has none.
fn handlers(group: &Json) -> &[Json] {
    group
        .as_object()
        .and_then(|object| object.get(HOOKS))
        .and_then(Json::as_array)
        .unwrap_or_default()
}

/// The entry amx writes for one subscription.
fn entry(sub: &Subscription, command: &str) -> Json {
    let mut members: Vec<(String, Json)> = Vec::with_capacity(2);
    // The matcher first, the way both agents' own documentation writes it.
    if let Some(matcher) = &sub.matcher {
        members.push((MATCHER.to_owned(), Json::String(matcher.clone())));
    }
    let handler: Object = [
        (TYPE.to_owned(), Json::String(COMMAND.to_owned())),
        (COMMAND.to_owned(), Json::String(command.to_owned())),
    ]
    .into_iter()
    .collect();
    members.push((HOOKS.to_owned(), Json::Array(vec![Json::Object(handler)])));
    Json::Object(members.into_iter().collect())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

    use std::path::Path;

    use amx_core::agent::AgentKind;

    use super::super::command;
    use super::super::edit::Json;
    use super::{Subscription, apply, installed, strip};

    fn claude() -> AgentKind {
        AgentKind::new("claude").expect("a valid id")
    }

    fn subs() -> Vec<Subscription> {
        vec![
            Subscription {
                event: "Stop".to_owned(),
                matcher: None,
            },
            Subscription {
                event: "PreToolUse".to_owned(),
                matcher: Some("*".to_owned()),
            },
        ]
    }

    fn parse_json(text: &str) -> Json {
        serde_json::from_str(text).expect("parse")
    }

    #[test]
    fn a_second_apply_produces_the_identical_document() {
        let mut document = parse_json(r#"{"model":"opus"}"#);
        let line = command::build(Path::new("/usr/bin/amx"), &claude(), 1);
        apply(&mut document, &claude(), &line, &subs()).expect("install");
        let once = document.clone();
        apply(&mut document, &claude(), &line, &subs()).expect("reinstall");
        assert_eq!(document, once, "reinstalling changed the document");
    }

    #[test]
    fn a_stale_entry_is_replaced_where_it_stands() {
        let mut document = parse_json(
            r#"{"hooks":{"Stop":[
                 {"hooks":[{"type":"command","command":"/old/amx _hook claude --marker 0"}]},
                 {"hooks":[{"type":"command","command":"/usr/bin/notify-send done"}]}]}}"#,
        );
        let line = command::build(Path::new("/usr/bin/amx"), &claude(), 1);
        apply(&mut document, &claude(), &line, &subs()).expect("install");

        let groups = document
            .as_object()
            .and_then(|root| root.get("hooks"))
            .and_then(Json::as_object)
            .and_then(|block| block.get("Stop"))
            .and_then(Json::as_array)
            .expect("the Stop entries");
        assert_eq!(groups.len(), 2, "the foreign entry stayed: {groups:?}");
        // Position preserved: amx's entry is still first, updated in place.
        assert!(format!("{:?}", groups[0]).contains("--marker 1"));
        assert!(format!("{:?}", groups[1]).contains("notify-send"));
    }

    #[test]
    fn stripping_leaves_a_foreign_only_file_untouched() {
        let text = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"beep"}]}]}}"#;
        let mut document = parse_json(text);
        assert_eq!(strip(&mut document, &claude()).expect("strip"), 0);
        assert_eq!(document, parse_json(text));
    }

    #[test]
    fn stripping_removes_the_containers_it_emptied_and_no_others() {
        let mut document = parse_json(r#"{"model":"opus","hooks":{"Quiet":[]}}"#);
        let line = command::build(Path::new("/usr/bin/amx"), &claude(), 1);
        apply(&mut document, &claude(), &line, &subs()).expect("install");
        assert_eq!(installed(&document, &claude()).len(), 2);

        assert_eq!(strip(&mut document, &claude()).expect("strip"), 2);
        // `Quiet` was the user's empty array, so it stays — and with it the
        // block that holds it.
        assert_eq!(
            document,
            parse_json(r#"{"model":"opus","hooks":{"Quiet":[]}}"#)
        );
    }

    #[test]
    fn a_hook_block_of_the_wrong_shape_is_refused_not_reshaped() {
        let mut document = parse_json(r#"{"hooks":42}"#);
        let line = command::build(Path::new("/usr/bin/amx"), &claude(), 1);
        let err = apply(&mut document, &claude(), &line, &subs()).expect_err("refused");
        assert!(err.to_string().contains("not a JSON object"), "{err}");
    }
}
