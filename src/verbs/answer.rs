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
//! measured four of them against claude 2.1.240, and each has an answer here.
//! A question that takes more than one choice checks boxes rather than
//! choosing, so nothing is submitted until the choices are left behind; a call
//! of several questions ends on a Submit tab of the vendor's own that has to be
//! confirmed; the free-text row every menu carries takes each key as a
//! character, so `--text` is how a caller says a `2` is the character and not
//! the second choice; and where the choices carry a preview the vendor draws a
//! field for a note, which `--note` fills before the choice that carries it is
//! made.
//!
//! Which shape is on the screen is not on the screen — the tab strip elides its
//! own headers as the pane narrows — so it is read off the record, where the
//! payload put it. Each of the three is refused where the question offers no
//! such thing, and the refusal is what keeps a key from landing on somebody
//! else's answer: `n` at a menu with no preview does nothing at all, and the
//! note would be typed at the menu with its first digit answering it.

use anyhow::Result;
use std::path::Path;

use crate::cli::AnswerArgs;
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

/// The key that puts the cursor in the notes field, on the one shape that
/// draws one. Measured against 2.1.240: at a menu with no preview it does
/// nothing at all, which is why a note is refused there rather than typed.
const TO_THE_NOTES: &str = "n";

/// The key that comes back out of the notes field with the note kept.
///
/// It is the same key that cancels the prompt from a choice, and from inside
/// the field it does not: measured against 2.1.240, `Escape` there leaves the
/// note behind and puts the cursor back on the choices. Submitting from inside
/// the field is what would go wrong, and it is what amx never does: the vendor
/// takes that as a complete answer and writes `(notes only)` where the choice
/// should be.
const OFF_THE_NOTES: &str = "Escape";

/// Run the verb against the machine.
pub fn from_env(id: &str, typed: &AnswerArgs) -> Result<i32> {
    let root = paths::state_root()?;
    run(&root, id, typed)
}

