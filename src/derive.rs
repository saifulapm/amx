//! What an agent is doing, worked out at the moment somebody asks.
//!
//! Nothing amx runs keeps this up to date, because nothing amx runs stays
//! resident. A reader weighs what it has, in this order:
//!
//! 1. **The record ended it.** An exit code was written, or `stop` was. That
//!    is not a guess and nothing overrules it.
//! 2. **The pane is gone.** No pane, no agent: it is stopped, whatever the
//!    last hook said.
//! 3. **The hooks are fresh.** Inside [`FRESH`] seconds the vendor's own
//!    events are the best account there is.
//! 4. **The screen, against the rules.** Older than that, the pane is captured
//!    and matched. A rule that claims it decides.
//! 5. **Neither.** The screen is claimed by nothing, so the answer is
//!    `unknown` — with how long it has been since anything was heard, because
//!    "I can't tell" is only useful with that beside it.

use anyhow::Result;
use serde::Serialize;
use std::path::Path;

use crate::rules::{Claim, Ruleset};
use crate::store::{Agent, Meta, Phase, Source, State};
use crate::tmux::Server;

/// How long the vendor's own events are taken at their word.
///
/// Long enough to cover the quiet inside a turn between two tool calls, short
/// enough that an agent somebody interrupted stops reading as working while
/// they watch it.
pub const FRESH: u64 = 8;

/// Which signal the answer came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Evidence {
    /// The record says how it ended.
    Record,
    /// There is no pane any more.
    Gone,
    /// The vendor's own events, recently enough to trust.
    Hooks,
    /// The screen, and the rule that claimed it.
    Screen,
    /// Nothing accounts for the screen.
    Unknown,
}

/// What a reader concluded, and what it concluded it from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Verdict {
    pub phase: Phase,
    pub evidence: Evidence,
    /// The rule that claimed the screen, when one did.
    pub rule: Option<String>,
    /// Seconds since anything was last heard from the agent.
    pub age: u64,
}

/// An agent as a reader sees it: the record, and what amx makes of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub meta: Meta,
    pub state: State,
    pub verdict: Verdict,
}

impl View {
    pub fn id(&self) -> &str {
        &self.meta.id
    }

    pub fn phase(&self) -> Phase {
        self.verdict.phase
    }

    /// The one line that says what this agent is up to: what it is waiting to
    /// be told, else what it is doing, else what it answered.
    pub fn line(&self) -> Option<&str> {
        self.state
            .question
            .as_deref()
            .or(self.state.summary.as_deref())
            .or(self.state.result.as_deref())
    }

    /// The stable shape `--json` prints. Fields are added, never renamed or
    /// removed: callers branch on these.
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.meta.id,
            "state": self.verdict.phase.as_str(),
            "evidence": self.verdict.evidence,
            "rule": self.verdict.rule,
            "age": self.verdict.age,
            "since": self.state.since,
            "last_event": self.state.last_event,
            "seq": self.state.seq,
            "summary": self.state.summary,
            "question": self.state.question,
            "result": self.state.result,
            "source": self.state.source.map(source_name),
            "exit": self.state.exit,
            "task": self.meta.task,
            "dir": self.meta.dir,
            "worktree": self.meta.worktree,
            "branch": self.meta.branch,
            "base": self.meta.base,
            "pane": self.meta.pane.as_str(),
            "socket": self.meta.socket,
            "session": self.meta.session,
            "created": self.meta.created,
        })
    }
}

fn source_name(source: Source) -> &'static str {
    match source {
        Source::Payload => "payload",
        Source::Transcript => "transcript",
        Source::Screen => "screen",
        Source::Error => "error",
    }
}

/// Work out what an agent is doing.
///
/// `alive` is whether its pane is still there, and `capture` is asked for the
/// screen only when it is going to be read — a fresh record needs no tmux call
/// at all, which is what keeps `ls` cheap with a wall full of agents.
pub fn decide(
    state: &State,
    alive: bool,
    capture: impl FnOnce() -> Option<String>,
    rules: &Ruleset,
    now: u64,
    looks: usize,
) -> Verdict {
    let age = now.saturating_sub(state.last_event.max(state.since));

    if state.state.is_terminal() {
        return Verdict {
            phase: state.state,
            evidence: Evidence::Record,
            rule: None,
            age,
        };
    }

    if !alive {
        // The pane went without recording an exit: killed, or its server died.
        return Verdict {
            phase: Phase::Stopped,
            evidence: Evidence::Gone,
            rule: None,
            age,
        };
    }

    if age <= FRESH {
        return Verdict {
            phase: state.state,
            evidence: Evidence::Hooks,
            rule: None,
            age,
        };
    }

    let Some(screen) = capture() else {
        return Verdict {
            phase: Phase::Unknown,
            evidence: Evidence::Unknown,
            rule: None,
            age,
        };
    };

    match rules.claim(&screen, state.state, looks) {
        Claim::Ruled(rule) => Verdict {
            phase: rule.state,
            evidence: Evidence::Screen,
            rule: Some(rule.name.clone()),
            age,
        },
        // A rule claims the screen but may not end a turn that is on the
        // record as running. The record stands, with its age beside it.
        Claim::Unsettled(rule) => Verdict {
            phase: state.state,
            evidence: Evidence::Hooks,
            rule: Some(rule.name.clone()),
            age,
        },
        Claim::Unclaimed => Verdict {
            phase: Phase::Unknown,
            evidence: Evidence::Unknown,
            rule: None,
            age,
        },
    }
}

