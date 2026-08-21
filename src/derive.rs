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
//!
//! A reader concludes and forgets, with one exception. When the screen it read
//! was a screen asking a question, the question and the choices under it go on
//! the record: they are the one thing on a pane that somebody has to act on
//! rather than merely read, and the pane is the only place the choices are
//! ever written. Nothing a hook reported is corrected by them.

use anyhow::Result;
use serde::Serialize;
use std::path::Path;

use crate::rules::{Claim, Ruleset};
use crate::store::{Agent, Meta, Phase, Question, Source, State};
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

    /// What kind of thing this agent is being asked, if anything.
    ///
    /// The record's own word for it, else what the rule that claimed the
    /// screen says. The order is the one amx keeps everywhere: a hook is the
    /// vendor's account of its own state and a rule is amx's reading of a
    /// picture of it, so the screen fills what the hooks left empty and
    /// corrects nothing.
    pub fn kind(&self) -> Option<crate::store::Kind> {
        self.state
            .kind
            .or_else(|| asked_kind(self.verdict.rule.as_deref()))
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
            "options": self.state.options,
            "result": self.state.result,
            "source": self.state.source.map(source_name),
            "exit": self.state.exit,
            "kind": self.kind(),
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

/// What kind of thing the screen a rule claimed is asking for.
///
/// The rules say which screen is on the pane; this says what that screen wants
/// back, which is the part anything answering an agent needs. It is by name
/// because the screens are told apart by name everywhere else in amx, and a
/// name amx does not know asks for nothing it can describe.
///
/// The folder-trust screen is here and nowhere else: it stands in front of the
/// session that every hook comes from, so no hook can ever report it, and the
/// pane is the only place it is ever seen.
fn asked_kind(rule: Option<&str>) -> Option<crate::store::Kind> {
    use crate::store::Kind;

    match rule? {
        "permission_prompt" => Some(Kind::Permission),
        // An approval, answered yes or no about something the agent is about
        // to do. That the vendor draws it as a menu does not make it a
        // question with an answer of your own.
        "plan_approval" => Some(Kind::Permission),
        "ask_menu" => Some(Kind::Question),
        "folder_trust" => Some(Kind::Trust),
        _ => None,
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

/// What a reader made of an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub verdict: Verdict,
    /// What the screen says the agent is asking, when the screen is what
    /// answered. Only ever the screen's: a question a hook reported is on the
    /// record already, and the screen is where the choices under it are.
    pub asking: Option<Question>,
}

/// Work out what an agent is doing.
///
/// `alive` is whether its pane is still there, and `capture` is asked for the
/// screen only when it is going to be read — a fresh record needs no tmux call
/// at all, which is what keeps `ls` cheap with a wall full of agents.
pub fn read(
    state: &State,
    alive: bool,
    capture: impl FnOnce() -> Option<String>,
    rules: &Ruleset,
    now: u64,
    looks: usize,
) -> Reading {
    let age = now.saturating_sub(state.last_event.max(state.since));
    let told = |phase, evidence, rule: Option<&str>| Reading {
        verdict: Verdict {
            phase,
            evidence,
            rule: rule.map(str::to_string),
            age,
        },
        asking: None,
    };

    if state.state.is_terminal() {
        return told(state.state, Evidence::Record, None);
    }

    if !alive {
        // The pane went without recording an exit: killed, or its server died.
        return told(Phase::Stopped, Evidence::Gone, None);
    }

    if age <= FRESH {
        return told(state.state, Evidence::Hooks, None);
    }

    let Some(screen) = capture() else {
        return told(Phase::Unknown, Evidence::Unknown, None);
    };

    match rules.claim(&screen, state.state, looks) {
        Claim::Ruled(rule) => Reading {
            verdict: Verdict {
                phase: rule.state,
                evidence: Evidence::Screen,
                rule: Some(rule.name.clone()),
                age,
            },
            asking: rule.question(&screen),
        },
        // A rule claims the screen but may not end a turn that is on the
        // record as running. The record stands, with its age beside it.
        Claim::Unsettled(rule) => told(state.state, Evidence::Hooks, Some(&rule.name)),
        Claim::Unclaimed => told(Phase::Unknown, Evidence::Unknown, None),
    }
}

/// Write down what a screen is asking, when the record has not got it.
///
/// The one thing a reader records rather than works out and forgets. A
/// question is not a conclusion about an agent: it is something a person or a
/// caller has to answer, and the pane is the only place the choices under it
/// are ever written down. `ls` and the view are looking at the screen anyway,
/// and throwing away what they read there would leave every caller to capture
/// the pane and parse it again for itself.
///
/// The writer's lock is taken only when there is something new to write, so
/// the promise that readers never wait on writers holds for every look but the
/// one that finds the question.
fn note(agent: &Agent, state: &mut State, asking: &Question) {
    if !state.learns_from(asking) {
        return;
    }

    let heard = state.last_event;
    let noted = agent.writer().and_then(|writer| {
        writer.observe(|current| {
            // A hook that arrived while the pane was being read is the
            // vendor's own account of a moment this picture is already behind.
            if current.last_event == heard {
                current.learn(asking);
            }
        })
    });

    match noted {
        Ok(current) => *state = current,
        // A record that cannot be written is still a question that can be
        // reported, and the next look will try again.
        Err(_) => state.learn(asking),
    }
}

/// Read one agent.
pub fn view(root: &Path, id: &str, rules: &Ruleset, now: u64) -> Result<View> {
    let agent = Agent::open(root, id)?;
    let meta = agent.meta()?;
    let mut state = agent.state()?;
    let server = Server::from_socket(meta.socket.clone());

    let alive = state.state.is_terminal() || server.pane_alive(&meta.pane);
    let reading = read(
        &state,
        alive,
        || server.capture(&meta.pane).ok(),
        rules,
        now,
        1,
    );
    if let Some(asking) = &reading.asking {
        note(&agent, &mut state, asking);
    }

    Ok(View {
        meta,
        state,
        verdict: reading.verdict,
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
        let mut state = agent.state()?;
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

        let reading = read(
            &state,
            alive,
            || server.capture(&meta.pane).ok(),
            rules,
            now,
            1,
        );
        if let Some(asking) = &reading.asking {
            note(&agent, &mut state, asking);
        }

        views.push(View {
            meta,
            state,
            verdict: reading.verdict,
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

    /// A permission box, which is a screen with a question on it.
    const A_BLOCKING_SCREEN: &str = "\
────────────────────────────────
 Bash command
   rm -rf build
 Do you want to proceed?
 ❯ 1. Yes
   2. No
 Esc to cancel · Tab to amend
";

    fn state(phase: Phase, last_event: u64) -> State {
        State {
            state: phase,
            last_event,
            since: last_event,
            ..State::default()
        }
    }

    fn reading(state: &State, alive: bool, screen: Option<&str>, now: u64) -> Reading {
        read(
            state,
            alive,
            || screen.map(str::to_string),
            rules::bundled(),
            now,
            1,
        )
    }

    fn decided(state: &State, alive: bool, screen: Option<&str>, now: u64) -> Verdict {
        reading(state, alive, screen, now).verdict
    }

    #[test]
    fn reader_takes_the_question_off_the_screen_that_answered() {
        let reading = reading(
            &state(Phase::Working, 1_000),
            true,
            Some(A_BLOCKING_SCREEN),
            1_100,
        );
        assert_eq!(reading.verdict.phase, Phase::Waiting);

        let asking = reading.asking.expect("a blocking screen is asking");
        assert_eq!(asking.text, "Do you want to proceed?");
        assert_eq!(asking.options, ["Yes", "No"]);
    }

    #[test]
    fn reader_has_no_question_from_a_screen_it_never_looked_at() {
        // Fresh hooks answer without a capture, so there is nothing to read a
        // question out of — and the record already has whatever they reported.
        let fresh = reading(
            &state(Phase::Waiting, 1_000),
            true,
            Some(A_BLOCKING_SCREEN),
            1_000,
        );
        assert_eq!(fresh.verdict.evidence, Evidence::Hooks);
        assert_eq!(fresh.asking, None);

        // And a screen that is not asking anything says nothing about it.
        let quiet = reading(
            &state(Phase::Starting, 1_000),
            true,
            Some(IDLE_SCREEN),
            1_100,
        );
        assert_eq!(quiet.verdict.phase, Phase::Idle);
        assert_eq!(quiet.asking, None);
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

    #[test]
    fn reader_has_a_kind_for_every_screen_that_blocks() {
        use crate::store::Kind;

        // Every rule that stops an agent stops it on something somebody has to
        // answer, and what may be sent back depends on which. A blocking rule
        // added without a kind here would leave a caller guessing again.
        for rule in rules::bundled().rules() {
            let kind = asked_kind(Some(&rule.name));
            assert_eq!(
                kind.is_some(),
                rule.state == Phase::Waiting,
                "{} claims a {} screen",
                rule.name,
                rule.state
            );
        }
        assert_eq!(asked_kind(Some("folder_trust")), Some(Kind::Trust));
        assert_eq!(asked_kind(Some("ask_menu")), Some(Kind::Question));
        assert_eq!(asked_kind(None), None);
        assert_eq!(
            asked_kind(Some("a rule from a ruleset amx has not met")),
            None
        );
    }

    #[test]
    fn reader_says_the_kind_the_record_holds_over_the_kind_it_read() {
        use crate::store::Kind;

        let meta = Meta {
            id: "fix-login-a1b".to_string(),
            task: "fix the login bug".to_string(),
            dir: std::path::PathBuf::from("/srv/app"),
            worktree: None,
            branch: None,
            base: None,
            socket: crate::tmux::Socket::Name("amx".to_string()),
            pane: crate::tmux::PaneId::new("%7").unwrap(),
            session: None,
            transcript: None,
            created: 1,
        };
        let claimed = |rule: &str| Verdict {
            phase: Phase::Waiting,
            evidence: Evidence::Screen,
            rule: Some(rule.to_string()),
            age: 30,
        };

        // Nothing on the record: the screen is all there is, and the
        // folder-trust screen is the one kind no hook can ever report, because
        // it stands in front of the session every hook comes from.
        let read = View {
            meta: meta.clone(),
            state: State::default(),
            verdict: claimed("folder_trust"),
        };
        assert_eq!(read.kind(), Some(Kind::Trust));
        assert_eq!(read.json()["kind"], "trust");

        // A hook said so, and a hook is the vendor's own account.
        let told = View {
            meta,
            state: State {
                kind: Some(Kind::Question),
                ..State::default()
            },
            verdict: claimed("permission_prompt"),
        };
        assert_eq!(told.kind(), Some(Kind::Question));
    }
}
