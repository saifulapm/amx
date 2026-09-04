//! `amx uninstall` — take amx back out of the machine.
//!
//! The hooks come out of the vendor's settings and the agents' records are
//! deleted. It refuses while any agent is still running: those agents would
//! keep working with nothing recording what they do, and their records would
//! be the only place their answers were kept.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;

use crate::{exit, install, paths, store};

/// Run the verb against the machine's own paths.
pub fn from_env() -> Result<i32> {
    let state_root = paths::state_root()?;
    let settings = install::settings_file()?;
    let mut out = std::io::stdout().lock();
    run(&state_root, &settings, store::now(), &mut out)
}

/// Run the verb, with everything it touches named.
pub fn run(state_root: &Path, settings: &Path, now: u64, out: &mut impl Write) -> Result<i32> {
    let live = crate::spawn::live(state_root)?;
    if !live.is_empty() {
        writeln!(
            out,
            "still running: {}. stop them first, or their answers go with the records.",
            live.join(", ")
        )?;
        return Ok(exit::FAILURE);
    }

    let report = install::uninstall(settings, now)?;
    if report.changed {
        writeln!(out, "took the hooks out of {}", report.path.display())?;
    } else {
        writeln!(out, "no hooks of amx's in {}", report.path.display())?;
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
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    /// A private tmux server that goes when the test does.
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

    fn record(root: &Path, id: &str, phase: Phase, socket: Socket, pane: PaneId) -> Agent {
        let agent = Agent::create(
            root,
            &Meta {
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                agent: None,
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

    fn settings_with_amx(dir: &Path) -> PathBuf {
        let path = dir.join("settings.json");
        std::fs::write(&path, "{\"model\": \"opus\"}\n").unwrap();
        install::install(&path, "/home/dev/bin/amx _hook", 1).unwrap();
        path
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn uninstall_refuses_while_an_agent_is_still_running() {
        let home = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let settings = settings_with_amx(home.path());

        let server = TestServer::new();
        let (_, pane) = server
            .0
            .new_session(&Spawn {
                command: &["sh", "-c", "while :; do sleep 0.05; done"],
                ..Spawn::default()
            })
            .unwrap();
        record(
            root.path(),
            "fix-login-a1b",
            Phase::Working,
            server.0.socket().clone(),
            pane,
        );

        let mut said = Vec::new();
        let code = run(root.path(), &settings, 2, &mut said).unwrap();

        assert_eq!(code, exit::FAILURE);
        assert!(
            String::from_utf8(said).unwrap().contains("fix-login-a1b"),
            "the refusal names who is still working"
        );
        assert!(root.path().join("fix-login-a1b").exists(), "records kept");
        assert!(
            !install::installed_events(&read(&settings), "/home/dev/bin/amx _hook").is_empty(),
            "and the hooks are still wired"
        );
    }

    #[test]
    fn uninstall_takes_the_hooks_out_and_the_records_with_them() {
        let home = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let settings = settings_with_amx(home.path());

        record(
            root.path(),
            "fix-login-a1b",
            Phase::Done,
            Socket::Name("amx".to_string()),
            PaneId::new("%1").unwrap(),
        );

        let mut said = Vec::new();
        let code = run(root.path(), &settings, 2, &mut said).unwrap();

        assert_eq!(code, exit::OK);
        assert!(!root.path().exists(), "the records are gone");
        assert!(
            install::installed_events(&read(&settings), "/home/dev/bin/amx _hook").is_empty(),
            "and so are the hooks"
        );
        assert_eq!(read(&settings)["model"], "opus", "the rest is left alone");
    }

    #[test]
    fn uninstall_does_not_wait_on_an_agent_whose_pane_is_gone() {
        // A record left saying `working` after a reboot is not a running
        // agent, and must not block a person from removing amx.
        let home = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let settings = settings_with_amx(home.path());

        record(
            root.path(),
            "fix-login-a1b",
            Phase::Working,
            Socket::Name("amx-test-no-such-server".to_string()),
            PaneId::new("%404").unwrap(),
        );

        let mut said = Vec::new();
        assert_eq!(
            run(root.path(), &settings, 2, &mut said).unwrap(),
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
        let settings = home.path().join("settings.json");
        std::fs::write(&settings, "{\"model\": \"opus\"}\n").unwrap();
        std::fs::remove_dir_all(root.path()).unwrap();

        let mut said = Vec::new();
        assert_eq!(run(root.path(), &settings, 2, &mut said).unwrap(), exit::OK);
        assert_eq!(
            std::fs::read_to_string(&settings).unwrap(),
            "{\"model\": \"opus\"}\n",
            "an untouched file stays untouched"
        );
        assert!(String::from_utf8(said).unwrap().contains("no hooks"));
    }

    #[test]
    fn uninstall_leaves_the_settings_amx_never_wrote_to() {
        let home = TempDir::new().unwrap();
        let settings = home.path().join("settings.json");
        std::fs::write(&settings, json!({"hooks": {}}).to_string()).unwrap();
        let root = TempDir::new().unwrap();

        let mut said = Vec::new();
        assert_eq!(run(root.path(), &settings, 2, &mut said).unwrap(), exit::OK);
    }
}
