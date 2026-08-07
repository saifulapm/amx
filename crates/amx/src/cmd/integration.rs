//! `amx integration install|uninstall|status`.
//!
//! 04 §8: "Integrations have a lifecycle, not just an install: `amx integration
//! install|uninstall|status` with version markers in installed hook assets, and
//! `status` runs after self-update — a stale `amx _hook` is worse for amx than
//! for herdr, since hooks feed the fusion tier."
//!
//! What V01 measured, and what **V10** must therefore say out loud:
//!
//! - **Claude Code** needs no hook-approval step. A settings file written
//!   seconds before launch ran its hooks. But the *folder-trust* dialog is a
//!   hard gate: in a directory Claude Code had never seen, no hook fired at all
//!   for the eight seconds the dialog was up, and the dialog does not even
//!   mention hooks. So install is genuinely non-interactive, and honest status
//!   wording is: installed hooks run in any workspace the user has already
//!   trusted; in a brand new one their next interactive launch asks once.
//! - **Codex 0.147.0 is not the Codex the plan described.** `codex features
//!   list` prints `hooks  stable  true` — enabled by default, no flag to set;
//!   there is no `codex_hooks` feature (R-M2-2 should be withdrawn, and 04 §5's
//!   spelling was right). Config is `$CODEX_HOME/hooks.json`, a JSON file with
//!   PascalCase event keys shaped like Claude Code's settings block. And the
//!   trust gate is real and stronger than described: a fresh `hooks.json`
//!   raises a blocking startup prompt, declining is silent and total (a full
//!   turn ran with zero hook invocations), `codex exec` fires nothing and warns
//!   nothing, and the gate is keyed to the file's content hash. **No
//!   user-readable trust record was found**, so `status` must report that it
//!   cannot see trust state rather than implying the hooks are live.
//!
//! Two rules for the editing itself, both from herdr's scars:
//!
//! - **Preserve foreign entries byte for byte.** Only amx-owned entries,
//!   matched by command shape, are ever touched, and reinstalling over a
//!   current install is a no-op write.
//! - **`status` verifies the thing that actually breaks.** herdr's check greps
//!   a version comment and reports `current` for an installation that silently
//!   does nothing; amx's checks the marker *and* that the referenced binary
//!   exists and executes.
//!
//! # Task ownership
//!
//! **V10** fills this and `crates/amx/src/integration/{mod,claude,codex,edit}.rs`
//! beside it. The event list per agent is registry data (V03's stanzas), not a
//! list here — writing an agent's name anywhere but its stanza is W6 growing
//! back. `status` after self-update is M3's wiring: leave the function
//! callable, not called.
//!
//! V02 planted the file and the clap tree so no wave task edits `cli.rs`.

use std::process::ExitCode;

use amx_core::Env;
use clap::ArgMatches;

/// Run the integration verb `matches` names.
///
/// **V10 fills this.**
pub async fn run(env: &Env, matches: &ArgMatches, sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    let _ = (env, matches, sub);
    anyhow::bail!("amx integration is not implemented yet; V10 fills it")
}
