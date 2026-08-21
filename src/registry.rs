//! What a vendor can be launched with: the dials it declares and the one
//! rule that turns a resolved dial into vendor argv.
//!
//! Launch metadata only. What a vendor's screens mean, and how its states
//! read, is `rules` and `derive`; the day a second vendor arrives is the day
//! somebody wants to put screen rules here, and that is the wrong home for
//! them.

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

/// What a vendor can be launched with. A `None` dial means this vendor has no
/// such dial at all, which is a different thing from having one nobody has
/// turned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentEntry {
    pub name: &'static str,
    pub model: Option<DialSpec>,
    pub permission: Option<DialSpec>,
    pub effort: Option<DialSpec>,
}

/// The value every dial rests at: the vendor's own configured behaviour,
/// which amx expresses by passing no flag whatsoever. There is no flag that
/// means "whatever you were going to do", so the only way to say it is
/// silence.
pub const DEFAULT: &str = "default";

/// claude, measured against 2.1.237's `--help`. Re-measure at every vendor
/// bump: these are the vendor's words, and a renamed mode or a dropped alias
/// turns a dial into a spawn that fails.
static ENTRIES: [AgentEntry; 1] = [AgentEntry {
    name: "claude",
    // Open: `--help` says an alias "or a model's full name", so the cycle
    // lists the three aliases and the dial takes anything.
    model: Some(DialSpec {
        cycle: &[DEFAULT, "fable", "opus", "sonnet"],
        open: true,
        flag: "--model",
    }),
    // Closed, and the vendor enforces it: `--permission-mode nonsense` is a
    // hard error naming these six.
    permission: Some(DialSpec {
        cycle: &[
            DEFAULT,
            "acceptEdits",
            "auto",
            "bypassPermissions",
            "manual",
            "dontAsk",
            "plan",
        ],
        open: false,
        flag: "--permission-mode",
    }),
    // Closed by judgement rather than by the vendor: `--effort nonsense`
    // warns and falls back to the default rather than refusing. Five levels
    // are the whole documented set, so amx warns at the config it can see
    // instead of leaving the person to find the vendor's warning scrolled off
    // the top of a pane.
    effort: Some(DialSpec {
        cycle: &[DEFAULT, "low", "medium", "high", "xhigh", "max"],
        open: false,
        flag: "--effort",
    }),
}];

/// What `agent` can be launched with, or `None` when amx has registered no
/// dials for it.
///
/// `agent` is a command line rather than a program name, because that is what
/// the config key holds: the entry is found by the program the command runs.
pub fn entry(agent: &str) -> Option<&'static AgentEntry> {
    let program = agent.split_whitespace().next()?;
    ENTRIES.iter().find(|e| e.name == program)
}

/// Every registered vendor, in the order a cycle key offers them.
pub fn entries() -> &'static [AgentEntry] {
    &ENTRIES
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
/// spelling, in `vendor_args` or in `agent`'s own arguments, because both end
/// up in one argv. A flag the caller wrote wins over a dial by the dial
/// standing down, which is why nothing here has to know about precedence.
///
/// An agent with no entry is never given a flag, whatever the dials say.
pub fn inject(
    agent: &str,
    model: &str,
    permission: &str,
    effort: &str,
    vendor_args: &[String],
) -> Vec<String> {
    let mut injected: Vec<String> = Vec::new();

    if let Some(entry) = entry(agent) {
        // Declaration order, so an argv is diffable against the table it came
        // from. Nothing downstream reads the order; a person comparing two
        // spawns does.
        for (dial, value) in [
            (entry.model, model),
            (entry.permission, permission),
            (entry.effort, effort),
        ] {
            let Some(spec) = dial else {
                continue;
            };
            if value != DEFAULT && !carried(agent, vendor_args, spec.flag) {
                injected.push(spec.flag.to_string());
                injected.push(value.to_string());
            }
        }
    }

    injected.extend(vendor_args.iter().cloned());
    injected
}

