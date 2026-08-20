//! `amx answer` — press the key the agent's question is waiting for.
//!
//! The verb is a grammar and a refusal. The grammar is deliberately tiny —
//! `y`, `n`, `1`–`9`, `enter`, `esc` — because those are the keys the vendor's
//! prompts read, and anything wider would be amx inventing an input language
//! for a program it does not control. A word is a message, and `send` is the
//! verb for those.
//!
//! The refusal matters as much. A key typed at an agent that is *not* asking
//! lands in whatever it does next, so "nothing pending" is an answer of its
//! own — exit 2 — and not a quiet success.
//!
//! Answering also clears the question from the record. The vendor says nothing
//! when a prompt is dismissed: the next hook comes when the agent gets to it,
//! which can be a while. Until then a caller reading the record would find the
//! same question still pending and answer it a second time, with the second key
//! landing somewhere nobody chose.

use anyhow::Result;
use std::path::Path;

use crate::derive;
use crate::store::{Agent, Event, Phase};
use crate::tmux::{PaneId, Server};
use crate::verbs::send::nothing_more_is_coming;
use crate::{exit, paths, rules, store};

/// Run the verb against the machine.
pub fn from_env(id: &str, key: &str) -> Result<i32> {
    let root = paths::state_root()?;
    run(&root, id, key)
}

/// The verb, with the state directory named.
pub fn run(root: &Path, id: &str, key: &str) -> Result<i32> {
    // Before anything is opened, let alone typed: a key amx cannot name is a
    // mistake in the command line, and the agent must never see it.
    let Some(pressed) = named(key) else {
        eprintln!("amx: `{key}` is not an answer. use y, n, 1-9, enter or esc");
        return Ok(exit::USAGE);
    };

    let view = derive::view(root, id, rules::bundled(), store::now())?;
    let phase = view.phase();
    if phase.is_terminal() {
        return Ok(nothing_more_is_coming(id, phase));
    }
    if phase != Phase::Waiting {
        eprintln!("amx: {id} has no pending question; nothing to answer");
        return Ok(exit::BLOCKED);
    }

    let agent = Agent::open(root, id)?;
    let server = Server::from_socket(view.meta.socket.clone());
    press(&agent, &server, &view.meta.pane, &pressed)?;
    Ok(exit::OK)
}

/// Type one key of the grammar at the agent, and record that it was typed.
///
/// The record is what stops the question being answered twice: the vendor says
/// nothing when a prompt is dismissed, so until its next hook arrives the only
/// thing that knows the question is dealt with is this. The view answers
/// through here for the same reason.
pub fn press(agent: &Agent, server: &Server, pane: &PaneId, pressed: &str) -> Result<()> {
    server.send_keys(pane, &[pressed])?;

    // The question is answered; the agent is getting on with it. What it is
    // really doing is the next hook's business, and the screen's after that.
    let writer = agent.writer()?;
    writer.append(&Event::new("answer", serde_json::json!({ "key": pressed })))?;
    writer.update_state(|state| {
        state.state = Phase::Working;
        state.question = None;
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
}
