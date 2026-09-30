//! One module per verb, plus the output helpers several verbs share.

pub mod adopt;
pub mod allow;
pub mod answer;
pub mod attach;
pub mod clear;
pub mod diff;
pub mod doctor;
pub mod events;
pub mod fork;
pub mod interrupt;
pub mod logs;
pub mod ls;
pub mod new;
pub mod park;
pub mod rename;
pub mod result;
pub mod resume;
pub mod send;
pub mod setup;
pub mod status;
pub mod statusline;
pub mod stop;
pub mod sub;
pub mod sweep;
pub mod uninstall;
pub mod wait;

/// Print a question and its numbered choices, one per line.
pub(crate) fn print_question(
    question: &str,
    options: &[String],
    to_terminal: bool,
    out: &mut impl std::io::Write,
) -> anyhow::Result<()> {
    send::line(&send::rendered(question, to_terminal), out)?;
    for choice in send::numbered(options) {
        send::line(&send::rendered(&choice, to_terminal), out)?;
    }
    Ok(())
}

/// The first line of `text`, trimmed and sanitized so it cannot drive a terminal.
pub(crate) fn inert_line(text: &str) -> String {
    crate::tmux::sanitize(text.lines().next().unwrap_or(""))
        .trim()
        .to_string()
}
