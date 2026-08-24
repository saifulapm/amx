//! `amx answer` — give the agent's question what it is waiting for.
//!
//! The verb is a grammar and a refusal, and the grammar is the question's
//! rather than amx's. A permission box and the folder-trust screen read one
//! key — `y`, `n`, `1`–`9`, `enter`, `esc` — and anything wider typed at one
//! of those would be amx inventing an input language for a program it does not
//! control. A question the vendor asked itself is the other case: it offers
//! choices *and* a field for words of your own, so words are an answer to it
//! and to nothing else.
//!
//! The refusal matters as much. A key typed at an agent that is *not* asking
//! lands in whatever it does next, so "nothing pending" is an answer of its
//! own — exit 2 — and not a quiet success. The record is read before anything
//! is typed, because what may be sent back depends on what is being asked; a
//! command line amx cannot make an answer of never reaches the pane.
//!
//! Answering also clears the question from the record. The vendor says nothing
//! when a prompt is dismissed: the next hook comes when the agent gets to it,
//! which can be a while. Until then a caller reading the record would find the
//! same question still pending and answer it a second time, with the second key
//! landing somewhere nobody chose.
//!
//! A question the vendor asked itself is not one screen, and the keys that
//! finish one shape of it leave another standing. `docs/question-shapes.md`
//! measured the four against claude 2.1.240, and two of them are here: a
//! question that takes more than one choice checks boxes rather than choosing,
//! so nothing is submitted until the choices are left behind, and a call of
//! several questions ends on a Submit tab of the vendor's own that has to be
//! confirmed. Which shape is on the screen is not on the screen — the tab strip
//! elides its own headers as the pane narrows — so it is read off the record,
//! where the payload put it.

use anyhow::Result;
use std::path::Path;

use crate::derive;
use crate::store::{Agent, Ask, Event, Kind, Phase, State};
use crate::tmux::{PaneId, Server};
use crate::verbs::send::nothing_more_is_coming;
use crate::{exit, paths, rules, store};

/// The key that moves a menu's cursor onto the vendor's own free-text row.
///
/// Measured against claude 2.1.237 on 2026-08-21, reading the menu its
/// `AskUserQuestion` tool draws: the row list it hands its select is the
/// tool's own choices followed by one `Other` row, which is a text field
/// rather than a choice, and moving up from the first row wraps to the last —
/// so this lands on the field whatever the tool named, however many choices it
/// named, and without amx recognising a word of the vendor's own furniture.
///
/// Counting to it would be the alternative, and it is the one that goes wrong:
/// the number of choices amx holds is the tool's when a hook carried them and
/// the screen's — two rows longer — when a reader read them off the pane.
const TO_THE_FIELD: &str = "Up";

/// The key that leaves the choices of a question that takes more than one.
///
/// Measured against claude 2.1.240 on 2026-08-24: on a checkbox question every
/// digit and every `Enter` is a toggle, and the only way off the choices is
/// sideways. `→` is the next tab, which on a call of one question is the
/// vendor's own Submit tab.
const OFF_THE_CHOICES: &str = "Right";

/// The key that leaves the free-text row for the `Submit` row under it, on the
/// one shape that draws one. Measured with the rest of the checkbox screen.
const OFF_THE_FIELD: &str = "Down";

/// The key that takes what is highlighted: a choice, the `Submit` row of a
/// checkbox box, or `1. Submit answers` on the review screen.
const TAKE_IT: &str = "Enter";

/// Run the verb against the machine.
pub fn from_env(id: &str, typed: &str) -> Result<i32> {
    let root = paths::state_root()?;
    run(&root, id, typed)
}

