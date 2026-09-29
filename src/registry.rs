//! Lookups into the vendor table by the `agent` config key.
//!
//! `agent` holds a command line (`claude`, `claude --add-dir ..`, or a wrapper
//! script), so this module resolves it to a [`Vendor`] entry and treats the
//! arguments it carries as part of the argv a spawn builds.

use crate::vendor::{self, Vendor};

pub use crate::vendor::{DEFAULT, DialSpec, accepts, program};

/// The vendor entry for `agent`, or `None` for a program amx has no entry for.
pub fn entry(agent: &str) -> Option<&'static Vendor> {
    vendor::find(agent)
}

/// Every registered vendor, in the order a cycle key offers them.
pub fn entries() -> &'static [Vendor] {
    vendor::table()
}

/// The resolved dials as vendor argv, followed by `vendor_args`.
///
/// Arguments already on the `agent` command line count as present, so a dial
/// stands down for a flag written there too. They are not returned: the caller
/// already has them in the command. An agent with no entry gets no flags.
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
        // `claude --add-dir ..` still gets claude's dials.
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
        // The sentinel injects no flag at all.
        assert!(inject("claude", DEFAULT, DEFAULT, DEFAULT, &[]).is_empty());
    }

    #[test]
    fn a_dial_yields_to_a_flag_the_agent_command_already_carries() {
        // The command's own arguments share the argv with the injected ones,
        // so injecting the same flag would pass it twice. They are not
        // repeated in the result.
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
        // The caller's own args still pass through.
        assert!(inject("mock-claude", "fable", "plan", "high", &[]).is_empty());
        assert_eq!(
            inject("mock-claude", "fable", "plan", "high", &v(&["-p", "hi"])),
            v(&["-p", "hi"])
        );
    }
}
