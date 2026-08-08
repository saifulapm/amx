//! Where a worktree goes (D-M3-10).
//!
//! One question, and it is asked twice: `amx work <branch>` asks it to decide
//! where to put a tree, and `amx work done` asks it again to decide whether the
//! tree it has been pointed at is the one this template derives. That second
//! reading is the pin that keeps the destructive verb off a path the user typed,
//! and it only works if the answer is a pure function of the repository, the
//! branch and the template — so the answer lives here, apart from the verb, with
//! nothing else in scope that it could come to depend on.
//!
//! The other half of the same question is *which spelling* an answer is in.
//! [`derive_path`] renders a template and nothing more, so what it hands back is
//! whatever characters the template held; git answers the same question in the
//! filesystem's own spelling, with every symlink resolved. [`resolve`] is where
//! the two are made comparable, and it is deliberately not inside `derive_path`:
//! the pin on the destructive path is that the derivation is a pure function of
//! three strings, and a derivation that touched the filesystem would not be one.
//!
//! [`crate::cmd::work`] is the verb; [`crate::git`] is the git.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use amx_core::{Config, Ctx};
use anyhow::{Context as _, bail};

/// Where a worktree goes when `work.dir` says nothing.
///
/// A sibling of the repository, named for the repository and the branch, so that
/// repo-internal tooling — a test that walks the tree, a linter with a glob, a
/// build that hashes every file — never trips over a second checkout of the same
/// project.
///
/// The separator is a double dash because a branch name cannot hold one: git
/// refuses `a..b` as a ref, so `repo--feature/x` reads back unambiguously.
pub const DEFAULT_DIR: &str = "{repo_parent}/{repo_name}--{branch}";

/// The `work.dir` template: the config file's, then the built-in default.
///
/// Read from disk here rather than asked of the server, and leniently, through
/// the same [`amx_core::config::reload`] every server uses — so a half-edited
/// file costs the user the default rather than a failure. The same shape
/// `amx update`'s channel override has.
#[must_use]
pub fn template(ctx: &Ctx) -> String {
    std::fs::read_to_string(&ctx.config_path)
        .ok()
        .and_then(|text| {
            amx_core::config::reload(&Config::default(), &text)
                .0
                .work
                .dir
        })
        .unwrap_or_else(|| DEFAULT_DIR.to_owned())
}

/// Substitute `{repo_parent}`, `{repo_name}` and `{branch}` into `template`.
///
/// # Errors
///
/// If the repository path has no parent or no final component, if the result is
/// not absolute, if it still holds a `{...}` token — a placeholder amx does not
/// know is a typo in someone's config, and quietly making a directory named
/// after it would be worse than saying so — or if any component is `..`, which
/// no substitution of a git-legal branch name can produce and no template should
/// need.
pub fn derive_path(template: &str, repo: &Path, branch: &str) -> anyhow::Result<PathBuf> {
    let parent = repo
        .parent()
        .with_context(|| format!("{} has no parent directory", repo.display()))?;
    let name = repo
        .file_name()
        .with_context(|| format!("{} has no final path component", repo.display()))?;
    let rendered = template
        .replace("{repo_parent}", &parent.to_string_lossy())
        .replace("{repo_name}", &name.to_string_lossy())
        .replace("{branch}", branch);

    if let Some(start) = rendered.find('{') {
        let token: String = rendered[start..]
            .chars()
            .take_while(|c| *c != '}')
            .chain(std::iter::once('}'))
            .collect();
        bail!(
            "work.dir template {template:?} holds {token}, which amx does not substitute; \
             the ones it does are {{repo_parent}}, {{repo_name}} and {{branch}}"
        );
    }
    let path = PathBuf::from(rendered);
    anyhow::ensure!(
        path.is_absolute(),
        "work.dir template {template:?} derives {}, which is not an absolute path",
        path.display(),
    );
    anyhow::ensure!(
        !path
            .components()
            .any(|part| part == std::path::Component::ParentDir),
        "work.dir template {template:?} derives {}, which walks upwards",
        path.display(),
    );
    Ok(path)
}