/// The verb, with the state directory named.
pub fn run(root: &Path, id: &str, typed: &AnswerArgs) -> Result<i32> {
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
    let (answer, note) = match read(typed, view.kind(), &view.state) {
        Ok(read) => read,
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
        note.as_deref(),
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

/// Read the command line as an answer to this question, and the note the
/// vendor lets one ride beside.
///
/// The note is read first because it is the part that is not an answer: it
/// rides beside one, and a question that draws no field for it takes none.
fn read(
    args: &AnswerArgs,
    kind: Option<Kind>,
    state: &State,
) -> Result<(Answer, Option<String>), String> {
    let note = note(args, state.pending())?;
    Ok((answer(args, kind, state)?, note))
}

/// The note to send beside the answer, once the question draws a field for it.
///
/// The vendor draws that field where a choice carries a preview and only
/// there: `n` at a menu without one does nothing at all, so the note would be
/// typed at the menu itself and the first digit in it would answer the
/// question.
fn note(args: &AnswerArgs, pending: Option<&Ask>) -> Result<Option<String>, String> {
    let Some(note) = &args.note else {
        return Ok(None);
    };
    if !previewed(pending) {
        return Err(
            "this question draws no notes field: the vendor draws one where a choice \
             carries a preview, and none of these do"
                .to_string(),
        );
    }
    if note.trim().is_empty() {
        return Err("a note with nothing in it is not a note".to_string());
    }
    Ok(Some(note.trim().to_string()))
}

/// Read the command line as an answer to this question.
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
fn answer(args: &AnswerArgs, kind: Option<Kind>, state: &State) -> Result<Answer, String> {
    let pending = state.pending();
    let multi = state.multi();

    // `--text` says which of the two a thing that reads as both is, so it is
    // read before the grammar rather than through it.
    if let Some(text) = &args.text {
        return field(text, kind, pending, multi);
    }

    let typed = args.key.as_deref().unwrap_or_default();
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
        Some(Kind::Question) if !typed.trim().is_empty() && !previewed(pending) => {
            Ok(Answer::Words(typed.trim().to_string()))
        }
        _ => Err(format!(
            "`{typed}` is not an answer. {}",
            grammar(kind, multi)
        )),
    }
}

/// Read words for the row a question offers for words.
///
/// The refusals are what the flag is worth having for. A permission box and
/// the trust screen have no such row, and words typed at one land on whatever
/// is highlighted. Neither does a question the vendor draws a preview beside:
/// measured against 2.1.240, that layout has no `Other` row at all, so the
/// `Up` this sends would land on a choice and the words would be typed at the
/// menu itself.
fn field(
    text: &str,
    kind: Option<Kind>,
    pending: Option<&Ask>,
    multi: bool,
) -> Result<Answer, String> {
    if kind != Some(Kind::Question) {
        return Err(format!(
            "this prompt has no row for words of your own. {}",
            grammar(kind, multi)
        ));
    }
    if previewed(pending) {
        return Err(
            "this question draws a preview beside its choices, and that shape has no row \
             for words of your own: answer it with a choice"
                .to_string(),
        );
    }
    if text.trim().is_empty() {
        return Err(
            "words with nothing in them are not an answer: the vendor reads a blank \
             submission as a cancel"
                .to_string(),
        );
    }
    Ok(Answer::Words(text.trim().to_string()))
}

/// Whether the vendor draws this question with a preview beside its choices,
/// which is the shape that has no free-text row.
fn previewed(pending: Option<&Ask>) -> bool {
    pending.is_some_and(Ask::takes_notes)
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
fn steps(answer: &Answer, note: Option<&str>, shape: Shape) -> Vec<Step> {
    let key = |name: &str| Step::Key(name.to_string());
    let mut steps = Vec::new();
    // The note first, and out of its field before the answer is given: from
    // inside it the key that would choose submits the note with no choice at
    // all, which the vendor writes down as `(notes only)`.
    if let Some(note) = note {
        steps.push(key(TO_THE_NOTES));
        steps.push(Step::Type(note.to_string()));
        steps.push(key(OFF_THE_NOTES));
    }
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
        None,
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
    reply(
        agent,
        server,
        pane,
        Answer::Words(words.to_string()),
        None,
        shape,
    )
}

/// Type an answer at the pane, and write down what was answered.
fn reply(
    agent: &Agent,
    server: &Server,
    pane: &PaneId,
    answer: Answer,
    note: Option<&str>,
    shape: Shape,
) -> Result<()> {
    drive(server, pane, &steps(&answer, note, shape))?;
    answered(agent, &answer, note)
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
fn answered(agent: &Agent, answer: &Answer, note: Option<&str>) -> Result<()> {
    let writer = agent.writer()?;
    let said = answer.said(writer.state()?.pending());
    let mut what = answer.event(&said);
    if let Some(note) = note {
        what["note"] = serde_json::json!(note);
    }
    writer.append(&Event::new("answer", what))?;
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

    /// The previewed question of the same measurement: the vendor draws each
    /// choice's `preview` beside it, and that layout carries a notes field and
    /// no free-text row.
    fn a_previewed_question() -> State {
        asking(vec![Ask {
            header: Some("Layout".to_string()),
            text: "Which header layout should the page use?".to_string(),
            options: vec![
                Choice {
                    label: "Stacked".to_string(),
                    description: Some("Title over subtitle".to_string()),
                    preview: Some("+----------+\n| TITLE    |\n+----------+".to_string()),
                },
                Choice {
                    label: "Inline".to_string(),
                    description: Some("Title beside subtitle".to_string()),
                    preview: Some(
                        "+---------------------+\n| TITLE - subtitle    |\n+---------------------+"
                            .to_string(),
                    ),
                },
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

    /// The command line `amx answer <id> <answer>`.
    fn given(answer: &str) -> AnswerArgs {
        AnswerArgs {
            key: Some(answer.to_string()),
            ..AnswerArgs::default()
        }
    }

    /// The command line `amx answer <id> --text <words>`.
    fn given_text(words: &str) -> AnswerArgs {
        AnswerArgs {
            text: Some(words.to_string()),
            ..AnswerArgs::default()
        }
    }

    /// What amx would type at this question for this command line.
    fn typed(state: &State, args: &AnswerArgs) -> Vec<Step> {
        let answer = answer(args, state.kind, state).expect("an answer this question takes");
        steps(&answer, None, Shape::of(state))
    }

    /// The one step that is not a key.
    fn words(text: &str) -> Step {
        Step::Type(text.to_string())
    }

    #[test]
    fn surfaces_a_question_that_takes_several_choices_takes_several() {
        // Measured against 2.1.240: a digit checks a box without moving the
        // cursor and without submitting, `→` leaves the choices for the Submit
        // tab, and the `Enter` after it is the one on the review screen.
        let state = a_checkbox_question();
        assert_eq!(
            answer(&given("1,3"), Some(Kind::Question), &state),
            Ok(Answer::Toggle(vec![1, 3]))
        );
        assert_eq!(
            typed(&state, &given("1,3")),
            keys(&["1", "3", "Right", "Enter"])
        );

        // Space around a choice is a shell's doing, not the caller's meaning.
        assert_eq!(
            typed(&state, &given(" 1 , 3 ")),
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
            answer(&given("1"), Some(Kind::Question), &checkbox),
            Ok(Answer::Toggle(vec![1]))
        );
        assert_eq!(
            typed(&checkbox, &given("1")),
            keys(&["1", "Right", "Enter"])
        );

        let plain = a_plain_question();
        assert_eq!(
            answer(&given("1"), Some(Kind::Question), &plain),
            Ok(Answer::Key("1".to_string()))
        );
        assert_eq!(typed(&plain, &given("1")), keys(&["1"]));
    }

    #[test]
    fn surfaces_a_question_that_takes_one_choice_is_offered_one() {
        // `1,3` at a plain menu would choose the first, submit the tab, and
        // leave the `3` to land on whatever the vendor drew next.
        for state in [a_plain_question(), a_permission_box(), State::default()] {
            let refused = answer(&given("1,3"), state.kind, &state).expect_err("one choice");
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
                answer(&given(refused), Some(Kind::Question), &state).is_err(),
                "{refused} is not a choice this question offers"
            );
        }

        // And checking the same box twice leaves it as it was.
        let refused = answer(&given("1,1"), Some(Kind::Question), &state).expect_err("twice");
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
        assert_eq!(
            typed(&state, &given("1")),
            keys(&["1"]),
            "and the vendor advances"
        );

        state.answered("MIT");
        assert!(Shape::of(&state).confirms, "this one is the last of them");
        assert_eq!(
            typed(&state, &given("1,3")),
            keys(&["1", "3", "Right", "Enter"])
        );

        // A lone plain menu draws no Submit tab at all: the digit submits it,
        // and an `Enter` after that would land in the composer.
        assert!(!Shape::of(&a_plain_question()).confirms);
        assert!(!Shape::of(&a_permission_box()).confirms);
    }

    #[test]
    fn surfaces_words_of_your_own_go_in_the_row_the_question_offers_for_them() {
        // On a plain menu the cursor wraps up onto that row, the words are
        // pasted into it, and `Enter` submits what was typed rather than what
        // was highlighted.
        let plain = a_plain_question();
        assert_eq!(
            answer(&given_text("BSD-3-Clause"), Some(Kind::Question), &plain),
            Ok(Answer::Words("BSD-3-Clause".to_string()))
        );
        assert_eq!(
            typed(&plain, &given_text("BSD-3-Clause")),
            vec![
                Step::Key("Up".to_string()),
                words("BSD-3-Clause"),
                Step::Key("Enter".to_string())
            ]
        );

        // On a checkbox menu that same `Enter` unchecks the row the words just
        // checked, so the cursor leaves the row first and the vendor's own
        // Submit row is what takes it.
        let checkbox = a_checkbox_question();
        assert_eq!(
            typed(&checkbox, &given_text("Audit")),
            vec![
                Step::Key("Up".to_string()),
                words("Audit"),
                Step::Key("Down".to_string()),
                Step::Key("Enter".to_string()),
                Step::Key("Enter".to_string()),
            ]
        );

        // Space around them is a shell's doing, not the caller's meaning, and
        // a blank submission is read by the vendor as a cancel.
        assert_eq!(
            answer(&given_text("  Audit  "), Some(Kind::Question), &checkbox),
            Ok(Answer::Words("Audit".to_string()))
        );
        assert!(answer(&given_text("   "), Some(Kind::Question), &checkbox).is_err());
    }

    #[test]
    fn surfaces_the_row_for_words_reads_a_digit_as_the_character_it_is() {
        // The reason the flag is there. Once the cursor is on that row every
        // key is a character in it, so `--text 2` is the literal "2" the
        // vendor recorded when it was measured, while a bare `2` is the second
        // choice and is submitted the moment it is typed.
        let state = a_plain_question();
        assert_eq!(
            answer(&given_text("2"), Some(Kind::Question), &state),
            Ok(Answer::Words("2".to_string()))
        );
        assert_eq!(
            answer(&given("2"), Some(Kind::Question), &state),
            Ok(Answer::Key("2".to_string()))
        );
    }

    #[test]
    fn surfaces_a_prompt_with_no_row_for_words_is_offered_none() {
        // A permission box and the trust screen read one key: words at either
        // land on whatever is highlighted, which is an answer nobody chose.
        for kind in [None, Some(Kind::Permission), Some(Kind::Trust)] {
            let state = State {
                kind,
                ..a_permission_box()
            };
            let refused = answer(&given_text("keep both"), kind, &state).expect_err("no row");
            assert!(refused.contains("y, n, 1-9"), "{refused}");
        }

        // And neither has a question the vendor draws a preview beside: that
        // layout has no free-text row at all, so the `Up` would land on a
        // choice and the words would be typed at the menu itself.
        let previewed = a_previewed_question();
        for line in [given_text("stacked, please"), given("stacked, please")] {
            let refused = answer(&line, Some(Kind::Question), &previewed).expect_err("no row");
            assert!(
                refused.contains("preview") || refused.contains("1-9"),
                "{refused}"
            );
        }
    }

    #[test]
    fn surfaces_a_note_rides_beside_the_choice_it_is_about() {
        // Measured against 2.1.240: `n` puts the cursor in the notes field,
        // `Escape` leaves it with the note kept rather than cancelling the
        // prompt, and the choice made after that carries the note back beside
        // it in the vendor's own annotations.
        let state = a_previewed_question();
        let line = AnswerArgs {
            key: Some("1".to_string()),
            note: Some("prefer the stacked one".to_string()),
            ..AnswerArgs::default()
        };
        let (answer, note) =
            read(&line, Some(Kind::Question), &state).expect("a note and a choice");
        assert_eq!(answer, Answer::Key("1".to_string()));
        assert_eq!(
            steps(&answer, note.as_deref(), Shape::of(&state)),
            vec![
                Step::Key("n".to_string()),
                words("prefer the stacked one"),
                Step::Key("Escape".to_string()),
                Step::Key("1".to_string()),
            ],
            "the note is typed before the answer that carries it"
        );
    }

    #[test]
    fn surfaces_a_question_that_draws_no_notes_field_takes_no_note() {
        // `n` at a menu with no preview does nothing at all, so the note would
        // be typed at the menu itself and its first digit would answer it.
        for state in [
            a_plain_question(),
            a_checkbox_question(),
            a_permission_box(),
        ] {
            let line = AnswerArgs {
                key: Some("1".to_string()),
                note: Some("prefer the stacked one".to_string()),
                ..AnswerArgs::default()
            };
            let refused = read(&line, state.kind, &state).expect_err("no notes field");
            assert!(refused.contains("preview"), "{refused}");
        }

        // And a note with nothing in it is not a note.
        let blank = AnswerArgs {
            key: Some("1".to_string()),
            note: Some("  ".to_string()),
            ..AnswerArgs::default()
        };
        assert!(read(&blank, Some(Kind::Question), &a_previewed_question()).is_err());
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
            answer(&given("neither, keep both"), Some(Kind::Question), &state),
            Ok(Answer::Words("neither, keep both".to_string()))
        );
        // Space around them is a shell's doing, not the caller's meaning.
        assert_eq!(
            answer(&given("  the sqlite one  "), Some(Kind::Question), &state),
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
                answer(&given("neither, keep both"), kind, &state).is_err(),
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
                    answer(&given(key), Some(Kind::Question), &state),
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
                answer(&given(blank), Some(Kind::Question), &state).is_err(),
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

        answered(&agent, &Answer::Key("2".to_string()), None).unwrap();
        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Waiting, "the prompt is still up");
        assert_eq!(state.asking[0].answer.as_deref(), Some("Apache-2.0"));
        assert_eq!(
            state.question.as_deref(),
            Some("Which features should be enabled?")
        );
        assert!(state.multi());

        answered(&agent, &Answer::Toggle(vec![1, 3]), None).unwrap();
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
            answered(&agent, &Answer::Key(key.to_string()), None).unwrap();

            let state = agent.state().unwrap();
            assert_eq!(state.state, Phase::Working, "{key}");
            assert_eq!(state.question, None, "{key}");
            assert!(state.asking.is_empty(), "{key}");
            assert_eq!(state.kind, None, "{key}");
        }
    }
}
