//! `amx diff` — what an agent has done to its tree, while it is still doing it.
//!
//! The comparison is against the commit the tree was cut from, recorded when it
//! was cut. Not against the repository's HEAD, which has moved on since, and not
//! against the agent's own HEAD, which would hide everything it has committed —
//! what a person wants to see is the whole of this agent's work.
//!
//! An agent working directly in a directory has nothing to compare, and a tree
//! somebody has removed is not there to read. Both are ordinary answers to an
//! ordinary question, so both say what happened rather than failing at git.

use anyhow::{Result, bail};
use std::io::Write;
use std::path::Path;

use crate::store::Agent;
use crate::{exit, paths, worktree};

/// Run the verb against the machine.
pub fn from_env(id: &str) -> Result<i32> {
    let root = paths::state_root()?;
    let mut out = std::io::stdout().lock();
    run(&root, id, &mut out)
}

/// The verb, with the state directory named.
pub fn run(root: &Path, id: &str, out: &mut impl Write) -> Result<i32> {
    let meta = Agent::open(root, id)?.meta()?;

    let (Some(tree), Some(base)) = (&meta.worktree, &meta.base) else {
        bail!(
            "`{id}` has no worktree of its own — it works in {}, \
             so there is nothing to compare it against",
            meta.dir.display()
        );
    };

    if !tree.exists() {
        match &meta.branch {
            Some(branch) => bail!("{} is gone; what `{id}` did is on {branch}", tree.display()),
            None => bail!("{} is gone", tree.display()),
        }
    }

    worktree::diff(tree, base, out)?;
    Ok(exit::OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Meta, now};
    use crate::tmux::{PaneId, Socket};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// A record of an agent, with its tree wherever the test wants it.
    fn record(root: &Path, id: &str, worktree: Option<&Path>) {
        Agent::create(
            root,
            &Meta {
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                dir: worktree
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from("/srv/app")),
                worktree: worktree.map(Path::to_path_buf),
                branch: worktree.map(|_| worktree::branch_for(id)),
                base: worktree.map(|_| "0f1e2d3".to_string()),
                socket: Socket::Name("amx".to_string()),
                pane: PaneId::new("%1").unwrap(),
                session: None,
                transcript: None,
                created: now(),
            },
        )
        .expect("the record");
    }

    fn refused(root: &Path, id: &str) -> String {
        let mut out = Vec::new();
        format!("{:#}", run(root, id, &mut out).unwrap_err())
    }

    #[test]
    fn diff_has_nothing_to_compare_for_an_agent_without_a_tree() {
        let root = TempDir::new().unwrap();
        record(root.path(), "no-tree-b2c", None);

        let said = refused(root.path(), "no-tree-b2c");
        assert!(said.contains("no worktree"), "{said}");
        assert!(
            said.contains("/srv/app"),
            "and it names where the work happens instead: {said}"
        );
    }

    #[test]
    fn diff_says_where_the_work_went_when_the_tree_is_gone() {
        let root = TempDir::new().unwrap();
        record(
            root.path(),
            "fix-login-a1b",
            Some(Path::new("/srv/app/.amx/worktrees/fix-login-a1b")),
        );

        let said = refused(root.path(), "fix-login-a1b");
        assert!(said.contains("gone"), "{said}");
        assert!(said.contains("amx/fix-login-a1b"), "{said}");
    }

    #[test]
    fn diff_is_never_taken_through_something_that_is_not_an_id() {
        // `root.join(id)` is not a lookup: an id shaped like a path would name
        // a record anywhere on the machine, and then a tree to run git in.
        let root = TempDir::new().unwrap();
        assert!(refused(root.path(), "../elsewhere").contains("no agent"));
        assert!(refused(root.path(), "never-made-abc").contains("no agent"));
    }
}
