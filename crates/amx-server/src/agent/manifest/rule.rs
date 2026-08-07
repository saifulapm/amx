//! The rule grammar as it is written, before it is compiled.
//!
//! These are the TOML shapes and nothing else — [`compile`](super::compile)
//! turns them into something that can be evaluated. Keeping the two apart is
//! what lets the caps below reject a manifest *before* a single regex is built,
//! which matters because an override manifest is a user's file and a user's
//! file is arbitrary input.
//!
//! # A rule
//!
//! ```toml
//! [[rule]]
//! name = "permission_dialog"
//! state = "blocked"
//! priority = 900
//! region = "bottom_non_empty_lines(12)"
//! contains = ["do you want to proceed?"]
//! any = [
//!   { line_regex = ['^\s*❯?\s*1\.\s*Yes'] },
//!   { line_regex = ['^\s*2\.\s*No'] },
//! ]
//! not = [{ contains = ["esc to interrupt"] }]
//! ```
//!
//! A rule is a gate plus a verdict. The gate is the AND of everything written
//! at its level — every `contains`, every `regex`, every `line_regex`, every
//! nested `all`, at least one of `any` when `any` is present, and none of
//! `not` — which is herdr's semantics, kept because a manifest author who has
//! written one should not have to relearn the other. The verdict is either a
//! `state` the rule asserts or `skip_state_update`, the assert-nothing third
//! outcome that freezes whatever the pane already had.
//!
//! # The caps
//!
//! Bounded because evaluation runs per damage batch on a busy pane and because
//! a manifest can come from disk. They are deliberately far above anything the
//! shipped manifests use — the bundled `claude` manifest is a dozen rules — so
//! that hitting one means something has gone wrong rather than something has
//! grown.

use serde::Deserialize;
use thiserror::Error;

use amx_core::agent::AgentState;

/// The most rules one manifest may carry.
pub const MAX_RULES: usize = 64;

/// The deepest a gate tree may nest.
pub const MAX_GATE_DEPTH: usize = 6;

/// The most matchers — `contains`, `regex` and `line_regex` patterns together —
/// one manifest may carry.
pub const MAX_MATCHERS: usize = 512;

/// The longest one matcher may be.
pub const MAX_MATCHER_CHARS: usize = 512;

/// A manifest file, as parsed.
///
/// `deny_unknown_fields` on purpose, and against the tolerance the wire applies
/// in both directions (04 §4): the wire's rule protects a *running session* from
/// a peer of another version, while a manifest is a file someone is editing, and
/// a silently ignored `contain = [...]` typo there is a rule that never fires
/// and never says why.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestFile {
    /// The agent id this manifest detects — the registry stanza's id.
    pub id: String,
    /// The manifest's own version, reported by `agent explain`.
    ///
    /// Free-form: it exists so a user reading an explanation can say which
    /// revision of the rules produced it, not for any comparison amx makes.
    #[serde(default)]
    pub version: Option<String>,
    /// The rules, in file order — which is also the tie-break order.
    #[serde(default, rename = "rule")]
    pub rules: Vec<RuleFile>,
}

/// One rule, as parsed.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleFile {
    /// The rule's name, unique within the manifest.
    ///
    /// It is what `agent explain` prints and what a bug report quotes, so it
    /// names the screen it recognises rather than the state it asserts.
    pub name: String,
    /// The state a match asserts. Absent exactly when
    /// [`skip_state_update`](Self::skip_state_update) is set.
    #[serde(default)]
    pub state: Option<RuleState>,
    /// Higher wins. Ties break by file order, earliest first.
    #[serde(default)]
    pub priority: i32,
    /// Which part of the screen to read, from the whitelist in
    /// [`region`](super::region).
    #[serde(default = "whole_recent")]
    pub region: String,
    /// Assert nothing and hold the pane's previous state.
    ///
    /// herdr's `skip_state_update`, and the reason it exists is a screen that
    /// is *about* the agent rather than *of* it: a transcript viewer, a model
    /// picker, a paging overlay. The last thing the agent's own UI said is
    /// still true underneath, so a rule that recognises the overlay says "I
    /// know what this is, and it is not a state" instead of guessing.
    #[serde(default)]
    pub skip_state_update: bool,
    /// The prompt box is genuinely on screen, so this idle needs no
    /// confirmation window.
    ///
    /// D-M2-6's flicker fix, kept as data: an idle asserted by a rule that
    /// matched the actual prompt box bypasses the confirmation hold, while one
    /// inferred from the *absence* of a working marker does not. Only meaningful
    /// on a rule asserting [`RuleState::Idle`].
    #[serde(default)]
    pub visible_idle: bool,
    /// The rule's own gate, flattened into the rule table.
    #[serde(flatten)]
    pub gate: GateFile,
}

/// One node of a gate tree, as parsed.
///
/// Every field is a conjunct: the node matches when all of its `contains`, all
/// of its `regex`, all of its `line_regex`, all of its `all` children match, at
/// least one of its `any` children matches (when `any` is non-empty), and none
/// of its `not` children match. A node with no fields at all matches
/// everything, which is why [`ManifestFile`] rejects one.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateFile {
    /// Case-insensitive substring matches, all of which must hold.
    #[serde(default)]
    pub contains: Vec<String>,
    /// Regular expressions matched against the region as one text.
    #[serde(default)]
    pub regex: Vec<String>,
    /// Regular expressions matched against each line, any one of which may
    /// satisfy the pattern.
    ///
    /// The per-line form is what lets a pattern anchor with `^`/`$` against a
    /// screen row rather than against the whole region.
    #[serde(default)]
    pub line_regex: Vec<String>,
    /// Nested gates, all of which must match.
    #[serde(default)]
    pub all: Vec<GateFile>,
    /// Nested gates, at least one of which must match.
    #[serde(default)]
    pub any: Vec<GateFile>,
    /// Nested gates, none of which may match.
    #[serde(default, rename = "not")]
    pub not: Vec<GateFile>,
}

