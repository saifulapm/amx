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

use std::process::{Command, Stdio};

use crate::store::Phase;

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

/// Post a notice, if this machine has anywhere to post it.
pub fn post(notice: &Notice) {
    let Some(mut command) = notifier(notice) else {
        return;
    };
    // Started and not waited for: the hook has an agent waiting on it.
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
