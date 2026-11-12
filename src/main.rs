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
mod rules;
mod shade;
mod spawn;
mod store;
mod theme;
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
            // underscore verbs run inside agent panes, stay silent, and read
            // the config only when they need it: a nested hook never does.
            let internal = parsed.verb().is_some_and(|verb| verb.starts_with('_'));
            if !internal {
                for warning in config::init() {
                    warn!("amx: {warning}");
                }
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

/// Run the parsed command line and return its exit code.
fn run(cli: &cli::Cli) -> i32 {
    match &cli.command {
        Some(cli::Command::Hook) => hook::from_env(&mut std::io::stdin().lock()),
        Some(cli::Command::Exit { id, code }) => {
            hook::exited_from_env(id, *code, config::current())
        }
        Some(cli::Command::New(args)) => finish(verbs::new::from_env(args)),
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
            id.as_deref(),
            message.as_deref(),
            *all,
        )),
        Some(cli::Command::Adopt(args)) => finish(verbs::adopt::from_env(args)),
        Some(cli::Command::Fork { id, task }) => finish(verbs::fork::from_env(id, task.as_deref())),
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
        None => finish(cockpit::from_env(config::current(), cli.dir.as_deref())),
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

    /// The scrub every run of this suite needs, quoted by the guard below.
    const CLEAN_ROOM: &str = "env -u AMX_BIN -u AMX_AGENT_DIR -u AMX_NESTED -u AMX_DIR -u AMX_ID \
        -u TMUX -u TMUX_PANE HOME=$(mktemp -d) CARGO_HOME=~/.cargo RUSTUP_HOME=~/.rustup cargo test";

    #[test]
    fn the_suite_runs_outside_an_amx_pane_over_an_empty_home() {
        // From an agent's pane the suite reads that pane's variables and the
        // person's own home, and a dozen unrelated tests fail with no hint
        // why. This one names the cause.
        let leaked: Vec<&str> = [
            "AMX_BIN",
            "AMX_AGENT_DIR",
            "AMX_NESTED",
            "AMX_DIR",
            "AMX_ID",
            "TMUX",
            "TMUX_PANE",
        ]
        .into_iter()
        .filter(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
        .collect();
        let home = std::env::home_dir().unwrap_or_default();
        let lived_in: Vec<&str> = [".local/state/amx", ".claude"]
            .into_iter()
            .filter(|dir| home.join(dir).exists())
            .collect();
        assert!(
            leaked.is_empty() && lived_in.is_empty(),
            "the suite is not isolated: set {leaked:?}, {} holds {lived_in:?}; run it as `{CLEAN_ROOM}`",
            home.display()
        );
    }

    #[test]
    fn the_suite_builds_outside_tmp() {
        // A build under /tmp, a scratchpad included, runs out of the quota
        // part way and reads as a page of unrelated failures.
        let exe = std::env::current_exe().unwrap_or_default();
        assert!(
            !exe.starts_with("/tmp"),
            "the suite is built under /tmp ({}); point CARGO_TARGET_DIR \
             somewhere with room, such as ~/.cache",
            exe.display()
        );
    }

    #[test]
    fn a_complaint_says_nothing_a_terminal_will_act_on() {
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
    fn the_only_escapes_in_a_painted_line_are_the_ones_amx_wrote() {
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
    fn a_reader_that_stopped_reading_is_not_a_failure() {
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

    /// Every place outside tests that reads a record's own phase or the time
    /// it entered it, with what covers it when the reading disagrees with the
    /// record: a test that builds such a pair, or why a disagreement cannot
    /// mislead it.
    ///
    /// A prompt only the screen shows leaves the record working since before
    /// it, and a pane that died leaves it working over a stopped verdict. Two
    /// rules tested only on agreeing pairs were wrong (timed sleep's early
    /// wake, sweep's second reading).
    const RECORDED_PHASE_READS: [(&str, &str, &str); 8] = [
        (
            "src/tui/rows.rs",
            "view.state.since,",
            "a timed sleep also wakes on the verdict's own change, \
             test arranged_a_question_read_off_the_screen_ends_a_timed_sleep_too; \
             the idle shelve counts from the last change the record knows",
        ),
        (
            "src/tui/rows.rs",
            "shelves(self.shelved.get(view.id()), view.phase(), view.state.since)",
            "a mark holds only while the verdict says the turn is over, and an older \
             since only keeps it",
        ),
        (
            "src/tui/rows.rs",
            "view.state.last_event.max(view.state.since)",
            "shown as an age, decides nothing",
        ),
        (
            "src/derive.rs",
            "\"since\": self.state.since,",
            "printed beside the verdict's own age, decides nothing",
        ),
        (
            "src/derive.rs",
            "let alive = if record.state.state.is_terminal() {",
            "a finished record is final whatever its pane shows",
        ),
        (
            "src/verbs/statusline.rs",
            ".filter(|record| !record.state.state.is_terminal())",
            "a finished record is final whatever its pane shows",
        ),
        (
            "src/verbs/doctor.rs",
            "view.state.state == Phase::Starting && view.phase() == Phase::Unknown",
            "asks for the disagreement itself",
        ),
        (
            "src/verbs/sweep.rs",
            "let (seq, was) = (listed.state.seq, listed.state.state);",
            "test sweep_takes_a_candidate_whose_record_says_working_over_a_pane_that_died",
        ),
    ];

    /// The production code of every source file under `src`, by path.
    fn production_sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, found: &mut Vec<(String, String)>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, found);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    let code = text.split("\n#[cfg(test)]\nmod ").next().unwrap_or(&text);
                    let name = path
                        .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                        .unwrap()
                        .display()
                        .to_string();
                    found.push((name, code.to_string()));
                }
            }
        }
        let mut found = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut found,
        );
        found
    }

    #[test]
    fn every_read_of_a_records_own_phase_says_what_covers_a_reading_that_disagrees() {
        let sources = production_sources();
        let mut unlisted = Vec::new();
        for (file, code) in &sources {
            // Test modules in their own file are test code.
            if file.ends_with("tests.rs") {
                continue;
            }
            for line in code.lines() {
                let reads = [".state.state", ".state.since"].iter().any(|field| {
                    line.match_indices(field).any(|(at, _)| {
                        !line[at + field.len()..]
                            .starts_with(|c: char| c.is_alphanumeric() || c == '_')
                    })
                });
                let line = line.trim();
                if reads
                    && !RECORDED_PHASE_READS
                        .iter()
                        .any(|(listed, read, _)| listed == file && line.contains(read))
                {
                    unlisted.push(format!("{file}: {line}"));
                }
            }
        }
        assert!(
            unlisted.is_empty(),
            "these read the record's phase or its since, which a reading of the \
             screen can disagree with; use the view's phase, or add each to \
             RECORDED_PHASE_READS with a test that builds a verdict over a record \
             that disagrees (a Stopped verdict over a Working record, a Waiting \
             one over Working):\n{}",
            unlisted.join("\n")
        );

        for (file, read, covered) in RECORDED_PHASE_READS {
            let code = &sources.iter().find(|(name, _)| name == file).unwrap().1;
            assert!(code.contains(read), "{file} no longer reads `{read}`");
            for (at, _) in covered.match_indices("test ") {
                let test: String = covered[at + 5..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                let whole = std::fs::read_to_string(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file),
                )
                .unwrap();
                assert!(
                    whole.contains(&format!("fn {test}()")),
                    "{file}: `{read}` names a test it does not hold: {test}"
                );
            }
        }
    }

    /// Every file amx names as a constant and reads back, with the tests that
    /// forge one bad entry beside good ones in it, or why it has none.
    /// A file's name, the tests that forge a bad entry in it (file and name),
    /// and how it reads.
    type ReadBack = (
        &'static str,
        &'static [(&'static str, &'static str)],
        &'static str,
    );

    const READ_BACK: [ReadBack; 12] = [
        (
            "view.json",
            &[(
                "src/tui/tests.rs",
                "remembered_a_field_that_does_not_read_costs_that_field_and_not_the_marks",
            )],
            "each field reads on its own",
        ),
        (
            "visited.json",
            &[(
                "src/verbs/attach.rs",
                "attach_an_entry_on_the_trail_that_does_not_read_costs_that_entry_and_not_the_trail",
            )],
            "each entry reads on its own",
        ),
        (
            "pr.json",
            &[(
                "src/pr.rs",
                "a_request_that_does_not_read_costs_that_request_and_not_the_others",
            )],
            "each request reads on its own",
        ),
        (
            "state.json",
            &[
                (
                    "src/derive.rs",
                    "records_reads_a_question_kind_this_build_has_never_heard_of_as_unknown",
                ),
                (
                    "src/derive.rs",
                    "records_reads_an_answer_source_this_build_has_never_heard_of_as_unknown",
                ),
                (
                    "src/hook.rs",
                    "hook_keeps_the_keys_a_newer_amx_wrote_in_both_files",
                ),
            ],
            "a newer amx only adds keys and values; a field of the wrong type is a \
             hand edit and costs the row by design (finding B2RZWETZ)",
        ),
        (
            "meta.json",
            &[(
                "src/store.rs",
                "store_a_rewrite_keeps_the_keys_a_newer_amx_wrote",
            )],
            "a newer amx only adds keys; a field of the wrong type is a hand edit \
             and costs the row by design (finding B2RZWETZ)",
        ),
        (
            "events.jsonl",
            &[(
                "src/store.rs",
                "store_survives_a_damaged_line_without_losing_the_rest",
            )],
            "each line reads on its own",
        ),
        (
            "handoff.json",
            &[],
            "one record the agent's own spawn writes once and resume reads whole",
        ),
        (
            "boot-env.json",
            &[],
            "one record the agent's own spawn writes and its boot reads and removes",
        ),
        (
            ".claude.json",
            &[(
                "src/trust.rs",
                "trust_forgets_the_tree_and_leaves_every_other_key_alone",
            )],
            "claude's own file: amx forgets one tree and keeps every other key",
        ),
        (
            ".claude-plugin/plugin.json",
            &[],
            "the plugin amx installs; uninstall reads it as any JSON or none",
        ),
        (
            "hooks.json",
            &[],
            "the plugin's hooks amx installs; doctor only says whether it is there",
        ),
        (
            "summary.asked",
            &[],
            "one claim about one agent, whose loss costs one summary ask",
        ),
    ];

    #[test]
    fn every_file_amx_reads_back_names_the_test_that_forges_one_bad_entry_in_it() {
        // One bad field in view.json lost every mark, twice, and one bad
        // request in pr.json hid every badge. So a file amx keeps and reads
        // back is listed here with the test that proves one bad entry costs
        // that entry and not the good ones beside it.
        let mut unlisted = Vec::new();
        let mut named = Vec::new();
        for (file, code) in production_sources() {
            for line in code.lines() {
                let line = line.trim();
                let Some(value) = line
                    .strip_prefix("pub ")
                    .or(line.strip_prefix("pub(crate) "))
                    .unwrap_or(line)
                    .strip_prefix("const ")
                    .and_then(|rest| rest.split_once(": &str = \""))
                    .and_then(|(_, value)| value.strip_suffix("\";"))
                else {
                    continue;
                };
                let kept = [".json", ".jsonl", ".asked"]
                    .iter()
                    .any(|ext| value.ends_with(ext));
                if kept {
                    named.push(value.to_string());
                    if !READ_BACK.iter().any(|(name, _, _)| *name == value) {
                        unlisted.push(format!("{file}: {value}"));
                    }
                }
            }
        }
        assert!(
            unlisted.is_empty(),
            "these files are read back but not in READ_BACK; read each field or \
             entry on its own, forge one bad entry beside good ones in a test, and \
             list it:\n{}",
            unlisted.join("\n")
        );

        for (name, tests, why) in READ_BACK {
            assert!(
                named.iter().any(|value| value == name),
                "{name} is no longer a file amx names"
            );
            assert!(!why.is_empty(), "{name} says nothing of how it reads");
            for (file, test) in tests {
                let whole = std::fs::read_to_string(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file),
                )
                .unwrap();
                assert!(
                    whole.contains(&format!("fn {test}()")),
                    "{name}: {file} holds no test {test}"
                );
            }
        }
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
