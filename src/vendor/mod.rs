//! What amx knows about a vendor: the dials it declares, and the one rule that
//! turns a resolved dial into vendor argv.
//!
//! A vendor is a descriptor in a static table, not a trait: no dynamic
//! dispatch, no second implementation of anything, and no place for a vendor
//! to hide behaviour. A second vendor is a second entry, and what it declares
//! is data a person can read in one sitting and diff against the vendor's own
//! `--help`. Where a field would want a function is where to think again, and
//! not before.
//!
//! Launch metadata only. What a vendor's screens mean, and how its states
//! read, is `rules` and `derive`. A vendor may say which of those are written
//! for it; what they say about a screen stays where screens are read.
//!
//! `registry` is the crate's door to this table, because the rest of amx asks
//! its questions in terms of the `agent` config key: a command line, arguments
//! and all.

pub mod claude;
pub mod pi;

/// A vendor that exists to keep this module honest. Test builds only: it is
/// nobody's agent, and it is not in the table.
#[cfg(test)]
pub mod second;

/// One dial a vendor declares. `cycle` is what a cycle key offers and always
/// starts at [`DEFAULT`]; `open` says whether a value the cycle never names
/// is still worth passing on; `flag` is the vendor argv flag [`inject`]
/// emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialSpec {
    pub cycle: &'static [&'static str],
    pub open: bool,
    pub flag: &'static str,
}

/// One moment in a turn that amx listens for.
///
/// What a vendor calls each of these is the vendor's own word and lives in the
/// table beside it. What one means for an agent's record is `hook`'s business
/// and lives there, which is why nothing here says what any of them does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    /// A session begins, and says which session it is.
    Started,
    /// A prompt has been sent, and a turn is under way.
    Prompted,
    /// A tool is about to run.
    Calling,
    /// Leave to run one is being asked for.
    Asked,
    /// Leave was refused, and the tool never ran.
    Refused,
    /// The vendor is telling somebody something about this session.
    Notified,
    /// The turn is over.
    Ended,
}

impl Moment {
    /// Every moment amx listens for. A vendor that reports at all names all of
    /// them: what amx does with an event it was never told about is nothing.
    pub const ALL: [Moment; 7] = [
        Moment::Started,
        Moment::Prompted,
        Moment::Calling,
        Moment::Asked,
        Moment::Refused,
        Moment::Notified,
        Moment::Ended,
    ];
}

/// One moment, under the name its vendor gives it, and how the entry that
/// listens for it is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wiring {
    pub moment: Moment,
    /// The vendor's own name for it, which is what amx writes into the
    /// settings and what arrives in a payload.
    pub event: &'static str,
    /// Whether this vendor's entry for it takes a tool matcher.
    pub matched: bool,
}

/// What amx needs in order to wire itself into a vendor's hooks and read back
/// what they say: where the wiring goes, what the vendor calls each moment,
/// and the words the vendor's own screens are worded with.
///
/// A vendor that reports nothing has none of this, and that is what
/// [`Capability::Hooks`] is the question about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hooks {
    /// How amx's hook command reaches this vendor: what `install` writes,
    /// where, and what `uninstall` takes back out.
    pub wire: Wire,
    /// The moments this vendor reports, in the order the entries are wired.
    /// Every one of them, and never a moment twice: a vendor with no way to
    /// say a thing leaves the moment out, and amx hears nothing of it.
    pub events: &'static [Wiring],
    /// What this vendor's matcher for every tool is spelled.
    pub matcher: &'static str,
    /// The one tool call that is not work: it draws a menu and waits on it.
    pub question_tool: &'static str,
    /// The vendor's word for a notice about a session nobody is using.
    pub idle_notice: &'static str,
    /// And for the one that repeats a permission box.
    pub permission_notice: &'static str,
    /// The sentence this vendor writes on a permission box, with [`TOOL`]
    /// where the tool it is about goes.
    pub permission_sentence: &'static str,
}

