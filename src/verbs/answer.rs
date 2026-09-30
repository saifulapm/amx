//! `amx answer`: type an answer to the question an agent is waiting on.
//!
//! The grammar depends on the screen. A permission box or trust screen takes
//! one key (`y`, `n`, `1`-`9`, `enter`, `esc`). A question the vendor asked
//! itself also takes words of your own. A list with no numbers on it takes a
//! walk (`down enter`), and a list amx numbered itself off the cursor mark
//! takes those digits. The shape of a question (checkbox, several tabs,
//! preview) comes from the record, since a narrow pane elides the tab strip.
//! The measurements are in `docs/question-shapes.md`.
//!
//! - Nothing is typed unless the question can take the answer. An agent with
//!   no pending question exits `BLOCKED`.
//! - Every key goes in its own `send-keys` call with a pause after it; see
//!   [`drive`].
//! - An answer amx can name clears the question from the record, since the
//!   vendor fires no hook when a prompt is dismissed. One it cannot name
//!   (`y`, `enter`, `esc`, a hand-written walk) leaves the record alone,
//!   because the key may have done nothing.

use anyhow::Result;
use std::path::Path;
use std::time::Duration;

use crate::cli::AnswerArgs;
use crate::derive;
use crate::store::{Agent, Ask, Event, Kind, Phase, State};
use crate::tmux::{PaneId, Server};
use crate::verbs::send::{ends_its_own_paste, nothing_more_is_coming};
use crate::{exit, paths, store, warn};

/// Moves a menu's cursor onto the vendor's free-text row.
///
/// claude 2.1.237's `AskUserQuestion` menu ends in one `Other` row and wraps
/// from the first row to the last, so `Up` reaches the field whatever the
/// choices. Counting rows would not: a hook's payload and a pane reading
/// disagree by two rows.
const TO_THE_FIELD: &str = "Up";

/// Leaves the choices of a checkbox question for the Submit tab.
///
/// On claude 2.1.240 every digit and `Enter` there toggles a box, so the only
/// way off the choices is the next tab.
const OFF_THE_CHOICES: &str = "Right";

/// Leaves a checkbox question's free-text row for the `Submit` row under it.
const OFF_THE_FIELD: &str = "Down";

/// Takes the highlighted row: a choice, a checkbox `Submit` row, or
/// `1. Submit answers` on the review screen.
const TAKE_IT: &str = "Enter";

/// Walks a list's cursor one row up.
///
/// pi 0.85.1 lists clamp at both ends, so `rows - 1` of these reach the first
/// row from anywhere. claude 2.1.276's trust list wraps; see [`to_the_row`].
const TO_THE_TOP: &str = "Up";

/// Walks a list's cursor one row down.
const DOWN_A_ROW: &str = "Down";

/// The keys that move the cursor of a list with no numbers on it.
///
/// claude 2.1.259's trust gate draws `❯ No, exit` over `Yes, I trust this
/// folder` with no numbers: `1`, `2` and `y` do nothing, `n` and `Enter` end
/// the agent, and only `Down` reaches the other row (`docs/claude-screens.md`).
/// pi's lists work the same way.
const WALKS: [&str; 2] = [TO_THE_TOP, DOWN_A_ROW];

/// Puts the cursor in the notes field of a previewed question. Elsewhere it
/// does nothing (claude 2.1.240), so a note is refused there.
const TO_THE_NOTES: &str = "n";

/// Leaves the notes field with the note kept.
///
/// From inside the field `Escape` returns to the choices instead of cancelling
/// (claude 2.1.240). Submitting from inside the field would record the answer
/// as `(notes only)`, so amx always leaves it first.
const OFF_THE_NOTES: &str = "Escape";

/// Runs the verb against the real state root.
pub fn from_env(id: &str, typed: &AnswerArgs) -> Result<i32> {
    let root = paths::state_root()?;
    run(&root, id, typed)
}

/// Runs the verb against `root`.
pub fn run(root: &Path, id: &str, typed: &AnswerArgs) -> Result<i32> {
    let view = derive::view(root, id, store::now())?;
    let phase = view.phase();
    if phase.is_terminal() {
        return Ok(nothing_more_is_coming(id, phase));
    }
    if phase != Phase::Waiting {
        warn!("amx: {id} has no pending question; nothing to answer");
        return Ok(exit::BLOCKED);
    }

    let agent = Agent::open(root, id)?;
    let server = Server::from_socket(view.meta.socket.clone());
    match given(&agent, &server, &view, typed)? {
        Answered::Yes => Ok(exit::OK),
        Answered::No(refused) => {
            warn!("amx: {refused}");
            Ok(exit::USAGE)
        }
    }
}

/// The outcome of [`given`].
pub enum Answered {
    /// The keys were typed and the answer recorded.
    Yes,
    /// Refused before anything was typed, with the reason.
    No(String),
}

/// Answers the question the agent in `view` is waiting on.
///
/// Shared by the verb and the view, so a refusal comes back as
/// [`Answered::No`] instead of being printed: the view has no stderr while in
/// raw mode. Nothing is typed unless the question can take the answer.
pub fn given(
    agent: &Agent,
    server: &Server,
    view: &derive::View,
    typed: &AnswerArgs,
) -> Result<Answered> {
    let (answer, note) = match read(typed, view.kind(), &view.state) {
        Ok(read) => read,
        Err(refused) => return Ok(Answered::No(refused)),
    };
    let letters = crate::registry::read_as(view.meta.agent.as_deref().unwrap_or_default())
        .is_none_or(|vendor| vendor.menus_take_letters);
    let answer = match answer {
        Answer::Key(key) if !letters => Answer::Key(row_saying(&key, &view.state).unwrap_or(key)),
        answer => answer,
    };
    // The walk starts from wherever the cursor is now, so read the pane last.
    let answer = match answer {
        Answer::Picked(at, _) => Answer::Picked(
            at,
            to_the_row(at, view.state.options.len(), marked_now(server, view)),
        ),
        answer => answer,
    };
    reply(
        agent,
        server,
        &view.meta.pane,
        &view.state,
        answer,
        note.as_deref(),
        Shape::of(&view.state),
    )?;
    Ok(Answered::Yes)
}

/// A command line parsed into something this question takes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Answer {
    /// One key of the grammar, under its tmux name.
    Key(String),
    /// Boxes to check on a multi-choice question, 1-based, in the order given.
    Toggle(Vec<usize>),
    /// Free text for the question's `Other` row.
    Words(String),
    /// Cursor moves on an unnumbered list, then the key that takes the row.
    Walk(Vec<String>),
    /// A row of a list amx numbered itself (1-based) and the walk to it.
    Picked(usize, Vec<String>),
}

impl Answer {
    /// Whether this answer names the choice it makes.
    ///
    /// A digit, a set of boxes, words, or a digit on a walked list do: the row
    /// they pick is known, so the question is recorded as answered. `y`,
    /// `enter`, `esc` and a hand-written walk do not: their effect on the screen
    /// is unknown, so the record is left for the next hook or reader.
    fn chose(&self) -> bool {
        match self {
            Answer::Key(key) => one_choice(key).is_some(),
            Answer::Walk(_) => false,
            _ => true,
        }
    }

