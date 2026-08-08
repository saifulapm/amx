//! The typed flag layer, and the law that keeps it honest.
//!
//! A row's parameters come from one place, so the danger is drift: a payload
//! grows a field, `--params` can still reach it, and the flag form quietly
//! cannot. Three assertions close that off, and between them a new field has
//! nowhere to hide:
//!
//! 1. the row's parse builds its payload with a struct literal, so a new field
//!    fails the **build** until somebody handles it;
//! 2. [`fixtures`] below builds every flagged payload the same way and
//!    serializes it, and the keys that come out must be exactly the fields the
//!    row declares — so a field handled as `None` and forgotten fails **here**;
//! 3. every declared field must name an argument the leaf really carries, and
//!    every argument the leaf carries must be named by a field — so a flag that
//!    writes nothing, or a field pointing at an argument nobody added, fails
//!    here too.
//!
//! A field that genuinely should not be a flag is declared
//! [`Source::ParamsOnly`](amx_proto::control::cli::Source) with its reason,
//! which satisfies (2) without inventing a flag for it. That is a decision
//! somebody writes down, which is the whole point.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::collections::BTreeSet;
use std::path::PathBuf;

use amx_core::WorkspaceId;
use amx_core::agent::AgentKind;
use amx_proto::control::cli::{
    Field, PARAMS, ROWS, Source, method_commands, millis, params_from_flags, row,
};
use amx_proto::control::{Method, SPECS, agent, pane, session, wait};
use clap::{ArgMatches, Command};
use serde_json::{Value, json};

// --------------------------------------------------------------- the fixtures

/// Every flagged row's payload, with **every** field populated.
///
/// Struct literals on purpose: `..Default::default()` would let a new field in
/// without a word from anybody, which is the exact failure this file exists to
/// prevent. Populated on purpose too — the payloads skip serializing their
/// absent optionals, so a fixture that left one out would prove nothing about
/// it.
///
/// `pane.wait_output` carries both `match` and `regex` here even though the
/// call demands exactly one: this is a question about field *names*, and the
/// exactly-one rule is asserted where it lives, over a parse.
fn fixtures() -> Vec<(Method, Value)> {
    let target = || pane::PaneTarget::new("build");
    vec![
        (
            Method::SessionHandoff,
            json(&session::HandoffParams {
                binary: PathBuf::from("/home/s/.local/state/amx/staged/amx"),
                timeout_ms: Some(30_000),
            }),
        ),
        (
            Method::Wait,
            json(&wait::WaitParams {
                until: wait::WaitUntil::Blocked,
                target: target(),
                timeout_ms: Some(1_000),
            }),
        ),
        (
            Method::PaneWaitOutput,
            json(&wait::WaitOutputParams {
                target: target(),
                match_text: Some("tests passed".to_owned()),
                regex: Some("tests? passed".to_owned()),
                timeout_ms: Some(1_000),
            }),
        ),
        (
            Method::AgentStart,
            json(&agent::StartParams {
                name: "review".to_owned(),
                kind: AgentKind::new("claude").expect("a well-formed id"),
                args: vec!["--model".to_owned()],
                cwd: Some(PathBuf::from("/tmp")),
                workspace: Some(WorkspaceId::new_v4()),
                timeout_ms: Some(1_000),
            }),
        ),
        (
            Method::AgentPrompt,
            json(&agent::PromptParams {
                target: target(),
                text: "summarise the failing test".to_owned(),
                wait: agent::PromptWait::Blocked,
                timeout_ms: Some(1_000),
            }),
        ),
        (
            Method::AgentExplain,
            json(&agent::ExplainParams { target: target() }),
        ),
        (
            Method::AgentNext,
            json(&agent::NextParams {
                workspace: Some(WorkspaceId::new_v4()),
            }),
        ),
        (
            Method::PaneRun,
            json(&pane::RunParams {
                target: target(),
                text: "cargo test".to_owned(),
            }),
        ),
        (
            Method::PaneRead,
            json(&pane::ReadParams {
                target: target(),
                lines: Some(40),
            }),
        ),
        (
            Method::PaneSendText,
            json(&pane::SendTextParams {
                target: target(),
                text: "y".to_owned(),
            }),
        ),
        (
            Method::PaneSendKeys,
            json(&pane::SendKeysParams {
                target: target(),
                keys: vec!["ctrl+c".to_owned()],
            }),
        ),
    ]
}

