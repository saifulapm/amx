//! An isolated machine for a binary at an **arbitrary path**.
//!
//! [`super::env::Env`] runs `CARGO_BIN_EXE_amx` where it stands, which is all
//! most suites need. Three things in M3 need more than that, and all three need
//! the same more:
//!
//! - `update.rs` installs *over* the binary it is running, so it must run a
//!   copy — and the copy's path is the fixture, because `current_exe()` is how
//!   `amx update apply` decides whether it is looking at a Homebrew tree.
//! - `bridge.rs` seeds a binary onto a "remote" and then compares bytes.
//! - the worktree flow wants a binary somewhere a repo can sit beside.
//!
//! W10 wrote this inline in `update.rs` and recorded that W11 and W12 would
//! both want it; W11 lifted it here on arrival, which is also the split
//! `support/mod.rs` had been owed since it passed the soft budget.
//!
//! A copy and never a symlink: `current_exe()` resolves symlinks, so a
//! symlinked fixture would report the path in `target/debug` and every
//! package-manager shape would evaporate.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use super::env::{Output, TempDir, assert_sun_path_fits, roots_on, spawn_on_tty, wait_until};
use super::tty::Terminal;

/// A machine with its own roots, and somewhere to put binaries.
pub struct Rig {
    dir: TempDir,
    /// The session name every command in this rig uses.
    pub session: String,
    server: Option<Child>,
}

impl Rig {
    /// A fresh machine whose session name is unique to it.
    pub fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        assert_sun_path_fits(dir.path(), tag);
        for sub in ["run", "state", "config/amx"] {
            fs::create_dir_all(dir.path().join(sub)).expect("create a root");
        }
        Self {
            dir,
            session: tag.to_owned(),
            server: None,
        }
    }

    /// This rig's temp root.
    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    /// This rig's session socket, once something binds it.
    pub fn socket(&self) -> PathBuf {
        self.root()
            .join("run")
            .join("amx")
            .join(&self.session)
            .join("sock")
    }

    /// Where `amx update apply` stages a download.
    pub fn staging(&self) -> PathBuf {
        self.root()
            .join("state")
            .join("amx")
            .join(&self.session)
            .join("update")
    }

    /// Put a working copy of the binary under test at `rel`.
    pub fn plant(&self, rel: &str) -> PathBuf {
        let path = self.root().join(rel);
        fs::create_dir_all(path.parent().expect("a directory")).expect("create the bin directory");
        let source = PathBuf::from(env!("CARGO_BIN_EXE_amx"));
        // A copy and never a hard link, however cheap the link would be: a link
        // hands the plant the build artifact's own inode, and then the two
        // alias. `amx update apply` renames over a planted path, and cargo
        // rewrites `target/debug/amx` whenever it relinks — and while that
        // write is open, every concurrent spawn of a planted path fails with
        // ETXTBSY. A few megabytes buys a plant that is nobody else's file.
        fs::copy(&source, &path).expect("copy the binary under test");
        path
    }

    /// Write the config file, pointing the update channel at `url`.
    pub fn channel(&self, url: &str) {
        fs::write(
            self.root().join("config/amx/config.toml"),
            format!("[update]\nchannel = \"{url}\"\n"),
        )
        .expect("write config.toml");
    }

    /// A command running `exe` with this rig's roots.
    pub fn command(&self, exe: &Path) -> Command {
        let mut command = Command::new(exe);
        roots_on(&mut command, self.root(), &self.session);
        command
    }

    /// Run `exe` with this rig's roots.
    pub fn run(&self, exe: &Path, args: &[&str]) -> Output {
        let out = self
            .command(exe)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run amx");
        Output::of(&out)
    }

    /// Run `exe` on a fresh pseudoterminal of this size.
    pub fn run_on_tty(&self, exe: &Path, args: &[&str], rows: u16, cols: u16) -> Terminal {
        spawn_on_tty(&mut self.command(exe), args, rows, cols)
    }

    /// Start a server from `exe` and wait until it answers.
    pub fn serve(&mut self, exe: &Path) -> u32 {
        let child = self
            .command(exe)
            .arg("server")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the server");
        let pid = child.id();
        self.server = Some(child);
        let socket = self.socket();
        wait_until("the server answers its socket", || {
            amx_server::session::probe::probe(&socket).is_ok_and(|state| state.is_running())
        });
        pid
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(mut server) = self.server.take() {
            let _ = server.kill();
            let _ = server.wait();
        }
    }
}

/// What a finished invocation did.
///
/// Kept as a name of its own because `update.rs` reads as `Done` throughout;
/// the type behind it is [`Output`], which every other suite already uses.
pub type Done = Output;
