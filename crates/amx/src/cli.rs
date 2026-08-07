//! The clap tree: the generated table plus the verbs that have no wire method.
//!
//! 04 §4 derives the CLI from the same method table as the wire names and the
//! dispatch trait, "which fixes W6's four hand-synced lists". So the `ping`,
//! `workspace …` and `pane …` subcommands are not written here — they are
//! [`amx_proto::control::cli::method_commands`], and they arrive with their
//! parameters already on them: `--params '<JSON>'` on every row, plus the typed
//! flags of [`amx_proto::control::cli::ROWS`] on the verbs a human drives. Both
//! are built beside the payload types they translate into, which is what keeps
//! a row's parameters coming from one place.
//!
//! What *is* written here is the set of verbs with nothing behind them on the
//! wire: `attach`, `server` and `session …` are process lifecycle, not control
//! calls, and a method table row for them would be a row no server could
//! handle. M2 adds four more of the same kind — `_hook`, `integration`,
//! `skill`, and the streaming half of `events` — and V02 plants all of them
//! here rather than letting four wave tasks each edit this file. That is the
//! U01 precedent: `cli.rs` is a file every milestone wants a line in, so its
//! lines land once, in the contracts task.
//!
//! M3 adds six, planted by W03 on the same terms (`docs/09-m3-plan.md` §4):
//! `update`, `work`, `layout` and `apply` are public verbs that compose
//! existing capabilities client-side, and `_bridge` and `_handoff-caps` are
//! hidden — one is a byte splice, the other a single exec that prints what a
//! binary can be handed. `amx session handoff` needs no line here at all: it is
//! a real method-table row, so the generated tree carries it, flags and all.
//!
//! # Task ownership
//!
//! The trees below are complete; the command modules behind them are stubs.
//! **V09** fills `_hook`, **V10** `integration`, **V11** `events`, **V16**
//! `skill`. For M3: **W10** fills `update`, **W11** `_bridge`, **W12** `work`,
//! **W13** `layout` and `apply`, and **W06** `_handoff-caps` (the pre-flight
//! probe its orchestrator runs).

use clap::{Arg, ArgAction, Command};

pub use amx_proto::control::cli::PARAMS;

/// The global `--session` argument's id.
pub const SESSION: &str = "session";

/// The `--json` argument's id, carried by `session report` and `events`.
pub const JSON: &str = "json";

/// The `--after-seq` argument's id, carried by `events`.
pub const AFTER_SEQ: &str = "after-seq";

/// The whole `amx` command tree.
#[must_use]
pub fn cli() -> Command {
    let mut root = Command::new("amx")
        .about("A minimal, keyboard-only agent terminal multiplexer")
        .version(env!("CARGO_PKG_VERSION"))
        .subcommand_required(false)
        .arg(
            Arg::new(SESSION)
                .long("session")
                .short('s')
                .global(true)
                .value_name("NAME")
                .help("The named session to use [env: AMX_SESSION] [default: default]"),
        )
        .subcommand(attach())
        .subcommand(server())
        .subcommand(hook())
        .subcommand(integration())
        .subcommand(skill())
        .subcommand(update())
        .subcommand(work())
        .subcommand(layout())
        .subcommand(apply())
        .subcommand(bridge())
        .subcommand(handoff_caps())
        .subcommands(amx_proto::control::cli::method_commands());

    // The generated tree owns the `session` group (it carries `session.state`);
    // the lifecycle verbs merge into it rather than shadowing it, so one
    // `amx session …` namespace serves both.
    root = root.mut_subcommand("session", |generated| {
        session_lifecycle(generated.about("Session state and the lifecycle of running sessions"))
    });

    // Same shape for `events`, which the table owns through `events.subscribe`
    // while the *streaming* verb 04 §8 promises — `amx events --json` — is a
    // long-lived client, not a one-shot call.
    root = root.mut_subcommand("events", |generated| {
        events_stream(generated.about("Subscribe to the session's event stream"))
    });

    root
}

/// `amx attach` — this terminal, one session.
fn attach() -> Command {
    Command::new("attach")
        .about("Attach this terminal to a session")
        .arg(
            Arg::new("pane")
                .long("pane")
                .value_name("TARGET")
                .help("Attach full-screen to one pane, with no chrome"),
        )
        .arg(
            Arg::new("takeover")
                .long("takeover")
                .action(ArgAction::SetTrue)
                .requires("pane")
                .help("Take size authority for the pane from other clients"),
        )
}

/// `amx server` — the daemon entry point.
fn server() -> Command {
    Command::new("server")
        .about("Run the session server in the foreground")
        .long_about(
            "Run the session server in the foreground.\n\n\
             `amx` starts this for you, detached, when nothing answers on the \
             session socket. Run it yourself to watch a session's logs, or \
             under a service manager.",
        )
}

