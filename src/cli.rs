//! The command line: every verb amx answers to.
//!
//! Bare `amx` has no subcommand — that is the front door (the cockpit), not a
//! usage error. The four underscore verbs are amx talking to itself from
//! inside a pane, a vendor hook, or a timer a tmux server is holding; they are
//! hidden from help but are as much of the contract as the rest.

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
    /// Only the agents whose work is under this directory, and where the
    /// view stands.
    ///
    /// The front door's own narrowing, so `amx --dir /srv/app` is the list of
    /// that project's agents and nothing else, drawn or printed. The view
    /// opened by it stands in that directory as if it had been run there: the
    /// header names it and a task typed at the view starts under it. A verb
    /// that takes the same flag reads its own first.
    #[arg(long, value_name = "PATH")]
    pub dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    /// The verb as it was typed, or `None` for bare `amx`.
    pub fn verb(&self) -> Option<&'static str> {
        use Command::*;
        Some(match self.command.as_ref()? {
            New(_) => "new",
            Ls { .. } => "ls",
            Status { .. } => "status",
            Send { .. } => "send",
            Answer { .. } => "answer",
            Interrupt { .. } => "interrupt",
            Rename { .. } => "rename",
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

    /// List agents and their states.
    Ls {
        /// Print the stable JSON instead of the table.
        #[arg(long)]
        json: bool,

        /// Only the agents whose work is under this directory.
        ///
        /// An agent is that directory's when it runs under it, and a worktree
        /// agent is its repository's wherever amx put the tree. Nothing is
        /// hidden and nothing is written down: it is one reading of one
        /// question, and the same agent is in two of them when the
        /// directories nest.
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
    },

    /// Show one agent, and which signal that state came from.
    Status {
        id: String,
        /// Print the stable JSON instead of the summary.
        #[arg(long)]
        json: bool,
    },

    /// Send a message to a working or idle agent.
    Send {
        id: String,

        /// What to put in front of it.
        #[arg(required_unless_present = "file", conflicts_with = "file")]
        text: Option<String>,

        /// Read the message from this file instead, or from stdin for `-`.
        ///
        /// The same door `amx new --file` opens, for the follow-up too long to
        /// quote into a shell: the file is read whole, its last newline taken
        /// off, and what is left is the message.
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
    },

    /// Answer a waiting agent's question: y, n, 1-9, 1,3, enter, esc, or words.
    ///
    /// The grammar is the question's rather than amx's. A permission prompt
    /// and the folder-trust screen read one key. A question the vendor asked
    /// itself offers a field beside its choices, so words of your own are an
    /// answer to that one and to nothing else, and a question that takes more
    /// than one choice — `.multi` in `amx status --json` — is answered by
    /// naming them: `1,3`.
    Answer {
        /// The agent that is waiting on one.
        id: String,
        /// What the question is answered with.
        #[command(flatten)]
        key: AnswerArgs,
    },

    /// Stop the turn an agent is in the middle of.
    ///
    /// Escape at the pane: the turn ends where it stands and the agent goes
    /// back to its prompt with the conversation behind it intact. It is for
    /// the turn that has gone the wrong way, and the next `send` is what says
    /// which way it should have gone.
    ///
    /// A question is not a turn — Escape there answers it, so `amx answer <id>
    /// esc` is what dismisses one — and a command row has no vendor in it to
    /// read a key, so `amx stop` is what ends one of those.
    Interrupt { id: String },

    /// Call an agent something else on the wall.
    ///
    /// The word in the name column and nothing else. The id is untouched — it
    /// is what every verb takes and what the pane, the branch and the worktree
    /// are named after — so `amx rename <id> <id>` is what puts the row back to
    /// the name amx gives it.
    Rename {
        /// The agent being renamed.
        id: String,
        /// What to call it.
        name: String,
    },

    /// Wait for the agent's turn to end and print its answer.
    Result {
        id: String,
        /// Give up after this many seconds.
        #[arg(long, value_name = "SECONDS")]
        timeout: Option<u64>,
    },

    /// Wait for several agents at once and say which are ready.
    ///
    /// One clock over a fleet: it blocks until every agent named has settled —
    /// its turn over, or stopped on a question — and prints `<id> <state>` for
    /// each as each settles. `--any` ends at the first. What the agent said is
    /// not here: `amx result <id>` hands that back, and returns at once for an
    /// agent this has already named.
    ///
    /// `--for <state>` waits for one named phase instead of for an ending, so
    /// `--for working` is how a caller confirms a fleet started.
    Wait {
        /// The agents to wait on.
        #[arg(num_args = 1.., required = true)]
        ids: Vec<String>,

        /// Come back as soon as one of them has settled.
        #[arg(long)]
        any: bool,

        /// Wait for this state instead of for a turn that is over.
        #[arg(long = "for", value_name = "STATE", value_parser = a_phase)]
        state: Option<Phase>,

        /// Give up after this many seconds.
        #[arg(long, value_name = "SECONDS")]
        timeout: Option<u64>,
    },

    /// Attach to the agent's pane.
    ///
    /// Without an id, the wall says which agent: the order the view draws
    /// under the arrangement it was left in, stepped through from whichever
    /// agent's session this was typed in. Made for a tmux key.
    Attach {
        #[arg(
            required_unless_present_any = ["next", "prev", "waiting", "last"],
            conflicts_with_all = ["next", "prev", "waiting", "last"],
        )]
        id: Option<String>,

        /// The agent after this one on the wall, wrapping at the foot.
        #[arg(long, conflicts_with_all = ["prev", "waiting", "last"])]
        next: bool,

        /// The agent before this one on the wall, wrapping at the top.
        #[arg(long, conflicts_with_all = ["next", "waiting", "last"])]
        prev: bool,

        /// The first agent that is waiting on you.
        #[arg(long, conflicts_with_all = ["next", "prev", "last"])]
        waiting: bool,

        /// The agent you were in before this one.
        #[arg(long, conflicts_with_all = ["next", "prev", "waiting"])]
        last: bool,
    },

    /// Print an agent's recent output without attaching to it.
    ///
    /// While the pane is there this is the pane: the last of what it has drawn,
    /// and as much of what has scrolled off it as tmux still holds. It is a
    /// picture of a screen rather than the agent's own words — `amx result`
    /// hands back those. Once the pane is gone the record is what is left, and
    /// what the agent answered with is what this prints.
    Logs {
        id: String,
        /// How many lines of it to print.
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

    /// Clear away the finished agents whose work has landed.
    ///
    /// An agent whose pull request merged or closed, or whose branch somebody
    /// merged themselves, is holding a record, a worktree and a branch that are
    /// a copy of what the repository already has. This lists them with the
    /// reason each is on the list, asks once, and then takes all three.
    ///
    /// A worktree holding work no commit has is kept, and its record with it.
    Sweep {
        /// Take them without asking.
        #[arg(long)]
        force: bool,
    },

    /// Forget the finished agents, whether or not their work landed.
    ///
    /// Every agent whose turn is over — done, failed, or stopped — is holding a
    /// record, and most of them are holding a worktree nothing outside amx will
    /// ever have an opinion about. This lists them with the reason each one is
    /// finished, asks once, and then takes the record and the worktree. Work
    /// that landed goes the way `sweep` takes it, branch and all; everything
    /// else keeps its branch.
    ///
    /// A worktree holding work no commit has is kept, and its record with it.
    /// An agent sitting at its prompt has not finished and is not on the list.
    Clear {
        /// Take them without asking.
        #[arg(long)]
        force: bool,
    },

    /// Show the agent's worktree against the commit it started from.
    Diff {
        id: String,
        /// Summarise the patch instead of printing it.
        #[arg(long)]
        stat: bool,
    },

    /// Restart a stopped agent, continuing its recorded session.
    ///
    /// A message is the first turn of the agent that comes back. It rides the
    /// vendor's argv as its prompt, where `new` puts a task, so the agent is
    /// working the moment its pane exists rather than standing at its prompt
    /// waiting to be told what happens next.
    Resume {
        #[arg(required_unless_present = "all")]
        id: Option<String>,
        /// What to put to the agent as it comes back. Without one it picks up
        /// where it was and waits for a turn.
        #[arg(value_parser = a_task, conflicts_with = "all")]
        message: Option<String>,
        /// Every stopped agent, as after a tmux server death.
        #[arg(long, conflicts_with = "id")]
        all: bool,
    },

    /// Start a second agent on a copy of this one's conversation.
    ///
    /// The copy runs where the original ran, on everything it had been told up
    /// to now, and goes its own way from there: a different approach to the
    /// same problem, without giving up the one already tried. Both agents are
    /// their own from the moment it starts, and nothing either does reaches the
    /// other.
    ///
    /// It is the recorded session that is copied, so an agent that never
    /// announced one cannot be forked at all — `amx new` is what starts an
    /// agent with no conversation behind it.
    Fork {
        /// The agent whose conversation is copied.
        id: String,
        /// What the copy should do first. Without one it opens the
        /// conversation and waits for a turn.
        #[arg(value_parser = a_task)]
        task: Option<String>,
    },

    /// Put the agent already running in this pane on the wall.
    ///
    /// For the agent you started yourself, in your own tmux, and then wanted
    /// beside the ones amx started: it gets a record, an id and a row, and
    /// every verb that reads or answers an agent works on it from then on.
    ///
    /// It is typed *inside* the agent being adopted, which is what tells amx
    /// which pane and which conversation are meant — ask the agent to run it,
    /// or run it yourself in its shell mode. Nothing is started, nothing is
    /// sent, and the agent goes on with whatever it was doing.
    ///
    /// amx cut no worktree for it and holds no command it was launched with,
    /// so `stop` takes its pane and nothing else, and there is nothing for
    /// `resume` or `fork` to start again.
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

    /// Print the counts a status line has room for: ✽ moving, ⚠ waiting.
    ///
    /// Meant for tmux's own `status-right '#(amx statusline)'`, but it is
    /// plain text and prints nothing at all when no agent needs saying.
    Statusline,

    /// Check what amx needs from this machine, and what is missing.
    ///
    /// Nine things have to be true before an agent can run: tmux, the agent
    /// command, the config, amx's hooks in the vendor's settings, one amx on
    /// the PATH and this the one, a state directory to keep records in, no
    /// handoff still carrying the spawner's environment from before that
    /// moved to a file of its own, no agent already stopped at a screen
    /// the vendor puts in front of the work, and no tree amx cut still named
    /// in the agent's own trust store after the tree itself has gone.
    ///
    /// Where a tmux server is already running, a tenth: that the directory
    /// the server itself is standing in still exists. One that outlived its
    /// own working directory kills every pane it starts.
    Doctor {
        /// Install what is missing.
        #[arg(long)]
        fix: bool,
    },

    /// Wire an agent's hooks, so what it does reports back to amx.
    ///
    /// The agent is named, and never guessed: `amx setup claude` adds amx's
    /// hooks to `~/.claude/settings.json` beside whatever is already there,
    /// `amx setup pi` writes amx's extension where pi loads one from. Either
    /// file is copied aside before it is touched, and `amx uninstall` puts
    /// the copy back.
    ///
    /// A machine usually has more than one agent on it, so a bare `amx setup`
    /// prints the agents amx has an entry for and writes nothing.
    Setup {
        /// Which agent to wire: `claude` or `pi`.
        vendor: Option<String>,
    },

    /// Remove amx's hooks and state, restoring the settings backup.
    Uninstall,

    /// Print the completion script for a shell.
    ///
    /// It goes to stdout for the shell to keep or to read at every start,
    /// whichever that shell does with these:
    /// `amx completion fish > ~/.config/fish/completions/amx.fish`. What it
    /// offers is this build's own surface, so a kept copy is written again
    /// after an upgrade.
    Completion {
        /// The shell the script is written for.
        shell: clap_complete::Shell,
    },

    /// Record one vendor hook event. Reads the payload on stdin.
    #[command(name = "_hook", hide = true)]
    Hook,

    /// Record how the agent's command exited.
    #[command(name = "_exit", hide = true)]
    Exit { id: String, code: i32 },

    /// Start the agent's command inside its pane.
    #[command(name = "_boot", hide = true)]
    Boot { id: String },

    /// Let an idle agent's pane go, keeping everything else about it.
    #[command(name = "_park", hide = true)]
    Park { id: String },
}

