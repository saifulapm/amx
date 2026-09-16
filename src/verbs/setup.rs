//! `amx setup <vendor>` — wire one agent's reporting into this machine.
//!
//! Everything amx knows about a running agent arrives through the vendor's own
//! hooks, and putting them where that vendor looks for them is this verb.
//! `uninstall` is its mirror: that one walks the whole table and takes every
//! vendor's wiring out, this one takes a name and wires that one.
//!
//! The name is not optional and is never guessed. A machine usually has more
//! than one agent on it, and the one a config happens to name is not evidence
//! about the others — amx wiring an agent nobody asked it to would be writing
//! under somebody's home on a hunch. So a bare `amx setup` prints the agents
//! it knows and writes nothing.
//!
//! Nor does it ask. `doctor --fix` had to, because one flag stood for every
//! repair it could make and the settings file is the person's; naming the
//! agent on the command line is that consent, said more precisely. What is
//! kept from that door is the sentence: the file being written is named before
//! it is written, and copied aside first.

use anyhow::Result;
use std::io::Write;
use std::path::Path;

use crate::vendor::Wire;
use crate::{exit, install, registry, store};

/// Run the verb against the machine's own paths.
pub fn from_env(vendor: Option<&str>) -> Result<i32> {
    let home = install::home()?;
    let mut out = std::io::stdout().lock();
    run(vendor, &home, store::now(), &mut out)
}

/// Run the verb, with everything it touches named: the agent, and the home its
/// wiring goes under.
///
/// The hook command is not among them. Every wire amx writes now runs `amx`
/// off the PATH rather than the path this amx happens to stand at, which is
/// the thing `doctor` insists on when it asks that there be one amx and this
/// be it.
pub fn run(vendor: Option<&str>, home: &Path, now: u64, out: &mut impl Write) -> Result<i32> {
    let Some(name) = vendor else {
        writeln!(out, "name an agent to set up: {}", every_agent())?;
        return Ok(exit::USAGE);
    };
    let Some(entry) = registry::entry(name) else {
        writeln!(out, "amx has no entry for `{name}`: {}", every_agent())?;
        return Ok(exit::USAGE);
    };
    let Some(hooks) = &entry.hooks else {
        writeln!(
            out,
            "{} reports nothing amx can wire, so its pane is what amx reads",
            entry.name
        )?;
        return Ok(exit::OK);
    };

    let path = install::wire_path(hooks, home);
    writeln!(
        out,
        "{}",
        install::consent_line(hooks, &path, path.exists())
    )?;

    let wrote = install::install_hooks(hooks, home, now)?;
    if !wrote.changed {
        writeln!(
            out,
            "nothing to do: {} is already that",
            wrote.path.display()
        )?;
        return Ok(exit::OK);
    }
    match hooks.wire {
        Wire::File { .. } => writeln!(out, "wrote the extension to {}", wrote.path.display())?,
        Wire::Plugin { .. } => writeln!(out, "wrote the plugin to {}", wrote.path.display())?,
    }
    if let Some(backup) = wrote.backup {
        writeln!(out, "the file as it was is at {}", backup.display())?;
    }
    Ok(exit::OK)
}

/// Every agent amx has an entry for, as a sentence names them.
fn every_agent() -> String {
    registry::entries()
        .iter()
        .map(|vendor| vendor.name)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use tempfile::TempDir;

    /// Run the verb over a home of the test's own, and answer with what it
    /// exited and what it printed.
    fn said(vendor: Option<&str>, home: &Path, now: u64) -> (i32, String) {
        let mut out = Vec::new();
        let code = run(vendor, home, now, &mut out).unwrap();
        (code, String::from_utf8(out).unwrap())
    }

    #[test]
    fn setup_writes_claudes_plugin_and_keeps_the_skill_that_was_there() {
        // claude reports through a plugin amx writes under the skills
        // directory, not through entries in anybody's settings. A skill
        // already standing at that name is somebody's own until amx has left
        // a manifest there, so it is copied aside rather than lost.
        let home = TempDir::new().unwrap();
        let hooks = crate::vendor::claude::VENDOR.hooks.expect("claude reports");
        let dir = install::wire_path(&hooks, home.path());
        std::fs::create_dir_all(&dir).unwrap();
        let theirs = "---\nname: amx\n---\n\ntheir own copy\n";
        std::fs::write(dir.join("SKILL.md"), theirs).unwrap();

        let (code, printed) = said(Some("claude"), home.path(), 7);

        assert_eq!(code, exit::OK, "{printed}");
        assert!(
            printed.contains(&dir.display().to_string()),
            "the directory is named before it is written: {printed}"
        );
        assert!(printed.contains("wrote the plugin to"), "{printed}");

        let manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(install::MANIFEST)).unwrap())
                .unwrap();
        assert_eq!(manifest["name"], "amx");
        let wiring = std::fs::read_to_string(dir.join("hooks/hooks.json")).unwrap();
        for event in install::events(&hooks) {
            assert!(wiring.contains(event), "{event} is not wired: {wiring}");
        }
        assert!(
            !home.path().join(".claude/settings.json").exists(),
            "no settings file of anybody's was opened, let alone written"
        );

        let backup = install::latest_backup(&dir.join("SKILL.md"))
            .unwrap()
            .expect("a copy of their skill");
        assert!(printed.contains(&backup.display().to_string()), "{printed}");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), theirs);
    }

    #[test]
    fn setup_writes_pis_extension_where_pi_loads_one() {
        let home = TempDir::new().unwrap();
        let hooks = crate::vendor::pi::VENDOR.hooks.expect("pi reports");
        let extension = install::wire_path(&hooks, home.path());

        let (code, printed) = said(Some("pi"), home.path(), 1);

        assert_eq!(code, exit::OK, "{printed}");
        assert!(printed.contains("wrote the extension to"), "{printed}");
        assert!(
            printed.contains(&extension.display().to_string()),
            "{printed}"
        );
        let written = std::fs::read_to_string(&extension).expect("the extension");
        assert!(
            written.contains("_hook"),
            "it reports through amx: {written}"
        );
    }

    #[test]
    fn setup_run_again_writes_nothing_and_says_so() {
        let home = TempDir::new().unwrap();
        for agent in ["claude", "pi"] {
            assert_eq!(said(Some(agent), home.path(), 1).0, exit::OK);

            let (code, printed) = said(Some(agent), home.path(), 2);
            assert_eq!(code, exit::OK, "{printed}");
            assert!(printed.contains("nothing to do"), "{agent}: {printed}");
        }
    }

    #[test]
    fn setup_with_no_name_writes_nothing_and_names_every_agent() {
        let home = TempDir::new().unwrap();

        let (code, printed) = said(None, home.path(), 1);

        assert_eq!(code, exit::USAGE, "{printed}");
        for vendor in registry::entries() {
            assert!(printed.contains(vendor.name), "{printed}");
        }
        assert_eq!(
            std::fs::read_dir(home.path()).unwrap().count(),
            0,
            "an agent amx was not asked about is one it does not write for"
        );
    }

    #[test]
    fn setup_refuses_a_name_amx_has_no_entry_for() {
        let home = TempDir::new().unwrap();

        let (code, printed) = said(Some("codex"), home.path(), 1);

        assert_eq!(code, exit::USAGE, "{printed}");
        assert!(printed.contains("codex"), "it names what was asked for");
        for vendor in registry::entries() {
            assert!(printed.contains(vendor.name), "{printed}");
        }
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    }
}