/// The `amx session …` lifecycle verbs, added onto the generated group.
///
/// The generated `report` leaf is mutated rather than added: it is a real
/// method-table row, and all it gains here is the `--json` escape hatch out of
/// the human table `cmd::session` prints for it.
fn session_lifecycle(group: Command) -> Command {
    let name = || {
        Arg::new("name")
            .value_name("NAME")
            .help("The session to act on [default: the selected session]")
    };
    group
        .mut_subcommand("report", |report| {
            report.arg(
                Arg::new(JSON)
                    .long("json")
                    .action(ArgAction::SetTrue)
                    .help("Print the reply as JSON instead of a table"),
            )
        })
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(Command::new("list").about("List the sessions with a server running"))
        .subcommand(
            Command::new("attach")
                .about("Attach this terminal to a named session")
                .arg(name()),
        )
        .subcommand(
            Command::new("stop")
                .about("Shut a session's server down, leaving its state on disk")
                .arg(name()),
        )
        .subcommand(
            Command::new("delete")
                .about("Remove a stopped session's runtime and state directories")
                .arg(name()),
        )
}

/// `amx _hook <agent>` — the agent hook emitter (D-M2-4).
///
/// Hidden: it is not a verb a user runs, it is the command amx writes into an
/// agent's hook configuration. One static binary rather than herdr's
/// sh-plus-python heredoc, which silently no-ops when `python3` is missing
/// while `integration status` still reports `current` — amx's emitter *is* the
/// binary that already speaks the protocol, so that failure mode is deleted
/// rather than detected.
///
/// **V09** fills it. The contract it must keep: read one payload from stdin,
/// issue one `agent.report`, and **exit 0 silently whatever happens**, under a
/// total budget of about 500 ms. A hook must never break or slow a turn.
fn hook() -> Command {
    Command::new("_hook")
        .hide(true)
        .about("Forward one agent hook event to the session server")
        .arg(
            Arg::new("agent")
                .required(true)
                .value_name("AGENT")
                .help("The registry id of the agent this hook was installed for"),
        )
        .arg(
            Arg::new("marker")
                .long("marker")
                .value_name("N")
                .help("The installed asset's version marker, checked by `integration status`"),
        )
}

/// `amx integration install|uninstall|status` — the hook lifecycle (04 §8).
///
/// **V10** fills these. Two things V01 measured that `status` has to say out
/// loud: Claude Code's hooks run without any approval step of their own, but
/// **only in a folder the user has already trusted** — in a brand new one the
/// next interactive launch asks once, and until it is answered no hook fires at
/// all. And Codex gates hooks on an interactive, hash-pinned "trust these"
/// prompt whose state this spike found no way to read, so `status` must report
/// that it cannot see it rather than implying the hooks are live.
fn integration() -> Command {
    let agent = || {
        Arg::new("agent")
            .value_name("AGENT")
            .help("Which agent's integration [default: every agent in the registry]")
    };
    Command::new("integration")
        .about("Install, remove or check amx's agent hook integrations")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("install")
                .about("Write amx's hook entries into an agent's configuration")
                .arg(agent()),
        )
        .subcommand(
            Command::new("uninstall")
                .about("Remove amx's own hook entries, leaving foreign ones untouched")
                .arg(agent()),
        )
        .subcommand(
            Command::new("status")
                .about("Report whether each agent's integration is current")
                .arg(agent()),
        )
}

/// `amx skill install` — the in-binary agent skill (04 §8, K10).
///
/// **V16** fills it: an asset written out of the binary that teaches an agent
/// to drive amx, gated on the `AMX_ENV=1` and pane/workspace variables V07
/// injects.
fn skill() -> Command {
    Command::new("skill")
        .about("Install the amx agent skill")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("install")
                .about("Write the agent skill into the current project")
                .arg(
                    Arg::new("path")
                        .long("path")
                        .value_name("DIR")
                        .help("Where to write it [default: the current directory]"),
                ),
        )
}

/// `amx update check|apply` — self-update (D-M3-8).
///
/// **W10** fills these. What `check` must be honest about: until a release
/// pipeline exists there is no manifest at the default channel URL, and the
/// answer to that is the plain sentence, not an error and not a stub pretending
/// otherwise (R-M3-4). `apply` stages, verifies a sha256, renames atomically
/// over the running exe — legal on unix; ETXTBSY guards writes, not renames —
/// and then asks the running session for `session.handoff`.
fn update() -> Command {
    Command::new("update")
        .about("Check for a newer amx, or install one without dropping a pane")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(Command::new("check").about("Report whether a newer amx is published"))
        .subcommand(
            Command::new("apply")
                .about("Install the newest amx and hand the running session over to it")
                .arg(
                    Arg::new("channel")
                        .long("channel")
                        .value_name("URL")
                        .help("The channel manifest to read [default: the configured channel]"),
                ),
        )
}

