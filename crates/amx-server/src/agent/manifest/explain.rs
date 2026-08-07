//! `agent explain`: every rule's verdict, and the evidence for it.
//!
//! 04 §5 keeps herdr's `agent explain` in the list of things worth keeping
//! wholesale, and D-M2-3 gives the one-line reason: a detection you cannot
//! interrogate is a detection you cannot fix. So this reports every rule in
//! file order — not only the winner — with what in the region made it match or
//! made it fail, plus the text the rules actually read.
//!
//! The rules that did *not* match are the half that answers the question a user
//! actually has. "Why does my pane say working" is answered by the winner;
//! "why does my pane not say blocked" is answered by the `blocked` rule
//! reporting that it wanted `"do you want to proceed?"` and the region did not
//! have it.
//!
//! This is the cold path. It allocates freely, runs once per `agent explain`,
//! and shares nothing with [`Manifest::evaluate`](super::Manifest::evaluate)
//! except the rules themselves — but it *does* walk them the same way, so an
//! explanation can never disagree with the detection it explains.

use amx_core::PaneId;
use amx_core::agent::{AgentKind, AgentSnapshot};
use amx_proto::control::agent::{ExplainReply, RuleVerdict};

use super::{Manifest, Region, Rule, ScreenText};

/// How many lines of region text an explanation carries.
///
/// Enough to hold a permission dialog and the prompt box under it; short
/// enough that an explanation stays a thing a person reads in a terminal.
const PREVIEW_LINES: usize = 24;

/// How many characters of one preview line survive.
const PREVIEW_CHARS: usize = 200;

/// What one evaluation looked at, rule by rule.
///
/// Everything `agent.explain` reports about tier 2. The pane's *fused* status —
/// what the machine concluded once the hook tier had its say — is the hub's to
/// add, which is why [`into_reply`](Self::into_reply) takes it rather than this
/// module inventing one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Explanation {
    /// Where the manifest came from: `bundled:claude.toml`, or the override
    /// path that shadows it.
    pub manifest: String,
    /// The manifest's declared version.
    pub manifest_version: Option<String>,
    /// The rule that won, if one did.
    pub matched: Option<String>,
    /// The text the rules were evaluated against, as they saw it.
    pub region_preview: Vec<String>,
    /// Every rule, in file order.
    pub rules: Vec<RuleVerdict>,
}

impl Explanation {
    /// Turn this into the wire reply, with what only the hub knows.
    ///
    /// `kind` is the agent identity tier 3 settled on and `agent` its fused
    /// status; both are `None` for a pane whose foreground program was never
    /// identified, which is the honest answer for a plain shell.
    #[must_use]
    pub fn into_reply(
        self,
        pane: PaneId,
        kind: Option<AgentKind>,
        agent: Option<AgentSnapshot>,
    ) -> ExplainReply {
        ExplainReply {
            pane,
            kind,
            manifest: Some(self.manifest),
            manifest_version: self.manifest_version,
            matched: self.matched,
            region_preview: self.region_preview,
            rules: self.rules,
            agent,
        }
    }
}

impl Manifest {
    /// Evaluate `screen` and report every rule's verdict with its evidence.
    ///
    /// The winner is picked by the same arbitration
    /// [`evaluate`](Manifest::evaluate) uses — highest priority, ties to the
    /// earliest rule in file order — so `agent explain` and the status line can
    /// never name different rules for one screen.
    #[must_use]
    pub fn explain(&self, screen: ScreenText<'_>) -> Explanation {
        let mut winner: Option<&Rule> = None;
        let mut rules = Vec::with_capacity(self.rules().len());

        for rule in self.rules() {
            let region = rule.region().slice(&screen);
            let matched = rule.gate.matches(region);
            rules.push(RuleVerdict {
                rule: rule.name().to_owned(),
                priority: rule.priority(),
                region: rule.region().to_string(),
                matched,
                asserts: rule.asserts(),
                evidence: Some(rule.gate.evidence(region)),
            });
            if !matched {
                continue;
            }
            match winner {
                Some(previous) if previous.priority() >= rule.priority() => {}
                _ => winner = Some(rule),
            }
        }

        // The winner's own region, because that is the text that decided the
        // pane's status. With no winner there is nothing to be specific about,
        // so the preview falls back to the whole visible grid — which is what a
        // user staring at a pane that reports nothing wants to compare against.
        let region = winner.map_or(Region::WholeRecent, Rule::region);
        Explanation {
            manifest: self.source().to_string(),
            manifest_version: self.version().map(ToOwned::to_owned),
            matched: winner.map(|rule| rule.name().to_owned()),
            region_preview: preview(region.slice(&screen)),
            rules,
        }
    }
}

/// The last [`PREVIEW_LINES`] lines of `text`, each bounded.
///
/// The *last*, because every state rule worth explaining is bottom-anchored: a
/// preview that cut the prompt box off to show the top of a scrolled turn would
/// show the one part of the region no rule was reading.
fn preview(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(PREVIEW_LINES)..]
        .iter()
        .map(|line| {
            let mut bounded: String = line.chars().take(PREVIEW_CHARS).collect();
            if line.chars().count() > PREVIEW_CHARS {
                bounded.push('…');
            }
            bounded
        })
        .collect()
}
