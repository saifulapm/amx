mod cli;
mod config;

// The verbs are stubs, so outside their own tests parts of these have no
// caller yet. `expect` rather than `allow`: the day every item is reached, the
// compiler asks for the attribute back.
#[cfg_attr(not(test), expect(dead_code, reason = "the verbs are still stubs"))]
mod exit;
#[cfg_attr(not(test), expect(dead_code, reason = "the verbs are still stubs"))]
mod ids;
#[cfg_attr(not(test), expect(dead_code, reason = "the verbs are still stubs"))]
mod paths;
#[cfg_attr(not(test), expect(dead_code, reason = "the verbs are still stubs"))]
mod rules;
#[cfg_attr(not(test), expect(dead_code, reason = "the verbs are still stubs"))]
mod store;
#[cfg_attr(not(test), expect(dead_code, reason = "the verbs are still stubs"))]
mod tmux;

use clap::Parser;
use std::process::ExitCode;

fn main() -> ExitCode {
    let code = match cli::Cli::try_parse_from(std::env::args_os()) {
        Ok(parsed) => {
            // Config is a convenience, so anything wrong with it is said once
            // and the verb runs anyway.
            let (_config, warnings) = config::load();
            for warning in warnings {
                eprintln!("amx: {warning}");
            }
            run(&parsed)
        }
        Err(err) => {
            let _ = err.print();
            cli::usage_exit_code(&err)
        }
    };
    ExitCode::from(code as u8)
}

/// Run the parsed command line and answer with its exit code.
fn run(cli: &cli::Cli) -> i32 {
    eprintln!("{}", stub_line(cli.verb()));
    exit::FAILURE
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
        let cli = cli::Cli::try_parse_from(["amx", "ls"]).unwrap();
        assert_eq!(run(&cli), exit::FAILURE);
    }
}