#[derive(Debug, Args)]
pub struct NewArgs {
    /// What the agent should do.
    #[arg(
        value_parser = a_task,
        required_unless_present_any = ["file", "edit"],
        conflicts_with_all = ["file", "edit"]
    )]
    pub task: Option<String>,

    /// Read the task from this file instead, or from stdin for `-`.
    ///
    /// A brief long enough to be worth writing down is a brief nobody wants to
    /// quote into a shell: the file is read whole, its last newline taken off,
    /// and what is left is the task exactly as a typed one would have been.
    #[arg(long, value_name = "PATH")]
    pub file: Option<PathBuf>,

    /// Write the task in `$VISUAL`, `$EDITOR` or `vi` first.
    ///
    /// `--file` for the brief you have not written yet: an empty file is opened
    /// in your editor, and what you leave in it is the task, exactly as a typed
    /// one would have been. There is no task on the command line beside it, and
    /// no file either — that would be the task somewhere else already. An
    /// editor closed on an empty file is an empty task and refused as one, and
    /// an editor that exits unhappily starts no agent.
    #[arg(long, conflicts_with = "file")]
    pub edit: bool,

    /// Name the agent instead of deriving a name from the task.
    #[arg(long)]
    pub name: Option<String>,

    /// Run in this directory instead of the current one.
    #[arg(long)]
    pub dir: Option<PathBuf>,

    /// Run in the directory as it is, without a worktree of its own.
    #[arg(long)]
    pub no_worktree: bool,

    /// Cut the worktree from this ref instead of from what is checked out.
    ///
    /// Any branch, tag or commit git will resolve. It is what the agent starts
    /// on and what `diff` compares its work against, so `--base main` sends an
    /// agent off the branch the work belongs on rather than off whatever the
    /// last thing you were doing left behind. The `base` key says it for every
    /// spawn; this flag says it for one.
    #[arg(long, value_name = "REF")]
    pub base: Option<String>,

    /// Start the agent on this branch, which already exists.
    ///
    /// The work carries on where somebody left it: a branch this checkout has
    /// is cut on as it stands, and one only the origin has is fetched first. A
    /// leading `origin/` comes off, since that is how a branch on the forge is
    /// usually read out. The branch is what the record keeps, so the commits
    /// land on it and `stop` leaves it where it is. It says what the tree is
    /// on and where the work goes, so it is refused beside `--base`, `--pr`,
    /// `--no-worktree` and `--exec`; `--with-changes` stands beside it, since
    /// what you have not committed belongs on that branch as much as anywhere.
    #[arg(long, value_name = "NAME", conflicts_with_all = ["base", "pr", "no_worktree", "exec"])]
    pub branch: Option<String>,

    /// Start the agent on this pull request instead.
    ///
    /// `gh` is asked where the request's head is, its branch is fetched from
    /// the origin, and the tree is cut on that branch at the commit the
    /// request is at now. The branch is recorded, so the row says which
    /// request it is on and `stop` keeps the branch rather than deleting work
    /// somebody else is reviewing. The request says what the tree is cut from
    /// and where it goes, so it is refused beside `--base`, `--with-changes`,
    /// `--no-worktree` and `--exec`.
    #[arg(long, value_name = "N", conflicts_with_all = ["base", "with_changes", "no_worktree", "exec"])]
    pub pr: Option<u64>,

    /// Move the uncommitted work here into the agent's tree.
    ///
    /// The half hour you had already spent when you thought to start an agent
    /// on it. What git is tracking goes, staged or not, and the new file with
    /// it, and this directory is left as its last commit had it; what
    /// `.gitignore` names stays where it was made. There has to be a tree to
    /// move it into and something to move, so it is refused beside
    /// `--no-worktree` and `--exec`, and a directory with nothing uncommitted
    /// in it starts no agent.
    #[arg(long, conflicts_with_all = ["no_worktree", "exec"])]
    pub with_changes: bool,

    /// Run the task as a shell command rather than give it to an agent.
    ///
    /// The whole of it goes to `sh -c`, so a pipeline or an `&&` is one row,
    /// and the row ends done or failed by what the command exits with. It runs
    /// in the directory as it is: a command has no conversation to keep, so
    /// there is nothing for a worktree of its own to keep it apart from.
    ///
    /// There is no vendor here, which is why amx's four agent flags are
    /// refused beside it, and nothing is passed through: the command is the
    /// whole of what runs.
    #[arg(long, conflicts_with_all = ["AgentArgs", "vendor_args"])]
    pub exec: bool,

    /// The vendor and the dials for this one spawn, `None` when the caller
    /// named none of them and the config answers for all four.
    #[command(flatten)]
    pub agent: Option<AgentArgs>,

    /// Arguments passed to the agent command verbatim.
    #[arg(last = true, value_name = "AGENT_ARGS")]
    pub vendor_args: Vec<String>,
}

