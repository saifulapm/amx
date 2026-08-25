//! The crate's door to the vendor table.
//!
//! Everything amx knows about a vendor lives in `vendor`, keyed by a program
//! name. Everything that asks holds the `agent` config key instead, which is a
//! command line: `claude`, or `claude --add-dir ..`, or a wrapper script
//! somebody wrote. Reading that key as a vendor, and reading the arguments it
//! carries as part of the argv a spawn is heading for, is all this is.

use crate::vendor::{self, Vendor};

pub use crate::vendor::{DEFAULT, DialSpec, accepts, program};

/// What `agent` can be launched with, or `None` when amx has registered no
/// dials for it.
pub fn entry(agent: &str) -> Option<&'static Vendor> {
    vendor::find(agent)
}

/// Every registered vendor, in the order a cycle key offers them.
pub fn entries() -> &'static [Vendor] {
    vendor::table()
}

/// The dials this spawn resolved, as vendor argv in front of `vendor_args`.
///
/// The agent command's own arguments are half of the argv the vendor will
/// see, so a dial stands down for a flag written there just as it does for one
/// written in `vendor_args`. They are not passed on from here: the caller put
/// them in the command line already, and adding them again would run the
/// program with its own arguments twice.
///
/// An agent with no entry is never given a flag, whatever the dials say.
pub fn inject(
    agent: &str,
    model: &str,
    permission: &str,
    effort: &str,
    vendor_args: &[String],
) -> Vec<String> {
    let Some(vendor) = entry(agent) else {
        return vendor_args.to_vec();
    };
    let carried: Vec<String> = agent
        .split_whitespace()
        .skip(1)
        .map(str::to_string)
        .collect();
    vendor::inject(vendor, model, permission, effort, &carried, vendor_args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_registry_answers_out_of_the_vendor_table() {
        assert_eq!(entry("claude").map(|v| v.name), Some("claude"));
        assert_eq!(
            entries().iter().map(|v| v.name).collect::<Vec<_>>(),
            vendor::table().iter().map(|v| v.name).collect::<Vec<_>>()
        );
        assert!(entry("mock-claude").is_none());
    }

    #[test]
    fn an_agent_command_is_read_as_the_program_it_runs() {
        // `agent` is a command line, not a program name, so someone who
        // configures `claude --add-dir ..` still gets claude's dials.
        assert_eq!(
            entry("claude --dangerously-skip-permissions").map(|v| v.name),
            Some("claude")
        );
        assert_eq!(program("claude --add-dir .."), "claude");
        assert!(entry("my-claude").is_none(), "a longer name is not claude");
    }

    #[test]
    fn a_resolved_dial_puts_its_flag_in_front_of_the_callers_args() {
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
        // The proof of a sentinel is an absent flag, not a flag carrying the
        // word default.
        assert!(inject("claude", DEFAULT, DEFAULT, DEFAULT, &[]).is_empty());
    }

    #[test]
    fn a_dial_yields_to_a_flag_the_agent_command_already_carries() {
        // `agent = "claude --model opus"` is one argv with the caller's args,
        // so a dial that injected on top of it would hand claude the flag
        // twice and leave which one wins to the vendor. The command's own
        // arguments are not repeated here: they are already in the command.
        assert_eq!(
            inject("claude --model opus", "fable", DEFAULT, "high", &[]),
            v(&["--effort", "high"])
        );
        assert_eq!(
            inject(
                "claude --model=opus",
                "fable",
                DEFAULT,
                DEFAULT,
                &v(&["-x"])
            ),
            v(&["-x"])
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