/// `amx work <branch> [--kind]` / `amx work done [branch]` — worktrees
/// (D-M3-10).
///
/// **W12** fills these. The server learns no git: the CLI runs `git worktree
/// add` as argv — never a shell string — and the association it creates lives
/// on the workspace as the block W03 added to state and snapshot. `done`
/// collapses all three (agent, workspace, tree) and refuses a dirty tree
/// without `--force`, which is the destructive-op caution M1's delete work set
/// as policy.
fn work() -> Command {
    Command::new("work")
        .about("Start a workspace on a git worktree, or take one down")
        .subcommand_required(false)
        .arg_required_else_help(true)
        .arg(
            Arg::new("branch")
                .value_name("BRANCH")
                .help("The branch to check out into a worktree of its own"),
        )
        .arg(
            Arg::new("kind")
                .long("kind")
                .value_name("AGENT")
                .help("Start this agent in the new workspace, by registry id"),
        )
        .subcommand(
            Command::new("done")
                .about("Kill the workspace and remove its worktree")
                .arg(
                    Arg::new("branch")
                        .value_name("BRANCH")
                        .help("Which one [default: the workspace this pane is in]"),
                )
                .arg(
                    Arg::new("force")
                        .long("force")
                        .action(ArgAction::SetTrue)
                        .help("Remove the worktree even when it has uncommitted changes"),
                ),
        )
}

/// `amx layout export` — a session's shape as a file (D-M3-11).
///
/// **W13** fills it, entirely client-side over the public verbs: export renders
/// `session.state` into TOML. Session refs are deliberately not exported — a
/// layout is a shape, not a conversation — which is also why the pair with
/// [`apply`] is one-way per invocation rather than a sync.
fn layout() -> Command {
    Command::new("layout")
        .about("Export this session's shape")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("export")
                .about("Write this session's workspaces, splits and agent kinds to a file")
                .arg(
                    Arg::new("out")
                        .long("out")
                        .value_name("FILE")
                        .help("Where to write it [default: stdout]"),
                ),
        )
}

/// `amx apply <file>` — replay a layout through the public verbs (D-M3-11).
///
/// **W13** fills it. A top-level verb rather than `layout apply`, as 04 §4's
/// CLI surface spells it. It adds workspaces to the running session and
/// suffixes names on collision; replacing a session is `session stop` plus
/// this, two explicit steps.
fn apply() -> Command {
    Command::new("apply")
        .about("Build workspaces, splits and agents from a layout file")
        .arg(
            Arg::new("file")
                .required(true)
                .value_name("FILE")
                .help("The layout file to apply"),
        )
}

/// `amx _bridge` — the byte splice an SSH remote speaks through (D-M3-9).
///
/// Hidden: it is not a verb a user runs, it is what `amx --remote host` runs on
/// the far side, as `ssh host exec amx _bridge`. **W11** fills it, and it is a
/// splice and nothing more — resolve the session, connect, copy both ways, and
/// exit with the connect error before the first protocol byte if there is one.
fn bridge() -> Command {
    Command::new("_bridge")
        .hide(true)
        .about("Splice this process's stdio onto a session socket")
        .arg(
            Arg::new("daemonize")
                .long("daemonize")
                .action(ArgAction::SetTrue)
                .help("Start the session server first when nothing answers"),
        )
}

/// `amx _handoff-caps` — what this binary can be handed (D-M3-6 point 2).
///
/// Hidden, and one exec with JSON on stdout: `{version, handoff: [min,max],
/// proto: [min,max]}`. It exists so an exporter can refuse a wrong successor
/// *before* it quiesces a single pane — herdr validates after pausing
/// everything and pays a full quiesce and rollback for a binary it could have
/// rejected for free. **W06** fills it, since the orchestrator is the only
/// caller and the windows it prints are the ones that orchestrator checks.
fn handoff_caps() -> Command {
    Command::new("_handoff-caps")
        .hide(true)
        .about("Print this binary's version and handoff/protocol windows as JSON")
}

/// The streaming half of `amx events`, added onto the generated group.
///
/// `events.subscribe` is a real method row and stays generated; this adds the
/// long-lived consumer 04 §8 promises — "any program can `amx events --json`".
/// **V11** filled it, and its help text is where the gap-resync contract is
/// documented for the humans who write those programs: a `gap` delivery means
/// re-query `session.state` and resume from the seq it carries.
fn events_stream(group: Command) -> Command {
    group
        .subcommand_required(false)
        .long_about(
            "Subscribe to the session's event stream and print one delivery per line.\n\n\
             Every line is either an event envelope or a `gap`. A gap is not an \
             error and must not be skipped: it means this consumer fell behind \
             the server's replay buffer, and the events it names are gone. The \
             recovery is fixed — re-query `amx session state`, which carries the \
             bus sequence it was captured at, and resume from there with \
             `--after-seq`. A consumer that ignores gaps silently misses \
             transitions, which is the one failure the event bus is designed to \
             make impossible to have unknowingly.",
        )
        .arg(
            Arg::new(JSON)
                .long("json")
                .action(ArgAction::SetTrue)
                .help("Print each delivery as one line of NDJSON, including `gap`"),
        )
        .arg(
            Arg::new(AFTER_SEQ)
                .long("after-seq")
                .value_name("SEQ")
                .help("Resume after this bus sequence, as `session.state` reported it"),
        )
}
