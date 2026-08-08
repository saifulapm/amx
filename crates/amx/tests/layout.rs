//! `amx layout export` and `amx apply`: a session's shape as a file, and back
//! (D-M3-11).
//!
//! The round trip is the specification, so it is the first test here: a shape
//! built by hand through the ordinary verbs, exported, applied into a second
//! session that has never seen it, and exported again. The two files are
//! compared byte for byte — which is only meaningful because the format carries
//! no ids at all, and is why "modulo ids" costs this test no filtering.
//!
//! Everything here needs a real server, because what it asserts is what a
//! session *became*. Its sibling `layout_file.rs` holds the translations that
//! need none — the projection of a state reply and the plan a file produces,
//! including the agent path, which no runner can run for real (R-M2-8).

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use amx::layout::build::{self, Step, Taken};
use amx::layout::schema;
use amx_proto::control::pane::SplitDirection;
use amx_proto::control::session::StateReply;
use amx_server::session::probe::probe;
use support::{Env, wait_until};

/// A session with a server answering and no workspace in it.
///
/// `amx server` rather than `amx attach`: an attaching client seeds a first
/// workspace into an empty session, and a layout applied on top of one would
/// be a layout plus somebody else's shell.
fn empty_session(tag: &str) -> (Env, std::process::Child) {
    let env = Env::new(tag);
    let server = env.spawn(&["server"]);
    wait_until("the server binds", || {
        probe(&env.socket()).expect("probe").is_running()
    });
    (env, server)
}

/// The pane ids `session state` reports, in reply order.
fn pane_ids(env: &Env) -> Vec<String> {
    let state: serde_json::Value =
        serde_json::from_str(env.run(&["session", "state"]).ok()).expect("state is JSON");
    state["panes"]
        .as_array()
        .expect("panes is an array")
        .iter()
        .map(|pane| pane["pane"].as_str().expect("a pane id").to_owned())
        .collect()
}

/// The reply to `session state`, typed.
fn state(env: &Env) -> StateReply {
    serde_json::from_str(env.run(&["session", "state"]).ok()).expect("state deserializes")
}