/// Does the argv this spawn is heading for already carry `flag`, either as
/// its own token or as `flag=value`? A string prefix would not do: `--models`
/// is somebody else's flag, and reading it as this one would cancel the
/// injection without a word.
fn carried(agent: &str, vendor_args: &[String], flag: &str) -> bool {
    let equals = format!("{flag}=");
    let from_agent = agent.split_whitespace().skip(1).map(str::to_string);
    from_agent
        .chain(vendor_args.iter().cloned())
        .any(|arg| arg == flag || arg.starts_with(&equals))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn claude_declares_a_model_a_permission_and_an_effort_dial() {
        // Measured against claude 2.1.237's `--help`. Re-measure at every
        // vendor bump: a renamed mode or a dropped alias turns a dial into a
        // spawn that fails.
        let claude = entry("claude").expect("claude is registered");

        let model = claude.model.expect("claude has a model dial");
        assert_eq!(model.flag, "--model");
        assert_eq!(model.cycle, ["default", "fable", "opus", "sonnet"]);
        assert!(model.open, "--model takes a full model name too");

        let permission = claude.permission.expect("claude has a permission dial");
        assert_eq!(permission.flag, "--permission-mode");
        assert_eq!(
            permission.cycle,
            [
                "default",
                "acceptEdits",
                "auto",
                "bypassPermissions",
                "manual",
                "dontAsk",
                "plan"
            ]
        );
        assert!(!permission.open, "--permission-mode is a closed set");

        let effort = claude.effort.expect("claude has an effort dial");
        assert_eq!(effort.flag, "--effort");
        assert_eq!(
            effort.cycle,
            ["default", "low", "medium", "high", "xhigh", "max"]
        );
        assert!(!effort.open, "--effort is a closed set");
    }

    #[test]
    fn an_agent_the_table_never_heard_of_declares_no_dials() {
        // The other half of the same law: no entry, so no dials to offer, no
        // config keys to obey and nothing to inject. mock-claude is the
        // fixture every end to end test spawns, and it must stay unregistered.
        assert!(entry("mock-claude").is_none());
        assert!(entry("codex").is_none());
        assert!(entry("").is_none());
    }

    #[test]
    fn the_table_lists_claude_and_only_claude() {
        let names: Vec<_> = entries().iter().map(|e| e.name).collect();
        assert_eq!(names, ["claude"]);
    }

    #[test]
    fn an_agent_command_is_read_as_the_program_it_runs() {
        // `agent` is a command line, not a program name, so someone who
        // configures `claude --add-dir ..` still gets claude's dials.
        assert_eq!(
            entry("claude --dangerously-skip-permissions").map(|e| e.name),
            Some("claude")
        );
        assert!(entry("my-claude").is_none(), "a longer name is not claude");
    }

    #[test]
    fn every_cycle_starts_at_the_sentinel_that_injects_nothing() {
        // A law about the whole table rather than about claude: a dial whose
        // cycle did not start at the sentinel would have no way back to the
        // vendor's own behaviour.
        for e in entries() {
            for (dial, which) in [
                (e.model, "model"),
                (e.permission, "permission"),
                (e.effort, "effort"),
            ] {
                if let Some(spec) = dial {
                    assert_eq!(
                        spec.cycle.first(),
                        Some(&DEFAULT),
                        "{}'s {which} dial starts somewhere else",
                        e.name
                    );
                }
            }
        }
    }

    #[test]
    fn the_sentinel_is_acceptable_on_an_open_dial_and_a_closed_one() {
        let claude = entry("claude").unwrap();
        assert!(accepts(&claude.model.unwrap(), DEFAULT));
        assert!(accepts(&claude.permission.unwrap(), DEFAULT));
        assert!(accepts(&claude.effort.unwrap(), DEFAULT));
    }

    #[test]
    fn a_closed_dial_takes_its_cycle_and_nothing_else() {
        // The vendor rejects an unlisted permission mode outright, so amx
        // saying no first is the same answer sooner.
        let permission = entry("claude").unwrap().permission.unwrap();
        for mode in ["acceptEdits", "auto", "bypassPermissions", "manual", "plan"] {
            assert!(accepts(&permission, mode), "{mode}");
        }
        for refused in ["nonsense", "acceptedits", "Plan", "ask", ""] {
            assert!(!accepts(&permission, refused), "{refused:?}");
        }

        let effort = entry("claude").unwrap().effort.unwrap();
        for level in ["low", "medium", "high", "xhigh", "max"] {
            assert!(accepts(&effort, level), "{level}");
        }
        assert!(!accepts(&effort, "hard"));
    }

    #[test]
    fn an_open_dial_takes_a_value_its_cycle_never_names() {
        // `--model` documents full model names beside the aliases, so the
        // cycle is what a key offers, never the set of legal values.
        let model = entry("claude").unwrap().model.unwrap();
        assert!(accepts(&model, "opus"));
        assert!(accepts(&model, "claude-fable-5"));
        assert!(accepts(&model, "anything-at-all"));
    }

    #[test]
    fn a_resolved_dial_puts_its_flag_in_front_of_the_callers_args() {
        // The task is appended after all of these by the caller, so injected
        // flags go first and the caller's own args keep their order.
        assert_eq!(
            inject("claude", "fable", "plan", "high", &v(&["--verbose"])),
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
        assert!(inject("claude", DEFAULT, DEFAULT, DEFAULT, &[]).is_empty());
        assert_eq!(
            inject("claude", DEFAULT, "acceptEdits", DEFAULT, &v(&["-x"])),
            v(&["--permission-mode", "acceptEdits", "-x"])
        );
    }

    #[test]
    fn a_dial_yields_to_the_same_flag_already_in_the_vendor_args() {
        // Both spellings claude accepts, and the yield is per dial: a carried
        // --model leaves the other two dials alone.
        assert_eq!(
            inject("claude", "fable", "plan", DEFAULT, &v(&["--model", "opus"])),
            v(&["--permission-mode", "plan", "--model", "opus"])
        );
        assert_eq!(
            inject("claude", "fable", DEFAULT, "max", &v(&["--model=opus"])),
            v(&["--effort", "max", "--model=opus"])
        );
        assert_eq!(
            inject(
                "claude",
                "fable",
                "plan",
                "high",
                &v(&["--model=opus", "--permission-mode", "auto", "--effort=low"])
            ),
            v(&["--model=opus", "--permission-mode", "auto", "--effort=low"])
        );
    }

    #[test]
    fn a_dial_yields_to_a_flag_the_agent_command_already_carries() {
        // `agent = "claude --model opus"` is one argv with the caller's args,
        // so a dial that injected on top of it would hand claude the flag
        // twice and leave which one wins to the vendor.
        assert_eq!(
            inject("claude --model opus", "fable", DEFAULT, "high", &[]),
            v(&["--effort", "high"])
        );
    }

    #[test]
    fn a_flag_that_merely_begins_the_same_is_not_that_dials_flag() {
        // A whole token or a `flag=` prefix, never a string prefix: reading
        // --models as this dial's flag would drop the injection silently.
        assert_eq!(
            inject("claude", "fable", DEFAULT, DEFAULT, &v(&["--models"])),
            v(&["--model", "fable", "--models"])
        );
        assert_eq!(
            inject(
                "claude",
                "fable",
                DEFAULT,
                DEFAULT,
                &v(&["--model-name=opus"])
            ),
            v(&["--model", "fable", "--model-name=opus"])
        );
    }

    #[test]
    fn an_unregistered_agent_never_has_a_flag_injected() {
        // Whatever the dials say, an agent with no entry spawns exactly as it
        // did before there were dials. The caller's own args still travel.
        assert!(inject("mock-claude", "fable", "plan", "high", &[]).is_empty());
        assert_eq!(
            inject("mock-claude", "fable", "plan", "high", &v(&["-p", "hi"])),
            v(&["-p", "hi"])
        );
    }
}
