//! One module per verb.

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

/// Print a question and its numbered choices where an answer would have gone.
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
