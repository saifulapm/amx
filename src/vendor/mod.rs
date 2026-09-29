//! The vendor table: what amx knows about each agent program it can launch.
//!
//! A vendor is a descriptor in a static table, not a trait. Adding a vendor
//! means adding an entry whose fields can be checked against the vendor's own
//! `--help` and source. This module holds launch metadata and the rule that
//! turns a resolved dial into argv; what a vendor's screens mean lives in
//! `rules` and `derive`. The rest of the crate reaches the table through
//! `registry`, which works in terms of the `agent` config key.

use std::collections::BTreeMap;

pub mod claude;
pub mod codex;
pub mod opencode;
pub mod pi;

/// A fixture vendor that differs from claude in every field. Test builds only,
/// and not in the table.
#[cfg(test)]
pub mod second;

/// One launch dial (model, permission or effort) a vendor declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialSpec {
    /// The values a cycle key offers. Always starts at [`DEFAULT`].
    pub cycle: &'static [&'static str],
    /// Whether a value outside `cycle` is still passed on.
    pub open: bool,
    /// The argv flag [`inject`] writes. Empty for a dial carried in `env`.
    pub flag: &'static str,
    /// The setting name for a dial written as `flag key=value`, such as codex's
    /// `-c model_reasoning_effort=high`.
    ///
    /// For a dial carried in `env`, the JSON key the value goes under.
    pub key: Option<&'static str>,
    /// Whether the flag alone is written, with no value: a closed dial with
    /// one value besides [`DEFAULT`], such as opencode's `--auto`.
    pub bare: bool,
    /// The environment variable the dial is carried in instead of argv, as
    /// `{"<key>": "<value>"}`. See [`env_dials`]. Only model dials use this.
    pub env: Option<&'static str>,
}

impl DialSpec {
    /// A dial written as `flag value` that passes on any value.
    pub const fn open(flag: &'static str, cycle: &'static [&'static str]) -> DialSpec {
        DialSpec {
            cycle,
            open: true,
            flag,
            key: None,
            bare: false,
            env: None,
        }
    }

    /// A dial written as `flag value` that takes only what `cycle` names.
    pub const fn closed(flag: &'static str, cycle: &'static [&'static str]) -> DialSpec {
        DialSpec {
            open: false,
            ..DialSpec::open(flag, cycle)
        }
    }
}

/// Where a vendor's list of models comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Models {
    /// The model dial's cycle, less [`DEFAULT`]. Nothing is run.
    Cycle,
    /// The vendor prints its models when run with these arguments.
    ///
    /// Running it costs a process, so the caller decides when to.
    Printed(&'static [&'static str]),
    /// The vendor prints its models as JSON when run with these arguments.
    ///
    /// The output is an object whose `models` each carry a `slug` and a
    /// `visibility`; only slugs with visibility `list` are offered.
    Json(&'static [&'static str]),
}

/// A point in a turn that amx listens for.
///
/// Each vendor names these events in its own words (see [`Wiring`]); what
/// each one does to an agent's record is decided in `hook`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    /// A session began, and names its id.
    Started,
    /// A prompt was submitted and a turn started.
    Prompted,
    /// A message queued behind the running turn was delivered into it.
    ///
    /// Only pi and opencode report this; claude and codex report a second
    /// `Prompted` instead.
    Taken,
    /// A tool is about to run.
    Calling,
    /// Permission to run a tool is being asked for.
    Asked,
    /// Permission was refused and the tool did not run.
    Refused,
    /// The vendor sent a notice about the session.
    Notified,
    /// The turn ended.
    Ended,
}

#[cfg(test)]
impl Moment {
    /// Every moment amx listens for.
    pub const ALL: [Moment; 8] = [
        Moment::Started,
        Moment::Prompted,
        Moment::Taken,
        Moment::Calling,
        Moment::Asked,
        Moment::Refused,
        Moment::Notified,
        Moment::Ended,
    ];
}

/// One moment under the vendor's own event name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wiring {
    pub moment: Moment,
    /// The vendor's event name, as written in its hook config and payloads.
    pub event: &'static str,
    /// Whether the vendor's entry for this event takes a tool matcher.
    pub matched: bool,
}

impl Wiring {
    /// `moment` under the vendor's name `event`, with no tool matcher.
    pub const fn new(moment: Moment, event: &'static str) -> Wiring {
        Wiring {
            moment,
            event,
            matched: false,
        }
    }

    /// `moment` under the vendor's name `event`, for an entry that takes a
    /// tool matcher.
    pub const fn with_matcher(moment: Moment, event: &'static str) -> Wiring {
        Wiring {
            matched: true,
            ..Wiring::new(moment, event)
        }
    }
}

/// How a vendor reports to amx: where the wiring goes, its event names, and
/// the words its payloads use.
///
/// Present exactly when the vendor has [`Capability::Hooks`]. An empty string
/// field means the vendor has no such thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hooks {
    /// The wire `amx setup` installs and `amx uninstall` removes.
    pub wire: Wire,
    /// Extra wires installed only by `amx setup <vendor> --subagent`.
    ///
    /// doctor repairs one only where it is already installed, and never
    /// reports one missing.
    pub opt_in: &'static [Wire],
    /// The moments this vendor reports, in wiring order, each at most once.
    pub events: &'static [Wiring],
    /// The vendor's matcher for every tool.
    pub matcher: &'static str,
    /// The tool that asks the person a question and waits for the answer.
    pub question_tool: &'static str,
    /// The notification type for a session that has sat idle.
    pub idle_notice: &'static str,
    /// The notification type that repeats an open permission box.
    pub permission_notice: &'static str,
    /// The text of the vendor's permission box, with [`TOOL`] for the tool.
    pub permission_sentence: &'static str,
    /// Prefixes of prompts the vendor submits itself, such as a background
    /// task finishing. A turn started by one is not an answer to anybody.
    pub injected: &'static [&'static str],
    /// The session-start `source` that means a new conversation, as opposed
    /// to a resume, clear or compact.
    pub fresh_start: Option<&'static str>,
    /// Payload `kind` values that make a notice a question for the person.
    pub question_kinds: &'static [&'static str],
}

/// How amx's hook command is installed for a vendor. Paths are relative to
/// the home directory.
///
/// Readers of hook payloads cannot tell the shapes apart; the one behavioural
/// difference is [`Wire::listens`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    /// A file of amx's own, written whole where the vendor loads extensions,
    /// that runs the hook command itself. pi's extension.
    File {
        path: &'static str,
        body: &'static str,
    },
    /// A plugin directory of amx's own; `files` are paths relative to `dir`.
    /// The vendor's hook runner reads the hook config inside it, so no file of
    /// the person's is edited. claude's plugin.
    Plugin {
        dir: &'static str,
        files: &'static [(&'static str, &'static str)],
    },
    /// amx's groups merged into the vendor's `hooks.json`, each trusted in the
    /// `config.toml` beside it. codex's hooks.
    ///
    /// The directory is `$<dir_env>` when set, else `dir`; `body` is a hooks
    /// file holding only amx's groups.
    Hooks {
        dir_env: &'static str,
        dir: &'static str,
        body: &'static str,
    },
    /// A [`Wire::File`] placed at `path` inside a directory a variable can
    /// move: `$<dir_env>` when set, else `dir`. opencode's TUI plugin.
    Placed {
        dir_env: &'static str,
        dir: &'static str,
        path: &'static str,
        body: &'static str,
    },
}

