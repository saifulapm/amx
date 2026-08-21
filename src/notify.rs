//! Telling the person when an agent needs them.
//!
//! Two moments are worth interrupting somebody for: an agent that has stopped
//! on a question, and one that has finished. Everything else is on the wall
//! and in `ls`.
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

use std::process::{Command, Stdio};

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

/// Post a notice, if this machine has anywhere to post it and anybody still
/// needs telling.
///
/// The deciding is a child's work. Whether somebody is already looking at the
/// agent's pane is a question for tmux, and this runs on the hook path, where
/// the pane being asked about is the one waiting for the hook to return. So
/// the notifier forks away first and the hook comes straight back.
///
/// A machine that cannot fork posts without asking: one notification too many
/// is the cheap way to be wrong.
pub fn post(notice: &Notice) {
    match detach() {
        Fork::Hook => (),
        Fork::Notifier => {
            if !watched(var(SERVER_ENV).as_deref(), var(PANE_ENV).as_deref()) {
                raise(notice);
            }
            // This process is a copy of the hook, and the hook's work is
            // already done. Leaving by any other door would do it twice.
            unsafe { nix::libc::_exit(crate::exit::OK) };
        }
        Fork::Neither => raise(notice),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::{Socket, Spawn};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

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