/// How a vendor is wired to amx's hook command, under the person's home
/// directory.
///
/// Two shapes, both measured off the vendor that takes them. Which a vendor
/// takes is its own business and `install`'s to honour; nothing that reads a
/// payload afterwards can tell the two apart. The one thing the wire decides
/// past that is whether the hook may answer — see [`Wire::listens`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    /// One entry per event, merged into the vendor's own JSON settings file
    /// beside whatever is already there, each running the hook command. The
    /// path is relative to the home directory.
    Settings(&'static str),
    /// A file of amx's own, written whole where the vendor loads extensions
    /// from, which reports through the hook command itself. The path is
    /// relative to the home directory, and the body is the file as it ships.
    File {
        path: &'static str,
        body: &'static str,
    },
}

impl Wire {
    /// Whether the other end of this wire hears what the hook command prints.
    ///
    /// A file wire is amx's own code running inside the vendor, and it reads
    /// the answer: the record's directory, which a pane amx did not start has
    /// no other way of learning. A settings wire is the vendor's own hook
    /// runner, and what a hook prints is the vendor's to show — claude puts a
    /// `UserPromptSubmit` hook's stdout into the conversation — so a hook run
    /// over one says nothing.
    pub fn listens(self) -> bool {
        matches!(self, Wire::File { .. })
    }

    /// Where the wiring goes, under the home directory.
    pub fn path(&self) -> &'static str {
        match self {
            Wire::Settings(path) => path,
            Wire::File { path, .. } => path,
        }
    }
}

impl Hooks {
    /// The moment `event` is, when it is one amx listens for.
    pub fn moment(&self, event: &str) -> Option<Moment> {
        self.events
            .iter()
            .find(|wiring| wiring.event == event)
            .map(|wiring| wiring.moment)
    }
}

/// Where the tool a vendor sentence is about goes, in the sentence.
pub const TOOL: &str = "{tool}";

/// How a vendor branches a session it opened into a copy, for a vendor that
/// claims [`Capability::Fork`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkSpec {
    /// A bare flag with no value of its own, written beside the resume flag:
    /// the copy still opens through [`SessionSpec::resume`], and this is what
    /// tells the vendor to branch rather than carry on. claude's
    /// `--fork-session`.
    Marker(&'static str),
    /// A flag naming the session to branch from, carrying the origin's id
    /// rather than riding beside the resume flag.
    Origin(&'static str),
}

/// How a vendor spells the flags that decide which session a process opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionSpec {
    /// The flag that starts a session under an id amx chose, or `None` from a
    /// vendor amx never asks to do that. claude declares none: its own
    /// Started hook already names the session it opened, and the id it wants
    /// there is a UUID, not the id amx mints for a session it starts.
    pub start: Option<&'static str>,
    /// The flag that carries an agent on into a session it already has.
    pub resume: &'static str,
    /// Whether the value rides joined onto [`resume`](Self::resume) with `=`,
    /// the way claude spells `--resume=<id>`, rather than as a word of its
    /// own.
    pub joined: bool,
    /// Every other flag that also names a session, so a resume or a fork
    /// replaces it rather than leaving two words that both claim to say
    /// which session the vendor opens. Never
    /// [`resume`](Self::resume) itself: that flag is always replaced with the
    /// session being carried on to, which needs no listing here.
    pub conflicts: &'static [&'static str],
    /// How this vendor branches a session into a copy, for a vendor that
    /// claims [`Capability::Fork`]. `None` from a vendor that cannot.
    pub fork: Option<ForkSpec>,
}

