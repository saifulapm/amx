//! The agent registry, and the conformance test that keeps it honest (V03).
//!
//! D-M2-2 trades a compile-time guarantee for a test, and says exactly what the
//! test owes in exchange:
//!
//! > What replaces the compile-time guarantees is the **conformance test**
//! > (V03): it walks the parsed embedded registry and asserts every generated
//! > surface agrees — ids and aliases unique across stanzas, every named
//! > manifest present and compiling, every resume template well-formed (exactly
//! > one `{ref}`, absolute or bare program name, no shell metacharacters — argv
//! > is data, never a shell string), every `edges`/`full` stanza naming an
//! > integration asset, every `coverage` value paired with the tiers it needs.
//! > A stanza that lies fails the build's test run.
//!
//! The subject is [`registry::BUILTIN`] — the compiled-in string — and not a
//! file found on disk, so what is proved here is what the binary ships.
//!
//! One assertion is unusual and deliberate: `conformance_coverage_classes_
//! match_spike_findings` reads `docs/notes/hook-coverage.md`'s own proposed
//! registry block and compares it field by field with the shipped stanzas — the
//! findings document is the *fixture*. 05 M2 requires coverage classes "set from
//! measurements, not hope", and a class copied by hand drifts from the document
//! the first week either changes; this makes that drift a test failure rather
//! than a silent lie about an agent's hook system.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use amx_core::agent::{AgentKind, CoverageClass, RefKind};
use amx_server::agent::registry::{self, REF_SLOT, Registry};
use support::TempDir;

/// The spike's findings, which the shipped classes are copied from.
const FINDINGS: &str = "docs/notes/hook-coverage.md";

/// Where V06's bundled manifests live, relative to the crate root.
const MANIFEST_DIR: &str = "assets/manifests";

/// A whole, valid override of the `claude` builtin — the shape D-M2-2 requires,
/// since a replacement is never a field merge. The rejection cases break
/// exactly one of its rules each, so what each one proves is that rule.
const CLAUDE_REPLACEMENT: &str = r#"
[[agent]]
id          = "claude"
label       = "Claude Code (local build)"
executables = ["claude"]
coverage    = "identity"
start       = ["/opt/claude/bin/claude"]
resume      = ["/opt/claude/bin/claude", "--resume", "{ref}"]
manifest    = "claude.toml"
"#;

fn kind(name: &str) -> AgentKind {
    AgentKind::new(name).expect("a well-formed agent id")
}

/// The registry this binary ships, with its parse refusing to have refused
/// anything.
fn builtin() -> Registry {
    let (registry, diagnostics) = Registry::parse(registry::BUILTIN);
    assert!(
        diagnostics.is_empty(),
        "the embedded agents.toml did not load clean: {}",
        diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    );
    registry
}

/// The crate root — `crates/amx-server`; the repository root is two above it.
fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn registry_parses_embedded_stanzas_for_claude_and_codex() {
    let registry = builtin();

    let ids: Vec<&str> = registry
        .stanzas()
        .iter()
        .map(|stanza| stanza.id.as_str())
        .collect();
    assert_eq!(
        ids,
        ["claude", "codex"],
        "M2 ships exactly the two agents 05's installer scope names, in file order"
    );

    let claude = registry
        .resolve(&kind("claude"))
        .expect("the claude stanza");
    assert_eq!(claude.label, "Claude Code");
    assert_eq!(claude.executables, ["claude"]);
    assert_eq!(claude.start, ["claude"]);
    assert_eq!(claude.resume, ["claude", "--resume", REF_SLOT]);
    assert_eq!(claude.ref_kind, RefKind::Id);
    assert_eq!(claude.manifest.as_deref(), Some("claude.toml"));
    // V01 §6: "raise to 5 s for Codex, keep 3 s for Claude" — the sentence that
    // made this a per-stanza field rather than one constant.
    assert_eq!(claude.startup_grace_ms, Some(3_000));

    let codex = registry.resolve(&kind("codex")).expect("the codex stanza");
    assert_eq!(codex.label, "Codex CLI");
    assert_eq!(codex.resume, ["codex", "resume", REF_SLOT]);
    assert_eq!(codex.startup_grace_ms, Some(5_000));
    // Codex has no `Notification` event at all, so a Codex pane gets no delayed
    // corroboration of a block or an idle (V01 §4). Subscribing one would be a
    // promise the tool cannot keep.
    assert!(!codex.hook_events.iter().any(|e| e == "Notification"));
}

