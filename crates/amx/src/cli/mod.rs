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
//! M4 adds two, planted by X02 on the same terms (`docs/11-m4-plan.md` §5):
//! `agents` renders `agent.list` for a person (D-M4-11 — the table already
//! generates the machine spelling, `amx agent list`), and `keys` prints the
//! keybinding table 04 §7 promised and D-M4-8 finally makes data.
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

mod verbs;

use clap::{Arg, ArgAction, Command};

pub use amx_proto::control::cli::PARAMS;

/// The global `--session` argument's id.
pub const SESSION: &str = "session";

/// The `--json` argument's id, carried by `session report` and `events`.
pub const JSON: &str = "json";

/// The global `--remote` argument's id.
///
/// Declared for `--help` and never matched: [`crate::remote::split`] removes
/// the flag from `argv` before this tree parses anything.
pub const REMOTE: &str = "remote";

/// The `--after-seq` argument's id, carried by `events`.
pub const AFTER_SEQ: &str = "after-seq";

/// The `--handoff-import` argument's id, carried by `server`.
///
/// Hidden surface (`docs/09-m3-plan.md` §4): an exporter spawns
/// `amx server --handoff-import <socket>` and writes the handoff token to its
/// stdin, and nobody types it. A flag on an existing verb rather than a
/// routing arm, which is why W03 left it out and W07 adds it here.
pub const HANDOFF_IMPORT: &str = "handoff-import";

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
        // Documentary, and deliberately so. `--remote` selects *which machine
        // parses the rest of the command line*, so `remote::split` takes it off
        // `argv` in `main` before clap ever sees it — clap will therefore never
        // match this argument. It is declared anyway because a flag missing
        // from `amx --help` is a flag nobody finds: W11 had to strip it and
        // recorded the cost, and this is the line that pays it.
        .arg(
            Arg::new(REMOTE)
                .long("remote")
                .global(true)
                .value_name("HOST")
                .help("Attach to the session on HOST over ssh, through `amx _bridge`"),
        )
        .subcommand(attach())
        .subcommand(server())
        .subcommand(verbs::hook())
        .subcommand(verbs::integration())
        .subcommand(verbs::skill())
        .subcommand(verbs::update())
        .subcommand(verbs::work())
        .subcommand(verbs::layout())
        .subcommand(verbs::apply())
        .subcommand(verbs::bridge())
        .subcommand(verbs::handoff_caps())
        .subcommand(verbs::agents())
        .subcommand(verbs::keys())
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
        .arg(
            Arg::new(HANDOFF_IMPORT)
                .long("handoff-import")
                .value_name("SOCKET")
                .hide(true)
                .help("Take a running session over from the exporter on SOCKET"),
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
