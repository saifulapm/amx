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
use std::io::IsTerminal;
use std::process::ExitCode;

/// Say on stderr that something amx was asked to do could not be done.
///
/// Takes what `format!` takes. Every verb reaches stderr through this or
/// through [`warn!`], so what a line is worth is written down where the line is
/// and nowhere else.
#[macro_export]
macro_rules! complain {
    ($($arg:tt)*) => { $crate::tell($crate::Severity::Failed, &format!($($arg)*)) };
}

/// Say on stderr a warning, or a refusal that is not a failure.
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { $crate::tell($crate::Severity::Warned, &format!($($arg)*)) };
}

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
            complain!("amx: {e:#}");
            exit::FAILURE
        }
    }
}

/// What a line on stderr is worth, and so the colour it is said in.
///
/// Two channels, the split the view's notices already make at the foot of the
/// screen: something amx was asked to do and could not is red, and a warning
/// or a refusal — amx working as it should, and saying so — is yellow. One
/// colour for both teaches people to read neither.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    /// It was attempted and it failed.
    Failed,
    /// A warning, or a refusal that is not a failure.
    Warned,
}

impl Severity {
    /// The foreground this severity is said in.
    ///
    /// The terminal's own red and yellow rather than the values the view
    /// measured out of the vendor's binary: a line here lands in a shell among
    /// git's and cargo's, where the person's own theme is what everything else
    /// obeys.
    fn colour(self) -> &'static str {
        match self {
            Severity::Failed => "31",
            Severity::Warned => "33",
        }
    }
}

/// One line for stderr: made inert first, then painted by what it is worth.
///
/// Every line amx says for itself quotes something it did not write: the id as
/// it was typed at the shell, a path out of a record, git's own words about a
/// repository it would not read. A terminal is an interpreter, and the same
/// bytes that name an agent can retitle the window or clear the screen, so they
/// go through the sieve a captured pane goes through. Line breaks live, because
/// git says its piece in as many lines as it likes and the breaks in it are how
/// it reads.
///
/// **The words are made inert before the colour goes on, never after.** A sieve
/// run over the paint would take the paint with it, and paint laid over an
/// escape somebody else wrote would be handing that escape to a terminal in
/// amx's own voice.
///
/// The colour is opened and closed on every row it covers, so a complaint git
/// wrote in four rows leaves nothing open across a break another program's
/// output may arrive in. `39` closes the foreground and nothing else, because a
/// foreground is all that was opened.
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

/// Say one line on stderr, in colour when stderr is a terminal.
///
/// **Whether *stderr* is a terminal is the question, and stdout's answer is not
/// it.** `amx result fix-login-a1b | jq` took stdout and left this line on the
/// screen it was always going to; `amx new "…" 2>notes` wants the words in the
/// file and nothing else. The two macros above are how this is reached.
pub fn tell(severity: Severity, text: &str) {
    eprintln!("{}", said(severity, text, std::io::stderr().is_terminal()));
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

        // git says its piece in as many lines as it likes, and the breaks in
        // it are how it reads.
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
        // `amx send x y 2>notes` wants the words in the file and nothing else,
        // and a caller matching on stderr is reading text, not paint.
        for severity in [Severity::Failed, Severity::Warned] {
            assert_eq!(
                said(severity, "amx: no agent `fix-login-a1b`", false),
                "amx: no agent `fix-login-a1b`"
            );
        }
    }

    #[test]
    fn a_line_of_several_rows_opens_and_closes_its_colour_on_each() {
        // A colour left open across a row boundary is one that bleeds into
        // whatever is printed next, and git says its piece in as many rows as
        // it likes.
        assert_eq!(
            said(Severity::Failed, "amx: fatal: bad object\nsecond", true),
            "\u{1b}[31mamx: fatal: bad object\u{1b}[39m\n\u{1b}[31msecond\u{1b}[39m"
        );
        // A row with nothing on it is nothing to paint.
        assert_eq!(
            said(Severity::Warned, "one\n\ntwo", true),
            "\u{1b}[33mone\u{1b}[39m\n\n\u{1b}[33mtwo\u{1b}[39m"
        );
    }

    #[test]
    fn hardening_the_only_escapes_in_a_painted_line_are_the_ones_amx_wrote() {
        // The words are made inert before the colour goes on, never after: a
        // sanitiser run over the paint would take the paint with it, and paint
        // laid over an escape somebody else wrote would hand it to a terminal.
        let line = said(Severity::Failed, "amx: `x\u{1b}]0;PWNED\u{7}y`", true);
        assert_eq!(line.matches('\u{1b}').count(), 2, "{line:?}");
        assert!(line.starts_with("\u{1b}[31m"), "{line:?}");
        assert!(line.ends_with("\u{1b}[39m"), "{line:?}");
        assert!(line.contains("]0;PWNED"), "still readable: {line:?}");
    }

    /// Every source that has anything to say on stderr. A verb printing there
    /// with `eprintln!` is one saying it in whatever colour was already in
    /// force, which is the split this pair of macros exists to keep.
    const VERBS: [(&str, &str); 17] = [
        ("adopt", include_str!("verbs/adopt.rs")),
        ("answer", include_str!("verbs/answer.rs")),
        ("attach", include_str!("verbs/attach.rs")),
        ("diff", include_str!("verbs/diff.rs")),
        ("doctor", include_str!("verbs/doctor.rs")),
        ("events", include_str!("verbs/events.rs")),
        ("fork", include_str!("verbs/fork.rs")),
        ("logs", include_str!("verbs/logs.rs")),
        ("ls", include_str!("verbs/ls.rs")),
        ("new", include_str!("verbs/new.rs")),
        ("result", include_str!("verbs/result.rs")),
        ("resume", include_str!("verbs/resume.rs")),
        ("send", include_str!("verbs/send.rs")),
        ("status", include_str!("verbs/status.rs")),
        ("statusline", include_str!("verbs/statusline.rs")),
        ("stop", include_str!("verbs/stop.rs")),
        ("uninstall", include_str!("verbs/uninstall.rs")),
    ];

    #[test]
    fn every_verb_says_its_piece_through_one_of_the_two_severities() {
        for (verb, source) in VERBS {
            // Only what ships: a test of its own may print however it likes,
            // and what it prints goes to whoever is running the suite.
            let code = source.split("#[cfg(test)]").next().unwrap_or(source);
            assert!(
                !code.contains("eprint"),
                "{verb} says something on stderr without saying how loudly"
            );
        }
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