impl Wire {
    /// Whether the code behind this wire reads what the hook command prints.
    ///
    /// File and placed wires are amx's own code in the vendor process and
    /// read the answer (the record's directory, for a pane amx did not
    /// start). Plugin and hooks wires run through the vendor's hook runner,
    /// which shows a hook's stdout to the model (claude for
    /// `UserPromptSubmit`, codex always), so the hook command stays silent.
    pub fn listens(self) -> bool {
        matches!(self, Wire::File { .. } | Wire::Placed { .. })
    }

    /// The wire's path under the home directory. For a hooks or placed wire,
    /// the directory used when its variable is unset.
    pub fn path(&self) -> &'static str {
        match self {
            Wire::File { path, .. } => path,
            Wire::Plugin { dir, .. } => dir,
            Wire::Hooks { dir, .. } | Wire::Placed { dir, .. } => dir,
        }
    }
}

impl Hooks {
    /// The moment `event` names, if amx listens for it.
    pub fn moment(&self, event: &str) -> Option<Moment> {
        self.events
            .iter()
            .find(|wiring| wiring.event == event)
            .map(|wiring| wiring.moment)
    }

    /// The vendor's permission-box sentence about `tool`, or `None` from a
    /// vendor that draws no box.
    ///
    /// Must match the text of the notification the vendor sends a few seconds
    /// later, which replaces it on the record.
    pub fn permission_sentence(&self, tool: &str) -> Option<String> {
        (!self.permission_sentence.is_empty())
            .then(|| self.permission_sentence.replace(TOOL, &rendered(tool)))
    }
}

/// A tool name as claude 2.1.237 writes it into a sentence.
///
/// Takes the last `__` segment (MCP tools arrive as `mcp__<server>__<tool>`),
/// turns underscores into spaces and uppercases the first letter after any
/// non-alphanumeric character, like the vendor's `\b\w`.
fn rendered(tool: &str) -> String {
    let mut boundary = true;
    tool.rsplit("__")
        .next()
        .unwrap_or(tool)
        .chars()
        .map(|letter| {
            let letter = if letter == '_' { ' ' } else { letter };
            let raised = if boundary {
                letter.to_ascii_uppercase()
            } else {
                letter
            };
            boundary = !letter.is_ascii_alphanumeric();
            raised
        })
        .collect()
}

/// The placeholder for the tool name in [`Hooks::permission_sentence`].
pub const TOOL: &str = "{tool}";

/// How a vendor with [`Capability::Fork`] branches a session into a copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkSpec {
    /// A valueless flag written beside the resume flag, such as claude's
    /// `--fork-session`. The copy opens through [`SessionSpec::resume`].
    Marker(&'static str),
    /// A flag whose value is the origin session's id, such as pi's `--fork`.
    Origin(&'static str),
    /// A subcommand right after the program, then the origin's id, such as
    /// `codex fork <id>`. No resume words are written beside it.
    Subcommand(&'static str),
}

/// The flags that choose which session a vendor process opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionSpec {
    /// The flag that starts a session under an id amx chose, or `None` if
    /// amx never does that for this vendor.
    pub start: Option<&'static str>,
    /// How to continue an existing session.
    pub resume: Resume,
    /// Other flags that also name a session, which a resume or fork removes.
    /// Never lists the resume flag itself.
    pub conflicts: &'static [&'static str],
    /// How to branch a session. `None` without [`Capability::Fork`].
    pub fork: Option<ForkSpec>,
}

/// How a vendor is told to continue an existing session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// A flag carrying the id, written after the other flags. `joined` writes
    /// it as `flag=<id>` (claude's `--resume=<id>`) instead of two words.
    Flag { flag: &'static str, joined: bool },
    /// A subcommand right after the program, then the id: `codex resume <id>`.
    Subcommand(&'static str),
}

impl SessionSpec {
    /// The words that continue `session`. A [`Resume::Subcommand`] goes right
    /// after the program; a flag goes with the caller's other flags.
    pub fn resume_args(&self, session: &str) -> Vec<String> {
        match self.resume {
            Resume::Flag { flag, joined: true } => vec![format!("{flag}={session}")],
            Resume::Flag {
                flag,
                joined: false,
            } => vec![flag.to_string(), session.to_string()],
            Resume::Subcommand(word) => vec![word.to_string(), session.to_string()],
        }
    }

    /// Whether other session flags take their id with `=`, as the resume flag
    /// does. False for a subcommand.
    pub fn joined(&self) -> bool {
        matches!(self.resume, Resume::Flag { joined: true, .. })
    }

    /// Whether `word` names a session: `Some(true)` if its value is the next
    /// word, `Some(false)` if the value is joined with `=`, `None` otherwise.
    ///
    /// `first` says whether the word is right after the program, the only
    /// place a subcommand counts. A [`ForkSpec::Origin`] flag counts too: a
    /// fork's recorded command carries it beside the minted id, and a
    /// respawn that kept both would ask to branch into a session that already
    /// exists, which pi refuses. It is not in `conflicts` because conflicts
    /// also suppress the start flag, and a hand-written fork on `amx new`
    /// still needs an id minted.
    pub fn names_a_session(&self, word: &str, first: bool) -> Option<bool> {
        let resumes = matches!(self.resume, Resume::Subcommand(subcommand) if word == subcommand);
        let forks =
            matches!(self.fork, Some(ForkSpec::Subcommand(subcommand)) if word == subcommand);
        if (resumes || forks) && first && !word.starts_with('-') {
            return Some(true);
        }
        let mut flags: Vec<&str> = self.conflicts.to_vec();
        if let Resume::Flag { flag, .. } = self.resume {
            flags.push(flag);
        }
        // A marker carries no id: the origin rides on the resume flag, which
        // is already in the list.
        if let Some(ForkSpec::Origin(flag)) = self.fork {
            flags.push(flag);
        }
        flags.into_iter().find_map(|flag| {
            if word == flag {
                return Some(true);
            }
            word.strip_prefix(flag)
                .is_some_and(|rest| rest.starts_with('='))
                .then_some(false)
        })
    }
}

/// One vendor's entry in the table.
///
/// A `None` dial means the vendor has no such dial, which differs from a dial
/// left at [`DEFAULT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vendor {
    /// The program name the table is keyed by.
    pub name: &'static str,
    pub model: Option<DialSpec>,
    /// Where the vendor's models are listed, for finding which vendor offers
    /// a model somebody named.
    pub models: Models,
    pub permission: Option<DialSpec>,
    pub effort: Option<DialSpec>,
    /// The session flags. `None` if not measured, and then resume and fork
    /// cannot be spelled.
    pub session: Option<SessionSpec>,
    /// The variable the vendor sets in its child processes to name the
    /// session they belong to. Required for [`Capability::Adopt`].
    pub session_env: Option<&'static str>,
    /// The vendor's own variables that mark a process as part of a session,
    /// stripped from the environment of a pane amx starts.
    ///
    /// A vendor that inherits these believes it is a child of the spawning
    /// session. Variables common to every pane are the caller's concern.
    pub not_inherited: &'static [&'static str],
    /// What amx may ask this vendor to do. A verb asking for anything else
    /// refuses up front.
    pub capabilities: &'static [Capability],
    /// How the vendor reports what it is doing. Present exactly when
    /// [`Capability::Hooks`] is claimed; read by `install` and `hook`.
    pub hooks: Option<Hooks>,
    /// The TOML document of screen rules `crate::rules` reads for this
    /// vendor's panes.
    ///
    /// `None` until the screens have been measured against a running
    /// program; the pane is then watched but never classified.
    pub screens: Option<&'static str>,
    /// The on-disk conversation format, for `crate::conversation`.
    ///
    /// The file's path arrives on a hook payload, so this only matters with
    /// [`Capability::Transcript`].
    pub transcript: Option<Transcript>,
    /// Where the vendor loads skills, commands and agents from, for task-line
    /// completion. `None` if not measured.
    pub catalog: Option<Catalog>,
    /// The word after which the vendor reads everything as the message,
    /// written before a task, resume message or fork prompt so that one
    /// starting with `-` or `@` is not read as a flag or attachment.
    pub ends_options: Option<&'static str>,
    /// Whether a message word starting with `@` is still read as a file to
    /// attach after [`ends_options`](Self::ends_options). amx prefixes such a
    /// word with a space.
    pub attaches_at: bool,
    /// Whether cancelling a turn puts queued messages back into the composer
    /// unsent. `amx interrupt` then records them, and `send` waits for the
    /// next prompt before typing.
    pub restores_queued_on_cancel: bool,
    /// The flag a message is passed on as one `flag=<text>` word, for a
    /// vendor with no positional prompt.
    pub prompt_flag: Option<&'static str>,
    /// Characters that open a composer popup at the start of a word. A
    /// message whose last word starts with one gets a trailing space so Enter
    /// submits instead of picking from the popup.
    pub popups: &'static [char],
    /// How many Escapes, 300 ms apart, cancel a turn. opencode's first press
    /// only arms the cancel.
    pub cancel_presses: u8,
    /// The signal sent to the pane's process to end the turn before `stop`
    /// kills the pane.
    pub interrupt_signal: Option<nix::sys::signal::Signal>,
    /// Flags every process of this vendor is started with. `new` writes them
    /// right after the program; `resume` and `fork` keep them in place.
    pub launch: &'static [&'static str],
}