#[derive(Debug, Args, Default)]
pub struct AdoptArgs {
    /// What the agent is working on, for the row to say. Without one the row
    /// is named after the directory the pane is in.
    ///
    /// It is a label and nothing else: adopting sends the agent nothing.
    #[arg(long, value_name = "TEXT", value_parser = a_task)]
    pub task: Option<String>,

    /// Name the agent instead of deriving a name from the task.
    #[arg(long)]
    pub name: Option<String>,
}

/// Which vendor a spawn runs, and where its dials are pointed.
///
/// One group because they are one decision: a dial only means anything
/// against the vendor it is turned on, and the vendor amx is about to launch
/// is the one that says which dials exist at all. Every field is optional and
/// falls back to the config, which falls back to the vendor's own behaviour.
///
/// These are amx's flags, not the vendor's. Anything after `--` is the
/// vendor's own and is passed through untouched, including the same words:
/// `--model` before the separator turns amx's dial, `--model` after it is
/// claude's flag, and a dial stands down rather than send the flag twice.
#[derive(Debug, Args)]
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
/// One group because a question takes one answer. What is typed is the answer
/// itself, and the flag is there for the answer that reads as something else:
/// `--text 2` is the character `2` in the row the question offers for words of
/// your own, which is what the vendor writes down when it is typed there, while
/// a bare `2` is the second choice. Naming which one is meant is the only way
/// to say it, so the two cannot be given together.
#[derive(Debug, Args, Default)]
pub struct AnswerArgs {
    /// One key of the grammar, several choices, or words of your own.
    #[arg(
        value_name = "ANSWER",
        required_unless_present = "text",
        conflicts_with = "text"
    )]
    pub key: Option<String>,

    /// Words for the free-text row the question offers, whatever they look
    /// like.
    #[arg(long, value_name = "WORDS")]
    pub text: Option<String>,

    /// A note to send beside the choice, where the question draws a field for
    /// one.
    #[arg(long, value_name = "WORDS", conflicts_with = "text")]
    pub note: Option<String>,
}

