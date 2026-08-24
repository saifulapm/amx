mod ansi;
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
mod paths;
mod pr;
mod registry;
#[cfg_attr(not(test), expect(dead_code, reason = "reached by the tests alone"))]
mod rules;
mod spawn;
#[cfg_attr(not(test), expect(dead_code, reason = "reached by the tests alone"))]
mod store;
#[cfg_attr(not(test), expect(dead_code, reason = "reached by the tests alone"))]
mod tmux;
mod trust;
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
                    eprintln!("{}", complaint(&warning));
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
        Some(cli::Command::Ls { json, dir }) => finish(verbs::ls::from_env(
            *json,
            dir.as_deref().or(cli.dir.as_deref()),
        )),
        Some(cli::Command::Status { id, json }) => finish(verbs::status::from_env(id, *json)),
        Some(cli::Command::Send { id, text }) => finish(verbs::send::from_env(id, text)),
        Some(cli::Command::Answer { id, key }) => finish(verbs::answer::from_env(id, key)),
        Some(cli::Command::Result { id, timeout }) => finish(verbs::result::from_env(id, *timeout)),
        Some(cli::Command::Attach { id }) => finish(verbs::attach::from_env(id)),
        Some(cli::Command::Logs { id, lines }) => finish(verbs::logs::from_env(id, *lines)),
        Some(cli::Command::Diff { id, stat }) => finish(verbs::diff::from_env(id, *stat)),
        Some(cli::Command::Resume { id, all }) => {
            finish(verbs::resume::from_env(config, id.as_deref(), *all))
        }
        Some(cli::Command::Adopt(args)) => finish(verbs::adopt::from_env(args)),
        Some(cli::Command::Fork { id, task }) => {
            finish(verbs::fork::from_env(config, id, task.as_deref()))
        }
        Some(cli::Command::Events { ids, follow, json }) => {
            finish(verbs::events::from_env(ids, *follow, *json))
        }
        Some(cli::Command::Statusline) => finish(verbs::statusline::from_env()),
        Some(cli::Command::Boot { id }) => finish(spawn::boot_from_env(id)),
        Some(cli::Command::Stop(args)) => finish(verbs::stop::from_env(args)),
        Some(cli::Command::Doctor { fix }) => finish(verbs::doctor::from_env(config, *fix)),
        Some(cli::Command::Uninstall) => finish(verbs::uninstall::from_env()),
        None => finish(cockpit::from_env(config, cli.dir.as_deref())),
    }
}

/// A verb's outcome as an exit code: what it decided, or a failure with the
/// reason on stderr.
fn finish(outcome: Result<i32>) -> i32 {
    match outcome {
        Ok(code) => code,
        Err(e) if broke_the_pipe(&e) => exit::OK,
        Err(e) => {
            eprintln!("{}", complaint(&format!("{e:#}")));
            exit::FAILURE
        }
    }
}

/// One line about something that went wrong, with nothing in it a terminal
/// will act on.
///
/// Every complaint quotes something amx did not write: the id as it was typed
/// at the shell, a path out of a record, git's own words about a repository it
/// would not read. A terminal is an interpreter, and the same bytes that name
/// an agent can retitle the window or clear the screen, so they go through the
/// sieve a captured pane goes through. Line breaks live, because git says its
/// piece in as many lines as it likes and the breaks in it are how it reads.
fn complaint(text: &str) -> String {
    format!("amx: {}", tmux::sanitize(text))
}

/// Whether what went wrong is the far end of a pipe closing.
///
/// A verb writes its answer to stdout, and `amx diff fix-login-a1b | head`
/// takes that pipe away the moment head has its ten lines. Rust turns SIGPIPE
/// off for the whole process before `main`, so the write comes back as an
/// error instead of ending the process the way the signal would — and amx
/// keeps it that way, because a verb halfway through writing a record should
/// get to finish it.
///
/// What is left is to answer as the signal would have: nothing on stderr,
/// which may be the same pipe, and no failure to report, because a reader that
/// has what it came for is not a failure. It is looked for down the whole
/// chain, since the io error arrives wrapped in whatever the verb was doing.
fn broke_the_pipe(e: &anyhow::Error) -> bool {
    e.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardening_a_complaint_says_nothing_a_terminal_will_act_on() {
        // Every line amx prints about a failure quotes something it did not
        // write. The id is whatever was typed at the shell, and it is quoted
        // back in the refusal.
        let said = complaint("no agent `x\u{1b}]0;PWNED\u{7}y`");
        assert!(said.starts_with("amx: no agent"), "{said:?}");
        assert!(said.contains("]0;PWNED"), "still readable: {said:?}");
        assert_eq!(
            said.chars().filter(|c| c.is_control()).count(),
            0,
            "and inert: {said:?}"
        );

        // git says its piece in as many lines as it likes, and the breaks in
        // it are how it reads.
        let git = complaint("git diff 0f1e2d3: fatal: bad object\nsecond line");
        assert!(git.ends_with("bad object\nsecond line"), "{git:?}");
    }

    #[test]
    fn hardening_a_reader_that_stopped_reading_is_not_a_failure() {
        // `amx diff fix-login-a1b | head` closes the pipe as soon as head has
        // what it asked for. Rust turns that into an error rather than the
        // signal a shell would report as 141, and it arrives here wrapped in
        // whatever the verb was doing at the time.
        let closed = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "Broken pipe (os error 32)",
        ))
        .context("reading the diff");
        assert!(broke_the_pipe(&closed));
        assert_eq!(finish(Err(closed)), exit::OK, "and says nothing about it");

        let elsewhere = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::StorageFull,
            "No space left on device",
        ))
        .context("writing the diff");
        assert!(
            !broke_the_pipe(&elsewhere),
            "another write that would not go"
        );
        assert!(!broke_the_pipe(&anyhow::anyhow!(
            "no agent `fix-login-a1b`"
        )));
    }

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
