//! amx: run coding agents as tmux panes.
//!
//! This file declares the modules, dispatches the parsed command line to its
//! verb, and owns how amx writes to stderr: every line goes through
//! [`complain!`] or [`warn!`], sanitized and coloured by [`Severity`].

mod ansi;
mod catalog;
mod cli;
mod cockpit;
mod config;
mod consent;
mod conversation;
mod derive;
mod errand;
mod gc;

// Some modules below have items only tests reach. `expect` rather than
// `allow`, so the compiler flags the attribute once they all have callers.
mod exit;
mod furniture;
mod hook;
mod ids;
mod install;
mod models;
mod notify;
mod paths;
mod pr;
mod registry;
mod role;
#[cfg_attr(not(test), expect(dead_code, reason = "reached by the tests alone"))]
mod rules;
mod shade;
mod spawn;
#[cfg_attr(not(test), expect(dead_code, reason = "reached by the tests alone"))]
mod store;
mod theme;
#[cfg_attr(not(test), expect(dead_code, reason = "reached by the tests alone"))]
mod tmux;
mod trust;
mod tui;
mod vendor;
mod verbs;
mod worktree;

use anyhow::Result;
use clap::Parser;
use std::io::IsTerminal;
use std::process::ExitCode;

/// Print a failure to stderr, in red on a terminal. Takes `format!` arguments.
///
/// Verbs write to stderr only through this or [`warn!`].
#[macro_export]
macro_rules! complain {
    ($($arg:tt)*) => { $crate::tell($crate::Severity::Failed, &format!($($arg)*)) };
}

/// Print a warning, or a refusal that is not a failure, to stderr in yellow on
/// a terminal. Takes `format!` arguments.
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { $crate::tell($crate::Severity::Warned, &format!($($arg)*)) };
}

