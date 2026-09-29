//! The command line: every verb amx answers to.
//!
//! Doc comments on these derive types are the `--help` text. Bare `amx` opens
//! the view, or prints the table when stdout is not a terminal. The four
//! underscore verbs are what amx runs against itself from a pane, a vendor
//! hook or a tmux timer: hidden from help and completion, but still part of
//! the contract.

use crate::store::Phase;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(
    name = "amx",
    version,
    about = "Run coding agents as tmux panes",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Only the agents working under this directory.
    ///
    /// With no verb, the view (or the table, when stdout is not a terminal)
    /// lists only these agents, and a task typed in the view starts in this
    /// directory. `ls`, `allow` and `doctor` read it too; a verb's own `--dir`
    /// takes precedence.
    #[arg(long, value_name = "PATH")]
    pub dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    /// The verb as typed, or `None` for bare `amx`.
    pub fn verb(&self) -> Option<&'static str> {
        use Command::*;
        Some(match self.command.as_ref()? {
            New(_) => "new",
            Sub(_) => "sub",
            Ls { .. } => "ls",
            Status { .. } => "status",
            Send { .. } => "send",
            Answer { .. } => "answer",
            Interrupt { .. } => "interrupt",
            Rename { .. } => "rename",
            Allow { .. } => "allow",
            Result { .. } => "result",
            Wait { .. } => "wait",
            Attach { .. } => "attach",
            Logs { .. } => "logs",
            Stop(_) => "stop",
            Sweep { .. } => "sweep",
            Clear { .. } => "clear",
            Diff { .. } => "diff",
            Resume { .. } => "resume",
            Fork { .. } => "fork",
            Adopt(_) => "adopt",
            Events { .. } => "events",
            Statusline => "statusline",
            Doctor { .. } => "doctor",
            Setup { .. } => "setup",
            Uninstall => "uninstall",
            Completion { .. } => "completion",
            Hook => "_hook",
            Exit { .. } => "_exit",
            Boot { .. } => "_boot",
            Park { .. } => "_park",
        })
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start an agent on a task.
    New(NewArgs),

    /// Start a subagent on a task and wait for its answer.
    ///
    /// `amx new` and `amx result` in one call. The child records the agent
    /// whose pane it was started from as its parent, and by default runs in
    /// that agent's directory.
    Sub(SubArgs),

    /// List agents and their states.
    Ls {
        /// Print stable JSON instead of the table.
        #[arg(long)]
        json: bool,

        /// A directory filter: only the agents working under this directory.
        ///
        /// An agent belongs to a directory when it runs under it, and a
        /// worktree agent belongs to its repository wherever the tree is.
        /// Nothing is stored, and an agent appears under every directory that
        /// contains it.
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
    },

    /// Show one agent, and which signal its state came from.
    Status {
        id: String,
        /// Print stable JSON instead of the summary.
        #[arg(long)]
        json: bool,
    },

    /// Send a message to a working or idle agent.
    Send {
        id: String,

        /// The message.
        #[arg(
            required_unless_present = "file",
            conflicts_with = "file",
            value_parser = a_task
        )]
        text: Option<String>,

        /// Read the message from this file, or from stdin for `-`.
        ///
        /// The file is read whole, minus its final newline.
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
    },

    /// Answer a waiting agent's question: y, n, 1-9, 1,3, enter, esc, or words.
    ///
    /// A permission prompt or the folder-trust screen takes one key. A
    /// question the agent asks with a free-text field also takes your own
    /// words. A question that takes several choices (`.multi` in `amx status
    /// --json`) is answered with a list such as `1,3`.
    Answer {
        /// The waiting agent.
        id: String,
        #[command(flatten)]
        key: AnswerArgs,
    },

    /// Stop the turn an agent is in the middle of.
    ///
    /// Presses Escape in the pane: the turn ends and the agent returns to its
    /// prompt with the conversation intact. To dismiss a question, use `amx
    /// answer <id> esc`; to end a command row, use `amx stop`.
    Interrupt { id: String },

    /// Change the name an agent is listed under.
    ///
    /// Only the displayed name changes; the id, and the pane, branch and
    /// worktree named after it, stay as they are. `amx rename <id> <id>`
    /// restores the default name.
    Rename {
        /// The agent to rename.
        id: String,
        /// The new name.
        name: String,
    },

    /// Allow amx to use this project's `.amx/config.toml`.
    ///
    /// A project config can name the program a pane runs and shell commands
    /// amx runs, so amx ignores it until it is allowed. This prints the file
    /// and records exactly what it printed; any later edit, including one by
    /// an agent, has to be allowed again.
    Allow {
        /// The project directory, if not the current one.
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,

        /// Stop allowing it.
        #[arg(long)]
        forget: bool,
    },

    /// Wait for an agent's turn to end and print its answer.
    ///
    /// With `--children`, wait for every child of the named agent and print
    /// each answer in turn. `--json` keys them by child id, and a child
    /// stopped on a question is included with its question. The exit code is
    /// the most urgent outcome: 3 timed out, 2 a question, 1 a failure
    /// (including no children), 0 every answer in.
    Result {
        /// The agent to wait for.
        #[arg(required_unless_present = "children", conflicts_with = "children")]
        id: Option<String>,

        /// Wait for every child of this agent instead.
        #[arg(long, value_name = "ID")]
        children: Option<String>,

        /// Print one JSON object keyed by child id. Only with `--children`.
        #[arg(long, conflicts_with = "id")]
        json: bool,

        /// Give up after this many seconds.
        #[arg(long, value_name = "SECONDS")]
        timeout: Option<u64>,
    },

    /// Wait for several agents and say which are ready.
    ///
    /// Blocks until every named agent has settled (its turn over, or stopped
    /// on a question) and prints `<id> <state>` as each one does. `--any`
    /// returns at the first. Answers are not printed: `amx result <id>`
    /// prints one, at once for an agent that has already settled.
    ///
    /// `--for <state>` waits for that state instead, so `--for working`
    /// confirms a batch has started. `--children <id>` waits on every child
    /// of that agent, the fan-in after `amx sub --bg`; a parent with no
    /// children exits 1.
    Wait {
        /// The agents to wait on.
        #[arg(
            num_args = 1..,
            required_unless_present = "children",
            conflicts_with = "children"
        )]
        ids: Vec<String>,

        /// Wait on every child of this agent instead of on named ids.
        #[arg(long, value_name = "ID")]
        children: Option<String>,

        /// Return as soon as one of them has settled.
        #[arg(long)]
        any: bool,

        /// Wait for this state instead of for a finished turn.
        #[arg(long = "for", value_name = "STATE", value_parser = a_phase)]
        state: Option<Phase>,

        /// Give up after this many seconds.
        #[arg(long, value_name = "SECONDS")]
        timeout: Option<u64>,
    },

    /// Attach to an agent's pane.
    ///
    /// Without an id, the agent is picked by its place in the view's order,
    /// counting from the agent whose session this runs in. Meant for tmux
    /// key bindings.
    Attach {
        #[arg(
            required_unless_present_any = ["next", "prev", "waiting", "last"],
            conflicts_with_all = ["next", "prev", "waiting", "last"],
        )]
        id: Option<String>,

        /// The next agent in the view, wrapping at the end.
        #[arg(long, conflicts_with_all = ["prev", "waiting", "last"])]
        next: bool,

        /// The previous agent in the view, wrapping at the start.
        #[arg(long, conflicts_with_all = ["next", "waiting", "last"])]
        prev: bool,

        /// The first agent waiting on you.
        #[arg(long, conflicts_with_all = ["next", "prev", "last"])]
        waiting: bool,

        /// The agent you were attached to before this one.
        #[arg(long, conflicts_with_all = ["next", "prev", "waiting"])]
        last: bool,
    },

    /// Print an agent's recent output without attaching to it.
    ///
    /// Reads the agent's transcript where the vendor keeps one: prompts,
    /// answers and tool calls, whether or not the pane still exists. Without a
    /// transcript it prints the pane, including the scrollback tmux still
    /// holds, and once the pane is gone, the agent's last answer. `amx result`
    /// prints a turn's answer alone.
    Logs {
        id: String,
        /// How many lines to print.
        #[arg(
            long,
            value_name = "N",
            default_value_t = crate::verbs::logs::LINES,
            value_parser = clap::value_parser!(u32).range(1..),
        )]
        lines: u32,
    },

    /// Stop an agent and decide what happens to its worktree and branch.
    Stop(StopArgs),

    /// Remove finished agents whose work has landed.
    ///
    /// An agent qualifies when its pull request merged or closed, its branch
    /// was merged into the main line, or the origin no longer has its branch.
    /// Lists them with the reason, asks once, then removes each one's record,
    /// worktree and branch. A worktree with uncommitted work is kept, and so
    /// is its record.
    Sweep {
        /// Remove them without asking.
        #[arg(long)]
        force: bool,
    },

    /// Remove finished agents, whether or not their work landed.
    ///
    /// Lists every agent whose turn is over (done, failed or stopped) with the
    /// reason, asks once, then removes each one's record and worktree. Work
    /// that landed goes the way `sweep` removes it, branch included; other
    /// branches are kept. A worktree with uncommitted work is kept, and so is
    /// its record. An agent idle at its prompt is not finished and is not
    /// listed.
    Clear {
        /// Remove them without asking.
        #[arg(long)]
        force: bool,
    },

    /// Show an agent's worktree against the commit it started from.
    Diff {
        id: String,
        /// Print a diffstat instead of the patch.
        #[arg(long)]
        stat: bool,
        /// Compare against this ref instead of the recorded base.
        ///
        /// Any branch, tag or commit git resolves. For a record with no base,
        /// or to narrow the comparison.
        #[arg(long, value_name = "REF")]
        from: Option<String>,
    },

    /// Restart a stopped agent in its recorded session.
    ///
    /// A message becomes the restarted agent's first turn. It is passed on the
    /// vendor's command line, the way `new` passes a task, so the agent starts
    /// working at once.
    Resume {
        #[arg(required_unless_present = "all")]
        id: Option<String>,
        /// The restarted agent's first message. Without one it waits at its
        /// prompt.
        #[arg(value_parser = a_task, conflicts_with = "all")]
        message: Option<String>,
        /// Every stopped agent, for example after the tmux server died.
        #[arg(long, conflicts_with = "id")]
        all: bool,
    },

    /// Start a second agent on a copy of this one's conversation.
    ///
    /// The copy runs in the same directory with everything the original has
    /// been told so far, then goes its own way; neither agent affects the
    /// other. Only a recorded session can be copied, so an agent that never
    /// reported one cannot be forked.
    Fork {
        /// The agent whose conversation to copy.
        id: String,
        /// The copy's first task. Without one it opens the copied
        /// conversation and waits.
        #[arg(value_parser = a_task)]
        task: Option<String>,
    },

    /// Add the agent already running in this pane to amx.
    ///
    /// For an agent you started yourself, in your own tmux: it gets a record,
    /// an id and a row, and every verb works on it from then on.
    ///
    /// Run it inside the agent being adopted, which is how amx knows the pane
    /// and the conversation: ask the agent to run it, or use its shell mode.
    /// Nothing is started or sent.
    ///
    /// amx cut no worktree for it and has no command to relaunch it with, so
    /// `stop` only closes its pane, and `resume` and `fork` cannot restart it.
    Adopt(AdoptArgs),

    /// Print the agents' event streams, merged.
    Events {
        /// Agents to read; every agent when none are named.
        ids: Vec<String>,
        /// Keep printing as events arrive.
        #[arg(long, short)]
        follow: bool,
        /// Print one JSON object per event instead of the table.
        #[arg(long)]
        json: bool,
    },

    /// Print agent counts for a status line: ✽ moving, ⚠ waiting.
    ///
    /// Meant for tmux, as `status-right '#(amx statusline)'`. Prints plain
    /// text, and nothing at all when no agent needs mentioning.
    Statusline,

    /// Check what amx needs from this machine, and what is missing.
    ///
    /// Ten things are checked: tmux, the agent command, the config, amx's
    /// hooks for each installed agent, that the `amx` on the PATH is this
    /// one, the state directory, handoffs still carrying the spawner's
    /// environment, agents stopped at a vendor screen before their task,
    /// trust-store entries for worktrees that are gone, and id directories or
    /// worktrees with no record.
    ///
    /// When a tmux server is running, it also checks that the server's
    /// working directory still exists: a server that outlived it cannot start
    /// panes. As `amx --dir <path> doctor`, it checks whether an agent started
    /// there would stop at the vendor's folder-trust screen, without writing
    /// anything, and the exit code is the answer.
    ///
    /// `amx setup <agent>` wires an agent's hooks; doctor names any agent left
    /// unwired.
    Doctor {
        /// Repair what amx wrote itself.
        ///
        /// Cleans the environment out of old handoffs, drops trust-store
        /// entries for removed worktrees amx cut, removes id directories with
        /// no record once they are ten minutes old, and rebuilds agent clocks
        /// from their logs. It never removes a worktree.
        #[arg(long)]
        fix: bool,
    },

    /// Wire an agent's hooks so it reports to amx.
    ///
    /// `amx setup claude` and `amx setup opencode` write amx's plugin where
    /// that agent loads plugins, `amx setup pi` writes amx's extension where
    /// pi loads extensions, and `amx setup codex` merges amx's hooks into
    /// codex's `hooks.json` and trusts them in its `config.toml`. A file amx
    /// did not write is copied aside first, and `amx uninstall` puts it back.
    /// Without an agent, it lists the agents amx knows and writes nothing.
    Setup {
        /// The agent to wire: `claude`, `pi`, `codex` or `opencode`.
        vendor: Option<String>,

        /// Also install the agent's optional subagent tool.
        ///
        /// Only pi has one: a `subagent` tool that hands a task to `amx sub`.
        /// `amx uninstall` removes it. For an agent without one, setup says
        /// so and writes nothing else.
        #[arg(long)]
        subagent: bool,
    },

    /// Remove amx's hooks and state, restoring any file amx copied aside.
    ///
    /// Refuses while any agent is still running.
    Uninstall,

    /// Print the completion script for a shell.
    ///
    /// For example `amx completion fish > ~/.config/fish/completions/amx.fish`.
    /// The script covers this build's verbs and flags, so write it again after
    /// an upgrade.
    Completion {
        /// The shell to write the script for.
        shell: clap_complete::Shell,
    },

    /// Record one vendor hook event, read from stdin.
    #[command(name = "_hook", hide = true)]
    Hook,

    /// Record how the agent's command exited.
    #[command(name = "_exit", hide = true)]
    Exit { id: String, code: i32 },

    /// Start the agent's command inside its pane.
    #[command(name = "_boot", hide = true)]
    Boot { id: String },

    /// Close an idle agent's pane, keeping its record.
    #[command(name = "_park", hide = true)]
    Park { id: String },
}

