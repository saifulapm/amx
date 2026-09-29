//! The example `assets/tmux.conf`, sourced into a real tmux.
//!
//! amx never reads this file, so nothing else would notice it breaking. tmux
//! stops sourcing at the first line it cannot parse, so one stale option
//! costs every line after it. Each test checks a claim the file's own
//! comments make.

mod common;

use common::Harness;
use std::path::{Path, PathBuf};

fn shipped() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/tmux.conf")
}

/// A server with only the shipped file sourced into it.
///
/// `source-file` needs a running server, and a server with no session exits,
/// so a session is created first.
fn with_the_conf() -> Harness {
    let amx = Harness::new();
    amx.tmux(&["new-session", "-d", "-s", "somewhere"]);
    amx.tmux(&["source-file", &shipped().to_string_lossy()]);
    amx
}

#[test]
fn the_shipped_conf_is_a_file_this_tmux_can_read() {
    // `source-file` fails on the first line it cannot parse, and the harness
    // asserts that tmux succeeded.
    let amx = with_the_conf();
    assert_eq!(amx.tmux(&["display-message", "-p", "ok"]), "ok");
}

#[test]
fn the_extended_keys_the_view_reads_are_turned_on() {
    let amx = with_the_conf();
    assert_eq!(
        amx.tmux(&["show", "-g", "extended-keys"]),
        "extended-keys on"
    );
    assert_eq!(
        amx.tmux(&["show", "-g", "extended-keys-format"]),
        "extended-keys-format csi-u"
    );
}

#[test]
fn the_counts_go_in_the_status_line_and_are_refreshed() {
    let amx = with_the_conf();
    let right = amx.tmux(&["show", "-g", "status-right"]);
    assert!(
        right.contains("#(amx statusline)"),
        "status-right should run the verb: {right}"
    );
    assert_eq!(
        amx.tmux(&["show", "-g", "status-interval"]),
        "status-interval 5"
    );
}

#[test]
fn the_attach_keys_are_bound_under_the_prefix_and_not_at_the_root() {
    let amx = with_the_conf();
    let prefix = amx.tmux(&["list-keys", "-T", "prefix"]);

    for (key, command) in [
        ("a", "amx attach --waiting"),
        ("A", "amx attach --next"),
        ("C-a", "amx attach --last"),
    ] {
        let bound = prefix.lines().any(|line| {
            let mut word = line.split_whitespace().skip(3);
            word.next() == Some(key) && line.contains(command)
        });
        assert!(bound, "{key} should run `{command}`:\n{prefix}");
    }

    // A root-table binding would take the key from every agent's pane, which
    // the file advises against.
    let root = amx.tmux(&["list-keys", "-T", "root"]);
    assert!(
        !root.contains("amx attach"),
        "no attach key belongs in the root table:\n{root}"
    );
}

#[test]
fn the_view_key_reaches_for_the_window_already_open() {
    let amx = with_the_conf();
    let prefix = amx.tmux(&["list-keys", "-T", "prefix"]);
    let bound = prefix
        .lines()
        .find(|line| line.split_whitespace().nth(3) == Some("v"))
        .unwrap_or_default();
    assert!(
        bound.contains("select-window -t amx") && bound.contains("new-window -n amx"),
        "the view key should find the window before making one: {bound}"
    );
}

#[test]
fn a_client_is_not_thrown_out_of_tmux_when_an_agents_session_goes() {
    let amx = with_the_conf();
    assert_eq!(
        amx.tmux(&["show", "-g", "detach-on-destroy"]),
        "detach-on-destroy off"
    );
}