// ---------------------------------------------------------------- the law

#[test]
fn every_flagged_payload_field_is_reachable_by_flag_or_declared_params_only() {
    let fixtures = fixtures();
    assert_eq!(
        fixtures
            .iter()
            .map(|(method, _)| *method)
            .collect::<BTreeSet<_>>(),
        ROWS.iter().map(|row| row.method).collect::<BTreeSet<_>>(),
        "every flagged row owes this file a fully-populated fixture, and only \
         a flagged row may have one"
    );

    for (method, params) in fixtures {
        let row = row(method).expect("a flagged row");
        let keys: BTreeSet<&str> = params
            .as_object()
            .expect("a payload is an object")
            .keys()
            .map(String::as_str)
            .collect();
        let declared: BTreeSet<&str> = row.fields.iter().map(|field| field.name).collect();
        assert_eq!(
            keys,
            declared,
            "{}'s parameters and its field map disagree; a field that should \
             not be a flag is declared params-only with its reason, never left \
             out",
            method.wire_name()
        );
    }
}

#[test]
fn every_declared_field_names_an_argument_its_leaf_carries_and_the_reverse() {
    for row in ROWS {
        let leaf = leaf(row.method.spec().cli);
        let carried: BTreeSet<&str> = leaf
            .get_arguments()
            .map(|arg| arg.get_id().as_str())
            .filter(|id| *id != PARAMS && *id != "help")
            .collect();
        let declared: BTreeSet<&str> = row.fields.iter().filter_map(Field::arg_id).collect();
        assert_eq!(
            declared,
            carried,
            "{}'s field map and its arguments disagree: a flag that fills no \
             field writes nothing, and a field naming no flag is unreachable",
            row.method.wire_name()
        );
    }
}

#[test]
fn a_params_only_field_says_why_and_is_reachable_no_other_way() {
    for row in ROWS {
        for field in row.fields {
            if let Source::ParamsOnly(why) = field.source {
                assert!(
                    !why.trim().is_empty(),
                    "{}'s `{}` is params-only without a reason",
                    row.method.wire_name(),
                    field.name
                );
            }
        }
    }
}

#[test]
fn the_flagged_rows_are_the_verbs_the_roadmap_spells_with_flags() {
    // Named literally, because "which verbs got flags" is a product decision
    // rather than a derivation: a row added here silently would be a surface
    // nobody chose. 05's M2 lists `wait`, `agent prompt --wait` and `pane
    // wait-output --match/--regex` by name; the rest of those two verbs'
    // groups follow so a human need not remember which half speaks which
    // language. M3 adds one more on the same grounds: `docs/09-m3-plan.md` §7
    // spells the exit test's own invocation `amx session handoff --binary
    // <path>`, so the flag is part of how the verb is meant to be typed.
    assert_eq!(
        ROWS.iter()
            .map(|row| row.method.wire_name())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "session.handoff",
            "wait",
            "pane.wait_output",
            "agent.start",
            "agent.prompt",
            "agent.explain",
            "agent.next",
            "pane.run",
            "pane.read",
            "pane.send_text",
            "pane.send_keys",
        ])
    );

    // And everything else is `--params` only — the machine surfaces above all.
    for method in [
        Method::AgentReport,
        Method::ClientViewport,
        Method::StreamBind,
        Method::PaneSplit,
        Method::SessionState,
        Method::EventsSubscribe,
    ] {
        assert!(
            row(method).is_none(),
            "{} is written by a program, not typed",
            method.wire_name()
        );
    }
}

#[test]
fn every_generated_leaf_still_takes_params_whatever_flags_it_grew() {
    for spec in SPECS {
        let leaf = leaf(spec.cli);
        assert!(
            leaf.get_arguments().any(|arg| arg.get_id() == PARAMS),
            "{} lost its escape hatch",
            spec.wire
        );
    }
}

// ------------------------------------------------------------------ the parse

