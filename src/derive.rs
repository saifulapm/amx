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
//!    events are the best account there is — of what the agent is doing. They
//!    can say it has stopped on a question without saying which, and that part
//!    is on the pane and nowhere else, so it is read from there at once rather
//!    than waited for.
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
//!
//! Whatever it concludes, it concludes once. Every reader of an agent — `ls`,
//! `status`, the view, `--json` — is handed one [`View`], and what is on that
//! view agrees with the phase on it. A record can disagree with itself; the
//! answer a reader gives from it may not.

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
    /// An agent as one answer rather than two.
    ///
    /// A question belongs to the turn that is waiting for it to be answered.
    /// Once a reader has concluded that the agent is not waiting — it is back
    /// at work, the turn is over, the command has gone — a question still on
    /// the record is about a moment that has passed, and handing it out beside
    /// that conclusion is amx saying two things at once. The record is what
    /// lags: an older amx wrote it, or a nudge arrived after the answer did.
    ///
    /// The exception is `unknown`, which is a reader saying it cannot tell. It
    /// cannot tell that the question was answered either, and the one thing on
    /// a pane that somebody has to act on is the last thing to hide from them.
    ///
    /// The other half of one answer is that a question handed out is one
    /// somebody can act on. A sentence the vendor sent in place of a question
    /// is not, so it goes no further than here — see [`forget_the_placeholder`].
    pub fn new(meta: Meta, mut state: State, verdict: Verdict) -> View {
        if !matches!(verdict.phase, Phase::Waiting | Phase::Unknown) {
            state.asks(None);
        }
        forget_the_placeholder(&mut state);
        View {
            meta,
            state,
            verdict,
        }
    }

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
            // The question showing above, and the whole of the call it came
            // from here: every question in it with its own choices, the
            // sentences under them and the flag saying how many may be taken.
            // `multi` is the showing one's, because that is the question a
            // caller is about to answer.
            "questions": self.state.asking,
            "multi": self.state.multi(),
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
    /// What the screen says the agent is asking, when the screen was read —
    /// because it answered, or because the record said waiting and could not
    /// say what for. Only ever the screen's: a question a hook reported is on
    /// the record already, and the screen is where the choices under it are.
    pub asking: Option<Question>,
}

/// The sentences the vendor sends about a dialog it will not describe.
///
/// Measured against claude 2.1.240 on 2026-08-24, read out of the binary's own
/// dialog host: six seconds after a dialog goes up it fires a
/// `permission_prompt` notification whose whole message is that dialog's
/// title, and the title it gives every tool dialog is `Claude needs your
/// permission` — no tool, no command, nothing anybody could weigh. A tool
/// permission box has a notifier of its own on the same six-second timer, and
/// that one sends `Claude needs your permission to use <tool>`. Which of the
/// two lands last is the vendor's business, so what is recognised here is a
/// whole sentence and never the start of one: the sentence that names the tool
/// is one a caller can act on.
///
/// The idle nudge is the other, and it is not about a question at all: the
/// vendor sends it about a session with nothing open on it. One that says so
/// in its own payload is turned away where hooks are folded, but an older
/// vendor sends it with no type on it, and records outlive the amx that wrote
/// them.
const PLACEHOLDERS: [&str; 2] = [
    "Claude needs your permission",
    "Claude is waiting for your input",
];

/// Whether a question on the record is one of those, or has no words in it at
/// all. Either way there is nothing in it about what is being asked.
fn placeholder(question: &str) -> bool {
    let question = question.trim();
    question.is_empty() || PLACEHOLDERS.contains(&question)
}

/// Forget a question that says nothing about what is being asked.
///
/// A row carrying one says an agent is waiting, which is what a row carrying
/// nothing says, and it says it in words that read like an answer — so a
/// caller stops looking and a person reads the pane themselves. The choices go
/// with it, the way choices always go where their question goes.
///
/// What kind of thing is being asked stays. The vendor said that much, it is
/// not in the words, and it is what decides what may be sent back.
fn forget_the_placeholder(state: &mut State) {
    if state.question.as_deref().is_some_and(placeholder) {
        state.question = None;
        state.options.clear();
    }
}

