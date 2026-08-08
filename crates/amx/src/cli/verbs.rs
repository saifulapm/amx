//! The verbs with nothing behind them on the wire.
//!
//! Every subcommand here is a `Command` builder for something the method table
//! does not carry: process lifecycle, hidden plumbing, and the public verbs
//! that compose existing capabilities client-side. The trees that *shape* the
//! root or a generated group stay in [`super`], because those change when the
//! table does; these change when a verb does.
//!
//! Split out of [`super`] by X02 before M4's two new verbs pushed that file
//! past the soft budget (`docs/11-m4-plan.md` R-M4-5). The move is mechanical:
//! not a line of any tree below changed.
//!
//! # Task ownership
//!
//! The trees are complete; the command modules behind them are stubs. **V09**
//! fills `_hook`, **V10** `integration`, **V16** `skill`. For M3: **W10** fills
//! `update`, **W11** `_bridge`, **W12** `work`, **W13** `layout` and `apply`,
//! and **W06** `_handoff-caps`. For M4: **X16** fills `agents`, **X07** `keys`.

use clap::{Arg, ArgAction, Command};

use super::JSON;

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
pub(super) fn hook() -> Command {
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
pub(super) fn integration() -> Command {
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
pub(super) fn skill() -> Command {
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
pub(super) fn update() -> Command {
    Command::new("update")
        .about("Check for a newer amx, or install one without dropping a pane")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("check")
                .about("Report whether a newer amx is published")
                // Symmetric with `apply` on purpose: W03's tree put `--channel`
                // on the writing verb only, which left the read-only one unable
                // to look anywhere but the configured channel — exactly
                // backwards, since checking somewhere else is the cheap half.
                .arg(channel()),
        )
        .subcommand(
            Command::new("apply")
                .about("Install the newest amx and hand the running session over to it")
                .arg(channel()),
        )
}

/// `--channel <URL>`, on both update verbs.
fn channel() -> Arg {
    Arg::new("channel")
        .long("channel")
        .value_name("URL")
        .help("The channel manifest to read [default: the configured channel]")
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
pub(super) fn work() -> Command {
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
pub(super) fn layout() -> Command {
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
pub(super) fn apply() -> Command {
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
pub(super) fn bridge() -> Command {
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
pub(super) fn handoff_caps() -> Command {
    Command::new("_handoff-caps")
        .hide(true)
        .about("Print this binary's version and handoff/protocol windows as JSON")
}

/// `amx agents [--watch] [--json] [--workspace]` — D15 surface 3.
///
/// **X16** fills it. A top-level verb rather than a method-table row, and
/// D-M4-11 is why there are two spellings: the table already generates
/// `amx agent list` as the machine surface, and this is the same reply rendered
/// for a person — with `--json` printing it verbatim so a consumer never has to
/// know a human form exists.
///
/// The workflow it exists for is a phone SSH window with no client attached:
/// `--watch` is the read-only mission-control screen, and it must work at 45
/// columns.
pub(super) fn agents() -> Command {
    Command::new("agents")
        .about("Show every agent's status, reason, age and last line")
        .arg(
            Arg::new("watch")
                .long("watch")
                .action(ArgAction::SetTrue)
                .help("Keep the table live until `q`, redialling across a server swap"),
        )
        .arg(
            Arg::new(JSON)
                .long("json")
                .action(ArgAction::SetTrue)
                .help("Print the `agent.list` reply verbatim instead of a table"),
        )
        .arg(
            Arg::new("workspace")
                .long("workspace")
                .value_name("NAME")
                .help("Scope either form to one workspace"),
        )
}

/// `amx keys` — print the resolved keybinding table (04 §7).
///
/// **X07** fills it, along with the `[keys]` section it reads. It reaches no
/// server: the bindings are resolved entirely client-side out of the config
/// file, so this answers with no session running, which is also what makes it
/// the thing to run when a rebound prefix has left you unable to reach the
/// prefix layer.
pub(super) fn keys() -> Command {
    Command::new("keys")
        .about("Print the resolved keybindings, and where each one came from")
        .arg(
            Arg::new(JSON)
                .long("json")
                .action(ArgAction::SetTrue)
                .help("Print the resolved table as JSON instead of a table"),
        )
}
