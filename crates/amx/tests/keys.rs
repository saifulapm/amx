//! X07 acceptance: `amx keys`, and a rebound prefix on a real terminal.
//!
//! `crates/amx-client/tests/{config,input}.rs` prove the resolution and the
//! machine that runs it. What is left is the two claims only the binary can
//! make: that `amx keys` prints the resolved table from a file with no session
//! running, and that a rebound prefix actually reaches the input machine of an
//! attached client — the hop through `Ctx::config_path` and `App::input` that
//! a test inside the client crate cannot see.
//!
//! That second one is the whole point of D-M4-8. 10 §D14 said the phone
//! profile was "documentation work, not code" against a tree whose prefix was
//! a `const`; a suite that checked the table and not the attach would be the
//! same mistake one level down.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use support::{ALT_ENTER, ALT_LEAVE, Env, PREFIX, window};

/// The terminal size the attaching tests use.
const ROWS: u16 = 24;
const COLS: u16 = 80;

/// `ctrl+t`, the prefix the fixtures rebind to.
const REBOUND: u8 = 0x14;

#[test]
fn keys_prints_the_shipped_table_with_no_config_and_no_session() {
    let env = Env::new("keysbare");
    let out = env.run(&["keys"]);
    assert_eq!(out.code, Some(0), "{out:?}");

    // Every shipped binding, its action, and where it came from.
    for row in [
        "prefix ctrl+a, from shipped",
        "a       next-attention    shipped",
        "d       detach            shipped",
        "p       picker            shipped",
        "v       split-vertical    shipped",
        "w       navigate          shipped",
        "x       split-horizontal  shipped",
        "z       zoom              shipped",
        "ctrl+a  literal           prefix escape",
    ] {
        assert!(
            out.stdout.contains(row),
            "{row:?} missing from:\n{}",
            out.stdout
        );
    }
    assert!(out.stderr.is_empty(), "nothing to complain about: {out:?}");

    // And it reached no server: the command that prints keybindings must not
    // be the command that starts a session daemon.
    assert!(
        !env.socket().exists(),
        "`amx keys` started a server it has no business starting"
    );
}

#[test]
fn keys_says_which_bindings_came_from_the_file_and_which_did_not_resolve() {
    let env = Env::new("keyscfg");
    env.config(
        r#"
        [keys]
        prefix = "ctrl+t"

        [keys.bind]
        q = "detach"
        f1 = "zoom"
        "#,
    );
    let out = env.run(&["keys"]);
    assert_eq!(out.code, Some(0), "a rejected row is not a failed command");

    assert!(
        out.stdout.contains("prefix ctrl+t, from config.toml"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("q       detach            config.toml"),
        "the row the file added is marked as the file's:\n{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("w       navigate          shipped"),
        "and the rows it did not mention are still marked shipped:\n{}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains("ctrl+t  literal           prefix escape"),
        "the escape followed the prefix:\n{}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("ctrl+a"),
        "and left no stale row behind on the shipped prefix:\n{}",
        out.stdout
    );

    // The row that did not resolve is named, on stderr, with the reason.
    assert!(out.stderr.contains("f1"), "{out:?}");
    assert!(out.stderr.contains("escape sequence"), "{out:?}");
}

#[test]
fn keys_json_carries_the_table_the_file_it_read_and_what_it_rejected() {
    let env = Env::new("keysjson");
    let path = env.config("[keys]\nprefix = \"ctrl+t\"\n[keys.bind]\nf1 = \"zoom\"\n");
    let out = env.run(&["keys", "--json"]);
    assert_eq!(out.code, Some(0), "{out:?}");

    let json: serde_json::Value = serde_json::from_str(&out.stdout).expect("keys --json is JSON");
    assert_eq!(json["config"], path.display().to_string());
    assert_eq!(json["prefix"]["key"], "ctrl+t");
    assert_eq!(json["prefix"]["source"], "config.toml");

    let rows = json["bindings"].as_array().expect("bindings is an array");
    let literal = rows
        .iter()
        .find(|row| row["action"] == "literal")
        .expect("the escape is a row like any other");
    assert_eq!(literal["key"], "ctrl+t");
    assert_eq!(literal["source"], "prefix escape");

    let rejected = json["rejected"].as_array().expect("rejected is an array");
    assert_eq!(rejected.len(), 1, "{rejected:?}");
    assert_eq!(rejected[0]["section"], "keys");
    assert!(
        rejected[0]["message"]
            .as_str()
            .expect("a message")
            .contains("f1"),
        "{rejected:?}"
    );
}

#[tokio::test]
async fn a_rebound_prefix_reaches_an_attached_client() {
    let env = Env::new("prefixrb");
    env.config("[keys]\nprefix = \"ctrl+t\"\n");

    let mut client = env.spawn_on_tty(&[], ROWS, COLS);
    client.wait_for(ALT_ENTER);

    // Both halves of the claim in one positive edge. The shipped prefix goes
    // first and must be an ordinary byte; the rebound one follows and must
    // open the layer, which the status line says out loud. Had `ctrl+a` opened
    // it, `ctrl+t` would have arrived as an unknown prefix key — swallowed,
    // mode back to terminal — and PREFIX would never be painted at all.
    client.send(&[PREFIX]);
    client.send(&[REBOUND]);
    client.wait_for(b"PREFIX");

    // And the layer the rebound prefix opened is the shipped table.
    client.send(b"d");
    assert_eq!(client.wait(), Some(0), "the rebound prefix reaches detach");
    assert!(
        window(client.output(), ALT_LEAVE),
        "and leaves the terminal the way every other detach does"
    );
}

#[tokio::test]
async fn a_config_file_that_binds_nothing_leaves_the_client_exactly_as_shipped() {
    // The differential for the test above, and the acceptance's first clause:
    // a malformed section is a no-op, not a client with no bindings at all.
    let env = Env::new("prefixjunk");
    env.config("[keys]\nprefix = \"f1\"\n[keys.bind]\nd = \"teleport\"\n");

    let mut client = env.spawn_on_tty(&[], ROWS, COLS);
    client.wait_for(ALT_ENTER);
    client.chord(b'd');
    assert_eq!(
        client.wait(),
        Some(0),
        "the shipped prefix and the shipped `d` both still work"
    );
    assert!(window(client.output(), ALT_LEAVE));
}