/// One vendor, and everything amx has been taught about it.
///
/// A `None` dial means this vendor has no such dial at all, which is a
/// different thing from having one nobody has turned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vendor {
    /// The program this vendor is, which is what the table is keyed by and
    /// what a warning about it should name.
    pub name: &'static str,
    pub model: Option<DialSpec>,
    pub permission: Option<DialSpec>,
    pub effort: Option<DialSpec>,
    /// The flags that decide which session a process this vendor starts
    /// opens. `None` from a vendor amx has measured no session vocabulary
    /// for, and then a verb that would carry a session on or branch one has
    /// nothing to spell either with.
    pub session: Option<SessionSpec>,
    /// Where this vendor tells a process it starts which conversation that
    /// process belongs to. `None` from a vendor that says nothing, and then
    /// there is no way for the events of an agent amx did not start to find
    /// their way home.
    pub session_env: Option<&'static str>,
    /// The vendor's own variables that name the session a command was typed
    /// inside, and so the ones a pane amx starts must not inherit. A vendor
    /// handed these believes it is a child of the session that spawned it,
    /// and it is not.
    ///
    /// The vendor's alone: the variables that belong to any pane, whoever is
    /// running in it, are the caller's business and not listed here.
    pub not_inherited: &'static [&'static str],
    /// What amx may ask this vendor to do. Anything left out is a verb that
    /// has to say so and stop, rather than spawn a command the vendor will
    /// refuse and leave the person reading the pane to work out why.
    pub capabilities: &'static [Capability],
    /// How this vendor reports what it is doing, and where amx asks it to.
    /// `None` from a vendor that reports nothing, which is the same thing
    /// [`Capability::Hooks`] says and the reason both are here: the capability
    /// is what a verb asks, and this is what `install` and `hook` read.
    pub hooks: Option<Hooks>,
    /// What this vendor's screens look like, as the document that says what
    /// each of them means — its rules, the chrome it draws under them, and the
    /// sentences it sends about a dialog it will not describe. `crate::rules`
    /// reads it; the whole of what belongs here is which document is this
    /// vendor's.
    ///
    /// `None` from a vendor nobody has sat in front of yet, and then its pane
    /// is watched and never named. Screens are measured against a running
    /// program, and a document written from anywhere else is a transcription.
    pub screens: Option<&'static str>,
    /// The shape of the conversation this vendor writes to disk, for
    /// `crate::conversation` to read it by. Which file that is arrives on a
    /// hook payload, so a vendor that reports nothing never puts one on a
    /// record — [`Capability::Transcript`] is what a verb asks before it opens
    /// one, and this is how the reader that opens it reads what it finds.
    ///
    /// `None` from a vendor whose file amx has never sat down with. A shape
    /// is measured off a real session, the way a screen is off a real pane.
    pub transcript: Option<Transcript>,
    /// What this vendor can be asked for by name: where it loads skills,
    /// commands and agents from, and what it answers out of itself. A word
    /// typed on a task line is completed against it.
    ///
    /// `None` from a vendor whose layout amx has not measured, and then a
    /// word on a line for it is only ever the word somebody typed.
    pub catalog: Option<Catalog>,
}

/// A directory a vendor loads something from, under the root it hangs off.
///
/// Two roots, because they are what every vendor measured so far reads: the
/// person's own files, and the project the agent is running in. The path is
/// relative to one of them, and a `*` segment stands for every directory at
/// that level — claude keeps its plugins under a market, a plugin and a
/// version, none of which is amx's to name in advance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// Under the person's home directory.
    Person(&'static str),
    /// Under the project the agent is running in.
    Project(&'static str),
}

impl Place {
    /// The directory itself, relative to the root it hangs off.
    pub fn path(&self) -> &'static str {
        match self {
            Place::Person(path) | Place::Project(path) => path,
        }
    }
}

/// What a vendor can be asked for by name, as places to look and words it
/// answers to.
///
/// Where to look and what to call what is found, and nothing past that: what a
/// file in one of these says about itself is read out of the file, and where
/// the word goes on a task line is the line's business.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Catalog {
    /// Where this vendor loads skills from, in the order it reads them.
    pub skills: &'static [Place],
    /// Where its commands come from, whatever it calls them: pi's own word
    /// for the same thing is prompts.
    pub commands: &'static [Place],
    /// Where its agents come from. Empty from a vendor that has no such
    /// thing, and then nothing on a line for it names one.
    pub agents: &'static [Place],
    /// The commands the vendor answers out of itself, which are in no
    /// directory and are named here or nowhere. Each without the mark that
    /// runs it: writing that is the line's business. Empty from a vendor
    /// whose own list amx has not measured.
    pub builtins: &'static [&'static str],
    /// What stands in front of a skill's name in the word that runs one: pi
    /// spells a skill `/skill:name`, and claude spells it `/name`.
    pub skill_prefix: &'static str,
}

