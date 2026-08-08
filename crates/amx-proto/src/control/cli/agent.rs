//! Flags for the `agent …` verbs.
//!
//! 05's M2: "Agent addressing (name | pane UUID); `amx agent start <name>
//! --kind K` with readiness handshake" and "`amx agent prompt --wait`". The
//! four verbs a human drives are here; `agent.report` is not, because nothing
//! types a hook payload by hand — `amx _hook` builds it and `--params` is the
//! only way in, which is the right shape for a machine surface.

use std::path::PathBuf;

use amx_core::WorkspaceId;
use amx_core::agent::AgentKind;
use clap::{Arg, ArgMatches, Command};
use serde_json::Value;

use crate::control::Method;
use crate::control::agent::{ExplainParams, NextParams, PromptParams, PromptWait, StartParams};
use crate::control::pane::PaneTarget;

use super::flag::{self, FlagError, FlagRow, from_arg};

/// The name a started agent takes, which is also its address.
const NAME: &str = "name";
/// Which agent to start.
const KIND: &str = "kind";
/// Where to start it.
const CWD: &str = "cwd";
/// Which workspace to put it in.
const WORKSPACE: &str = "workspace";
/// Extra argv for the agent itself.
const ARGS: &str = "args";
/// What `agent prompt` waits for after submitting.
const WAIT: &str = "wait";

/// `amx agent start <NAME> --kind <AGENT>`.
pub(super) const START: FlagRow = FlagRow {
    method: Method::AgentStart,
    fields: &[
        from_arg("name", NAME),
        from_arg("kind", KIND),
        from_arg("args", ARGS),
        from_arg("cwd", CWD),
        from_arg("workspace", WORKSPACE),
        from_arg("timeout_ms", flag::TIMEOUT),
    ],
    args: start_args,
    parse: start,
};

/// `amx agent prompt <TARGET> --text <TEXT> [--wait blocked|idle]`.
pub(super) const PROMPT: FlagRow = FlagRow {
    method: Method::AgentPrompt,
    fields: &[
        from_arg("target", flag::TARGET),
        from_arg("text", flag::TEXT),
        from_arg("wait", WAIT),
        from_arg("timeout_ms", flag::TIMEOUT),
    ],
    args: prompt_args,
    parse: prompt,
};

/// `amx agent explain <TARGET>`.
pub(super) const EXPLAIN: FlagRow = FlagRow {
    method: Method::AgentExplain,
    fields: &[from_arg("target", flag::TARGET)],
    args: explain_args,
    parse: explain,
};

/// `amx agent next [--workspace <UUID>]`.
///
/// It took nothing and was listed anyway, so that the day it grew a parameter
/// the coverage test would fail here instead of the verb silently gaining an
/// unreachable field. That day is D15's scoped cycling: clearing one project's
/// queue must not yank focus across projects, so the verb learned a scope and
/// this row learned the flag that reaches it.
pub(super) const NEXT: FlagRow = FlagRow {
    method: Method::AgentNext,
    fields: &[from_arg("workspace", WORKSPACE)],
    args: next_args,
    parse: next,
};

fn start_args(command: Command) -> Command {
    command
        .arg(
            Arg::new(NAME)
                .value_name("NAME")
                .help("What to call it: the pane's label, and how you address it after this"),
        )
        .arg(
            Arg::new(KIND)
                .long("kind")
                .value_name("AGENT")
                .help("Which agent to start, by registry id or alias (`claude`, `codex`)"),
        )
        .arg(
            Arg::new(CWD)
                .long("cwd")
                .value_name("DIR")
                .help("Where to start it [default: where a split would land]"),
        )
        .arg(
            Arg::new(WORKSPACE)
                .long("workspace")
                .value_name("UUID")
                .help("Which workspace to put it in [default: the focused one]"),
        )
        .arg(flag::timeout_arg(
            "How long to wait for it to be ready [default: the server's 30s budget]",
        ))
        .arg(
            Arg::new(ARGS)
                .value_name("ARG")
                .num_args(0..)
                .last(true)
                .allow_hyphen_values(true)
                .help("Arguments appended to the agent's own argv, after a `--`"),
        )
}

fn start(matches: &ArgMatches) -> Result<Value, FlagError> {
    let kind = flag::required(matches, KIND, "--kind")?;
    let params = StartParams {
        name: flag::required(matches, NAME, "<NAME>")?.clone(),
        kind: AgentKind::new(kind.as_str())
            .map_err(|why| FlagError::invalid("--kind", why.to_string()))?,
        args: matches
            .get_many::<String>(ARGS)
            .map(|args| args.cloned().collect())
            .unwrap_or_default(),
        cwd: matches.get_one::<String>(CWD).map(PathBuf::from),
        workspace: flag::parse_opt::<WorkspaceId>(matches, WORKSPACE, "--workspace")?,
        timeout_ms: flag::timeout(matches)?,
    };
    flag::encode(&params)
}

fn prompt_args(command: Command) -> Command {
    command
        .arg(flag::target_arg(
            "Which agent, by pane UUID or by the name it was started with",
        ))
        .arg(flag::text_arg("The prompt to submit"))
        .arg(
            Arg::new(WAIT)
                .long("wait")
                .value_name("STATE")
                .num_args(0..=1)
                .default_missing_value("idle")
                .value_parser(["none", "blocked", "idle"])
                .help("Return only once the agent blocks or goes idle [bare `--wait`: idle]")
                .long_help(
                    "Return only once the agent is `blocked` (it needs an answer) or \
                     `idle` (it finished). A bare `--wait` means `idle`.\n\n\
                     The wait is anchored after the submission, so an agent that was \
                     already blocked when you called does not satisfy it.",
                ),
        )
        .arg(flag::timeout_arg(
            "How long to wait for that [default: no limit]",
        ))
}

fn prompt(matches: &ArgMatches) -> Result<Value, FlagError> {
    let params = PromptParams {
        target: PaneTarget::new(flag::required(matches, flag::TARGET, "<TARGET>")?.as_str()),
        text: flag::required(matches, flag::TEXT, "--text")?.clone(),
        wait: match matches.get_one::<String>(WAIT).map(String::as_str) {
            None | Some("none") => PromptWait::None,
            Some("blocked") => PromptWait::Blocked,
            Some("idle") => PromptWait::Idle,
            Some(other) => {
                return Err(FlagError::invalid(
                    "--wait",
                    format!("{other:?} is not one of none, blocked, idle"),
                ));
            }
        },
        timeout_ms: flag::timeout(matches)?,
    };
    flag::encode(&params)
}

fn next_args(command: Command) -> Command {
    command.arg(
        Arg::new(WORKSPACE)
            .long("workspace")
            .value_name("UUID")
            .help("Cycle only this workspace's blocked agents [default: the whole queue]"),
    )
}

fn next(matches: &ArgMatches) -> Result<Value, FlagError> {
    let params = NextParams {
        workspace: flag::parse_opt::<WorkspaceId>(matches, WORKSPACE, "--workspace")?,
    };
    flag::encode(&params)
}

fn explain_args(command: Command) -> Command {
    command.arg(flag::target_arg(
        "Which pane's detection to explain, by UUID or by name",
    ))
}

fn explain(matches: &ArgMatches) -> Result<Value, FlagError> {
    let params = ExplainParams {
        target: PaneTarget::new(flag::required(matches, flag::TARGET, "<TARGET>")?.as_str()),
    };
    flag::encode(&params)
}
