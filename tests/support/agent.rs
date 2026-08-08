//! Scripted stand-in agents, anchored to the V01 spike's recordings.
//!
//! The M2 exit criterion is about five agents in one session, and CI runners
//! have no agent tooling on them (R-M2-8b). So the agents here are POSIX-sh
//! scripts — the `MarkerShell` tradition — and everything *around* them is the
//! shipped product: the real `amx` binary, the real session socket, the real
//! `amx _hook` emitter, the real registry override path, and the manifest
//! Claude Code's own stanza names.
//!
//! # What is real and what stands in
//!
//! | Layer | In this rig |
//! |---|---|
//! | the agent process | **stand-in** — a shell script driven by typed commands |
//! | the hook payloads | **real shapes** — the JSON of V01 §2's "Payload shapes, as recorded", field for field |
//! | the hook transport | **real** — the script pipes into `amx _hook fake`, the shipped emitter, over the pane's own socket |
//! | the screens | **stand-in text, real strings** — compact blocks built from the exact phrases V01 recorded (`esc to interrupt`, `Do you want to proceed?`, `Interrupted · What should Claude do instead?`, the composer's `❯` between two rules) |
//! | the screen rules | **real** — [`MANIFEST`] is the shipped `claude.toml`, not a test manifest |
//! | the registry | **real path** — a stanza in `$XDG_CONFIG_HOME/amx/agents.toml`, the override merge D-M2-2 calls "the test seam" |
//! | fusion, the hub, waits, resume | **real** — nothing here is stubbed below the script |
//!
//! The screens are compact rather than the 100×30 captures under
//! `crates/amx-server/tests/fixtures/manifest/` for one reason: those are
//! asserted against the shipped rules by `crates/amx-server/tests/manifest.rs`
//! at their recorded geometry, and a pane in a five-agent session is not that
//! geometry. What travels here is the *text the rules key on*, which is what a
//! pane of any width has to get right.
//!
//! # The hook silences are the point
//!
//! V01 §7 lists thirteen transitions the exit test must replay, and the ones
//! that matter emit **nothing at all**: Esc during generation, Esc during a
//! tool call, a permission dialog answered "No", the same dialog cancelled with
//! Esc. So [`ESC`] and [`CANCEL`] below deliberately call no hook — the fake
//! must reproduce the silence as faithfully as the traffic, or the exit test
//! proves the easy half of fusion and none of the hard half.

use std::path::{Path, PathBuf};

use crate::env::Env;

/// The registry id (and `_hook` argument) of the scripted agent.
pub const KIND: &str = "fake";

/// The manifest the stanza names: **the shipped Claude Code rules**.
///
/// Not a manifest written for the fixtures. The screens below carry the strings
/// V01 recorded from Claude Code 2.1.224, so the rules that read a real agent
/// are the rules under test here, and a change to them that stops matching the
/// real UI fails this suite as well as `tests/manifest.rs`.
pub const MANIFEST: &str = "claude.toml";

/// Submit a prompt: `UserPromptSubmit`, then the working screen.
pub const PROMPT: &str = "prompt";
/// Start a tool call: `PreToolUse`, then the working screen.
pub const TOOL: &str = "tool";
/// Ask for permission: `PreToolUse` → `PermissionRequest`, then the dialog.
pub const ASK: &str = "ask";
/// End the turn the way the agent does: `Stop`, then the prompt box.
pub const STOP: &str = "stop";
/// Esc mid-turn — **no hook of any kind**, the interrupted screen (V01 §7.1–2).
pub const ESC: &str = "esc";
/// Answer the dialog "No", or cancel it with Esc — **no hook of any kind**,
/// and byte-identical either way, because V01 §7.4 measured them
/// indistinguishable and nothing downstream may pretend otherwise.
pub const CANCEL: &str = "cancel";
/// A nested subagent turn: `SubagentStart` → `SubagentStop`, both carrying an
/// `agent_id` (V01 §7.6).
pub const SUBAGENT: &str = "subagent";
/// The anonymous `SubagentStop` that lands 1.9–3.0 s after the parent's `Stop`
/// on essentially every tool-using turn (V01 §7.5).
pub const LATE_SUBAGENT: &str = "late";
/// Paint a distinctively styled sentinel above the idle box: `sentinel <tag>`.
///
/// M3's exit criterion asks for "no visible screen content lost" across a live
/// upgrade, and reads it strictly (§7 step 4): a *styled* sentinel — colours
/// and attributes, not just text — painted in every pane, compared cell for
/// cell either side of the swap. Bold, underlined, a 24-bit foreground and a
/// 24-bit background, which is four different things the grid synthesizer has
/// to put back and the widest SGR run a single cell can carry short of the
/// parameter overflow W04 found.
///
/// It rides above a full idle box rather than replacing one, so a pane wearing
/// a sentinel is still an idle agent as far as the shipped rules are concerned.
pub const SENTINEL: &str = "sentinel";

/// The text a [`SENTINEL`] paint puts on the screen, before its tag.
pub const SENTINEL_TEXT: &str = "AMX-SENTINEL";