#[derive(Debug, Args, Clone)]
pub struct NewArgs {
    /// What the agent should do.
    #[arg(
        value_parser = a_task,
        required_unless_present_any = ["file", "edit"],
        conflicts_with_all = ["file", "edit"]
    )]
    pub task: Option<String>,

    /// Read the task from this file, or from stdin for `-`.
    ///
    /// The file is read whole, minus its final newline.
    #[arg(long, value_name = "PATH")]
    pub file: Option<PathBuf>,

    /// Write the task in `$VISUAL`, `$EDITOR` or `vi` first.
    ///
    /// Opens an empty file, and what you save is the task. An empty file, or
    /// an editor that exits with an error, starts no agent.
    #[arg(long, conflicts_with = "file")]
    pub edit: bool,

    /// Name the agent instead of deriving a name from the task.
    #[arg(long)]
    pub name: Option<String>,

    /// Spawn with a role's settings and brief.
    ///
    /// A role is `~/.config/amx/agents/<name>.md`, or the project's
    /// `.amx/agents/<name>.md`, which takes precedence. Its settings are
    /// defaults that flags on this command line override, and its body goes
    /// in front of the task. An unknown name lists the roles available here.
    #[arg(long, value_name = "NAME")]
    pub role: Option<String>,

    /// Run in this directory instead of the current one.
    #[arg(long)]
    pub dir: Option<PathBuf>,

    /// Run in the directory itself, without a worktree of its own.
    #[arg(long)]
    pub no_worktree: bool,

    /// Cut the worktree from this ref instead of the current checkout.
    ///
    /// Any branch, tag or commit git resolves. The agent starts on it and
    /// `diff` compares against it. The `base` config key sets a default.
    #[arg(long, value_name = "REF")]
    pub base: Option<String>,

    /// Start the agent on this existing branch.
    ///
    /// A local branch is used as it stands, and one only the origin has is
    /// fetched first; a leading `origin/` is dropped. The commits land on the
    /// branch, and `stop` keeps it. Cannot be combined with `--base`, `--pr`,
    /// `--no-worktree` or `--exec`; `--with-changes` is allowed.
    #[arg(long, value_name = "NAME", conflicts_with_all = ["base", "pr", "no_worktree", "exec"])]
    pub branch: Option<String>,

    /// Start the agent on this pull request.
    ///
    /// Asks `gh` for the request's head branch, fetches it from the origin,
    /// and cuts the tree at the request's current commit. `stop` keeps the
    /// branch. Cannot be combined with `--base`, `--with-changes`,
    /// `--no-worktree` or `--exec`.
    #[arg(long, value_name = "N", conflicts_with_all = ["base", "with_changes", "no_worktree", "exec"])]
    pub pr: Option<u64>,

    /// Move this directory's uncommitted changes into the agent's worktree.
    ///
    /// Tracked changes, staged or not, and new files move; ignored files stay.
    /// This directory is left at its last commit. Cannot be combined with
    /// `--no-worktree` or `--exec`, and a directory with nothing uncommitted
    /// starts no agent.
    #[arg(long, conflicts_with_all = ["no_worktree", "exec"])]
    pub with_changes: bool,

    /// Run the task as a shell command instead of giving it to an agent.
    ///
    /// The whole task goes to `sh -c`, so a pipeline is one row, and the row
    /// ends done or failed by the command's exit code. It runs in the
    /// directory itself, without a worktree. `--agent`, `--model`,
    /// `--permission`, `--effort`, `--role` and arguments after `--` are
    /// refused.
    #[arg(long, conflicts_with_all = ["AgentArgs", "vendor_args", "role"])]
    pub exec: bool,

    /// The vendor and settings for this spawn; `None` when the caller named
    /// none and the config decides.
    #[command(flatten)]
    pub agent: Option<AgentArgs>,

    /// Arguments passed to the agent command verbatim.
    #[arg(last = true, value_name = "AGENT_ARGS")]
    pub vendor_args: Vec<String>,

    /// A preamble the vendor reads but the record does not keep, such as a
    /// subagent's digest of its parent (see [`SubArgs::context`]). Never set
    /// from the command line.
    #[arg(skip)]
    pub context_brief: Option<String>,

    /// The agent this one is a child of. Set only by `amx sub`: a plain `new`
    /// is a root whatever the pane's `AMX_ID` says. Never set from the command
    /// line.
    #[arg(skip)]
    pub parent: Option<String>,
}