    /// The answer as the vendor would record it: the chosen label, the checked
    /// labels joined with `, `, or the words typed.
    ///
    /// Takes the whole state because a walked list has no `Ask` behind it; its
    /// labels are in `state.options`.
    fn said(&self, state: &State) -> String {
        let pending = state.pending();
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
            Answer::Walk(keys) => keys.join(" "),
            Answer::Picked(at, _) => match state.options.get(at - 1) {
                Some(label) => label.clone(),
                None => at.to_string(),
            },
        }
    }

    /// The `answer` event payload: what was typed and what it came to.
    fn event(&self, said: &str) -> serde_json::Value {
        match self {
            Answer::Key(key) => serde_json::json!({ "key": key }),
            Answer::Toggle(checked) => serde_json::json!({
                "key": checked.iter().map(usize::to_string).collect::<Vec<_>>().join(","),
                "answer": said,
            }),
            Answer::Words(words) => serde_json::json!({ "text": words }),
            Answer::Walk(keys) => serde_json::json!({ "key": keys.join(" ") }),
            // Log the digit, not the walk: the row is what was answered.
            Answer::Picked(at, _) => serde_json::json!({
                "key": at.to_string(),
                "answer": said,
            }),
        }
    }
}

/// How the screen treats an answer to the question showing.
///
/// Read off the record, never the pane: at narrow widths the tab strip elides
/// its headers, so only the payload says how many questions a call holds and
/// which take several choices.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Shape {
    /// The question takes several choices, so a digit toggles a box instead of
    /// answering.
    multi: bool,
    /// This answer completes a call that ends on the vendor's own Submit tab,
    /// which amx must press itself: no rule claims that screen.
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
            // Several questions, or one multi-choice question, get a Submit tab.
            confirms: outstanding == 1 && (state.asking.len() > 1 || state.multi()),
        }
    }
}

/// One step of the sequence that answers a question.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    /// A key, under its tmux name.
    Key(String),
    /// Text pasted into the field that has the cursor.
    Type(String),
}

/// Parses the command line into an answer to this question and an optional
/// note.
fn read(
    args: &AnswerArgs,
    kind: Option<Kind>,
    state: &State,
) -> Result<(Answer, Option<String>), String> {
    // Words and notes are pasted, so refuse any that end the paste early.
    let typed = [&args.key, &args.text, &args.note];
    if typed
        .into_iter()
        .flatten()
        .any(|text| ends_its_own_paste(text))
    {
        return Err("that answer carries the end of a bracketed paste; \
                    what follows it would be typed at the agent rather than pasted"
            .to_string());
    }
    let note = note(args, state.pending())?;
    Ok((answer(args, kind, state)?, note))
}

/// The note to type beside the answer.
///
/// Only a previewed question has a notes field. Elsewhere `n` does nothing and
/// the note would be typed at the menu, where its first digit would answer it.
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

/// Parses the command line into an answer to this question.
///
/// The key grammar wins where it applies: a bare `2` at a menu is the second
/// choice. Words are accepted only where there is a field for them. The same
/// key means different things per shape: a digit answers a plain menu but only
/// toggles a box on a multi-choice one.
fn answer(args: &AnswerArgs, kind: Option<Kind>, state: &State) -> Result<Answer, String> {
    let pending = state.pending();
    let multi = state.multi();

    // `--text` marks the input as words, so it bypasses the key grammar.
    if let Some(text) = &args.text {
        return field(text, kind, state);
    }

    let typed = args.key.as_deref().unwrap_or_default();
    if a_list(typed) {
        return boxes(typed, multi, pending);
    }
    if let Some(walked) = a_walk(typed) {
        return walk(walked);
    }
    if let Some(key) = named(typed) {
        if state.walked {
            return at_a_walked_list(&key, state);
        }
        if unnumbered(kind, state) && key != "Escape" {
            return Err(match key == TAKE_IT {
                true => format!(
                    "`{typed}` takes the row the cursor is on, and this screen numbers none \
                     of its rows for amx to see which that is: walk to the one you mean, \
                     as `down enter`"
                ),
                false => format!(
                    "`{typed}` does nothing to this screen, which numbers none of its rows \
                     and reads no letter: walk to the one you mean, as `down enter`"
                ),
            });
        }
        return match (multi, one_choice(&key)) {
            (true, Some(_)) => boxes(&key, multi, pending),
            (false, Some(at)) => {
                if let Some(ask) = pending {
                    a_choice_of(at, ask)?;
                }
                Ok(Answer::Key(key))
            }
            _ => Ok(Answer::Key(key)),
        };
    }
    match kind {
        // claude reads a blank submission as a cancel.
        Some(Kind::Question) if !typed.trim().is_empty() && !previewed(pending) => {
            Ok(Answer::Words(typed.trim().to_string()))
        }
        _ => Err(format!(
            "`{typed}` is not an answer. {}",
            grammar(kind, state)
        )),
    }
}

/// Parses a key at a list amx numbered itself.
///
/// pi draws every blocking list with an arrow and no numbers, so amx's digits
/// are the whole grammar. On pi 0.85.1 `1`, `2`, `y` and `n` do nothing to the
/// selector and `Enter` takes the row under the cursor, so those are refused.
/// `esc` still cancels.
fn at_a_walked_list(key: &str, state: &State) -> Result<Answer, String> {
    if key == "Escape" {
        return Ok(Answer::Key(key.to_string()));
    }
    let rows = state.options.len();
    match one_choice(key) {
        Some(at) if at <= rows => Ok(Answer::Picked(at, to_the_row(at, rows, None))),
        Some(_) => Err(format!(
            "this screen lists {rows} choices, and `{key}` is not one of them: press {}",
            digits(rows)
        )),
        None if key == TAKE_IT => Err(format!(
            "`enter` takes the row the cursor is on, which is the first row until \
             somebody moves it: press {} for the row you mean",
            digits(rows)
        )),
        None => Err(format!(
            "`{key}` does nothing to this screen, which reads no letter and draws no \
             number of its own: press {} for the row you mean",
            digits(rows)
        )),
    }
}

/// The keys that move the cursor to row `at` of `rows` and take it.
///
/// From a known row `from`, the walk is the difference: claude 2.1.276's trust
/// list wraps at the top (2.1.259 clamped), so walking to the top first could
/// land on `No, exit`. Without a reading of the pane it walks to the top first,
/// which works on lists that clamp, such as pi 0.85.1's.
fn to_the_row(at: usize, rows: usize, from: Option<usize>) -> Vec<String> {
    let moves: Vec<String> = match from {
        Some(from) if from >= at => vec![TO_THE_TOP.to_string(); from - at],
        Some(from) => vec![DOWN_A_ROW.to_string(); at - from],
        None => std::iter::repeat_n(TO_THE_TOP.to_string(), rows.saturating_sub(1))
            .chain(std::iter::repeat_n(
                DOWN_A_ROW.to_string(),
                at.saturating_sub(1),
            ))
            .collect(),
    };
    moves.into_iter().chain([TAKE_IT.to_string()]).collect()
}