/// The same directory, spelled the way the filesystem spells it.
///
/// git is the reason this exists. `git worktree list --porcelain` prints each
/// worktree's *realpath* — verified on git 2.55.0, both for a tree added through
/// a symlinked path and for a list asked through one — while [`derive_path`]
/// prints whatever the template said. The two spellings are the same directory
/// and are not the same characters, and on darwin they differ for every
/// temporary directory there is: `$TMPDIR` sits under `/var`, which is a symlink
/// to `/private/var`. Comparing characters answers "a different worktree" about
/// one worktree, and on the destructive path that answer is a refusal to remove
/// a tree amx made itself.
///
/// **A path that is not there resolves as far as it goes**: the deepest ancestor
/// that does exist is canonicalized and the rest of the components are put back
/// on. That is what makes this usable where it is needed most — `amx work done`
/// derives, compares and refuses for a tree somebody has already deleted, which
/// is exactly the input [`std::fs::canonicalize`] answers with an error. A path
/// with no existing ancestor at all comes back unchanged, the same shape
/// [`crate::integration::edit::store`] uses when it cannot resolve a file.
///
/// Resolution is a question about a directory, so it is asked of the filesystem
/// every time rather than remembered: a symlink can be re-pointed between two
/// runs of amx, and a cached answer would outlive the arrangement it described.
#[must_use]
pub fn resolve(path: &Path) -> PathBuf {
    // Deepest-first, so the loop reads back as "canonicalize what exists, then
    // re-attach what does not".
    let mut trailing: Vec<&OsStr> = Vec::new();
    let mut head = path;
    loop {
        if let Ok(real) = head.canonicalize() {
            let mut resolved = real;
            resolved.extend(trailing.iter().rev().copied());
            return resolved;
        }
        // No parent left means the root itself did not resolve — a filesystem
        // with nothing to say about this path, so the path stands as it is.
        let (Some(parent), Some(name)) = (head.parent(), head.file_name()) else {
            return path.to_owned();
        };
        trailing.push(name);
        head = parent;
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{DEFAULT_DIR, derive_path, resolve};

    #[test]
    fn the_default_template_puts_the_tree_beside_the_repository() {
        assert_eq!(
            derive_path(DEFAULT_DIR, Path::new("/src/amx"), "feat").unwrap(),
            PathBuf::from("/src/amx--feat"),
        );
        // A branch with a slash nests, which is a directory the template's own
        // author asked for by writing `{branch}` — still under the sibling
        // prefix, still nowhere near the repository's inside.
        assert_eq!(
            derive_path(DEFAULT_DIR, Path::new("/src/amx"), "feature/x").unwrap(),
            PathBuf::from("/src/amx--feature/x"),
        );
    }

    #[test]
    fn a_template_amx_cannot_satisfy_is_refused_by_name() {
        let err = derive_path(
            "{repo_parent}/{repo}--{branch}",
            Path::new("/src/amx"),
            "feat",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("{repo}"), "{err}");

        for bad in ["trees/{branch}", "{repo_parent}/../{branch}"] {
            assert!(
                derive_path(bad, Path::new("/src/amx"), "feat").is_err(),
                "{bad:?} must not derive a path",
            );
        }
    }

    #[test]
    fn a_directory_that_is_not_there_resolves_as_far_as_it_goes() {
        // `$TMPDIR` is the case that matters and the one darwin gets wrong for
        // free: there it is a path under `/var`, a symlink to `/private/var`, so
        // the two spellings of this directory really are different characters on
        // the runner this has to pass on. On Linux the two coincide and the
        // assertion is a tautology — which is the whole reason the symlink case
        // is driven end to end in `tests/work.rs` rather than only here.
        let temp = std::env::temp_dir();
        let real = resolve(&temp);
        assert!(real.is_absolute(), "{}", real.display());

        // Two components that do not exist, under one that does. Nothing is
        // created: the answer for a missing path is the point.
        let missing = temp.join("amx-no-such-tree").join("feat");
        assert!(!missing.exists(), "the fixture path must not exist");
        assert_eq!(
            resolve(&missing),
            real.join("amx-no-such-tree").join("feat")
        );

        // And a path the filesystem cannot help with at all is left alone.
        assert_eq!(resolve(Path::new("/")), PathBuf::from("/"));
        assert_eq!(
            resolve(Path::new("relative/leaf")),
            PathBuf::from("relative/leaf"),
        );
    }
}
