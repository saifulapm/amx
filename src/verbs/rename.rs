//! `amx rename` — call an agent something else.
//!
//! The word on the row and nothing more. The id is what the record is filed
//! under, what a shell addresses, and what the pane, the branch and the tree
//! amx cut are named after, so it is untouched: an id that moved would leave
//! every one of those pointing at a name nothing answers to. What a rename
//! changes is the thing a person reads a hundred times a day.
//!
//! A name a row cannot carry is refused rather than cut down, and refused as a
//! usage error: nothing about the agent came into it, and what is wrong is the
//! word that was typed. The wall's `ctrl+r` asks the same function and puts the
//! same sentence where its keys are, which is why the reading lives here and
//! not in either surface.

use anyhow::Result;
use std::io::Write;
use std::path::Path;

use crate::store::Agent;
use crate::{complain, exit, paths};

/// What a rename came to.
pub enum Renamed {
    /// The wall calls it something else now, and this says what.
    Yes(String),
    /// It is called what it was called, and this says why.
    No(String),
}

/// The longest name a row will carry. Past this the column that holds it cuts,
/// and a name that only reads whole in the line it was typed on is not a name
/// on the wall.
pub const NAME: usize = 24;

/// Run the verb against the machine.
pub fn from_env(id: &str, name: &str) -> Result<i32> {
    let root = paths::state_root()?;
    let mut out = std::io::stdout().lock();
    run(&root, id, name, &mut out)
}

/// The verb, with the state directory named.
///
/// The sentence goes to stdout, where a caller reads what amx did, and a
/// refusal to stderr with `EX_USAGE` behind it: a name no row can carry is a
/// word typed wrong rather than anything the agent did or is doing.
pub fn run(root: &Path, id: &str, name: &str, out: &mut impl Write) -> Result<i32> {
    match rename(root, id, name)? {
        Renamed::Yes(said) => {
            writeln!(out, "{said}")?;
            Ok(exit::OK)
        }
        Renamed::No(why) => {
            complain!("amx: {why}");
            Ok(exit::USAGE)
        }
    }
}

