//! X20 acceptance: the configuration reference describes the tree it ships in.
//!
//! `docs/12-config.md` and `examples/keys-phone.toml` make claims that go stale
//! silently — a section added to `SECTIONS`, an action added to
//! `PrefixAction`, a shipped binding moved to another key — and a document that
//! quietly stops being true is worse than no document, because it is read.
//! These tests are the pin: what the reference says about sections, actions and
//! the shipped table is checked against the code that answers for them, and
//! every TOML example in it — including the one the profile ships as a file —
//! is resolved through the real reader rather than eyeballed.
//!
//! They live in `amx-client` because that is the crate that resolves `[client]`
//! and `[keys]`; the two server-side sections are checked by name only, which
//! is all a reference has to get right about them.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::fs;
use std::path::{Path, PathBuf};

use amx_client::config::{self, Bindings, PrefixAction, SHIPPED_PREFIX, Source};
use amx_core::Config;

/// The repository root, from this crate's manifest directory.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the crate sits two levels under the repository root")
}

/// The configuration reference, as text.
fn reference() -> String {
    let path = repo().join("docs/12-config.md");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The phone profile, as text.
fn profile() -> String {
    let path = repo().join("examples/keys-phone.toml");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Every fenced block in `text` whose info string is `info`.
///
/// The reference's examples are the part of it a reader copies, so they are the
/// part worth running rather than reading.
fn fenced<'a>(text: &'a str, info: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    // The info string of the fence currently open, so a block this call is not
    // collecting still closes properly — otherwise its closing fence reads as
    // the opening one of the next block.
    let mut open: Option<String> = None;
    let mut body: Vec<&'a str> = Vec::new();
    for line in text.lines() {
        let Some(fence) = line.strip_prefix("```") else {
            if open.is_some() {
                body.push(line);
            }
            continue;
        };
        match open.take() {
            None => open = Some(fence.trim().to_owned()),
            Some(was) => {
                if was == info {
                    blocks.push(format!("{}\n", body.join("\n")));
                }
                body.clear();
            }
        }
    }
    blocks
}

#[test]
fn the_reference_documents_every_section_amx_reads() {
    let text = reference();
    for section in amx_core::config::SECTIONS {
        assert!(
            text.contains(&format!("`[{section}]`")),
            "docs/12-config.md documents no `[{section}]` section, and \
             amx_core::config::SECTIONS says amx reads one",
        );
    }
}

#[test]
fn the_reference_names_every_action_a_bind_row_can_name() {
    let text = reference();
    for action in PrefixAction::ALL {
        assert!(
            text.contains(&format!("| `{}` |", action.name())),
            "docs/12-config.md's action table has no row for `{}`, which a \
             `[keys] bind` row may name today",
            action.name(),
        );
    }
}

#[test]
fn the_reference_prints_the_table_amx_actually_ships() {
    let text = reference();
    let shipped = Bindings::default();
    let block = fenced(&text, "")
        .into_iter()
        .find(|block| block.starts_with("prefix "))
        .expect("docs/12-config.md shows the shipped table verbatim");

    assert!(
        block.starts_with(&format!(
            "prefix {}, from {}\n",
            config::key_name(shipped.prefix()),
            shipped.prefix_source().name(),
        )),
        "the shipped prefix line in docs/12-config.md is not what a client \
         resolves with no `[keys]` section:\n{block}",
    );

    let rows: Vec<&str> = block
        .lines()
        .skip_while(|line| !line.starts_with("key "))
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(
        rows.len(),
        shipped.rows().count(),
        "docs/12-config.md prints {} rows and amx ships {}:\n{block}",
        rows.len(),
        shipped.rows().count(),
    );
    for (key, binding) in shipped.rows() {
        let name = config::key_name(key);
        assert!(
            rows.iter().any(|row| {
                let mut fields = row.split_whitespace();
                fields.next() == Some(name.as_str())
                    && fields.next() == Some(binding.action.name())
                    && row.ends_with(binding.source.name())
            }),
            "docs/12-config.md has no row for {name} → {} ({}), which amx \
             ships:\n{block}",
            binding.action.name(),
            binding.source.name(),
        );
    }
}

#[test]
fn every_toml_example_in_the_reference_is_a_file_amx_can_read() {
    for block in fenced(&reference(), "toml") {
        let (_, diagnostics) = amx_core::config::reload(&Config::default(), &block);
        assert!(
            !diagnostics.iter().any(|entry| entry.section.is_none()),
            "an example in docs/12-config.md is not valid TOML:\n{block}\n{diagnostics:?}",
        );
    }
}

#[test]
fn the_phone_profile_resolves_with_nothing_rejected() {
    let (parsed, whole_file) = amx_core::config::reload(&Config::default(), &profile());
    assert!(whole_file.is_empty(), "{whole_file:?}");
    let (settings, diagnostics) = config::resolve(&parsed);
    assert!(
        diagnostics.is_empty(),
        "the shipped phone profile is a file a user copies; every row of it has \
         to resolve: {diagnostics:?}",
    );

    // The two settings the profile exists to change, and the one it leaves
    // alone. `mouse` is the asymmetry D14 is about: off everywhere else, on
    // here, because a touch-scroll gesture arrives as a wheel event.
    assert!(settings.mouse, "the phone profile turns mouse reporting on");
    assert_eq!(
        settings.narrow_cols,
        config::NarrowCols::default(),
        "the profile leaves the narrow threshold at the shipped default; a \
         phone is below it either way",
    );

    let prefix = settings.bindings.prefix();
    assert_ne!(
        prefix, SHIPPED_PREFIX,
        "a phone profile that kept `ctrl+a` would be a profile that changed \
         nothing about the keyboard it exists for",
    );
    assert_eq!(settings.bindings.prefix_source(), Source::Config);
    assert_eq!(
        settings.bindings.action(prefix),
        Some(PrefixAction::Literal),
        "pressing the rebound prefix twice must still send it to the pane",
    );
}

#[test]
fn the_reference_and_the_profile_point_at_each_other() {
    assert!(
        reference().contains("examples/keys-phone.toml"),
        "the reference is where a user finds the profile",
    );
    assert!(
        profile().contains("docs/12-config.md"),
        "the profile is where a user finds the reference",
    );
}
