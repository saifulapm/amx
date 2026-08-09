//! X07 acceptance, first half: what `[client]` and `[keys]` resolve to.
//!
//! The resolution is a pure function over a parsed [`Config`], so these drive
//! it directly and `input.rs` drives the machine the result feeds. The division
//! matters: a table that resolves correctly and a machine that runs it are two
//! claims, and a suite that only made one of them would be the shape of bug
//! D-M4-8 exists to fix — a mechanism that is right in a type and absent in a
//! running amx.
//!
//! The rule every case here is about is `amx-core::config`'s own, one level
//! further down than that module can enforce it: **a row that does not resolve
//! loses that row and nothing else.** The config module can keep a section
//! whose TOML does not parse; it cannot know that `f1` is not a key this
//! client can bind, and that is what this file pins.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_client::config::{self, Bindings, KeyError, PrefixAction, Source};
use amx_core::Config;

/// Resolve a `config.toml` body, as `load` would after reading it.
fn resolve(text: &str) -> (config::Settings, Vec<amx_core::ConfigDiagnostic>) {
    let (parsed, whole_file) = amx_core::config::reload(&Config::default(), text);
    assert!(
        whole_file.is_empty(),
        "the fixture is meant to parse as TOML: {whole_file:?}"
    );
    config::resolve(&parsed)
}

/// The action bound to `key`, if any.
fn action(bindings: &Bindings, key: u8) -> Option<PrefixAction> {
    bindings.action(key)
}

#[test]
fn an_absent_section_resolves_to_exactly_what_amx_ships() {
    let (settings, diagnostics) = resolve("");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(
        settings.bindings,
        Bindings::default(),
        "a file with no [keys] leaves the shipped table alone"
    );
    assert_eq!(settings.narrow_cols.0, 60, "10 §D14's threshold");
    assert!(
        !settings.mouse,
        "X01's outcome (b): asking for the mouse costs the user their \
         terminal's own selection, so it is not the default",
    );
    assert_eq!(settings.bindings.prefix(), 0x01, "ctrl+a, 04 §7");
    assert_eq!(settings.bindings.prefix_source(), Source::Shipped);
}

