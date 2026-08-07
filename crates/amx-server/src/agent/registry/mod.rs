//! The agent registry: one declarative file, parsed once, override-merged.
//!
//! 04 §5: "A single declarative file (`agents.toml`, compiled in, overridable)
//! is the **only** place an agent is defined: id, aliases, executable, label,
//! resume argv template, hook coverage class, manifest, integration asset.
//! Every consumer — lookup, resume planner, fusion configuration, integration
//! installer, docs table — is generated from it. Adding an agent = one stanza
//! (fixes W6)."
//!
//! D-M2-2 delivers "generated" as parse-at-startup rather than codegen, and
//! says why: every consumer of the registry is *runtime* data, and 04 §5
//! requires the compiled-in registry to be overridable, which forces a runtime
//! parser to exist regardless. Two parse paths — a macro for the builtin, TOML
//! for the override — would be W6 wearing a new hat. One shape, parsed once.
//! R-M2-9 flags the wording in case "generated" was meant strictly.
//!
//! What replaces the compile-time guarantee is V03's **conformance test**: it
//! walks the parsed embedded registry and asserts every derived surface agrees
//! — ids and aliases unique across stanzas, every named manifest present and
//! compiling, every resume template well-formed, every `edges`/`full` stanza
//! naming an integration asset. A stanza that lies fails the test run, which is
//! as close to a compile error as data gets. One of its assertions reads
//! `docs/notes/hook-coverage.md`'s own table as the fixture, so a shipped
//! [`coverage`](AgentStanza::coverage) can never drift from the measurement.
//!
//! Half of that list does not wait for the test: [`AgentStanza::check`] runs at
//! every admission, so the rules the builtin is held to are the rules a user's
//! override is held to, and both fail the same way.
//!
//! # No I/O here
//!
//! [`super`]'s rule holds: this module is pure functions over `&str`, like
//! `amx_core::config`. Reading [`OVERRIDE_NAME`] off disk belongs to whoever
//! assembles the session — V08, in `session/serve.rs` — which is what lets the
//! conformance test walk the compiled-in registry without a filesystem and lets
//! V17's rig plant an override stanza as a string.
//!
//! # Task ownership
//!
//! V02 froze the stanza schema, because V04's fusion, V08's hub and V10's
//! installers all read it and are built in parallel. **V03** fills the parse,
//! the merge and the lookup, adds `assets/agents.toml`, and writes the
//! conformance test.

pub mod stanza;

use std::path::{Path, PathBuf};

use amx_core::agent::AgentKind;

pub use stanza::{AgentStanza, REF_SLOT, StanzaField, StanzaProblem};

/// The registry compiled into this binary, as text.
///
/// Public because it is the conformance test's subject: the test parses *this*
/// string, not a file it found, so what it proves is what the binary ships.
pub const BUILTIN: &str = include_str!("../../../assets/agents.toml");

/// The `[[agent]]` array of tables every registry document is.
pub const AGENT_ARRAY: &str = "agent";

/// The file name of the user's override, beside `config.toml`.
pub const OVERRIDE_NAME: &str = "agents.toml";

/// One thing that went wrong while loading a registry document.
///
/// Shaped like [`ConfigDiagnostic`](amx_core::config::ConfigDiagnostic) and for
/// the same reason: the loser of a rejected stanza is one agent, so the caller
/// needs to say which.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RegistryDiagnostic {
    /// The stanza's id, when it deserialized far enough to have one.
    pub agent: Option<AgentKind>,
    /// Its position in the document's `[[agent]]` array, when it had one.
    pub index: Option<usize>,
    /// What the parser or the check said.
    pub message: String,
}

impl RegistryDiagnostic {
    /// The whole document failed; nothing in it applied.
    fn document(message: impl Into<String>) -> Self {
        Self {
            agent: None,
            index: None,
            message: message.into(),
        }
    }

    /// One stanza failed; every other stanza still applied.
    fn stanza(index: usize, agent: Option<AgentKind>, message: impl Into<String>) -> Self {
        Self {
            agent,
            index: Some(index),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RegistryDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.agent, self.index) {
            (Some(agent), _) => write!(f, "agent `{agent}`: {}", self.message),
            (None, Some(index)) => write!(f, "agent #{index}: {}", self.message),
            (None, None) => f.write_str(&self.message),
        }
    }
}

/// The merged registry: the embedded stanzas, plus the user's overrides.
///
/// The merge rule, from D-M2-2 and V03's prompt: an override stanza adds or
/// *replaces a whole agent*, never field-merges. A partial stanza is rejected
/// with the builtin kept, which is M1's per-section lenient config rule applied
/// one level down — a broken override costs the agent it names and nothing
/// else.
///
/// The override path is also the test seam: V17's rig plants a stanza for a
/// scripted fake agent instead of patching the binary.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Registry {
    stanzas: Vec<AgentStanza>,
}

impl Registry {
    /// The registry compiled into this binary.
    ///
    /// Infallible by construction: [`BUILTIN`] is a compiled-in string and
    /// `crates/amx-server/tests/registry.rs` parses that exact string and
    /// asserts the diagnostic list is empty, so a diagnostic here is a build
    /// whose tests did not run. The debug assertion says so where a developer
    /// would meet it.
    #[must_use]
    pub fn builtin() -> Self {
        let (registry, diagnostics) = Self::parse(BUILTIN);
        debug_assert!(
            diagnostics.is_empty(),
            "the embedded agents.toml is conformance-tested: {diagnostics:?}"
        );
        registry
    }

