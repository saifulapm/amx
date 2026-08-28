//! What a thing means, as the paint that says so.
//!
//! Every one of these takes the [`Theme`] the screen carries and answers one
//! question against it — how did this go, what is this row, what has not
//! happened yet. They are the only place a role becomes a colour, so a band
//! asks for what a thing means and never for a colour it picked itself.

use ratatui::style::{Modifier, Style};

use crate::pr::Standing;
use crate::store::Phase;
use crate::theme::Theme;

/// What a row's name is painted in: the colour of a thing waiting on a person,
/// and the weight to go with it, where that is what the row is; the colour of a
/// failure where the work ended in one; and the terminal's own everywhere else.
///
/// Two states out of eight, because a column of names in eight colours is a
/// column nobody reads. Those two are the ones a person scanning the wall is
/// looking for, and the rest have said all they have to say on the glyph.
pub(super) fn name_colour(theme: Theme, phase: Phase) -> Style {
    match phase {
        Phase::Waiting => Style::new().fg(theme.waiting).add_modifier(Modifier::BOLD),
        Phase::Failed => Style::new().fg(theme.failed),
        _ => Style::new(),
    }
}

/// What a state is worth saying in colour.
///
/// Whether anything is running is the mark's job, which leaves the colour to
/// carry how it went: an agent still at work has nothing to say about that
/// yet, so it takes the terminal's own colour and earns one by ending.
pub(super) fn colour(theme: Theme, phase: Phase) -> Style {
    match phase {
        // What amx cannot account for wants a person as much as a question
        // does, and the mark is what says which of the two it is.
        Phase::Waiting | Phase::Unknown => Style::new().fg(theme.waiting),
        Phase::Starting | Phase::Working => Style::new(),
        Phase::Idle => dim(),
        Phase::Done => Style::new().fg(theme.done),
        Phase::Failed => Style::new().fg(theme.failed),
        Phase::Stopped => Style::new().fg(theme.stopped),
    }
}

/// What a pull request's standing is worth saying in colour.
///
/// The same five roles the rest of the view is painted in, asked the same
/// question: how did it go. A merged request and an approved one went the way
/// they were meant to; a failing check was attempted and failed; a reviewer
/// asking for changes is a thing waiting on a person; a request that was shut
/// was ended by hand. Two of them take the terminal's own colour, because a
/// request whose checks are still running and one nobody has read yet have the
/// same answer to that question — nothing yet. Which of the two it is, is what
/// the card says in words.
pub(super) fn request_colour(theme: Theme, standing: Standing) -> Style {
    match standing {
        Standing::Merged | Standing::Ready => Style::new().fg(theme.done),
        Standing::Failing => Style::new().fg(theme.failed),
        Standing::Changes => Style::new().fg(theme.waiting),
        Standing::Closed => Style::new().fg(theme.stopped),
        Standing::Draft => dim(),
        Standing::Running | Standing::Open => Style::new(),
    }
}

pub(super) fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

pub(super) fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

/// What the next agent may do without asking, under the line that would start
/// it: the same accent every dial above the list wears, because it is one of
/// them, promoted to where somebody is about to press enter past it.
///
/// Weight as well as colour, which is what sets it apart from the dials it came
/// from and holds it apart on a terminal with the colour turned off: the row
/// has to read as amx's own answer for a spawn rather than as another line of
/// the composer it is under.
pub(super) fn prospective(theme: Theme) -> Style {
    Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
}
