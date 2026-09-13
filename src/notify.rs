//! Telling the person when an agent needs them.
//!
//! Two moments are worth interrupting somebody for: an agent that has stopped
//! on a question, and one that has finished. Everything else is on the wall
//! and in `ls`.
//!
//! A notice goes by the roads the `notifications` key names: the desktop's own
//! notifier, the terminals the person is sitting at, both of them or neither.
//! The second road is for a person over SSH, who has a terminal there and a
//! desktop somewhere else entirely.
//!
//! Posting is best effort by design. The hook path is measured in fractions of
//! a millisecond and runs while an agent waits on it, so a desktop with no
//! notifier — or one that is slow to answer — costs nothing: the notifier is
//! started and never waited for, and any failure is silence.
//!
//! One notice is not worth posting at all: the one about a pane its person is
//! already looking at. Who is looking is a question for tmux, and the hook
//! makes no tmux calls, so the notifier forks away from the hook first and
//! asks on its own time.
//!
//! That fork is also what starts an [`Errand`] — the command somebody asked to
//! have run when an agent reaches a moment. The two go together because they
//! want the same things: to be off the hook path, to know whether anybody was
//! looking, and to be left alone once started.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::Delivery;
use crate::store::Phase;
use crate::tmux::{PaneId, Server};

/// What tmux writes into the environment of every pane it makes: the server,
/// and which pane this is. An agent's hook runs inside the agent's own pane,
/// so these two name the pane a notice is about, and reading them is not a
/// tmux call.
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

    /// An agent's command has finished, well or badly.
    pub fn finished(id: &str, phase: Phase, exit: Option<i32>) -> Option<Notice> {
        let body = match (phase, exit) {
            (Phase::Done, _) => "finished".to_string(),
            (Phase::Failed, Some(code)) => format!("failed, exit {code}"),
            (Phase::Failed, None) => "failed".to_string(),
            // Nothing else is worth an interruption: a person who stopped an
            // agent knows it stopped.
            _ => return None,
        };
        Some(Notice {
            title: format!("{id} {body}"),
            body: body.to_string(),
        })
    }
}

/// A command a moment is worth running, ready to start — see
/// [`crate::errand`], which is what decides there is one and fills this in.
///
/// It travels this far rather than being read here because the fork that
/// starts it is the fork the notice already pays for: the hook gets one child
/// for both, and everything it needs was worked out on the near side of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Errand {
    /// The command line, as the config file holds it, for `sh -c`.
    pub command: String,
    /// Where it runs: the agent's tree, or the directory it was started in.
    pub dir: PathBuf,
    /// What it is told, beyond whatever this process inherited.
    pub env: Vec<(String, String)>,
    /// The event that moved the agent, as one JSON line.
    pub stdin: Vec<u8>,
}

/// Tell the person what happened, and run what they asked to have run.
///
/// The deciding is a child's work. Whether somebody is already looking at the
/// agent's pane is a question for tmux, and this runs on the hook path, where
/// the pane being asked about is the one waiting for the hook to return. So
/// the notifier forks away first and the hook comes straight back.
///
/// Asked once on the far side of the fork and spent on both: a notice is not
/// posted about a screen its person is looking at, and an errand is told what
/// the answer was rather than asking again.
///
/// A machine that cannot fork does the same inline. Whoever called this is
/// then waiting on somebody's desktop, which is the cost of the fork not being
/// there; one notification too many is the cheap way to be wrong.
pub fn post(notice: Option<&Notice>, delivery: Delivery, errand: Option<&Errand>) {
    // A notice nothing will deliver is not a reason to fork, and neither is a
    // moment nobody wrote a command for.
    let notice = notice.filter(|_| delivery.tells());
    if notice.is_none() && errand.is_none() {
        return;
    }

    match detach() {
        Fork::Hook => (),
        Fork::Notifier => {
            deliver(notice, delivery, errand);
            // This process is a copy of the hook, and the hook's work is
            // already done. Leaving by any other door would do it twice.
            unsafe { nix::libc::_exit(crate::exit::OK) };
        }
        Fork::Neither => deliver(notice, delivery, errand),
    }
}

/// Both roads, in the one process that has time for them.
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