/// The verb, with the state directory named.
pub fn run(root: &Path, id: &str, typed: &str) -> Result<i32> {
    let view = derive::view(root, id, rules::bundled(), store::now())?;
    let phase = view.phase();
    if phase.is_terminal() {
        return Ok(nothing_more_is_coming(id, phase));
    }
    if phase != Phase::Waiting {
        eprintln!("amx: {id} has no pending question; nothing to answer");
        return Ok(exit::BLOCKED);
    }

    // Nothing is opened, let alone typed, until this holds: an answer this
    // question cannot take is a mistake in the command line, and the agent
    // must never see it.
    let answer = match answer(typed, view.kind(), &view.state) {
        Ok(answer) => answer,
        Err(refused) => {
            eprintln!("amx: {refused}");
            return Ok(exit::USAGE);
        }
    };

    let agent = Agent::open(root, id)?;
    let server = Server::from_socket(view.meta.socket.clone());
    reply(
        &agent,
        &server,
        &view.meta.pane,
        answer,
        Shape::of(&view.state),
    )?;
    Ok(exit::OK)
}

/// What was typed at amx, once it is something this question can take.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Answer {
    /// One key of the grammar, under the name tmux knows it by.
    Key(String),
    /// The boxes to check on a question that takes more than one choice, in
    /// the order they were named and counting from one.
    Toggle(Vec<usize>),
    /// Words of the caller's own, for the question that offers a field.
    Words(String),
}

impl Answer {
    /// Whether this answer names the choice it is making.
    ///
    /// A digit, a set of them and words of your own all say what they are
    /// answering, so amx can put the answer on the record and press the
    /// vendor's confirm for it. `y`, `enter` and `esc` are keys whose effect
    /// on the screen amx does not model: what they did to the prompt is the
    /// next hook's business and the screen's after that.
    fn chose(&self) -> bool {
        match self {
            Answer::Key(key) => one_choice(key).is_some(),
            _ => true,
        }
    }

    /// The answer as the vendor itself would write it down: the label that was
    /// chosen, the labels that were checked joined the way its own answer map
    /// joins them, or the words that were typed.
    fn said(&self, pending: Option<&Ask>) -> String {
        let label = |at: usize| match pending.and_then(|ask| ask.options.get(at - 1)) {
            Some(choice) => choice.label.clone(),
            None => at.to_string(),
        };
        match self {
            Answer::Key(key) => match one_choice(key) {
                Some(at) => label(at),
                None => key.clone(),
            },
            Answer::Toggle(checked) => checked
                .iter()
                .map(|at| label(*at))
                .collect::<Vec<_>>()
                .join(", "),
            Answer::Words(words) => words.clone(),
        }
    }

    /// What the log keeps of it: what was typed, under the name the event
    /// stream already prints, and what it came to.
    fn event(&self, said: &str) -> serde_json::Value {
        match self {
            Answer::Key(key) => serde_json::json!({ "key": key }),
            Answer::Toggle(checked) => serde_json::json!({
                "key": checked.iter().map(usize::to_string).collect::<Vec<_>>().join(","),
                "answer": said,
            }),
            Answer::Words(words) => serde_json::json!({ "text": words }),
        }
    }
}

/// What the screen does with an answer to the question showing.
///
/// Both of these are read off the record and never off the pane. The tab strip
/// elides its own headers as the pane narrows — at 24 columns the showing
/// tab's name is drawn as an ellipsis and nothing else — so how many questions
/// a call holds, which are answered and which take more than one choice is in
/// the payload and only there.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Shape {
    /// The question showing takes more than one choice, so a key that would
    /// answer a plain menu only checks a box here.
    multi: bool,
    /// This answer leaves nothing of the call unanswered, and the call is one
    /// the vendor draws its own Submit tab for. Nothing else will press it: no
    /// rule in the shipped ruleset claims that screen, so an agent left on it
    /// reads `unknown` with no question on its row.
    confirms: bool,
}

