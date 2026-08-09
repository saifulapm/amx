#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
//! V06 acceptance: the tier-2 manifest engine, over fixture text.
//!
//! The engine is pure — screen text in, verdict out — so these drive it
//! directly rather than through a pty. Two kinds of fixture appear here and
//! they are doing different jobs:
//!
//! - **Hand-written manifests**, inline, exercising one grammar feature each.
//!   They are the grammar's specification: priority arbitration, gate
//!   semantics, region slicing, `skip_state_update`, and the caps.
//! - **Recorded screens**, under `tests/fixtures/manifest/`, captured from
//!   Claude Code 2.1.224 and Codex CLI 0.147.0 — the versions
//!   `docs/notes/hook-coverage.md` measured — through a real terminal on
//!   2026-08-07. They are what keeps the *shipped* manifests honest: a rule
//!   that stops matching the real UI fails here rather than in a status line.
//!
//! Every recorded screen is a transition V01 named. The idle ones are what an
//! Esc-interrupt and a cancelled permission dialog leave behind, which is the
//! whole reason tier 2 exists on an `edges` agent: those two exits emit no hook
//! event at all, so this file is the only thing that sees them.

use std::path::PathBuf;

use amx_core::PaneId;
use amx_core::agent::AgentState;
use amx_server::agent::manifest::{
    BUNDLED, Manifest, ManifestError, ManifestSource, Region, ScreenBuf, ScreenText, Verdict,
};

// A test crate root's module directory is `tests/`, so the harness needs its
// path spelled out to live beside its one suite rather than next to
// everybody's.
#[path = "manifest/harness.rs"]
mod harness;

use harness::{
    Counting, GRAMMAR, allocations, fixtures, parse, reject, screen, state_of, winner_of,
};

/// The counter the non-allocation acceptance rests on; see [`harness`].
#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[test]
fn rules_parse_with_priority_region_gates_and_skip_state_update() {
    let manifest = parse(GRAMMAR);

    assert_eq!(manifest.id(), "grammar");
    assert_eq!(manifest.version(), Some("1"));
    assert_eq!(
        manifest.source(),
        &ManifestSource::Override {
            path: PathBuf::from("/tmp/test.toml")
        },
    );

    let names: Vec<&str> = manifest.rules().iter().map(|rule| rule.name()).collect();
    assert_eq!(
        names,
        ["overlay", "dialog", "spinner", "prompt"],
        "rules keep file order, which is also the tie-break order",
    );

    let overlay = &manifest.rules()[0];
    assert_eq!(overlay.priority(), 1000);
    assert_eq!(overlay.region(), Region::BottomLines(3));
    assert_eq!(
        overlay.asserts(),
        None,
        "a skip_state_update rule asserts no state at all",
    );

    let dialog = &manifest.rules()[1];
    assert_eq!(dialog.asserts(), Some(AgentState::Blocked));
    assert_eq!(dialog.region(), Region::BottomNonEmptyLines(4));
    assert!(!dialog.visible_idle());

    assert_eq!(manifest.rules()[2].region(), Region::Title);

    let prompt = &manifest.rules()[3];
    assert!(prompt.visible_idle(), "the prompt box is a visible idle");
    assert_eq!(
        prompt.region(),
        Region::BottomNonEmptyLines(2),
        "an unnamed region would have defaulted to whole_recent",
    );
}

#[test]
fn gate_semantics_and_of_contains_regex_line_regex_all_or_of_any_not() {
    let manifest = parse(
        r#"
id = "gates"

[[rule]]
name = "everything"
state = "blocked"
region = "whole_recent"
contains = ["alpha", "beta"]
regex = ['(?m)^gamma$']
line_regex = ['^\s+delta']
all = [{ contains = ["epsilon"] }]
any = [{ contains = ["zeta"] }, { contains = ["eta"] }]
not = [{ contains = ["theta"] }]
"#,
    );
    let ok = "alpha beta\ngamma\n  delta\nepsilon\neta\n";
    assert_eq!(state_of(&manifest, ok, ""), Some(AgentState::Blocked));

    // Each conjunct on its own is enough to fail the gate.
    for (missing, screen) in [
        ("a `contains`", ok.replace("beta", "")),
        ("a `regex`", ok.replace("gamma", " gamma")),
        ("a `line_regex`", ok.replace("  delta", "delta")),
        ("an `all` child", ok.replace("epsilon", "")),
        ("every `any` child", ok.replace("eta", "")),
    ] {
        assert_eq!(
            state_of(&manifest, &screen, ""),
            None,
            "the gate must fail without {missing}",
        );
    }
    assert_eq!(
        state_of(&manifest, &format!("{ok}theta"), ""),
        None,
        "a matching `not` child excludes the rule",
    );

    // `contains` folds ASCII case; `line_regex` needs one *line* to match, not
    // every line.
    assert_eq!(
        state_of(&manifest, &ok.replace("alpha", "ALPHA"), ""),
        Some(AgentState::Blocked),
    );
}

