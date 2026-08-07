//! Tier 2: the screen manifest engine.
//!
//! 04 §5 keeps herdr's TOML rule engine wholesale — "priority, region, gate
//! trees, `skip_state_update` freezes, bottom-buffer snapshot, `agent explain`
//! debuggability … running continuously as the fusion partner for `edges`
//! agents and the sole source for `identity` agents". amx writes its own
//! engine against that grammar; herdr is Apache-2.0 and amx is an independent
//! implementation, so the mechanism is studied and never the lines.
//!
//! What amx changes, per D-M2-3:
//!
//! - **The scrolled-viewport hazard is gone by construction.** herdr anchors
//!   its detection buffer to the scrollback bottom and regression-tests that a
//!   scrolled viewport never moves it, because its server owns the scrolled
//!   view. amx's published snapshot *is* the live visible grid — scrollback and
//!   scroll position are client-side (04 §3) — so there is no anchor to keep.
//! - **Evaluation is pushed, never polled.** The trigger is the `PaneDamage`
//!   event stream plus per-pane coalescing in `AgentHub`, not herdr's permanent
//!   300–500 ms scan loop (03 §5: push, never poll).
//! - **The region vocabulary starts minimal and whitelisted**: `whole_recent`,
//!   `bottom_lines(N)`, `bottom_non_empty_lines(N)`, `title`. A whitelist so
//!   rules cannot drift into whole-scrollback greps; minimal because herdr's
//!   structural prompt-marker regions should only arrive if a shipped manifest
//!   needs one. Bias state rules to bottom-anchored regions — herdr's changelog
//!   documents `whole_recent` matching stale "esc to interrupt" scrollback as a
//!   real production bug.
//!
//! Every regex compiles **once at load**, stored beside its parsed rule. This
//! runs per damage batch; a pattern recompiled there would be paid for on every
//! frame of a busy pane.
//!
//! # Using it
//!
//! The engine is pure: text in, verdict out, no I/O and no clock, which is what
//! lets it be tested against fixture screens rather than against a pty.
//!
//! ```
//! use amx_core::agent::AgentState;
//! use amx_server::agent::manifest::{Manifest, ScreenBuf, Verdict};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let manifest = Manifest::load_bundled("claude.toml")?;
//!
//! // One buffer per pane, refilled from the snapshot's rows per damage batch;
//! // it reuses its allocation, so the detection path does not allocate.
//! let mut buf = ScreenBuf::new();
//! buf.fill(["✻ Cogitating… (esc to interrupt · ctrl+t to hide todos)"]);
//!
//! match manifest.evaluate(buf.screen("")) {
//!     Verdict::Assert { rule, state } => {
//!         assert_eq!(state, AgentState::Working);
//!         let _ = rule.name();
//!     }
//!     Verdict::Hold { .. } | Verdict::NoMatch => unreachable!(),
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Three outcomes and not two: [`Verdict::Hold`] is `skip_state_update`, the
//! assert-nothing case a transcript viewer or a model picker produces. A pane
//! showing one of those is still whatever it was underneath, and a machine that
//! only knew "matched" and "did not match" would have to guess which.
//!
//! # Task ownership
//!
//! **V06** fills this whole subtree — `rule.rs`, `region.rs`, `compile.rs`,
//! `explain.rs` beside this file, and the `claude` and `codex` manifests under
//! `assets/manifests/`. Its screen fixtures are recorded from the real UIs of
//! the versions V01 measured.
//!
//! Two consumers are waiting on it, and neither is here:
//!
//! - **V08**'s `AgentHub` owns the loop: one [`ScreenBuf`] per tracked pane,
//!   refilled from the `SnapshotFeed` frame per coalesced damage batch, fed to
//!   [`Manifest::evaluate`], and the [`Verdict`] handed to V04's tracker.
//! - **V03**'s registry names a manifest per stanza and its conformance test
//!   asserts every one of them is present and compiles — [`BUNDLED`] and
//!   [`Manifest::load_bundled`] are what it walks.
//!
//! `agent.explain`'s dispatch arm stays a seam for now. It needs a pane's
//! tracker and its frames, which live behind `AgentCommand::Explain` in a hub
//! that does not exist until V08; what V06 lands is the whole explanation
//! ([`Manifest::explain`]), so filling the arm is a mailbox round trip and a
//! call to [`Explanation::into_reply`].

pub mod compile;
pub mod explain;
pub mod region;
pub mod rule;

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use amx_core::agent::AgentState;

use compile::CompiledGate;
use rule::{MAX_MATCHERS, MAX_RULES, ManifestFile};

pub use explain::Explanation;
pub use region::{Region, ScreenBuf, ScreenText};
pub use rule::RuleError as ManifestError;