impl Shape {
    fn of(state: &State) -> Shape {
        let outstanding = state
            .asking
            .iter()
            .filter(|ask| ask.answer.is_none())
            .count();
        Shape {
            multi: state.multi(),
            // More than one question, or one that takes more than one choice:
            // the two shapes the vendor draws a tab strip and a Submit tab for.
            confirms: outstanding == 1 && (state.asking.len() > 1 || state.multi()),
        }
    }
}

/// One step of the sequence that answers a question.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    /// A key, under the name tmux knows it by.
    Key(String),
    /// Text, into whatever field has the cursor.
    Type(String),
}

/// Read what was typed as an answer to this question.
///
/// The grammar comes first wherever it applies: a bare `2` at a menu is the
/// second choice, not a two-character opinion, and that is how every prompt
/// the vendor draws reads it. Words are what is left, and they are an answer
/// only where there is a field to put them in.
///
/// What the record holds about the question is read before any of that,
/// because the same key is not the same act at every shape: at a plain menu a
/// digit chooses and submits at once, and at a question that takes more than
/// one choice it checks a box and leaves the prompt standing.
fn answer(typed: &str, kind: Option<Kind>, state: &State) -> Result<Answer, String> {
    let pending = state.pending();
    let multi = state.multi();

    if a_list(typed) {
        return boxes(typed, multi, pending);
    }
    if let Some(key) = named(typed) {
        return match (multi, one_choice(&key)) {
            (true, Some(_)) => boxes(&key, multi, pending),
            _ => Ok(Answer::Key(key)),
        };
    }
    match kind {
        // Empty is never an answer, and at this prompt it is worse than
        // nothing: the vendor reads a blank submission as a cancel.
        Some(Kind::Question) if !typed.trim().is_empty() => {
            Ok(Answer::Words(typed.trim().to_string()))
        }
        _ => Err(format!(
            "`{typed}` is not an answer. {}",
            grammar(kind, multi)
        )),
    }
}

/// Whether what was typed is a list of choices rather than a sentence with a
/// comma in it.
///
/// A comma is a caller's punctuation as often as it is their separator, and
/// "neither, keep both" is an answer in words at the one question that takes
/// them. Every part of a list is a single key, which no sentence is.
fn a_list(typed: &str) -> bool {
    typed.contains(',')
        && typed
            .split(',')
            .all(|part| part.trim().chars().count() <= 1)
}

/// Read the boxes to check, for the question that has boxes to check.
///
/// The refusal is the point of it. `1,3` at a plain menu would choose the
/// first, submit the tab with it, and leave the `3` to land on whatever the
/// vendor drew next; a choice the question never offered is one of the two
/// numbered rows the vendor adds to every menu it draws, and checking a box
/// twice leaves it as it was.
fn boxes(typed: &str, multi: bool, pending: Option<&Ask>) -> Result<Answer, String> {
    if !multi {
        return Err(format!(
            "`{typed}` checks the boxes of a question that takes several, \
             and this one takes one choice"
        ));
    }
    let offered = pending.map(|ask| ask.options.len()).unwrap_or_default();
    let mut checked = Vec::new();
    for part in typed.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(format!("`{typed}` has a choice missing between its commas"));
        }
        let Some(at) = one_choice(part) else {
            return Err(format!("`{part}` is not one of this question's choices"));
        };
        if at > offered {
            return Err(format!(
                "this question offers {offered} choices, and `{part}` is not one of them"
            ));
        }
        if checked.contains(&at) {
            return Err(format!(
                "`{part}` is checked twice, which leaves it as it was"
            ));
        }
        checked.push(at);
    }
    Ok(Answer::Toggle(checked))
}

/// The choice a key makes, counting from one, for the keys that make one.
fn one_choice(key: &str) -> Option<usize> {
    match key.as_bytes() {
        [digit @ b'1'..=b'9'] => Some(usize::from(digit - b'0')),
        _ => None,
    }
}

