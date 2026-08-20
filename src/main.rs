mod cli;
mod cockpit;
mod config;
mod derive;
mod gc;

// Parts of these are reached only by the tests that pin them: a store field
// nothing reads back yet, a tmux call no verb has needed. `expect` rather than
// `allow`: the day every item has a caller, the compiler asks for the
// attribute back.
mod exit;
mod hook;
mod ids;
mod install;
mod notify;
#[cfg_attr(not(test), expect(dead_code, reason = "reached by the tests alone"))]
mod paths;
#[cfg_attr(not(test), expect(dead_code, reason = "reached by the tests alone"))]
mod rules;
mod spawn;
#[cfg_attr(not(test), expect(dead_code, reason = "reached by the tests alone"))]
mod store;
#[cfg_attr(not(test), expect(dead_code, reason = "reached by the tests alone"))]
mod tmux;
mod tui;
mod verbs;
mod worktree;

use anyhow::Result;
use clap::Parser;
use std::process::ExitCode;

fn main() -> ExitCode {
    let code = match cli::Cli::try_parse_from(std::env::args_os()) {
        Ok(parsed) => {
            // Config is a convenience, so anything wrong with it is said once
            // and the verb runs anyway — except in the verbs amx runs against
            // itself inside an agent's pane, which say nothing at all.
            let (config, warnings) = config::load();
            let internal = parsed.verb().is_some_and(|verb| verb.starts_with('_'));
            if !internal {
                for warning in warnings {
                    eprintln!("amx: {warning}");
                }
            }
            run(&parsed, &config)
        }
        Err(err) => {
            let _ = err.print();
            cli::usage_exit_code(&err)
        }
    };
    ExitCode::from(code as u8)
}

/// Run the parsed command line and answer with its exit code.
fn run(cli: &cli::Cli, config: &config::Config) -> i32 {
    match &cli.command {
        Some(cli::Command::Hook) => hook::from_env(&mut std::io::stdin().lock(), config),
        Some(cli::Command::Exit { id, code }) => hook::exited_from_env(id, *code, config),
        Some(cli::Command::New(args)) => finish(verbs::new::from_env(config, args)),
        Some(cli::Command::Ls { json }) => finish(verbs::ls::from_env(*json)),
        Some(cli::Command::Status { id, json }) => finish(verbs::status::from_env(id, *json)),
        Some(cli::Command::Send { id, text }) => finish(verbs::send::from_env(id, text)),
        Some(cli::Command::Answer { id, key }) => finish(verbs::answer::from_env(id, key)),
        Some(cli::Command::Result { id, timeout }) => finish(verbs::result::from_env(id, *timeout)),
        Some(cli::Command::Attach { id }) => finish(verbs::attach::from_env(id)),
        Some(cli::Command::Diff { id }) => finish(verbs::diff::from_env(id)),
        Some(cli::Command::Resume { id, all }) => {
            finish(verbs::resume::from_env(config, id.as_deref(), *all))
        }
        Some(cli::Command::Events { ids, follow }) => finish(verbs::events::from_env(ids, *follow)),
        Some(cli::Command::Boot { id }) => finish(spawn::boot_from_env(id)),
        Some(cli::Command::Stop(args)) => finish(verbs::stop::from_env(args)),
        Some(cli::Command::Doctor { fix }) => finish(verbs::doctor::from_env(config, *fix)),
        Some(cli::Command::Uninstall) => finish(verbs::uninstall::from_env()),
        None => finish(cockpit::from_env()),
    }
}

/// A verb's outcome as an exit code: what it decided, or a failure with the
/// reason on stderr.
fn finish(outcome: Result<i32>) -> i32 {
    match outcome {
        Ok(code) => code,
        Err(e) => {
            eprintln!("amx: {e:#}");
            exit::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verb_that_cannot_reach_its_agent_fails_with_the_reason() {
        // Every verb is behind `finish`, which is what turns anything that
        // went wrong into an exit code and a line saying so.
        assert_eq!(finish(Ok(exit::BLOCKED)), exit::BLOCKED);
        assert_eq!(
            finish(Err(anyhow::anyhow!("no agent `fix-login-a1b`"))),
            exit::FAILURE
        );
    }
}
