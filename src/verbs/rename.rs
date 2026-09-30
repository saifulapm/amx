//! `amx rename`: change the name an agent's row shows.
//!
//! Only the display name changes. The id, which names the record, the tmux
//! session, the branch and the worktree, stays as it was. A name that is empty
//! or too long for a row is refused with `EX_USAGE`. The view's `ctrl+r` calls
//! [`rename`] too, so both apply the same rules.

use anyhow::Result;
use std::io::Write;
use std::path::Path;

use crate::store::Agent;
use crate::{complain, exit, paths};

/// The outcome of a rename.
pub enum Renamed {
    /// Renamed; the sentence saying what it is called now.
    Yes(String),
    /// Refused; the reason.
    No(String),
}

/// The longest name, in characters, that the view's name column shows whole.
pub const NAME: usize = 24;

/// Run the verb against the machine.
pub fn from_env(id: &str, name: &str) -> Result<i32> {
    let root = paths::state_root()?;
    let mut out = std::io::stdout().lock();
    run(&root, id, name, &mut out)
}

/// The verb, with the state directory named. A refused name goes to stderr
/// with `EX_USAGE`.
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

/// Set the agent's display name.
///
/// Control characters become spaces and runs of whitespace collapse to one, so
/// the record never holds a control character. Renaming to the id clears the
/// name.
pub fn rename(root: &Path, id: &str, typed: &str) -> Result<Renamed> {
    // A space rather than nothing, so a name pasted over two lines stays two
    // words.
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

    // `observe`, so `last_event` does not move: a rename is not news from the
    // agent, and a fresher record would be trusted over the pane.
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

    fn recorded(root: &Path, id: &str) -> Agent {
        Agent::create(
            root,
            &Meta {
                role: None,
                parent: None,
                depth: 0,
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                agent: None,
                model: None,
                effort: None,
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
    fn rename_puts_the_name_on_the_record_and_leaves_the_id_where_it_was() {
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
    fn rename_refuses_what_no_row_could_carry() {
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

        // Renaming to the id clears the name.
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
        // The reason goes to stderr, so stdout holds nothing a caller would
        // read as a name.
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
        // The only way to take a name back from a shell.
        let root = TempDir::new().unwrap();
        let agent = recorded(root.path(), "fix-login-a1b");

        let mut out = Vec::new();
        run(root.path(), "fix-login-a1b", "auth", &mut out).unwrap();
        let code = run(root.path(), "fix-login-a1b", "fix-login-a1b", &mut out).unwrap();

        assert_eq!(code, exit::OK);
        assert_eq!(agent.state().unwrap().name, None);
    }
}