fn main() -> ExitCode {
    let code = match cli::Cli::try_parse_from(std::env::args_os()) {
        Ok(parsed) => {
            // Config problems are warnings and the verb still runs. The
            // underscore verbs run inside agent panes and stay silent.
            let (config, warnings) = config::load();
            let internal = parsed.verb().is_some_and(|verb| verb.starts_with('_'));
            if !internal {
                for warning in warnings {
                    warn!("amx: {warning}");
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

/// Run the parsed command line and return its exit code.
fn run(cli: &cli::Cli, config: &config::Config) -> i32 {
    match &cli.command {
        Some(cli::Command::Hook) => hook::from_env(&mut std::io::stdin().lock(), config),
        Some(cli::Command::Exit { id, code }) => hook::exited_from_env(id, *code, config),
        Some(cli::Command::New(args)) => finish(verbs::new::from_env(config, args)),
        Some(cli::Command::Sub(args)) => finish(verbs::sub::from_env(args)),
        Some(cli::Command::Ls { json, dir }) => finish(verbs::ls::from_env(
            *json,
            dir.as_deref().or(cli.dir.as_deref()),
        )),
        Some(cli::Command::Status { id, json }) => finish(verbs::status::from_env(id, *json)),
        Some(cli::Command::Send { id, text, file }) => {
            finish(verbs::send::from_env(id, text.as_deref(), file.as_deref()))
        }
        Some(cli::Command::Answer { id, key }) => finish(verbs::answer::from_env(id, key)),
        Some(cli::Command::Interrupt { id }) => finish(verbs::interrupt::from_env(id)),
        Some(cli::Command::Rename { id, name }) => finish(verbs::rename::from_env(id, name)),
        Some(cli::Command::Allow { dir, forget }) => finish(verbs::allow::from_env(
            dir.as_deref().or(cli.dir.as_deref()),
            *forget,
        )),
        Some(cli::Command::Result {
            id,
            children,
            json,
            timeout,
        }) => match children {
            Some(parent) => finish(verbs::result::family_from_env(parent, *timeout, *json)),
            None => finish(verbs::result::from_env(
                id.as_deref()
                    .expect("an id or --children, and clap refuses both"),
                *timeout,
            )),
        },
        Some(cli::Command::Wait {
            ids,
            children,
            any,
            state,
            timeout,
        }) => finish(verbs::wait::from_env(
            ids,
            children.as_deref(),
            *any,
            *state,
            *timeout,
        )),
        Some(cli::Command::Attach {
            id,
            next,
            prev,
            last,
            ..
        }) => {
            use verbs::attach::Aim;
            // clap allows exactly one of the id and the four flags, so the
            // fallthrough is --waiting.
            let aim = match id {
                Some(id) => Aim::Id(id.clone()),
                None if *next => Aim::Next,
                None if *prev => Aim::Prev,
                None if *last => Aim::Last,
                None => Aim::Waiting,
            };
            finish(verbs::attach::from_env(&aim))
        }
        Some(cli::Command::Logs { id, lines }) => finish(verbs::logs::from_env(id, *lines)),
        Some(cli::Command::Diff { id, stat, from }) => {
            finish(verbs::diff::from_env(id, *stat, from.as_deref()))
        }
        Some(cli::Command::Resume { id, message, all }) => finish(verbs::resume::from_env(
            config,
            id.as_deref(),
            message.as_deref(),
            *all,
        )),
        Some(cli::Command::Adopt(args)) => finish(verbs::adopt::from_env(args)),
        Some(cli::Command::Fork { id, task }) => {
            finish(verbs::fork::from_env(config, id, task.as_deref()))
        }
        Some(cli::Command::Events { ids, follow, json }) => {
            finish(verbs::events::from_env(ids, *follow, *json))
        }
        Some(cli::Command::Statusline) => finish(verbs::statusline::from_env()),
        Some(cli::Command::Boot { id }) => finish(spawn::boot_from_env(id)),
        Some(cli::Command::Park { id }) => finish(verbs::park::from_env(id)),
        Some(cli::Command::Stop(args)) => finish(verbs::stop::from_env(args)),
        Some(cli::Command::Sweep { force }) => finish(verbs::sweep::from_env(*force)),
        Some(cli::Command::Clear { force }) => finish(verbs::clear::from_env(*force)),
        Some(cli::Command::Doctor { fix }) => {
            finish(verbs::doctor::from_env(*fix, cli.dir.as_deref()))
        }
        Some(cli::Command::Setup { vendor, subagent }) => {
            finish(verbs::setup::from_env(vendor.as_deref(), *subagent))
        }
        Some(cli::Command::Uninstall) => finish(verbs::uninstall::from_env()),
        Some(cli::Command::Completion { shell }) => finish(completion(*shell)),
        None => finish(cockpit::from_env(config, cli.dir.as_deref())),
    }
}

/// Write a shell's completion script to stdout.
fn completion(shell: clap_complete::Shell) -> Result<i32> {
    use std::io::Write;
    std::io::stdout()
        .lock()
        .write_all(&cli::completion_script(shell))?;
    Ok(exit::OK)
}

/// A verb's outcome as an exit code, printing the error on failure.
///
/// A closed stdout pipe is not a failure: see [`broke_the_pipe`].
fn finish(outcome: Result<i32>) -> i32 {
    match outcome {
        Ok(code) => code,
        Err(e) if broke_the_pipe(&e) => exit::OK,
        Err(e) => {
            complain!("amx: {e:#}");
            exit::FAILURE
        }
    }
}

/// How serious a stderr line is, which sets its colour.
///
/// The same split the view's notices use: red for something that failed,
/// yellow for a warning or a deliberate refusal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    /// Something amx was asked to do failed.
    Failed,
    /// A warning, or a refusal that is not a failure.
    Warned,
}

impl Severity {
    /// The SGR foreground code.
    ///
    /// The terminal's own red and yellow, so the line follows the person's
    /// theme like git's and cargo's output does.
    fn colour(self) -> &'static str {
        match self {
            Severity::Failed => "31",
            Severity::Warned => "33",
        }
    }
}

/// A stderr line, sanitized and then coloured when `to_terminal`.
///
/// The text quotes things amx did not write (a typed id, a path, git's error
/// output), so control sequences are stripped with [`tmux::sanitize`] before
/// the colour is added, never after. Newlines are kept.
///
/// Each row is coloured separately and closed with `39` (default foreground),
/// so nothing stays open across a line break.
pub fn said(severity: Severity, text: &str, to_terminal: bool) -> String {
    let inert = tmux::sanitize(text);
    if !to_terminal {
        return inert;
    }
    inert
        .split('\n')
        .map(|row| match row.is_empty() {
            true => String::new(),
            false => format!("\u{1b}[{}m{row}\u{1b}[39m", severity.colour()),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Print one line to stderr, coloured when stderr (not stdout) is a terminal.
///
/// Reached through [`complain!`] and [`warn!`].
pub fn tell(severity: Severity, text: &str) {
    eprintln!("{}", said(severity, text, std::io::stderr().is_terminal()));
}

/// Whether the error, anywhere in its chain, is a broken pipe on output.
///
/// Rust ignores SIGPIPE, so `amx diff <id> | head` gets an `EPIPE` error rather
/// than being killed; amx leaves it that way so a verb can finish writing a
/// record. The caller treats it as the signal would: silent, exit 0.
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
        // The id is quoted back as typed, escape sequences included.
        let line = said(
            Severity::Failed,
            "amx: no agent `x\u{1b}]0;PWNED\u{7}y`",
            false,
        );
        assert!(line.starts_with("amx: no agent"), "{line:?}");
        assert!(line.contains("]0;PWNED"), "still readable: {line:?}");
        assert_eq!(
            line.chars().filter(|c| c.is_control()).count(),
            0,
            "and inert: {line:?}"
        );

        // Newlines survive.
        let git = said(
            Severity::Failed,
            "amx: git diff 0f1e2d3: fatal: bad object\nsecond line",
            false,
        );
        assert!(git.ends_with("bad object\nsecond line"), "{git:?}");
    }

    #[test]
    fn a_failure_is_red_and_a_warning_yellow_on_a_terminal() {
        assert_eq!(
            said(Severity::Failed, "amx: no agent `fix-login-a1b`", true),
            "\u{1b}[31mamx: no agent `fix-login-a1b`\u{1b}[39m"
        );
        assert_eq!(
            said(Severity::Warned, "amx: nothing to answer", true),
            "\u{1b}[33mamx: nothing to answer\u{1b}[39m"
        );
    }

    #[test]
    fn nothing_is_painted_down_a_pipe() {
        for severity in [Severity::Failed, Severity::Warned] {
            assert_eq!(
                said(severity, "amx: no agent `fix-login-a1b`", false),
                "amx: no agent `fix-login-a1b`"
            );
        }
    }

    #[test]
    fn a_line_of_several_rows_opens_and_closes_its_colour_on_each() {
        assert_eq!(
            said(Severity::Failed, "amx: fatal: bad object\nsecond", true),
            "\u{1b}[31mamx: fatal: bad object\u{1b}[39m\n\u{1b}[31msecond\u{1b}[39m"
        );
        // Empty rows get no colour codes.
        assert_eq!(
            said(Severity::Warned, "one\n\ntwo", true),
            "\u{1b}[33mone\u{1b}[39m\n\n\u{1b}[33mtwo\u{1b}[39m"
        );
    }

    #[test]
    fn hardening_the_only_escapes_in_a_painted_line_are_the_ones_amx_wrote() {
        // Sanitized before colouring: the only escapes left are amx's own.
        let line = said(Severity::Failed, "amx: `x\u{1b}]0;PWNED\u{7}y`", true);
        assert_eq!(line.matches('\u{1b}').count(), 2, "{line:?}");
        assert!(line.starts_with("\u{1b}[31m"), "{line:?}");
        assert!(line.ends_with("\u{1b}[39m"), "{line:?}");
        assert!(line.contains("]0;PWNED"), "still readable: {line:?}");
    }

    /// The source of every verb, checked for direct `eprint` calls that
    /// bypass [`tell`].
    const VERBS: [(&str, &str); 26] = [
        ("adopt", include_str!("verbs/adopt.rs")),
        ("allow", include_str!("verbs/allow.rs")),
        ("answer", include_str!("verbs/answer.rs")),
        ("attach", include_str!("verbs/attach.rs")),
        ("clear", include_str!("verbs/clear.rs")),
        ("diff", include_str!("verbs/diff.rs")),
        ("doctor", include_str!("verbs/doctor.rs")),
        ("events", include_str!("verbs/events.rs")),
        ("fork", include_str!("verbs/fork.rs")),
        ("interrupt", include_str!("verbs/interrupt.rs")),
        ("logs", include_str!("verbs/logs.rs")),
        ("ls", include_str!("verbs/ls.rs")),
        ("new", include_str!("verbs/new.rs")),
        ("park", include_str!("verbs/park.rs")),
        ("rename", include_str!("verbs/rename.rs")),
        ("result", include_str!("verbs/result.rs")),
        ("resume", include_str!("verbs/resume.rs")),
        ("send", include_str!("verbs/send.rs")),
        ("setup", include_str!("verbs/setup.rs")),
        ("status", include_str!("verbs/status.rs")),
        ("statusline", include_str!("verbs/statusline.rs")),
        ("stop", include_str!("verbs/stop.rs")),
        ("sub", include_str!("verbs/sub.rs")),
        ("sweep", include_str!("verbs/sweep.rs")),
        ("uninstall", include_str!("verbs/uninstall.rs")),
        ("wait", include_str!("verbs/wait.rs")),
    ];

    #[test]
    fn every_verb_says_its_piece_through_one_of_the_two_severities() {
        let files = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src/verbs"))
            .unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_name() != "mod.rs")
            .count();
        assert_eq!(files, VERBS.len(), "a verb this test does not read");
        for (verb, source) in VERBS {
            // Test code may print however it likes.
            let code = source.split("#[cfg(test)]").next().unwrap_or(source);
            assert!(
                !code.contains("eprint"),
                "{verb} says something on stderr without saying how loudly"
            );
        }
    }

    #[test]
    fn hardening_a_reader_that_stopped_reading_is_not_a_failure() {
        // The io error arrives wrapped in the verb's own context.
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
        assert_eq!(finish(Ok(exit::BLOCKED)), exit::BLOCKED);
        assert_eq!(
            finish(Err(anyhow::anyhow!("no agent `fix-login-a1b`"))),
            exit::FAILURE
        );
    }
}