impl GateFile {
    /// Whether this node constrains anything at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.contains.is_empty()
            && self.regex.is_empty()
            && self.line_regex.is_empty()
            && self.all.is_empty()
            && self.any.is_empty()
            && self.not.is_empty()
    }

    /// Whether this node asserts something positive.
    ///
    /// A gate of nothing but `not` matches every screen that fails to hold the
    /// thing it forbids — including a blank one — which is a rule that fires on
    /// a pane that has painted nothing yet.
    #[must_use]
    pub fn has_positive(&self) -> bool {
        !self.contains.is_empty()
            || !self.regex.is_empty()
            || !self.line_regex.is_empty()
            || !self.any.is_empty()
            || self.all.iter().any(Self::has_positive)
    }

    /// Every matcher pattern in this node and below it.
    pub fn matchers(&self) -> impl Iterator<Item = &String> {
        let here = self
            .contains
            .iter()
            .chain(&self.regex)
            .chain(&self.line_regex);
        let below = self
            .all
            .iter()
            .chain(&self.any)
            .chain(&self.not)
            .flat_map(|gate| Box::new(gate.matchers()) as Box<dyn Iterator<Item = &String>>);
        here.chain(below)
    }

    /// How deep this node's tree runs, counting itself as one.
    #[must_use]
    pub fn depth(&self) -> usize {
        1 + self
            .all
            .iter()
            .chain(&self.any)
            .chain(&self.not)
            .map(Self::depth)
            .max()
            .unwrap_or(0)
    }
}

/// The three states a screen rule may assert.
///
/// Tier 2 speaks about identified agents, so it has no `busy`/`quiet`: those
/// belong to tier 3's [`Activity`](amx_core::agent::Activity), and the split is
/// the same discipline in the other direction — a probe cannot spell `blocked`,
/// and a manifest cannot spell `busy`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleState {
    /// The agent is at its prompt.
    Idle,
    /// The agent is working on a turn.
    Working,
    /// The agent is waiting for the user to answer something.
    Blocked,
}

impl From<RuleState> for AgentState {
    fn from(state: RuleState) -> Self {
        match state {
            RuleState::Idle => Self::Idle,
            RuleState::Working => Self::Working,
            RuleState::Blocked => Self::Blocked,
        }
    }
}

/// The region a rule that names none reads.
fn whole_recent() -> String {
    "whole_recent".to_owned()
}

/// Why a manifest file is not a manifest.
///
/// Every variant names the rule it came from: a manifest is edited by hand, and
/// "rule `live_prompt_box`: …" is the difference between a fixable error and a
/// hunt.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum RuleError {
    /// The file is not TOML, or not this shape.
    #[error("manifest is not well-formed: {message}")]
    Toml {
        /// What the parser said.
        message: String,
    },
    /// The manifest declared no id.
    #[error("manifest declares no id")]
    NoId,
    /// Two rules share a name.
    #[error("rule {name:?} is declared twice; rule names are how explain reports them")]
    DuplicateRule {
        /// The repeated name.
        name: String,
    },
    /// The manifest carries more than [`MAX_RULES`] rules.
    #[error("manifest carries {found} rules, at most {MAX_RULES} are allowed")]
    TooManyRules {
        /// How many it carried.
        found: usize,
    },
    /// The manifest carries more than [`MAX_MATCHERS`] matchers.
    #[error("manifest carries {found} matchers, at most {MAX_MATCHERS} are allowed")]
    TooManyMatchers {
        /// How many it carried.
        found: usize,
    },
    /// One matcher is longer than [`MAX_MATCHER_CHARS`].
    #[error("rule {rule:?} has a matcher longer than {MAX_MATCHER_CHARS} characters")]
    MatcherTooLong {
        /// The rule it is in.
        rule: String,
    },
    /// A rule's gate tree nests deeper than [`MAX_GATE_DEPTH`].
    #[error("rule {rule:?} nests {found} deep, at most {MAX_GATE_DEPTH} is allowed")]
    GateTooDeep {
        /// The rule.
        rule: String,
        /// How deep it went.
        found: usize,
    },
    /// A rule matches everything, or matches on absence alone.
    #[error("rule {rule:?} asserts nothing positive; a gate of only `not` matches a blank screen")]
    EmptyGate {
        /// The rule.
        rule: String,
    },
    /// A rule neither asserts a state nor freezes one.
    #[error("rule {rule:?} names no state and does not set skip_state_update")]
    NoVerdict {
        /// The rule.
        rule: String,
    },
    /// A rule both asserts a state and freezes one.
    #[error("rule {rule:?} sets skip_state_update *and* a state; it can only do one")]
    TwoVerdicts {
        /// The rule.
        rule: String,
    },
    /// A rule's region is not in the whitelist.
    #[error("rule {rule:?}: {source}")]
    Region {
        /// The rule.
        rule: String,
        /// What was wrong with the spec.
        #[source]
        source: super::region::RegionError,
    },
    /// A rule's pattern is not a regular expression.
    #[error("rule {rule:?}: pattern {pattern:?} does not compile: {message}")]
    Regex {
        /// The rule.
        rule: String,
        /// The pattern.
        pattern: String,
        /// What the regex engine said.
        message: String,
    },
}