#[test]
fn the_flag_forms_build_the_payloads_the_roadmap_promises() {
    assert_eq!(
        params(&["wait", "--until", "blocked", "--target", "review"]).unwrap(),
        json!({"until": "blocked", "target": "review"})
    );
    assert_eq!(
        params(&[
            "wait",
            "--until",
            "exited",
            "--target",
            "build",
            "--timeout",
            "30s"
        ])
        .unwrap(),
        json!({"until": "exited", "target": "build", "timeout_ms": 30_000})
    );
    assert_eq!(
        params(&["agent", "prompt", "review", "--text", "go", "--wait"]).unwrap(),
        json!({"target": "review", "text": "go", "wait": "idle"}),
        "a bare --wait is --wait idle"
    );
    assert_eq!(
        params(&[
            "agent", "prompt", "review", "--text", "go", "--wait", "blocked"
        ])
        .unwrap(),
        json!({"target": "review", "text": "go", "wait": "blocked"})
    );
    assert_eq!(
        params(&["agent", "prompt", "review", "--text", "go"]).unwrap(),
        json!({"target": "review", "text": "go"}),
        "no --wait is the default, which does not serialize"
    );
    assert_eq!(
        params(&["agent", "start", "dev", "--kind", "claude", "--cwd", "/w"]).unwrap(),
        json!({"name": "dev", "kind": "claude", "cwd": "/w"})
    );
    assert_eq!(
        params(&[
            "agent", "start", "dev", "--kind", "claude", "--", "--model", "opus"
        ])
        .unwrap(),
        json!({"name": "dev", "kind": "claude", "args": ["--model", "opus"]}),
        "what follows `--` is the agent's own argv, hyphens and all"
    );
    assert_eq!(
        params(&["agent", "explain", "review"]).unwrap(),
        json!({"target": "review"})
    );
    assert_eq!(params(&["agent", "next"]).unwrap(), json!({}));
    assert_eq!(
        params(&["pane", "run", "build", "--text", "cargo test"]).unwrap(),
        json!({"target": "build", "text": "cargo test"})
    );
    assert_eq!(
        params(&["pane", "read", "build", "--lines", "40"]).unwrap(),
        json!({"target": "build", "lines": 40})
    );
    assert_eq!(
        params(&["pane", "send-text", "build", "--text", "y"]).unwrap(),
        json!({"target": "build", "text": "y"})
    );
    assert_eq!(
        params(&["pane", "send-keys", "build", "ctrl+c", "enter"]).unwrap(),
        json!({"target": "build", "keys": ["ctrl+c", "enter"]})
    );
    assert_eq!(
        params(&[
            "pane",
            "wait-output",
            "build",
            "--match",
            "tests passed",
            "--timeout",
            "2m"
        ])
        .unwrap(),
        json!({"target": "build", "match": "tests passed", "timeout_ms": 120_000})
    );
    assert_eq!(
        params(&["pane", "wait-output", "build", "--regex", "\\d+ passed"]).unwrap(),
        json!({"target": "build", "regex": "\\d+ passed"})
    );
}

