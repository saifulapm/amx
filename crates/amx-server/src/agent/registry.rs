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
//! # Task ownership
//!
//! V02 froze the stanza schema, because V04's fusion, V08's hub and V10's
//! installers all read it and are built in parallel. **V03** fills the parse,
//! the merge and the lookup, adds `assets/agents.toml`, and writes the
//! conformance test.

use amx_core::agent::{AgentKind, CoverageClass, RefKind};
use serde::{Deserialize, Serialize};

/// One agent, as `agents.toml` declares it.
///
/// Everything amx knows about an agent is a field here. If a task finds itself
/// writing an agent's name anywhere but its stanza, that is W6 growing back —
/// herdr defined its agents across roughly ten hand-synced match sites, and
/// `docs/02-herdr-critique.md` W6 is the account of what that cost.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct AgentStanza {
    /// The id, unique across the merged registry.
    pub id: AgentKind,
    /// Other names that resolve to it, unique across the merged registry too —
    /// an alias colliding with another stanza's id is a conformance failure.
    #[serde(default)]
    pub aliases: Vec<AgentKind>,
    /// What a human sees: the status line, the picker, `agent explain`.
    pub label: String,
    /// Executable basenames the identity tier accepts as this agent.
    ///
    /// Basenames, matched after V07's wrapper unwrapping walks through
    /// `node`/`bun`/`python`/`sh -c` and path tokens. Never matched against an
    /// eval-flag argument: `python -c "codex"` is not codex, and V07 carries
    /// herdr's negative test for it, re-derived.
    #[serde(default)]
    pub executables: Vec<String>,
    /// How much of this agent's lifecycle its hooks actually report.
    ///
    /// **Measured, never assumed** (05, M2: "set from measurements, not hope").
    /// Copied from `docs/notes/hook-coverage.md`, which V03's conformance test
    /// reads back as its fixture. As of V01 both shipped stanzas are
    /// [`CoverageClass::Edges`].
    pub coverage: CoverageClass,
    /// The argv `agent start` spawns, before the caller's extra arguments.
    pub start: Vec<String>,
    /// The argv template `agent resume` substitutes into.
    ///
    /// Exactly one element is the literal `{ref}`, and the substitution puts
    /// the ref in as *one* element (D-M2-7: "argv is data"; nothing is ever
    /// interpolated into a shell string). The conformance test rejects a
    /// template with no slot, two slots, or shell metacharacters.
    #[serde(default)]
    pub resume: Vec<String>,
    /// Which kind of ref this agent's `resume` accepts.
    ///
    /// A `path` ref for an `id` agent is refused even when it is well-formed —
    /// D-M2-7's "`path` only for agents whose stanza says so".
    #[serde(default = "default_ref_kind")]
    pub ref_kind: RefKind,
    /// The lifecycle events `amx integration install` subscribes for this
    /// agent, spelled as the agent spells them.
    ///
    /// Fewer than the agent offers, on purpose: each subscription costs a
    /// process spawn per occurrence, so V01 §5 lists exactly which events earn
    /// their keep and which do not (`PostToolBatch` fires for calls that never
    /// ran; `PermissionDenied` never fired once).
    #[serde(default)]
    pub hook_events: Vec<String>,
    /// The manifest file this agent's tier-2 rules live in, under the bundled
    /// manifest directory.
    ///
    /// Every `edges` and `identity` stanza needs one: those classes cannot
    /// leave a held state without the screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    /// How long to wait after a spawn before believing this agent's screen.
    ///
    /// A field rather than one global constant because V01 §6 measured the two
    /// shipped agents differing enough to matter: Claude Code is ready about
    /// 1.1 s after launch, Codex took 2.8–4.6 s to finish its startup gates and
    /// emits no hook at all until the first prompt. The recommendation is
    /// literally "raise to 5 s for Codex, keep 3 s for Claude", which is not
    /// something one number can say.
    ///
    /// Absent falls back to
    /// [`IDENTITY_GRACE`](super::fusion::IDENTITY_GRACE).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_grace_ms: Option<u64>,
}

/// `id`, the kind both shipped agents use.
///
/// V01 §3 M8 and §4: Claude Code's resume ref is its `session_id` (a v4-shaped
/// UUID) and Codex's is a UUIDv7, and in both cases `<agent> --resume <id>`
/// round-trips the same conversation.
const fn default_ref_kind() -> RefKind {
    RefKind::Id
}

/// The merged registry: the embedded stanzas, plus the user's overrides.
///
/// **V03 fills this.** The merge rule, from D-M2-2 and V03's prompt: an
/// override stanza adds or *replaces a whole agent*, never field-merges. A
/// partial stanza is rejected with the builtin kept, which is M1's per-section
/// lenient config rule applied one level down — a broken override costs the
/// agent it names and nothing else.
///
/// The override path is also the test seam: V17's rig plants a stanza for a
/// scripted fake agent instead of patching the binary.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Registry {
    stanzas: Vec<AgentStanza>,
}

impl Registry {
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
    /// **V03 fills this.** Ids are checked before aliases, so an alias that
    /// collides with another stanza's id cannot shadow it — though the
    /// conformance test refuses to let that registry load in the first place.
    #[must_use]
    pub fn resolve(&self, name: &AgentKind) -> Option<&AgentStanza> {
        let _ = name;
        todo!("V03: id-then-alias lookup over the merged stanzas")
    }
}