#[derive(Debug, Args)]
pub struct SubArgs {
    /// What the child should do.
    #[arg(value_parser = a_task)]
    pub task: String,

    /// Give the child its own worktree instead of the parent's directory.
    ///
    /// By default a child runs where its parent runs and sees the parent's
    /// uncommitted files. This is for a child that will change things.
    #[arg(long)]
    pub worktree: bool,

    /// Run the child in this directory instead of the parent's.
    #[arg(long)]
    pub dir: Option<PathBuf>,

    /// Run the child in the directory itself, without a worktree of its own.
    ///
    /// A child started from a pane already does. This is for `amx sub` run
    /// outside any pane, which otherwise cuts a worktree like `amx new`.
    #[arg(long, conflicts_with = "worktree")]
    pub no_worktree: bool,

    /// Name the child instead of deriving a name from the task.
    #[arg(long)]
    pub name: Option<String>,

    /// Spawn the child with a role's settings and brief.
    ///
    /// The same role files as `amx new --role`. The role overrides what the
    /// parent passes down, and flags on this command line override both. An
    /// unknown name lists the roles available here.
    #[arg(long, value_name = "NAME")]
    pub role: Option<String>,

    /// Record this agent as the parent, instead of the pane's own.
    ///
    /// For a caller outside any pane, which has no `$AMX_ID`. The parent may
    /// have ended but must have a record; an unknown id exits 64 before
    /// anything is started.
    #[arg(long, value_name = "ID", conflicts_with = "no_parent")]
    pub parent: Option<String>,

    /// Record no parent, even when run inside an agent's pane.
    #[arg(long)]
    pub no_parent: bool,

    /// How much of the parent's context the child starts with.
    ///
    /// `fresh`, the default, is the child's own task alone. `digest` puts the
    /// parent's task and its latest answer in front of it; the child can
    /// still read the whole conversation with `amx logs $AMX_PARENT`.
    /// `digest` without a parent is refused.
    #[arg(long, value_name = "WHEN")]
    pub context: Option<Context>,

    /// Print one JSON object instead of the answer and the id.
    ///
    /// The object has `id`, `parent`, `phase`, `answer` and `evidence`.
    #[arg(long)]
    pub json: bool,

    /// Return as soon as the child's id is known, without waiting for it.
    #[arg(long)]
    pub bg: bool,

    /// Give up waiting for the answer after this many seconds.
    #[arg(long, value_name = "SECONDS")]
    pub timeout: Option<u64>,

    /// The vendor and settings for this child.
    ///
    /// The parent's model and effort are inherited when the child runs the
    /// same vendor, and anything named here wins. `--permission` is refused
    /// unless the `subagents_may_escalate` key allows it.
    #[command(flatten)]
    pub agent: Option<AgentArgs>,

    /// Arguments passed to the agent command verbatim.
    #[arg(last = true, value_name = "AGENT_ARGS")]
    pub vendor_args: Vec<String>,
}

#[derive(Debug, Args, Default)]
pub struct AdoptArgs {
    /// What the agent is working on, shown in its row.
    ///
    /// Only a label: nothing is sent to the agent. Without it, the row is
    /// named after the pane's directory.
    #[arg(long, value_name = "TEXT", value_parser = a_task)]
    pub task: Option<String>,

    /// Name the agent instead of deriving a name from the task.
    #[arg(long)]
    pub name: Option<String>,
}

/// The vendor for one spawn and the settings it launches with.
///
/// Each field falls back to the config, then to the vendor's own default.
/// These are amx's flags: anything after `--` goes to the vendor untouched,
/// and a setting whose flag already appears there is not passed a second time.
#[derive(Debug, Args, Clone, Default)]
pub struct AgentArgs {
    /// The agent command to run instead of the configured one.
    #[arg(long = "agent", value_name = "COMMAND")]
    pub command: Option<String>,

    /// The model to run the agent on.
    #[arg(long, value_name = "MODEL")]
    pub model: Option<String>,

    /// The permission mode to start the agent in.
    #[arg(long, value_name = "MODE")]
    pub permission: Option<String>,

    /// How much reasoning effort the agent spends.
    #[arg(long, value_name = "LEVEL")]
    pub effort: Option<String>,
}

/// What a question is answered with.
///
/// A bare `2` picks the second choice, while `--text 2` types the character
/// `2` into the question's free-text field; the two cannot be combined.
#[derive(Debug, Args, Default)]
pub struct AnswerArgs {
    /// A key of the grammar, a list of choices, or your own words.
    #[arg(
        value_name = "ANSWER",
        required_unless_present = "text",
        conflicts_with = "text"
    )]
    pub key: Option<String>,

    /// Words for the question's free-text field, taken literally.
    #[arg(long, value_name = "WORDS")]
    pub text: Option<String>,

    /// A note to send with the choice, where the question has a field for
    /// one.
    #[arg(long, value_name = "WORDS", conflicts_with = "text")]
    pub note: Option<String>,
}

#[derive(Debug, Args)]
pub struct StopArgs {
    pub id: String,

    /// Take the defaults for everything without asking.
    #[arg(long)]
    pub force: bool,

    /// Remove the agent's record too.
    #[arg(long)]
    pub delete: bool,

    /// What to do with the agent's worktree.
    #[arg(long, value_enum)]
    pub worktree: Option<Disposition>,

    /// What to do with the agent's branch.
    #[arg(long, value_enum)]
    pub branch: Option<Disposition>,
}

/// Refuse a task that is empty or only whitespace.
///
/// An empty prompt starts an agent with nothing to do that still holds a pane
/// and a worktree; `amx new "$TASK"` with `TASK` unset is the usual cause.
/// Anything else is passed on exactly as typed.
fn a_task(text: &str) -> Result<String, String> {
    match text.trim().is_empty() {
        true => Err("an agent needs something to do".to_string()),
        false => Ok(text.to_string()),
    }
}

/// Parse a state name for `wait --for`.
///
/// Only the eight words `amx ls --json` prints: any other word would be a wait
/// that never ends. The error lists all eight.
fn a_phase(word: &str) -> Result<Phase, String> {
    PHASES
        .into_iter()
        .find(|phase| phase.as_str() == word)
        .ok_or_else(|| {
            format!(
                "no state `{word}`: {}",
                PHASES.map(Phase::as_str).join(", ")
            )
        })
}

/// Every state, in the order a turn goes through them.
const PHASES: [Phase; 8] = [
    Phase::Starting,
    Phase::Working,
    Phase::Waiting,
    Phase::Idle,
    Phase::Done,
    Phase::Failed,
    Phase::Stopped,
    Phase::Unknown,
];

