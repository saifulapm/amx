//! What one `[[agent]]` stanza declares, and every rule it must not break.
//!
//! Split from [`super`] to keep both halves inside the module budget, along the
//! seam D-M2-2 already draws: a *stanza* is what a document says about one
//! agent, and the [`Registry`](super::Registry) is what a merged set of them
//! answers.
//!
//! [`AgentStanza::check`] is the half of D-M2-2's conformance list a stanza can
//! be held to on its own — it runs at every admission rather than only in the
//! test, so a user's override meets the standard the shipped stanzas meet. The
//! other half needs context this module does not have: name uniqueness is the
//! registry's, and whether a named manifest exists on disk is the conformance
//! test's, since nothing here does I/O.

use std::path::Path;

use amx_core::agent::{AgentKind, CoverageClass, RefKind};
use amx_proto::control::agent::HookEvent;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The single element a `resume` template substitutes a ref into.
pub const REF_SLOT: &str = "{ref}";

/// Characters a shell would read as syntax, and argv is data (D-M2-7).
///
/// A template holding one of these is a template somebody meant to hand to a
/// shell, and amx never does: `plan()` substitutes into an element and returns
/// a `Vec<String>`. Rejecting them at load is what keeps that promise checkable
/// rather than merely intended — including for a stanza that arrived from a
/// user's override file.
const SHELL_METACHARACTERS: &[char] = &[
    '|', '&', ';', '<', '>', '(', ')', '$', '`', '\\', '"', '\'', '*', '?', '[', ']', '{', '}',
    '~', '!', '#', '\n', '\r',
];

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
    /// [`IDENTITY_GRACE`](crate::agent::fusion::IDENTITY_GRACE).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_grace_ms: Option<u64>,
}

impl AgentStanza {
    /// Every name this stanza answers to: its id, then its aliases.
    ///
    /// One iterator rather than two call sites that each remember to look at
    /// `aliases` — the uniqueness rule and [`Registry::resolve`](super::Registry::resolve) read the same
    /// list, so an alias can never be unique-checked and then unreachable.
    pub fn names(&self) -> impl Iterator<Item = &AgentKind> {
        std::iter::once(&self.id).chain(self.aliases.iter())
    }

    /// Everything about this stanza that can be judged without the others.
    ///
    /// D-M2-2's conformance list, minus the two questions a single stanza
    /// cannot answer: name uniqueness (the [`Registry`](super::Registry)'s, at admission) and
    /// whether the named manifest exists on disk (the conformance test's, since
    /// this module does no I/O).
    ///
    /// It runs at *every* admission, not only in the test, so a user's override
    /// stanza is held to exactly the standard the shipped ones are. A stanza
    /// that lies is refused and the builtin it would have replaced is kept.
    ///
    /// # Errors
    ///
    /// The first [`StanzaProblem`] found, named precisely enough to fix.
    pub fn check(&self) -> Result<(), StanzaProblem> {
        if self.label.trim().is_empty() {
            return Err(StanzaProblem::EmptyLabel);
        }
        // Tier 3 is the only thing that knows what a pane is running before the
        // first hook — and for Codex, V01 §4 measured that "before" lasting
        // until the user's first prompt. A stanza with no executables is an
        // agent amx could never identify in a pane the user typed it into.
        if self.executables.is_empty() {
            return Err(StanzaProblem::NoExecutables);
        }
        check_argv(&self.start, StanzaField::Start)?;
        if !self.resume.is_empty() {
            check_argv(&self.resume, StanzaField::Resume)?;
            check_ref_slot(&self.resume)?;
        }
        self.check_coverage()
    }

    /// The tiers this stanza's [`coverage`](Self::coverage) class needs.
    ///
    /// "Every `coverage` value paired with the tiers it needs" (D-M2-2), read
    /// off the class rather than off a list of agent ids:
    ///
    /// - [`Edges`](CoverageClass::Edges) and
    ///   [`Identity`](CoverageClass::Identity) need a manifest, because neither
    ///   can leave a held state without the screen. `full` does not (its hooks
    ///   own state outright) and `none` does not (it has no state to leave —
    ///   tier 3 alone).
    /// - Every class whose
    ///   [`hook_entries_apply`](CoverageClass::hook_entries_apply) needs an
    ///   event list, because that list *is* what `amx integration install`
    ///   writes: a class that promises hook edges and subscribes nothing has no
    ///   integration asset to install, which is D-M2-2's "every `edges`/`full`
    ///   stanza naming an integration asset".
    fn check_coverage(&self) -> Result<(), StanzaProblem> {
        let class = self.coverage;
        if matches!(class, CoverageClass::Edges | CoverageClass::Identity) {
            let Some(manifest) = &self.manifest else {
                return Err(StanzaProblem::MissingManifest { class });
            };
            check_manifest_name(manifest)?;
        }
        if class.hook_entries_apply() {
            if self.hook_events.is_empty() {
                return Err(StanzaProblem::NoHookEvents { class });
            }
            // A subscription the fusion machine has no arm for costs a process
            // spawn per occurrence and asserts nothing. `HookEvent` carries an
            // `Other` fallback for events amx meets on the wire; a *stanza* is
            // amx's own text, so it may not name one.
            if let Some(unknown) = self
                .hook_events
                .iter()
                .find(|name| matches!(HookEvent::from((*name).clone()), HookEvent::Other(_)))
            {
                return Err(StanzaProblem::UnknownHookEvent {
                    event: unknown.clone(),
                });
            }
        }
        Ok(())
    }
}

/// `id`, the kind both shipped agents use.
///
/// V01 §3 M8 and §4: Claude Code's resume ref is its `session_id` (a v4-shaped
/// UUID) and Codex's is a UUIDv7, and in both cases `<agent> --resume <id>`
/// round-trips the same conversation.
const fn default_ref_kind() -> RefKind {
    RefKind::Id
}