#[derive(Debug, Args)]
pub struct StopArgs {
    pub id: String,

    /// Take the defaults for everything, asking nothing.
    #[arg(long)]
    pub force: bool,

    /// Remove the agent's record too, so nothing of it is left.
    #[arg(long)]
    pub delete: bool,

    /// What to do with the agent's worktree.
    #[arg(long, value_enum)]
    pub worktree: Option<Disposition>,

    /// What to do with the agent's branch.
    #[arg(long, value_enum)]
    pub branch: Option<Disposition>,
}

/// A task with something in it.
///
/// An empty task is not a small task: the vendor is handed an empty prompt,
/// and what starts is an agent sitting at its prompt with nothing to do,
/// holding a pane and a worktree while it does. It is easy to type by
/// accident — `amx new "$TASK"` with `TASK` unset is one — so it is answered
/// here, where nothing has been made yet and there is nothing to clean up.
///
/// Only wholly empty is refused. What is inside a task is the person's
/// business, and a task is passed on exactly as it was typed.
fn a_task(text: &str) -> Result<String, String> {
    match text.trim().is_empty() {
        true => Err("an agent needs something to do".to_string()),
        false => Ok(text.to_string()),
    }
}

/// One of the states amx reads an agent as being in.
///
/// The eight words `amx ls --json` prints and nothing else: `amx wait --for` is
/// given a state to hold out for, and a word amx has no state for is a wait
/// that would never end. Refused here, where clap answers it as the usage error
/// it is, and the refusal names all eight so the one that was meant is in front
/// of whoever mistyped it.
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