/// The SGR parameters [`SENTINEL`] paints with, as the escape carries them.
pub const SENTINEL_SGR: &str = "1;4;38;2;255;90;30;48;2;10;20;60";

/// Leave, closing the pane.
pub const QUIT: &str = "quit";

/// The phrase the working screen carries, and the footer rule keys on.
pub const WORKING_TEXT: &str = "esc to interrupt";
/// The phrase the permission dialog carries.
pub const BLOCKED_TEXT: &str = "Do you want to proceed?";
/// The dialog's *last* line, and the second anchor `permission_dialog` needs.
///
/// The shipped rule requires the question **and** one of the option lines under
/// it (`1. Yes`, `2. No`, or this), so a screen carrying only [`BLOCKED_TEXT`]
/// is a half-painted dialog that matches no rule at all — the question is on
/// screen and `prompt_box_idle`'s `not` clause refuses it, so `agent explain`
/// answers `matched: null`. A waiter that stops at the question is therefore
/// waiting for the wrong fact; this is the line that says the paint is whole,
/// because the script below writes it last.
pub const BLOCKED_TAIL_TEXT: &str = "(esc to cancel)";
/// What Claude Code paints where an interrupted tool call was.
pub const INTERRUPTED_TEXT: &str = "Interrupted · What should Claude do instead?";
/// A phrase only the prompt box carries, for waiting on a painted idle screen.
///
/// The rules match the box *structurally* — a `❯` composer between two full
/// width rules — and a test that waited on those anchors would be asserting the
/// rule rather than the paint. This is the footer beside them.
pub const IDLE_TEXT: &str = "? for shortcuts";

/// The scripted agents installed into one [`Env`].
#[derive(Debug)]
pub struct FakeAgents {
    dir: PathBuf,
    script: PathBuf,
}

impl FakeAgents {
    /// Write the script, the registry stanza, and the environment they need.
    ///
    /// Called before the server starts, because the hub parses the registry
    /// override once at assembly — which is the production path, not a back
    /// door (D-M2-2).
    pub fn install(env: &mut Env) -> Self {
        let dir = env.scratch().join("agents");
        std::fs::create_dir_all(&dir).expect("create the fake agents' directory");
        let script = dir.join(KIND);
        std::fs::write(&script, SCRIPT).expect("write the fake agent");
        make_executable(&script);

        plant_stanza(&env.config_path(), &script);
        // Read by the script, which is a pane child and therefore inherits the
        // server's environment. `AMX_SOCKET` and friends come from the spawn
        // itself (V07); these two are the rig's own.
        env.set_var("AMX_RIG_AMX", &env.exe().to_string_lossy());
        env.set_var("AMX_RIG_DIR", &dir.to_string_lossy());
        Self { dir, script }
    }

    /// The program the stanza starts, as the registry spells it.
    pub fn program(&self) -> &Path {
        &self.script
    }

    /// Every `--resume <ref>` a restarted agent recorded, in no order.
    ///
    /// One line per invocation: the restored pane is a **shell**, amx types the
    /// planned argv into it (D-M2-7's "launch is type-in, not exec"), the shell
    /// runs the script, and the script writes down the conversation it was
    /// handed. Nothing about that path is faked, which is why the file is the
    /// evidence the exit test wants.
    pub fn resumed(&self) -> Vec<String> {
        let text = std::fs::read_to_string(self.dir.join("resumed")).unwrap_or_default();
        text.lines().map(str::to_owned).collect()
    }

    /// The conversation ref a pane's agent reports, derived the way the script
    /// derives it.
    ///
    /// Per-pane, because five agents in one session must be five conversations
    /// — the exit criterion is that *every one* of them resumes, which a shared
    /// ref would make untestable (and which the dedupe reservation would refuse
    /// anyway, correctly).
    pub fn conversation(pane: &str) -> String {
        format!("conv-{pane}")
    }
}

/// Plant the registry override the fake agent arrives through.
///
/// `startup_grace_ms = 0`: the grace is the window in which fusion refuses to
/// read the screen because "a booting TUI's splash matches nonsense", and a
/// script that paints a settled prompt box on its first line has no splash to
/// wait out. The two shipped stanzas keep theirs; this one states why it has
/// none.
fn plant_stanza(config_path: &Path, script: &Path) {
    let dir = config_path.parent().expect("a config directory");
    std::fs::create_dir_all(dir).expect("create the config directory");
    let text = format!(
        r#"
# The V17 exit suite's scripted agent. `tests/support/agent.rs` says which
# parts of it are real and which stand in.
[[agent]]
id          = "{KIND}"
aliases     = ["scripted"]
label       = "Scripted agent"
executables = ["{KIND}"]
coverage    = "edges"
start       = ["{script}"]
resume      = ["{script}", "--resume", "{{ref}}"]
ref_kind    = "id"
manifest    = "{MANIFEST}"
startup_grace_ms = 0
hook_events = [
  "SessionStart",
  "UserPromptSubmit",
  "Stop",
  "PreToolUse",
  "PermissionRequest",
  "SubagentStart",
  "SubagentStop",
]
"#,
        script = script.display(),
    );
    std::fs::write(dir.join("agents.toml"), text).expect("write the registry override");
}

fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let mut perms = std::fs::metadata(path)
        .expect("stat the fake agent")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod the fake agent");
}

/// The scripted agent.
///
/// Three things about its shape are deliberate:
///
/// - **`stty -echo`, not `stty raw`.** The commands arrive through `pane.run`,
///   which submits with `\r`; the line discipline's `ICRNL` turns that into the
///   `\n` a `read` needs, so the loop below blocks on a **line**, never on a
///   timer or a byte count. Turning echo off is what keeps the driver's words
///   off the screen the manifest is reading.
/// - **Every paint scrolls first.** A real TUI's state lives at the bottom of
///   the grid and the shipped rules read bottom-anchored regions
///   (`bottom_lines(6)`, `bottom_non_empty_lines(5)`), because herdr's
///   changelog records `whole_recent` matching a *stale* "esc to interrupt" in
///   scrollback as a production bug. Scrolling the previous block away is how a
///   script gets the same property.
/// - **The silent transitions call nothing.** `esc` and `cancel` paint and
///   return. That is the measurement, not an omission.
const SCRIPT: &str = r#"#!/bin/sh
# A stand-in agent for amx's M2 exit suite. See tests/support/agent.rs.
#
# Started as the pane's own process by `agent.start`; started again, after a
# restart, by the restored pane's shell typing `<this> --resume <ref>`.

dir=$AMX_RIG_DIR
conv=conv-$AMX_PANE_ID

if [ "$1" = "--resume" ] && [ -n "$2" ]; then
    printf '%s\n' "$2" >> "$dir/resumed"
    conv=$2
fi

stty -echo 2>/dev/null

# One hook invocation: the payload on stdin, the shipped emitter on the other
# end. Silent and ignored, exactly as an agent treats its own hooks.
hook() {
    printf '{"hook_event_name":"%s","session_id":"%s","transcript_path":"%s/%s.jsonl","cwd":"%s","permission_mode":"default"%s}' \
        "$1" "$conv" "$dir" "$conv" "$dir" "$2" \
        | "$AMX_RIG_AMX" _hook fake >/dev/null 2>&1
}

# Push whatever is on screen into scrollback, so the block that follows is the
# bottom of the grid and nothing stale is in a bottom-anchored region.
scroll() {
    i=0
    while [ "$i" -lt 60 ]; do
        printf '\n'
        i=$((i + 1))
    done
}

paint_working() {
    scroll
    printf '%s' '✻ Percolating… (2s · thinking)
──────────────────────────────────────
❯
──────────────────────────────────────
  ⏸ manual mode on · esc to interrupt · ← for agents'
}

paint_blocked() {
    scroll
    printf '%s' '● Bash(echo spike-permission-probe)
──────────────────────────────────────
  Do you want to proceed?
❯ 1. Yes
  2. No
  (esc to cancel)'
}

paint_idle() {
    scroll
    printf '%s' '✻ Worked for 3s
──────────────────────────────────────
❯
──────────────────────────────────────
  ⏸ manual mode on · ? for shortcuts'
}

paint_sentinel() {
    scroll
    printf '\033[1;4;38;2;255;90;30;48;2;10;20;60mAMX-SENTINEL %s\033[0m\n' "$1"
    printf '%s' '✻ Worked for 3s
──────────────────────────────────────
❯
──────────────────────────────────────
  ⏸ manual mode on · ? for shortcuts'
}

paint_interrupted() {
    scroll
    printf '%s' '● Bash(echo spike-permission-probe)
  ⎿  Interrupted · What should Claude do instead?
──────────────────────────────────────
❯
──────────────────────────────────────
  ⏸ manual mode on · ? for shortcuts'
}

hook SessionStart ',"source":"startup","model":"claude-opus-5"'
paint_idle

while read -r cmd rest; do
    case $cmd in
    prompt)
        hook UserPromptSubmit ',"prompt_id":"p-1"'
        paint_working
        ;;
    tool)
        hook PreToolUse ',"tool_name":"Bash"'
        paint_working
        ;;
    ask)
        hook PreToolUse ',"tool_name":"Bash"'
        hook PermissionRequest ',"tool_name":"Bash"'
        paint_blocked
        ;;
    stop)
        hook Stop ',"stop_hook_active":false'
        paint_idle
        ;;
    esc | cancel)
        # V01 §7.1–4: nothing fires. The screen is the only witness.
        paint_interrupted
        ;;
    subagent)
        hook SubagentStart ',"agent_id":"sub-1","agent_type":"general-purpose"'
        hook SubagentStop ',"agent_id":"sub-1","agent_type":"general-purpose"'
        ;;
    late)
        # The anonymous one: an agent_id, an empty agent_type, and a transcript
        # path that does not exist (V01 §3 M4).
        hook SubagentStop ',"agent_id":"anon-1","agent_type":""'
        ;;
    sentinel)
        paint_sentinel "$rest"
        ;;
    quit)
        exit 0
        ;;
    *)
        : "$rest"
        ;;
    esac
done
"#;
