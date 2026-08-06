//! The CLI tree, built from the method table.
//!
//! 04 §4 derives the clap tree from the same table as the wire names and the
//! dispatch trait: "adding a method is one enum variant + one handler". This
//! module is the consumer of [`SPECS`](super::SPECS) that makes the third of
//! W6's four hand-synced lists disappear — the binary adds its hand-written
//! verbs (`attach`, `server`, `session …`) on top of what this returns.

use std::collections::BTreeMap;

use clap::Command;

use super::SPECS;

/// Build the subcommand tree for every method in the table.
///
/// Returns bare `Command`s named after each method's CLI path; argument
/// definitions per method are added by the binary as the parameter types grow.
#[must_use]
pub fn method_commands() -> Vec<Command> {
    let mut root = Node::default();
    for spec in SPECS {
        root.insert(spec.cli, spec.about);
    }
    root.into_commands()
}

/// Whether the table declares a method reachable at this CLI path.
#[must_use]
pub fn has_path(path: &[&str]) -> bool {
    SPECS.iter().any(|spec| spec.cli == path)
}

#[derive(Default)]
struct Node {
    about: Option<&'static str>,
    children: BTreeMap<&'static str, Node>,
}

impl Node {
    fn insert(&mut self, path: &[&'static str], about: &'static str) {
        match path.split_first() {
            None => self.about = Some(about),
            Some((head, rest)) => self.children.entry(head).or_default().insert(rest, about),
        }
    }

    fn into_commands(self) -> Vec<Command> {
        self.children
            .into_iter()
            .map(|(name, node)| node.into_command(name))
            .collect()
    }

    fn into_command(self, name: &'static str) -> Command {
        let about = self.about;
        let mut command = Command::new(name);
        if let Some(about) = about {
            command = command.about(about);
        }
        let children = self.into_commands();
        if children.is_empty() {
            command
        } else {
            command
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommands(children)
        }
    }
}