/// A task or a message read out of a file, or off stdin where the path is `-`.
///
/// The whole file, with one trailing newline taken off: every editor writes
/// that newline and nobody means it as part of the text, and a `$(cat brief)`
/// in a shell would have dropped it too. Nothing else is trimmed — what is
/// inside a task is the person's business here as much as it is when it is
/// typed.
///
/// Then through [`a_task`], because a file with nothing in it says exactly what
/// an empty argument says: an agent with nothing to do, holding a pane while it
/// does nothing. `send` refuses an empty file for the same reason — a message
/// of no words is a turn spent on nothing.
///
/// One reader for both verbs, so `--file` means the same thing wherever it is
/// typed: `amx new --file brief.md` and `amx send <id> --file notes.md` read
/// the file the same way and refuse the same files.
pub fn text_of(path: &Path) -> Result<String, String> {
    let text = match path == Path::new("-") {
        true => std::io::read_to_string(std::io::stdin()).map_err(|e| format!("stdin: {e}")),
        false => std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display())),
    }?;
    a_text(&text)
}

/// The task inside text somebody wrote somewhere other than the command line:
/// the last newline off, and then through [`a_task`].
///
/// The reading itself, apart from where the text came from, because a file is
/// not the only place it comes from: `new --edit` opens an editor, and the task
/// it is closed on is read the same way the same editor's file would have been.
pub fn a_text(text: &str) -> Result<String, String> {
    a_task(text.strip_suffix('\n').unwrap_or(text))
}

/// What becomes of a worktree or a branch when its agent stops.
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

/// The exit code for a command line clap refused to parse.
///
/// `--help` and `--version` arrive here as errors too, and they are not
/// failures: they exit `OK`. Everything else is a malformed command line, and
/// a malformed command line is never a state-machine outcome — it does not
/// borrow the blocked or failed codes.
pub fn usage_exit_code(err: &clap::Error) -> i32 {
    use clap::error::ErrorKind;
    match err.kind() {
        ErrorKind::DisplayHelp
        | ErrorKind::DisplayVersion
        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => crate::exit::OK,
        _ => crate::exit::USAGE,
    }
}