#[test]
fn alias_and_id_lookup_resolve_to_one_stanza() {
    let registry = builtin();

    for stanza in registry.stanzas() {
        for name in stanza.names() {
            let resolved = registry
                .resolve(name)
                .unwrap_or_else(|| panic!("`{name}` is declared but resolves to nothing"));
            assert_eq!(
                resolved.id, stanza.id,
                "`{name}` resolves to `{}`, not to the stanza that declares it",
                resolved.id
            );
        }
    }

    assert_eq!(
        registry.resolve(&kind("claude-code")).map(|s| &s.id),
        registry.resolve(&kind("claude")).map(|s| &s.id),
        "an alias and its id are one stanza, not two"
    );
    assert!(
        registry.resolve(&kind("no-such-agent")).is_none(),
        "an unknown name resolves to nothing rather than to the first stanza"
    );
}

#[test]
fn conformance_ids_and_aliases_are_unique_across_stanzas() {
    let registry = builtin();

    let mut seen = BTreeSet::new();
    for stanza in registry.stanzas() {
        for name in stanza.names() {
            assert!(
                seen.insert(name.clone()),
                "`{name}` is declared twice; a name resolves to one agent"
            );
        }
        // Every stanza also passes the checks admission runs, which is what a
        // user's override is held to. Asserted here as well so a shipped stanza
        // that lies names *itself* in the failure rather than failing inside a
        // parse that swallowed it.
        stanza
            .check()
            .unwrap_or_else(|problem| panic!("the `{}` stanza lies: {problem}", stanza.id));
    }
}

#[test]
fn conformance_resume_templates_carry_exactly_one_ref_slot() {
    for stanza in builtin().stanzas() {
        assert!(
            !stanza.resume.is_empty(),
            "the `{}` stanza cannot be resumed, which V15 needs it to be",
            stanza.id
        );

        let slots = stanza.resume.iter().filter(|arg| *arg == REF_SLOT).count();
        assert_eq!(
            slots, 1,
            "the `{}` resume template holds {slots} `{REF_SLOT}` elements",
            stanza.id
        );
        // The ref is a whole element — `--resume={ref}` would substitute into a
        // fragment, one quoting question away from a shell string — and every
        // other element is a literal amx spawns, so only the slot holds braces.
        for argument in stanza.resume.iter().filter(|arg| *arg != REF_SLOT) {
            assert!(
                !argument.contains(REF_SLOT),
                "the `{}` template buries the slot inside {argument:?}",
                stanza.id
            );
            assert!(
                !argument
                    .chars()
                    .any(|c| c.is_control() || "|&;<>()$`\\\"'*?[]{}~!#".contains(c)),
                "the `{}` template's {argument:?} holds shell syntax; argv is data",
                stanza.id
            );
        }

        let program = &stanza.resume[0];
        assert!(
            !program.contains('/') || Path::new(program).is_absolute(),
            "the `{}` program {program:?} resolves against the server's cwd",
            stanza.id
        );
    }
}

#[test]
fn conformance_every_stanza_names_a_present_compiling_manifest() {
    let manifests = crate_root().join(MANIFEST_DIR);

    for stanza in builtin().stanzas() {
        // The pairing rule, read off the class rather than off a list of agent
        // ids: `edges` and `identity` both leave held states through the
        // screen, so neither can work without tier-2 rules.
        if !matches!(
            stanza.coverage,
            CoverageClass::Edges | CoverageClass::Identity
        ) {
            continue;
        }
        let name = stanza
            .manifest
            .as_deref()
            .unwrap_or_else(|| panic!("class `{}` needs a manifest", stanza.coverage));
        assert!(
            !name.contains('/') && name.ends_with(".toml"),
            "the `{}` stanza's manifest {name:?} is not a bare file name",
            stanza.id
        );

        // V06 owns `assets/manifests/**` and lands in the same wave as this
        // task, so on a branch that has only V03 the directory does not exist
        // yet. The moment it does, every named manifest must be in it — this
        // check has no other mode, and it is V17's seam if the two disagree.
        if !manifests.is_dir() {
            continue;
        }
        let path = manifests.join(name);
        assert!(
            path.is_file(),
            "the `{}` stanza names {name}, which is not in {MANIFEST_DIR}",
            stanza.id
        );
        // "Compiling" as far as V03 can judge it: the rule grammar is V06's, so
        // what is asserted here is that the file is a TOML document at all and
        // not, say, a half-written one.
        let text = std::fs::read_to_string(&path).expect("read the manifest");
        text.parse::<toml::Table>()
            .unwrap_or_else(|err| panic!("{name} is not a TOML document: {err}"));
    }
}