#[test]
fn params_and_flags_are_exclusive_per_invocation() {
    let refused = params(&["pane", "read", "build", "--params", "{}"]).unwrap_err();
    assert!(
        refused.contains("--params") && refused.contains("cannot be used with"),
        "the two ways of naming a parameter cannot be mixed: {refused}"
    );

    // And the escape hatch alone still works, on a flagged row and on one that
    // has no flags at all.
    assert_eq!(
        params(&["pane", "read", "--params", r#"{"target":"build"}"#]).unwrap(),
        json!({"target": "build"}),
        "the JSON object reaches a flagged row untouched"
    );
    assert_eq!(params(&["session", "state"]).unwrap(), json!({}));
}

#[test]
fn a_missing_or_impossible_value_says_what_to_do_about_it() {
    for (argv, wanted) in [
        (vec!["pane", "read"], "<TARGET> is required"),
        (vec!["pane", "run", "build"], "--text is required"),
        (vec!["wait", "--until", "idle"], "--target is required"),
        (vec!["wait", "--target", "build"], "--until is required"),
        (
            vec!["pane", "wait-output", "build"],
            "--match or --regex is required",
        ),
        (vec!["agent", "start", "dev"], "--kind is required"),
    ] {
        let refused = params(&argv).unwrap_err();
        assert!(
            refused.contains(wanted) && refused.contains("--params"),
            "`amx {}` should say {wanted:?} and point at --params: {refused}",
            argv.join(" ")
        );
    }

    // Values that cannot be what they claim, each named by the flag that took
    // them rather than by the type that refused them.
    for (argv, wanted) in [
        (
            vec![
                "wait",
                "--until",
                "idle",
                "--target",
                "b",
                "--timeout",
                "soon",
            ],
            "--timeout",
        ),
        (vec!["pane", "read", "b", "--lines", "many"], "--lines"),
        (vec!["agent", "start", "d", "--kind", "Claude!"], "--kind"),
        (
            vec![
                "agent",
                "start",
                "d",
                "--kind",
                "claude",
                "--workspace",
                "7",
            ],
            "--workspace",
        ),
    ] {
        let refused = params(&argv).unwrap_err();
        assert!(
            refused.starts_with(wanted),
            "`amx {}` should be refused by {wanted}: {refused}",
            argv.join(" ")
        );
    }

    // clap refuses the two spellings of one condition before the parse sees
    // them, which is where the exactly-one rule of `pane.wait_output` lives.
    let both = params(&["pane", "wait-output", "b", "--match", "x", "--regex", "y"]).unwrap_err();
    assert!(both.contains("cannot be used with"), "{both}");
}

#[test]
fn a_duration_is_ms_s_m_h_or_a_bare_number_of_seconds() {
    assert_eq!(millis("500ms"), Some(500));
    assert_eq!(millis("30s"), Some(30_000));
    assert_eq!(millis("2m"), Some(120_000));
    assert_eq!(millis("1h"), Some(3_600_000));
    assert_eq!(millis("30"), Some(30_000), "a bare number is seconds");
    assert_eq!(millis("0"), Some(0));
    assert_eq!(millis(" 45s "), Some(45_000));

    assert_eq!(millis(""), None);
    assert_eq!(millis("s"), None);
    assert_eq!(millis("-1s"), None);
    assert_eq!(
        millis("1.5s"),
        None,
        "integers only, so `1500ms` is the way"
    );
    assert_eq!(millis("soon"), None);
    assert_eq!(millis("99999999999999999999h"), None, "no silent wrap");
}

// ----------------------------------------------------------------- the harness

/// One payload, as it goes on the wire.
fn json<T: serde::Serialize>(params: &T) -> Value {
    serde_json::to_value(params).expect("a payload serializes")
}

/// The generated command at this CLI path.
fn leaf(path: &[&str]) -> Command {
    let mut command = Command::new("amx").subcommands(method_commands());
    for segment in path {
        command = command
            .find_subcommand(segment)
            .unwrap_or_else(|| panic!("{} is not in the generated tree", path.join(" ")))
            .clone();
    }
    command
}

/// Parse `argv` against the generated tree and build the parameters it names.
///
/// The rule is `cmd::call`'s, repeated here rather than reached into: whatever
/// `--params` carries wins, and the flag layer answers only when it is absent.
/// The binary's own tree adds hand-written verbs on top of this one; nothing
/// here depends on them, so the test stays in the crate that owns the table.
fn params(argv: &[&str]) -> Result<Value, String> {
    let root = Command::new("amx").subcommands(method_commands());
    let matches = root
        .try_get_matches_from(std::iter::once("amx").chain(argv.iter().copied()))
        .map_err(|why| why.to_string())?;

    let (path, leaf) = deepest(&matches);
    let spec = SPECS
        .iter()
        .find(|spec| spec.cli == path.as_slice())
        .unwrap_or_else(|| panic!("{} is not a method row", path.join(" ")));
    match leaf.get_one::<String>(PARAMS) {
        Some(raw) => serde_json::from_str(raw).map_err(|why| why.to_string()),
        None => params_from_flags(spec.method, leaf).map_err(|why| why.to_string()),
    }
}

/// The full path a set of matches took, and the matches of its last segment.
fn deepest(matches: &ArgMatches) -> (Vec<&str>, &ArgMatches) {
    let mut path = Vec::new();
    let mut leaf = matches;
    while let Some((name, next)) = leaf.subcommand() {
        path.push(name);
        leaf = next;
    }
    (path, leaf)
}