/// Run an errand and leave it to it.
///
/// Through `sh`, because the key holds a command line. The event goes in on
/// stdin whole: it is the vendor's own JSON and an argv is the one place it
/// could be read as syntax. What it says goes nowhere — somebody who wants a
/// log of it redirects in the command they wrote.
///
/// Nothing waits on it. The line is a few hundred bytes against a pipe that
/// holds pages of them, so writing it cannot block, and the handle goes at the
/// end of this, which is what tells the command the line is all of it.
///
/// `watched` is what the pane was doing when the moment arrived, where
/// anybody asked. `None` from a caller with no pane to ask about leaves the
/// variable off rather than guessing at it.
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

    // A command that is not there, or a fork this machine cannot spare, is
    // silence: this is somebody's errand, not the agent's work.
    let Ok(mut child) = command.spawn() else {
        return;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&errand.stdin);
    }
}

/// Which side of the fork a process is on.
enum Fork {
    /// The hook, whose part in this is over.
    Hook,
    /// The notifier, on its own from here.
    Notifier,
    /// Neither: this machine could not fork, and the caller is still the hook.
    Neither,
}

/// Put a notifier behind the hook and come straight back.
///
/// The child gives up two things it inherited, and both of them matter:
///
/// * **The hook's stdio.** The vendor reads what a hook writes, and a reader
///   waiting for end of input waits for every process holding the pipe. A
///   notifier still holding it would hand the agent back the delay this fork
///   exists to take away — measured at about 2ms of it.
/// * **The pane's session.** Stopping an agent signals the pane's process
///   group, and telling somebody about the agent is not part of the agent.
fn detach() -> Fork {
    // SAFETY: the hook is single threaded, and between this fork and the
    // command it starts the child reads its own environment and nothing else.
    match unsafe { nix::unistd::fork() } {
        Ok(nix::unistd::ForkResult::Parent { .. }) => Fork::Hook,
        Ok(nix::unistd::ForkResult::Child) => {
            hand_back_stdio();
            let _ = nix::unistd::setsid();
            Fork::Notifier
        }
        Err(_) => Fork::Neither,
    }
}

/// Point this process's three standard streams at nothing, so whoever is
/// reading them sees the end of them.
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

/// Whether the notice would be telling somebody what is already on their
/// screen.
fn watched(server: Option<&str>, pane: Option<&str>) -> bool {
    pane_here(server, pane).is_some_and(|(server, pane)| server.pane_watched(&pane))
}

/// The pane this process is running in, as tmux named it in the environment.
///
/// Outside tmux there is no pane, and a pane nobody can name is a pane nobody
/// is looking at.
fn pane_here(server: Option<&str>, pane: Option<&str>) -> Option<(Server, PaneId)> {
    Some((Server::from_tmux_env(server?)?, PaneId::new(pane?).ok()?))
}

/// One variable of the environment, empty read as absent.
fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Start the notifier and do not wait for it: a desktop that is slow to
/// answer, or that answers with an error, is nobody's business here.
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

/// How this machine posts a notification.
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

