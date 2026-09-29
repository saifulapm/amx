//! Process exit codes.
//!
//! These are part of amx's interface: scripts and orchestrators branch on
//! them, so changing a number is a breaking change.

/// Success. For `result`, the answer was printed.
pub const OK: i32 = 0;

/// Failure with no answer coming. For `result`, the agent failed or was
/// stopped. For any verb, the id names no agent.
pub const FAILURE: i32 = 1;

/// Blocked. Returned by:
///
/// - `result`, `sub` and `interrupt` on a waiting agent (the question goes to
///   stdout);
/// - `send` to a waiting agent, and `answer` with nothing pending;
/// - `new`, `sub`, `fork` and `resume` at `max_agents` or `max_total`;
/// - `new` and `sub` past `subagent_depth`, and `sub` past `max_children` or
///   refused a `--permission`;
/// - `resume` on an agent that is still running.
pub const BLOCKED: i32 = 2;

/// `result --timeout` expired.
pub const TIMEOUT: i32 = 3;

/// Malformed command line (`EX_USAGE`): an unknown verb or flag, a missing
/// argument, or an unparseable value. `--help` and `--version` exit `OK`.
pub const USAGE: i32 = 64;

#[cfg(test)]
mod tests {
    use super::*;

    /// The numbers are the contract, so they are pinned.
    #[test]
    fn exit_codes_are_pinned() {
        assert_eq!(OK, 0);
        assert_eq!(FAILURE, 1);
        assert_eq!(BLOCKED, 2);
        assert_eq!(TIMEOUT, 3);
        assert_eq!(USAGE, 64);
    }
}