/// Which argv a [`StanzaProblem`] is about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StanzaField {
    /// [`AgentStanza::start`].
    Start,
    /// [`AgentStanza::resume`].
    Resume,
}

impl std::fmt::Display for StanzaField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Start => "start",
            Self::Resume => "resume",
        })
    }
}

/// Both argvs' shared rules: a program amx can spawn, and data in every slot.
fn check_argv(argv: &[String], field: StanzaField) -> Result<(), StanzaProblem> {
    let Some(program) = argv.first() else {
        return Err(StanzaProblem::EmptyArgv { field });
    };
    // Absolute, or a bare program name resolved against `PATH`. Anything in
    // between — `./claude`, `bin/claude` — resolves against whatever directory
    // the server happened to start in, which is not a thing a registry may
    // depend on.
    let bare =
        !program.is_empty() && !program.contains('/') && !program.chars().any(char::is_whitespace);
    if !bare && !Path::new(program).is_absolute() {
        return Err(StanzaProblem::BadProgram {
            field,
            program: program.clone(),
        });
    }
    for argument in argv {
        if argument == REF_SLOT {
            continue;
        }
        if let Some(found) = argument
            .chars()
            .find(|c| c.is_control() || SHELL_METACHARACTERS.contains(c))
        {
            return Err(StanzaProblem::ShellMetacharacter {
                field,
                argument: argument.clone(),
                found,
            });
        }
    }
    Ok(())
}

/// The resume template's one rule: exactly one slot, and it is a whole element.
fn check_ref_slot(resume: &[String]) -> Result<(), StanzaProblem> {
    let slots = resume
        .iter()
        .filter(|argument| *argument == REF_SLOT)
        .count();
    if slots != 1 {
        return Err(StanzaProblem::RefSlots { found: slots });
    }
    // `--resume={ref}` would substitute into a *fragment*, which is one quoting
    // question away from being a shell string again. The ref is one element.
    if let Some(argument) = resume
        .iter()
        .find(|argument| *argument != REF_SLOT && argument.contains(REF_SLOT))
    {
        return Err(StanzaProblem::EmbeddedRefSlot {
            argument: argument.clone(),
        });
    }
    Ok(())
}

/// A manifest name is a bare file name under the bundled manifest directory.
///
/// Never a path: a stanza that could name `../../etc/anything` would make the
/// override file a file-read primitive.
fn check_manifest_name(name: &str) -> Result<(), StanzaProblem> {
    let bare = !name.is_empty()
        && !name.contains('/')
        && name != ".."
        && name.ends_with(".toml")
        && !name.chars().any(|c| c.is_control() || c.is_whitespace());
    if bare {
        Ok(())
    } else {
        Err(StanzaProblem::BadManifestName {
            name: name.to_owned(),
        })
    }
}

/// Why a stanza may not join the registry.
///
/// Typed, and specific to the rule broken, because these reach a user: an
/// override stanza that is refused costs them the agent it names, and "invalid
/// stanza" would not tell them which line to fix.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum StanzaProblem {
    /// A name is already another stanza's id or alias.
    #[error("the name `{name}` already belongs to `{owner}`")]
    DuplicateName {
        /// The colliding id or alias.
        name: AgentKind,
        /// The stanza that had it first.
        owner: AgentKind,
    },
    /// The label was empty or blank.
    #[error("the label is what a human sees; it cannot be blank")]
    EmptyLabel,
    /// No executable basenames, so tier 3 could never identify this agent.
    #[error("no executables, so nothing could ever identify this agent in a pane")]
    NoExecutables,
    /// An argv had no program.
    #[error("`{field}` names no program")]
    EmptyArgv {
        /// Which argv.
        field: StanzaField,
    },
    /// The program was neither absolute nor a bare name.
    #[error("`{field}`'s program {program:?} is neither absolute nor a bare name")]
    BadProgram {
        /// Which argv.
        field: StanzaField,
        /// What it said.
        program: String,
    },
    /// An argument held a character a shell would read as syntax.
    #[error(
        "`{field}`'s argument {argument:?} holds {found:?}; argv is data, never a shell string"
    )]
    ShellMetacharacter {
        /// Which argv.
        field: StanzaField,
        /// The offending argument.
        argument: String,
        /// The first offending character.
        found: char,
    },
    /// The resume template had no `{ref}` slot, or more than one.
    #[error("a resume template holds exactly one `{slot}` element, this one holds {found}", slot = REF_SLOT)]
    RefSlots {
        /// How many it held.
        found: usize,
    },
    /// The `{ref}` slot was part of a larger argument.
    #[error("{argument:?} embeds `{slot}`; the slot is a whole argv element", slot = REF_SLOT)]
    EmbeddedRefSlot {
        /// The argument it was buried in.
        argument: String,
    },
    /// A class that cannot leave a held state without the screen named no
    /// manifest.
    #[error("class `{class}` leaves held states through the screen, so it needs a manifest")]
    MissingManifest {
        /// The class that needs one.
        class: CoverageClass,
    },
    /// The manifest name was a path, or not a `.toml` file name.
    #[error("the manifest name {name:?} is not a bare `.toml` file name")]
    BadManifestName {
        /// What it said.
        name: String,
    },
    /// A class whose hook entries apply subscribed nothing.
    #[error("class `{class}` promises hook edges but subscribes no events")]
    NoHookEvents {
        /// The class that promised them.
        class: CoverageClass,
    },
    /// A subscribed event no build of amx has an arm for.
    #[error(
        "no build of amx handles the hook event {event:?}, so subscribing it only costs a process spawn"
    )]
    UnknownHookEvent {
        /// What it said.
        event: String,
    },
}
