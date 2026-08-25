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
/// One real entry today. The table is the whole of what makes a second vendor
/// possible, and a test-only [`second`] proves that nothing in here is shaped
/// around the first.
static TABLE: [Vendor; 1] = [claude::VENDOR];

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
    fn the_table_lists_claude_and_only_claude() {
        let names: Vec<_> = table().iter().map(|v| v.name).collect();
        assert_eq!(names, ["claude"]);
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
