//! Notices to the person when an agent stops on a question or finishes, and the
//! moment commands ([`Errand`]) that run at the same points.
//!
//! A notice goes where the `notifications` key says: the desktop notifier, the
//! terminals of attached tmux clients (for a person over SSH), both, or
//! neither. Delivery is best effort. The hook forks a notifier and returns at
//! once; the notifier asks tmux whether the pane is being watched, skips the
//! notice if so, and starts the errand. Failures are silent.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::Delivery;
use crate::store::Phase;
use crate::tmux::{PaneId, Server};

/// The variables tmux sets in every pane: its server and the pane's id. A hook
/// runs in the agent's pane, so these name the pane without a tmux call.
const SERVER_ENV: &str = "TMUX";
const PANE_ENV: &str = "TMUX_PANE";

/// Something to tell the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

impl Notice {
    /// An agent has stopped on a question.
    pub fn waiting(id: &str, question: Option<&str>) -> Notice {
        Notice {
            title: format!("{id} needs an answer"),
            body: question.unwrap_or("waiting on a question").to_string(),
        }
    }

    /// An agent's command has finished, or `None` for a phase not worth a
    /// notice.
    pub fn finished(id: &str, phase: Phase, exit: Option<i32>) -> Option<Notice> {
        let body = match (phase, exit) {
            (Phase::Done, _) => "finished".to_string(),
            (Phase::Failed, Some(code)) => format!("failed, exit {code}"),
            (Phase::Failed, None) => "failed".to_string(),
            // Whoever stopped an agent already knows.
            _ => return None,
        };
        Some(Notice {
            title: format!("{id} {body}"),
            body: body.to_string(),
        })
    }
}

/// A moment command ready to start, assembled by [`crate::errand`].
///
/// Passed to [`post`] so the one fork that delivers the notice also starts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Errand {
    /// The command line from the config, run with `sh -c`.
    pub command: String,
    /// Where it runs: the agent's tree, or the directory it was started in.
    pub dir: PathBuf,
    /// Variables set on top of the inherited environment.
    pub env: Vec<(String, String)>,
    /// The event that moved the agent, as one JSON line.
    pub stdin: Vec<u8>,
}

/// Deliver a notice and start an errand, off the hook path.
///
/// Forks a notifier and returns at once, since the hook's caller is waiting.
/// The notifier asks tmux once whether the pane is watched: a watched pane gets
/// no notice, and the errand is told the answer. Without fork the work runs
/// inline, where an extra notification is the cheaper error.
pub fn post(notice: Option<&Notice>, delivery: Delivery, errand: Option<&Errand>) {
    // Nothing to deliver and nothing to run is no reason to fork.
    let notice = notice.filter(|_| delivery.tells());
    if notice.is_none() && errand.is_none() {
        return;
    }

    match detach() {
        Fork::Hook => (),
        Fork::Notifier => {
            deliver(notice, delivery, errand);
            // The child is a copy of the hook, whose work is done; returning
            // would run the rest of it twice.
            unsafe { nix::libc::_exit(crate::exit::OK) };
        }
        Fork::Neither => deliver(notice, delivery, errand),
    }
}

/// Deliver the notice, unless the pane is watched, and start the errand.
fn deliver(notice: Option<&Notice>, delivery: Delivery, errand: Option<&Errand>) {
    let watched = watched(var(SERVER_ENV).as_deref(), var(PANE_ENV).as_deref());
    if let Some(notice) = notice.filter(|_| !watched) {
        if delivery.desktop() {
            raise(notice);
        }
        if delivery.terminal() {
            tell_terminals(notice);
        }
    }
    if let Some(errand) = errand {
        start(errand, Some(watched));
    }
}

/// Start an errand with `sh -c` and return without waiting for it.
///
/// The event goes in on stdin, never in argv, where it could be read as
/// syntax. Output is discarded. The line is small enough that the write never
/// blocks, and closing stdin afterwards marks its end. A detached thread reaps
/// the child.
///
/// `watched` sets [`crate::errand::WATCHED_ENV`]; `None`, from a caller with no
/// pane to ask about, leaves it unset.
pub fn start(errand: &Errand, watched: Option<bool>) {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(&errand.command)
        .current_dir(&errand.dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (name, value) in &errand.env {
        command.env(name, value);
    }
    if let Some(watched) = watched {
        command.env(crate::errand::WATCHED_ENV, if watched { "1" } else { "0" });
    }

    // A command that cannot be started is ignored: it is the person's errand,
    // not the agent's work.
    let Ok(mut child) = command.spawn() else {
        return;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&errand.stdin);
    }
    // The view and `stop` call this from long-lived processes, where an
    // unwaited child stays a zombie.
    let _ = std::thread::Builder::new()
        .name("amx-errand".to_string())
        .spawn(move || child.wait());
}