/// A directory a vendor loads from, relative to one of two roots.
///
/// A `*` segment matches every directory at that level, for layouts such as
/// claude's plugin cache (market, plugin, version).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// Relative to the home directory.
    Person(&'static str),
    /// Relative to the project the agent runs in.
    Project(&'static str),
}

impl Place {
    /// The path relative to its root.
    pub fn path(&self) -> &'static str {
        match self {
            Place::Person(path) | Place::Project(path) => path,
        }
    }
}

/// What a vendor can be asked for by name on a task line, and where to find
/// it. `crate::catalog` reads the places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Catalog {
    /// Skill directories, in the vendor's read order.
    pub skills: &'static [Place],
    /// Command directories (pi calls them prompts).
    pub commands: &'static [Place],
    /// Agent definition directories. Empty if the vendor has none.
    pub agents: &'static [Place],
    /// The flag that runs a session as one of those agents, used when a task
    /// line starts with an agent's name.
    pub agent_flag: Option<&'static str>,
    /// Commands built into the vendor, without the sigil.
    pub builtins: &'static [&'static str],
    /// The character that starts a catalog word: `/` for claude, pi and
    /// opencode, `$` for codex.
    pub sigil: char,
    /// What goes between the sigil and a skill's name: `skill:` for pi, empty
    /// for claude.
    pub skill_prefix: &'static str,
}

/// The format of a vendor's on-disk conversation, one JSON document per line.
///
/// `crate::conversation` interprets each format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transcript {
    /// Entries typed `user` or `assistant` at the top level, with blocks
    /// under `message.content`. A prompt is a `user` entry with string
    /// content; a tool result is a `user` entry with blocks. As of claude
    /// 2.1.240.
    Claude,
    /// Entries typed `message`, with the role (`user`, `assistant`,
    /// `toolResult`) under `message.role` and blocks under `message.content`;
    /// a tool call is a `toolCall` block. As of pi 0.84.4.
    Pi,
    /// codex's rollout under `$CODEX_HOME/sessions`: messages under
    /// `response_item` and turn ends under `event_msg`. As of codex 0.157.1.
    Codex,
    /// The message list amx's opencode plugin writes to
    /// `$AMX_DIR/opencode-messages.jsonl` at each turn's end, one message per
    /// line as `message.list` returns it in opencode 2.0.16. amx never opens
    /// opencode's own database.
    Opencode,
}

/// Something amx can do only with the vendor's cooperation.
///
/// A program with no table entry has none of these: amx only watches its pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Reports its activity to amx's hook command.
    Hooks,
    /// Keeps the conversation in a file amx can read.
    Transcript,
    /// Can continue a session it opened.
    Resume,
    /// Can branch a session into a copy, leaving the original intact.
    Fork,
    /// Can be adopted after being started outside amx; needs
    /// [`Vendor::session_env`].
    Adopt,
    /// Shows a folder-trust screen that amx can answer for a worktree it cut.
    Trust,
}

/// The value every dial starts at: the vendor's own default, expressed by
/// passing no flag at all.
pub const DEFAULT: &str = "default";

/// Every vendor amx has an entry for, in the order a cycle key offers them.
///
/// claude must stay first: `rules::of` uses `entries().first()` for the
/// screens of an unregistered agent.
static TABLE: [Vendor; 4] = [claude::VENDOR, pi::VENDOR, codex::VENDOR, opencode::VENDOR];

/// The hooks a record's `agent` reports through, or `None` for a vendor
/// without hooks.
///
/// Payloads do not say which vendor sent them, so they are read in the
/// record's vendor's words only. An unregistered agent command reads as
/// claude, on the assumption that it wraps claude.
pub fn hooks_for(agent: &str) -> Option<Hooks> {
    match find(agent) {
        Some(vendor) => vendor.hooks,
        None => claude::VENDOR.hooks,
    }
}

impl Vendor {
    /// Whether this vendor has `what`.
    pub fn can(&self, what: Capability) -> bool {
        self.capabilities.contains(&what)
    }

    /// The three dials with their names, in declaration order, which is the
    /// order their flags appear in argv.
    pub fn dials(&self) -> [(&'static str, Option<DialSpec>); 3] {
        [
            ("model", self.model),
            ("permission", self.permission),
            ("effort", self.effort),
        ]
    }
}

/// The vendor the agent command line `agent` runs, if it has an entry.
pub fn find(agent: &str) -> Option<&'static Vendor> {
    let program = program(agent);
    TABLE.iter().find(|vendor| vendor.name == program)
}

/// Every registered vendor, in the order a cycle key offers them.
pub fn table() -> &'static [Vendor] {
    &TABLE
}

/// The program an agent command line runs, without arguments or directory:
/// `/opt/pi/bin/pi --approve` runs `pi`.
pub fn program(agent: &str) -> &str {
    let first = agent.split_whitespace().next().unwrap_or(agent);
    first.rsplit('/').next().unwrap_or(first)
}

/// Whether `value` may be passed to `dial`: [`DEFAULT`], a value in the
/// cycle, or anything on an open dial.
pub fn accepts(dial: &DialSpec, value: &str) -> bool {
    value == DEFAULT || dial.cycle.contains(&value) || dial.open
}