/// The shape of a conversation on disk: one JSON document a line, and where
/// in each the words are.
///
/// Two vendors, two shapes, both measured. What each entry means is
/// `crate::conversation`'s business; this is only which of them a file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transcript {
    /// Typed at the top — `user`, `assistant`, and bookkeeping under other
    /// names — with the message's blocks under `message.content`. A prompt
    /// is a `user` entry whose content is a string; a tool's result is a
    /// `user` entry whose content is blocks. Measured off claude 2.1.240 on
    /// 2026-08-25.
    Claude,
    /// Typed `message` with the voice under `message.role` — `user`,
    /// `assistant`, `toolResult` — and the blocks under `message.content`,
    /// where a tool call is a `toolCall` block. The rest of the file is the
    /// session's own bookkeeping under other types. Measured off pi 0.84.4's
    /// `~/.pi/agent/sessions/` on 2026-09-05.
    Pi,
}

/// Something amx can do only where the vendor takes part.
///
/// A vendor amx has no entry for can do none of these, which is the floor
/// every unregistered command stands on: a pane to watch, and nothing amx
/// pretends to know about what is in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Reports what it is doing to a command amx installs, so an agent's
    /// record is kept from what it said rather than guessed from its screen.
    Hooks,
    /// Keeps the conversation in a file amx can read back afterwards.
    Transcript,
    /// Can be told to carry on the session it opened, rather than start a
    /// second one that knows nothing.
    Resume,
    /// Can branch a session it opened, leaving the original where it was.
    Fork,
    /// Can be taken over after somebody else started it, which needs it to
    /// name the session in the environment of what it starts.
    Adopt,
    /// Asks whether a folder is trusted, in a screen amx knows how to answer
    /// for a tree it cut itself.
    Trust,
}

/// The value every dial rests at: the vendor's own configured behaviour,
/// which amx expresses by passing no flag whatsoever. There is no flag that
/// means "whatever you were going to do", so the only way to say it is
/// silence.
pub const DEFAULT: &str = "default";

/// Every vendor amx has an entry for, in the order a cycle key offers them.
///
/// claude first, because `rules::of` reads `entries().first()` as the screens
/// an unregistered agent is watched by, and reordering this would silently
/// change what it answers. The table is the whole of what makes a second
/// vendor possible, and a test-only [`second`] proves that nothing in here is
/// shaped around the first.
static TABLE: [Vendor; 2] = [claude::VENDOR, pi::VENDOR];

/// The moment a vendor's event is, whichever vendor in the table names it.
///
/// A payload arrives under the vendor's own word for the moment and nothing
/// else says whose word it is, so every entry that reports is asked. The
/// names do not collide — each vendor spells its events its own way — and an
/// event no entry names is one amx has no business acting on.
pub fn moment_of(event: &str) -> Option<Moment> {
    table()
        .iter()
        .filter_map(|vendor| vendor.hooks)
        .find_map(|hooks| hooks.moment(event))
}

impl Vendor {
    /// Whether amx may ask this vendor for `what`.
    pub fn can(&self, what: Capability) -> bool {
        self.capabilities.contains(&what)
    }

    /// This vendor's three dials in declaration order, each with the word the
    /// rest of amx calls it by.
    ///
    /// Declaration order, so an argv is diffable against the table it came
    /// from. Nothing downstream reads the order; a person comparing two spawns
    /// does.
    pub fn dials(&self) -> [(&'static str, Option<DialSpec>); 3] {
        [
            ("model", self.model),
            ("permission", self.permission),
            ("effort", self.effort),
        ]
    }
}

/// The vendor `agent` runs, or `None` when amx has registered none for it.
///
/// `agent` is a command line rather than a program name, because that is what
/// the config key holds: the entry is found by the program the command runs.
pub fn find(agent: &str) -> Option<&'static Vendor> {
    let program = program(agent);
    TABLE.iter().find(|vendor| vendor.name == program)
}

/// Every registered vendor, in the order a cycle key offers them.
pub fn table() -> &'static [Vendor] {
    &TABLE
}

/// The program an agent command runs, without its arguments. What a warning
/// about a vendor should name, and what the table is keyed by.
pub fn program(agent: &str) -> &str {
    agent.split_whitespace().next().unwrap_or(agent)
}

/// Is `value` worth passing to this dial? [`DEFAULT`] always is, so is any
/// value the cycle names, and anything else only on an open dial.
pub fn accepts(dial: &DialSpec, value: &str) -> bool {
    value == DEFAULT || dial.cycle.contains(&value) || dial.open
}