#[test]
fn highest_priority_wins_ties_break_by_file_order() {
    let manifest = parse(
        r#"
id = "priority"

[[rule]]
name = "low"
state = "idle"
priority = 10
contains = ["shared"]

[[rule]]
name = "first_high"
state = "working"
priority = 900
contains = ["shared"]

[[rule]]
name = "second_high"
state = "blocked"
priority = 900
contains = ["shared"]
"#,
    );

    assert_eq!(state_of(&manifest, "shared", ""), Some(AgentState::Working));
    assert_eq!(
        winner_of(&manifest, "shared", ""),
        Some("first_high".to_owned()),
        "a tie goes to the earlier rule, so a manifest reads top-down",
    );
}

#[test]
fn regions_slice_whole_bottom_lines_bottom_non_empty_and_title() {
    let grid = "one\ntwo\n\n\nthree\nfour";
    let screen = ScreenText::new(grid, "spinner title");

    assert_eq!(Region::WholeRecent.slice(&screen), grid);
    assert_eq!(Region::Title.slice(&screen), "spinner title");
    assert_eq!(Region::BottomLines(2).slice(&screen), "three\nfour");
    assert_eq!(
        Region::BottomLines(4).slice(&screen),
        "\n\nthree\nfour",
        "bottom_lines counts blank lines like any other",
    );
    assert_eq!(
        Region::BottomNonEmptyLines(3).slice(&screen),
        "two\n\n\nthree\nfour",
        "a non-empty count still slices contiguous text, blanks included",
    );
    assert_eq!(
        Region::BottomLines(99).slice(&screen),
        grid,
        "a count past the top is the whole grid, never a panic",
    );
    assert_eq!(
        Region::BottomNonEmptyLines(2).slice(&ScreenText::new("\n\n \n", "")),
        "",
        "a blank screen has no non-empty tail, and that is not the whole screen",
    );

    // The buffer, not the rules, is where a grid becomes text: rows lose their
    // padding and the blank tail below what was painted goes entirely.
    let mut buf = ScreenBuf::new();
    buf.fill(["top   ", "", "bottom\t", "   ", ""]);
    assert_eq!(buf.as_str(), "top\n\nbottom");

    assert!(Region::parse("bottom_lines(0)").is_err());
    assert!(Region::parse("bottom_lines(99999)").is_err());
    assert!(Region::parse("after_last_prompt_marker").is_err());
    assert_eq!(
        Region::parse("bottom_non_empty_lines(5)")
            .unwrap()
            .to_string(),
        "bottom_non_empty_lines(5)",
        "a region round-trips through its manifest spelling",
    );
}

#[test]
fn skip_state_update_holds_the_previous_state_and_names_the_rule() {
    let manifest = parse(GRAMMAR);
    let overlay = "> irrelevant\nshowing detailed transcript\n↑↓ scroll";

    let mut buf = ScreenBuf::new();
    buf.fill(overlay.lines());
    match manifest.evaluate(buf.screen("")) {
        Verdict::Hold { rule } => {
            assert_eq!(rule.name(), "overlay");
            assert_eq!(rule.asserts(), None);
        }
        other => panic!("a skip_state_update match must hold, got {other:?}"),
    }

    // The freeze outranks the idle the prompt line would otherwise assert, and
    // that is the point: the overlay is *about* the agent, not *of* it, so the
    // state underneath survives.
    assert_eq!(state_of(&manifest, overlay, ""), None);
    assert_eq!(
        state_of(&manifest, "> idle prompt", ""),
        Some(AgentState::Idle),
        "without the overlay the same screen is an ordinary idle",
    );
}