/// The row of a walked list the cursor is on now, read off the pane.
///
/// `None` if the pane cannot be read or shows no mark.
fn marked_now(server: &Server, view: &derive::View) -> Option<usize> {
    let screen = server.capture(&view.meta.pane).ok()?;
    crate::rules::of(view.meta.agent.as_deref().unwrap_or_default())
        .asking(&screen)?
        .marked
}

/// The digit range that reaches a row of a list of `rows`, as a usage line
/// writes it.
///
/// Capped at nine, the last key the grammar has. Zero rows means amx counted
/// none, so it offers `1-9`.
pub(crate) fn digits(rows: usize) -> String {
    match rows.min(9) {
        0 => "1-9".to_string(),
        1 => "1".to_string(),
        last => format!("1-{last}"),
    }
}

/// Whether the screen is a trust gate whose rows amx could not number.
///
/// With no rows read, no digit means anything and `Enter` takes the row the
/// vendor opened on, which is `No, exit` on claude 2.1.259 and 2.1.276. A
/// gate whose rows were read comes back walked instead.
pub(crate) fn unnumbered(kind: Option<Kind>, state: &State) -> bool {
    kind == Some(Kind::Trust) && state.options.is_empty()
}

/// The keys of `typed` if it is a walk: one or more moves, then at most one
/// take.
///
/// Anything else is read another way: `esc` alone is a key, a lone `enter`
/// takes the highlighted row, and a sentence containing `up` is words.
fn a_walk(typed: &str) -> Option<Vec<String>> {
    let keys: Vec<String> = typed.split_whitespace().map(named).collect::<Option<_>>()?;
    let (last, moves) = keys.split_last()?;
    let a_move = |key: &String| walks(key);
    match moves.iter().all(a_move) && (a_move(last) || (!moves.is_empty() && last == TAKE_IT)) {
        true => Some(keys),
        false => None,
    }
}

/// Accepts a walk only if it ends by taking a row.
///
/// Moves alone answer nothing, and the take must be in the same answer: on
/// claude 2.1.259's gate the row the cursor opens on is `No, exit`.
fn walk(keys: Vec<String>) -> Result<Answer, String> {
    match keys.last().is_some_and(|key| key == TAKE_IT) {
        true => Ok(Answer::Walk(keys)),
        false => Err(
            "a walk moves the cursor and answers nothing on its own: say what to do at \
             the end of it, as `down enter`"
                .to_string(),
        ),
    }
}

/// Whether `key` moves the cursor.
fn walks(key: &str) -> bool {
    WALKS.contains(&key)
}

/// How many choices the pending question offers, per the record.
///
/// Zero where amx holds no `Ask` for it (a permission box, the trust screen, a
/// question read off the pane), so digits are not checked against it.
fn offered(pending: Option<&Ask>) -> usize {
    pending.map(|ask| ask.options.len()).unwrap_or_default()
}

/// Checks that digit `at` lands on one of the question's own choices.
///
/// claude 2.1.240 numbers two rows past the payload: a free-text row and
/// `Chat about this` (`docs/question-shapes.md`). On a plain menu the
/// free-text row's digit moves the cursor onto the field without answering; on
/// a checkbox menu it submits an empty string. `Chat about this` leaves the
/// question. A previewed question numbers neither row.
fn a_choice_of(at: usize, pending: &Ask) -> Result<(), String> {
    let offered = pending.options.len();
    if at <= offered {
        return Ok(());
    }
    if at == offered + 1 && !pending.takes_notes() {
        return Err(format!(
            "`{at}` is the row the vendor draws for words of your own, and pressing it \
             answers nothing: give words with --text"
        ));
    }
    Err(format!(
        "this question offers {offered} choices, and `{at}` is not one of them"
    ))
}