/// The keystrokes that put this answer on that question's screen, in order.
///
/// Every sequence here was measured against claude 2.1.240 on 2026-08-24 and
/// is written down in `docs/question-shapes.md`. The two that are not a single
/// key are the two that go wrong quietly: a checkbox question submits nothing
/// on its own, and its free-text row checks itself as it is typed into, so an
/// `Enter` there unchecks it again and leaves the prompt up.
fn steps(answer: &Answer, shape: Shape) -> Vec<Step> {
    let key = |name: &str| Step::Key(name.to_string());
    let mut steps = Vec::new();
    match answer {
        Answer::Key(pressed) => steps.push(key(pressed)),
        Answer::Toggle(checked) => {
            steps.extend(checked.iter().map(|at| key(&at.to_string())));
            steps.push(key(OFF_THE_CHOICES));
        }
        Answer::Words(words) => {
            steps.push(key(TO_THE_FIELD));
            steps.push(Step::Type(words.clone()));
            if shape.multi {
                steps.push(key(OFF_THE_FIELD));
            }
            steps.push(key(TAKE_IT));
        }
    }
    if shape.confirms && answer.chose() {
        steps.push(key(TAKE_IT));
    }
    steps
}

/// What this question would have taken, for the caller who typed something
/// else.
fn grammar(kind: Option<Kind>, multi: bool) -> String {
    match (kind, multi) {
        (Some(Kind::Question), true) => {
            "use 1-9, 1,3 for several, enter, esc, or words of your own".to_string()
        }
        (Some(Kind::Question), false) => "use 1-9, enter, esc, or words of your own".to_string(),
        _ => "use y, n, 1-9, enter or esc".to_string(),
    }
}

/// Type one key of the grammar at the agent, and record that it was typed.
///
/// One key and nothing after it: this is the view's way in, and what a key
/// does to a screen amx has not read the shape of is not amx's to guess at.
/// The record is what stops the question being answered twice — the vendor
/// says nothing when a prompt is dismissed, so until its next hook arrives the
/// only thing that knows the question is dealt with is this.
pub fn press(agent: &Agent, server: &Server, pane: &PaneId, pressed: &str) -> Result<()> {
    reply(
        agent,
        server,
        pane,
        Answer::Key(pressed.to_string()),
        Shape::default(),
    )
}

/// Put words of the caller's own into the field the question offers, and
/// record what was put there.
///
/// The cursor is moved onto that field before a byte of the words is sent, and
/// that order is the whole of the care here. A menu reads a digit as a choice,
/// so words delivered to a menu that is still on its first row would have
/// their own digits picked out and pressed — "keep fixture 2" answering `2`,
/// which is somebody else's answer and cannot be taken back. Once the field
/// has the cursor every key is a character in it, and `Enter` submits what was
/// typed rather than what was highlighted.
///
/// Which keys finish it is the record's to say, so the record is read: on a
/// question that takes more than one choice the same `Enter` unchecks the row
/// the words just checked.
pub fn say(agent: &Agent, server: &Server, pane: &PaneId, words: &str) -> Result<()> {
    let shape = Shape::of(&agent.state()?);
    reply(agent, server, pane, Answer::Words(words.to_string()), shape)
}

/// Type an answer at the pane, and write down what was answered.
fn reply(
    agent: &Agent,
    server: &Server,
    pane: &PaneId,
    answer: Answer,
    shape: Shape,
) -> Result<()> {
    drive(server, pane, &steps(&answer, shape))?;
    answered(agent, &answer)
}

/// Type one sequence at the pane.
///
/// Runs of keys go in one call, because tmux takes them in order and a pane
/// that has begun reading them should not have to wait on another process
/// being started for the next.
fn drive(server: &Server, pane: &PaneId, steps: &[Step]) -> Result<()> {
    let mut pressing: Vec<&str> = Vec::new();
    for step in steps {
        match step {
            Step::Key(key) => pressing.push(key),
            Step::Type(text) => {
                if !pressing.is_empty() {
                    server.send_keys(pane, &pressing)?;
                    pressing.clear();
                }
                server.paste(pane, text)?;
            }
        }
    }
    match pressing.is_empty() {
        true => Ok(()),
        false => server.send_keys(pane, &pressing),
    }
}