/// Read a task or message from a file, or from stdin when the path is `-`.
///
/// Shared by `new --file` and `send --file`. The text goes through [`a_text`],
/// so an empty file is refused like an empty argument.
pub fn text_of(path: &Path) -> Result<String, String> {
    let text = match path == Path::new("-") {
        true => std::io::read_to_string(std::io::stdin()).map_err(|e| format!("stdin: {e}")),
        false => std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display())),
    }?;
    a_text(&text)
}

/// Read text written outside the command line as a task: one trailing newline
/// removed, then [`a_task`].
///
/// Editors end a file with a newline nobody means as part of the text, and
/// `$(cat file)` would drop it too. Nothing else is trimmed. Also used for
/// what `new --edit` reads back from the editor.
pub fn a_text(text: &str) -> Result<String, String> {
    a_task(text.strip_suffix('\n').unwrap_or(text))
}

/// How much of its parent's context a child starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Context {
    /// The child's own task alone.
    Fresh,
    /// The parent's task and latest answer, in front of the child's task.
    Digest,
}

/// What happens to a worktree or a branch when its agent stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Disposition {
    Keep,
    Delete,
}

impl Disposition {
    pub fn is_keep(self) -> bool {
        self == Disposition::Keep
    }
}

/// The exit code for a command line clap refused.
///
/// `--help` and `--version` arrive as errors too and exit 0. Anything else is
/// a usage error, which never borrows the codes for an agent's outcome.
pub fn usage_exit_code(err: &clap::Error) -> i32 {
    use clap::error::ErrorKind;
    match err.kind() {
        ErrorKind::DisplayHelp
        | ErrorKind::DisplayVersion
        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => crate::exit::OK,
        _ => crate::exit::USAGE,
    }
}

/// The completion script for one shell, generated from the parser.
///
/// Rendered whole rather than streamed, so a shell never reads half a script.
pub fn completion_script(shell: clap_complete::Shell) -> Vec<u8> {
    let mut script = Vec::new();
    clap_complete::generate(shell, &mut public_surface(), "amx", &mut script);
    script
}