/// Parses `--text` words for the question's free-text row.
///
/// Refused on a permission box or trust screen, which have no such row, and on
/// a previewed question, which draws no `Other` row (claude 2.1.240): the `Up`
/// would land on a choice and the words would be typed at the menu.
fn field(text: &str, kind: Option<Kind>, state: &State) -> Result<Answer, String> {
    let pending = state.pending();
    if kind != Some(Kind::Question) {
        return Err(format!(
            "this prompt has no row for words of your own. {}",
            grammar(kind, state)
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

/// Whether the question draws a preview beside its choices, the one shape with
/// a notes field and no free-text row.
pub(crate) fn previewed(pending: Option<&Ask>) -> bool {
    pending.is_some_and(Ask::takes_notes)
}

/// Whether `typed` is a comma-separated list of single keys.
///
/// A comma alone is not enough: "neither, keep both" is words.
fn a_list(typed: &str) -> bool {
    typed.contains(',')
        && typed
            .split(',')
            .all(|part| part.trim().chars().count() <= 1)
}

/// Parses the boxes to check on a multi-choice question.
///
/// Refused on a single-choice question, where `1,3` would choose the first,
/// submit, and type `3` at whatever comes next. A box outside the choices, or
/// one named twice (which unchecks it), is refused too.
fn boxes(typed: &str, multi: bool, pending: Option<&Ask>) -> Result<Answer, String> {
    if !multi {
        return Err(format!(
            "`{typed}` checks the boxes of a question that takes several, \
             and this one takes one choice"
        ));
    }
    let mut checked = Vec::new();
    for part in typed.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(format!("`{typed}` has a choice missing between its commas"));
        }
        let Some(at) = one_choice(part) else {
            return Err(format!("`{part}` is not one of this question's choices"));
        };
        if let Some(ask) = pending {
            a_choice_of(at, ask)?;
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

/// The 1-based choice a digit key makes.
fn one_choice(key: &str) -> Option<usize> {
    match key.as_bytes() {
        [digit @ b'1'..=b'9'] => Some(usize::from(digit - b'0')),
        _ => None,
    }
}

/// The keystrokes that put this answer on the question's screen, in order.
///
/// Measured on claude 2.1.240 (`docs/question-shapes.md`). A checkbox question
/// submits nothing by itself, and its free-text row checks itself as it is
/// typed into, so `Enter` there would uncheck it; the field is left with
/// `Down` first.
fn steps(answer: &Answer, note: Option<&str>, shape: Shape) -> Vec<Step> {
    let key = |name: &str| Step::Key(name.to_string());
    let mut steps = Vec::new();
    // Type the note first and leave its field: from inside it, the key that
    // chooses submits the note alone.
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
        // A walk ends in its own take; nothing is appended.
        Answer::Walk(walked) | Answer::Picked(_, walked) => {
            steps.extend(walked.iter().map(|pressed| key(pressed)))
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

/// The answers this question takes, for a refusal to suggest.
///
/// Digits run to the number of choices the question offers, or `1-9` where
/// amx holds no `Ask`. Words are offered wherever the question has a free-text
/// row. An unnumbered list is offered the walk, and a walked list only its
/// digits and `esc`.
fn grammar(kind: Option<Kind>, state: &State) -> String {
    if state.walked {
        return format!("use {} or esc", digits(state.options.len()));
    }
    let pending = state.pending();
    let offered = offered(pending);
    let run = digits(offered);
    let and_words = match previewed(pending) {
        true => "enter or esc",
        false => "enter, esc, or words of your own",
    };
    match (kind, state.multi()) {
        (Some(Kind::Question), true) if offered > 1 => {
            format!("use {run}, 1,{offered} for several, {and_words}")
        }
        (Some(Kind::Question), _) => format!("use {run}, {and_words}"),
        _ if unnumbered(kind, state) => "use down enter, up enter, or esc".to_string(),
        _ => "use y, n, 1-9, enter or esc".to_string(),
    }
}

/// Types an answer at the pane and records it.
///
/// Keys that move the cursor onto a field go before any text, so the menu
/// cannot read a digit in the words as a choice. The record update stops a
/// second caller answering the same question before the next hook arrives.
fn reply(
    agent: &Agent,
    server: &Server,
    pane: &PaneId,
    read: &State,
    answer: Answer,
    note: Option<&str>,
    shape: Shape,
) -> Result<()> {
    drive(&(server, pane), &steps(&answer, note, shape))?;
    answered(agent, read, &answer, note)
}

/// Where a sequence of steps is typed. A trait so tests can see how many
/// `send-keys` calls an answer takes and what each carries.
trait Keyboard {
    /// One `send-keys` call with these keys, in order.
    fn keys(&self, keys: &[&str]) -> Result<()>;
    /// Pastes text into the field that has the cursor.
    fn words(&self, text: &str) -> Result<()>;
    /// Gives the vendor time to redraw before the next key.
    fn settle(&self);
}

impl Keyboard for (&Server, &PaneId) {
    fn keys(&self, keys: &[&str]) -> Result<()> {
        self.0.send_keys(self.1, keys)
    }

    fn words(&self, text: &str) -> Result<()> {
        self.0.paste(self.1, text)
    }

    fn settle(&self) {
        std::thread::sleep(SETTLES);
    }
}

/// The pause between two keys of one answer.
///
/// claude's menu redraws on every key, and a key that arrives early is read
/// against the old screen. On claude 2.1.240, one key per call with a 50ms
/// pause was the only pattern that kept every key (16 of 16 rounds).
const SETTLES: Duration = Duration::from_millis(50);

/// Types a sequence at the pane: one `send-keys` call per key, with
/// [`SETTLES`] between steps.
///
/// Several keys in one call lose keys on claude 2.1.240: `send-keys 1 3`
/// checked neither box, and `Right Enter` sent the `Enter` to the tab it had
/// just left. None of it is reported, so the prompt stays up while the record
/// says it was answered.
fn drive(keyboard: &impl Keyboard, steps: &[Step]) -> Result<()> {
    for (after_the_first, step) in steps.iter().enumerate() {
        if after_the_first > 0 {
            keyboard.settle();
        }
        match step {
            Step::Key(key) => keyboard.keys(&[key.as_str()])?,
            Step::Type(text) => keyboard.words(text)?,
        }
    }
    Ok(())
}

/// Logs the answer and, where amx can name it, marks the question answered.
///
/// In a call of several questions the answer goes on its question and the
/// next one takes the screen; only when none is outstanding does the agent go
/// back to `working`, with the question, its choices and its kind cleared.
///
/// Only the event is written when amx cannot name the answer, or when the
/// record has changed since `read` (a hook may have put up a new question).
/// The record then stays as it was until the next hook or pane reading.
fn answered(agent: &Agent, read: &State, answer: &Answer, note: Option<&str>) -> Result<()> {
    let writer = agent.writer()?;
    let said = answer.said(read);
    let mut what = answer.event(&said);
    if let Some(note) = note {
        what["note"] = serde_json::json!(note);
    }
    writer.append(&Event::new("answer", what))?;
    if !answer.chose() || writer.state()?.last_event != read.last_event {
        return Ok(());
    }
    writer.update_state(|state| {
        // A no-op without an `Ask`; the clearing below covers that case.
        state.answered(said);
        // Nothing left outstanding: the next hook says what the agent does.
        if state.pending().is_none() {
            state.state = Phase::Working;
            state.asks(None);
        }
    })?;
    Ok(())
}

/// `y` or `n` as the number of the menu row whose first word says it, for a
/// vendor whose menus ignore letters.
fn row_saying(key: &str, state: &State) -> Option<String> {
    let word = match key {
        "y" => "yes",
        "n" => "no",
        _ => return None,
    };
    let at = state.options.iter().position(|label| {
        label
            .split(|c: char| !c.is_alphanumeric())
            .next()
            .is_some_and(|first| first.eq_ignore_ascii_case(word))
    })?;
    (at < 9).then(|| (at + 1).to_string())
}

/// One key of the grammar under its tmux name, or `None`.
///
/// Case and surrounding space are ignored. `enter`, `esc`, `up` and `down`
/// map to tmux key names so they are not typed as words.
pub fn named(key: &str) -> Option<String> {
    let key = key.trim().to_ascii_lowercase();
    match key.as_str() {
        "y" | "n" => Some(key),
        "enter" => Some("Enter".to_string()),
        "esc" => Some("Escape".to_string()),
        "up" => Some("Up".to_string()),
        "down" => Some("Down".to_string()),
        digit if matches!(digit.as_bytes(), [b'1'..=b'9']) => Some(key),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Choice;

    #[test]
    fn y_and_n_name_the_rows_that_say_yes_and_no() {
        // claude 2.1.284's permission menu, which ignores the letters.
        let state = State {
            options: [
                "Yes",
                "Yes, and don't ask again for: npm test *",
                "Yes, and switch to auto mode",
                "No",
            ]
            .map(str::to_string)
            .to_vec(),
            ..State::default()
        };
        assert_eq!(row_saying("y", &state).as_deref(), Some("1"));
        assert_eq!(row_saying("n", &state).as_deref(), Some("4"));
        assert_eq!(row_saying("2", &state), None, "only the letters move");
        assert_eq!(row_saying("y", &State::default()), None, "no rows read");
    }

    /// A choice with a description.
    fn choice(label: &str, description: &str) -> Choice {
        Choice {
            label: label.to_string(),
            description: Some(description.to_string()),
            preview: None,
        }
    }

    /// The record of a call whose hook carried the full payload.
    fn asking(questions: Vec<Ask>) -> State {
        let mut state = State {
            state: Phase::Waiting,
            kind: Some(Kind::Question),
            ..State::default()
        };
        state.asks_all(questions);
        state
    }

    /// The checkbox question from `docs/question-shapes.md` (claude 2.1.240):
    /// three choices, any number of them taken.
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

    /// The plain menu from the same measurement: one choice, submitted at once.
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

    /// The previewed question from the same measurement: a notes field and no
    /// free-text row.
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

    /// claude 2.1.259's trust gate as a reader records it: the question and no
    /// choices, since neither row is numbered.
    fn a_trust_gate() -> State {
        State {
            state: Phase::Waiting,
            question: Some(
                "Quick safety check: Is this a project you created or one you trust?".to_string(),
            ),
            kind: Some(Kind::Trust),
            ..State::default()
        }
    }

    /// A screen whose choices a reader read off the cursor arrow.
    fn walked(question: &str, options: &[&str], kind: Kind) -> State {
        State {
            state: Phase::Waiting,
            question: Some(question.to_string()),
            options: options.iter().map(|row| row.to_string()).collect(),
            walked: true,
            kind: Some(kind),
            ..State::default()
        }
    }

    /// pi 0.85.1's trust gate once read: five rows numbered by amx.
    fn a_walked_trust_gate() -> State {
        walked(
            "Trust project folder? /home/saiful/Sites/tries/pi-src",
            &[
                "Trust",
                "Trust parent folder (/home/saiful/Sites/tries)",
                "Trust (this session only)",
                "Do not trust",
                "Do not trust (this session only)",
            ],
            Kind::Trust,
        )
    }

    /// pi's tool gate, read the same way by its `dialog` rule.
    fn a_walked_dialog() -> State {
        walked(
            "Allow pi to run `rm -rf build`?",
            &["Allow once", "Allow always", "Deny"],
            Kind::Question,
        )
    }

    /// A permission box: no `Ask` behind it, answered with one key.
    fn a_permission_box() -> State {
        State {
            state: Phase::Waiting,
            question: Some("Claude needs your permission to use Bash".to_string()),
            options: vec!["Yes".to_string(), "No".to_string()],
            kind: Some(Kind::Permission),
            ..State::default()
        }
    }

    /// Key steps, in order.
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

    /// What amx types at this question for this command line.
    fn typed(state: &State, args: &AnswerArgs) -> Vec<Step> {
        let answer = answer(args, state.kind, state).expect("an answer this question takes");
        steps(&answer, None, Shape::of(state))
    }

    /// A paste step.
    fn words(text: &str) -> Step {
        Step::Type(text.to_string())
    }

    /// A keyboard that records each call it receives.
    #[derive(Default)]
    struct Typed(std::cell::RefCell<Vec<Vec<String>>>);

    impl Keyboard for Typed {
        fn keys(&self, keys: &[&str]) -> Result<()> {
            self.0
                .borrow_mut()
                .push(keys.iter().map(|key| key.to_string()).collect());
            Ok(())
        }

        fn words(&self, text: &str) -> Result<()> {
            self.0.borrow_mut().push(vec![format!("paste {text}")]);
            Ok(())
        }

        fn settle(&self) {
            self.0.borrow_mut().push(vec!["settle".to_string()]);
        }
    }

    #[test]
    fn surfaces_every_key_of_an_answer_is_a_call_of_its_own() {
        // On claude 2.1.240, `send-keys 1 3` checked neither box; two calls
        // checked both.
        let checkbox = a_checkbox_question();
        let typist = Typed::default();
        drive(&typist, &typed(&checkbox, &given("1,3"))).unwrap();
        assert_eq!(
            *typist.0.borrow(),
            vec![
                vec!["1"],
                vec!["settle"],
                vec!["3"],
                vec!["settle"],
                vec!["Right"],
                vec!["settle"],
                vec!["Enter"],
            ],
        );

        // Words go in as a paste with a key on each side.
        let typist = Typed::default();
        drive(&typist, &typed(&checkbox, &given_text("Audit"))).unwrap();
        assert_eq!(
            *typist.0.borrow(),
            vec![
                vec!["Up"],
                vec!["settle"],
                vec!["paste Audit"],
                vec!["settle"],
                vec!["Down"],
                vec!["settle"],
                vec!["Enter"],
                vec!["settle"],
                vec!["Enter"],
            ],
        );
    }

    #[test]
    fn surfaces_the_rows_the_vendor_adds_are_not_the_questions_choices() {
        // claude 2.1.240 draws `3. Type something.` and `4. Chat about this`
        // under a two-choice question, and `3` moves onto the field without
        // answering.
        let plain = a_plain_question();
        let refused = answer(&given("3"), Some(Kind::Question), &plain).expect_err("not a choice");
        assert!(refused.contains("--text"), "{refused}");
        let refused = answer(&given("4"), Some(Kind::Question), &plain).expect_err("not a choice");
        assert!(refused.contains("offers 2 choices"), "{refused}");

        // On a checkbox menu the same digit checks the empty field.
        let checkbox = a_checkbox_question();
        let refused = answer(&given("4"), Some(Kind::Question), &checkbox).expect_err("the field");
        assert!(refused.contains("--text"), "{refused}");

        // A previewed question has neither row.
        let previewed = a_previewed_question();
        let refused = answer(&given("3"), Some(Kind::Question), &previewed).expect_err("no row");
        assert!(refused.contains("offers 2 choices"), "{refused}");

        // A permission box has no `Ask` to check digits against.
        assert_eq!(
            answer(&given("3"), Some(Kind::Permission), &a_permission_box()),
            Ok(Answer::Key("3".to_string()))
        );
    }

    #[test]
    fn surfaces_the_grammar_counts_the_choices_the_question_offers() {
        // The offer must not include a digit that was just refused.
        assert_eq!(
            grammar(Some(Kind::Question), &a_plain_question()),
            "use 1-2, enter, esc, or words of your own"
        );
        assert!(
            grammar(Some(Kind::Question), &a_checkbox_question()).contains("1,3"),
            "a question that takes several says how to give it several"
        );
        assert_eq!(
            grammar(Some(Kind::Question), &a_previewed_question()),
            "use 1-2, enter or esc",
            "a previewed question has no row for words of your own"
        );

        // Without an `Ask` the row count is unknown.
        assert!(grammar(Some(Kind::Permission), &a_permission_box()).contains("y, n, 1-9"));
        assert!(grammar(Some(Kind::Question), &State::default()).contains("1-9"));

        // An unnumbered list is offered only the walk.
        assert_eq!(
            grammar(Some(Kind::Trust), &a_trust_gate()),
            "use down enter, up enter, or esc"
        );
    }

    #[test]
    fn surfaces_a_question_that_takes_several_choices_takes_several() {
        // claude 2.1.240: a digit toggles a box, `Right` moves to the Submit
        // tab, and `Enter` confirms there.
        let state = a_checkbox_question();
        assert_eq!(
            answer(&given("1,3"), Some(Kind::Question), &state),
            Ok(Answer::Toggle(vec![1, 3]))
        );
        assert_eq!(
            typed(&state, &given("1,3")),
            keys(&["1", "3", "Right", "Enter"])
        );

        // Surrounding space is ignored.
        assert_eq!(
            typed(&state, &given(" 1 , 3 ")),
            keys(&["1", "3", "Right", "Enter"])
        );
    }

    #[test]
    fn surfaces_one_choice_of_a_checkbox_menu_is_still_a_box() {
        // `1` answers a plain menu but only toggles a box on a checkbox one.
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
        // `1,3` at a plain menu would answer with `1` and type `3` at whatever
        // comes next.
        for state in [a_plain_question(), a_permission_box(), State::default()] {
            let refused = answer(&given("1,3"), state.kind, &state).expect_err("one choice");
            assert!(refused.contains("one choice"), "{refused}");
        }
    }

    #[test]
    fn surfaces_a_box_the_question_does_not_offer_is_not_a_box() {
        // Digits past the choices land on the vendor's own rows.
        let state = a_checkbox_question();
        for refused in ["1,4", "1,9", "0,1"] {
            assert!(
                answer(&given(refused), Some(Kind::Question), &state).is_err(),
                "{refused} is not a choice this question offers"
            );
        }

        // Checking a box twice unchecks it.
        let refused = answer(&given("1,1"), Some(Kind::Question), &state).expect_err("twice");
        assert!(refused.contains("twice"), "{refused}");
    }

    #[test]
    fn surfaces_the_answer_that_finishes_a_call_confirms_it() {
        // A call of several questions ends on a Submit tab nothing else presses.
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

        // A lone plain menu has no Submit tab; an extra `Enter` would reach the
        // composer.
        assert!(!Shape::of(&a_plain_question()).confirms);
        assert!(!Shape::of(&a_permission_box()).confirms);
    }

    #[test]
    fn surfaces_words_of_your_own_go_in_the_row_the_question_offers_for_them() {
        // On a plain menu `Up` wraps to the field, the words are pasted, and
        // `Enter` submits them.
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

        // On a checkbox menu `Enter` on the field would uncheck it, so the
        // cursor moves to the Submit row first.
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

        // Surrounding space is trimmed, and blank words are refused: claude
        // reads them as a cancel.
        assert_eq!(
            answer(&given_text("  Audit  "), Some(Kind::Question), &checkbox),
            Ok(Answer::Words("Audit".to_string()))
        );
        assert!(answer(&given_text("   "), Some(Kind::Question), &checkbox).is_err());
    }

    #[test]
    fn surfaces_the_row_for_words_reads_a_digit_as_the_character_it_is() {
        // Once the cursor is on the field every key is a character, so
        // `--text 2` is the text "2" while a bare `2` is the second choice.
        let state = a_plain_question();
        assert_eq!(
            answer(&given_text("2"), Some(Kind::Question), &state),
            Ok(Answer::Words("2".to_string()))
        );
        assert_eq!(
            answer(&given("2"), Some(Kind::Question), &state),
            Ok(Answer::Key("2".to_string()))
        );

        // Likewise `--text down enter` is words, not a walk.
        assert_eq!(
            answer(&given_text("down enter"), Some(Kind::Question), &state),
            Ok(Answer::Words("down enter".to_string()))
        );
    }

    #[test]
    fn surfaces_a_prompt_with_no_row_for_words_is_offered_none() {
        // A permission box and a trust screen have no field for words.
        for kind in [None, Some(Kind::Permission), Some(Kind::Trust)] {
            let state = State {
                kind,
                ..a_permission_box()
            };
            let refused = answer(&given_text("keep both"), kind, &state).expect_err("no row");
            assert!(refused.contains("y, n, 1-9"), "{refused}");
        }

        // Nor does a previewed question: `Up` would land on a choice.
        let previewed = a_previewed_question();
        let refused =
            answer(&given_text("stacked"), Some(Kind::Question), &previewed).expect_err("no row");
        assert!(refused.contains("preview"), "{refused}");

        // Without the flag the grammar refuses them and offers no words.
        let refused = answer(&given("stacked, please"), Some(Kind::Question), &previewed)
            .expect_err("no row");
        assert!(!refused.contains("words of your own"), "{refused}");
    }

    #[test]
    fn surfaces_a_note_rides_beside_the_choice_it_is_about() {
        // claude 2.1.240: `n` enters the notes field, `Escape` leaves it with
        // the note kept, and the choice after that carries the note.
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
        // Without a notes field the note would be typed at the menu.
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

        // A blank note is refused.
        let blank = AnswerArgs {
            key: Some("1".to_string()),
            note: Some("  ".to_string()),
            ..AnswerArgs::default()
        };
        assert!(read(&blank, Some(Kind::Question), &a_previewed_question()).is_err());
    }

    #[test]
    fn surfaces_the_record_names_the_choices_that_were_checked() {
        // The record gets the checked labels joined with a comma, as the
        // vendor writes them.
        let state = a_checkbox_question();
        assert_eq!(Answer::Toggle(vec![1, 3]).said(&state), "Logging, Tracing");
        assert_eq!(Answer::Key("2".to_string()).said(&state), "Metrics");
        assert_eq!(
            Answer::Words("audit".to_string()).said(&state),
            "audit",
            "and words of your own are their own answer"
        );

        // A walked list's labels come from `state.options`.
        let dialog = a_walked_dialog();
        assert_eq!(
            Answer::Picked(2, to_the_row(2, 3, None)).said(&dialog),
            "Allow always"
        );

        // With no choices recorded, the key is the answer.
        assert_eq!(Answer::Key("y".to_string()).said(&State::default()), "y");
    }

    #[test]
    fn the_grammar_is_y_n_one_through_nine_the_two_moves_enter_and_esc() {
        for key in ["y", "n", "1", "5", "9"] {
            assert_eq!(named(key).as_deref(), Some(key));
        }
        assert_eq!(named("enter").as_deref(), Some("Enter"));
        assert_eq!(named("esc").as_deref(), Some("Escape"));
        assert_eq!(named("down").as_deref(), Some("Down"));
        assert_eq!(named("up").as_deref(), Some("Up"));
    }

    #[test]
    fn surfaces_a_list_with_no_numbers_is_answered_by_walking_to_the_row() {
        // claude 2.1.259's gate: `1`, `2` and `y` do nothing, and only `Down`
        // then `Enter` reaches the second row (`docs/claude-screens.md`).
        let gate = a_trust_gate();
        assert_eq!(
            answer(&given("down enter"), Some(Kind::Trust), &gate),
            Ok(Answer::Walk(vec!["Down".to_string(), "Enter".to_string()]))
        );
        assert_eq!(typed(&gate, &given("down enter")), keys(&["Down", "Enter"]));

        // Longer lists take as many moves as needed.
        assert_eq!(
            typed(&gate, &given("down down enter")),
            keys(&["Down", "Down", "Enter"])
        );
        assert_eq!(typed(&gate, &given("up enter")), keys(&["Up", "Enter"]));

        // Each key is its own call with a pause after it.
        let typist = Typed::default();
        drive(&typist, &typed(&gate, &given("down enter"))).unwrap();
        assert_eq!(
            *typist.0.borrow(),
            vec![vec!["Down"], vec!["settle"], vec!["Enter"]],
        );
    }

    #[test]
    fn surfaces_a_walk_starts_from_the_row_the_cursor_is_on() {
        // claude 2.1.276's trust list wraps, so walking up from `Yes` lands on
        // `No, exit`. From a known row the walk is the difference.
        let walk = |keys: &[&str]| -> Vec<String> { keys.iter().map(|k| k.to_string()).collect() };
        assert_eq!(to_the_row(2, 2, Some(1)), walk(&["Down", "Enter"]));
        assert_eq!(to_the_row(2, 2, Some(2)), walk(&["Enter"]));
        assert_eq!(to_the_row(1, 2, Some(2)), walk(&["Up", "Enter"]));
        assert_eq!(to_the_row(4, 5, Some(2)), walk(&["Down", "Down", "Enter"]));
        assert_eq!(
            to_the_row(1, 5, Some(4)),
            walk(&["Up", "Up", "Up", "Enter"])
        );
        // With no reading of the pane: to the top first, for a list that clamps.
        assert_eq!(to_the_row(2, 3, None), walk(&["Up", "Up", "Down", "Enter"]));
    }

    #[test]
    fn surfaces_a_walk_that_takes_nothing_is_not_an_answer() {
        // A move alone leaves the prompt up while the record would say it was
        // answered.
        let gate = a_trust_gate();
        for walked in ["down", "up", "down down"] {
            let refused = answer(&given(walked), Some(Kind::Trust), &gate).expect_err("no take");
            assert!(refused.contains("down enter"), "{refused}");
        }

        // A walk is moves then one take; anything else is not a walk.
        assert_eq!(a_walk("enter down"), None);
        assert_eq!(a_walk("down enter enter"), None);
        assert_eq!(a_walk("down esc"), None);
        assert_eq!(a_walk("hold on, up to you"), None);
    }

    #[test]
    fn surfaces_the_key_that_takes_the_highlighted_row_is_refused_where_none_is_numbered() {
        // claude 2.1.259 opens the gate on `No, exit`, and with no numbered
        // rows amx cannot tell which row `enter` would take.
        let gate = a_trust_gate();
        let refused = answer(&given("enter"), Some(Kind::Trust), &gate).expect_err("the default");
        assert!(refused.contains("down enter"), "{refused}");

        // Letters and digits do nothing to the gate (claude 2.1.259, and pi
        // 0.85.1's trust selector is the same kind), so the walk is offered
        // instead. `esc` still cancels.
        for swallowed in ["y", "n", "1", "2"] {
            let refused = answer(&given(swallowed), Some(Kind::Trust), &gate).expect_err(swallowed);
            assert!(refused.contains("down enter"), "{swallowed}: {refused}");
        }
        assert_eq!(
            answer(&given("esc"), Some(Kind::Trust), &gate),
            Ok(Answer::Key("Escape".to_string()))
        );

        // Where the rows are numbered, `enter` takes the highlighted default.
        assert_eq!(
            answer(&given("enter"), Some(Kind::Permission), &a_permission_box()),
            Ok(Answer::Key("Enter".to_string()))
        );
        let numbered = State {
            kind: Some(Kind::Trust),
            ..a_permission_box()
        };
        assert_eq!(
            answer(&given("enter"), Some(Kind::Trust), &numbered),
            Ok(Answer::Key("Enter".to_string())),
            "including the trust screen of a vendor that still numbers its rows"
        );
    }

    #[test]
    fn surfaces_claudes_own_gate_takes_a_digit_now_its_rows_are_read() {
        // Once the rule reads both rows off the cursor glyph (`No, exit` first,
        // as 2.1.259 and 2.1.276 draw them), a digit becomes the walk to that
        // row.
        let gate = walked(
            "Quick safety check: Is this a project you created or one you trust?",
            &["No, exit", "Yes, I trust this folder"],
            Kind::Trust,
        );
        assert_eq!(typed(&gate, &given("1")), keys(&["Up", "Enter"]));
        assert_eq!(
            typed(&gate, &given("2")),
            keys(&["Up", "Down", "Enter"]),
            "the row that trusts the folder, from wherever the cursor was"
        );

        // A hand-written walk is still read as one.
        assert_eq!(typed(&gate, &given("down enter")), keys(&["Down", "Enter"]));

        // Keys the screen swallows are refused, including `enter`: the cursor
        // opens on the exit.
        for swallowed in ["enter", "y", "n", "3"] {
            let refused = answer(&given(swallowed), Some(Kind::Trust), &gate).expect_err(swallowed);
            assert!(refused.contains("press 1-2"), "{swallowed}: {refused}");
        }
        assert_eq!(
            answer(&given("esc"), Some(Kind::Trust), &gate),
            Ok(Answer::Key("Escape".to_string()))
        );
    }

    #[test]
    fn surfaces_a_digit_on_a_walked_list_is_the_walk_that_reaches_its_row() {
        // pi 0.85.1's selector clamps at both ends, so `rows - 1` ups reach the
        // top from anywhere and amx only needs the row count.
        let gate = a_walked_trust_gate();
        assert_eq!(
            typed(&gate, &given("1")),
            keys(&["Up", "Up", "Up", "Up", "Enter"]),
            "the first of five rows, from wherever the cursor was"
        );
        assert_eq!(
            typed(&gate, &given("5")),
            keys(&[
                "Up", "Up", "Up", "Up", "Down", "Down", "Down", "Down", "Enter"
            ])
        );

        let dialog = a_walked_dialog();
        assert_eq!(
            typed(&dialog, &given("3")),
            keys(&["Up", "Up", "Down", "Down", "Enter"])
        );

        // Each key is its own call with a pause after it.
        let typist = Typed::default();
        drive(&typist, &typed(&dialog, &given("1"))).unwrap();
        assert_eq!(
            *typist.0.borrow(),
            vec![
                vec!["Up"],
                vec!["settle"],
                vec!["Up"],
                vec!["settle"],
                vec!["Enter"],
            ],
        );
    }

    #[test]
    fn surfaces_a_digit_on_a_walked_list_records_the_row_it_chose() {
        // A numbered row is a known row: the record gets its label and the
        // question is cleared.
        let root = tempfile::TempDir::new().unwrap();
        let dialog = a_walked_dialog();
        let agent = recorded(root.path(), &dialog);
        let picked = answer(&given("3"), dialog.kind, &dialog).expect("the third row");
        answered(&agent, &agent.state().unwrap(), &picked, None).unwrap();

        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Working);
        assert_eq!(state.question, None);
        assert!(state.options.is_empty(), "{:?}", state.options);
        assert!(!state.walked, "and nothing is left claiming a walk");

        let event = &agent.events().unwrap()[0].payload;
        assert_eq!(event["key"], "3");
        assert_eq!(event["answer"], "Deny");
    }

    #[test]
    fn surfaces_the_keys_a_walked_list_swallows_are_refused_for_the_digits_amx_wrote() {
        // pi 0.85.1: `1`, `2`, `y` and `n` do nothing to the selector, and
        // `Enter` takes the row under the cursor. A digit past the rows is
        // nobody's row.
        let gate = a_walked_trust_gate();
        for swallowed in ["y", "n", "enter", "6"] {
            let refused = answer(&given(swallowed), gate.kind, &gate).expect_err(swallowed);
            assert!(refused.contains("press 1-5"), "{swallowed}: {refused}");
        }

        // `esc` still cancels, and a hand-written walk is still accepted.
        assert_eq!(
            answer(&given("esc"), gate.kind, &gate),
            Ok(Answer::Key("Escape".to_string()))
        );
        assert_eq!(
            answer(&given("down enter"), gate.kind, &gate),
            Ok(Answer::Walk(vec!["Down".to_string(), "Enter".to_string()]))
        );

        // The grammar offers exactly what is accepted.
        assert_eq!(grammar(gate.kind, &gate), "use 1-5 or esc");
        let dialog = a_walked_dialog();
        assert_eq!(grammar(dialog.kind, &dialog), "use 1-3 or esc");
    }

    #[test]
    fn surfaces_a_walk_leaves_no_answer_amx_cannot_name_behind_it() {
        // The row a walk lands on has no number and the record has no choices
        // for it, so only the keys are logged and the question stands.
        let root = tempfile::TempDir::new().unwrap();
        let gate = a_trust_gate();
        let agent = recorded(root.path(), &gate);
        let walked = Answer::Walk(vec!["Down".to_string(), "Enter".to_string()]);
        answered(&agent, &agent.state().unwrap(), &walked, None).unwrap();

        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Waiting);
        assert_eq!(state.question, gate.question);
        assert_eq!(state.kind, Some(Kind::Trust));
        assert_eq!(agent.events().unwrap()[0].payload["key"], "Down Enter");
    }

    #[test]
    fn a_shouted_key_is_the_same_key() {
        assert_eq!(named("Y").as_deref(), Some("y"));
        assert_eq!(named("ESC").as_deref(), Some("Escape"));
        assert_eq!(named(" 2 ").as_deref(), Some("2"));
    }

    #[test]
    fn nothing_else_is_an_answer() {
        // `0` is not a choice and a word is a message; both are refused before
        // anything is typed.
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
        // Surrounding space is trimmed.
        assert_eq!(
            answer(&given("  the sqlite one  "), Some(Kind::Question), &state),
            Ok(Answer::Words("the sqlite one".to_string()))
        );
    }

    #[test]
    fn surfaces_a_prompt_that_reads_one_key_is_offered_one_key() {
        // Words at a one-key prompt would land on the highlighted row.
        for kind in [None, Some(Kind::Permission), Some(Kind::Trust)] {
            let state = State {
                kind,
                ..a_permission_box()
            };
            assert!(
                answer(&given("neither, keep both"), kind, &state).is_err(),
                "{kind:?}"
            );
            assert!(grammar(kind, &state).contains("y, n, 1-9"), "{kind:?}");
        }
    }

    #[test]
    fn surfaces_the_choices_are_still_read_as_choices() {
        // A menu takes a field and numbered choices; `2` is the second choice.
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
        // claude reads a blank submission at its menu as a cancel.
        let state = a_plain_question();
        for blank in ["", "   ", "\t\n"] {
            assert!(
                answer(&given(blank), Some(Kind::Question), &state).is_err(),
                "{blank:?}"
            );
        }
        assert!(grammar(Some(Kind::Question), &state).contains("words of your own"));
    }

    #[test]
    fn hardening_an_answer_may_not_end_its_own_paste() {
        // Words and notes are pasted, so a paste terminator in them is refused.
        let end = "fine\u{1b}[201~/exit\r";
        let plain = a_plain_question();
        for line in [given(end), given_text(end)] {
            let refused = read(&line, Some(Kind::Question), &plain).unwrap_err();
            assert!(refused.contains("paste"), "{refused}");
        }

        let previewed = a_previewed_question();
        let noted = |note: &str| AnswerArgs {
            key: Some("1".to_string()),
            note: Some(note.to_string()),
            ..AnswerArgs::default()
        };
        let refused = read(&noted(end), Some(Kind::Question), &previewed).unwrap_err();
        assert!(refused.contains("paste"), "{refused}");
        assert!(read(&noted("fine"), Some(Kind::Question), &previewed).is_ok());
    }

    /// An agent with a record and no pane, holding `asking`.
    fn recorded(root: &Path, asking: &State) -> Agent {
        let meta = crate::store::Meta {
            role: None,
            parent: None,
            depth: 0,
            id: "pick-a1b".to_string(),
            task: "port the importer".to_string(),
            agent: None,
            model: None,
            effort: None,
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
        // claude 2.1.240: answering one tab leaves the prompt up until every
        // tab is answered, so the record must stay `waiting`.
        let call = asking(vec![
            a_plain_question().asking[0].clone(),
            a_checkbox_question().asking[0].clone(),
        ]);
        let root = tempfile::TempDir::new().unwrap();
        let agent = recorded(root.path(), &call);

        answered(
            &agent,
            &agent.state().unwrap(),
            &Answer::Key("2".to_string()),
            None,
        )
        .unwrap();
        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Waiting, "the prompt is still up");
        assert_eq!(state.asking[0].answer.as_deref(), Some("Apache-2.0"));
        assert_eq!(
            state.question.as_deref(),
            Some("Which features should be enabled?")
        );
        assert!(state.multi());

        answered(
            &agent,
            &agent.state().unwrap(),
            &Answer::Toggle(vec![1, 3]),
            None,
        )
        .unwrap();
        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Working, "and now it is over");
        assert_eq!(state.question, None);
        assert!(state.asking.is_empty(), "and it leaves nothing behind");

        // The labels the vendor's answer map would hold.
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
    fn surfaces_a_key_amx_cannot_name_the_answer_of_settles_nothing() {
        // `esc` and `enter` may or may not have changed the screen, so the
        // record keeps the question.
        for key in ["Escape", "Enter", "y"] {
            let root = tempfile::TempDir::new().unwrap();
            let call = asking(vec![
                a_plain_question().asking[0].clone(),
                a_checkbox_question().asking[0].clone(),
            ]);
            let agent = recorded(root.path(), &call);
            answered(
                &agent,
                &agent.state().unwrap(),
                &Answer::Key(key.to_string()),
                None,
            )
            .unwrap();

            let state = agent.state().unwrap();
            assert_eq!(state.state, Phase::Waiting, "{key}");
            assert_eq!(state.question, call.question, "{key}");
            assert_eq!(state.asking, call.asking, "{key}");
            assert_eq!(state.kind, Some(Kind::Question), "{key}");
            // What was typed is logged; its effect is not recorded.
            assert_eq!(agent.events().unwrap()[0].payload["key"], key, "{key}");
        }
    }

    #[test]
    fn surfaces_a_question_replaced_since_it_was_read_stays_on_the_record() {
        // A hook put up a new question between the reading and the write, so
        // the record keeps the new one.
        let root = tempfile::TempDir::new().unwrap();
        let agent = recorded(root.path(), &a_plain_question());
        let read = agent.state().unwrap();

        let mut replaced = a_checkbox_question();
        replaced.last_event = read.last_event + 1;
        std::fs::write(
            agent.dir().join("state.json"),
            serde_json::to_string(&replaced).unwrap(),
        )
        .unwrap();

        answered(&agent, &read, &Answer::Key("2".to_string()), None).unwrap();
        assert_eq!(agent.state().unwrap(), replaced);
        let logged = agent.events().unwrap();
        assert_eq!(logged[0].payload["key"], "2", "what was typed is logged");
    }
}