/// Read one agent.
pub fn view(root: &Path, id: &str, rules: &Ruleset, now: u64) -> Result<View> {
    let agent = Agent::open(root, id)?;
    let meta = agent.meta()?;
    let state = agent.state()?;
    let server = Server::from_socket(meta.socket.clone());

    let alive = state.state.is_terminal() || server.pane_alive(&meta.pane);
    let verdict = decide(
        &state,
        alive,
        || server.capture(&meta.pane).ok(),
        rules,
        now,
        1,
    );
    Ok(View {
        meta,
        state,
        verdict,
    })
}

/// Read every agent, oldest first.
///
/// One pane list per server rather than one per agent: a wall of ten agents is
/// one tmux call, not ten.
pub fn views(root: &Path, rules: &Ruleset, now: u64) -> Result<Vec<View>> {
    let mut views = Vec::new();
    let mut panes: Vec<(crate::tmux::Socket, Vec<crate::tmux::PaneId>)> = Vec::new();

    for id in crate::store::list(root)? {
        let agent = Agent::open(root, &id)?;
        let Ok(meta) = agent.meta() else { continue };
        let state = agent.state()?;
        let server = Server::from_socket(meta.socket.clone());

        let alive = if state.state.is_terminal() {
            true
        } else {
            let listed = match panes.iter().find(|(socket, _)| socket == &meta.socket) {
                Some((_, listed)) => listed,
                None => {
                    let listed = server.panes().unwrap_or_default();
                    panes.push((meta.socket.clone(), listed));
                    &panes.last().expect("just pushed").1
                }
            };
            listed.contains(&meta.pane)
        };

        let verdict = decide(
            &state,
            alive,
            || server.capture(&meta.pane).ok(),
            rules,
            now,
            1,
        );
        views.push(View {
            meta,
            state,
            verdict,
        });
    }

    views.sort_by_key(|view| (view.meta.created, view.meta.id.clone()));
    Ok(views)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules;

    const IDLE_SCREEN: &str = "\
✻ Worked for 2m 26s
❯
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    const A_SHELL: &str = "$ ls\nCargo.toml  src\n$\n";

    fn state(phase: Phase, last_event: u64) -> State {
        State {
            state: phase,
            last_event,
            since: last_event,
            ..State::default()
        }
    }

    fn decided(state: &State, alive: bool, screen: Option<&str>, now: u64) -> Verdict {
        decide(
            state,
            alive,
            || screen.map(str::to_string),
            rules::bundled(),
            now,
            1,
        )
    }

    #[test]
    fn reader_takes_the_record_when_the_record_says_how_it_ended() {
        for phase in [Phase::Done, Phase::Failed, Phase::Stopped] {
            // Long stale, no pane, and it does not matter: this is over.
            let verdict = decided(&state(phase, 100), false, Some(A_SHELL), 10_000);
            assert_eq!(verdict.phase, phase);
            assert_eq!(verdict.evidence, Evidence::Record);
        }
    }

    #[test]
    fn reader_calls_an_agent_with_no_pane_stopped() {
        let verdict = decided(&state(Phase::Working, 1_000), false, None, 1_001);
        assert_eq!(verdict.phase, Phase::Stopped);
        assert_eq!(
            verdict.evidence,
            Evidence::Gone,
            "fresh hooks do not outrank a pane that is not there"
        );
    }

    #[test]
    fn reader_trusts_the_hooks_while_they_are_fresh() {
        let verdict = decided(&state(Phase::Working, 1_000), true, None, 1_000 + FRESH);
        assert_eq!(verdict.phase, Phase::Working);
        assert_eq!(verdict.evidence, Evidence::Hooks);
        assert_eq!(verdict.age, FRESH);
        assert_eq!(verdict.rule, None, "the screen was never asked for");
    }

    #[test]
    fn reader_reads_the_screen_once_the_hooks_have_gone_quiet() {
        // Nothing outstanding on the record, so the idle rule may decide at
        // once — this is the parked agent that would otherwise sit at
        // `starting` for ever.
        let verdict = decided(
            &state(Phase::Starting, 1_000),
            true,
            Some(IDLE_SCREEN),
            1_100,
        );
        assert_eq!(verdict.phase, Phase::Idle);
        assert_eq!(verdict.evidence, Evidence::Screen);
        assert_eq!(verdict.rule.as_deref(), Some("idle_prompt"));
        assert_eq!(verdict.age, 100);
    }

    #[test]
    fn reader_does_not_end_a_running_turn_on_one_look() {
        // The idle screen and a mid-turn pause are the same bytes. With a turn
        // on the record, one look at a still screen decides nothing, and the
        // record stands with its age beside it.
        let verdict = decided(
            &state(Phase::Working, 1_000),
            true,
            Some(IDLE_SCREEN),
            1_100,
        );
        assert_eq!(verdict.phase, Phase::Working);
        assert_eq!(verdict.evidence, Evidence::Hooks);
        assert_eq!(verdict.rule.as_deref(), Some("idle_prompt"), "and says why");
        assert_eq!(verdict.age, 100);
    }

    #[test]
    fn reader_says_unknown_rather_than_guessing() {
        let verdict = decided(&state(Phase::Working, 1_000), true, Some(A_SHELL), 1_500);
        assert_eq!(verdict.phase, Phase::Unknown);
        assert_eq!(verdict.evidence, Evidence::Unknown);
        assert_eq!(verdict.age, 500, "and how long it has been out of touch");

        // A pane that cannot be captured is the same answer.
        let unreadable = decided(&state(Phase::Working, 1_000), true, None, 1_500);
        assert_eq!(unreadable.phase, Phase::Unknown);
    }

    #[test]
    fn reader_ages_from_whichever_is_later() {
        // A record written before its first event has a `since` and no
        // `last_event`; the agent is not therefore an hour stale.
        let mut fresh = state(Phase::Starting, 0);
        fresh.since = 1_000;
        assert_eq!(decided(&fresh, true, None, 1_002).age, 2);
    }
}