/// The argv for the resolved dials, followed by `vendor_args`.
///
/// Each dial not at [`DEFAULT`] adds its flag and value (`key=value` for a
/// keyed dial, the flag alone for a bare one). A dial is skipped when its
/// flag already appears in `vendor_args` or in `carried` (the arguments on
/// the agent command line), so a flag the caller wrote always wins. Dials
/// carried in the environment are left to [`env_dials`].
pub fn inject(
    vendor: &Vendor,
    model: &str,
    permission: &str,
    effort: &str,
    carried: &[String],
    vendor_args: &[String],
) -> Vec<String> {
    let mut injected: Vec<String> = Vec::new();

    for ((_, dial), value) in vendor.dials().into_iter().zip([model, permission, effort]) {
        let Some(spec) = dial else {
            continue;
        };
        if spec.env.is_some() || value == DEFAULT || already(carried, vendor_args, &spec) {
            continue;
        }
        injected.push(spec.flag.to_string());
        if spec.bare {
            continue;
        }
        injected.push(match spec.key {
            Some(key) => format!("{key}={value}"),
            None => value.to_string(),
        });
    }

    injected.extend(vendor_args.iter().cloned());
    injected
}

/// The environment variable a model dial carried in `env` adds to a new pane:
/// `{"<key>":"<value>"}` under its variable.
///
/// Empty for a flag dial, for [`DEFAULT`], or when `env` already sets the
/// variable, which wins as a hand-written flag does in [`inject`].
pub fn env_dials(
    vendor: &Vendor,
    model: &str,
    env: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let Some(DialSpec {
        env: Some(variable),
        key: Some(key),
        ..
    }) = vendor.model
    else {
        return Vec::new();
    };
    if model == DEFAULT || env.contains_key(variable) {
        return Vec::new();
    }
    vec![(
        variable.to_string(),
        serde_json::json!({ key: model }).to_string(),
    )]
}

/// Whether the argv already carries this dial's flag, as a whole word or as
/// `flag=value`. A plain prefix match would take `--models` for `--model`.
///
/// For a keyed dial the flag must be followed by this dial's key (`flag
/// key=..` or `flag=key=..`); the same flag with another key is not a match.
fn already(carried: &[String], vendor_args: &[String], dial: &DialSpec) -> bool {
    let flag = dial.flag;
    let equals = format!("{flag}=");
    let Some(key) = dial.key else {
        return carried
            .iter()
            .chain(vendor_args)
            .any(|arg| arg == flag || arg.starts_with(&equals));
    };
    let setting = format!("{key}=");
    [carried, vendor_args].into_iter().any(|args| {
        args.iter().any(|arg| {
            arg.strip_prefix(&equals)
                .is_some_and(|rest| rest.starts_with(&setting))
        }) || args
            .windows(2)
            .any(|pair| pair[0] == flag && pair[1].starts_with(&setting))
    })
}

