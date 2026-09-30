//! `amx setup <vendor>`: install one vendor's hooks on this machine.
//!
//! Every vendor with hooks has one reporting wire, always installed, and may
//! have opt-in wires (the subagent tool) installed only with `--subagent`.
//! `uninstall` removes every vendor's wiring.
//!
//! The vendor must be named; a bare `amx setup` lists the known vendors and
//! writes nothing. Naming it is the consent, so nothing is asked. Each write
//! is announced first, and any existing file at that path is copied aside. An
//! already wired machine is told so instead.

use anyhow::Result;
use std::io::Write;
use std::path::Path;

use crate::vendor::Wire;
use crate::{exit, install, registry, store};

/// Run the verb against the machine.
pub fn from_env(vendor: Option<&str>, subagent: bool) -> Result<i32> {
    let home = install::home()?;
    let mut out = std::io::stdout().lock();
    run(
        vendor,
        subagent,
        &home,
        &install::process_env,
        store::now(),
        &mut out,
    )
}

/// Run the verb with the home directory and environment named.
///
/// The hook command every wire runs is `amx` from `PATH`, not this binary's
/// path.
pub fn run(
    vendor: Option<&str>,
    subagent: bool,
    home: &Path,
    env: install::Env,
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
    // The vendor has hooks but no subagent tool: name the vendors that do.
    if subagent && hooks.opt_in.is_empty() {
        writeln!(
            out,
            "amx has no subagent to wire for `{}`: {}",
            entry.name,
            every_opt_in()
        )?;
        return Ok(exit::USAGE);
    }

    // Each wire is checked before its announcement, so an already wired
    // machine is not promised a write and a copy that never happen.
    let mut wires: Vec<&Wire> = vec![&hooks.wire];
    if subagent {
        wires.extend(hooks.opt_in.iter());
    }
    let mut wrote = false;
    for wire in wires {
        wrote |= wire_one(wire, home, env, now, out)?;
    }
    if !wrote {
        writeln!(
            out,
            "nothing to do: {} is already that",
            install::wire_path(&hooks.wire, home, env).display()
        )?;
    }
    Ok(exit::OK)
}

/// Install one wire unless it is already current, announcing the write first
/// and copying aside what was there. Returns whether anything was written.
fn wire_one(
    wire: &Wire,
    home: &Path,
    env: install::Env,
    now: u64,
    out: &mut impl Write,
) -> Result<bool> {
    let path = install::wire_path(wire, home, env);
    if install::wired(wire, home, env)
        == (install::Wired::File {
            present: true,
            current: true,
        })
    {
        return Ok(false);
    }
    let keep = install::would_keep_a_copy(wire, home, env);
    writeln!(out, "{}", install::consent_line(wire, &path, keep))?;
    let wrote = install::install_wire(wire, home, env, now)?;
    match wire {
        Wire::File { .. } => writeln!(out, "wrote the extension to {}", wrote.path.display())?,
        Wire::Plugin { .. } | Wire::Placed { .. } => {
            writeln!(out, "wrote the plugin to {}", wrote.path.display())?
        }
        Wire::Hooks { .. } => writeln!(
            out,
            "added the hooks to {} and trusted them in {}",
            wrote.path.display(),
            path.join(install::CONFIG_FILE).display()
        )?,
    }
    if let Some(backup) = wrote.backup {
        writeln!(out, "the file as it was is at {}", backup.display())?;
    }
    Ok(true)
}

