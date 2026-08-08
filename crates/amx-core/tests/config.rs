//! Config reload: two tiers of leniency, from a running config and some text.
//!
//! D-M1-8's rule is that a config file is edited while sessions are live, so a
//! half-typed file must cost the user nothing. That makes the interesting
//! assertions about what *survives* a bad file rather than what a good file
//! produces: the whole-file tier keeps everything, the per-section tier keeps
//! one section, and both report what they rejected instead of logging it.

use amx_core::config::{PERSIST_SECTION, SECTIONS, TERMINAL_SECTION, UPDATE_SECTION, reload};
use amx_core::{Config, PersistConfig, TerminalConfig, UpdateConfig};

/// A running config that differs from `Config::default()` in every field, so a
/// test asserting "kept" can never pass by accidentally producing defaults.
fn running() -> Config {
    Config {
        persist: PersistConfig { history: true },
        terminal: TerminalConfig {
            shell: Some("/bin/zsh".to_owned()),
        },
        update: UpdateConfig {
            channel: Some("https://example.invalid/latest.json".to_owned()),
        },
    }
}

/// The configuration of a machine with no file at all. A missing file must be
/// indistinguishable from an empty one, which is what makes "no config" a
/// normal startup rather than an error.
#[test]
fn defaults_are_the_no_file_configuration() {
    let defaults = Config::default();
    assert!(!defaults.persist.history, "scrollback holds secrets");
    assert_eq!(defaults.terminal.shell, None);
    assert_eq!(
        defaults.update.channel, None,
        "no channel override means the built-in default, not a second copy of it",
    );
    assert_eq!(
        SECTIONS,
        &[PERSIST_SECTION, TERMINAL_SECTION, UPDATE_SECTION]
    );

    let (next, diagnostics) = reload(&running(), "");
    assert_eq!(next, defaults, "an empty file is the missing file");
    assert!(diagnostics.is_empty());
}

#[test]
fn parse_error_keeps_the_entire_running_config() {
    let running = running();
    let (next, diagnostics) = reload(&running, "[persist\nhistory = ");

    assert_eq!(next, running, "a typo mid-edit must not yank live settings");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].section, None,
        "a whole-file failure names no section, because none applied",
    );
    assert!(!diagnostics[0].message.is_empty());
}

#[test]
fn invalid_section_keeps_current_values_while_valid_sections_apply() {
    let running = running();
    let text = "\
[persist]
history = \"yes\"

[terminal]
shell = \"/bin/fish\"
";
    let (next, diagnostics) = reload(&running, text);

    assert!(
        next.persist.history,
        "the rejected section keeps what was running, not its default",
    );
    assert_eq!(
        next.terminal.shell.as_deref(),
        Some("/bin/fish"),
        "one bad section must not hold the rest of the file back",
    );
    assert_eq!(diagnostics.len(), 1, "one rejection, one diagnostic");
}

#[test]
fn absent_section_resets_to_defaults() {
    let running = running();
    let (next, diagnostics) = reload(&running, "[terminal]\nshell = \"/bin/fish\"\n");

    assert_eq!(
        next.persist,
        PersistConfig::default(),
        "a deleted section returns to its default, so the file is the truth",
    );
    assert_eq!(next.terminal.shell.as_deref(), Some("/bin/fish"));
    assert!(diagnostics.is_empty(), "deleting a section is not an error");

    // Reload is idempotent with file content: feeding the same text to the
    // config it just produced changes nothing.
    let (again, diagnostics) = reload(&next, "[terminal]\nshell = \"/bin/fish\"\n");
    assert_eq!(again, next);
    assert!(diagnostics.is_empty());
}

#[test]
fn unknown_keys_and_sections_are_ignored_like_the_wire() {
    let running = running();
    let text = "\
[persist]
history = true
compression = \"zstd\"

[terminal]
shell = \"/bin/fish\"
scrollback = 10000

[colours]
background = \"#000000\"
";
    let (next, diagnostics) = reload(&running, text);

    assert!(next.persist.history);
    assert_eq!(next.terminal.shell.as_deref(), Some("/bin/fish"));
    assert!(
        diagnostics.is_empty(),
        "unknown keys and sections are tolerated silently, both directions",
    );
}

#[test]
fn diagnostics_name_the_failing_section() {
    let running = running();
    let text = "\
[persist]
history = 3

[terminal]
shell = 7

[update]
channel = 7
";
    let (next, diagnostics) = reload(&running, text);

    assert_eq!(next, running, "both sections were rejected, both were kept");
    let named: Vec<Option<&'static str>> = diagnostics.iter().map(|d| d.section).collect();
    assert_eq!(
        named,
        vec![
            Some(PERSIST_SECTION),
            Some(TERMINAL_SECTION),
            Some(UPDATE_SECTION)
        ]
    );
    for diagnostic in &diagnostics {
        assert!(
            !diagnostic.message.is_empty(),
            "a diagnostic without the parser's words cannot be acted on",
        );
    }
}

/// A section that is present but not a table at all — `persist = 5` — is a
/// section failure, not a file failure: the rest of the file still applies.
#[test]
fn a_section_that_is_not_a_table_fails_alone() {
    let running = running();
    let (next, diagnostics) = reload(&running, "persist = 5\n[terminal]\nshell = \"/bin/fish\"\n");

    assert!(next.persist.history, "kept, because it was rejected");
    assert_eq!(next.terminal.shell.as_deref(), Some("/bin/fish"));
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].section, Some(PERSIST_SECTION));
}
