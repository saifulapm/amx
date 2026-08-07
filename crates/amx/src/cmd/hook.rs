//! `amx _hook <agent>` — the agent hook emitter.
//!
//! D-M2-4: "read the agent's hook payload from stdin, map it to a single
//! `agent.report` control call over the session socket, exit 0 no matter what."
//!
//! The whole design is a reaction to one herdr failure: its hooks are sh plus a
//! python heredoc, they silently no-op when `python3` is missing, and
//! `integration status` still reports `current` because it only greps a version
//! comment. amx's emitter *is* the binary that already speaks the protocol, so
//! the failure mode is deleted rather than detected. (herdr's own Windows asset
//! already shells to the herdr CLI, which is the strongest evidence the POSIX
//! heredoc was never necessary.)
//!
//! The call sequence, and its budget: connect → Hello/Welcome → one request →
//! read reply → exit, under a total of about 500 ms. **Every** failure — no
//! socket, refused, timed out, an error reply, a server mid-shutdown — is a
//! silent success from the agent's point of view, because a hook must never
//! break or slow a turn, and because agents surface hook noise to the user.
//!
//! Identity comes from the environment V07 injects into every pane child:
//! `AMX_SOCKET`, `AMX_PANE_ID`, `AMX_HOOK_TOKEN`. V01 §3 M6 measured that chain
//! reaching hook processes on both agents — all 185 invocations in the run
//! carried all six variables verbatim, through an interactive shell — so the
//! scheme stands as designed and the degraded `session_id`-plus-process-tree
//! branch is not needed.
//!
//! Forward, do not filter. The emitter tags subagent scope from `agent_id` and
//! sends everything else through; the fusion machine owns the policy. herdr
//! baked its filtering into the installed scripts and paid for it by needing a
//! reinstall on every machine to change a rule.
//!
//! # Task ownership
//!
//! **V09** fills [`run`], together with `dispatch/agent.rs`'s report arm and
//! `crates/amx/tests/hook.rs`. Its hardest tests are the miserable paths —
//! missing env, absent socket, server mid-shutdown — because silent success is
//! the contract, so the tests have to tell "silently succeeded" from "silently
//! did nothing" through the hub's counters.
//!
//! V02 planted the file and the clap tree so no wave task edits `cli.rs`.

use std::process::ExitCode;

use amx_core::Env;
use clap::ArgMatches;

/// Forward one hook payload, and exit 0 whatever happens.
///
/// **V09 fills this.** The signature already promises the contract: there is no
/// `Result`, because there is no failure this command reports.
pub async fn run(env: &Env, matches: &ArgMatches) -> ExitCode {
    let _ = (env, matches);
    // Silence is the contract, so an unfinished emitter behaves exactly like a
    // finished one whose server is not listening: it does nothing, says
    // nothing, and gets out of the agent's way.
    ExitCode::SUCCESS
}