/// The one place a dial becomes a vendor flag: for each dial resolved to
/// something other than [`DEFAULT`], put its flag and value in front of
/// `vendor_args`, unless that flag is already there.
///
/// Already there means a whole token equal to the flag, or the `flag=value`
/// spelling, in `vendor_args` or in `carried` — the arguments the agent
/// command line brought with it — because both end up in one argv. A flag the
/// caller wrote wins over a dial by the dial standing down, which is why
/// nothing here has to know about precedence.
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
        if value != DEFAULT && !already(carried, vendor_args, spec.flag) {
            injected.push(spec.flag.to_string());
            injected.push(value.to_string());
        }
    }

    injected.extend(vendor_args.iter().cloned());
    injected
}

/// Does the argv this spawn is heading for already carry `flag`, either as
/// its own token or as `flag=value`? A string prefix would not do: `--models`
/// is somebody else's flag, and reading it as this one would cancel the
/// injection without a word.
fn already(carried: &[String], vendor_args: &[String], flag: &str) -> bool {
    let equals = format!("{flag}=");
    carried
        .iter()
        .chain(vendor_args)
        .any(|arg| arg == flag || arg.starts_with(&equals))
}

#[cfg(test)]
mod tests {
    use super::second::SECOND;
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    /// Every vendor these tests know of, registered or not. A law about the
    /// table is a law about the shape of a vendor, and the second one is where
    /// it is proved that the shape is not claude's.
    fn known() -> Vec<&'static Vendor> {
        table().iter().chain([&SECOND]).collect()
    }

    #[test]
    fn the_table_lists_claude_first_and_pi_second() {
        let names: Vec<_> = table().iter().map(|v| v.name).collect();
        assert_eq!(names, ["claude", "pi"]);
        assert!(
            !names.contains(&SECOND.name),
            "the second vendor is a fixture, not an agent anybody can spawn"
        );
    }

    #[test]
    fn an_agent_the_table_never_heard_of_has_no_entry() {
        // The other half of the same law: no entry, so no dials to offer, no
        // config keys to obey and nothing to inject. mock-claude is the
        // fixture every end to end test spawns, and it must stay unregistered.
        assert!(find("mock-claude").is_none());
        assert!(find("codex").is_none());
        assert!(find("").is_none());
    }

    #[test]
    fn an_agent_command_is_read_as_the_program_it_runs() {
        // `agent` is a command line, not a program name, so someone who
        // configures `claude --add-dir ..` still gets claude's dials.
        assert_eq!(
            find("claude --dangerously-skip-permissions").map(|v| v.name),
            Some("claude")
        );
        assert!(find("my-claude").is_none(), "a longer name is not claude");
        assert_eq!(program("claude --model opus"), "claude");
        assert_eq!(program("claude"), "claude");
    }

    #[test]
    fn every_cycle_starts_at_the_sentinel_that_injects_nothing() {
        // A dial whose cycle did not start at the sentinel would have no way
        // back to the vendor's own behaviour.
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
        // Two dials sharing a flag would inject it twice and leave the vendor
        // to decide which one it meant.
        for vendor in known() {
            let mut flags: Vec<&str> = vendor
                .dials()
                .into_iter()
                .filter_map(|(_, dial)| dial.map(|spec| spec.flag))
                .collect();
            let declared = flags.len();
            flags.sort_unstable();
            flags.dedup();
            assert_eq!(flags.len(), declared, "{} names a flag twice", vendor.name);
            assert!(
                flags.iter().all(|flag| flag.starts_with('-')),
                "{} declares a dial whose flag is not one",
                vendor.name
            );
        }
    }

    #[test]
    fn a_vendor_does_only_what_it_says_it_can_do() {
        // A capability is the question a verb asks before it refuses, and the
        // second vendor answers most of them the other way from claude, which
        // is the whole reason a verb asks rather than knows.
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
        // The capability is the question a verb asks before it refuses; the
        // entry is what install writes and what hook reads. A vendor that
        // answered the two differently would either promise a report amx has
        // no wiring for, or hold wiring nothing is allowed to use.
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
        // amx listens for a fixed set of moments and the vendor supplies the
        // names. A name given twice is two events folded into one, so no
        // moment is named twice. A moment a vendor has no event for is left
        // out and amx hears nothing of it — except the three every verb
        // stands on: which session opened, that a turn began, that it ended.
        // A vendor reporting through hooks that cannot say those is one
        // `send` could never confirm and `result` could never wait for.
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

    /// Every set of hooks amx knows: the ones on the entries in the table, and
    /// the one a vendor carries before its entry takes it up.
    fn every_hooks() -> Vec<Hooks> {
        let mut all: Vec<Hooks> = known().iter().filter_map(|vendor| vendor.hooks).collect();
        if !all.contains(&pi::HOOKS) {
            all.push(pi::HOOKS);
        }
        all
    }

    #[test]
    fn the_name_a_vendor_gives_a_moment_is_what_finds_it_again() {
        // Reading a payload is this lookup and nothing else, so an event the
        // table does not name is an event amx has no business acting on.
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
    fn the_table_finds_a_moment_under_whichever_vendors_word_for_it() {
        // Every event an entry in the table wires is found again from the
        // table alone, which is how a payload is read without knowing whose
        // it is; a word no entry uses is nobody's moment.
        for vendor in table() {
            let Some(hooks) = vendor.hooks else { continue };
            for wiring in hooks.events {
                assert_eq!(
                    moment_of(wiring.event),
                    Some(wiring.moment),
                    "{}",
                    vendor.name
                );
            }
        }
        assert_eq!(moment_of("NobodysEvent"), None);
        assert_eq!(moment_of(""), None);
    }

    #[test]
    fn a_vendor_is_wired_somewhere_under_the_persons_home() {
        // amx joins this onto a home directory. An absolute path would throw
        // the home away and write wherever the table said instead. A file
        // wire carries the file as well, and an empty one would install
        // nothing that reports.
        for hooks in every_hooks() {
            let path = hooks.wire.path();
            assert!(!path.is_empty(), "{hooks:?}");
            assert!(
                !std::path::Path::new(path).is_absolute(),
                "{hooks:?} is wired outside anybody's home"
            );
            if let Wire::File { body, .. } = hooks.wire {
                assert!(body.contains("_hook"), "{path} reports through nothing");
            }
        }
    }

    #[test]
    fn a_vendor_sentence_about_a_tool_says_where_the_tool_goes() {
        // The sentence is the vendor's own words and amx has one thing to put
        // in it. One with nowhere to put it would be quoted at whoever is
        // answering with the tool it is about left out. A vendor that draws
        // no permission box writes no sentence, and an empty one is that.
        for hooks in every_hooks() {
            assert!(
                hooks.permission_sentence.is_empty() || hooks.permission_sentence.contains(TOOL),
                "{hooks:?} writes a sentence with no room for the tool"
            );
        }
    }

    #[test]
    fn a_vendor_that_can_be_adopted_names_the_session_that_makes_it_possible() {
        // Taking over an agent amx did not start is finding, in the
        // environment of something that agent started, which conversation it
        // belongs to. A vendor claiming the one without naming the other
        // promises a verb something it cannot do.
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
        // The same law the dials answer to, over the words a session is
        // spelled with: a word that does not start with `-` would never be
        // read back as a flag once it sat in an argv.
        for vendor in known() {
            let Some(session) = vendor.session else {
                continue;
            };
            assert!(
                session.resume.starts_with('-'),
                "{}'s resume flag is not one",
                vendor.name
            );
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
                let (ForkSpec::Marker(flag) | ForkSpec::Origin(flag)) = fork;
                assert!(
                    flag.starts_with('-'),
                    "{}'s fork flag is not one",
                    vendor.name
                );
            }
        }
    }

    #[test]
    fn fork_spec_expresses_a_marker_beside_resume_and_a_flag_naming_the_origin() {
        // Both shapes measured so far, so the type does not change again once
        // a verb reads it: claude branches with a bare marker beside its
        // resume flag, and the other vendor already measured branches with a
        // flag naming the session to copy from.
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
        // The resume flag is always the one being replaced, with the session
        // being carried on to; listing it again here would say nothing a
        // reader does not already know from `resume` itself.
        for vendor in known() {
            let Some(session) = vendor.session else {
                continue;
            };
            assert!(
                !session.conflicts.contains(&session.resume),
                "{} names its own resume flag as one that conflicts with it",
                vendor.name
            );
        }
    }

    #[test]
    fn a_vendor_that_can_fork_declares_how_it_branches() {
        // Branching is asking the vendor for a second flag beside the one
        // that opens the session to copy; a vendor claiming the capability
        // without a shape for it promises a verb an argv it cannot build.
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
        // A vendor that reports through hooks has its Started hook to learn
        // a copy's id from, the way claude's does. One that reports through
        // none has no other way to find out what a fork opened, so the copy
        // is a session amx can never name again unless the entry itself can
        // ask for the id, which is what the start flag is for. pi is the
        // first entry of this shape.
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
        // The list is the vendor's own words and nothing here knows one of
        // them. What a new pane must not inherit is whatever this vendor calls
        // the session a spawn was typed inside; the tmux and shell variables
        // that go with any pane are not the vendor's business.
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
        // The variable saying which conversation a process belongs to is the
        // first one a new pane must not inherit: an agent that kept its
        // spawner's session id would file its events under somebody else.
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
    fn both_vendors_in_the_table_say_what_they_can_be_asked_for_by_name() {
        // A word typed on a task line is completed out of the vendor's own
        // files, and a vendor with no catalog is one with nowhere to look.
        // Both entries in the table have had their layout measured; the
        // fixture has not.
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
        // amx joins a place onto a home directory or onto the project the
        // agent is running in. An absolute path would throw the root away and
        // read wherever the table said instead. A built-in is the vendor's own
        // answer rather than a file, and it is named without the mark that
        // runs it: writing that mark is the line's business.
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
                    !builtin.starts_with('/'),
                    "{} spells {builtin} with the mark that runs it",
                    vendor.name
                );
            }
        }
    }

    #[test]
    fn a_vendor_that_declares_screens_declares_ones_that_parse() {
        // The document is read once, at the first look at a pane, and a
        // document that will not parse takes the binary with it there. Here
        // instead, where the vendor is being read anyway.
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
        // The vendor rejects an unlisted permission mode outright, so amx
        // saying no first is the same answer sooner.
        let permission = find("claude").unwrap().permission.unwrap();
        for mode in ["acceptEdits", "auto", "bypassPermissions", "manual", "plan"] {
            assert!(accepts(&permission, mode), "{mode}");
        }
        for refused in ["nonsense", "acceptedits", "Plan", "ask", ""] {
            assert!(!accepts(&permission, refused), "{refused:?}");
        }

        // And a closed dial of a different vendor's, whose values claude has
        // never heard of.
        let model = SECOND.model.unwrap();
        assert!(accepts(&model, "small"));
        assert!(!accepts(&model, "opus"), "that is the other vendor's word");
    }

    #[test]
    fn an_open_dial_takes_a_value_its_cycle_never_names() {
        // `--model` documents full model names beside the aliases, so the
        // cycle is what a key offers, never the set of legal values.
        let model = find("claude").unwrap().model.unwrap();
        assert!(accepts(&model, "opus"));
        assert!(accepts(&model, "claude-fable-5"));
        assert!(accepts(&model, "anything-at-all"));

        let effort = SECOND.effort.unwrap();
        assert!(accepts(&effort, "whatever-it-likes"));
    }

    #[test]
    fn a_resolved_dial_puts_its_flag_in_front_of_the_callers_args() {
        // The task is appended after all of these by the caller, so injected
        // flags go first and the caller's own args keep their order.
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
        // The proof is an absent flag, not a flag carrying the word default.
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
        // The second vendor has no permission dial, so a permission value
        // resolved for somebody else is not a flag it is handed.
        assert_eq!(
            inject(&SECOND, "large", "plan", "hard", &[], &[]),
            v(&["-m", "large", "--care", "hard"]),
        );
    }

    #[test]
    fn each_dial_emits_the_flag_its_own_vendor_declares() {
        // Nothing here knows the words --model or --effort: both vendors turn
        // the same two dials and the argv is theirs, not this module's.
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
        // Both spellings claude accepts, and the yield is per dial: a carried
        // --model leaves the other two dials alone.
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
        // And the same flag from the other half of the argv, the arguments the
        // agent command line brought with it.
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
        // A whole token or a `flag=` prefix, never a string prefix: reading
        // --models as this dial's flag would drop the injection silently.
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
}