/// Which side of the fork a process is on.
enum Fork {
    /// The hook, whose part is over.
    Hook,
    /// The forked notifier.
    Notifier,
    /// Fork failed; the caller is still the hook.
    Neither,
}

/// Fork a notifier and return at once in the hook.
///
/// The child drops two things it inherited:
///
/// - The hook's stdio. The vendor reads the hook's output until every holder of
///   the pipe closes it, so keeping it would add the notifier's time (about
///   2ms) back to the hook.
/// - The pane's session, via `setsid`. Stopping an agent signals the pane's
///   process group, which the notifier is not part of.
fn detach() -> Fork {
    use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};

    // SIGHUP is ignored across the fork. After `_exit` the pane's shell exits
    // at once and the kernel hangs up its foreground process group, which the
    // child is in until `setsid`. An ignored signal is discarded, not left
    // pending, so the child cannot die in that window.
    let ignore = SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty());
    // SAFETY: installing SIG_IGN runs no handler code.
    let before = unsafe { sigaction(Signal::SIGHUP, &ignore) }.ok();
    let restore = || {
        if let Some(before) = &before {
            // SAFETY: puts back the disposition read above.
            let _ = unsafe { sigaction(Signal::SIGHUP, before) };
        }
    };

    // SAFETY: the hook is single threaded, and between this fork and the
    // command it starts the child reads its own environment and nothing else.
    match unsafe { nix::unistd::fork() } {
        Ok(nix::unistd::ForkResult::Parent { .. }) => {
            restore();
            Fork::Hook
        }
        Ok(nix::unistd::ForkResult::Child) => {
            hand_back_stdio();
            let _ = nix::unistd::setsid();
            restore();
            Fork::Notifier
        }
        Err(_) => {
            restore();
            Fork::Neither
        }
    }
}

/// Point stdin, stdout and stderr at `/dev/null`, so readers of the old ones
/// see EOF.
fn hand_back_stdio() {
    let Ok(null) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")
    else {
        return;
    };
    let null = std::os::fd::AsFd::as_fd(&null);
    let _ = nix::unistd::dup2_stdin(null);
    let _ = nix::unistd::dup2_stdout(null);
    let _ = nix::unistd::dup2_stderr(null);
}

/// Whether someone is already looking at the pane named by these variables.
fn watched(server: Option<&str>, pane: Option<&str>) -> bool {
    pane_here(server, pane).is_some_and(|(server, pane)| server.pane_watched(&pane))
}

/// The server and pane named by `$TMUX` and `$TMUX_PANE`, if both are set.
///
/// Outside tmux there is no pane, and nobody is watching it.
fn pane_here(server: Option<&str>, pane: Option<&str>) -> Option<(Server, PaneId)> {
    Some((Server::from_tmux_env(server?)?, PaneId::new(pane?).ok()?))
}

