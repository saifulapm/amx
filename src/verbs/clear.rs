//! `amx clear` — forget the rows that are over.
//!
//! [`sweep`](super::sweep) takes the agents somebody else established are done
//! with: a request the forge settled, a branch git reads as in the main line.
//! Most of what fills a wall is none of those. A row somebody stopped, a
//! command that ran and exited, an agent that was never given a branch to land
//! anything on — nothing outside amx will ever have an opinion about them, so
//! nothing outside amx can say when their records go. Without this verb they
//! go one `ctrl+x` or one `amx stop --delete` at a time, which on a wall of
//! sixty is why they do not go at all.
//!
//! So the two verbs are the same shape over different lists: everything
//! finished is listed with the reason it is finished, one question covers the
//! list, and then each row is taken the way its own evidence says to — the
//! sweep's way where the work landed, and otherwise the way `ctrl+x` takes one
//! row, which is the record and a tree amx cut, with the branch left standing.
//!
//! An agent sitting at its prompt is not finished. It has a session somebody
//! can still send a turn to, and the whole cost of leaving it on the wall is a
//! row; the whole cost of getting it wrong is a conversation nobody can reach
//! again.
//!
//! The one law it will not break is `stop`'s: a tree holding work no commit
//! has is never removed, and neither is the record that names it.

use anyhow::Result;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::derive::{self, View};
use crate::store::Agent;
use crate::verbs::{stop, sweep};
use crate::{exit, paths, store, worktree};

/// What taking one row came to.
pub enum Taken {
    /// The record is gone, and the tree amx cut with it.
    Gone,
    /// Both are still here, because this tree holds work no commit has.
    Holding(PathBuf),
}

/// Run the verb against the machine.
pub fn from_env(force: bool) -> Result<i32> {
    let root = paths::state_root()?;
    let mut input = std::io::stdin().lock();
    let mut out = std::io::stdout().lock();
    run(&root, force, &mut input, &mut out)
}

/// The verb, with the state directory named.
pub fn run(
    root: &Path,
    force: bool,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<i32> {
    let views = derive::views(root, store::now())?;
    let rows = finished_rows(&views);

    if rows.is_empty() {
        writeln!(out, "nothing to clear")?;
        return Ok(exit::OK);
    }

    for (at, why) in &rows {
        writeln!(out, "{}  {why}", views[*at].id())?;
    }
    if !force && !agreed(rows.len(), input, out)? {
        writeln!(out, "nothing cleared")?;
        return Ok(exit::OK);
    }

    for (at, _) in &rows {
        let view = &views[*at];
        if let Taken::Holding(tree) = take_row(root, view)? {
            writeln!(
                out,
                "kept {}: {} holds work no commit has",
                view.id(),
                tree.display()
            )?;
        }
    }
    Ok(exit::OK)
}

/// Which of these rows are finished, and why each one is on the list.
///
/// Where the work landed the reason is the sweep's own — `#12 merged` — read
/// off what the last look wrote down and never off a network: this is what the
/// view presses for too, and a press must not stand still while a forge
/// answers. Everywhere else the reason is the phase, which is all there is to
/// say about a row that stopped.
///
/// The index rather than the view, because the caller has the views and the
/// view has no place on the wall until somebody counts them.
pub fn finished_rows(views: &[View]) -> Vec<(usize, String)> {
    views
        .iter()
        .enumerate()
        .filter(|(_, view)| view.phase().is_terminal())
        .map(|(at, view)| {
            let why = sweep::why_landed(view).unwrap_or_else(|| view.phase().to_string());
            (at, why)
        })
        .collect()
}

/// Take one finished row the way its own evidence says to.
///
/// Work that landed goes the sweep's way — the tree, the branch and the record
/// together, since the repository holds every commit that was on it. Work that
/// did not keeps its branch: nothing here says it is safe to lose, and a
/// branch costs a line in `git branch`.
///
/// What the ladder says as it goes is dropped. Both doors this is behind print
/// their own sentence about the whole list, and neither has room for the run
/// of lines `stop` writes per agent.
pub fn take_row(root: &Path, view: &View) -> Result<Taken> {
    if sweep::why_landed(view).is_none() {
        return forget_row(root, view);
    }
    if let Some(tree) = holding(view) {
        return Ok(Taken::Holding(tree));
    }
    sweep::take_landed(root, &view.meta, &mut std::io::sink())?;
    Ok(Taken::Gone)
}

/// Forget a row whose work went nowhere: its record, and the tree amx gave it.
///
/// A tree holding work no commit has keeps both. Its record is where the
/// branch and the commit that tree was cut from are named, and a tree nothing
/// names is work nobody will find again.
pub fn forget_row(root: &Path, view: &View) -> Result<Taken> {
    let agent = Agent::open(root, view.id())?;

    if let Some(tree) = holding(view) {
        return Ok(Taken::Holding(tree));
    }
    if let Some(tree) = &view.meta.worktree
        && tree.exists()
    {
        let repo = worktree::main_repo(tree).unwrap_or_else(|_| tree.clone());
        worktree::remove(&repo, tree)?;
        // And its key in the vendor's store with it, the way `stop` takes it:
        // the caller has one line to say what happened to the whole list.
        stop::forget(&view.meta, tree, &mut std::io::sink())?;
    }

    agent.remove()?;
    Ok(Taken::Gone)
}

/// The tree this row will not give up, if it has one.
///
/// A tree amx cannot read is read as dirty: the answer that keeps the work is
/// the answer to give when git will not say.
fn holding(view: &View) -> Option<PathBuf> {
    let tree = view.meta.worktree.as_ref()?;
    (tree.exists() && worktree::is_dirty(tree).unwrap_or(true)).then(|| tree.clone())
}

/// The one question, asked once for the whole list.
///
/// It reads the way every other question amx asks reads: the default is the
/// one that loses nothing, and anything that is not plainly yes — a shrug, a
/// typo, nobody there at all — takes it.
fn agreed(count: usize, input: &mut impl BufRead, out: &mut impl Write) -> Result<bool> {
    write!(out, "clear {count}? [y/N] ")?;
    out.flush()?;

    let mut answer = String::new();
    if input.read_line(&mut answer)? == 0 {
        writeln!(out)?;
        return Ok(false);
    }
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}
