//! What a wall with nothing on it says for itself.

use ratatui::text::Line;

use super::style::dim;
use crate::tui::rows::List;

/// What a wall nobody has put anything on says for itself.
///
/// One line of amx's own, where four headings with a sentence each used to
/// stand: there is nothing to read off the rows, and a view that explains the
/// list before there is a list is doing the manual's job on the screen a
/// person came to work at. What is worth knowing about this wall is that it is
/// the good one, and the keys under it already say which one starts an agent.
pub(super) const WELCOME: &str = "nothing running, nothing broken, nobody asking. enjoy it";

/// The one line a list holding nothing is drawn as.
///
/// Nothing to show is one thing while a narrowing is holding every agent back,
/// another while nobody has started one, and the line for the second of those
/// is a sentence rather than a label — so it is said whole or not at all, and a
/// screen too narrow for it gets the label instead of two thirds of a joke.
pub(super) fn nothing(list: &List, width: usize) -> Line<'static> {
    let room = width >= WELCOME.chars().count();
    let said = match (list.narrowing(), list.unstarted() && room) {
        (Some(narrowing), _) => format!("nothing matches {narrowing}"),
        (None, true) => WELCOME.to_string(),
        (None, false) => "no agents".to_string(),
    };
    Line::styled(said, dim())
}