#[test]
fn regexes_compile_once_at_load_never_per_evaluation() {
    // Load is where a pattern is judged: a rule with a broken regex is a
    // manifest that never loads, not a rule that fails at the first damage
    // batch on a busy pane.
    let err = reject(
        r#"
id = "broken"

[[rule]]
name = "bad"
state = "idle"
regex = ['([unclosed']
"#,
    );
    assert!(
        matches!(&err, ManifestError::Regex { rule, .. } if rule == "bad"),
        "a bad pattern is reported against its rule: {err}",
    );

    // And evaluation is pure reading: no pattern is rebuilt, no region is
    // copied, no lowercase clone of the screen is made. Zero allocations is a
    // stronger statement than "the regexes are cached", and it is the one
    // HACKING.md's hot-path rule actually asks for.
    let manifest = parse(GRAMMAR);
    let grid = screen("claude-working-spinner");
    let mut buf = ScreenBuf::new();
    for _ in 0..4 {
        buf.fill(grid.lines());
        let _ = manifest.evaluate(buf.screen("⠹ warm"));
    }

    let allocated = allocations(|| {
        for _ in 0..64 {
            buf.fill(grid.lines());
            let _ = manifest.evaluate(buf.screen("⠹ warm"));
        }
    });
    assert_eq!(allocated, 0, "a warm evaluation must not allocate");
}

#[test]
fn explain_reports_every_rule_with_match_evidence_and_region_preview() {
    let manifest = Manifest::load_bundled("claude").expect("the bundled manifest compiles");
    let grid = screen("claude-blocked-permission");
    let mut buf = ScreenBuf::new();
    buf.fill(grid.lines());

    let explanation = manifest.explain(buf.screen(""));

    assert_eq!(explanation.manifest, "bundled:claude.toml");
    assert_eq!(explanation.manifest_version.as_deref(), Some("2026.08.09"));
    assert_eq!(explanation.matched.as_deref(), Some("permission_dialog"));
    assert_eq!(
        explanation.rules.len(),
        manifest.rules().len(),
        "every rule reports, not only the winner",
    );

    let dialog = explanation
        .rules
        .iter()
        .find(|verdict| verdict.rule == "permission_dialog")
        .expect("the winning rule is in the report");
    assert!(dialog.matched);
    assert_eq!(dialog.asserts, Some(AgentState::Blocked));
    assert_eq!(dialog.region, "bottom_non_empty_lines(8)");
    assert!(
        dialog
            .evidence
            .as_ref()
            .is_some_and(|why| why.contains("1. Yes")),
        "evidence quotes the line that made it match: {:?}",
        dialog.evidence,
    );

    // The half that answers "why is my pane *not* idle": the rule that did not
    // fire says which of its anchors the screen was missing.
    let idle = explanation
        .rules
        .iter()
        .find(|verdict| verdict.rule == "prompt_box_idle")
        .expect("the idle rule is in the report");
    assert!(!idle.matched);
    assert!(
        idle.evidence.as_ref().is_some_and(|why| why.contains('/')),
        "a failed line_regex names the pattern: {:?}",
        idle.evidence,
    );

    assert!(
        explanation
            .region_preview
            .iter()
            .any(|line| line.contains("Do you want to proceed?")),
        "the preview is the text the winning rule read",
    );

    // The reply the wire carries is this plus what only the hub knows.
    let pane = PaneId::new_v4();
    let reply = explanation.into_reply(pane, None, None);
    assert_eq!(reply.pane, pane);
    assert_eq!(reply.matched.as_deref(), Some("permission_dialog"));
}

#[test]
fn claude_manifest_matches_captured_idle_working_and_blocked_screens() {
    let manifest = Manifest::load_bundled("claude.toml").expect("the bundled manifest compiles");

    for (fixture, expected) in [
        ("claude-idle-prompt", AgentState::Idle),
        ("claude-idle-after-interrupt", AgentState::Idle),
        ("claude-working-stream", AgentState::Working),
        ("claude-working-spinner", AgentState::Working),
        ("claude-blocked-permission", AgentState::Blocked),
    ] {
        assert_eq!(
            state_of(&manifest, &screen(fixture), ""),
            Some(expected),
            "{fixture} reads as {expected}",
        );
    }

    // An idle screen the user has started typing into is still idle: the
    // footer hint the working rule reads disappears the moment the composer
    // has text in it, which is exactly the state a silent Esc-interrupt leaves.
    assert!(
        winner_of(&manifest, &screen("claude-idle-after-interrupt"), "")
            .is_some_and(|rule| rule == "prompt_box_idle"),
    );

    // The title is the earliest working signal and outranks the grid, so a
    // pane whose screen has not caught up is still reported as working.
    assert_eq!(
        state_of(
            &manifest,
            &screen("claude-idle-prompt"),
            "⠂ writing a haiku"
        ),
        Some(AgentState::Working),
        "a braille spinner in the title is a turn in flight",
    );
    assert_eq!(
        state_of(
            &manifest,
            &screen("claude-idle-prompt"),
            "✳ writing a haiku"
        ),
        Some(AgentState::Idle),
        "the resting title asserts nothing, and the screen decides",
    );
}

