//! The tmux config amx ships to be copied, against a real tmux.
//!
//! `assets/tmux.conf` is read by people, not by amx: nothing includes it and
//! no verb writes it anywhere. That is exactly why it is checked here. A conf
//! is sourced top to bottom and tmux stops at the first line it cannot parse,
//! so one stale option name in a file we hand somebody costs them every line
//! under it — and a shipped file nobody runs is a file that rots quietly.
//!
//! What each test asserts is the claim the file's own comments make, so a
//! line that changes its mind here fails until the prose agrees.

mod common;

use common::Harness;
use std::path::{Path, PathBuf};

/// The file as shipped.
fn shipped() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/tmux.conf")
}

/// A server with the shipped file sourced into it.
///
/// The harness starts every server with `-f /dev/null`, so nothing but this
/// file has said anything to the tmux being asked. `source-file` connects to
/// a server rather than starting one, and a server with no session in it does
/// not stay, so there is a session here for the same reason a person's server
/// has one.
fn with_the_conf() -> Harness {
    let amx = Harness::new();
    amx.tmux(&["new-session", "-d", "-s", "somewhere"]);
    amx.tmux(&["source-file", &shipped().to_string_lossy()]);
    amx
}

#[test]
fn the_shipped_conf_is_a_file_this_tmux_can_read() {
    // `source-file` exits non-zero on the first line it cannot parse, and the
    // harness asserts on that, so arriving here at all is the check.
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

    // The file argues for the prefix table over `bind -n`, because a root
    // binding takes that key from every agent's pane. It should not quietly
    // do the thing it argues against.
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