/// A string as AppleScript will read it.
fn applescript(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Write the notice to every terminal the person is sitting at.
///
/// A desktop is not always there to post to: somebody working over SSH has a
/// terminal and nothing behind it, and the notifier on the machine amx is
/// running on would be raising notices on a screen nobody is at. What that
/// person does have is a tmux around the view, and a tmux client is a terminal
/// amx can write to.
///
/// Every client of every server of theirs, because the view is on one server
/// and the agents may be on another, and each terminal once: two servers
/// listing the same one is one person, who does not want the sentence twice.
fn tell_terminals(notice: &Notice) {
    let bytes = osc_notice(notice);
    for tty in terminals(crate::tmux::servers_here()) {
        tell(&tty, &bytes);
    }
}

/// The terminals of everybody sitting at one of these servers, each named once.
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

/// The notice as the escape sequence a terminal reads as a notification: OSC
/// 777, which is what tmux, foot, wezterm and the rest took from urxvt.
///
/// Both fields are made inert first. The title and the body are an agent's own
/// text — a question it printed, a command line somebody wrote — and they are
/// travelling inside an escape sequence, where a stray escape would start
/// another and a stray bell would end this one early. Every C0 control and the
/// delete go; the semicolon of the title becomes a comma, because it is what
/// tells the title from the body.
pub fn osc_notice(notice: &Notice) -> Vec<u8> {
    let mut bytes = b"\x1b]777;notify;".to_vec();
    bytes.extend(inert(&notice.title.replace(';', ",")));
    bytes.push(b';');
    bytes.extend(inert(&notice.body));
    bytes.push(0x07);
    bytes
}

/// One field of that sequence: the bytes a terminal would read as instruction
/// taken out, and whatever the text is spelled in left alone.
fn inert(text: &str) -> Vec<u8> {
    text.bytes()
        .filter(|byte| *byte >= 0x20 && *byte != 0x7f)
        .collect()
}

/// Write the sequence to one terminal, and say nothing about it either way.
///
/// Write only, because nothing here reads what the person is typing. No
/// controlling terminal, because the notifier is a session leader by then and
/// opening a tty would otherwise hand it one. Non-blocking, because a terminal
/// whose reader has stopped must not hold the notifier open on it.
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

    /// A private tmux server that goes when the test does.
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
            // An empty conf, so nothing in the developer's ~/.tmux.conf can
            // change what these tests measure.
            let server = Server::named(&socket).with_conf("/dev/null");
            Self { socket, server }
        }

        /// `$TMUX` as tmux writes it into a pane of this server: the socket's
        /// path, then two fields nothing here reads.
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

    /// A shell that sits there without exiting, so a pane stays a pane.
    fn idle() -> Spawn<'static> {
        Spawn {
            command: &["sh", "-c", "while :; do sleep 0.05; done"],
            ..Spawn::default()
        }
    }

    /// Poll until `f` is happy: no fixed sleep stands in for a state change.
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
        // tmux writes both of these into every pane it makes, and an agent's
        // hook runs inside the agent's own pane. Reading them is why the hook
        // can pose the question at all: it costs no tmux call.
        let (server, pane) = pane_here(Some("/tmp/tmux-test/amx,4242,0"), Some("%7")).unwrap();
        assert_eq!(
            server.socket(),
            &Socket::Path(PathBuf::from("/tmp/tmux-test/amx"))
        );
        assert_eq!(pane.as_str(), "%7");

        // Nothing to suppress against, and nothing asked of tmux either: there
        // is no pane, so nobody is looking at one and the notice goes out.
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

        // A client on a terminal of its own, which is what `session_attached`
        // counts: a pane on another server, running a client of this one.
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
        // The starter does not wait for what it starts, and this command
        // cannot finish until the test makes the file it is watching for —
        // which the test only reaches once `start` has returned to it.
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

    #[test]
    fn notify_an_errand_off_the_hook_path_is_told_nothing_about_the_pane() {
        // `stop` runs in a terminal of its own and asks tmux nothing about the
        // agent's pane, so the variable is absent rather than answered `0`.
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

        // A question is the agent's own text, and it travels inside an escape
        // sequence: nothing in it may end that sequence or start another.
        let notice = Notice {
            title: "a;b\u{1b}]0;stolen\u{7}".to_string(),
            body: "line\nand\u{7}more\u{7f}".to_string(),
        };
        assert_eq!(
            osc_notice(&notice),
            b"\x1b]777;notify;a,b]0,stolen;lineandmore\x07".to_vec(),
            "the semicolons of the title are the fields, and the controls go"
        );

        // What is not a control character is left as it was written.
        let notice = Notice {
            title: "héllo".to_string(),
            body: "naïve".to_string(),
        };
        assert_eq!(
            osc_notice(&notice),
            format!("\u{1b}]777;notify;héllo;naïve\u{7}").into_bytes()
        );
    }

    #[test]
    fn notify_a_terminal_is_written_to_once_and_an_error_is_silence() {
        let dir = TempDir::new().unwrap();
        let tty = dir.path().join("tty");
        std::fs::write(&tty, "").unwrap();

        tell(&tty, b"\x1b]777;notify;one;two\x07");
        assert_eq!(std::fs::read(&tty).unwrap(), b"\x1b]777;notify;one;two\x07");

        // A terminal that has gone since it was listed is nothing to report
        // and nothing to create in its place.
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
        // Somebody who stopped an agent does not need telling that it stopped,
        // and a turn ending is what the wall is for.
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
        // A question is the agent's text; it must not be able to end the
        // command line it travels on.
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
