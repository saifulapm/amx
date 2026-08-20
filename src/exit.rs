//! Process exit codes.
//!
//! These are amx's API: callers — a person's shell script and the orchestrator
//! alike — branch on them. Changing a number here changes the contract.

/// Success. `result`: the answer was printed.
pub const OK: i32 = 0;

/// Failure, and no answer is coming. `result`: the agent is failed or stopped.
pub const FAILURE: i32 = 1;

/// Blocked: `result` on a waiting agent (its question goes to stdout), `send`
/// refused because the agent is waiting, `answer` with nothing pending, `new`
/// at `max_agents`.
pub const BLOCKED: i32 = 2;

/// `result --timeout` expired.
pub const TIMEOUT: i32 = 3;

/// Malformed command line (`EX_USAGE`). An unknown verb or flag, a missing
/// argument or an unparseable option value is not a state-machine outcome, so
/// it never borrows one of the codes above. `--help` and `--version` are not
/// failures and exit `OK`.
pub const USAGE: i32 = 64;

#[cfg(test)]
mod tests {
    use super::*;

    /// The numbers are the contract; this test is the tripwire that says so.
    #[test]
    fn exit_codes_are_pinned() {
        assert_eq!(OK, 0);
        assert_eq!(FAILURE, 1);
        assert_eq!(BLOCKED, 2);
        assert_eq!(TIMEOUT, 3);
        assert_eq!(USAGE, 64);
    }
}
