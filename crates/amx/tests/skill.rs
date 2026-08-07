//! V16 acceptance: the agent skill.
//!
//! The skill exists to be *used by something outside amx*, which is why it is
//! not tested by reading the source it was written from: it is checked as the
//! file `amx skill install` actually wrote, and every command it names is
//! resolved against [`SPECS`](amx_proto::control::SPECS). A skill is a set of
//! promises about a CLI; a renamed method has to break here, in a test, rather
//! than in an agent's hands three weeks later.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::collections::BTreeSet;
use std::process::Stdio;

use amx_proto::control::SPECS;

mod support;

use support::{Env, TempDir};

/// The verbs the skill may name that are not method rows.
///
/// 04 §1's process lifecycle and 04 §8's streaming consumer: `amx events`
/// (bare) is a long-lived subscriber rather than the `events.subscribe` call,
/// and `skill` writes a file without touching a session. Neither could be a
/// table row, because a row is something a server handles. Everything else the
/// skill names has to be one.
const NON_WIRE: &[&str] = &["events", "skill"];

// ------------------------------------------------------------------- install

#[test]
fn skill_install_writes_the_asset_and_is_idempotent() {
    let env = Env::new("skill-install");
    let dir = TempDir::new("skill-install-target");
    let skill = dir.path().join("SKILL.md");

    let first = env.run(&[
        "skill",
        "install",
        "--path",
        &dir.path().display().to_string(),
    ]);
    assert!(
        first.ok().contains("wrote"),
        "a first install says what it wrote: {first:?}"
    );
    let written = std::fs::read_to_string(&skill).expect("the asset landed");
    assert!(
        written.starts_with("---\nname: amx\n"),
        "the asset is the skill, frontmatter and all:\n{written}"
    );
    assert!(
        first.stdout.contains("SKILL.md"),
        "the output names the path a user has to place: {first:?}"
    );

    // Idempotent means *unchanged*, not "written again with the same bytes":
    // an installer that cannot tell the difference cannot tell a user whether
    // their copy has just been replaced.
    let second = env.run(&[
        "skill",
        "install",
        "--path",
        &dir.path().display().to_string(),
    ]);
    assert!(
        second.ok().contains("unchanged"),
        "a reinstall over a current asset is a no-op: {second:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&skill).expect("still there"),
        written
    );

    // And an asset that has drifted is restored, with the word for it.
    std::fs::write(&skill, "half a skill").expect("edit the asset");
    let third = env.run(&[
        "skill",
        "install",
        "--path",
        &dir.path().display().to_string(),
    ]);
    assert!(
        third.ok().contains("updated"),
        "a differing copy is replaced, and said so: {third:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&skill).expect("restored"),
        written,
        "the shipped asset is the one that wins"
    );

    // No `--path`: the current directory, which is what a user in their
    // project gets when they type the command with no arguments.
    let here = TempDir::new("skill-install-cwd");
    let out = env
        .command()
        .args(["skill", "install"])
        .current_dir(here.path())
        .stdin(Stdio::null())
        .output()
        .expect("run amx skill install");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        std::fs::read_to_string(here.path().join("SKILL.md")).expect("the asset landed"),
        written
    );
}

#[test]
fn skill_content_names_only_verbs_that_exist_in_specs() {
    let env = Env::new("skill-verbs");
    let dir = TempDir::new("skill-verbs-target");
    env.run(&[
        "skill",
        "install",
        "--path",
        &dir.path().display().to_string(),
    ])
    .ok();
    let text = std::fs::read_to_string(dir.path().join("SKILL.md")).expect("the asset landed");

    let named = commands(&text);
    assert!(
        named.len() >= 10,
        "the extractor found only {} commands in the skill, which is fewer than \
         it teaches — it has stopped reading the file it is guarding: {named:?}",
        named.len()
    );

    let known: BTreeSet<Vec<&str>> = SPECS.iter().map(|spec| spec.cli.to_vec()).collect();
    for path in &named {
        let segments: Vec<&str> = path.iter().map(String::as_str).collect();
        if known.contains(&segments) {
            continue;
        }
        // Not a row: then it is one of the handful of verbs that cannot be
        // one, and it still has to exist in the tree the binary parses.
        assert!(
            NON_WIRE.contains(&segments[0]) && segments.len() == 1
                || NON_WIRE.contains(&segments[0]) && subcommand_exists(&segments),
            "the skill names `amx {}`, which is neither a method row nor one of \
             the non-wire verbs {NON_WIRE:?}",
            segments.join(" "),
        );
    }

    // The other direction, kept deliberately loose: the skill is an
    // introduction, not the schema, so it does not owe a line per row. What it
    // does owe is the surface it exists to teach — the pane-driving verbs of
    // 04 §8 and the agent verbs of 04 §5.
    for must in [
        vec!["pane", "split"],
        vec!["pane", "run"],
        vec!["pane", "read"],
        vec!["pane", "send-keys"],
        vec!["pane", "wait-output"],
        vec!["agent", "start"],
        vec!["agent", "prompt"],
        vec!["wait"],
        vec!["session", "state"],
    ] {
        assert!(
            named.iter().any(|path| path == &must),
            "the skill never teaches `amx {}`",
            must.join(" ")
        );
    }
}

/// Every `amx …` command the skill names, from its code spans and blocks.
///
/// Prose is not scanned: the file talks *about* amx in sentences, and a test
/// that read those would be checking English. What it checks is the thing a
/// reader would copy — anything in backticks or a fenced block.
fn commands(text: &str) -> Vec<Vec<String>> {
    let mut chunks = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.starts_with("```") {
            fenced = !fenced;
        } else if fenced {
            chunks.push(line.to_owned());
        } else {
            // Inline spans are the odd fields between backticks.
            chunks.extend(line.split('`').skip(1).step_by(2).map(str::to_owned));
        }
    }

    let mut found = Vec::new();
    for chunk in chunks {
        let words: Vec<&str> = chunk.split_whitespace().collect();
        for (n, word) in words.iter().enumerate() {
            if *word != "amx" {
                continue;
            }
            let path: Vec<String> = words[n + 1..]
                .iter()
                .take_while(|next| {
                    // A verb, not a flag and not the JSON after one: lowercase
                    // to start with, then the `wait-output` vocabulary.
                    next.starts_with(|c: char| c.is_ascii_lowercase())
                        && next
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                })
                .take(2)
                .map(|segment| (*segment).to_owned())
                .collect();
            if !path.is_empty() && !found.contains(&path) {
                found.push(path);
            }
        }
    }
    found
}

/// Whether the binary's own command tree has this path.
fn subcommand_exists(segments: &[&str]) -> bool {
    let mut command = amx::cli::cli();
    for segment in segments {
        match command.find_subcommand(segment) {
            Some(found) => command = found.clone(),
            None => return false,
        }
    }
    true
}
