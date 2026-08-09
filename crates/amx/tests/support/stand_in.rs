//! Scripted stand-in agents, for the suites that need a session with real
//! statuses in it.
//!
//! `amx agents` renders whatever `agent.list` answers, and `agent.list` only
//! lists a pane the hub has committed a status for — so a table with rows in it
//! needs an agent that actually transitions. Real agent tooling does not exist
//! on CI runners (R-M2-8b), so the agents here are POSIX-sh scripts, and
//! everything around them is the shipped product: the real binary, the real
//! socket, the real registry override path, and **the shipped `claude.toml`
//! rules**.
//!
//! # What is real and what stands in
//!
//! | Layer | Here |
//! |---|---|
//! | the agent process | **stand-in** — a shell script that paints one screen |
//! | the screen text | **stand-in text, real phrases** — the words the shipped rules key on |
//! | the screen rules | **real** — [`MANIFEST`] is `crates/amx-server/assets/manifests/claude.toml` |
//! | the registry | **real path** — a stanza in `$XDG_CONFIG_HOME/amx/agents.toml`, D-M2-2's test seam |
//! | fusion, the hub, `agent.list` | **real** — nothing below the script is stubbed |
//!
//! `tests/support/agent.rs` in the workspace rig is the richer version of this,
//! with hooks and thirteen scripted transitions. It cannot be reached from
//! here — it belongs to a different package — and this suite does not need it:
//! what `amx agents` has to get right is what a *status* renders as, not how
//! the status was arrived at.
//!
//! # Why the scripts repaint
//!
//! A screen nobody touches goes stale, and fusion leaves a held state when it
//! does — correctly, and with no `reason`, since nothing named the exit. Under
//! `nproc`-wide load a suite can take longer than that, so each script repaints
//! its screen every couple of seconds. A re-assertion of the state a pane
//! already holds corroborates it without moving it (X06's wave outcome), so the
//! repaint keeps the screen fresh without inventing a transition.

use std::path::{Path, PathBuf};

use super::env::Env;

/// The manifest every stanza names: the shipped Claude Code rules.
pub const MANIFEST: &str = "claude.toml";

/// A stand-in that paints the permission dialog: `blocked`, by the shipped
/// `permission_dialog` rule.
pub const BLOCKED: &str = "blocked";
/// A stand-in that paints the footer's interrupt hint: `working`, by
/// `footer_interrupt_hint_working`.
pub const WORKING: &str = "working";
/// A stand-in that paints the composer between two rules: `idle`, by
/// `prompt_box_idle`.
pub const IDLE: &str = "idle";

/// The last non-empty line each stand-in leaves on its screen, which is exactly
/// what `agent.list` reports as that pane's `last_line`.
pub const BLOCKED_LAST_LINE: &str = "2. No, and tell Claude what to do differently (esc)";
/// See [`BLOCKED_LAST_LINE`].
pub const WORKING_LAST_LINE: &str = "esc to interrupt";

/// The scripts, installed and registered.
pub struct StandIns {
    dir: PathBuf,
}

impl StandIns {
    /// Write the three scripts and the registry override they are named in.
    ///
    /// Called **before** the server starts: the hub parses the registry
    /// override once, at assembly, which is the production path and not a back
    /// door (D-M2-2).
    pub fn install(env: &Env) -> Self {
        let dir = env.root().join("agents");
        std::fs::create_dir_all(&dir).expect("create the stand-ins' directory");
        for (kind, screen) in [
            (BLOCKED, DIALOG),
            (WORKING, INTERRUPT_HINT),
            (IDLE, PROMPT_BOX),
        ] {
            let path = dir.join(kind);
            std::fs::write(&path, script(screen)).expect("write a stand-in");
            make_executable(&path);
        }
        plant(&dir, &env.root().join("config/amx/agents.toml"));
        Self { dir }
    }

    /// Where the scripts live, for a test that wants to name one.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// One stand-in: paint the screen, then keep painting it.
fn script(screen: &str) -> String {
    format!(
        "#!/bin/sh\n\
         # A stand-in agent for the `amx agents` suite. It is a screen and a\n\
         # loop; `tests/support/stand_in.rs` says what stands in for what.\n\
         while true; do\n\
         \tprintf '\\033[2J\\033[H'\n\
         {screen}\
         \tsleep 2\n\
         done\n"
    )
}

/// The permission dialog, in the words the shipped `permission_dialog` rule
/// matches: the question, and one of the three answers it accepts beside it.
const DIALOG: &str = "\techo 'Bash(git push origin main)'\n\
                      \techo\n\
                      \techo 'Do you want to proceed?'\n\
                      \techo '❯ 1. Yes'\n\
                      \techo '2. No, and tell Claude what to do differently (esc)'\n";

/// The footer hint that is on screen for the whole of a turn and none of the
/// rest of the time, which is what `footer_interrupt_hint_working` reads.
const INTERRUPT_HINT: &str = "\techo 'Running cargo test --workspace'\n\
                              \techo\n\
                              \techo 'esc to interrupt'\n";

/// The composer between two full-width rules, which is `prompt_box_idle`'s
/// three anchors.
const PROMPT_BOX: &str = "\techo '────────────────────────────────────────'\n\
                          \techo '❯ Try \"refactor <filepath>\"'\n\
                          \techo '────────────────────────────────────────'\n";

/// Write the registry override that names the three scripts.
///
/// `coverage = "identity"` because these agents emit no hooks at all: the
/// screen is the whole of their state, which is exactly what that class means
/// (`CoverageClass`, `amx-core/src/agent/status.rs`). `startup_grace_ms = 0`
/// because there is no startup for a suite to wait out.
fn plant(scripts: &Path, config: &Path) {
    let dir = config.parent().expect("a config directory");
    std::fs::create_dir_all(dir).expect("create the config directory");
    let mut text = String::from(
        "# The `amx agents` suite's stand-ins. `tests/support/stand_in.rs` says\n\
         # which parts of them are real and which stand in.\n",
    );
    for kind in [BLOCKED, WORKING, IDLE] {
        text.push_str(&format!(
            "\n[[agent]]\n\
             id          = \"{kind}\"\n\
             label       = \"{kind} stand-in\"\n\
             executables = [\"{kind}\"]\n\
             coverage    = \"identity\"\n\
             start       = [\"{}\"]\n\
             manifest    = \"{MANIFEST}\"\n\
             startup_grace_ms = 0\n",
            scripts.join(kind).display(),
        ));
    }
    std::fs::write(config, text).expect("write the registry override");
}

fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let mut perms = std::fs::metadata(path)
        .expect("stat a stand-in")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod a stand-in");
}
