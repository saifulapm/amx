//! The clap tree: the generated table plus the verbs that have no wire method.
//!
//! 04 §4 derives the CLI from the same method table as the wire names and the
//! dispatch trait, "which fixes W6's four hand-synced lists". So the `ping`,
//! `workspace …` and `pane …` subcommands are not written here — they are
//! [`amx_proto::control::cli::method_commands`], and the only thing this module
//! adds to them is the `--params` argument every wire call takes.
//!
//! What *is* written here is the set of verbs with nothing behind them on the
//! wire: `attach`, `server` and `session …` are process lifecycle, not control
//! calls, and a method table row for them would be a row no server could
//! handle.

use clap::{Arg, ArgAction, Command};

/// The global `--session` argument's id.
pub const SESSION: &str = "session";

/// The `--params` argument's id, carried by every generated method command.
pub const PARAMS: &str = "params";

/// The `--json` argument's id, carried by `session report` alone.
pub const JSON: &str = "json";

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
        .subcommands(amx_proto::control::cli::method_commands());

    // The generated tree owns the `session` group (it carries `session.state`);
    // the lifecycle verbs merge into it rather than shadowing it, so one
    // `amx session …` namespace serves both.
    root = root.mut_subcommand("session", |generated| {
        session_lifecycle(generated.about("Session state and the lifecycle of running sessions"))
    });

    // Every leaf of the generated tree takes its parameters as one JSON object,
    // so a method that grows a field needs no edit here. Typed flags per method
    // are a later refinement of the same table, not a second source of truth.
    for spec in amx_proto::control::SPECS {
        root = add_params(root, spec.cli);
    }
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

/// The parameters argument a generated method command carries.
fn params_arg() -> Arg {
    Arg::new(PARAMS)
        .long("params")
        .value_name("JSON")
        .default_value("{}")
        .help("The call's parameters, as one JSON object")
}

/// Add [`params_arg`] to the command at `path` within `cmd`.
fn add_params(cmd: Command, path: &[&str]) -> Command {
    match path.split_first() {
        None => cmd.arg(params_arg()),
        Some((head, rest)) => cmd.mut_subcommand(head, |sub| add_params(sub, rest)),
    }
}