/// The manifests compiled into the binary, by file name.
///
/// The registry stanza's `manifest` field names one of these (V03), and its
/// conformance test walks this table so a stanza cannot name a manifest that
/// does not exist. Adding an agent is a stanza plus a file plus a row here —
/// three edits in three files, none of which is a second list of agent names.
pub const BUNDLED: &[(&str, &str)] = &[
    (
        "claude.toml",
        include_str!("../../../assets/manifests/claude.toml"),
    ),
    (
        "codex.toml",
        include_str!("../../../assets/manifests/codex.toml"),
    ),
];

/// Where a manifest came from.
///
/// Reported by `agent explain` because "which rules am I even running" is the
/// first question a wrong detection raises, and an override that shadows a
/// bundled manifest is the usual answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ManifestSource {
    /// Compiled into the binary, under `assets/manifests/`.
    Bundled {
        /// The file name, as [`BUNDLED`] keys it.
        name: String,
    },
    /// Read from the user's override directory.
    ///
    /// Re-scanned on the config watcher's reload event (R-M2-13); a remote
    /// catalog is M4's, not M2's.
    Override {
        /// The file it was read from.
        path: PathBuf,
    },
}

impl fmt::Display for ManifestSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bundled { name } => write!(f, "bundled:{name}"),
            Self::Override { path } => write!(f, "override:{}", path.display()),
        }
    }
}

/// One compiled rule: a gate, a region, and what a match means.
#[derive(Debug)]
pub struct Rule {
    name: String,
    priority: i32,
    region: Region,
    asserts: Option<AgentState>,
    visible_idle: bool,
    gate: CompiledGate,
}

/// Two rules are the same rule when they read the same and say the same.
///
/// The compiled gate is deliberately outside the comparison: a compiled regex
/// has no equality worth the name, and what a caller means by "the same rule" is
/// the one the manifest declares, not the automaton it became.
impl PartialEq for Rule {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.priority == other.priority
            && self.region == other.region
            && self.asserts == other.asserts
            && self.visible_idle == other.visible_idle
    }
}

impl Eq for Rule {}

impl Rule {
    /// The rule's name, as the manifest spells it.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Its priority. Higher wins; ties break by file order, earliest first.
    #[must_use]
    pub const fn priority(&self) -> i32 {
        self.priority
    }

    /// The region it reads.
    #[must_use]
    pub const fn region(&self) -> Region {
        self.region
    }

    /// The state a match asserts, or `None` for a `skip_state_update` rule.
    #[must_use]
    pub const fn asserts(&self) -> Option<AgentState> {
        self.asserts
    }

    /// Whether an idle this rule asserts bypasses fusion's confirmation hold.
    ///
    /// D-M2-6: an idle read off the *visible prompt box* is a positive sighting
    /// and applies at once, while an idle inferred from the absence of a
    /// working marker waits out the confirmation window. herdr's flicker fix,
    /// kept as data rather than as a special case in the machine.
    #[must_use]
    pub const fn visible_idle(&self) -> bool {
        self.visible_idle
    }
}

/// What one evaluation concluded.
///
/// Borrowed from the manifest, so an evaluation costs no allocation at all —
/// the caller reads what it needs off [`Rule`] and drops the verdict.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict<'m> {
    /// A rule matched and asserts a state.
    Assert {
        /// The winning rule.
        rule: &'m Rule,
        /// What it asserts.
        state: AgentState,
    },
    /// A `skip_state_update` rule matched: assert nothing, hold what the pane
    /// already had, and name the rule that froze it.
    Hold {
        /// The winning rule.
        rule: &'m Rule,
    },
    /// Nothing matched. Tier 2 has no opinion about this screen, which is not
    /// the same as an opinion that the pane is idle.
    NoMatch,
}

/// A compiled screen manifest for one agent.
#[derive(Debug)]
pub struct Manifest {
    id: String,
    version: Option<String>,
    source: ManifestSource,
    rules: Vec<Rule>,
}