#[test]
fn conformance_coverage_classes_match_spike_findings() {
    let doc = crate_root().join("../..").join(FINDINGS);
    let text =
        std::fs::read_to_string(&doc).unwrap_or_else(|err| panic!("read {}: {err}", doc.display()));

    // The findings block is a *fragment* — the two fields V01 measured, not a
    // whole stanza — so it is read as raw TOML rather than through the stanza
    // deserializer, and compared field by field.
    let block = toml_block(&text);
    let document: toml::Table = block.parse().expect("the findings block is TOML");
    let proposed = document
        .get(registry::AGENT_ARRAY)
        .and_then(toml::Value::as_array)
        .expect("the findings block is an `[[agent]]` array");

    let registry = builtin();
    let mut measured = BTreeSet::new();

    for entry in proposed {
        let id = entry
            .get("id")
            .and_then(toml::Value::as_str)
            .expect("every proposed stanza names an id");
        measured.insert(id.to_owned());

        let stanza = registry.resolve(&kind(id)).unwrap_or_else(|| {
            panic!("{FINDINGS} measured `{id}`, which the registry does not ship")
        });

        let coverage = entry
            .get("coverage")
            .and_then(toml::Value::as_str)
            .expect("every proposed stanza names a coverage class");
        assert_eq!(
            stanza.coverage.as_str(),
            coverage,
            "`{id}` ships coverage `{}` while {FINDINGS} measured `{coverage}`. \
             Classes are set from measurements, not hope (05, M2): re-run \
             scripts/spike/ before changing either.",
            stanza.coverage
        );

        let events = entry
            .get("hook_events")
            .and_then(toml::Value::as_array)
            .expect("every proposed stanza lists its hook events");
        let expected: BTreeSet<&str> = events.iter().filter_map(toml::Value::as_str).collect();
        let shipped: BTreeSet<&str> = stanza.hook_events.iter().map(String::as_str).collect();
        assert_eq!(
            shipped, expected,
            "`{id}` subscribes a different event set than {FINDINGS} §5 proposes. \
             Each subscription costs a process spawn per occurrence, and the \
             document says which ones earn it."
        );
    }

    let shipped: BTreeSet<String> = registry
        .stanzas()
        .iter()
        .map(|stanza| stanza.id.to_string())
        .collect();
    assert_eq!(
        shipped, measured,
        "the registry and {FINDINGS} disagree about which agents M2 ships"
    );
}