/// The M4 exit's second finding, as a test.
///
/// `docs/notes/m4-live-smoke.md` §6.8 item 1 found the shipped
/// `permission_dialog` rule keyed on `do you want to proceed?`, which is what a
/// Bash dialog asks and what the one recorded dialog said. A `Write` asks
/// `Do you want to overwrite exit-probe.txt?`, so tier 2 could not see the
/// dialog at all and `agent explain` answered `matched: null` with it on the
/// screen — while the suite stayed green, because the suite owned one fixture.
///
/// So this walks the directory rather than a list: every recorded dialog must
/// read `blocked`, and must read it off the dialog rule rather than by
/// accident. Recording a new class is enough to cover it.
#[test]
fn every_recorded_permission_dialog_reads_blocked_from_the_dialog_rule() {
    let manifest = Manifest::load_bundled("claude").expect("the bundled manifest compiles");
    let recorded = fixtures("claude-blocked-");

    assert!(
        recorded.len() >= 6,
        "six dialog classes are recorded, one file each: {recorded:?}",
    );

    for name in &recorded {
        let grid = screen(name);
        assert_eq!(
            state_of(&manifest, &grid, ""),
            Some(AgentState::Blocked),
            "{name} is a user being waited on",
        );
        assert_eq!(
            winner_of(&manifest, &grid, "").as_deref(),
            Some("permission_dialog"),
            "{name} must be seen by the rule that exists for it",
        );
    }
}

/// What each recorded dialog actually says, quoted.
///
/// A fixture is a file, and files get copied; this is what stops the six from
/// collapsing into six copies of the easy one. Each literal here was read off a
/// real 2.1.226 screen, and together they are the evidence for the comment in
/// `claude.toml` that says the question wording varies by tool. A seventh class
/// was driven and not recorded: a `WebSearch` asks `Do you want to proceed?`,
/// word for word what the bash dialog asks, so it would be a second copy of a
/// screen already here.
#[test]
fn the_recorded_dialogs_are_six_different_questions() {
    for (fixture, question) in [
        ("claude-blocked-permission", "Do you want to proceed?"),
        (
            "claude-blocked-write-create",
            "Do you want to create exit-probe.txt?",
        ),
        (
            "claude-blocked-write-overwrite",
            "Do you want to overwrite exit-probe.txt?",
        ),
        (
            "claude-blocked-edit",
            "Do you want to make this edit to exit-probe.txt?",
        ),
        (
            "claude-blocked-fetch",
            "Do you want to allow Claude to fetch this content?",
        ),
        ("claude-blocked-plan", "Would you like to proceed?"),
    ] {
        assert!(
            screen(fixture).contains(question),
            "{fixture} is the screen that asks {question:?}",
        );
    }

    // One of the six says "proceed", which is the whole finding: the rule was
    // keyed on the wording of a single dialog, and the fixture that carried
    // that wording was the single dialog anyone had recorded.
    let proceeding = fixtures("claude-blocked-")
        .iter()
        .filter(|name| screen(name).contains("Do you want to proceed?"))
        .count();
    assert_eq!(
        proceeding, 1,
        "the bash dialog, and no other recorded screen"
    );
}

/// The other half of the fix: nothing that is *not* a dialog became one.
///
/// Widening a blocked rule is the change most likely to put an idle agent in
/// the attention queue and keep it there, so the idle and working screens are
/// re-asserted against the widened rule rather than assumed.
#[test]
fn widening_the_dialog_rule_leaves_the_idle_and_working_screens_alone() {
    let manifest = Manifest::load_bundled("claude").expect("the bundled manifest compiles");

    for fixture in [
        "claude-idle-prompt",
        "claude-idle-after-interrupt",
        "claude-working-stream",
        "claude-working-spinner",
    ] {
        assert_ne!(
            state_of(&manifest, &screen(fixture), ""),
            Some(AgentState::Blocked),
            "{fixture} has nobody waiting on it",
        );
    }

    // The composer is the case the `1. Yes` anchor has to be careful about: a
    // user may type anything into it, including the answer to a dialog that is
    // no longer there. The dialog's list is indented and the composer's `❯`
    // sits at column 0, so the idle rule's guard reads the indentation.
    let typed = screen("claude-idle-prompt").replace("❯\u{a0}Try", "❯\u{a0}1. Yes — Try");
    assert_eq!(
        state_of(&manifest, &typed, ""),
        Some(AgentState::Idle),
        "an idle screen with `1. Yes` typed into the composer is still idle",
    );
}

