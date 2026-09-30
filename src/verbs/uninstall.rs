//! `amx uninstall`: remove amx's wiring from every vendor and delete the records.
//!
//! Removes claude's plugin, pi's extension, and codex's hooks and their trust
//! entries. A directory without amx's manifest is left alone. Refuses while
//! any agent is still running, since its record is the only place its answer
//! is kept and nothing would record what it does next.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;

use crate::vendor::Wire;
use crate::{exit, install, paths, registry};

/// Run the verb against the machine's own paths.
pub fn from_env() -> Result<i32> {
    let state_root = paths::state_root()?;
    let home = install::home()?;
    let mut out = std::io::stdout().lock();
    run(&state_root, &home, &install::process_env, &mut out)
}

/// The verb, with the state root, the home the wiring lives under, and the
/// environment a wire may name its directory in.
pub fn run(state_root: &Path, home: &Path, env: install::Env, out: &mut impl Write) -> Result<i32> {
    let still_there = crate::spawn::unfinished(state_root)?;
    if !still_there.is_empty() {
        writeln!(
            out,
            "still running: {}. stop them first, or their answers go with the records.",
            still_there.join(", ")
        )?;
        return Ok(exit::FAILURE);
    }

    for vendor in registry::entries() {
        let Some(hooks) = &vendor.hooks else { continue };
        // Opt-in wires go too, not only the reporting one.
        for wire in std::iter::once(&hooks.wire).chain(hooks.opt_in.iter()) {
            let report = install::uninstall_wire(wire, home, env)?;
            let path = report.path.display();
            match (wire, report.changed) {
                (Wire::File { .. }, true) => writeln!(out, "removed {path}")?,
                (Wire::File { .. }, false) => writeln!(out, "no extension of amx's at {path}")?,
                (Wire::Plugin { .. } | Wire::Placed { .. }, true) => {
                    writeln!(out, "removed the plugin at {path}")?
                }
                (Wire::Plugin { .. } | Wire::Placed { .. }, false) => {
                    writeln!(out, "no plugin of amx's at {path}")?
                }
                (Wire::Hooks { .. }, true) => writeln!(out, "took amx's hooks out of {path}")?,
                (Wire::Hooks { .. }, false) => writeln!(out, "no hooks of amx's in {path}")?,
            }
        }
    }

    if state_root.exists() {
        std::fs::remove_dir_all(state_root)
            .with_context(|| format!("removing {}", state_root.display()))?;
        writeln!(out, "removed {}", state_root.display())?;
    }
    Ok(exit::OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Agent, Meta, Phase};
    use crate::tmux::{PaneId, Server, Socket, Spawn};
    use crate::vendor::claude;
    use serde_json::Value;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    /// A private tmux server, killed on drop.
    struct TestServer(Server);

    impl TestServer {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let tag = format!(
                "amx-test-uninstall-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            Self(Server::named(tag).with_conf("/dev/null"))
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            let _ = self.0.kill();
        }
    }

    fn record(
        root: &Path,
        id: &str,
        vendor: Option<&str>,
        phase: Phase,
        socket: Socket,
        pane: PaneId,
    ) -> Agent {
        let agent = Agent::create(
            root,
            &Meta {
                role: None,
                parent: None,
                depth: 0,
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                agent: vendor.map(str::to_string),
                model: None,
                effort: None,
                dir: PathBuf::from("/srv/app"),
                worktree: None,
                branch: None,
                base: None,
                socket,
                pane,
                bg: false,
                session: None,
                transcript: None,
                created: 1,
            },
        )
        .unwrap();
        agent
            .writer()
            .unwrap()
            .update_state(|s| s.state = phase)
            .unwrap();
        agent
    }

    /// Install amx's claude plugin under `home`, returning its directory.
    fn plugin_with_amx(home: &Path) -> PathBuf {
        let dir = install::wire_path(&claude::HOOKS.wire, home, &install::no_env);
        install::install_wire(&claude::HOOKS.wire, home, &install::no_env, 1).unwrap();
        dir
    }

    /// Whether `dir` holds a manifest naming amx, which is how claude finds it.
    fn plugin_is_there(dir: &Path) -> bool {
        std::fs::read_to_string(dir.join(install::MANIFEST))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .is_some_and(|manifest| manifest["name"] == "amx")
    }

    #[test]
    fn uninstall_refuses_while_an_agent_is_still_running() {
        // Anything unended with a live pane counts, idle agents and commands
        // included: deleting their records loses their output.
        let home = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let plugin = plugin_with_amx(home.path());

        let server = TestServer::new();
        // Named the way spawn::place names sessions, so the pane answers for
        // the agent.
        let pane_for = |id: &str| {
            server
                .0
                .new_session(&Spawn {
                    name: Some(&format!("{}{id}", crate::tmux::SESSION_PREFIX)),
                    command: &["sh", "-c", "while :; do sleep 0.05; done"],
                    ..Spawn::default()
                })
                .unwrap()
                .1
        };

        // A shell command (no vendor) and an idle agent.
        for (id, vendor, phase) in [
            ("watch-log-a1b", None, Phase::Working),
            ("port-it-b2c", Some("claude"), Phase::Idle),
        ] {
            let pane = pane_for(id);
            record(
                root.path(),
                id,
                vendor,
                phase,
                server.0.socket().clone(),
                pane,
            );
        }

        let mut said = Vec::new();
        let code = run(root.path(), home.path(), &install::no_env, &mut said).unwrap();

        assert_eq!(code, exit::FAILURE);
        let said = String::from_utf8(said).unwrap();
        assert!(
            said.contains("watch-log-a1b") && said.contains("port-it-b2c"),
            "the refusal names everything still there: {said}"
        );
        assert!(root.path().join("watch-log-a1b").exists(), "records kept");
        assert!(plugin_is_there(&plugin), "and the hooks are still wired");
    }

    #[test]
    fn uninstall_takes_the_hooks_out_and_the_records_with_them() {
        let home = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let plugin = plugin_with_amx(home.path());

        record(
            root.path(),
            "fix-login-a1b",
            Some("claude"),
            Phase::Done,
            Socket::Name("amx".to_string()),
            PaneId::new("%1").unwrap(),
        );

        let mut said = Vec::new();
        let code = run(root.path(), home.path(), &install::no_env, &mut said).unwrap();

        assert_eq!(code, exit::OK);
        assert!(!root.path().exists(), "the records are gone");
        assert!(!plugin_is_there(&plugin), "and so are the hooks");
        assert!(
            !home.path().join(".claude/settings.json").exists(),
            "no settings file of anybody's was written, so none was restored"
        );
    }

    #[test]
    fn uninstall_refuses_a_tmux_that_cannot_be_asked_and_removes_nothing() {
        // An unanswered tmux may hide a running agent, and a deleted record is
        // gone for good.
        let home = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let plugin = plugin_with_amx(home.path());
        record(
            root.path(),
            "fix-login-a1b",
            Some("claude"),
            Phase::Working,
            crate::tmux::unaskable(),
            PaneId::new("%3").unwrap(),
        );

        let mut said = Vec::new();
        let why = run(root.path(), home.path(), &install::no_env, &mut said).unwrap_err();

        assert!(
            format!("{why:#}").starts_with("tmux could not be asked: "),
            "{why:#}"
        );
        assert!(
            root.path().join("fix-login-a1b").is_dir(),
            "the record stays"
        );
        assert!(plugin_is_there(&plugin), "and so do the hooks");
    }

    #[test]
    fn uninstall_does_not_wait_on_an_agent_whose_pane_is_gone() {
        // A record left `working` by a reboot is not a running agent.
        let home = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        plugin_with_amx(home.path());

        record(
            root.path(),
            "fix-login-a1b",
            Some("claude"),
            Phase::Working,
            Socket::Name("amx-test-no-such-server".to_string()),
            PaneId::new("%404").unwrap(),
        );

        let mut said = Vec::new();
        assert_eq!(
            run(root.path(), home.path(), &install::no_env, &mut said).unwrap(),
            exit::OK,
            "{}",
            String::from_utf8_lossy(&said)
        );
        assert!(!root.path().exists());
    }

    #[test]
    fn uninstall_says_so_when_there_was_nothing_of_amxs_to_remove() {
        let home = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let dir = install::wire_path(&claude::HOOKS.wire, home.path(), &install::no_env);
        std::fs::create_dir_all(&dir).unwrap();
        let theirs = "---\nname: amx\n---\n\ntheir own copy\n";
        std::fs::write(dir.join("SKILL.md"), theirs).unwrap();
        std::fs::remove_dir_all(root.path()).unwrap();

        let mut said = Vec::new();
        assert_eq!(
            run(root.path(), home.path(), &install::no_env, &mut said).unwrap(),
            exit::OK
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("SKILL.md")).unwrap(),
            theirs,
            "a directory amx never wrote a manifest into stays untouched"
        );
        assert!(String::from_utf8(said).unwrap().contains("no plugin"));
    }
}
