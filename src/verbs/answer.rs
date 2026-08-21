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

use anyhow::Result;
use std::path::Path;

use crate::derive;
use crate::store::{Agent, Event, Kind, Phase};
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
    let Some(answer) = answer(typed, view.kind()) else {
        eprintln!("amx: `{typed}` is not an answer. {}", grammar(view.kind()));
        return Ok(exit::USAGE);
    };

    let agent = Agent::open(root, id)?;
    let server = Server::from_socket(view.meta.socket.clone());
    match answer {
        Answer::Key(pressed) => press(&agent, &server, &view.meta.pane, &pressed)?,
        Answer::Words(words) => say(&agent, &server, &view.meta.pane, &words)?,
    }
    Ok(exit::OK)
}

/// What was typed at amx, once it is something this question can take.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Answer {
    /// One key of the grammar, under the name tmux knows it by.
    Key(String),
    /// Words of the caller's own, for the question that offers a field.
    Words(String),
}

/// Read what was typed as an answer to this kind of question.
///
/// The grammar comes first wherever it applies: a bare `2` at a menu is the
/// second choice, not a two-character opinion, and that is how every prompt
/// the vendor draws reads it. Words are what is left, and they are an answer
/// only where there is a field to put them in.
fn answer(typed: &str, kind: Option<Kind>) -> Option<Answer> {
    if let Some(key) = named(typed) {
        return Some(Answer::Key(key));
    }
    match kind {
        // Empty is never an answer, and at this prompt it is worse than
        // nothing: the vendor reads a blank submission as a cancel.
        Some(Kind::Question) if !typed.trim().is_empty() => {
            Some(Answer::Words(typed.trim().to_string()))
        }
        _ => None,
    }
}

/// What this question would have taken, for the caller who typed something
/// else.
fn grammar(kind: Option<Kind>) -> &'static str {
    match kind {
        Some(Kind::Question) => "use 1-9, enter, esc, or words of your own",
        _ => "use y, n, 1-9, enter or esc",
    }
}

/// Type one key of the grammar at the agent, and record that it was typed.
///
/// The record is what stops the question being answered twice: the vendor says
/// nothing when a prompt is dismissed, so until its next hook arrives the only
/// thing that knows the question is dealt with is this. The view answers
/// through here for the same reason.
pub fn press(agent: &Agent, server: &Server, pane: &PaneId, pressed: &str) -> Result<()> {
    server.send_keys(pane, &[pressed])?;
    answered(agent, serde_json::json!({ "key": pressed }))
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
pub fn say(agent: &Agent, server: &Server, pane: &PaneId, words: &str) -> Result<()> {
    server.send_keys(pane, &[TO_THE_FIELD])?;
    server.paste(pane, words)?;
    server.send_keys(pane, &["Enter"])?;
    answered(agent, serde_json::json!({ "text": words }))
}

/// Write down that the question was answered, and that it is no longer one.
///
/// The question goes with its choices and its kind: they were the choices
/// under *this* question, and a row still offering them after it has been
/// answered is a row inviting somebody to answer it again.
fn answered(agent: &Agent, what: serde_json::Value) -> Result<()> {
    // The question is answered; the agent is getting on with it. What it is
    // really doing is the next hook's business, and the screen's after that.
    let writer = agent.writer()?;
    writer.append(&Event::new("answer", what))?;
    writer.update_state(|state| {
        state.state = Phase::Working;
        state.asks(None);
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
        assert_eq!(
            answer("neither, keep both", Some(Kind::Question)),
            Some(Answer::Words("neither, keep both".to_string()))
        );
        // Space around them is a shell's doing, not the caller's meaning.
        assert_eq!(
            answer("  the sqlite one  ", Some(Kind::Question)),
            Some(Answer::Words("the sqlite one".to_string()))
        );
    }

    #[test]
    fn surfaces_a_prompt_that_reads_one_key_is_offered_one_key() {
        // A permission box, the trust screen, and a question amx has not been
        // told the kind of. Words at any of them land on whatever is
        // highlighted, which is an answer nobody chose.
        for kind in [None, Some(Kind::Permission), Some(Kind::Trust)] {
            assert_eq!(answer("neither, keep both", kind), None, "{kind:?}");
            assert!(grammar(kind).contains("y, n, 1-9"), "{kind:?}");
        }
    }

    #[test]
    fn surfaces_the_choices_are_still_read_as_choices() {
        // A menu offers a field *and* numbered choices, and `2` at one of them
        // is the second choice — which is how the vendor reads it, and what a
        // caller writing `amx answer <id> 2` means.
        for key in ["2", "y", "enter", "esc"] {
            assert!(
                matches!(answer(key, Some(Kind::Question)), Some(Answer::Key(_))),
                "{key:?}"
            );
        }
    }

    #[test]
    fn surfaces_an_empty_answer_is_not_an_answer_to_a_question_either() {
        // The vendor reads a blank submission at its own menu as a cancel, so
        // this is not a harmless nothing.
        for blank in ["", "   ", "\t\n"] {
            assert_eq!(answer(blank, Some(Kind::Question)), None, "{blank:?}");
        }
        assert!(grammar(Some(Kind::Question)).contains("words of your own"));
    }
}