#[test]
fn a_section_that_resolves_nothing_leaves_the_shipped_bindings_untouched() {
    // Every row is wrong in a different way: a prefix that is not one byte, a
    // key that is not one byte, a key nothing names, and an action that does
    // not exist. The section is valid TOML throughout, so the config module's
    // own leniency never fires and this is entirely X07's rule.
    let (settings, diagnostics) = resolve(
        r#"
        [keys]
        prefix = "f1"
        [keys.bind]
        up = "zoom"
        wiggle = "detach"
        q = "teleport"
        "#,
    );
    assert_eq!(
        settings.bindings,
        Bindings::default(),
        "four bad rows changed nothing: {:?}",
        settings.bindings,
    );
    assert_eq!(diagnostics.len(), 4, "{diagnostics:?}");
    for entry in &diagnostics {
        assert_eq!(entry.section, Some("keys"), "{entry:?}");
    }
    // Each names the row it rejected, so a user knows which line to fix rather
    // than that "the keys section" was wrong.
    let messages = diagnostics
        .iter()
        .map(|entry| entry.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for fragment in ["prefix = \"f1\"", "\"up\"", "\"wiggle\"", "\"q\""] {
        assert!(messages.contains(fragment), "{fragment}: {messages}");
    }
    assert!(
        messages.contains("split-horizontal"),
        "the unknown-action complaint lists the actions there are: {messages}"
    );
}

#[test]
fn a_bind_row_replaces_one_row_and_leaves_the_rest() {
    let (settings, diagnostics) = resolve(
        r#"
        [keys.bind]
        z = "picker"
        n = "next-attention"
        "#,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let bindings = &settings.bindings;
    assert_eq!(
        action(bindings, b'z'),
        Some(PrefixAction::Picker),
        "replaced"
    );
    assert_eq!(
        action(bindings, b'n'),
        Some(PrefixAction::NextAttention),
        "added"
    );
    assert_eq!(
        action(bindings, b'a'),
        Some(PrefixAction::NextAttention),
        "the shipped row for the same action stays: a bind adds a way in, it \
         does not take the old one away"
    );
    for (key, shipped) in [
        (b'w', PrefixAction::Navigate),
        (b'x', PrefixAction::SplitHorizontal),
        (b'v', PrefixAction::SplitVertical),
        (b'd', PrefixAction::Detach),
        (b'p', PrefixAction::Picker),
    ] {
        assert_eq!(action(bindings, key), Some(shipped), "{key:?} moved");
    }

    // And the provenance is per row, which is what `amx keys` prints.
    let sources: Vec<(String, &str)> = bindings
        .rows()
        .map(|(key, binding)| (config::key_name(key), binding.source.name()))
        .collect();
    assert!(
        sources.contains(&("z".to_owned(), "config.toml")),
        "{sources:?}"
    );
    assert!(
        sources.contains(&("n".to_owned(), "config.toml")),
        "{sources:?}"
    );
    assert!(
        sources.contains(&("w".to_owned(), "shipped")),
        "{sources:?}"
    );
}

#[test]
fn a_rebound_prefix_takes_its_escape_row_with_it() {
    let (settings, diagnostics) = resolve(
        r#"
        [keys]
        prefix = "ctrl+t"
        "#,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let bindings = &settings.bindings;
    assert_eq!(bindings.prefix(), 0x14, "ctrl+t");
    assert_eq!(bindings.prefix_source(), Source::Config);
    assert_eq!(
        action(bindings, 0x14),
        Some(PrefixAction::Literal),
        "the prefix-twice escape follows the prefix"
    );
    assert_eq!(
        action(bindings, 0x01),
        None,
        "and the old prefix keeps no row at all: it is an ordinary byte now"
    );
}

#[test]
fn the_prefix_escape_cannot_be_bound_away() {
    // The one row a file cannot take: a user who has moved the prefix to a key
    // their phone keyboard can emit still needs a way to send that key.
    let (settings, diagnostics) = resolve(
        r#"
        [keys]
        prefix = "ctrl+t"
        [keys.bind]
        "ctrl+t" = "zoom"
        "#,
    );
    assert_eq!(
        action(&settings.bindings, 0x14),
        Some(PrefixAction::Literal),
        "the escape survived a row aimed at it"
    );
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(
        diagnostics[0].message.contains("cannot take effect"),
        "a row that was overruled says so rather than vanishing: {:?}",
        diagnostics[0],
    );
}

#[test]
fn a_key_amx_cannot_read_as_one_byte_is_refused_by_name() {
    // The refusals are the design, not an omission: the input machine matches
    // bytes so kitty sequences reach the pane intact, and a multi-byte binding
    // would put a second key decoder in the one place 04 §7 forbids it.
    assert!(matches!(
        config::key_byte("f4"),
        Err(KeyError::NotOneByte { .. })
    ));
    assert!(matches!(
        config::key_byte("hyper+x"),
        Err(KeyError::UnknownModifier(_))
    ));
    assert!(matches!(
        config::key_byte("wiggle"),
        Err(KeyError::UnknownKey(_))
    ));
    assert!(matches!(config::key_byte(""), Err(KeyError::Empty)));

    // What it does read is the `pane.send_keys` spelling, kept identical so a
    // user who has read one part of the docs can write the other.
    assert_eq!(config::key_byte("ctrl+a"), Ok(0x01));
    assert_eq!(config::key_byte("CTRL+A"), Ok(0x01), "case-insensitive");
    assert_eq!(config::key_byte("esc"), Ok(0x1b));
    assert_eq!(config::key_byte("space"), Ok(0x20));
    assert_eq!(
        config::key_byte("+"),
        Ok(b'+'),
        "plus is a key, not only a separator"
    );
    assert_eq!(config::key_byte("ctrl+_"), Ok(0x1f));
    // ...and `ctrl` with a key it makes no control byte out of is refused with
    // the reason, rather than folded onto some nearby byte.
    assert!(matches!(
        config::key_byte("ctrl++"),
        Err(KeyError::NotOneByte { .. })
    ));
}

#[test]
fn the_narrow_threshold_is_read_and_overridable() {
    let (settings, diagnostics) = resolve("[client]\nnarrow_cols = 45\n");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(settings.narrow_cols.0, 45);
    assert!(settings.narrow_cols.is_narrow(44));
    assert!(!settings.narrow_cols.is_narrow(45));
}

/// D14's wheel exception is opt-in, and the opt-in is one key. The phone
/// profile is what sets it (X20), which is the right place: the people who need
/// touch-scroll are the people not selecting text with a mouse.
#[test]
fn the_mouse_switch_is_off_until_a_file_turns_it_on() {
    let (shipped, diagnostics) = resolve("[client]\nnarrow_cols = 45\n");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert!(
        !shipped.mouse,
        "a [client] section that says nothing about \
        the mouse must not turn it on"
    );

    let (asked, diagnostics) = resolve("[client]\nmouse = true\n");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert!(asked.mouse);
    assert_eq!(
        asked.narrow_cols.0, 60,
        "the other key in the section keeps its default",
    );
}

#[test]
fn an_unreadable_file_keeps_the_shipped_settings_and_says_so() {
    // A directory where a file should be: read_to_string fails with something
    // that is not NotFound, which is the one branch a missing file does not
    // cover and the one a client must not die on.
    let dir = std::env::temp_dir().join(format!("amx-cfg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create the stand-in");
    let (settings, diagnostics) = config::load(&dir);
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(settings, config::Settings::default());
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(
        diagnostics[0].section, None,
        "an unreadable file is a whole-file failure, not a section's"
    );

    // And a file that is simply not there is not a complaint at all.
    let absent = std::env::temp_dir().join("amx-config-that-is-not-there.toml");
    let (settings, diagnostics) = config::load(&absent);
    assert_eq!(settings, config::Settings::default());
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}