/// The `[[agent]]` block the findings document proposes, out of its §5 fence.
fn toml_block(text: &str) -> String {
    let mut lines = text.lines();
    assert!(
        lines.any(|line| line.starts_with("## ") && line.contains("Proposed registry data")),
        "{FINDINGS} has no \"Proposed registry data\" section; the fixture moved"
    );
    let fence = lines
        .by_ref()
        .find(|line| line.trim_start().starts_with("```"))
        .expect("the section opens a fenced block");
    assert!(fence.contains("toml"), "the fence is {fence:?}, not TOML");
    lines
        .take_while(|line| !line.trim_start().starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn override_file_adds_an_agent_without_touching_builtins() {
    let dir = TempDir::new("ovrd");
    let config = dir.path().join("amx/config.toml");
    let path = Registry::override_path(&config);
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some(registry::OVERRIDE_NAME),
        "the override sits beside config.toml under the same XDG root"
    );
    std::fs::create_dir_all(path.parent().expect("a parent directory")).expect("create the dir");
    // The V17 seam: a scripted fake agent arrives as a stanza, not as a patched
    // binary.
    std::fs::write(
        &path,
        r#"
[[agent]]
id          = "fake"
aliases     = ["fixture"]
label       = "Scripted fake"
executables = ["fake-agent.sh"]
coverage    = "edges"
start       = ["fake-agent.sh"]
resume      = ["fake-agent.sh", "--resume", "{ref}"]
manifest    = "fake.toml"
hook_events = ["SessionStart", "UserPromptSubmit", "Stop"]
"#,
    )
    .expect("write the override");

    let builtins = builtin();
    let text = std::fs::read_to_string(&path).expect("read the override back");
    let (merged, diagnostics) = builtins.with_override(&text);
    assert_eq!(diagnostics, [], "the override loaded clean");

    // Added, at the tail: builtins first, then overrides.
    let ids: Vec<&str> = merged
        .stanzas()
        .iter()
        .map(|stanza| stanza.id.as_str())
        .collect();
    assert_eq!(ids, ["claude", "codex", "fake"]);
    assert_eq!(
        merged.resolve(&kind("fixture")).map(|s| s.id.as_str()),
        Some("fake"),
        "an override's aliases resolve like any other"
    );

    // Untouched: byte for byte the stanzas the binary shipped.
    for id in ["claude", "codex"] {
        assert_eq!(
            merged.resolve(&kind(id)),
            builtins.resolve(&kind(id)),
            "`{id}` changed although the override never named it"
        );
    }

    // And replacement is whole-agent, never a field merge: an override of
    // `claude` that drops `aliases` drops them, rather than inheriting the
    // builtin's.
    let (replaced, diagnostics) = builtins.with_override(CLAUDE_REPLACEMENT);
    assert_eq!(diagnostics, []);
    let claude = replaced.resolve(&kind("claude")).expect("still there");
    assert_eq!(claude.coverage, CoverageClass::Identity);
    assert_eq!(claude.aliases, [], "the replacement's fields, not a blend");
    assert!(
        replaced.resolve(&kind("claude-code")).is_none(),
        "the builtin's alias went with the stanza it belonged to"
    );
    assert_eq!(
        replaced.stanzas().len(),
        2,
        "a replacement replaces in place; it does not append"
    );
}

#[test]
fn malformed_override_is_rejected_with_the_builtin_kept() {
    let builtins = builtin();
    let before = builtins.resolve(&kind("claude")).cloned();

    // M1's rule, one level down: a whole-file failure keeps everything.
    let (merged, diagnostics) = builtins.with_override("[[agent]\nid = \"claude\"");
    assert_eq!(merged, builtins, "a broken file yanked live agents away");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].agent, None,
        "a whole-file failure names no agent"
    );

    // Each case is [`CLAUDE_REPLACEMENT`] with exactly one rule broken.
    let cases = [
        // A partial stanza — D-M2-2's own example — is rejected, not merged
        // field by field over the builtin.
        (
            "a stanza missing a required field",
            CLAUDE_REPLACEMENT.replace("label       = \"Claude Code (local build)\"\n", ""),
        ),
        (
            "a resume template with two ref slots",
            CLAUDE_REPLACEMENT.replace(r#""{ref}"]"#, r#""{ref}", "{ref}"]"#),
        ),
        (
            "a start argv holding shell syntax",
            CLAUDE_REPLACEMENT.replace(
                r#"start       = ["/opt/claude/bin/claude"]"#,
                r#"start       = ["/opt/claude/bin/claude", "&&", "rm -rf /"]"#,
            ),
        ),
        (
            "a screen-owned class naming no manifest",
            CLAUDE_REPLACEMENT.replace("manifest    = \"claude.toml\"\n", ""),
        ),
        (
            "an alias belonging to another agent",
            CLAUDE_REPLACEMENT.replace(
                r#"id          = "claude""#,
                "id          = \"impostor\"\naliases     = [\"claude\"]",
            ),
        ),
    ];

    for (what, text) in &cases {
        let (merged, diagnostics) = builtins.with_override(text);
        assert_eq!(
            diagnostics.len(),
            1,
            "{what} should be refused with one diagnostic"
        );
        assert_eq!(
            merged.resolve(&kind("claude")).cloned(),
            before,
            "{what} changed the builtin it was refused in favour of"
        );
        assert_eq!(
            merged.stanzas().len(),
            builtins.stanzas().len(),
            "{what} still added a stanza"
        );
    }

    // The loss is reported by name: a user who broke one stanza is told which,
    // and which rule they broke. 04 §6's rule for restore, applied to config.
    let (_, diagnostics) = builtins.with_override(&cases[1].1);
    assert_eq!(
        diagnostics[0].agent.as_ref().map(AgentKind::as_str),
        Some("claude")
    );
    assert!(
        diagnostics[0].to_string().contains(REF_SLOT),
        "the diagnostic names the rule broken: {}",
        diagnostics[0]
    );
}
