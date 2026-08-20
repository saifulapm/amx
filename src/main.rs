mod cli;
mod config;
mod derive;
mod gc;

// The verbs are stubs, so outside their own tests parts of these have no
// caller yet. `expect` rather than `allow`: the day every item is reached, the
// compiler asks for the attribute back — as every exit code now has a verb
// that answers with it.
mod exit;
mod hook;
mod ids;
mod install;
mod notify;
#[cfg_attr(not(test), expect(dead_code, reason = "the verbs are still stubs"))]
mod paths;
#[cfg_attr(not(test), expect(dead_code, reason = "the verbs are still stubs"))]
mod rules;
mod spawn;
#[cfg_attr(not(test), expect(dead_code, reason = "the verbs are still stubs"))]
mod store;
#[cfg_attr(not(test), expect(dead_code, reason = "the verbs are still stubs"))]
mod tmux;
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
        Some(cli::Command::Events { ids, follow }) => finish(verbs::events::from_env(ids, *follow)),
        Some(cli::Command::Boot { id }) => finish(spawn::boot_from_env(id)),
        Some(cli::Command::Stop(args)) => finish(verbs::stop::from_env(args)),
        Some(cli::Command::Doctor { fix }) => finish(verbs::doctor::from_env(config, *fix)),
        Some(cli::Command::Uninstall) => finish(verbs::uninstall::from_env()),
        _ => {
            eprintln!("{}", stub_line(cli.verb()));
            exit::FAILURE
        }
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

/// What a verb says while it has no implementation behind it.
fn stub_line(verb: Option<&str>) -> String {
    match verb {
        Some(verb) => format!("amx {verb}: not implemented yet"),
        None => "amx: not implemented yet".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stub_names_the_verb_and_says_it_does_nothing_yet() {
        let line = stub_line(Some("status"));
        assert!(line.contains("status"), "{line}");
        assert!(line.contains("not implemented"), "{line}");
    }

    #[test]
    fn the_front_door_is_a_stub_too() {
        let line = stub_line(None);
        assert!(line.contains("amx"), "{line}");
        assert!(line.contains("not implemented"), "{line}");
    }

    #[test]
    fn a_stub_run_fails_rather_than_reporting_success() {
        // A verb with nothing behind it yet, and one that reads nothing while
        // it says so.
        let cli = cli::Cli::try_parse_from(["amx", "resume", "fix-login-a1b"]).unwrap();
        assert_eq!(run(&cli, &config::Config::default()), exit::FAILURE);
    }
}