#[cfg(test)]
mod tests {
    use super::second::{BRANCHING, ELSEWHERE, SECOND};
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    /// Every vendor in the table plus the three test fixtures, so each check
    /// below also runs against shapes unlike claude's.
    fn known() -> Vec<&'static Vendor> {
        table()
            .iter()
            .chain([&SECOND, &BRANCHING, &ELSEWHERE])
            .collect()
    }

    #[test]
    fn the_table_lists_claude_first_then_pi_then_codex_then_opencode() {
        let names: Vec<_> = table().iter().map(|v| v.name).collect();
        assert_eq!(names, ["claude", "pi", "codex", "opencode"]);
        assert!(
            !names.contains(&SECOND.name),
            "the second vendor is a fixture, not an agent anybody can spawn"
        );
    }

    #[test]
    fn an_agent_the_table_never_heard_of_has_no_entry() {
        // mock-claude is what the e2e tests spawn, and it must stay
        // unregistered.
        assert!(find("mock-claude").is_none());
        assert!(find("aider").is_none());
        assert!(find("").is_none());
    }

    #[test]
    fn an_agent_command_is_read_as_the_program_it_runs() {
        assert_eq!(
            find("claude --dangerously-skip-permissions").map(|v| v.name),
            Some("claude")
        );
        assert!(find("my-claude").is_none(), "a longer name is not claude");
        assert_eq!(program("claude --model opus"), "claude");
        assert_eq!(program("claude"), "claude");
    }

    #[test]
    fn an_agent_spelled_as_a_path_is_the_vendor_it_names() {
        let agent = "/opt/pi/bin/pi --approve";
        assert_eq!(program(agent), "pi");
        let vendor = find(agent).expect("a path to pi is pi");
        assert_eq!(vendor.name, "pi");
        assert_eq!(vendor.model, pi::VENDOR.model);
        assert_eq!(vendor.session, pi::VENDOR.session);
        assert!(std::ptr::eq(
            crate::rules::of(agent),
            crate::rules::of("pi")
        ));
        assert_eq!(hooks_for(agent), pi::VENDOR.hooks);
        assert!(find("/usr/local/bin/my-claude").is_none());
        assert_eq!(program("./wrap"), "wrap");
    }

    #[test]
    fn every_cycle_starts_at_the_sentinel_that_injects_nothing() {
        // Otherwise a cycle key could never return to the vendor's default.
        for vendor in known() {
            for (which, dial) in vendor.dials() {
                if let Some(spec) = dial {
                    assert_eq!(
                        spec.cycle.first(),
                        Some(&DEFAULT),
                        "{}'s {which} dial starts somewhere else",
                        vendor.name
                    );
                }
            }
        }
    }

    #[test]
    fn every_dial_a_vendor_declares_names_a_flag_of_its_own() {
        // Keyed dials may share a flag, so uniqueness is over (flag, key).
        for vendor in known() {
            let mut flags: Vec<(&str, Option<&str>)> = vendor
                .dials()
                .into_iter()
                .filter_map(|(_, dial)| dial.filter(|spec| spec.env.is_none()))
                .map(|spec| (spec.flag, spec.key))
                .collect();
            let declared = flags.len();
            flags.sort_unstable();
            flags.dedup();
            assert_eq!(flags.len(), declared, "{} names a flag twice", vendor.name);
            assert!(
                flags.iter().all(|(flag, _)| flag.starts_with('-')),
                "{} declares a dial whose flag is not one",
                vendor.name
            );
            for (flag, key) in flags {
                assert!(
                    key.is_none_or(|key| !key.is_empty() && !key.contains('=')),
                    "{}'s {flag} dial names a key that is not one",
                    vendor.name
                );
            }
        }
    }

    #[test]
    fn every_word_a_vendor_is_launched_with_is_a_flag_that_names_no_session() {
        // resume and fork keep launch flags only because they name no
        // session; one that did would be dropped or doubled.
        for vendor in known() {
            for word in vendor.launch {
                assert!(
                    word.starts_with('-'),
                    "{} is launched with {word}, which is not a flag",
                    vendor.name
                );
                let Some(session) = vendor.session else {
                    continue;
                };
                for first in [true, false] {
                    assert_eq!(
                        session.names_a_session(word, first),
                        None,
                        "{} is launched with {word}, which names a session",
                        vendor.name
                    );
                }
                assert_ne!(session.start, Some(*word), "{}", vendor.name);
            }
        }
        assert_eq!(
            BRANCHING.launch,
            ["--alone"],
            "the law is not held vacuously"
        );
    }

    /// Whether a model listing's argv is usable. An empty argv would start an
    /// interactive agent, and a printed listing that does not open with a flag
    /// would be read as a task. A JSON listing is a subcommand.
    fn says_what_to_run(models: Models) -> bool {
        match models {
            Models::Cycle => true,
            Models::Printed(argv) => argv.first().is_some_and(|word| word.starts_with('-')),
            Models::Json(argv) => !argv.is_empty(),
        }
    }

    #[test]
    fn a_vendor_that_prints_its_models_says_what_to_run_for_the_listing() {
        for vendor in known() {
            assert!(
                says_what_to_run(vendor.models),
                "{} lists its models by {:?}",
                vendor.name,
                vendor.models
            );
        }
    }

    #[test]
    fn the_law_takes_a_json_listing_by_subcommand_and_refuses_one_asking_nothing() {
        assert!(says_what_to_run(Models::Json(&["debug", "models"])));
        assert!(!says_what_to_run(Models::Json(&[])));
        assert!(!says_what_to_run(Models::Printed(&[])));
        assert!(
            !says_what_to_run(Models::Printed(&["list"])),
            "a printed listing still opens with a flag"
        );
    }

    #[test]
    fn a_vendor_does_only_what_it_says_it_can_do() {
        let claude = find("claude").unwrap();
        assert!(claude.can(Capability::Fork));
        assert!(claude.can(Capability::Trust));

        assert!(SECOND.can(Capability::Resume), "it can carry a session on");
        assert!(
            !SECOND.can(Capability::Fork),
            "but it cannot branch one, and a verb that tried would be asking \
             for a flag this vendor does not have"
        );
        for cannot in [Capability::Hooks, Capability::Transcript, Capability::Trust] {
            assert!(!SECOND.can(cannot), "{cannot:?}");
        }
    }

    #[test]
    fn a_vendor_reports_through_hooks_or_amx_has_none_to_wire() {
        for vendor in known() {
            assert_eq!(
                vendor.can(Capability::Hooks),
                vendor.hooks.is_some(),
                "{}",
                vendor.name
            );
        }
        assert!(
            SECOND.hooks.is_none(),
            "the vendor amx cannot be told anything by is the shape install \
             has to leave alone"
        );
    }

    #[test]
    fn a_vendor_that_reports_names_a_moment_once_and_the_three_a_turn_stands_on() {
        // Started, Prompted and Ended are required: without them `send`
        // cannot confirm delivery and `result` cannot wait for a turn.
        for hooks in every_hooks() {
            for moment in Moment::ALL {
                let named = hooks
                    .events
                    .iter()
                    .filter(|wiring| wiring.moment == moment)
                    .count();
                assert!(named <= 1, "{hooks:?} names {moment:?} {named} times");
            }
            for needed in [Moment::Started, Moment::Prompted, Moment::Ended] {
                assert!(
                    hooks.events.iter().any(|wiring| wiring.moment == needed),
                    "{hooks:?} has no event for {needed:?}"
                );
            }

            let mut events: Vec<&str> = hooks.events.iter().map(|w| w.event).collect();
            let wired = events.len();
            events.sort_unstable();
            events.dedup();
            assert_eq!(events.len(), wired, "{hooks:?} wires one event twice");
        }
    }

    /// The hooks of every known vendor, plus [`second::HOOKS`].
    fn every_hooks() -> Vec<Hooks> {
        let mut all: Vec<Hooks> = known().iter().filter_map(|vendor| vendor.hooks).collect();
        all.push(second::HOOKS);
        all
    }

    #[test]
    fn the_name_a_vendor_gives_a_moment_is_what_finds_it_again() {
        for vendor in known() {
            let Some(hooks) = vendor.hooks else { continue };
            for wiring in hooks.events {
                assert_eq!(
                    hooks.moment(wiring.event),
                    Some(wiring.moment),
                    "{}'s {}",
                    vendor.name,
                    wiring.event
                );
            }
            assert_eq!(hooks.moment("nothing wired this"), None, "{}", vendor.name);
            assert_eq!(hooks.moment(""), None, "{}", vendor.name);
        }
    }

    #[test]
    fn a_record_is_read_in_its_own_vendors_words() {
        for vendor in table() {
            assert_eq!(hooks_for(vendor.name), vendor.hooks, "{}", vendor.name);
        }
        assert_eq!(hooks_for("pi --approve"), pi::VENDOR.hooks);
        assert_eq!(hooks_for("my-wrapper"), claude::VENDOR.hooks);
        assert_eq!(hooks_for(""), claude::VENDOR.hooks);
        let pi = hooks_for("pi").unwrap();
        for wiring in claude::HOOKS.events {
            assert_eq!(pi.moment(wiring.event), None, "{}", wiring.event);
        }
    }

    #[test]
    fn a_vendor_raises_a_tools_name_at_every_word_boundary() {
        // A word starts after any non-alphanumeric character, not just `_`,
        // as the pane shows 'Resolve-Library-Id'.
        assert_eq!(
            rendered("mcp__context7__resolve-library-id"),
            "Resolve-Library-Id"
        );
        assert_eq!(rendered("mcp__playwright__browser_click"), "Browser Click");
        assert_eq!(rendered("mcp__acme__fs.read_file"), "Fs.Read File");
        // A digit does not start a new word.
        assert_eq!(rendered("mcp__totp__get2fa-codes"), "Get2fa-Codes");
        assert_eq!(rendered("Bash"), "Bash");
    }

    #[test]
    fn a_vendor_that_draws_no_box_writes_no_sentence() {
        assert_eq!(pi::HOOKS.permission_sentence("Bash"), None);
        assert_eq!(
            second::HOOKS.permission_sentence("Bash").as_deref(),
            Some("second may not run Bash yet")
        );
    }

    #[test]
    fn a_vendor_is_wired_somewhere_under_the_persons_home() {
        // An absolute path would discard the home directory it is joined to.
        for hooks in every_hooks() {
            let path = hooks.wire.path();
            assert!(!path.is_empty(), "{hooks:?}");
            assert!(
                !std::path::Path::new(path).is_absolute(),
                "{hooks:?} is wired outside anybody's home"
            );
            // Opt-in wires are held to the same path rules, but need not
            // report.
            for wire in hooks.opt_in {
                let path = wire.path();
                assert!(!path.is_empty(), "{hooks:?} carries an empty opt-in path");
                assert!(
                    !std::path::Path::new(path).is_absolute(),
                    "{path} is written outside anybody's home"
                );
            }
            match hooks.wire {
                Wire::File { body, .. } => {
                    assert!(body.contains("_hook"), "{path} reports through nothing");
                }
                Wire::Placed {
                    dir_env,
                    path: file,
                    body,
                    ..
                } => {
                    assert!(!dir_env.is_empty(), "{path} names no variable");
                    assert!(!file.is_empty(), "{path} places no file");
                    assert!(
                        !std::path::Path::new(file).is_absolute(),
                        "{path}/{file} is written outside the directory"
                    );
                    assert!(body.contains("_hook"), "{path} reports through nothing");
                }
                Wire::Hooks { dir_env, body, .. } => {
                    assert!(!dir_env.is_empty(), "{path} names no variable");
                    assert!(body.contains("_hook"), "{path} reports through nothing");
                }
                Wire::Plugin { files, .. } => {
                    assert!(
                        files.iter().any(|(_, body)| body.contains("_hook")),
                        "{path} reports through nothing"
                    );
                    for (name, _) in files {
                        assert!(!name.is_empty(), "{path} ships a file with no name");
                        assert!(
                            !std::path::Path::new(name).is_absolute(),
                            "{path}/{name} is written outside the plugin"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_vendor_sentence_about_a_tool_says_where_the_tool_goes() {
        // Empty means the vendor draws no permission box.
        for hooks in every_hooks() {
            assert!(
                hooks.permission_sentence.is_empty() || hooks.permission_sentence.contains(TOOL),
                "{hooks:?} writes a sentence with no room for the tool"
            );
        }
    }

    /// Whether a vendor claiming a transcript can say where it is: the path
    /// only ever arrives on a hook payload.
    fn finds_its_transcript(vendor: &Vendor) -> bool {
        !vendor.can(Capability::Transcript) || vendor.can(Capability::Hooks)
    }

    #[test]
    fn a_vendor_with_a_transcript_reports_through_hooks() {
        for vendor in known() {
            assert!(
                finds_its_transcript(vendor),
                "{} claims a transcript and has no hooks to name it",
                vendor.name
            );
        }
    }

    #[test]
    fn the_law_refuses_a_transcript_with_no_hooks() {
        let vendor = Vendor {
            capabilities: &[Capability::Transcript],
            ..SECOND
        };
        assert!(!finds_its_transcript(&vendor));
    }

    #[test]
    fn a_vendor_that_can_be_adopted_names_the_session_that_makes_it_possible() {
        // Adopting reads the session id from the environment of a process
        // the agent started.
        for vendor in known() {
            if vendor.can(Capability::Adopt) {
                assert!(
                    vendor.session_env.is_some(),
                    "{} claims it can be adopted and names no session",
                    vendor.name
                );
            }
        }
    }

    #[test]
    fn every_flag_a_vendor_declares_for_its_session_is_a_flag() {
        // A word without a leading `-` would never be read back as a flag.
        for vendor in known() {
            let Some(session) = vendor.session else {
                continue;
            };
            if let Resume::Flag { flag, .. } = session.resume {
                assert!(
                    flag.starts_with('-'),
                    "{}'s resume flag is not one",
                    vendor.name
                );
            }
            if let Some(start) = session.start {
                assert!(
                    start.starts_with('-'),
                    "{}'s start flag is not one",
                    vendor.name
                );
            }
            for conflict in session.conflicts {
                assert!(
                    conflict.starts_with('-'),
                    "{}'s {conflict} is not a flag",
                    vendor.name
                );
            }
            if let Some(fork) = session.fork {
                assert!(
                    spelled_as_its_shape(fork),
                    "{}'s {fork:?} is not spelled the way its shape is read",
                    vendor.name
                );
            }
        }
    }

    /// Whether a fork's word matches its shape: markers and origins are
    /// flags, and a subcommand is a non-empty word that is not a flag.
    fn spelled_as_its_shape(fork: ForkSpec) -> bool {
        match fork {
            ForkSpec::Marker(flag) | ForkSpec::Origin(flag) => flag.starts_with('-'),
            ForkSpec::Subcommand(word) => !word.is_empty() && !word.starts_with('-'),
        }
    }

    #[test]
    fn the_law_takes_a_fork_subcommand_and_refuses_one_spelled_as_a_flag() {
        assert!(spelled_as_its_shape(ForkSpec::Subcommand("fork")));
        assert!(!spelled_as_its_shape(ForkSpec::Subcommand("--fork")));
        assert!(!spelled_as_its_shape(ForkSpec::Subcommand("")));
        assert!(!spelled_as_its_shape(ForkSpec::Marker("fork")));
    }

    #[test]
    fn resume_args_spells_a_joined_flag_a_split_flag_and_a_subcommand() {
        let spelled = |resume| SessionSpec {
            start: None,
            resume,
            conflicts: &[],
            fork: None,
        };
        let joined = spelled(Resume::Flag {
            flag: "--resume",
            joined: true,
        });
        assert_eq!(joined.resume_args("abc-123"), ["--resume=abc-123"]);
        let split = spelled(Resume::Flag {
            flag: "--session-id",
            joined: false,
        });
        assert_eq!(split.resume_args("abc-123"), ["--session-id", "abc-123"]);
        let subcommand = spelled(Resume::Subcommand("resume"));
        assert_eq!(subcommand.resume_args("abc-123"), ["resume", "abc-123"]);
    }

    /// Whether [`SessionSpec::names_a_session`] recognises every word
    /// [`SessionSpec::resume_args`] writes, so a second resume replaces the
    /// first one's session instead of keeping both.
    fn round_trips(spec: &SessionSpec) -> bool {
        let mut words = spec.resume_args("abc-123").into_iter().peekable();
        let mut kept = Vec::new();
        let mut first = true;
        while let Some(word) = words.next() {
            match spec.names_a_session(&word, first) {
                Some(true) => {
                    words.next();
                }
                Some(false) => {}
                None => kept.push(word),
            }
            first = false;
        }
        kept.is_empty()
    }

    #[test]
    fn every_vendor_that_resumes_reads_its_own_spelling_back() {
        for vendor in known() {
            if !vendor.can(Capability::Resume) {
                continue;
            }
            let session = vendor.session.unwrap_or_else(|| {
                panic!("{} claims it can resume and names no session", vendor.name)
            });
            assert!(
                round_trips(&session),
                "{} writes a resume it cannot read back",
                vendor.name
            );
        }
    }

    #[test]
    fn the_law_refuses_a_subcommand_spelled_as_a_flag() {
        let spec = SessionSpec {
            resume: Resume::Subcommand("--again"),
            ..SECOND.session.unwrap()
        };
        assert!(!round_trips(&spec));
    }

    #[test]
    fn a_subcommand_names_a_session_only_right_after_the_program() {
        let spec = SECOND.session.unwrap();
        assert_eq!(spec.names_a_session("again", true), Some(true));
        assert_eq!(
            spec.names_a_session("again", false),
            None,
            "anywhere else it is a word like any other"
        );
    }

    #[test]
    fn a_fork_subcommand_names_a_session_only_right_after_the_program() {
        // A copy is recorded as `<prog> fork <id> ..`, and forking it again
        // must replace that id.
        let spec = BRANCHING.session.unwrap();
        assert_eq!(spec.names_a_session("fork", true), Some(true));
        assert_eq!(spec.names_a_session("fork", false), None);
        assert_eq!(spec.names_a_session("again", true), Some(true));
        assert_eq!(SECOND.session.unwrap().names_a_session("fork", true), None);
    }

    #[test]
    fn fork_spec_expresses_a_marker_beside_resume_and_a_flag_naming_the_origin() {
        assert_eq!(
            ForkSpec::Marker("--fork-session"),
            ForkSpec::Marker("--fork-session")
        );
        assert_ne!(
            ForkSpec::Marker("--fork-session"),
            ForkSpec::Origin("--fork")
        );
    }

    #[test]
    fn a_vendor_never_lists_its_own_resume_flag_among_what_conflicts_with_it() {
        for vendor in known() {
            let Some(session) = vendor.session else {
                continue;
            };
            let Resume::Flag { flag, .. } = session.resume else {
                continue;
            };
            assert!(
                !session.conflicts.contains(&flag),
                "{} names its own resume flag as one that conflicts with it",
                vendor.name
            );
        }
    }

    #[test]
    fn a_vendor_that_can_fork_declares_how_it_branches() {
        for vendor in known() {
            if vendor.can(Capability::Fork) {
                let session = vendor.session.unwrap_or_else(|| {
                    panic!("{} claims it can fork and names no session", vendor.name)
                });
                assert!(
                    session.fork.is_some(),
                    "{} claims it can fork and names no shape for it",
                    vendor.name
                );
            }
        }
    }

    #[test]
    fn a_vendor_that_forks_through_no_hooks_declares_a_start_flag() {
        // Without a Started hook to name the copy's session, amx has to mint
        // the id itself through the start flag.
        for vendor in known() {
            if vendor.can(Capability::Fork) && vendor.hooks.is_none() {
                let session = vendor.session.unwrap_or_else(|| {
                    panic!("{} claims it can fork and names no session", vendor.name)
                });
                assert!(
                    session.start.is_some(),
                    "{} forks through no hooks and declares no start flag, \
                     so a copy it makes is one it can never name again",
                    vendor.name
                );
            }
        }
    }

    #[test]
    fn a_vendor_keeps_the_variables_that_name_its_own_session_to_itself() {
        // Pane-wide variables such as TMUX are not a vendor's to list.
        for vendor in known() {
            for name in vendor.not_inherited {
                assert_eq!(
                    *name,
                    name.to_uppercase(),
                    "{} names a variable that is not one",
                    vendor.name
                );
            }
            assert!(
                !vendor.not_inherited.contains(&"TMUX"),
                "{} claims a variable that belongs to the pane, not to it",
                vendor.name
            );
        }
    }

    #[test]
    fn a_vendor_that_names_a_session_never_lets_that_name_travel() {
        // An agent that inherited its spawner's session id would file its
        // events under the spawner.
        for vendor in known() {
            let Some(session) = vendor.session_env else {
                continue;
            };
            assert!(
                vendor.not_inherited.contains(&session),
                "{} hands {session} to the agents it spawns",
                vendor.name
            );
        }
    }

    #[test]
    fn every_vendor_in_the_table_says_what_it_can_be_asked_for_by_name() {
        for vendor in table() {
            assert!(
                vendor.catalog.is_some(),
                "{} says nothing it can be asked for by name",
                vendor.name
            );
        }
        assert!(
            SECOND.catalog.is_none(),
            "the vendor nobody has measured a layout for is the shape a \
             reader of those places has to leave alone"
        );
    }

    #[test]
    fn a_vendor_loads_what_it_offers_from_under_a_root_rather_than_a_path_of_its_own() {
        // Places are joined onto a root, so none may be absolute, and
        // built-ins are listed without their sigil.
        for vendor in known() {
            let Some(catalog) = vendor.catalog else {
                continue;
            };
            for place in catalog
                .skills
                .iter()
                .chain(catalog.commands)
                .chain(catalog.agents)
            {
                let path = place.path();
                assert!(!path.is_empty(), "{} looks in nowhere", vendor.name);
                assert!(
                    !std::path::Path::new(path).is_absolute(),
                    "{}'s {path} hangs off no root",
                    vendor.name
                );
            }
            for builtin in catalog.builtins {
                assert!(!builtin.is_empty(), "{} names nothing", vendor.name);
                assert!(
                    !builtin.starts_with(catalog.sigil),
                    "{} spells {builtin} with the mark that runs it",
                    vendor.name
                );
            }
        }
    }

    #[test]
    fn a_vendor_that_can_be_told_to_be_one_of_its_agents_names_the_flag_for_it() {
        for vendor in known() {
            let Some(catalog) = vendor.catalog else {
                continue;
            };
            let Some(flag) = catalog.agent_flag else {
                continue;
            };
            assert!(
                flag.starts_with('-'),
                "{}'s agent flag is not one",
                vendor.name
            );
            assert!(
                !catalog.agents.is_empty(),
                "{} names a flag for agents it loads none of",
                vendor.name
            );
        }
    }

    #[test]
    fn a_vendor_that_declares_screens_declares_ones_that_parse() {
        // A document that fails to parse would panic at the first pane read.
        for vendor in known() {
            let Some(screens) = vendor.screens else {
                continue;
            };
            let screens = crate::rules::Ruleset::parse(screens)
                .unwrap_or_else(|e| panic!("{}'s screens: {e:#}", vendor.name));
            assert!(
                !screens.rules().is_empty(),
                "{} declares a document with no screen in it, which is the \
                 same as declaring none",
                vendor.name
            );
        }
    }

    #[test]
    fn the_sentinel_is_acceptable_on_an_open_dial_and_a_closed_one() {
        for vendor in known() {
            for (which, dial) in vendor.dials() {
                if let Some(spec) = dial {
                    assert!(accepts(&spec, DEFAULT), "{}'s {which} dial", vendor.name);
                }
            }
        }
    }

    #[test]
    fn a_closed_dial_takes_its_cycle_and_nothing_else() {
        let permission = find("claude").unwrap().permission.unwrap();
        for mode in ["acceptEdits", "auto", "bypassPermissions", "manual", "plan"] {
            assert!(accepts(&permission, mode), "{mode}");
        }
        for refused in ["nonsense", "acceptedits", "Plan", "ask", ""] {
            assert!(!accepts(&permission, refused), "{refused:?}");
        }

        let model = SECOND.model.unwrap();
        assert!(accepts(&model, "small"));
        assert!(!accepts(&model, "opus"), "that is the other vendor's word");
    }

    #[test]
    fn an_open_dial_takes_a_value_its_cycle_never_names() {
        let model = find("claude").unwrap().model.unwrap();
        assert!(accepts(&model, "opus"));
        assert!(accepts(&model, "claude-fable-5"));
        assert!(accepts(&model, "anything-at-all"));

        let effort = SECOND.effort.unwrap();
        assert!(accepts(&effort, "whatever-it-likes"));
    }

    #[test]
    fn a_resolved_dial_puts_its_flag_in_front_of_the_callers_args() {
        assert_eq!(
            inject(
                find("claude").unwrap(),
                "fable",
                "plan",
                "high",
                &[],
                &v(&["--verbose"])
            ),
            v(&[
                "--model",
                "fable",
                "--permission-mode",
                "plan",
                "--effort",
                "high",
                "--verbose"
            ])
        );
    }

    #[test]
    fn a_dial_left_at_the_sentinel_injects_nothing_at_all() {
        assert!(inject(find("claude").unwrap(), DEFAULT, DEFAULT, DEFAULT, &[], &[]).is_empty());
        assert_eq!(
            inject(
                find("claude").unwrap(),
                DEFAULT,
                "acceptEdits",
                DEFAULT,
                &[],
                &v(&["-x"])
            ),
            v(&["--permission-mode", "acceptEdits", "-x"])
        );
    }

    #[test]
    fn a_dial_the_vendor_does_not_declare_is_never_injected() {
        assert_eq!(
            inject(&SECOND, "large", "plan", "hard", &[], &[]),
            v(&["-m", "large", "--care", "hard"]),
        );
    }

    #[test]
    fn each_dial_emits_the_flag_its_own_vendor_declares() {
        assert_eq!(
            inject(&SECOND, "large", DEFAULT, DEFAULT, &[], &[]),
            v(&["-m", "large"])
        );
        assert_eq!(
            inject(find("claude").unwrap(), "opus", DEFAULT, DEFAULT, &[], &[]),
            v(&["--model", "opus"])
        );
    }

    #[test]
    fn a_dial_yields_to_the_same_flag_the_argv_already_carries() {
        // Each dial yields on its own; a carried --model leaves the others.
        let claude = find("claude").unwrap();
        assert_eq!(
            inject(
                claude,
                "fable",
                "plan",
                DEFAULT,
                &[],
                &v(&["--model", "opus"])
            ),
            v(&["--permission-mode", "plan", "--model", "opus"])
        );
        assert_eq!(
            inject(claude, "fable", DEFAULT, "max", &[], &v(&["--model=opus"])),
            v(&["--effort", "max", "--model=opus"])
        );
        // The same, with the flag on the agent command line.
        assert_eq!(
            inject(
                claude,
                "fable",
                DEFAULT,
                "high",
                &v(&["--model", "opus"]),
                &[]
            ),
            v(&["--effort", "high"])
        );
    }

    #[test]
    fn a_flag_that_merely_begins_the_same_is_not_that_dials_flag() {
        let claude = find("claude").unwrap();
        assert_eq!(
            inject(claude, "fable", DEFAULT, DEFAULT, &[], &v(&["--models"])),
            v(&["--model", "fable", "--models"])
        );
        assert_eq!(
            inject(
                claude,
                "fable",
                DEFAULT,
                DEFAULT,
                &[],
                &v(&["--model-name=opus"])
            ),
            v(&["--model", "fable", "--model-name=opus"])
        );
    }

    #[test]
    fn a_keyed_dial_writes_its_key_as_the_flags_value() {
        assert_eq!(
            inject(&BRANCHING, "large", DEFAULT, "thorough", &[], &v(&["-x"])),
            v(&["-m", "large", "-c", "care=thorough", "-x"])
        );
    }

    #[test]
    fn a_keyed_dial_stands_down_only_for_its_own_key() {
        for carried in [
            v(&["-c", "care=quick"]),
            v(&["-c=care=quick"]),
            v(&["-x", "-c", "care="]),
        ] {
            assert_eq!(
                inject(&BRANCHING, DEFAULT, DEFAULT, "thorough", &carried, &[]),
                v(&[]),
                "{carried:?}"
            );
            assert_eq!(
                inject(&BRANCHING, DEFAULT, DEFAULT, "thorough", &[], &carried),
                carried,
                "{carried:?}"
            );
        }
        // Another key under the same flag, or the key not right after the
        // flag, is not this dial.
        for other in [
            v(&["-c", "other=1"]),
            v(&["-c", "cared=1"]),
            v(&["-c=other=1"]),
            v(&["care=quick"]),
            v(&["-c"]),
        ] {
            let mut expected = v(&["-c", "care=thorough"]);
            expected.extend(other.clone());
            assert_eq!(
                inject(&BRANCHING, DEFAULT, DEFAULT, "thorough", &[], &other),
                expected,
                "{other:?}"
            );
        }
    }

    #[test]
    fn a_bare_dial_writes_its_flag_alone() {
        assert_eq!(
            inject(&ELSEWHERE, DEFAULT, "loose", DEFAULT, &[], &v(&["-x"])),
            v(&["--loose", "-x"])
        );
        assert_eq!(
            inject(&ELSEWHERE, DEFAULT, "loose", DEFAULT, &v(&["--loose"]), &[]),
            v(&[])
        );
    }

    #[test]
    fn a_dial_carried_in_the_env_writes_no_argv_and_a_variable_instead() {
        assert_eq!(
            inject(&ELSEWHERE, "large", DEFAULT, DEFAULT, &[], &v(&["-x"])),
            v(&["-x"])
        );
        let none = BTreeMap::new();
        assert_eq!(
            env_dials(&ELSEWHERE, "large", &none),
            [(
                "SECOND_CONFIG".to_string(),
                r#"{"size":"large"}"#.to_string()
            )]
        );
        // The value is JSON-escaped.
        assert_eq!(
            env_dials(&ELSEWHERE, r#"a"b"#, &none)[0].1,
            r#"{"size":"a\"b"}"#
        );
        assert!(env_dials(&ELSEWHERE, DEFAULT, &none).is_empty());
        // A variable already set wins.
        let set = BTreeMap::from([("SECOND_CONFIG".to_string(), "{}".to_string())]);
        assert!(env_dials(&ELSEWHERE, "large", &set).is_empty());
        assert!(env_dials(find("claude").unwrap(), "opus", &none).is_empty());
        assert!(env_dials(&SECOND, "large", &none).is_empty());
    }

    #[test]
    fn a_model_carried_in_opencodes_config_variable_is_its_model_key() {
        // opencode loads OPENCODE_CONFIG_CONTENT last, over every config
        // file, so the model goes there as `{"model":"<value>"}`.
        let opencode = Vendor {
            model: Some(DialSpec {
                key: Some("model"),
                env: Some("OPENCODE_CONFIG_CONTENT"),
                ..DialSpec::open("", &[DEFAULT])
            }),
            ..ELSEWHERE
        };
        assert_eq!(
            env_dials(
                &opencode,
                "opencode/longcat-2.5-preview-free",
                &BTreeMap::new()
            ),
            [(
                "OPENCODE_CONFIG_CONTENT".to_string(),
                r#"{"model":"opencode/longcat-2.5-preview-free"}"#.to_string()
            )]
        );
    }

    #[test]
    fn a_bare_dial_is_closed_with_one_value_its_flag_says() {
        // A bare flag carries no value, so it can only express one.
        for vendor in known() {
            for (which, dial) in vendor.dials() {
                let Some(spec) = dial.filter(|spec| spec.bare) else {
                    continue;
                };
                assert!(!spec.open, "{}'s {which} dial", vendor.name);
                assert_eq!(spec.cycle.len(), 2, "{}'s {which} dial", vendor.name);
                assert_eq!(spec.key, None, "{}'s {which} dial", vendor.name);
                assert!(spec.env.is_none(), "{}'s {which} dial", vendor.name);
            }
        }
    }

    #[test]
    fn only_a_model_dial_is_carried_in_the_env_under_a_key_and_no_flag() {
        // env_dials only reads the model dial.
        for vendor in known() {
            for (which, dial) in vendor.dials() {
                let Some(spec) = dial.filter(|spec| spec.env.is_some()) else {
                    continue;
                };
                assert_eq!(which, "model", "{}", vendor.name);
                assert!(spec.key.is_some_and(|key| !key.is_empty()));
                assert!(spec.env.is_some_and(|env| !env.is_empty()));
                assert_eq!(spec.flag, "", "{} writes no argv for it", vendor.name);
            }
        }
    }

    #[test]
    fn a_vendor_cuts_a_turn_in_one_press_or_more_and_prompts_on_a_flag() {
        for vendor in known() {
            assert!(vendor.cancel_presses >= 1, "{}", vendor.name);
            if let Some(flag) = vendor.prompt_flag {
                assert!(flag.starts_with('-'), "{}", vendor.name);
                assert!(!flag.contains('='), "{}", vendor.name);
            }
            assert!(
                vendor.popups.iter().all(|c| !c.is_whitespace()),
                "{}",
                vendor.name
            );
        }
    }

    #[test]
    fn the_vendors_before_opencode_keep_the_argv_and_keys_they_had() {
        for vendor in table().iter().filter(|vendor| vendor.name != "opencode") {
            assert_eq!(vendor.prompt_flag, None, "{}", vendor.name);
            assert!(vendor.popups.is_empty(), "{}", vendor.name);
            assert_eq!(vendor.cancel_presses, 1, "{}", vendor.name);
            assert_eq!(vendor.interrupt_signal, None, "{}", vendor.name);
            for (which, dial) in vendor.dials() {
                if let Some(spec) = dial {
                    assert!(!spec.bare, "{}'s {which}", vendor.name);
                    assert!(spec.env.is_none(), "{}'s {which}", vendor.name);
                }
            }
        }
    }
}
