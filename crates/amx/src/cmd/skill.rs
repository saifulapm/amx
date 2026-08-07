//! `amx skill install` — the in-binary agent skill.
//!
//! 04 §8, herdr's K10 kept: "The **agent skill** ships in-binary: `amx skill
//! install` teaches agents to drive amx — spawn panes, send input, prompt
//! siblings, `wait --until blocked`, read outputs — gated on `AMX_ENV=1` with
//! pane/workspace identity env vars."
//!
//! In-binary because a skill fetched from anywhere else is a skill that can
//! disagree with the binary it is describing. The test that keeps it honest
//! walks every verb the asset names against `SPECS`, so a renamed method breaks
//! the build here rather than in an agent's hands.
//!
//! # Where it is written, and why amx does not choose
//!
//! Install copies [`ASSET`] into a directory the caller names, defaulting to
//! the current one — and that is the whole of its policy. It deliberately does
//! *not* know where any particular agent loads skills from: skill placement is
//! a per-agent fact, per-agent facts live in the registry stanza and nowhere
//! else (D-M2-2's rule, W6's lesson), and no stanza carries one today. Writing
//! `.claude/…` into this file would be an agent's name outside its stanza —
//! the exact fan-out the registry exists to prevent — so the path is the
//! user's, and [`WHERE`] tells them the one placement amx can state as a fact
//! about a tool it already ships a stanza for.
//!
//! # Task ownership
//!
//! **V16** filled this, with the asset under `crates/amx/assets/skill/` and
//! `examples/notify.sh` beside it.
//!
//! V02 planted the file and the clap tree so no wave task edits `cli.rs`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use amx_core::Env;
use anyhow::Context as _;
use clap::ArgMatches;

/// The clap id of the `--path` argument.
const PATH: &str = "path";

/// The skill, as the files it is written out as.
///
/// A table rather than one constant because a skill is a *directory* on every
/// loader that reads one, and a second file (a longer reference, a worked
/// example) must be an entry here rather than a second code path.
pub const ASSET: &[(&str, &str)] = &[("SKILL.md", include_str!("../../assets/skill/SKILL.md"))];

/// What a reader of the output needs in order to place it.
///
/// One sentence of fact, kept next to the thing it describes. Not behavior: see
/// the module docs for why install writes where it is told and nowhere else.
const WHERE: &str = "Claude Code loads a skill from `<dir>/SKILL.md` under \
                     `.claude/skills/` in a project or `~/.claude/skills/`, so \
                     `amx skill install --path .claude/skills/amx` installs it \
                     for this project.";

/// Write the agent skill into a project.
///
/// Neither the environment nor the root matches are read: this verb touches no
/// session, starts no server and makes no control call. It is the one command
/// in the tree that is purely a file being written.
pub async fn run(_env: &Env, _matches: &ArgMatches, sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    let (_verb, args) = sub
        .subcommand()
        .context("`amx skill` needs a verb; `install` is the only one")?;
    let dir = match args.get_one::<String>(PATH) {
        Some(path) => PathBuf::from(path),
        None => std::env::current_dir().context("read the current directory")?,
    };

    for (name, text) in ASSET {
        let path = dir.join(name);
        let outcome = write(&path, text)?;
        println!("{} {}", word(outcome), path.display());
    }
    println!("{WHERE}");
    Ok(ExitCode::SUCCESS)
}

/// What writing one file did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Outcome {
    /// The file was not there.
    Written,
    /// It was there, saying something else.
    Updated,
    /// It was there, byte for byte.
    Unchanged,
}

/// Put `text` at `path`, reporting which of the three things happened.
///
/// Reading before writing is what makes a reinstall a no-op *and* what makes
/// the difference visible: an installer that always writes and always says
/// "installed" cannot tell a user whether their edited copy has just been
/// replaced. The asset is amx's own file and a differing copy is overwritten —
/// a customised skill belongs at a path of the user's own, which `--path` is
/// for — but the word says so.
fn write(path: &Path, text: &str) -> anyhow::Result<Outcome> {
    let existing = std::fs::read_to_string(path).ok();
    if existing.as_deref() == Some(text) {
        return Ok(Outcome::Unchanged);
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(if existing.is_some() {
        Outcome::Updated
    } else {
        Outcome::Written
    })
}

/// The word for an outcome.
const fn word(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Written => "wrote",
        Outcome::Updated => "updated",
        Outcome::Unchanged => "unchanged",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

    use super::{ASSET, Outcome, write};

    #[test]
    fn writing_the_same_bytes_twice_is_unchanged_the_second_time() {
        let dir = std::env::temp_dir().join(format!("amx-skill-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the temp dir");
        let path = dir.join("nested").join("SKILL.md");

        assert_eq!(write(&path, "one").expect("write"), Outcome::Written);
        assert_eq!(write(&path, "one").expect("write"), Outcome::Unchanged);
        assert_eq!(write(&path, "two").expect("write"), Outcome::Updated);
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "two");

        std::fs::remove_dir_all(&dir).expect("remove the temp dir");
    }

    #[test]
    fn the_asset_is_present_and_named() {
        let (name, text) = ASSET.first().expect("the skill has at least one file");
        assert_eq!(*name, "SKILL.md");
        assert!(
            text.starts_with("---\nname: amx\n"),
            "the asset must open with its own frontmatter"
        );
    }
}