    /// Parse one registry document, leniently.
    ///
    /// Two tiers, exactly M1's config rule (D-M1-8) one level down:
    ///
    /// - a document that is not a TOML table, or whose `agent` key is not an
    ///   array, yields an empty registry and one diagnostic;
    /// - a stanza that fails to deserialize, or fails [`AgentStanza::check`],
    ///   is dropped with a diagnostic naming it, and every other stanza still
    ///   loads.
    ///
    /// Unknown keys are ignored — the tolerance rule the wire applies in both
    /// directions (04 §4), so a registry written by a newer amx still loads the
    /// agents this build understands.
    #[must_use]
    pub fn parse(text: &str) -> (Self, Vec<RegistryDiagnostic>) {
        let mut registry = Self::default();
        let diagnostics = registry.absorb(text);
        (registry, diagnostics)
    }

    /// Fold a user's override document into this registry.
    ///
    /// D-M2-2's merge rule, and the reason it is not a field merge: **an
    /// override stanza adds or replaces a *whole* agent.** Half a stanza from
    /// the file and half from the builtin would mean a resume template paired
    /// with an executable list it was never measured against — a registry no
    /// document on disk describes and nobody could debug. So a stanza whose
    /// `id` matches a builtin replaces it in place, keeping its position;
    /// anything else appends; and a rejected stanza leaves the builtin exactly
    /// as it was.
    #[must_use]
    pub fn with_override(&self, text: &str) -> (Self, Vec<RegistryDiagnostic>) {
        let mut merged = self.clone();
        let diagnostics = merged.absorb(text);
        (merged, diagnostics)
    }

    /// Where the user's override lives, given [`Ctx::config_path`].
    ///
    /// Derived from the config path rather than from the environment a second
    /// time, so the two files are guaranteed to be the same directory's — a
    /// test pointing `Ctx` at a tempdir moves both together.
    ///
    /// [`Ctx::config_path`]: amx_core::Ctx::config_path
    #[must_use]
    pub fn override_path(config_path: &Path) -> PathBuf {
        config_path.with_file_name(OVERRIDE_NAME)
    }

    /// Every stanza, in file order — builtins first, then overrides.
    ///
    /// File order is load-bearing: it is what breaks priority ties in the
    /// manifest engine and what makes the conformance test's diagnostics point
    /// at a line rather than at a set.
    #[must_use]
    pub fn stanzas(&self) -> &[AgentStanza] {
        &self.stanzas
    }

    /// The stanza `name` resolves to, by id or by alias.
    ///
    /// Ids are checked before aliases, so an alias that collides with another
    /// stanza's id cannot shadow it — though [`admit`](Self::admit) refuses to
    /// let that registry load in the first place.
    #[must_use]
    pub fn resolve(&self, name: &AgentKind) -> Option<&AgentStanza> {
        self.stanzas
            .iter()
            .find(|stanza| &stanza.id == name)
            .or_else(|| {
                self.stanzas
                    .iter()
                    .find(|stanza| stanza.aliases.contains(name))
            })
    }

    /// Read `text` into this registry, collecting what it had to refuse.
    fn absorb(&mut self, text: &str) -> Vec<RegistryDiagnostic> {
        let document = match text.parse::<toml::Table>() {
            Ok(document) => document,
            Err(err) => return vec![RegistryDiagnostic::document(err.to_string())],
        };
        let Some(value) = document.get(AGENT_ARRAY) else {
            // A document with no `[[agent]]` at all is empty, not broken: it is
            // what a user's override file looks like before they write one.
            return Vec::new();
        };
        let Some(array) = value.as_array() else {
            return vec![RegistryDiagnostic::document(format!(
                "`{AGENT_ARRAY}` is an array of tables, not {}",
                value.type_str()
            ))];
        };

        let mut diagnostics = Vec::new();
        for (index, entry) in array.iter().enumerate() {
            match entry.clone().try_into::<AgentStanza>() {
                Ok(stanza) => {
                    if let Err(problem) = self.admit(stanza) {
                        diagnostics.push(problem);
                    }
                }
                // A partial stanza — one missing `id`, `label` or `coverage` —
                // lands here, and D-M2-2 says what happens to it: rejected,
                // with whatever it would have replaced kept.
                Err(err) => {
                    let agent = entry
                        .get("id")
                        .and_then(toml::Value::as_str)
                        .and_then(|id| AgentKind::new(id).ok());
                    diagnostics.push(RegistryDiagnostic::stanza(index, agent, err.to_string()));
                }
            }
        }
        diagnostics
    }

    /// Add or replace one checked stanza.
    fn admit(&mut self, stanza: AgentStanza) -> Result<(), RegistryDiagnostic> {
        let refused = |problem: StanzaProblem| RegistryDiagnostic {
            agent: Some(stanza.id.clone()),
            index: None,
            message: problem.to_string(),
        };
        stanza.check().map_err(refused)?;

        let replacing = self.stanzas.iter().position(|held| held.id == stanza.id);
        // Uniqueness is checked against every stanza *except* the one being
        // replaced: an override of `claude` re-declaring `claude-code` is
        // replacing that alias, not colliding with it.
        for name in stanza.names() {
            let owner = self
                .stanzas
                .iter()
                .enumerate()
                .filter(|(position, _)| Some(*position) != replacing)
                .find(|(_, held)| held.names().any(|held_name| held_name == name));
            if let Some((_, owner)) = owner {
                return Err(refused(StanzaProblem::DuplicateName {
                    name: name.clone(),
                    owner: owner.id.clone(),
                }));
            }
        }

        match replacing {
            Some(position) => self.stanzas[position] = stanza,
            None => self.stanzas.push(stanza),
        }
        Ok(())
    }
}
