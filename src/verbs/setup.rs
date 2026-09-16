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
    let command = install::hook_command(&std::env::current_exe()?);
    let mut out = std::io::stdout().lock();
    run(vendor, &home, &command, store::now(), &mut out)
}

/// Run the verb, with everything it touches named: the agent, the home its
/// wiring goes under, and the hook command that wiring will run.
pub fn run(
    vendor: Option<&str>,
    home: &Path,
    command: &str,
    now: u64,
    out: &mut impl Write,
) -> Result<i32> {
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

    let wrote = install::install_hooks(hooks, home, command, now)?;
    if !wrote.changed {
        writeln!(
            out,
            "nothing to do: {} is already that",
            wrote.path.display()
        )?;
        return Ok(exit::OK);
    }
    match hooks.wire {
        Wire::Settings(_) => writeln!(out, "wired the hooks into {}", wrote.path.display())?,
        Wire::File { .. } => writeln!(out, "wrote the extension to {}", wrote.path.display())?,
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

    const COMMAND: &str = "/home/dev/.cargo/bin/amx _hook";

    /// Run the verb over a home of the test's own, and answer with what it
    /// exited and what it printed.
    fn said(vendor: Option<&str>, home: &Path, now: u64) -> (i32, String) {
        let mut out = Vec::new();
        let code = run(vendor, home, COMMAND, now, &mut out).unwrap();
        (code, String::from_utf8(out).unwrap())
    }

    #[test]
    fn setup_wires_claudes_hooks_into_the_settings_file_and_keeps_a_copy() {
        let home = TempDir::new().unwrap();
        let hooks = crate::vendor::claude::VENDOR.hooks.expect("claude reports");
        let settings = install::wire_path(&hooks, home.path());
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        std::fs::write(&settings, "{\"model\": \"opus\"}\n").unwrap();

        let (code, printed) = said(Some("claude"), home.path(), 7);

        assert_eq!(code, exit::OK, "{printed}");
        assert!(
            printed.contains(&settings.display().to_string()),
            "the file is named before it is written: {printed}"
        );
        assert!(printed.contains("wired the hooks into"), "{printed}");

        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(
            install::installed_events(&hooks, &written, COMMAND).len(),
            hooks.events.len(),
            "every event claude's entry names: {printed}"
        );
        assert_eq!(written["model"], "opus", "the rest is left alone");

        let backup = install::latest_backup(&settings).unwrap().expect("a copy");
        assert!(printed.contains(&backup.display().to_string()), "{printed}");
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            "{\"model\": \"opus\"}\n"
        );
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