/// Write down that the question was answered, and what is left of the call it
/// belonged to.
///
/// A call of several questions does not end when one of them is answered: the
/// vendor records it, moves to the tab after it, and the prompt is still up.
/// So an answer amx can name goes on the question it answered and the next one
/// takes the screen, and only a call with nothing left outstanding leaves the
/// agent working again.
///
/// The question goes with its choices and its kind: they were the choices
/// under *this* question, and a row still offering them after it has been
/// answered is a row inviting somebody to answer it again.
fn answered(agent: &Agent, answer: &Answer) -> Result<()> {
    let writer = agent.writer()?;
    let said = answer.said(writer.state()?.pending());
    writer.append(&Event::new("answer", answer.event(&said)))?;
    writer.update_state(|state| {
        match answer.chose() && !state.asking.is_empty() {
            // The answer goes on the question it answered, and the tab after
            // it takes the screen.
            true => state.answered(said),
            // Any other key is one whose effect amx does not model, so it
            // stops claiming to know what is on the screen at all.
            false => state.asks(None),
        }
        // Nothing of it is outstanding: the agent is getting on with it. What
        // it is really doing is the next hook's business, and the screen's
        // after that.
        if state.pending().is_none() {
            state.state = Phase::Working;
            state.asks(None);
        }
    })?;
    Ok(())
}