/// Whether the pane is worth reading for a question the record has not got.
///
/// The vendor's own events say an agent has stopped on a question earlier and
/// surer than any screen does, and they can say it without saying what the
/// question is: `PermissionRequest` fires as the box goes up carrying the tool
/// it is about, so a box with no tool named leaves nothing written down at
/// all. The notification that describes it is six seconds behind and the
/// freshness window is eight, so a reader that waited for the record to go
/// stale would hand back a waiting agent with nothing to answer — which is
/// what `status` and `answer` did on the first ask, every time.
fn wants_the_question(state: &State) -> bool {
    state.state == Phase::Waiting && state.question.as_deref().is_none_or(placeholder)
}

/// Work out what an agent is doing.
///
/// `alive` is whether its pane is still there, and `capture` is asked for the
/// screen only when it is going to be read: a fresh record needs no tmux call
/// at all unless it is a record of an agent waiting on a question it cannot
/// name, which is what keeps `ls` cheap with a wall full of agents.
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
        // A record that says waiting and cannot say what for is half an
        // answer, and the half it is missing is the half somebody has to act
        // on. The hooks still decide the phase — this is the same conclusion
        // with the question beside it, on the first look rather than on the
        // one after the freshness runs out.
        let asking = wants_the_question(state)
            .then(capture)
            .flatten()
            .and_then(|screen| rules.asking(&screen));
        return Reading {
            asking,
            ..told(state.state, Evidence::Hooks, None)
        };
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
/// A placeholder is dropped first, and dropped from the document as well as
/// from the reading. It holds the one field the screen was going to fill, so a
/// record still carrying one has nothing to learn, and every reader after this
/// would find it there and go back to the pane for what this look already had.
///
/// The writer's lock is taken only when there is something new to write, so
/// the promise that readers never wait on writers holds for every look but the
/// one that finds the question.
fn note(agent: &Agent, state: &mut State, asking: &Question) {
    forget_the_placeholder(state);
    if !state.learns_from(asking) {
        return;
    }

    let heard = state.last_event;
    let noted = agent.writer().and_then(|writer| {
        writer.observe(|current| {
            // A hook that arrived while the pane was being read is the
            // vendor's own account of a moment this picture is already behind.
            if current.last_event == heard {
                forget_the_placeholder(current);
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

    Ok(View::new(meta, state, reading.verdict))
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

        views.push(View::new(meta, state, reading.verdict));
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

    /// The vendor's own notification about a dialog it will not describe,
    /// whole. It says a question exists and not one word about what it wants.
    const A_PLACEHOLDER: &str = "Claude needs your permission";

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

    fn meta() -> Meta {
        Meta {
            id: "fix-login-a1b".to_string(),
            task: "fix the login bug".to_string(),
            dir: std::path::PathBuf::from("/srv/app"),
            worktree: None,
            branch: None,
            base: None,
            socket: crate::tmux::Socket::Name("amx".to_string()),
            pane: crate::tmux::PaneId::new("%7").unwrap(),
            bg: false,
            session: None,
            transcript: None,
            created: 1,
        }
    }

    fn verdict(phase: Phase, evidence: Evidence, rule: Option<&str>) -> Verdict {
        Verdict {
            phase,
            evidence,
            rule: rule.map(str::to_string),
            age: 30,
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
    fn reader_reads_the_pane_the_moment_the_hooks_say_waiting() {
        // The record says waiting and cannot say what for: the event that put
        // the box up carried no tool name, and the notification that would
        // have described it is six seconds behind. The hooks are as fresh as
        // they ever get, and the pane is still the only place the question and
        // the choices under it are written.
        let first_look = reading(
            &state(Phase::Waiting, 1_000),
            true,
            Some(A_BLOCKING_SCREEN),
            1_000,
        );
        assert_eq!(first_look.verdict.phase, Phase::Waiting);
        assert_eq!(
            first_look.verdict.evidence,
            Evidence::Hooks,
            "the hooks still say what the agent is doing"
        );
        assert_eq!(
            first_look.verdict.rule, None,
            "the screen was read for the question, not for the state"
        );

        let asking = first_look.asking.expect("the pane is asking something");
        assert_eq!(asking.text, "Do you want to proceed?");
        assert_eq!(asking.options, ["Yes", "No"]);

        // A pane that cannot be read leaves the conclusion exactly where the
        // hooks left it.
        let unreadable = reading(&state(Phase::Waiting, 1_000), true, None, 1_000);
        assert_eq!(unreadable.verdict.phase, Phase::Waiting);
        assert_eq!(unreadable.verdict.evidence, Evidence::Hooks);
        assert_eq!(unreadable.asking, None);
    }

    #[test]
    fn reader_reads_the_pane_past_a_question_that_names_nothing() {
        // The vendor's dialog host sends the dialog's title and nothing else,
        // so the record ends up holding a sentence that says a question
        // exists. A caller can do as much with that as with an empty field,
        // and the pane is where the rest of it is.
        let mut placeheld = state(Phase::Waiting, 1_000);
        placeheld.question = Some(A_PLACEHOLDER.to_string());
        let asking = reading(&placeheld, true, Some(A_BLOCKING_SCREEN), 1_000)
            .asking
            .expect("a sentence that names nothing leaves the question unasked");
        assert_eq!(asking.text, "Do you want to proceed?");
        assert_eq!(asking.options, ["Yes", "No"]);

        // The sentence that does name the tool is the vendor telling a caller
        // something, and a reader holding one does not go to the pane at all.
        let mut told = state(Phase::Waiting, 1_000);
        told.question = Some(format!("{A_PLACEHOLDER} to use Bash"));
        assert_eq!(
            reading(&told, true, Some(A_BLOCKING_SCREEN), 1_000).asking,
            None,
            "the placeholder is a whole sentence, not the start of one"
        );
    }

    #[test]
    fn reader_never_hands_a_caller_the_placeholder() {
        use crate::store::Kind;

        // Whatever the pane said or failed to say, none of these reaches
        // anybody: a row carrying one says an agent is waiting, which is what
        // a row carrying nothing says, and it says it in words that read like
        // an answer.
        for nothing in [A_PLACEHOLDER, "Claude is waiting for your input", "   "] {
            let held = State {
                state: Phase::Waiting,
                question: Some(nothing.to_string()),
                options: vec!["Yes".to_string()],
                kind: Some(Kind::Permission),
                ..State::default()
            };
            let waiting = View::new(meta(), held, verdict(Phase::Waiting, Evidence::Hooks, None));

            assert_eq!(waiting.line(), None, "{nothing:?} is not a question");
            assert_eq!(waiting.json()["question"], serde_json::Value::Null);
            assert!(
                waiting.state.options.is_empty(),
                "and they are nobody's choices"
            );
            assert_eq!(
                waiting.kind(),
                Some(Kind::Permission),
                "what kind of thing is being asked is still known"
            );
        }
    }

    #[test]
    fn reader_lets_the_pane_answer_what_the_placeholder_was_holding() {
        use crate::store::Kind;

        // The forgetting comes before the record is asked whether the screen
        // tells it anything. Otherwise the placeholder sits in the one field
        // the screen was going to fill, the record learns nothing, and the
        // next reader finds it there and asks the pane all over again.
        let mut state = State {
            state: Phase::Waiting,
            question: Some(A_PLACEHOLDER.to_string()),
            kind: Some(Kind::Permission),
            ..State::default()
        };
        let seen = Question {
            text: "Do you want to proceed?".to_string(),
            options: vec!["Yes".to_string(), "No".to_string()],
        };

        forget_the_placeholder(&mut state);
        assert!(state.learns_from(&seen), "there is something to learn now");
        state.learn(&seen);
        assert_eq!(state.question.as_deref(), Some("Do you want to proceed?"));
        assert_eq!(state.options, ["Yes", "No"]);
        assert_eq!(
            state.kind,
            Some(Kind::Permission),
            "and it was a permission box all along"
        );
    }

    #[test]
    fn reader_has_no_question_from_a_screen_it_never_looked_at() {
        // A record that already says what is being asked has nothing to learn
        // from the pane, so fresh hooks answer without a capture.
        let mut told = state(Phase::Waiting, 1_000);
        told.question = Some("Do you want to proceed?".to_string());
        let fresh = reading(&told, true, Some(A_BLOCKING_SCREEN), 1_000);
        assert_eq!(fresh.verdict.evidence, Evidence::Hooks);
        assert_eq!(fresh.asking, None);

        // Neither has an agent that is working, whatever is on its screen.
        let mid_turn = reading(
            &state(Phase::Working, 1_000),
            true,
            Some(A_BLOCKING_SCREEN),
            1_000,
        );
        assert_eq!(mid_turn.verdict.phase, Phase::Working);
        assert_eq!(mid_turn.asking, None);

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

        let claimed = |rule: &str| verdict(Phase::Waiting, Evidence::Screen, Some(rule));

        // Nothing on the record: the screen is all there is, and the
        // folder-trust screen is the one kind no hook can ever report, because
        // it stands in front of the session every hook comes from.
        let read = View {
            meta: meta(),
            state: State::default(),
            verdict: claimed("folder_trust"),
        };
        assert_eq!(read.kind(), Some(Kind::Trust));
        assert_eq!(read.json()["kind"], "trust");

        // A hook said so, and a hook is the vendor's own account.
        let told = View {
            meta: meta(),
            state: State {
                kind: Some(Kind::Question),
                ..State::default()
            },
            verdict: claimed("permission_prompt"),
        };
        assert_eq!(told.kind(), Some(Kind::Question));
    }

    /// A call of two questions, the second taking more than one choice, as a
    /// hook folded it onto the record.
    fn a_call_of_two() -> State {
        use crate::store::{Ask, Choice, Kind};

        let choice = |label: &str, description: &str| Choice {
            label: label.to_string(),
            description: Some(description.to_string()),
            preview: None,
        };
        let mut state = State {
            state: Phase::Waiting,
            kind: Some(Kind::Question),
            ..State::default()
        };
        state.asks_all(vec![
            Ask {
                header: Some("Runtime".to_string()),
                text: "Which runtime should the service target?".to_string(),
                options: vec![
                    choice("Node", "Widest library support"),
                    choice("Deno", "Batteries included"),
                ],
                multi: false,
                answer: None,
            },
            Ask {
                header: Some("Rollout".to_string()),
                text: "Which rollout steps should run?".to_string(),
                options: vec![
                    choice("Canary", "Five percent first"),
                    choice("Announce", "Post to the channel"),
                ],
                multi: true,
                answer: None,
            },
        ]);
        state
    }

    #[test]
    fn reader_hands_a_caller_every_question_of_the_call() {
        let waiting = View::new(
            meta(),
            a_call_of_two(),
            verdict(Phase::Waiting, Evidence::Hooks, None),
        );
        let json = waiting.json();

        // What was there before means what it always meant: the question on
        // the screen and the choices under it.
        assert_eq!(json["question"], "Which runtime should the service target?");
        assert_eq!(json["options"][0], "Node");
        assert_eq!(json["options"][1], "Deno");
        assert_eq!(json["kind"], "question");

        // And beside them, the part no screen carries.
        assert_eq!(json["multi"], false, "the one showing takes one choice");
        assert_eq!(json["questions"].as_array().unwrap().len(), 2);
        assert_eq!(json["questions"][0]["header"], "Runtime");
        assert_eq!(
            json["questions"][0]["options"][0]["description"],
            "Widest library support"
        );
        assert_eq!(json["questions"][1]["multi"], true);
        assert_eq!(json["questions"][0]["answer"], serde_json::Value::Null);
    }

    #[test]
    fn reader_says_which_question_of_a_call_is_the_one_showing() {
        // A caller answering a call of several is told what to answer next by
        // the same fields it read the first time.
        let mut state = a_call_of_two();
        state.answered("Node");

        let waiting = View::new(
            meta(),
            state,
            verdict(Phase::Waiting, Evidence::Hooks, None),
        );
        let json = waiting.json();
        assert_eq!(json["question"], "Which rollout steps should run?");
        assert_eq!(json["options"][0], "Canary");
        assert_eq!(json["multi"], true, "and this one takes more than one");
        assert_eq!(json["questions"][0]["answer"], "Node");
    }

    #[test]
    fn reader_coherence_a_call_that_is_over_goes_with_its_question() {
        // The questions behind the one showing are as answered as it is, and
        // an agent that is back at work is not being asked any of them.
        let working = View::new(
            meta(),
            a_call_of_two(),
            verdict(Phase::Working, Evidence::Screen, Some("thinking")),
        );
        assert_eq!(working.json()["question"], serde_json::Value::Null);
        assert_eq!(working.json()["questions"], serde_json::json!([]));
        assert_eq!(working.json()["multi"], false);
        assert!(working.state.asking.is_empty());
    }

    #[test]
    fn reader_coherence_gives_one_account_of_an_agent_that_has_finished() {
        use crate::store::Kind;

        // The record the agent read-readme-md-and-799 was left with on
        // 2026-08-20: done, with the answer of its last turn, and with the
        // vendor's idle nudge still on it as the question. The hooks no longer
        // write a record like this one, and records outlive the amx that wrote
        // them, so a reader gives one answer whatever it is handed.
        let answered = State {
            state: Phase::Done,
            exit: Some(0),
            question: Some("Claude is waiting for your input".to_string()),
            options: vec!["Yes".to_string()],
            kind: Some(Kind::Permission),
            result: Some("Three that made me stop and re-read:".to_string()),
            source: Some(Source::Payload),
            ..State::default()
        };

        let view = View::new(
            meta(),
            answered,
            verdict(Phase::Done, Evidence::Record, None),
        );
        assert_eq!(view.phase(), Phase::Done);
        assert_eq!(view.line(), Some("Three that made me stop and re-read:"));
        assert_eq!(view.state.question, None);
        assert!(view.state.options.is_empty());
        assert_eq!(view.kind(), None);
        assert_eq!(view.json()["question"], serde_json::Value::Null);
        assert_eq!(
            view.json()["result"],
            "Three that made me stop and re-read:"
        );
    }

    #[test]
    fn reader_coherence_keeps_the_question_that_is_still_somebody_to_answer() {
        use crate::store::Kind;

        let asked = State {
            state: Phase::Waiting,
            question: Some("Do you want to proceed?".to_string()),
            options: vec!["Yes".to_string(), "No".to_string()],
            kind: Some(Kind::Permission),
            ..State::default()
        };

        let waiting = View::new(
            meta(),
            asked.clone(),
            verdict(Phase::Waiting, Evidence::Hooks, None),
        );
        assert_eq!(waiting.line(), Some("Do you want to proceed?"));
        assert_eq!(waiting.state.options, ["Yes", "No"]);
        assert_eq!(waiting.kind(), Some(Kind::Permission));

        // A reader that cannot say what the agent is doing cannot say the
        // question is answered either, and the one thing somebody has to act
        // on is the last thing to hide from them.
        let unreadable = View::new(
            meta(),
            asked.clone(),
            verdict(Phase::Unknown, Evidence::Unknown, None),
        );
        assert_eq!(unreadable.line(), Some("Do you want to proceed?"));
        assert_eq!(unreadable.kind(), Some(Kind::Permission));

        // Back at work: whatever that question was, it was answered on the
        // pane, and it is not what this agent is doing now.
        let working = State {
            summary: Some("Running Bash".to_string()),
            ..asked
        };
        let working = View::new(
            meta(),
            working,
            verdict(Phase::Working, Evidence::Screen, Some("thinking")),
        );
        assert_eq!(working.line(), Some("Running Bash"));
        assert_eq!(working.state.question, None);
    }
}