/// The command tree completion is generated from: what `amx --help` lists.
///
/// clap_complete emits hidden subcommands too, so the underscore verbs are
/// left out rather than marked hidden. The top-level flags, the version and
/// `disable_help_subcommand` are carried over; without the last, clap would
/// add a `help` verb amx does not answer to.
fn public_surface() -> clap::Command {
    use clap::CommandFactory;
    let full = Cli::command();
    let top = clap::Command::new("amx")
        .version(env!("CARGO_PKG_VERSION"))
        .disable_help_subcommand(full.is_disable_help_subcommand_set())
        .args(full.get_arguments().cloned());
    full.get_subcommands()
        .filter(|verb| !verb.is_hide_set())
        .fold(top, |surface, verb| surface.subcommand(verb.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exit;
    use std::path::Path;

    fn parse(argv: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(argv)
    }

    fn code(argv: &[&str]) -> i32 {
        match parse(argv) {
            Ok(_) => exit::OK,
            Err(e) => usage_exit_code(&e),
        }
    }

    #[test]
    fn bare_amx_is_the_front_door_not_a_usage_error() {
        let cli = parse(&["amx"]).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.verb(), None);
        assert_eq!(cli.dir, None, "the front door is about every agent");
    }

    #[test]
    fn ls_the_front_door_takes_a_directory_and_is_still_the_front_door() {
        let cli = parse(&["amx", "--dir", "/srv/app"]).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.dir.as_deref(), Some(Path::new("/srv/app")));
    }

    #[test]
    fn ls_the_verb_takes_the_directory_the_reading_is_about() {
        let cli = parse(&["amx", "ls", "--dir", "/srv/app", "--json"]).unwrap();
        let Some(Command::Ls { json, dir }) = cli.command else {
            panic!("expected ls");
        };
        assert!(json);
        assert_eq!(dir.as_deref(), Some(Path::new("/srv/app")));

        // A relative directory is kept as typed.
        let cli = parse(&["amx", "ls", "--dir", "."]).unwrap();
        let Some(Command::Ls { dir, .. }) = cli.command else {
            panic!("expected ls");
        };
        assert_eq!(dir.as_deref(), Some(Path::new(".")));

        // The top-level flag in front of the verb.
        let cli = parse(&["amx", "--dir", "/srv/app", "ls"]).unwrap();
        assert_eq!(cli.dir.as_deref(), Some(Path::new("/srv/app")));
        assert!(matches!(cli.command, Some(Command::Ls { dir: None, .. })));
    }

    #[test]
    fn every_verb_parses() {
        let lines: &[(&[&str], &str)] = &[
            (&["amx", "new", "fix the bug"], "new"),
            (&["amx", "new", "--file", "brief.md"], "new"),
            (&["amx", "new", "--file", "-"], "new"),
            (&["amx", "ls"], "ls"),
            (&["amx", "ls", "--json"], "ls"),
            (&["amx", "ls", "--dir", "/srv/app"], "ls"),
            (&["amx", "ls", "--dir", "/srv/app", "--json"], "ls"),
            (&["amx", "status", "fix-a1b"], "status"),
            (&["amx", "status", "fix-a1b", "--json"], "status"),
            (&["amx", "send", "fix-a1b", "carry on"], "send"),
            (&["amx", "send", "fix-a1b", "--file", "notes.md"], "send"),
            (&["amx", "send", "fix-a1b", "--file", "-"], "send"),
            (&["amx", "answer", "fix-a1b", "y"], "answer"),
            (&["amx", "answer", "fix-a1b", "1,3"], "answer"),
            (
                &["amx", "answer", "fix-a1b", "--text", "the sqlite one"],
                "answer",
            ),
            (
                &["amx", "answer", "fix-a1b", "1", "--note", "keep it short"],
                "answer",
            ),
            (&["amx", "interrupt", "fix-a1b"], "interrupt"),
            (&["amx", "rename", "fix-a1b", "auth"], "rename"),
            (&["amx", "result", "fix-a1b"], "result"),
            (&["amx", "result", "fix-a1b", "--timeout", "30"], "result"),
            (&["amx", "wait", "fix-a1b"], "wait"),
            (&["amx", "wait", "a", "b", "--any"], "wait"),
            (
                &["amx", "wait", "a", "--for", "working", "--timeout", "5"],
                "wait",
            ),
            (&["amx", "attach", "fix-a1b"], "attach"),
            (&["amx", "attach", "--next"], "attach"),
            (&["amx", "attach", "--prev"], "attach"),
            (&["amx", "attach", "--waiting"], "attach"),
            (&["amx", "attach", "--last"], "attach"),
            (&["amx", "logs", "fix-a1b"], "logs"),
            (&["amx", "logs", "fix-a1b", "--lines", "40"], "logs"),
            (&["amx", "stop", "fix-a1b"], "stop"),
            (&["amx", "stop", "fix-a1b", "--force"], "stop"),
            (&["amx", "stop", "fix-a1b", "--delete"], "stop"),
            (&["amx", "sweep"], "sweep"),
            (&["amx", "sweep", "--force"], "sweep"),
            (&["amx", "clear"], "clear"),
            (&["amx", "clear", "--force"], "clear"),
            (&["amx", "diff", "fix-a1b"], "diff"),
            (&["amx", "diff", "fix-a1b", "--stat"], "diff"),
            (&["amx", "resume", "fix-a1b"], "resume"),
            (&["amx", "resume", "--all"], "resume"),
            (&["amx", "resume", "fix-a1b", "carry on"], "resume"),
            (&["amx", "fork", "fix-a1b"], "fork"),
            (&["amx", "fork", "fix-a1b", "try it with sqlite"], "fork"),
            (&["amx", "adopt"], "adopt"),
            (&["amx", "adopt", "--task", "port the importer"], "adopt"),
            (&["amx", "events"], "events"),
            (&["amx", "events", "fix-a1b", "--follow"], "events"),
            (&["amx", "events", "--json"], "events"),
            (&["amx", "statusline"], "statusline"),
            (&["amx", "doctor"], "doctor"),
            (&["amx", "doctor", "--fix"], "doctor"),
            (&["amx", "uninstall"], "uninstall"),
            (&["amx", "completion", "fish"], "completion"),
            (&["amx", "_hook"], "_hook"),
            (&["amx", "_exit", "fix-a1b", "0"], "_exit"),
            (&["amx", "_boot", "fix-a1b"], "_boot"),
            (&["amx", "_park", "fix-a1b"], "_park"),
        ];
        for (argv, verb) in lines {
            let cli = parse(argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
            assert_eq!(cli.verb(), Some(*verb), "{argv:?}");
        }
    }

    #[test]
    fn new_carries_its_flags_and_hands_the_rest_to_the_vendor_verbatim() {
        let cli = parse(&[
            "amx",
            "new",
            "port the importer",
            "--name",
            "importer",
            "--dir",
            "/srv/app",
            "--no-worktree",
            "--base",
            "main",
            "--agent",
            "claude",
            "--",
            "--session-id",
            "abc-123",
            "--model",
            "opus",
        ])
        .unwrap();

        let Some(Command::New(args)) = cli.command else {
            panic!("expected new");
        };
        assert_eq!(args.task.as_deref(), Some("port the importer"));
        assert_eq!(args.name.as_deref(), Some("importer"));
        assert_eq!(args.dir, Some(PathBuf::from("/srv/app")));
        assert!(args.no_worktree);
        assert_eq!(args.base.as_deref(), Some("main"));
        assert!(!args.with_changes);
        assert_eq!(
            args.agent.and_then(|named| named.command).as_deref(),
            Some("claude")
        );
        assert_eq!(
            args.vendor_args,
            ["--session-id", "abc-123", "--model", "opus"]
        );
    }

    #[test]
    fn new_takes_the_uncommitted_work_with_it_only_where_there_is_a_tree_for_it() {
        let cli = parse(&["amx", "new", "port the importer", "--with-changes"]).unwrap();
        let Some(Command::New(args)) = cli.command else {
            panic!("expected new");
        };
        assert!(args.with_changes);

        // Neither has a worktree to move the changes into.
        for argv in [
            &["amx", "new", "port it", "--with-changes", "--no-worktree"][..],
            &["amx", "new", "--exec", "npm test", "--with-changes"],
        ] {
            assert_eq!(code(argv), exit::USAGE, "{argv:?}");
        }
    }

    #[test]
    fn dials_new_takes_the_vendor_and_its_three_dials() {
        let cli = parse(&[
            "amx",
            "new",
            "port the importer",
            "--agent",
            "claude",
            "--model",
            "opus",
            "--permission",
            "plan",
            "--effort",
            "high",
        ])
        .unwrap();

        let Some(Command::New(args)) = cli.command else {
            panic!("expected new");
        };
        let named = args
            .agent
            .expect("the caller named the vendor and its dials");
        assert_eq!(named.command.as_deref(), Some("claude"));
        assert_eq!(named.model.as_deref(), Some("opus"));
        assert_eq!(named.permission.as_deref(), Some("plan"));
        assert_eq!(named.effort.as_deref(), Some("high"));
    }

    #[test]
    fn exec_new_runs_a_command_where_it_would_have_started_an_agent() {
        let cli = parse(&["amx", "new", "--exec", "npm test && npm run lint"]).unwrap();
        let Some(Command::New(args)) = cli.command else {
            panic!("expected new");
        };
        assert!(args.exec);
        assert_eq!(
            args.task.as_deref(),
            Some("npm test && npm run lint"),
            "the command is what the row is for, so it is the task"
        );
    }

    #[test]
    fn exec_a_command_has_no_vendor_and_so_none_of_a_vendors_flags() {
        for argv in [
            &["amx", "new", "--exec", "npm test", "--agent", "claude"][..],
            &["amx", "new", "--exec", "npm test", "--model", "opus"],
            &["amx", "new", "--exec", "npm test", "--permission", "plan"],
            &["amx", "new", "--exec", "npm test", "--effort", "high"],
            // Vendor arguments have nowhere to go, so they are refused.
            &["amx", "new", "--exec", "npm test", "--", "--watch"],
        ] {
            assert_eq!(code(argv), exit::USAGE, "{argv:?}");
        }
    }

    #[test]
    fn dials_a_spawn_that_names_none_of_them_leaves_the_config_its_say() {
        // No dial named leaves `agent` as `None`, so the config decides.
        let cli = parse(&["amx", "new", "port the importer"]).unwrap();
        let Some(Command::New(args)) = cli.command else {
            panic!("expected new");
        };
        assert!(args.agent.is_none());

        let cli = parse(&["amx", "new", "port the importer", "--effort", "max"]).unwrap();
        let Some(Command::New(args)) = cli.command else {
            panic!("expected new");
        };
        let named = args.agent.expect("one dial is enough to be named");
        assert_eq!(named.effort.as_deref(), Some("max"));
        assert!(named.command.is_none() && named.model.is_none());
    }

    #[test]
    fn dials_the_vendors_own_model_flag_is_still_the_vendors() {
        // amx's `--model` before the separator, claude's after it.
        let cli = parse(&[
            "amx",
            "new",
            "port the importer",
            "--model",
            "fable",
            "--",
            "--model",
            "opus",
        ])
        .unwrap();
        let Some(Command::New(args)) = cli.command else {
            panic!("expected new");
        };
        assert_eq!(
            args.agent.and_then(|named| named.model),
            Some("fable".to_string())
        );
        assert_eq!(args.vendor_args, ["--model", "opus"]);
    }

    #[test]
    fn vendor_arguments_are_not_read_as_amxs_own() {
        // `--help` after the separator is passed on, not handled by amx.
        let cli = parse(&["amx", "new", "fix the log-in bug", "--", "--help"]).unwrap();
        let Some(Command::New(args)) = cli.command else {
            panic!("expected new");
        };
        assert_eq!(args.task.as_deref(), Some("fix the log-in bug"));
        assert_eq!(args.vendor_args, ["--help"]);
    }

    #[test]
    fn the_separator_belongs_to_the_vendor_so_it_cannot_stand_in_for_a_task() {
        assert_eq!(code(&["amx", "new", "--", "--model", "opus"]), exit::USAGE);
    }

    #[test]
    fn fork_takes_an_agent_to_copy_and_a_turn_of_its_own() {
        let cli = parse(&["amx", "fork", "fix-a1b"]).unwrap();
        let Some(Command::Fork { id, task }) = cli.command else {
            panic!("expected fork");
        };
        assert_eq!(id, "fix-a1b");
        assert_eq!(
            task, None,
            "a copy with nothing to do opens the conversation and waits"
        );

        let cli = parse(&["amx", "fork", "fix-a1b", "try it with sqlite"]).unwrap();
        let Some(Command::Fork { task, .. }) = cli.command else {
            panic!("expected fork");
        };
        assert_eq!(task.as_deref(), Some("try it with sqlite"));
    }

    #[test]
    fn wait_takes_the_agents_and_the_state_to_hold_out_for() {
        let cli = parse(&["amx", "wait", "a", "b", "c"]).unwrap();
        let Some(Command::Wait {
            ids,
            children,
            any,
            state,
            timeout,
        }) = cli.command
        else {
            panic!("expected wait");
        };
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(children, None);
        assert!(!any, "every agent named, unless the caller says otherwise");
        assert_eq!(state, None, "a turn that is over, whichever way it ended");
        assert_eq!(timeout, None);

        let cli = parse(&["amx", "wait", "a", "--any", "--for", "idle"]).unwrap();
        let Some(Command::Wait { any, state, .. }) = cli.command else {
            panic!("expected wait");
        };
        assert!(any);
        assert_eq!(state, Some(Phase::Idle));

        // Every word `ls --json` prints parses, and the error lists all eight.
        for phase in PHASES {
            assert_eq!(a_phase(phase.as_str()), Ok(phase));
        }
        let refusal = a_phase("sleeping").unwrap_err();
        for phase in PHASES {
            assert!(refusal.contains(phase.as_str()), "{refusal}");
        }
    }

    #[test]
    fn wait_and_result_take_a_parents_children_instead_of_ids() {
        let cli = parse(&["amx", "wait", "--children", "p"]).unwrap();
        let Some(Command::Wait { ids, children, .. }) = cli.command else {
            panic!("expected wait");
        };
        assert!(ids.is_empty());
        assert_eq!(children.as_deref(), Some("p"));

        let cli = parse(&["amx", "result", "--children", "p", "--json"]).unwrap();
        let Some(Command::Result {
            id, children, json, ..
        }) = cli.command
        else {
            panic!("expected result");
        };
        assert_eq!(id, None);
        assert_eq!(children.as_deref(), Some("p"));
        assert!(json);

        // Exactly one of the two is required.
        assert!(parse(&["amx", "result"]).is_err());
        assert!(parse(&["amx", "wait"]).is_err());
        assert!(parse(&["amx", "wait", "--children", "p", "a"]).is_err());
        assert!(parse(&["amx", "result", "a", "--children", "p"]).is_err());
    }

    #[test]
    fn adopt_takes_a_label_for_the_row_and_nothing_about_where_to_look() {
        // The pane and session come from the environment, so only the label is typed.
        let cli = parse(&["amx", "adopt"]).unwrap();
        let Some(Command::Adopt(args)) = cli.command else {
            panic!("expected adopt");
        };
        assert_eq!(args.task, None);
        assert_eq!(args.name, None);

        let cli = parse(&[
            "amx",
            "adopt",
            "--task",
            "port the importer",
            "--name",
            "importer",
        ])
        .unwrap();
        let Some(Command::Adopt(args)) = cli.command else {
            panic!("expected adopt");
        };
        assert_eq!(args.task.as_deref(), Some("port the importer"));
        assert_eq!(args.name.as_deref(), Some("importer"));

        // An empty label is refused, and there is no positional pane.
        assert_eq!(code(&["amx", "adopt", "--task", "  "]), exit::USAGE);
        assert_eq!(code(&["amx", "adopt", "%7"]), exit::USAGE);
    }

    #[test]
    fn stop_takes_its_dispositions_by_name() {
        let cli = parse(&[
            "amx",
            "stop",
            "fix-a1b",
            "--worktree",
            "keep",
            "--branch",
            "delete",
        ])
        .unwrap();
        let Some(Command::Stop(args)) = cli.command else {
            panic!("expected stop");
        };
        assert_eq!(args.worktree, Some(Disposition::Keep));
        assert_eq!(args.branch, Some(Disposition::Delete));
        assert!(!args.force);
    }

    #[test]
    fn a_malformed_command_line_exits_sixty_four() {
        for argv in [
            &["amx", "nosuchverb"][..],
            &["amx", "ls", "--nosuchflag"],
            // `--dir` needs a value in either position.
            &["amx", "ls", "--dir"],
            &["amx", "--dir"],
            &["amx", "status"],
            &["amx", "logs"],
            &["amx", "send", "fix-a1b"],
            &["amx", "interrupt"],
            // rename needs both the id and the name.
            &["amx", "rename"],
            &["amx", "rename", "fix-a1b"],
            &["amx", "answer", "fix-a1b"],
            // A choice and `--text` cannot be combined.
            &["amx", "answer", "fix-a1b", "2", "--text", "2"],
            // `--note` needs a choice to go with.
            &["amx", "answer", "fix-a1b", "--note", "keep it short"],
            &["amx", "answer", "fix-a1b", "--text", "2", "--note", "short"],
            &["amx", "result", "fix-a1b", "--timeout", "soon"],
            // `--json` is only for `--children`.
            &["amx", "result", "fix-a1b", "--json"],
            // wait needs ids, and `--for` needs a known state.
            &["amx", "wait"],
            &["amx", "wait", "a", "--for", "sleeping"],
            // attach takes an id or exactly one direction.
            &["amx", "attach", "fix-a1b", "--next"],
            &["amx", "attach", "--next", "--prev"],
            &["amx", "attach"],
            &["amx", "attach", "fix-a1b", "--last"],
            &["amx", "attach", "--last", "--waiting"],
            // `--lines` must be a positive number.
            &["amx", "logs", "fix-a1b", "--lines", "0"],
            &["amx", "logs", "fix-a1b", "--lines", "all"],
            &["amx", "stop", "fix-a1b", "--worktree", "burn"],
            &["amx", "resume"],
            &["amx", "resume", "fix-a1b", "--all"],
            &["amx", "resume", "--all", "carry on"],
            // fork needs an id, and an empty task is refused.
            &["amx", "fork"],
            &["amx", "fork", "fix-a1b", "  "],
            &["amx", "_exit", "fix-a1b"],
            &["amx", "new"],
        ] {
            assert_eq!(code(argv), exit::USAGE, "{argv:?}");
        }
    }

    #[test]
    fn help_and_version_are_not_failures() {
        for argv in [
            &["amx", "--help"][..],
            &["amx", "-h"],
            &["amx", "--version"],
            &["amx", "new", "--help"],
        ] {
            assert_eq!(code(argv), exit::OK, "{argv:?}");
        }
    }

    #[test]
    fn clibatch_a_task_with_nothing_in_it_is_not_a_task() {
        for argv in [
            &["amx", "new", ""][..],
            &["amx", "new", "   "],
            &["amx", "new", "\t\n"],
            &["amx", "new", "", "--no-worktree"],
        ] {
            assert_eq!(code(argv), exit::USAGE, "{argv:?}");
        }
    }

    #[test]
    fn clibatch_a_task_is_typed_or_read_from_a_file_and_never_both() {
        let cli = parse(&["amx", "new", "--file", "brief.md"]).unwrap();
        let Some(Command::New(args)) = cli.command else {
            panic!("expected new");
        };
        assert_eq!(args.task, None, "the file is where the task is");
        assert_eq!(args.file.as_deref(), Some(Path::new("brief.md")));

        // `-` is stdin.
        let cli = parse(&["amx", "new", "--file", "-"]).unwrap();
        let Some(Command::New(args)) = cli.command else {
            panic!("expected new");
        };
        assert_eq!(args.file.as_deref(), Some(Path::new("-")));

        // `--exec` reads its command from the file too.
        let cli = parse(&["amx", "new", "--exec", "--file", "release.sh"]).unwrap();
        let Some(Command::New(args)) = cli.command else {
            panic!("expected new");
        };
        assert!(args.exec);
        assert_eq!(args.file.as_deref(), Some(Path::new("release.sh")));

        // A typed task and `--file` together are refused, as is `--file` with no path.
        for argv in [
            &["amx", "new", "port the importer", "--file", "brief.md"][..],
            &["amx", "new", "--file"],
        ] {
            assert_eq!(code(argv), exit::USAGE, "{argv:?}");
        }
    }

    #[test]
    fn clibatch_a_task_can_be_written_in_an_editor_instead_of_typed() {
        let cli = parse(&["amx", "new", "--edit"]).unwrap();
        let Some(Command::New(args)) = cli.command else {
            panic!("expected new");
        };
        assert!(args.edit);
        assert_eq!(args.task, None, "the editor is where the task is");

        // `--exec` takes its command from the editor too.
        let cli = parse(&["amx", "new", "--exec", "--edit"]).unwrap();
        let Some(Command::New(args)) = cli.command else {
            panic!("expected new");
        };
        assert!(args.exec && args.edit);

        // Two sources of a task are refused, and so is none.
        for argv in [
            &["amx", "new", "port the importer", "--edit"][..],
            &["amx", "new", "--edit", "--file", "brief.md"],
            &["amx", "new"],
        ] {
            assert_eq!(code(argv), exit::USAGE, "{argv:?}");
        }
    }

    #[test]
    fn clibatch_a_message_is_typed_or_read_from_a_file_and_never_both() {
        let cli = parse(&["amx", "send", "fix-login-a1b", "--file", "notes.md"]).unwrap();
        let Some(Command::Send { id, text, file }) = cli.command else {
            panic!("expected send");
        };
        assert_eq!(id, "fix-login-a1b");
        assert_eq!(text, None, "the file is where the message is");
        assert_eq!(file.as_deref(), Some(Path::new("notes.md")));

        let cli = parse(&["amx", "send", "fix-login-a1b", "--file", "-"]).unwrap();
        let Some(Command::Send { file, .. }) = cli.command else {
            panic!("expected send");
        };
        assert_eq!(file.as_deref(), Some(Path::new("-")));

        // A typed message and `--file` together are refused.
        for argv in [
            &[
                "amx",
                "send",
                "fix-login-a1b",
                "carry on",
                "--file",
                "notes.md",
            ][..],
            &["amx", "send", "fix-login-a1b", "--file"],
        ] {
            assert_eq!(code(argv), exit::USAGE, "{argv:?}");
        }
    }

    #[test]
    fn clibatch_a_message_with_nothing_in_it_is_not_a_message() {
        for argv in [
            &["amx", "send", "fix-login-a1b", ""][..],
            &["amx", "send", "fix-login-a1b", "   "],
            &["amx", "send", "fix-login-a1b", "\t\n"],
        ] {
            assert_eq!(code(argv), exit::USAGE, "{argv:?}");
        }
        let cli = parse(&["amx", "send", "fix-login-a1b", "  carry on\n"]).unwrap();
        let Some(Command::Send { text, .. }) = cli.command else {
            panic!("expected send");
        };
        assert_eq!(text.as_deref(), Some("  carry on\n"), "passed on as typed");
    }

    #[test]
    fn clibatch_a_task_read_from_a_file_is_all_of_it_bar_the_last_newline() {
        let dir = tempfile::TempDir::new().unwrap();
        let brief = dir.path().join("brief.md");

        std::fs::write(&brief, "fix the login bug\n").unwrap();
        assert_eq!(text_of(&brief).unwrap(), "fix the login bug");

        // Only the one final newline is removed.
        std::fs::write(&brief, "fix the login bug\n\n").unwrap();
        assert_eq!(text_of(&brief).unwrap(), "fix the login bug\n");
        std::fs::write(&brief, "  fix the login bug").unwrap();
        assert_eq!(text_of(&brief).unwrap(), "  fix the login bug");

        // An empty file is an empty task.
        for written in ["", "\n", "  \n"] {
            std::fs::write(&brief, written).unwrap();
            assert!(text_of(&brief).is_err(), "{written:?}");
        }

        // A missing file is named in the error.
        let refusal = text_of(&dir.path().join("nothing.md")).unwrap_err();
        assert!(refusal.contains("nothing.md"), "{refusal}");
    }

    #[test]
    fn clibatch_text_written_somewhere_else_is_read_the_way_a_files_text_is() {
        // What `text_of` does after reading, and what `new --edit` reuses.
        assert_eq!(a_text("fix the login bug\n").unwrap(), "fix the login bug");
        assert_eq!(
            a_text("fix the login bug\n\n").unwrap(),
            "fix the login bug\n"
        );
        assert_eq!(
            a_text("  fix the login bug").unwrap(),
            "  fix the login bug"
        );
        for written in ["", "\n", "  \n"] {
            assert!(a_text(written).is_err(), "{written:?}");
        }
    }

    #[test]
    fn clibatch_a_task_reaches_the_vendor_as_it_was_typed() {
        // Only an empty task is refused; a task is never trimmed.
        let cli = parse(&["amx", "new", "  fix the login bug\n"]).unwrap();
        let Some(Command::New(args)) = cli.command else {
            panic!("expected new");
        };
        assert_eq!(args.task.as_deref(), Some("  fix the login bug\n"));
    }

    #[test]
    fn statusline_is_a_verb_a_person_can_find() {
        use clap::CommandFactory;

        let cli = parse(&["amx", "statusline"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Statusline)));

        // It goes into a tmux config once, so it has to be findable in help.
        let listed = Cli::command()
            .get_subcommands()
            .any(|verb| verb.get_name() == "statusline" && !verb.is_hide_set());
        assert!(listed, "statusline is not in help");

        // It takes no arguments.
        assert_eq!(code(&["amx", "statusline", "fix-a1b"]), exit::USAGE);
    }

    #[test]
    fn completion_writes_a_script_for_each_shell_it_names() {
        use clap_complete::Shell;

        for (named, shell) in [
            ("bash", Shell::Bash),
            ("elvish", Shell::Elvish),
            ("fish", Shell::Fish),
            ("powershell", Shell::PowerShell),
            ("zsh", Shell::Zsh),
        ] {
            let cli = parse(&["amx", "completion", named]).unwrap();
            assert!(matches!(cli.command, Some(Command::Completion { shell: s }) if s == shell));
            assert!(
                !completion_script(shell).is_empty(),
                "the {named} script is empty"
            );
        }

        // The shell is required, and an unsupported one is refused.
        assert_eq!(code(&["amx", "completion"]), exit::USAGE);
        assert_eq!(code(&["amx", "completion", "nushell"]), exit::USAGE);
    }

    #[test]
    fn completion_offers_every_verb_a_person_can_type_and_none_of_the_others() {
        use clap_complete::Shell;

        // The underscore verbs are hidden from help, so completion leaves them out too.
        for shell in [
            Shell::Bash,
            Shell::Elvish,
            Shell::Fish,
            Shell::PowerShell,
            Shell::Zsh,
        ] {
            let script = String::from_utf8(completion_script(shell)).expect("a script is text");
            for (verb, _) in listed_verbs() {
                assert!(
                    script.contains(&verb),
                    "the {shell} script never offers `{verb}`"
                );
            }
            for hidden in ["_hook", "_exit", "_boot", "_park"] {
                assert!(
                    !script.contains(hidden),
                    "the {shell} script offers `{hidden}`"
                );
            }
        }
    }

    /// clap's own consistency check: no duplicate names or conflicting flags.
    #[test]
    fn the_surface_is_well_formed() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    /// The documents that describe the command line.
    const README: &str = include_str!("../README.md");
    const SKILL: &str = include_str!("../skill/amx/SKILL.md");

    /// Every verb `amx --help` lists, with its long flags.
    fn listed_verbs() -> Vec<(String, Vec<String>)> {
        use clap::CommandFactory;
        Cli::command()
            .get_subcommands()
            .filter(|verb| !verb.is_hide_set())
            .map(|verb| {
                let flags = verb
                    .get_arguments()
                    .filter_map(|arg| arg.get_long())
                    .filter(|long| *long != "help")
                    .map(|long| format!("--{long}"))
                    .collect();
                (verb.get_name().to_string(), flags)
            })
            .collect()
    }

    /// Every verb, hidden ones included.
    fn every_verb() -> Vec<String> {
        use clap::CommandFactory;
        Cli::command()
            .get_subcommands()
            .map(|verb| verb.get_name().to_string())
            .collect()
    }

    /// Every key the view binds, read as text out of the table its `?` overlay
    /// is drawn from.
    ///
    /// The table is private to the view. Checking the declared length makes a
    /// change of shape fail here instead of passing quietly.
    fn keys_the_view_binds() -> Vec<&'static str> {
        let source = include_str!("tui/paint/help.rs");
        let (_, table) = source
            .split_once("const HELP: [(&str, &str); ")
            .expect("the table the overlay is drawn from");
        let (count, table) = table.split_once("] = [").expect("how many keys it holds");
        let (table, _) = table.split_once("\n];").expect("the end of it");

        // Two literals per entry: the key, then what it does.
        let written: Vec<&str> = table.split('"').skip(1).step_by(2).collect();
        let keys: Vec<&str> = written.into_iter().step_by(2).collect();
        assert_eq!(
            keys.len(),
            count.parse::<usize>().expect("a count"),
            "the keys table is not the shape this reads it in: {keys:?}"
        );
        keys
    }

    /// The verbs a document uses in code spans and fences; prose is skipped.
    fn verbs_named_in(text: &str) -> Vec<String> {
        let mut named = Vec::new();
        for (at, chunk) in text.split("```").enumerate() {
            let code: Vec<&str> = match at % 2 == 1 {
                true => vec![chunk],
                // Outside a fence, only what is between backticks.
                false => chunk.split('`').skip(1).step_by(2).collect(),
            };
            for line in code.iter().flat_map(|code| code.lines()) {
                // A shell comment inside a fence is prose.
                let line = line.split('#').next().unwrap_or_default();
                for after in line.split("amx ").skip(1) {
                    let verb: String = after
                        .chars()
                        .take_while(|c| c.is_ascii_lowercase() || *c == '_')
                        .collect();
                    if !verb.is_empty() {
                        named.push(verb);
                    }
                }
            }
        }
        named
    }

    #[test]
    fn docs_the_readme_names_every_verb_and_the_flags_it_takes() {
        for (verb, flags) in listed_verbs() {
            assert!(
                README.contains(&format!("amx {verb}")),
                "the README says nothing about `amx {verb}`"
            );
            for flag in flags {
                assert!(
                    README.contains(&flag),
                    "the README says nothing about `amx {verb} {flag}`"
                );
            }
        }
    }

    #[test]
    fn docs_the_readme_names_every_key_the_view_binds() {
        // A key column may name two keys; each must be documented.
        for key in keys_the_view_binds().iter().flat_map(|key| key.split(' ')) {
            assert!(
                README.contains(&format!("`{key}`")),
                "the README names no key `{key}`"
            );
        }
    }

    #[test]
    fn docs_the_readme_names_every_config_key() {
        for key in crate::config::KNOWN_KEYS {
            assert!(
                README.contains(&format!("\n{key} = ")),
                "the README's config file has no `{key}` in it"
            );
        }
    }

    /// One verb's short help.
    fn about(verb: &str) -> String {
        use clap::CommandFactory;
        Cli::command()
            .get_subcommands()
            .find(|listed| listed.get_name() == verb)
            .and_then(|listed| listed.get_about().map(ToString::to_string))
            .unwrap_or_else(|| panic!("nothing about `{verb}`"))
    }

    #[test]
    fn docs_the_help_for_answer_offers_the_grammar_the_verb_reads() {
        // answer's short help must list the grammar the verb accepts.
        let (_, offered) = about("answer")
            .split_once(": ")
            .map(|(said, grammar)| (said.to_string(), grammar.to_string()))
            .expect("the grammar it takes");
        let offered: Vec<String> = offered
            .trim_end_matches('.')
            .split(", ")
            .map(str::to_string)
            .collect();

        for key in ["y", "n", "1", "5", "9", "enter", "esc"] {
            assert!(
                crate::verbs::answer::named(key).is_some(),
                "the verb no longer reads `{key}`"
            );
        }
        for key in ["y", "n", "1-9", "1,3", "enter", "esc"] {
            assert!(
                offered.iter().any(|word| word == key),
                "the help does not offer `{key}`: {offered:?}"
            );
        }
        assert!(
            offered.iter().any(|word| word.contains("words")),
            "words of your own answer the question that has a field for them, \
             and the help never says so: {offered:?}"
        );
    }

    #[test]
    fn docs_the_help_and_the_readme_count_the_checks_doctor_makes() {
        // Both spell the number out, so both go stale without this.
        let checks = crate::verbs::doctor::report(&crate::verbs::doctor::Findings {
            tmux: None,
            vendor: String::new(),
            vendor_path: None,
            config: PathBuf::new(),
            config_warnings: Vec::new(),
            home: PathBuf::new(),
            // One agent, so the `hooks` check is counted.
            wirings: vec![crate::verbs::doctor::VendorWiring {
                vendor: "claude",
                hooks: None,
                wire: PathBuf::new(),
                wired: crate::install::Wired::Nothing,
                opt_in: Vec::new(),
            }],
            exe: PathBuf::new(),
            on_path: Vec::new(),
            state_root: PathBuf::new(),
            state_error: None,
            dirty_handoffs: Vec::new(),
            parked: Vec::new(),
            // The server and folder-trust checks are conditional and not counted.
            server: None,
            store: None,
            stale: Vec::new(),
            folder: None,
            orphan_ids: Vec::new(),
            orphan_trees: Vec::new(),
            zeroed: Vec::new(),
        });
        // Count kinds of check, not lines: `hooks` repeats per agent.
        let kinds: std::collections::BTreeSet<&str> = checks.iter().map(|c| c.name).collect();
        let counted = [
            "no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        ]
        .get(kinds.len())
        .expect("a count these have a word for");

        use clap::CommandFactory;
        let long = Cli::command()
            .get_subcommands()
            .find(|listed| listed.get_name() == "doctor")
            .and_then(|listed| listed.get_long_about().map(ToString::to_string))
            .expect("what doctor says of itself at length")
            .to_lowercase();
        assert!(
            long.contains(&format!("{counted} things")),
            "doctor asks {} kinds of check and its help says otherwise: {long}",
            kinds.len()
        );
        assert!(
            README.contains(&format!("the {counted} things")),
            "doctor asks {} kinds of check and the README says otherwise",
            kinds.len()
        );
    }

    #[test]
    fn docs_neither_document_names_a_verb_amx_does_not_have() {
        // A command copied from either document must name a real verb.
        let verbs = every_verb();
        for (document, text) in [("README", README), ("skill", SKILL)] {
            for named in verbs_named_in(text) {
                assert!(
                    verbs.contains(&named),
                    "the {document} says `amx {named}`, which is not a verb"
                );
            }
        }
    }

    #[test]
    fn docs_the_skill_is_one_the_vendor_can_load() {
        let (frontmatter, body) = SKILL
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .expect("frontmatter, which is what makes it a skill");

        assert!(
            frontmatter.lines().any(|line| line.trim() == "name: amx"),
            "the skill is not named for the directory it is in: {frontmatter}"
        );
        let description = frontmatter
            .lines()
            .find_map(|line| line.trim().strip_prefix("description:"))
            .expect("a description, which is what it is loaded on");
        assert!(!description.trim().is_empty(), "an empty description");
        assert!(!body.trim().is_empty(), "a skill with nothing in it");
    }

    #[test]
    fn docs_the_skill_teaches_the_loop() {
        // Every exit code is documented in the skill.
        for code in [
            exit::OK,
            exit::FAILURE,
            exit::BLOCKED,
            exit::TIMEOUT,
            exit::USAGE,
        ] {
            assert!(
                SKILL.contains(&format!("`{code}`")),
                "the skill does not say what exit {code} means"
            );
        }

        // The loop the skill teaches: start, wait, answer, send, stop.
        for taught in [
            "amx new",
            "amx result",
            "amx answer",
            "amx send",
            "amx stop",
            "--timeout",
            "stdout",
            "max_agents",
            "max_total",
        ] {
            assert!(SKILL.contains(taught), "the skill never mentions {taught}");
        }
    }

    /// Every state a record can hold. The exhaustive match stops this compiling
    /// when a state is added.
    fn every_phase() -> Vec<Phase> {
        let all = [
            Phase::Starting,
            Phase::Working,
            Phase::Waiting,
            Phase::Idle,
            Phase::Done,
            Phase::Failed,
            Phase::Stopped,
            Phase::Unknown,
        ];
        for phase in all {
            match phase {
                Phase::Starting
                | Phase::Working
                | Phase::Waiting
                | Phase::Idle
                | Phase::Done
                | Phase::Failed
                | Phase::Stopped
                | Phase::Unknown => {}
            }
        }
        all.to_vec()
    }

    /// The paragraph of a document that starts with `opening`.
    fn paragraph<'a>(text: &'a str, opening: &str) -> &'a str {
        let (_, from) = text
            .split_once(opening)
            .unwrap_or_else(|| panic!("nothing opens with {opening:?}"));
        from.split("\n\n").next().unwrap_or_default()
    }

    #[test]
    fn docs_the_readme_listing_says_the_words_ls_prints() {
        // Every row of the README's sample listing uses a word `ls` prints.
        let (_, listing) = README.split_once("$ amx ls\n").expect("a listing");
        let (listing, _) = listing.split_once("```").expect("the end of it");
        let printed: Vec<&str> = every_phase().into_iter().map(Phase::word).collect();
        for row in listing.lines() {
            let word = row.split_whitespace().next().unwrap_or_default();
            assert!(
                printed.contains(&word),
                "the README lists a row as `{word}`, which ls never prints: {row}"
            );
        }
    }

    #[test]
    fn docs_the_readme_names_every_state_and_the_word_the_table_says_for_it() {
        let said = paragraph(README, "`state` is one of");
        for phase in every_phase() {
            assert!(
                said.contains(&format!("`{}`", phase.as_str())),
                "the README's states leave out `{}`",
                phase.as_str()
            );
            // Where the table's word differs from the JSON state, the README says so.
            if phase.word() != phase.as_str() {
                assert!(
                    said.contains(&format!("`{}` as `{}`", phase.as_str(), phase.word())),
                    "the README never says the table prints `{}` as `{}`",
                    phase.as_str(),
                    phase.word()
                );
            }
        }
    }

    #[test]
    fn docs_both_exit_tables_name_every_verb_that_exits_2() {
        // Every verb that can return `exit::BLOCKED`, from a read of src/verbs.
        let blocking = [
            "answer",
            "fork",
            "interrupt",
            "new",
            "result",
            "resume",
            "send",
            "sub",
        ];
        let (_, readme) = README
            .split_once("\n| `2`  |")
            .expect("the README's row for 2");
        let readme = readme.lines().next().unwrap_or_default();
        let (_, skill) = SKILL
            .split_once("## Exit codes")
            .expect("the skill's exit codes");
        let (skill, _) = skill.split_once("\n## ").expect("the end of them");
        for verb in blocking {
            assert!(
                readme.contains(&format!("`{verb}`")),
                "the README's exit table never says `{verb}` exits 2: {readme}"
            );
            assert!(
                skill.contains(&format!("`{verb}`")),
                "the skill never says `{verb}` exits 2"
            );
        }
    }

    #[test]
    fn docs_logs_is_said_to_read_the_transcript_first() {
        use clap::CommandFactory;
        let long = Cli::command()
            .find_subcommand("logs")
            .and_then(|logs| logs.get_long_about().map(ToString::to_string))
            .expect("what logs says of itself at length");
        let transcript = long
            .find("transcript")
            .expect("logs help names no transcript");
        let pane = long.find("pane").expect("logs help names no pane");
        assert!(
            transcript < pane,
            "logs help reads the pane before the transcript: {long}"
        );

        let row = SKILL
            .lines()
            .find(|line| line.starts_with("| `amx logs"))
            .expect("the skill's row for logs");
        assert!(
            row.contains("transcript"),
            "the skill's logs is the pane: {row}"
        );
    }

    #[test]
    fn docs_ls_dir_is_called_a_directory_filter() {
        // It filters one listing and does not isolate runs from each other.
        use clap::CommandFactory;
        let dir = Cli::command()
            .find_subcommand("ls")
            .and_then(|ls| {
                ls.get_arguments()
                    .find(|arg| arg.get_id() == "dir")
                    .cloned()
            })
            .and_then(|dir| dir.get_help().map(ToString::to_string))
            .expect("ls --dir help");
        assert!(dir.contains("directory filter"), "{dir}");
        assert!(
            README.contains("directory filter"),
            "the README never says so"
        );
        assert!(
            !README.contains("no other run's"),
            "the README says ls --dir keeps runs apart"
        );
    }
}