/// Split `pane`, and return the id of the pane that appears.
fn split(env: &Env, pane: &str, direction: &str) -> String {
    let before = pane_ids(env);
    env.run(&[
        "pane",
        "split",
        "--params",
        &format!(r#"{{"pane":"{pane}","direction":"{direction}"}}"#),
    ])
    .ok();
    let after = pane_ids(env);
    after
        .into_iter()
        .find(|id| !before.contains(id))
        .expect("the split made a pane")
}

/// A shape with two workspaces, a nested split and a non-default ratio, built
/// through the ordinary verbs so that export is tested against a session apply
/// did not make.
fn build_shape(env: &Env) {
    env.run(&["workspace", "create", "--params", r#"{"label":"dev"}"#])
        .ok();
    let root = pane_ids(env).first().expect("a root pane").clone();
    let right = split(env, &root, "vertical");
    // 0.5 + 0.12 is a ratio the file has to carry, and the one the second
    // export has to reproduce through `pane.resize`.
    env.run(&[
        "pane",
        "resize",
        "--params",
        &format!(r#"{{"pane":"{root}","direction":"right","delta":0.12}}"#),
    ])
    .ok();
    let _below = split(env, &right, "horizontal");
    env.run(&[
        "pane",
        "rename",
        "--params",
        &format!(r#"{{"pane":"{root}","label":"edit"}}"#),
    ])
    .ok();

    env.run(&["workspace", "create", "--params", r#"{"label":"docs"}"#])
        .ok();
    let second_root = state(env)
        .workspaces
        .last()
        .and_then(|workspace| workspace.layout.panes().first().copied())
        .expect("the second workspace has a pane");
    split(env, &second_root.to_string(), "horizontal");
}

#[test]
fn export_apply_export_round_trips_modulo_ids() {
    let (source, mut source_server) = empty_session("rt-a");
    build_shape(&source);
    let first = source.run(&["layout", "export"]).ok().to_owned();

    // No id of any kind reaches the file: that is what makes it replayable
    // somewhere else, and what makes the comparison below exact rather than
    // "equal after masking".
    for pane in pane_ids(&source) {
        assert!(
            !first.contains(&pane),
            "a pane id reached the layout file:\n{first}"
        );
    }
    assert!(
        first.contains(r#"label = "dev""#) && first.contains("ratio = 0.620"),
        "the export carries the labels and the ratio:\n{first}"
    );

    let path = source.socket().with_file_name("layout.toml");
    std::fs::write(&path, &first).expect("write the exported layout");

    let (target, mut target_server) = empty_session("rt-b");
    let applied = target.run(&["apply", path.to_str().expect("utf-8 path")]);
    assert_eq!(applied.code, Some(0), "apply failed: {applied:?}");
    let second = target.run(&["layout", "export"]).ok().to_owned();

    assert_eq!(
        first, second,
        "a layout applied into a fresh session exports as itself"
    );

    let _ = source_server.kill();
    let _ = target_server.kill();
}

#[test]
fn apply_builds_the_bsp_by_splits_in_deterministic_order() {
    // A tree whose two halves are both split, so the order is observable: the
    // root's cut, then everything inside its first half, then everything
    // inside its second.
    let text = "\
version = 1

[[workspace]]
label = \"dev\"

[[workspace.pane]]
label = \"edit\"

[[workspace.pane]]
from = 0
direction = \"vertical\"
ratio = 0.700

[[workspace.pane]]
from = 0
direction = \"horizontal\"

[[workspace.pane]]
from = 1
direction = \"horizontal\"
";
    let file = schema::parse(text).expect("the fixture parses");
    let steps = build::plan(&file, &Taken::default());

    assert_eq!(
        steps,
        vec![
            Step::CreateWorkspace {
                label: Some("dev".to_owned()),
                workspace: 0,
                root: 0,
            },
            Step::Split {
                from: 0,
                into: 1,
                direction: SplitDirection::Vertical,
                cwd: None,
            },
            Step::Resize {
                pane: 0,
                ratio: 0.7,
                direction: SplitDirection::Vertical,
            },
            Step::Split {
                from: 0,
                into: 2,
                direction: SplitDirection::Horizontal,
                cwd: None,
            },
            Step::Split {
                from: 1,
                into: 3,
                direction: SplitDirection::Horizontal,
                cwd: None,
            },
            Step::Rename {
                pane: 0,
                label: "edit".to_owned(),
            },
        ],
        "the plan is the file's own order, and every `from` names a pane an \
         earlier step made"
    );

    // And the order is right, not merely fixed: replayed against a real
    // server, the tree it builds exports as the file it came from.
    let (env, mut server) = empty_session("bsp");
    let path = env.socket().with_file_name("bsp.toml");
    std::fs::write(&path, text).expect("write the fixture");
    let applied = env.run(&["apply", path.to_str().expect("utf-8 path")]);
    assert_eq!(applied.code, Some(0), "apply failed: {applied:?}");
    assert_eq!(
        env.run(&["layout", "export"]).ok(),
        file.to_string(),
        "the session apply built is the session the file describes"
    );
    let _ = server.kill();
}

#[test]
fn name_collisions_suffix_rather_than_merge() {
    let (env, mut server) = empty_session("clash");
    env.run(&["workspace", "create", "--params", r#"{"label":"dev"}"#])
        .ok();
    let root = pane_ids(&env).first().expect("a root pane").clone();
    env.run(&[
        "pane",
        "rename",
        "--params",
        &format!(r#"{{"pane":"{root}","label":"edit"}}"#),
    ])
    .ok();

    let path = env.socket().with_file_name("clash.toml");
    std::fs::write(
        &path,
        "version = 1\n\n[[workspace]]\nlabel = \"dev\"\n\n[[workspace.pane]]\nlabel = \"edit\"\n",
    )
    .expect("write the layout");
    let applied = env.run(&["apply", path.to_str().expect("utf-8 path")]);
    assert_eq!(applied.code, Some(0), "apply failed: {applied:?}");

    let state = state(&env);
    let workspaces: Vec<_> = state
        .workspaces
        .iter()
        .filter_map(|workspace| workspace.label.clone())
        .collect();
    assert_eq!(
        workspaces,
        vec!["dev".to_owned(), "dev-2".to_owned()],
        "the layout's workspace is a second one, not the running one rewritten"
    );
    let panes: Vec<_> = state
        .panes
        .iter()
        .filter_map(|pane| pane.label.clone())
        .collect();
    assert_eq!(
        panes,
        vec!["edit".to_owned(), "edit-2".to_owned()],
        "and the pane label collides the same way"
    );

    // The same rule inside one file, with nothing running: two workspaces
    // asking for one name get two names.
    let file = schema::parse(
        "version = 1\n\n[[workspace]]\nlabel = \"dev\"\n\n[[workspace]]\nlabel = \"dev\"\n",
    )
    .expect("the fixture parses");
    let labels: Vec<_> = build::plan(&file, &Taken::default())
        .into_iter()
        .filter_map(|step| match step {
            Step::CreateWorkspace { label, .. } => label,
            _ => None,
        })
        .collect();
    assert_eq!(labels, vec!["dev".to_owned(), "dev-2".to_owned()]);

    let _ = server.kill();
}

#[test]
fn a_malformed_layout_names_the_line_and_applies_nothing() {
    let (env, mut server) = empty_session("bad");
    let path = env.socket().with_file_name("bad.toml");
    // Line 8 asks for a ratio no split can hold. Everything above it is
    // valid, so a parser that validated as it went would have created a
    // workspace by the time it reached the refusal.
    std::fs::write(
        &path,
        "version = 1\n\n[[workspace]]\nlabel = \"dev\"\n\n[[workspace.pane]]\n\n\
         [[workspace.pane]]\nfrom = 0\ndirection = \"vertical\"\nratio = 3.5\n",
    )
    .expect("write the layout");

    let refused = env.run(&["apply", path.to_str().expect("utf-8 path")]);
    assert_eq!(refused.code, Some(1), "a malformed layout is a failure");
    assert!(
        refused.stderr.contains("line 8"),
        "the refusal names the line: {refused:?}"
    );
    assert!(
        state(&env).workspaces.is_empty(),
        "nothing was applied: {:?}",
        state(&env).workspaces
    );

    // The other refusals name their lines too, with nothing running at all.
    for (text, line, what) in [
        (
            "version = 1\n\n[[workspace]]\nlabel = \n",
            Some(4),
            "a value that is not there",
        ),
        (
            "version = 2\n\n[[workspace]]\n",
            Some(1),
            "a version this build does not read",
        ),
        (
            "version = 1\n\n[[workspace]]\n\n[[workspace.pane]]\nfrom = 0\n",
            Some(5),
            "a first pane that claims to be a split",
        ),
        (
            "version = 1\n\n[[workspace]]\n\n[[workspace.pane]]\n\n[[workspace.pane]]\nfrom = 1\n",
            Some(7),
            "a pane splitting one that does not precede it",
        ),
        (
            "version = 1\n\n[[workspace]]\n\n[[workspace.pane]]\n\n[[workspace.pane]]\n",
            Some(7),
            "a pane after the first with no source",
        ),
        (
            "version = 1\n\n[[workspace]]\ncolour = \"blue\"\n",
            Some(4),
            "a key this format does not have",
        ),
    ] {
        let refused = schema::parse(text).expect_err(what);
        assert_eq!(refused.line, line, "{what}: {refused}");
    }

    let _ = server.kill();
}