/// Call the agent something else.
///
/// The id is untouched, and a name that is the id is the name it already had:
/// the record goes back to holding none, and the row is a row amx names again.
///
/// What is typed is made safe where it is written down: a name goes on a row,
/// into a notice and back into a line somebody is editing, and a record that
/// never held a control character cannot hand one to any of them.
pub fn rename(root: &Path, id: &str, typed: &str) -> Result<Renamed> {
    // A control character becomes the space it stands in for rather than
    // nothing at all: a name pasted over two lines is two words, and dropping
    // the newline outright would run them into one.
    let spaced: String = typed
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let name = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = name.as_str();
    if name.is_empty() {
        return Ok(Renamed::No("a name is a word, not nothing".to_string()));
    }
    if name.chars().count() > NAME {
        return Ok(Renamed::No(format!(
            "a name is {NAME} characters at most, so that a row can carry it"
        )));
    }

    // Written the way a reading is written rather than as something the agent
    // said: a name is a fact about the wall, and moving the record's own clock
    // for it would have the next reader trust this document over the pane.
    let agent = Agent::open(root, id)?;
    agent
        .writer()?
        .observe(|state| state.name = (name != id).then(|| name.to_string()))?;
    Ok(Renamed::Yes(format!("{id} is {name}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{self, Meta};
    use crate::tmux::{PaneId, Socket};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// An agent with a record under `root`, for the rename to work on.
    fn recorded(root: &Path, id: &str) -> Agent {
        Agent::create(
            root,
            &Meta {
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                agent: None,
                dir: PathBuf::from("/srv/app"),
                worktree: None,
                branch: None,
                base: None,
                socket: Socket::Name("amx-not-a-server".to_string()),
                pane: PaneId::new("%404").unwrap(),
                bg: false,
                session: None,
                transcript: None,
                created: store::now(),
            },
        )
        .unwrap()
    }

    #[test]
    fn acts_rename_puts_the_name_on_the_record_and_leaves_the_id_where_it_was() {
        let root = TempDir::new().unwrap();
        let agent = recorded(root.path(), "fix-login-a1b");

        let Renamed::Yes(said) = rename(root.path(), "fix-login-a1b", "  auth\u{7}  ").unwrap()
        else {
            panic!("the rename was refused")
        };
        assert!(said.contains("auth"), "{said}");
        assert_eq!(
            agent.state().unwrap().name.as_deref(),
            Some("auth"),
            "trimmed, and without the characters a terminal reads as an \
             instruction rather than a letter"
        );
        assert_eq!(
            agent.meta().unwrap().id,
            "fix-login-a1b",
            "the id is what everything else addresses, and a rename is not \
             about the id"
        );

        rename(root.path(), "fix-login-a1b", "auth\nfix").unwrap();
        assert_eq!(
            agent.state().unwrap().name.as_deref(),
            Some("auth fix"),
            "and a name pasted over two lines is the two words it is"
        );
    }

    #[test]
    fn acts_rename_refuses_what_no_row_could_carry() {
        let root = TempDir::new().unwrap();
        let agent = recorded(root.path(), "fix-login-a1b");
        let refused = |typed: &str| match rename(root.path(), "fix-login-a1b", typed).unwrap() {
            Renamed::No(why) => why,
            Renamed::Yes(said) => panic!("{typed:?} was taken: {said}"),
        };

        assert!(refused("   ").contains("a name"), "nothing is not a name");
        assert!(
            refused(&"x".repeat(NAME + 1)).contains(&NAME.to_string()),
            "and one no row can draw whole is refused with the length in it"
        );
        assert_eq!(agent.state().unwrap().name, None, "and nothing was written");

        // A name that is the id is the name it already had, so the record goes
        // back to holding none.
        rename(root.path(), "fix-login-a1b", "auth").unwrap();
        rename(root.path(), "fix-login-a1b", "fix-login-a1b").unwrap();
        assert_eq!(agent.state().unwrap().name, None);
    }

    #[test]
    fn rename_says_what_the_agent_is_called_now() {
        let root = TempDir::new().unwrap();
        let agent = recorded(root.path(), "fix-login-a1b");

        let mut out = Vec::new();
        let code = run(root.path(), "fix-login-a1b", "auth", &mut out).unwrap();

        assert_eq!(code, exit::OK);
        assert_eq!(String::from_utf8(out).unwrap(), "fix-login-a1b is auth\n");
        assert_eq!(agent.state().unwrap().name.as_deref(), Some("auth"));
        assert_eq!(
            agent.meta().unwrap().id,
            "fix-login-a1b",
            "and it is still addressed by the id it was addressed by"
        );
    }

    #[test]
    fn rename_a_name_no_row_could_carry_is_a_usage_refusal() {
        // Nothing about the agent came into it: what is wrong is the word that
        // was typed, which is what `EX_USAGE` says. The reason goes to stderr,
        // so stdout holds nothing a caller would read as a name.
        let root = TempDir::new().unwrap();
        let agent = recorded(root.path(), "fix-login-a1b");

        for typed in ["   ", &"x".repeat(NAME + 1)] {
            let mut out = Vec::new();
            let code = run(root.path(), "fix-login-a1b", typed, &mut out).unwrap();
            assert_eq!(code, exit::USAGE, "{typed:?}");
            assert!(out.is_empty(), "{typed:?}: {out:?}");
        }
        assert_eq!(agent.state().unwrap().name, None, "and nothing was written");
    }

    #[test]
    fn rename_to_the_id_is_the_name_it_already_had() {
        // The row goes back to the word amx names it by, which is the only way
        // a name given at a shell is taken back again.
        let root = TempDir::new().unwrap();
        let agent = recorded(root.path(), "fix-login-a1b");

        let mut out = Vec::new();
        run(root.path(), "fix-login-a1b", "auth", &mut out).unwrap();
        let code = run(root.path(), "fix-login-a1b", "fix-login-a1b", &mut out).unwrap();

        assert_eq!(code, exit::OK);
        assert_eq!(agent.state().unwrap().name, None);
    }
}