/// An environment variable, with an empty value read as unset.
fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Start the desktop notifier without waiting for it or checking its result.
fn raise(notice: &Notice) {
    let Some(mut command) = notifier(notice) else {
        return;
    };
    let _ = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// The desktop notification command: `osascript` on macOS, else `notify-send`.
fn notifier(notice: &Notice) -> Option<Command> {
    if cfg!(target_os = "macos") {
        let mut command = Command::new("osascript");
        command.arg("-e").arg(format!(
            "display notification \"{}\" with title \"{}\"",
            applescript(&notice.body),
            applescript(&notice.title)
        ));
        return Some(command);
    }

    let mut command = Command::new("notify-send");
    command
        .arg("--app-name=amx")
        .arg("--")
        .arg(&notice.title)
        .arg(&notice.body);
    Some(command)
}

/// Escape a string for an AppleScript string literal.
fn applescript(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Write the notice to the tty of every client of every tmux server here.
///
/// For a person over SSH, whose desktop is on another machine. Every server,
/// since the view and the agents may be on different ones; each tty once.
fn tell_terminals(notice: &Notice) {
    let bytes = osc_notice(notice);
    for tty in terminals(crate::tmux::servers_here()) {
        tell(&tty, &bytes);
    }
}

/// The client ttys of these servers, without duplicates.
fn terminals(servers: Vec<Server>) -> Vec<PathBuf> {
    let mut ttys: Vec<PathBuf> = Vec::new();
    for server in servers {
        for tty in server.client_ttys() {
            if !ttys.contains(&tty) {
                ttys.push(tty);
            }
        }
    }
    ttys
}

/// The notice as an OSC 777 notification, the urxvt sequence tmux, foot,
/// wezterm and others support.
///
/// The title and body are agent text inside an escape sequence, so every
/// control character (C0, DEL and C1) is removed from both, and the title's
/// semicolons become commas, since `;` separates title from body.
pub fn osc_notice(notice: &Notice) -> Vec<u8> {
    let mut bytes = b"\x1b]777;notify;".to_vec();
    bytes.extend(inert(&notice.title.replace(';', ",")));
    bytes.push(b';');
    bytes.extend(inert(&notice.body));
    bytes.push(0x07);
    bytes
}

/// `text` without control characters, as bytes.
fn inert(text: &str) -> Vec<u8> {
    text.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .into_bytes()
}

/// Write `bytes` to one tty, ignoring any failure.
///
/// `O_NOCTTY` because the notifier is a session leader and would otherwise
/// acquire the tty as its controlling terminal. `O_NONBLOCK` so a tty nobody
/// reads cannot hold the notifier open.
pub fn tell(tty: &Path, bytes: &[u8]) {
    use std::os::unix::fs::OpenOptionsExt;
    let Ok(mut terminal) = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(nix::libc::O_NOCTTY | nix::libc::O_NONBLOCK)
        .open(tty)
    else {
        return;
    };
    let _ = terminal.write_all(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::{Socket, Spawn};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    /// A private tmux server, killed on drop.
    struct TestServer {
        socket: String,
        server: Server,
    }

    impl TestServer {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let socket = format!(
                "amx-test-notify-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            // An empty conf, so the developer's ~/.tmux.conf cannot affect
            // the tests.
            let server = Server::named(&socket).with_conf("/dev/null");
            Self { socket, server }
        }

        /// `$TMUX` as tmux sets it in a pane of this server: the socket path,
        /// then two fields nothing here reads.
        fn tmux_env(&self) -> String {
            let path = self
                .server
                .run(&["display-message", "-p", "#{socket_path}"])
                .unwrap();
            format!("{path},4242,0")
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            let _ = self.server.kill();
        }
    }

    /// A command that runs until killed, so the pane stays.
    fn idle() -> Spawn<'static> {
        Spawn {
            command: &["sh", "-c", "while :; do sleep 0.05; done"],
            ..Spawn::default()
        }
    }

    /// Poll `f` until it returns true, failing after ten seconds.
    fn until(what: &str, mut f: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {what}");
    }

    #[test]
    fn notify_names_the_pane_it_is_in_without_asking_tmux() {
        // tmux sets both in every pane, and the hook runs in the agent's pane,
        // so naming the pane costs no tmux call.
        let (server, pane) = pane_here(Some("/tmp/tmux-test/amx,4242,0"), Some("%7")).unwrap();
        assert_eq!(
            server.socket(),
            &Socket::Path(PathBuf::from("/tmp/tmux-test/amx"))
        );
        assert_eq!(pane.as_str(), "%7");

        // No pane named, so nothing is asked of tmux and the notice goes out.
        assert!(!watched(None, Some("%7")));
        assert!(!watched(Some("/tmp/tmux-test/amx,4242,0"), None));
        assert!(!watched(Some(""), Some("%7")));
        assert!(!watched(
            Some("/tmp/tmux-test/amx,4242,0"),
            Some("amx-wall")
        ));
    }

    #[test]
    fn notify_says_nothing_while_somebody_is_looking_at_the_pane() {
        let agent = TestServer::new();
        let (session, pane) = agent.server.new_session(&idle()).unwrap();
        let inside = agent.tmux_env();

        assert!(
            !watched(Some(&inside), Some(pane.as_str())),
            "an agent nobody is attached to is worth being told about"
        );

        // `session_attached` counts clients, so attach one from a pane on
        // another server.
        let watcher = TestServer::new();
        let attach = [
            "tmux",
            "-f",
            "/dev/null",
            "-L",
            agent.socket.as_str(),
            "attach-session",
            "-t",
            session.as_str(),
        ];
        watcher
            .server
            .new_session(&Spawn {
                command: &attach,
                ..Spawn::default()
            })
            .unwrap();

        until("somebody to attach to the agent", || {
            watched(Some(&inside), Some(pane.as_str()))
        });
    }

    #[test]
    fn notify_an_errand_is_handed_the_event_and_left_to_run() {
        // The command cannot finish until the test creates `go`, which it only
        // does after `start` returns, so `start` must not wait for it.
        let dir = TempDir::new().unwrap();
        let errand = Errand {
            command: "{ cat; until [ -e go ]; do sleep 0.02; done; \
                      echo \"$AMX_ID $AMX_STATE $AMX_WATCHED\"; } > said"
                .to_string(),
            dir: dir.path().to_path_buf(),
            env: vec![
                ("AMX_ID".to_string(), "fix-login-a1b".to_string()),
                ("AMX_STATE".to_string(), "waiting".to_string()),
            ],
            stdin: b"{\"kind\":\"Notification\"}\n".to_vec(),
        };

        start(&errand, Some(true));
        std::fs::write(dir.path().join("go"), "").unwrap();

        let said = dir.path().join("said");
        until("the errand to say what it was handed", || {
            std::fs::read_to_string(&said).is_ok_and(|said| said.contains("fix-login-a1b"))
        });
        assert_eq!(
            std::fs::read_to_string(&said).unwrap(),
            "{\"kind\":\"Notification\"}\nfix-login-a1b waiting 1\n",
            "the event arrives whole, and the stdin handle is let go after it"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn notify_an_errand_that_has_exited_is_reaped() {
        let dir = TempDir::new().unwrap();
        let errand = Errand {
            command: "cat > /dev/null; echo $$ > pid.new; mv pid.new pid".to_string(),
            dir: dir.path().to_path_buf(),
            env: Vec::new(),
            stdin: b"{}\n".to_vec(),
        };

        start(&errand, None);

        let pid = dir.path().join("pid");
        until("the errand to say its pid", || pid.exists());
        let pid = std::fs::read_to_string(&pid).unwrap();
        let proc = PathBuf::from(format!("/proc/{}", pid.trim()));
        until("the errand to be waited for", || !proc.exists());
    }

    #[test]
    fn notify_an_errand_off_the_hook_path_is_told_nothing_about_the_pane() {
        // `stop` asks tmux nothing about the pane, so the variable is unset
        // instead of `0`.
        let dir = TempDir::new().unwrap();
        let errand = Errand {
            command: "cat > /dev/null; echo \"[${AMX_WATCHED-unset}]\" > said".to_string(),
            dir: dir.path().to_path_buf(),
            env: Vec::new(),
            stdin: b"{}\n".to_vec(),
        };

        start(&errand, None);

        let said = dir.path().join("said");
        until("the errand to say what it was told", || said.exists());
        until("the errand to finish its line", || {
            std::fs::read_to_string(&said).is_ok_and(|said| said.contains(']'))
        });
        assert_eq!(std::fs::read_to_string(&said).unwrap(), "[unset]\n");
    }

    #[test]
    fn notify_an_osc_notice_is_one_escape_and_nothing_the_text_can_add_to() {
        let notice = Notice::waiting("fix-login-a1b", Some("Run the migration?"));
        assert_eq!(
            osc_notice(&notice),
            b"\x1b]777;notify;fix-login-a1b needs an answer;Run the migration?\x07".to_vec()
        );

        // Agent text inside an escape sequence must not end it or start
        // another.
        let notice = Notice {
            title: "a;b\u{1b}]0;stolen\u{7}".to_string(),
            body: "line\nand\u{7}more\u{7f}".to_string(),
        };
        assert_eq!(
            osc_notice(&notice),
            b"\x1b]777;notify;a,b]0,stolen;lineandmore\x07".to_vec(),
            "the semicolons of the title are the fields, and the controls go"
        );

        // C1 controls are two bytes in UTF-8, and a terminal reading C1 takes
        // U+009D as OSC and U+009C as its terminator.
        let notice = Notice {
            title: "a\u{9d}0;stolen\u{9c}b".to_string(),
            body: "c\u{9b}31md\u{85}e".to_string(),
        };
        assert_eq!(
            osc_notice(&notice),
            b"\x1b]777;notify;a0,stolenb;c31mde\x07".to_vec(),
            "the C1 controls go with the C0 ones"
        );

        // Everything else is left as written.
        let notice = Notice {
            title: "héllo".to_string(),
            body: "naïve".to_string(),
        };
        assert_eq!(
            osc_notice(&notice),
            "\u{1b}]777;notify;héllo;naïve\u{7}".as_bytes()
        );
    }

    #[test]
    fn notify_a_terminal_is_written_to_once_and_an_error_is_silence() {
        let dir = TempDir::new().unwrap();
        let tty = dir.path().join("tty");
        std::fs::write(&tty, "").unwrap();

        tell(&tty, b"\x1b]777;notify;one;two\x07");
        assert_eq!(std::fs::read(&tty).unwrap(), b"\x1b]777;notify;one;two\x07");

        // A tty gone since it was listed is ignored, and no file is created.
        let gone = dir.path().join("gone");
        tell(&gone, b"x");
        assert!(!gone.exists());
    }

    #[test]
    fn notify_a_terminal_two_servers_list_is_named_once() {
        let agent = TestServer::new();
        let (session, _) = agent.server.new_session(&idle()).unwrap();

        let watcher = TestServer::new();
        let attach = [
            "tmux",
            "-f",
            "/dev/null",
            "-L",
            agent.socket.as_str(),
            "attach-session",
            "-t",
            session.as_str(),
        ];
        watcher
            .server
            .new_session(&Spawn {
                command: &attach,
                ..Spawn::default()
            })
            .unwrap();
        until("somebody to attach to the agent", || {
            !agent.server.client_ttys().is_empty()
        });

        let told = terminals(vec![agent.server.clone(), agent.server.clone()]);
        assert_eq!(
            told,
            agent.server.client_ttys(),
            "one terminal, however many servers are listing it"
        );
    }

    #[test]
    fn hook_notices_say_who_and_what() {
        let waiting = Notice::waiting("fix-login-a1b", Some("Run the migration?"));
        assert!(waiting.title.contains("fix-login-a1b"));
        assert_eq!(waiting.body, "Run the migration?");

        let unasked = Notice::waiting("fix-login-a1b", None);
        assert!(!unasked.body.is_empty(), "there is always something to say");

        let done = Notice::finished("fix-login-a1b", Phase::Done, Some(0)).unwrap();
        assert!(done.title.contains("fix-login-a1b"), "{}", done.title);

        let failed = Notice::finished("fix-login-a1b", Phase::Failed, Some(2)).unwrap();
        assert!(failed.title.contains('2'), "the code is the useful part");
    }

    #[test]
    fn hook_notices_are_not_posted_for_what_a_person_already_knows() {
        // Stopping is the person's own act, and an idle turn is shown on the
        // wall.
        assert_eq!(
            Notice::finished("fix-login-a1b", Phase::Stopped, None),
            None
        );
        assert_eq!(Notice::finished("fix-login-a1b", Phase::Idle, None), None);
        assert_eq!(
            Notice::finished("fix-login-a1b", Phase::Working, None),
            None
        );
    }

    #[test]
    fn hook_notices_go_out_as_arguments_and_never_as_script() {
        // Agent text must reach the notifier as one argument.
        let notice = Notice::waiting("fix-login-a1b", Some("$(rm -rf ~); \"quoted\""));
        let command = notifier(&notice).unwrap();
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();

        if cfg!(target_os = "macos") {
            assert!(args[1].contains("\\\"quoted\\\""), "{args:?}");
        } else {
            assert!(args.contains(&"--".to_string()), "{args:?}");
            assert_eq!(args.last().unwrap(), "$(rm -rf ~); \"quoted\"");
        }
    }

    #[test]
    fn hook_notices_quote_for_applescript() {
        assert_eq!(applescript("plain"), "plain");
        assert_eq!(applescript("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(applescript("back\\slash"), "back\\\\slash");
    }
}