/// One key of the grammar, under the name tmux knows it by.
///
/// `enter` and `esc` are the two that are not their own keystrokes — sending
/// the letters would type a word at the agent. Case and surrounding space are
/// a typo, not a different intent. `enter` earns its place because a prompt
/// with a highlighted default takes it and nothing else.
pub fn named(key: &str) -> Option<String> {
    let key = key.trim().to_ascii_lowercase();
    match key.as_str() {
        "y" | "n" => Some(key),
        "enter" => Some("Enter".to_string()),
        "esc" => Some("Escape".to_string()),
        digit if matches!(digit.as_bytes(), [b'1'..=b'9']) => Some(key),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Choice;

    /// A choice with the sentence the screen draws under it.
    fn choice(label: &str, description: &str) -> Choice {
        Choice {
            label: label.to_string(),
            description: Some(description.to_string()),
            preview: None,
        }
    }

    /// The record of a call, as a hook that carried the whole payload leaves
    /// it.
    fn asking(questions: Vec<Ask>) -> State {
        let mut state = State {
            state: Phase::Waiting,
            kind: Some(Kind::Question),
            ..State::default()
        };
        state.asks_all(questions);
        state
    }

    /// The checkbox question measured against claude 2.1.240 on 2026-08-24,
    /// as `docs/question-shapes.md` records its payload: one question, three
    /// choices, and more than one of them may be taken.
    fn a_checkbox_question() -> State {
        asking(vec![Ask {
            header: Some("Features".to_string()),
            text: "Which features should be enabled?".to_string(),
            options: vec![
                choice("Logging", "Write a log file"),
                choice("Metrics", "Export counters"),
                choice("Tracing", "Emit spans"),
            ],
            multi: true,
            answer: None,
        }])
    }

    /// The plain menu of the same measurement: one choice, and the vendor
    /// submits the tab the moment it is made.
    fn a_plain_question() -> State {
        asking(vec![Ask {
            header: Some("License".to_string()),
            text: "Which license should the LICENSE file contain?".to_string(),
            options: vec![
                choice("MIT", "Short and permissive"),
                choice("Apache-2.0", "Permissive with a patent grant"),
            ],
            multi: false,
            answer: None,
        }])
    }

    /// A permission box: a question with no call behind it and one key to
    /// answer it.
    fn a_permission_box() -> State {
        State {
            state: Phase::Waiting,
            question: Some("Claude needs your permission to use Bash".to_string()),
            options: vec!["Yes".to_string(), "No".to_string()],
            kind: Some(Kind::Permission),
            ..State::default()
        }
    }

    /// Keys, in the order they are typed.
    fn keys(named: &[&str]) -> Vec<Step> {
        named.iter().map(|key| Step::Key(key.to_string())).collect()
    }

    /// What amx would type at this question to answer it this way.
    fn typed(state: &State, at: &str) -> Vec<Step> {
        let answer = answer(at, state.kind, state).expect("an answer this question takes");
        steps(&answer, Shape::of(state))
    }

    #[test]
    fn surfaces_a_question_that_takes_several_choices_takes_several() {
        // Measured against 2.1.240: a digit checks a box without moving the
        // cursor and without submitting, `→` leaves the choices for the Submit
        // tab, and the `Enter` after it is the one on the review screen.
        let state = a_checkbox_question();
        assert_eq!(
            answer("1,3", Some(Kind::Question), &state),
            Ok(Answer::Toggle(vec![1, 3]))
        );
        assert_eq!(typed(&state, "1,3"), keys(&["1", "3", "Right", "Enter"]));

        // Space around a choice is a shell's doing, not the caller's meaning.
        assert_eq!(
            typed(&state, " 1 , 3 "),
            keys(&["1", "3", "Right", "Enter"])
        );
    }

    #[test]
    fn surfaces_one_choice_of_a_checkbox_menu_is_still_a_box() {
        // The same key at the two shapes is not the same act: at a plain menu
        // `1` chooses and submits at once, at a checkbox one it checks a box
        // and the prompt stays up until something submits it.
        let checkbox = a_checkbox_question();
        assert_eq!(
            answer("1", Some(Kind::Question), &checkbox),
            Ok(Answer::Toggle(vec![1]))
        );
        assert_eq!(typed(&checkbox, "1"), keys(&["1", "Right", "Enter"]));

        let plain = a_plain_question();
        assert_eq!(
            answer("1", Some(Kind::Question), &plain),
            Ok(Answer::Key("1".to_string()))
        );
        assert_eq!(typed(&plain, "1"), keys(&["1"]));
    }

    #[test]
    fn surfaces_a_question_that_takes_one_choice_is_offered_one() {
        // `1,3` at a plain menu would choose the first, submit the tab, and
        // leave the `3` to land on whatever the vendor drew next.
        for state in [a_plain_question(), a_permission_box(), State::default()] {
            let refused = answer("1,3", state.kind, &state).expect_err("one choice");
            assert!(refused.contains("one choice"), "{refused}");
        }
    }

    #[test]
    fn surfaces_a_box_the_question_does_not_offer_is_not_a_box() {
        // The screen carries two numbered rows the question never named — the
        // free-text row and `Chat about this` — so a fourth choice at a
        // three-choice question is the vendor's own furniture, not a choice.
        let state = a_checkbox_question();
        for refused in ["1,4", "1,9", "0,1"] {
            assert!(
                answer(refused, Some(Kind::Question), &state).is_err(),
                "{refused} is not a choice this question offers"
            );
        }

        // And checking the same box twice leaves it as it was.
        let refused = answer("1,1", Some(Kind::Question), &state).expect_err("twice");
        assert!(refused.contains("twice"), "{refused}");
    }

    #[test]
    fn surfaces_the_answer_that_finishes_a_call_confirms_it() {
        // A call of more than one question ends on the vendor's own Submit
        // tab, which no rule claims and nothing else will press.
        let mut state = asking(vec![
            a_plain_question().asking[0].clone(),
            a_checkbox_question().asking[0].clone(),
        ]);
        assert!(!Shape::of(&state).confirms, "two questions are outstanding");
        assert_eq!(typed(&state, "1"), keys(&["1"]), "and the vendor advances");

        state.answered("MIT");
        assert!(Shape::of(&state).confirms, "this one is the last of them");
        assert_eq!(typed(&state, "1,3"), keys(&["1", "3", "Right", "Enter"]));

        // A lone plain menu draws no Submit tab at all: the digit submits it,
        // and an `Enter` after that would land in the composer.
        assert!(!Shape::of(&a_plain_question()).confirms);
        assert!(!Shape::of(&a_permission_box()).confirms);
    }

    #[test]
    fn surfaces_the_record_names_the_choices_that_were_checked() {
        // The vendor's own answer for a checkbox question is the labels joined
        // with a comma, and that is what a caller reading the record wants
        // back rather than the keys amx typed.
        let state = a_checkbox_question();
        let pending = state.pending();
        assert_eq!(Answer::Toggle(vec![1, 3]).said(pending), "Logging, Tracing");
        assert_eq!(Answer::Key("2".to_string()).said(pending), "Metrics");
        assert_eq!(
            Answer::Words("audit".to_string()).said(pending),
            "audit",
            "and words of your own are their own answer"
        );

        // A question with no choices on the record is answered by the key.
        assert_eq!(Answer::Key("y".to_string()).said(None), "y");
    }

    #[test]
    fn the_grammar_is_y_n_one_through_nine_enter_and_esc() {
        for key in ["y", "n", "1", "5", "9"] {
            assert_eq!(named(key).as_deref(), Some(key));
        }
        assert_eq!(named("enter").as_deref(), Some("Enter"));
        assert_eq!(named("esc").as_deref(), Some("Escape"));
    }

    #[test]
    fn a_shouted_key_is_the_same_key() {
        assert_eq!(named("Y").as_deref(), Some("y"));
        assert_eq!(named("ESC").as_deref(), Some("Escape"));
        assert_eq!(named(" 2 ").as_deref(), Some("2"));
    }

    #[test]
    fn nothing_else_is_an_answer() {
        // `0` is not an option any prompt offers, and a word is a message the
        // caller meant to send. Both are refused before a key is typed.
        for refused in [
            "", "0", "10", "z", "yes", "escape", "return", "esc esc", "^[",
        ] {
            assert_eq!(named(refused), None, "{refused:?} must not be an answer");
        }
    }

    #[test]
    fn surfaces_a_question_of_the_vendors_own_takes_words() {
        let state = a_plain_question();
        assert_eq!(
            answer("neither, keep both", Some(Kind::Question), &state),
            Ok(Answer::Words("neither, keep both".to_string()))
        );
        // Space around them is a shell's doing, not the caller's meaning.
        assert_eq!(
            answer("  the sqlite one  ", Some(Kind::Question), &state),
            Ok(Answer::Words("the sqlite one".to_string()))
        );
    }

    #[test]
    fn surfaces_a_prompt_that_reads_one_key_is_offered_one_key() {
        // A permission box, the trust screen, and a question amx has not been
        // told the kind of. Words at any of them land on whatever is
        // highlighted, which is an answer nobody chose.
        for kind in [None, Some(Kind::Permission), Some(Kind::Trust)] {
            let state = State {
                kind,
                ..a_permission_box()
            };
            assert!(
                answer("neither, keep both", kind, &state).is_err(),
                "{kind:?}"
            );
            assert!(grammar(kind, false).contains("y, n, 1-9"), "{kind:?}");
        }
    }

    #[test]
    fn surfaces_the_choices_are_still_read_as_choices() {
        // A menu offers a field *and* numbered choices, and `2` at one of them
        // is the second choice — which is how the vendor reads it, and what a
        // caller writing `amx answer <id> 2` means.
        let state = a_plain_question();
        for key in ["2", "y", "enter", "esc"] {
            assert!(
                matches!(
                    answer(key, Some(Kind::Question), &state),
                    Ok(Answer::Key(_))
                ),
                "{key:?}"
            );
        }
    }

    #[test]
    fn surfaces_an_empty_answer_is_not_an_answer_to_a_question_either() {
        // The vendor reads a blank submission at its own menu as a cancel, so
        // this is not a harmless nothing.
        let state = a_plain_question();
        for blank in ["", "   ", "\t\n"] {
            assert!(
                answer(blank, Some(Kind::Question), &state).is_err(),
                "{blank:?}"
            );
        }
        assert!(grammar(Some(Kind::Question), false).contains("words of your own"));
        assert!(
            grammar(Some(Kind::Question), true).contains("1,3"),
            "a question that takes several says how to give it several"
        );
    }

    /// An agent with a record and no pane: what is written down when a
    /// question is answered, without a tmux server in it.
    fn recorded(root: &Path, asking: &State) -> Agent {
        let meta = crate::store::Meta {
            id: "pick-a1b".to_string(),
            task: "port the importer".to_string(),
            dir: std::path::PathBuf::from("/srv/app"),
            worktree: None,
            branch: None,
            base: None,
            socket: crate::tmux::Socket::Name("amx".to_string()),
            pane: PaneId::new("%1").unwrap(),
            bg: false,
            session: None,
            transcript: None,
            created: 1,
        };
        let agent = Agent::create(root, &meta).unwrap();
        let asking = asking.clone();
        agent
            .writer()
            .unwrap()
            .update_state(|state| *state = asking)
            .unwrap();
        agent
    }

    #[test]
    fn surfaces_answering_one_question_of_a_call_puts_up_the_next() {
        // Measured against 2.1.240: answering a tab does not return the vendor
        // to its composer. Until every tab is answered the prompt is still up,
        // and a record that said `working` would have the next caller sending
        // a message into a question.
        let call = asking(vec![
            a_plain_question().asking[0].clone(),
            a_checkbox_question().asking[0].clone(),
        ]);
        let root = tempfile::TempDir::new().unwrap();
        let agent = recorded(root.path(), &call);

        answered(&agent, &Answer::Key("2".to_string())).unwrap();
        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Waiting, "the prompt is still up");
        assert_eq!(state.asking[0].answer.as_deref(), Some("Apache-2.0"));
        assert_eq!(
            state.question.as_deref(),
            Some("Which features should be enabled?")
        );
        assert!(state.multi());

        answered(&agent, &Answer::Toggle(vec![1, 3])).unwrap();
        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Working, "and now it is over");
        assert_eq!(state.question, None);
        assert!(state.asking.is_empty(), "and it leaves nothing behind");

        // The labels the vendor's own answer map would have held.
        let answers: Vec<_> = agent
            .events()
            .unwrap()
            .iter()
            .map(|event| event.payload.clone())
            .collect();
        assert_eq!(answers[0]["key"], "2");
        assert_eq!(answers[1]["key"], "1,3");
        assert_eq!(answers[1]["answer"], "Logging, Tracing");
    }

    #[test]
    fn surfaces_a_key_amx_cannot_name_the_answer_of_leaves_no_question_behind() {
        // `esc` takes the whole prompt away, tabs and all, and `enter` takes
        // whatever the cursor was on. Neither is a choice amx can write down,
        // so it stops claiming to know what is on the screen.
        for key in ["Escape", "Enter", "y"] {
            let root = tempfile::TempDir::new().unwrap();
            let call = asking(vec![
                a_plain_question().asking[0].clone(),
                a_checkbox_question().asking[0].clone(),
            ]);
            let agent = recorded(root.path(), &call);
            answered(&agent, &Answer::Key(key.to_string())).unwrap();

            let state = agent.state().unwrap();
            assert_eq!(state.state, Phase::Working, "{key}");
            assert_eq!(state.question, None, "{key}");
            assert!(state.asking.is_empty(), "{key}");
            assert_eq!(state.kind, None, "{key}");
        }
    }
}