/// The completion script for one shell, as that shell reads it.
///
/// Written out of the parser above rather than kept by hand, so a verb or a
/// flag added there is offered without anyone remembering to say so.
///
/// It is rendered whole rather than streamed, because a shell reading half a
/// script is worse off than one reading none.
pub fn completion_script(shell: clap_complete::Shell) -> Vec<u8> {
    let mut script = Vec::new();
    clap_complete::generate(shell, &mut public_surface(), "amx", &mut script);
    script
}

/// The surface a completion is written from: what `amx --help` lists, and
/// nothing it hides.
///
/// clap_complete writes out every subcommand a command holds, `hide` or not,
/// so the four amx runs against itself have to be left behind rather than
/// marked. Everything in front of the verb is carried across: the flags, the
/// version that adds two more of them, and `disable_help_subcommand`, without
/// which clap would build a `help` verb amx does not answer to and the script
/// would offer that instead.
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
        // `amx --dir <path>` is the same door with a narrower question behind
        // it, so what it is not is a usage error looking for a verb.
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

        // A relative directory is a directory: the shell is standing in one,
        // and `--dir .` is the whole point of the flag.
        let cli = parse(&["amx", "ls", "--dir", "."]).unwrap();
        let Some(Command::Ls { dir, .. }) = cli.command else {
            panic!("expected ls");
        };
        assert_eq!(dir.as_deref(), Some(Path::new(".")));

        // The front door's own flag, in front of the verb, where somebody who
        // narrowed the view once will type it again.
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

        // Both of these say the agent works in the directory as it stands, and
        // there is nowhere for the work to be moved to.
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
            // A command is the whole of what runs, so there is nowhere for
            // arguments after the separator to go. Dropping them quietly is
            // the one thing worse than saying so.
            &["amx", "new", "--exec", "npm test", "--", "--watch"],
        ] {
            assert_eq!(code(argv), exit::USAGE, "{argv:?}");
        }
    }

    #[test]
    fn dials_a_spawn_that_names_none_of_them_leaves_the_config_its_say() {
        // Absent is not the same as a dial turned to some neutral value: the
        // config, and then the vendor's own behaviour, answer for what the
        // caller never mentioned.
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
        // amx's `--model` and claude's are the same word for the same thing,
        // and the separator is what tells them apart. Neither reads the other.
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
        // `--help` after the separator is the vendor's business: amx must not
        // print its own help and exit, it must pass the flag along.
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
            any,
            state,
            timeout,
        }) = cli.command
        else {
            panic!("expected wait");
        };
        assert_eq!(ids, ["a", "b", "c"]);
        assert!(!any, "every agent named, unless the caller says otherwise");
        assert_eq!(state, None, "a turn that is over, whichever way it ended");
        assert_eq!(timeout, None);

        let cli = parse(&["amx", "wait", "a", "--any", "--for", "idle"]).unwrap();
        let Some(Command::Wait { any, state, .. }) = cli.command else {
            panic!("expected wait");
        };
        assert!(any);
        assert_eq!(state, Some(Phase::Idle));

        // Every word `ls --json` prints is a state to wait for, and the
        // refusal for anything else names all eight of them.
        for phase in PHASES {
            assert_eq!(a_phase(phase.as_str()), Ok(phase));
        }
        let refusal = a_phase("sleeping").unwrap_err();
        for phase in PHASES {
            assert!(refusal.contains(phase.as_str()), "{refusal}");
        }
    }

    #[test]
    fn adopt_takes_a_label_for_the_row_and_nothing_about_where_to_look() {
        // Which pane and which conversation come from the environment of the
        // claude that ran it, so there is nothing to type: what is left is
        // what the row should say.
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

        // A label with nothing in it is not a label, and a pane is not
        // something this takes.
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
            // A narrowing has to say what to, at either door.
            &["amx", "ls", "--dir"],
            &["amx", "--dir"],
            &["amx", "status"],
            &["amx", "logs"],
            &["amx", "send", "fix-a1b"],
            &["amx", "interrupt"],
            // A rename says which agent and what to call it, and one of the
            // two on its own says neither.
            &["amx", "rename"],
            &["amx", "rename", "fix-a1b"],
            &["amx", "answer", "fix-a1b"],
            // Which of the two a thing that reads as both is has to be said,
            // and saying both says neither.
            &["amx", "answer", "fix-a1b", "2", "--text", "2"],
            // A note rides beside a choice, and there is no choice here.
            &["amx", "answer", "fix-a1b", "--note", "keep it short"],
            &["amx", "answer", "fix-a1b", "--text", "2", "--note", "short"],
            &["amx", "result", "fix-a1b", "--timeout", "soon"],
            // A wait says which agents, and holds out for a state amx has a
            // reading for: a word nobody knows is a wait that never ends.
            &["amx", "wait"],
            &["amx", "wait", "a", "--for", "sleeping"],
            // An attach says which agent, and the wall answering that is the
            // one case where naming it too says it twice. Two directions is
            // no direction, and neither is none of them beside no id.
            &["amx", "attach", "fix-a1b", "--next"],
            &["amx", "attach", "--next", "--prev"],
            &["amx", "attach"],
            // Going back is a direction of its own, and it is still one.
            &["amx", "attach", "fix-a1b", "--last"],
            &["amx", "attach", "--last", "--waiting"],
            // A reading of no lines is not a reading.
            &["amx", "logs", "fix-a1b", "--lines", "0"],
            &["amx", "logs", "fix-a1b", "--lines", "all"],
            &["amx", "stop", "fix-a1b", "--worktree", "burn"],
            &["amx", "resume"],
            &["amx", "resume", "fix-a1b", "--all"],
            &["amx", "resume", "--all", "carry on"],
            // There is no conversation to copy without one to copy it from,
            // and an empty turn is not a turn.
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

        // A dash is stdin, which is a file the shell holds open rather than
        // one with a name.
        let cli = parse(&["amx", "new", "--file", "-"]).unwrap();
        let Some(Command::New(args)) = cli.command else {
            panic!("expected new");
        };
        assert_eq!(args.file.as_deref(), Some(Path::new("-")));

        // A command is the row's task, so a command out of a file is one too.
        let cli = parse(&["amx", "new", "--exec", "--file", "release.sh"]).unwrap();
        let Some(Command::New(args)) = cli.command else {
            panic!("expected new");
        };
        assert!(args.exec);
        assert_eq!(args.file.as_deref(), Some(Path::new("release.sh")));

        // Two tasks is not a task: which of them was meant has to be said.
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

        // A command out of an editor is a task out of an editor, the same way
        // `--exec --file` is.
        let cli = parse(&["amx", "new", "--exec", "--edit"]).unwrap();
        let Some(Command::New(args)) = cli.command else {
            panic!("expected new");
        };
        assert!(args.exec && args.edit);

        // Two tasks is not a task, and no task at all is still none.
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

        // A message typed beside a file is two messages, which is none.
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
    fn clibatch_a_task_read_from_a_file_is_all_of_it_bar_the_last_newline() {
        let dir = tempfile::TempDir::new().unwrap();
        let brief = dir.path().join("brief.md");

        std::fs::write(&brief, "fix the login bug\n").unwrap();
        assert_eq!(text_of(&brief).unwrap(), "fix the login bug");

        // One newline, the one every editor writes at the end. Anything else
        // inside the file is the task as it was written.
        std::fs::write(&brief, "fix the login bug\n\n").unwrap();
        assert_eq!(text_of(&brief).unwrap(), "fix the login bug\n");
        std::fs::write(&brief, "  fix the login bug").unwrap();
        assert_eq!(text_of(&brief).unwrap(), "  fix the login bug");

        // A file with nothing in it is an empty task, and an empty task is
        // refused wherever it was typed.
        for written in ["", "\n", "  \n"] {
            std::fs::write(&brief, written).unwrap();
            assert!(text_of(&brief).is_err(), "{written:?}");
        }

        // And a file that is not there is named, because the name is what was
        // mistyped.
        let refusal = text_of(&dir.path().join("nothing.md")).unwrap_err();
        assert!(refusal.contains("nothing.md"), "{refusal}");
    }

    #[test]
    fn clibatch_text_written_somewhere_else_is_read_the_way_a_files_text_is() {
        // The reading [`text_of`] does once the file is read, which is what an
        // editor's answer goes through too: one trailing newline off, and an
        // empty task refused wherever it was written.
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
        // Only wholly empty is refused. What is inside a task is the person's
        // business, and amx tidying up their prompt for them is not a service.
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

        // It is typed once, into somebody's own tmux config, and never again.
        // Hiding it would leave the one verb people have to be told about as
        // the only one they cannot find in `amx --help`.
        let listed = Cli::command()
            .get_subcommands()
            .any(|verb| verb.get_name() == "statusline" && !verb.is_hide_set());
        assert!(listed, "statusline is not in help");

        // It takes nothing: what it prints is the same for everyone, and a
        // dial here would be one more thing to get wrong inside a config file.
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

        // Which shell is the whole of what it takes, and a shell amx cannot
        // write for is better said than guessed at.
        assert_eq!(code(&["amx", "completion"]), exit::USAGE);
        assert_eq!(code(&["amx", "completion", "nushell"]), exit::USAGE);
    }

    #[test]
    fn completion_offers_every_verb_a_person_can_type_and_none_of_the_others() {
        use clap_complete::Shell;

        // The four amx runs against itself are kept out of help because they
        // are not typed by hand, and a completion that types them for you is
        // help by another name.
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

    /// clap's own contract check: the derived surface is internally consistent
    /// (no duplicate names, no conflicting short flags).
    #[test]
    fn the_surface_is_well_formed() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    /// What amx says about itself: the README somebody reads before they run
    /// it, and the skill an agent is given instead of reading anything.
    const README: &str = include_str!("../README.md");
    const SKILL: &str = include_str!("../skill/amx/SKILL.md");

    /// Every verb, as `amx --help` lists them, with the flags each one takes.
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

    /// Every verb amx answers to at all, the three it keeps out of help
    /// included.
    fn every_verb() -> Vec<String> {
        use clap::CommandFactory;
        Cli::command()
            .get_subcommands()
            .map(|verb| verb.get_name().to_string())
            .collect()
    }

    /// Every key the view binds, read out of the table its `?` overlay is
    /// drawn from.
    ///
    /// That table belongs to the view and is not public to the rest of the
    /// crate, so it is read as text. What pays for the parser is the length the
    /// table declares: a table this cannot read comes back the wrong length and
    /// says so, rather than quietly agreeing with whatever the README claims.
    fn keys_the_view_binds() -> Vec<&'static str> {
        let source = include_str!("tui/paint/help.rs");
        let (_, table) = source
            .split_once("const HELP: [(&str, &str); ")
            .expect("the table the overlay is drawn from");
        let (count, table) = table.split_once("] = [").expect("how many keys it holds");
        let (table, _) = table.split_once("\n];").expect("the end of it");

        // Two literals to an entry, the key and then what it does.
        let written: Vec<&str> = table.split('"').skip(1).step_by(2).collect();
        let keys: Vec<&str> = written.into_iter().step_by(2).collect();
        assert_eq!(
            keys.len(),
            count.parse::<usize>().expect("a count"),
            "the keys table is not the shape this reads it in: {keys:?}"
        );
        keys
    }

    /// The verbs a document puts in a command line, read out of its code
    /// alone: prose says `amx` about the program itself, and only code says it
    /// about something a person can type.
    fn verbs_named_in(text: &str) -> Vec<String> {
        let mut named = Vec::new();
        for (at, chunk) in text.split("```").enumerate() {
            let code: Vec<&str> = match at % 2 == 1 {
                true => vec![chunk],
                // Outside a fence, the code is whatever is between backticks.
                false => chunk.split('`').skip(1).step_by(2).collect(),
            };
            for line in code.iter().flat_map(|code| code.lines()) {
                // A comment inside a fence is prose that happens to be in one.
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
        // A key column may name two keys, and a person looking one of them up
        // is looking up the one they pressed.
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

    /// What one verb's help offers, as `amx --help` lists it.
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
        // The one verb whose help has to be a grammar rather than a sentence:
        // what it takes is not guessable, and getting it wrong types something
        // at an agent that cannot be taken back.
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
        // Both of them write the number out, so both go stale silently. What
        // is in the findings does not change how many checks are made of them.
        let checks = crate::verbs::doctor::report(&crate::verbs::doctor::Findings {
            tmux: None,
            vendor: String::new(),
            vendor_path: None,
            config: PathBuf::new(),
            config_warnings: Vec::new(),
            home: PathBuf::new(),
            wire: PathBuf::new(),
            wired: crate::install::Wired::Nothing,
            plugin: false,
            command: String::new(),
            exe: PathBuf::new(),
            on_path: Vec::new(),
            state_root: PathBuf::new(),
            state_error: None,
            dirty_handoffs: Vec::new(),
            parked: Vec::new(),
            // The counted checks are the ones every machine is asked. The
            // server check is asked only where there is a server to ask about,
            // so it is deliberately absent here.
            server: None,
            store: None,
            stale: Vec::new(),
        });
        let counted = [
            "no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
        ]
        .get(checks.len())
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
            "doctor makes {} checks and its help says otherwise: {long}",
            checks.len()
        );
        assert!(
            README.contains(&format!("the {counted} things")),
            "doctor makes {} checks and the README says otherwise",
            checks.len()
        );
    }

    #[test]
    fn docs_neither_document_names_a_verb_amx_does_not_have() {
        // The other half of parity, and the half that rots quietly: a command
        // line somebody copies out of the README fails at the shell, and one an
        // agent copies out of the skill fails where nobody is reading.
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
        // The exit codes are the whole interface a caller has, so a skill that
        // leaves one out is one that meets it unprepared.
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

        // The question arrives during the wait and comes back where the answer
        // would have been. A caller that does not know to read it there has
        // nothing to answer with.
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
}
