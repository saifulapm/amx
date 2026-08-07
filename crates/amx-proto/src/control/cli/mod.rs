//! The CLI tree, built from the method table.
//!
//! 04 §4 derives the clap tree from the same table as the wire names and the
//! dispatch trait: "adding a method is one enum variant + one handler". This
//! module is the consumer of [`SPECS`](super::SPECS) that makes the third of
//! W6's four hand-synced lists disappear — the binary adds its hand-written
//! verbs (`attach`, `server`, `session …`) on top of what this returns.
//!
//! A generated leaf carries its parameters two ways, and both are built here so
//! a row's parameters keep coming from one place:
//!
//! - `--params '<JSON>'`, on **every** row, forever. It is the escape hatch for
//!   the rows nothing types by hand and the only way to reach a field no flag
//!   covers.
//! - The typed flags of [`flag`], on the rows a human drives all day. They are
//!   a translation into the row's existing payload type and nothing else: no
//!   new wire surface, no second way for a call to mean something.
//!
//! The two are mutually exclusive per invocation, enforced by clap: every flag
//! a leaf carries joins one [`ArgGroup`], and `--params` conflicts with the
//! group. Mixing them would raise the question of which wins for a field both
//! named, and there is no good answer to it.

use std::collections::BTreeMap;

use clap::{Arg, ArgGroup, ArgMatches, Command};
use serde_json::Value;

use super::{Method, MethodSpec, SPECS};

mod agent;
mod flag;
mod pane;
mod wait;

pub use flag::{
    Field, FlagError, FlagRow, Source, TARGET, TEXT, TIMEOUT, encode, from_arg, millis,
    params_only, parse_opt, required, target_arg, text_arg, timeout, timeout_arg,
};

/// The `--params` argument's id, carried by every generated leaf.
pub const PARAMS: &str = "params";

/// The id of the group holding one leaf's typed flags.
///
/// Never typed by a user; it exists so `--params` can conflict with "any flag"
/// without naming them, which is what keeps a new flag from silently becoming
/// combinable with the JSON object.
const FLAGS: &str = "flags";

/// Every method row that carries typed flags.
///
/// The list is the promise of 05's M2 read literally — the verbs it spells with
/// flags — plus the rest of each of those verbs' own groups, so a human does
/// not have to remember which half of `pane` speaks which language.
pub const ROWS: &[FlagRow] = &[
    wait::WAIT,
    wait::WAIT_OUTPUT,
    agent::START,
    agent::PROMPT,
    agent::EXPLAIN,
    agent::NEXT,
    pane::RUN,
    pane::READ,
    pane::SEND_TEXT,
    pane::SEND_KEYS,
];

/// The flags this method carries, if it carries any.
#[must_use]
pub fn row(method: Method) -> Option<&'static FlagRow> {
    ROWS.iter().find(|row| row.method == method)
}

/// Build the subcommand tree for every method in the table.
///
/// Each leaf comes with its parameters attached: `--params` always, and the
/// typed flags of whichever [`FlagRow`] names it.
#[must_use]
pub fn method_commands() -> Vec<Command> {
    let mut root = Node::default();
    for spec in SPECS {
        root.insert(spec.cli, spec);
    }
    root.into_commands()
}

/// Whether the table declares a method reachable at this CLI path.
#[must_use]
pub fn has_path(path: &[&str]) -> bool {
    SPECS.iter().any(|spec| spec.cli == path)
}

/// The parameters this leaf's typed flags name.
///
/// Called only when `--params` was *not* given, since clap has already refused
/// the combination. A row with no flags, or one whose flags are all optional
/// and all absent, answers with the empty object — which is what `amx agent
/// next` and `amx session state` have always sent.
///
/// # Errors
///
/// [`FlagError`] when a required flag is missing or a value is not one it can
/// be. Every message names `--params` as the way past it.
pub fn params_from_flags(method: Method, matches: &ArgMatches) -> Result<Value, FlagError> {
    match row(method) {
        Some(row) => (row.parse)(matches),
        None => Ok(Value::Object(serde_json::Map::new())),
    }
}

/// The `--params` argument a generated leaf carries.
fn params_arg() -> Arg {
    Arg::new(PARAMS)
        .long("params")
        .value_name("JSON")
        .help("The call's parameters, as one JSON object")
}

/// Attach `method`'s parameters — its flags, and `--params` — to its leaf.
fn parameters(command: Command, method: Method) -> Command {
    let Some(row) = row(method) else {
        return command.arg(params_arg());
    };
    let command = (row.args)(command);
    let ids: Vec<&'static str> = row.fields.iter().filter_map(Field::arg_id).collect();
    if ids.is_empty() {
        return command.arg(params_arg());
    }
    command
        .group(ArgGroup::new(FLAGS).args(ids).multiple(true))
        .arg(params_arg().conflicts_with(FLAGS))
}

#[derive(Default)]
struct Node {
    spec: Option<&'static MethodSpec>,
    children: BTreeMap<&'static str, Node>,
}

impl Node {
    fn insert(&mut self, path: &[&'static str], spec: &'static MethodSpec) {
        match path.split_first() {
            None => self.spec = Some(spec),
            Some((head, rest)) => self.children.entry(head).or_default().insert(rest, spec),
        }
    }

    fn into_commands(self) -> Vec<Command> {
        self.children
            .into_iter()
            .map(|(name, node)| node.into_command(name))
            .collect()
    }

    fn into_command(self, name: &'static str) -> Command {
        let spec = self.spec;
        let mut command = Command::new(name);
        if let Some(spec) = spec {
            command = command.about(spec.about);
        }
        let children = self.into_commands();
        if !children.is_empty() {
            return command
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommands(children);
        }
        match spec {
            Some(spec) => parameters(command, spec.method),
            None => command,
        }
    }
}