/// Every vendor amx knows, comma separated.
fn every_agent() -> String {
    registry::entries()
        .iter()
        .map(|vendor| vendor.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every vendor with an opt-in wire, comma separated.
fn every_opt_in() -> String {
    registry::entries()
        .iter()
        .filter(|vendor| vendor.hooks.as_ref().is_some_and(|h| !h.opt_in.is_empty()))
        .map(|vendor| vendor.name)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use tempfile::TempDir;

    /// Run the verb over a test home; return the exit code and stdout.
    fn said(vendor: Option<&str>, home: &Path, now: u64) -> (i32, String) {
        said_with(vendor, false, home, now)
    }

    /// [`said`], with or without `--subagent`.
    fn said_with(vendor: Option<&str>, subagent: bool, home: &Path, now: u64) -> (i32, String) {
        let mut out = Vec::new();
        let code = run(vendor, subagent, home, &install::no_env, now, &mut out).unwrap();
        (code, String::from_utf8(out).unwrap())
    }

    #[test]
    fn setup_writes_claudes_plugin_and_keeps_the_skill_that_was_there() {
        // claude's hooks are a plugin under the skills directory. A skill
        // already there without amx's manifest is the person's, so it is
        // copied aside.
        let home = TempDir::new().unwrap();
        let hooks = crate::vendor::claude::VENDOR.hooks.expect("claude reports");
        let dir = install::wire_path(&hooks.wire, home.path(), &install::no_env);
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
        let extension = install::wire_path(&hooks.wire, home.path(), &install::no_env);

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
    fn setup_writes_opencodes_plugin_where_its_tui_loads_one() {
        // Into `OPENCODE_CONFIG_DIR`, else the default config dir. No config
        // file of the person's is opened.
        let home = TempDir::new().unwrap();
        let plugin = home.path().join(".config/opencode/plugins/amx/tui.js");

        let (code, printed) = said(Some("opencode"), home.path(), 1);

        assert_eq!(code, exit::OK, "{printed}");
        assert!(printed.contains("wrote the plugin to"), "{printed}");
        assert!(printed.contains(&plugin.display().to_string()), "{printed}");
        let written = std::fs::read_to_string(&plugin).expect("the plugin");
        assert!(written.starts_with("// installed by amx\n"), "{written}");
        assert!(written.contains("\"_hook\""), "it reports through amx");
        for theirs in ["opencode.json", "cli.json", "tui.json"] {
            assert!(!home.path().join(".config/opencode").join(theirs).exists());
        }
        assert!(
            said(Some("opencode"), home.path(), 2)
                .1
                .contains("nothing to do")
        );

        let moved = TempDir::new().unwrap();
        let env = |name: &str| {
            (name == "OPENCODE_CONFIG_DIR").then(|| moved.path().as_os_str().to_owned())
        };
        let mut out = Vec::new();
        let code = run(Some("opencode"), false, home.path(), &env, 3, &mut out).unwrap();
        assert_eq!(code, exit::OK);
        assert!(moved.path().join("plugins/amx/tui.js").exists());
    }

    #[test]
    fn setup_run_again_writes_nothing_and_says_so() {
        let home = TempDir::new().unwrap();
        for agent in ["claude", "pi"] {
            assert_eq!(said(Some(agent), home.path(), 1).0, exit::OK);

            let (code, printed) = said(Some(agent), home.path(), 2);
            assert_eq!(code, exit::OK, "{printed}");
            assert!(printed.contains("nothing to do"), "{agent}: {printed}");
            assert!(
                !printed.contains("amx will"),
                "and it does not first say it is about to: {printed}"
            );
        }
    }

    #[test]
    fn setup_writes_pis_subagent_only_when_asked_and_uninstall_takes_it_back() {
        // `amx setup pi` installs only the reporting wire. The opt-in file is
        // the tool that calls `amx sub`, and reports nothing.
        let home = TempDir::new().unwrap();
        let hooks = crate::vendor::pi::VENDOR.hooks.expect("pi reports");
        let hook = install::wire_path(&hooks.wire, home.path(), &install::no_env);
        let tool = install::wire_path(&hooks.opt_in[0], home.path(), &install::no_env);

        said(Some("pi"), home.path(), 1);
        assert!(hook.exists(), "the reporting wire is written");
        assert!(!tool.exists(), "and nothing opted into was");

        let (code, printed) = said_with(Some("pi"), true, home.path(), 2);
        assert_eq!(code, exit::OK, "{printed}");
        assert!(printed.contains(&tool.display().to_string()), "{printed}");
        let written = std::fs::read_to_string(&tool).expect("the tool");
        assert!(written.starts_with("// installed by amx\n"), "{written}");
        assert!(written.contains("\"subagent\""), "{written}");
        assert!(written.contains("\"sub\""), "it runs the child: {written}");

        let (code, printed) = said_with(Some("pi"), true, home.path(), 3);
        assert_eq!(code, exit::OK, "{printed}");
        assert!(printed.contains("nothing to do"), "{printed}");

        install::uninstall_wire(&hooks.opt_in[0], home.path(), &install::no_env).unwrap();
        assert!(!tool.exists(), "and it comes back out on its own");
    }

    #[test]
    fn setup_refuses_a_subagent_for_a_vendor_that_carries_none() {
        // A vendor without a subagent tool refuses the flag.
        let home = TempDir::new().unwrap();

        let (code, printed) = said_with(Some("claude"), true, home.path(), 1);

        assert_eq!(code, exit::USAGE, "{printed}");
        assert!(printed.contains("no subagent"), "{printed}");
        assert!(
            printed.contains("pi"),
            "it names who carries one: {printed}"
        );
        assert_eq!(
            std::fs::read_dir(home.path()).unwrap().count(),
            0,
            "and writes nothing"
        );
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

        let (code, printed) = said(Some("aider"), home.path(), 1);

        assert_eq!(code, exit::USAGE, "{printed}");
        assert!(printed.contains("aider"), "it names what was asked for");
        for vendor in registry::entries() {
            assert!(printed.contains(vendor.name), "{printed}");
        }
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    }

    #[test]
    fn setup_merges_a_hooks_wire_and_says_so_once() {
        // A hooks wire of the tests' own: amx's groups are merged into the
        // person's hooks file, copied aside first, and a second run does
        // nothing.
        let home = TempDir::new().unwrap();
        let dir = install::wire_path(&install::HOOKS_WIRE, home.path(), &install::no_env);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hooks.json"), "{\"hooks\": {}}\n").unwrap();

        let mut out = Vec::new();
        let wrote = wire_one(
            &install::HOOKS_WIRE,
            home.path(),
            &install::no_env,
            7,
            &mut out,
        )
        .unwrap();
        let printed = String::from_utf8(out).unwrap();
        assert!(wrote, "{printed}");
        let hooks = dir.join("hooks.json").display().to_string();
        let config = dir.join("config.toml").display().to_string();
        assert!(
            printed.contains(&format!(
                "amx will add its hooks to {hooks} and trust them in {config}, keeping a copy"
            )),
            "{printed}"
        );
        assert!(
            printed.contains(&format!(
                "added the hooks to {hooks} and trusted them in {config}"
            )),
            "{printed}"
        );
        assert!(printed.contains("the file as it was is at"), "{printed}");

        let mut out = Vec::new();
        assert!(
            !wire_one(
                &install::HOOKS_WIRE,
                home.path(),
                &install::no_env,
                8,
                &mut out
            )
            .unwrap()
        );
        assert!(out.is_empty(), "{}", String::from_utf8_lossy(&out));
    }
}