#[test]
fn codex_manifest_matches_captured_idle_working_and_blocked_screens() {
    let manifest = Manifest::load_bundled("codex").expect("the bundled manifest compiles");

    for (fixture, expected) in [
        ("codex-idle-prompt", AgentState::Idle),
        ("codex-idle-after-interrupt", AgentState::Idle),
        ("codex-working", AgentState::Working),
        ("codex-blocked-approval", AgentState::Blocked),
    ] {
        assert_eq!(
            state_of(&manifest, &screen(fixture), ""),
            Some(expected),
            "{fixture} reads as {expected}",
        );
    }

    // The working screen still carries the *previous* turn's interrupt marker
    // in its transcript. A `not` guard on that text — the obvious reading of
    // herdr's manifest — would blank the state of every turn after an
    // interrupted one, so there is none, and this is the fixture that says so.
    assert!(screen("codex-working").contains("■ Conversation interrupted"));

    assert_eq!(
        state_of(&manifest, &screen("codex-idle-prompt"), "⠋ scratch"),
        Some(AgentState::Working),
        "Codex paints its spinner into the title before the grid says anything",
    );
}

#[test]
fn malformed_manifests_are_rejected_with_the_offending_rule_named() {
    let cases: [(&str, fn(&ManifestError) -> bool); 6] = [
        (
            "[[rule]]\nname = \"x\"\ncontains = [\"y\"]\n",
            |err| matches!(err, ManifestError::NoVerdict { rule } if rule == "x"),
        ),
        (
            "[[rule]]\nname = \"x\"\nstate = \"idle\"\nskip_state_update = true\ncontains = [\"y\"]\n",
            |err| matches!(err, ManifestError::TwoVerdicts { rule } if rule == "x"),
        ),
        (
            "[[rule]]\nname = \"x\"\nstate = \"idle\"\nnot = [{ contains = [\"y\"] }]\n",
            |err| matches!(err, ManifestError::EmptyGate { rule } if rule == "x"),
        ),
        (
            "[[rule]]\nname = \"x\"\nstate = \"idle\"\nregion = \"osc_title\"\ncontains = [\"y\"]\n",
            |err| matches!(err, ManifestError::Region { rule, .. } if rule == "x"),
        ),
        (
            "[[rule]]\nname = \"x\"\nstate = \"idle\"\ncontains = [\"y\"]\n\n[[rule]]\nname = \"x\"\nstate = \"working\"\ncontains = [\"z\"]\n",
            |err| matches!(err, ManifestError::DuplicateRule { name } if name == "x"),
        ),
        (
            "[[rule]]\nname = \"x\"\nstate = \"idle\"\ncontian = [\"y\"]\n",
            |err| matches!(err, ManifestError::Toml { .. }),
        ),
    ];

    for (body, expected) in cases {
        let text = format!("id = \"broken\"\n{body}");
        let err = reject(&text);
        assert!(expected(&err), "unexpected rejection: {err}");
    }

    // A whole manifest stands or falls together — unlike `config.toml`, whose
    // sections are lenient — so nothing above half-applies.
    assert!(matches!(
        Manifest::load_bundled("nonexistent"),
        Err(ManifestError::Toml { .. }),
    ));
}

/// V03's conformance test walks this table for the manifest each stanza names;
/// this is the same walk, so a manifest that stopped compiling fails here too
/// even before the registry has an opinion about it.
#[test]
fn every_bundled_manifest_compiles_and_declares_its_own_agent_id() {
    assert_eq!(BUNDLED.len(), 2, "M2 ships claude and codex");

    for (file, _) in BUNDLED {
        let manifest = Manifest::load_bundled(file)
            .unwrap_or_else(|err| panic!("bundled manifest {file} must compile: {err}"));
        assert_eq!(
            format!("{}.toml", manifest.id()),
            *file,
            "a manifest's id names its own file, so a stanza can find it",
        );
        assert!(
            manifest.version().is_some(),
            "{file} declares a version for explain to report",
        );
        assert!(
            !manifest.rules().is_empty(),
            "{file} carries rules; an empty manifest detects nothing",
        );
        assert_eq!(
            Manifest::load_bundled(manifest.id())
                .expect("lookup without the extension")
                .id(),
            manifest.id(),
            "a stanza may name the file or the agent",
        );
    }
}