impl Manifest {
    /// The bundled manifest source for `name`, with or without its extension.
    ///
    /// A registry stanza names a file (`claude.toml`); a person debugging names
    /// an agent (`claude`). Both resolve, so neither has to remember which this
    /// is.
    #[must_use]
    pub fn bundled(name: &str) -> Option<&'static str> {
        let wanted = name.strip_suffix(".toml").unwrap_or(name);
        BUNDLED
            .iter()
            .find(|(file, _)| file.strip_suffix(".toml") == Some(wanted))
            .map(|(_, text)| *text)
    }

    /// Parse and compile the bundled manifest `name`.
    ///
    /// # Errors
    ///
    /// [`ManifestError`] if there is no such bundled manifest, or if it does
    /// not compile — which a shipped manifest never does, because V03's
    /// conformance test calls exactly this for every stanza in the registry.
    pub fn load_bundled(name: &str) -> Result<Self, ManifestError> {
        let text = Self::bundled(name).ok_or_else(|| ManifestError::Toml {
            message: format!("no bundled manifest named {name:?}"),
        })?;
        let file = name.strip_suffix(".toml").unwrap_or(name);
        Self::parse(
            text,
            ManifestSource::Bundled {
                name: format!("{file}.toml"),
            },
        )
    }

    /// Parse and compile `text`, recording where it came from.
    ///
    /// The whole file stands or falls together, which is the opposite of the
    /// config file's per-section leniency (D-M1-8) and deliberately so: a
    /// half-applied manifest detects half a UI, and a pane whose status is
    /// wrong in a way nobody can see is worse than a pane running on the
    /// bundled rules with a diagnostic beside it.
    ///
    /// # Errors
    ///
    /// [`ManifestError`], naming the offending rule.
    pub fn parse(text: &str, source: ManifestSource) -> Result<Self, ManifestError> {
        let file: ManifestFile = toml::from_str(text).map_err(|err| ManifestError::Toml {
            message: err.to_string(),
        })?;
        if file.id.trim().is_empty() {
            return Err(ManifestError::NoId);
        }
        if file.rules.len() > MAX_RULES {
            return Err(ManifestError::TooManyRules {
                found: file.rules.len(),
            });
        }
        let matchers = file
            .rules
            .iter()
            .map(|rule| rule.gate.matchers().count())
            .sum::<usize>();
        if matchers > MAX_MATCHERS {
            return Err(ManifestError::TooManyMatchers { found: matchers });
        }

        let mut seen = HashSet::new();
        let mut rules = Vec::with_capacity(file.rules.len());
        for rule in &file.rules {
            if !seen.insert(rule.name.as_str()) {
                return Err(ManifestError::DuplicateRule {
                    name: rule.name.clone(),
                });
            }
            let depth = rule.gate.depth();
            if depth > rule::MAX_GATE_DEPTH {
                return Err(ManifestError::GateTooDeep {
                    rule: rule.name.clone(),
                    found: depth,
                });
            }
            if rule.gate.is_empty() || !rule.gate.has_positive() {
                return Err(ManifestError::EmptyGate {
                    rule: rule.name.clone(),
                });
            }
            let asserts = match (rule.state, rule.skip_state_update) {
                (Some(state), false) => Some(state.into()),
                (None, true) => None,
                (None, false) => {
                    return Err(ManifestError::NoVerdict {
                        rule: rule.name.clone(),
                    });
                }
                (Some(_), true) => {
                    return Err(ManifestError::TwoVerdicts {
                        rule: rule.name.clone(),
                    });
                }
            };
            rules.push(Rule {
                region: Region::parse(&rule.region).map_err(|source| ManifestError::Region {
                    rule: rule.name.clone(),
                    source,
                })?,
                gate: CompiledGate::build(&rule.gate, &rule.name)?,
                name: rule.name.clone(),
                priority: rule.priority,
                asserts,
                visible_idle: rule.visible_idle,
            });
        }

        Ok(Self {
            id: file.id,
            version: file.version,
            source,
            rules,
        })
    }

    /// Parse and compile an override manifest read from `path`.
    ///
    /// The read itself is the caller's: this module does no I/O, so the hub can
    /// re-scan the override directory on a config reload without the engine
    /// knowing what a directory is.
    ///
    /// # Errors
    ///
    /// [`ManifestError`], naming the offending rule.
    pub fn parse_override(text: &str, path: impl AsRef<Path>) -> Result<Self, ManifestError> {
        Self::parse(
            text,
            ManifestSource::Override {
                path: path.as_ref().to_path_buf(),
            },
        )
    }

    /// The agent id this manifest detects.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The manifest's declared version, if it declares one.
    #[must_use]
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Where it came from.
    #[must_use]
    pub const fn source(&self) -> &ManifestSource {
        &self.source
    }

    /// Its rules, in file order.
    #[must_use]
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Read `screen` and return what the highest-priority matching rule says.
    ///
    /// Priority arbitration, with file order as the tie-break: the *first* rule
    /// written at a given priority wins, so a manifest is read top-down the way
    /// it is written. Every rule is evaluated — there is no early exit — because
    /// a later rule may outrank an earlier match and because `agent explain`
    /// reports all of them from the same walk.
    #[must_use]
    pub fn evaluate<'m>(&'m self, screen: ScreenText<'_>) -> Verdict<'m> {
        let mut winner: Option<&Rule> = None;
        for rule in &self.rules {
            if !rule.gate.matches(rule.region.slice(&screen)) {
                continue;
            }
            match winner {
                Some(previous) if previous.priority >= rule.priority => {}
                _ => winner = Some(rule),
            }
        }
        match winner {
            Some(rule) => match rule.asserts {
                Some(state) => Verdict::Assert { rule, state },
                None => Verdict::Hold { rule },
            },
            None => Verdict::NoMatch,
        }
    }
}
