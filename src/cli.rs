//! The command line: every verb amx answers to.
//!
//! Bare `amx` has no subcommand — that is the front door (the cockpit), not a
//! usage error. The three underscore verbs are amx talking to itself from
//! inside a pane or a vendor hook; they are hidden from help but are as much
//! of the contract as the rest.

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "amx",
    version,
    about = "Run coding agents as tmux panes",
    disable_help_subcommand = true
)]
pub struct Cli {
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
            Result { .. } => "result",
            Attach { .. } => "attach",
            Stop(_) => "stop",
            Diff { .. } => "diff",
            Resume { .. } => "resume",
            Events { .. } => "events",
            Statusline => "statusline",
            Doctor { .. } => "doctor",
            Uninstall => "uninstall",
            Hook => "_hook",
            Exit { .. } => "_exit",
            Boot { .. } => "_boot",
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
    },

    /// Show one agent, and which signal that state came from.
    Status {
        id: String,
        /// Print the stable JSON instead of the summary.
        #[arg(long)]
        json: bool,
    },

    /// Send a message to a working or idle agent.
    Send { id: String, text: String },

    /// Answer a waiting agent's question: y, n, 1-9, enter or esc.
    Answer { id: String, key: String },

    /// Wait for the agent's turn to end and print its answer.
    Result {
        id: String,
        /// Give up after this many seconds.
        #[arg(long, value_name = "SECONDS")]
        timeout: Option<u64>,
    },

    /// Attach to the agent's pane.
    Attach { id: String },

    /// Stop an agent and decide what happens to its worktree and branch.
    Stop(StopArgs),

    /// Show the agent's worktree against the commit it started from.
    Diff {
        id: String,
        /// Summarise the patch instead of printing it.
        #[arg(long)]
        stat: bool,
    },

    /// Restart a stopped agent, continuing its recorded session.
    Resume {
        #[arg(required_unless_present = "all")]
        id: Option<String>,
        /// Every stopped agent, as after a tmux server death.
        #[arg(long, conflicts_with = "id")]
        all: bool,
    },

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

    /// Check tmux, the agent command, the config and the hook wiring.
    Doctor {
        /// Install what is missing.
        #[arg(long)]
        fix: bool,
    },

    /// Remove amx's hooks and state, restoring the settings backup.
    Uninstall,

    /// Record one vendor hook event. Reads the payload on stdin.
    #[command(name = "_hook", hide = true)]
    Hook,

    /// Record how the agent's command exited.
    #[command(name = "_exit", hide = true)]
    Exit { id: String, code: i32 },

    /// Start the agent's command inside its pane.
    #[command(name = "_boot", hide = true)]
    Boot { id: String },
}

#[derive(Debug, Args)]
pub struct NewArgs {
    /// What the agent should do.
    #[arg(value_parser = a_task)]
    pub task: String,

    /// Start out of sight, in a hidden session.
    #[arg(long)]
    pub bg: bool,

    /// Name the agent instead of deriving a name from the task.
    #[arg(long)]
    pub name: Option<String>,

    /// Run in this directory instead of the current one.
    #[arg(long)]
    pub dir: Option<PathBuf>,

    /// Run in the directory as it is, without a worktree of its own.
    #[arg(long)]
    pub no_worktree: bool,

    /// The vendor and the dials for this one spawn, `None` when the caller
    /// named none of them and the config answers for all four.
    #[command(flatten)]
    pub agent: Option<AgentArgs>,

    /// Arguments passed to the agent command verbatim.
    #[arg(last = true, value_name = "AGENT_ARGS")]
    pub vendor_args: Vec<String>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exit;

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
    }

    #[test]
    fn every_verb_parses() {
        let lines: &[(&[&str], &str)] = &[
            (&["amx", "new", "fix the bug"], "new"),
            (&["amx", "ls"], "ls"),
            (&["amx", "ls", "--json"], "ls"),
            (&["amx", "status", "fix-a1b"], "status"),
            (&["amx", "status", "fix-a1b", "--json"], "status"),
            (&["amx", "send", "fix-a1b", "carry on"], "send"),
            (&["amx", "answer", "fix-a1b", "y"], "answer"),
            (&["amx", "result", "fix-a1b"], "result"),
            (&["amx", "result", "fix-a1b", "--timeout", "30"], "result"),
            (&["amx", "attach", "fix-a1b"], "attach"),
            (&["amx", "stop", "fix-a1b"], "stop"),
            (&["amx", "stop", "fix-a1b", "--force"], "stop"),
            (&["amx", "stop", "fix-a1b", "--delete"], "stop"),
            (&["amx", "diff", "fix-a1b"], "diff"),
            (&["amx", "diff", "fix-a1b", "--stat"], "diff"),
            (&["amx", "resume", "fix-a1b"], "resume"),
            (&["amx", "resume", "--all"], "resume"),
            (&["amx", "events"], "events"),
            (&["amx", "events", "fix-a1b", "--follow"], "events"),
            (&["amx", "events", "--json"], "events"),
            (&["amx", "statusline"], "statusline"),
            (&["amx", "doctor"], "doctor"),
            (&["amx", "doctor", "--fix"], "doctor"),
            (&["amx", "uninstall"], "uninstall"),
            (&["amx", "_hook"], "_hook"),
            (&["amx", "_exit", "fix-a1b", "0"], "_exit"),
            (&["amx", "_boot", "fix-a1b"], "_boot"),
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
            "--bg",
            "--name",
            "importer",
            "--dir",
            "/srv/app",
            "--no-worktree",
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
        assert_eq!(args.task, "port the importer");
        assert!(args.bg);
        assert_eq!(args.name.as_deref(), Some("importer"));
        assert_eq!(args.dir, Some(PathBuf::from("/srv/app")));
        assert!(args.no_worktree);
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
        assert_eq!(args.task, "fix the log-in bug");
        assert_eq!(args.vendor_args, ["--help"]);
    }

    #[test]
    fn the_separator_belongs_to_the_vendor_so_it_cannot_stand_in_for_a_task() {
        assert_eq!(code(&["amx", "new", "--", "--model", "opus"]), exit::USAGE);
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
            &["amx", "status"],
            &["amx", "send", "fix-a1b"],
            &["amx", "result", "fix-a1b", "--timeout", "soon"],
            &["amx", "stop", "fix-a1b", "--worktree", "burn"],
            &["amx", "resume"],
            &["amx", "resume", "fix-a1b", "--all"],
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
            &["amx", "new", "", "--bg"],
        ] {
            assert_eq!(code(argv), exit::USAGE, "{argv:?}");
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
        assert_eq!(args.task, "  fix the login bug\n");
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

    /// clap's own contract check: the derived surface is internally consistent
    /// (no duplicate names, no conflicting short flags).
    #[test]
    fn the_surface_is_well_formed() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
