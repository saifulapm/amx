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

/// What a row's name is painted in, and at what strength: the colour of a
/// thing waiting on a person where that is what the row is, the colour of a
/// failure where the work ended in one, the accent on the row the terminal was
/// lent to, the terminal's own everywhere else — and dim unless `bright`, which
/// is the row under the cursor or the row under the pointer.
///
/// Two states out of eight take a colour, because a column of names in eight
/// colours is a column nobody reads. Those two are the ones a person scanning
/// the wall is looking for, and the rest have said all they have to say on the
/// glyph. So the accent goes on top of the states that said nothing and never
/// over the two that did: what an agent wants is worth more than where the
/// terminal has been, and a name cannot say both.
///
/// The wall spends no weight at all, so the one thing a name has left to say
/// about the person reading it is strength: every row is as quiet as the
/// summary beside it but the one being worked with, and that one comes up in
/// whatever colour it already had. Which is why `bright` is a strength rather
/// than a colour of its own — a row does not stop saying what it is to say
/// where the cursor is.
pub(super) fn name_colour(theme: Theme, phase: Phase, bright: bool, lent: bool) -> Style {
    let paint = match phase {
        Phase::Waiting => Style::new().fg(theme.waiting),
        Phase::Failed => Style::new().fg(theme.failed),
        _ if lent => Style::new().fg(theme.accent),
        _ => Style::new(),
    };
    match bright {
        true => paint,
        false => paint.add_modifier(Modifier::DIM),
    }
}

/// What a state is worth saying in colour.
///
/// Whether anything is running is the mark's job, which leaves the colour to
/// carry how it went: an agent still at work has nothing to say about that
/// yet, so it takes the terminal's own colour and earns one by ending.
pub(super) fn colour(theme: Theme, phase: Phase) -> Style {
    match phase {
        Phase::Waiting => Style::new().fg(theme.waiting),
        // What amx cannot account for is not a question, and there is one
        // shape for both: an agent it has lost track of, painted for a thing
        // waiting on a person, would be a row that says it is asking something
        // amx has no idea about. It stands still in the terminal's own, which
        // is what tells it from every other live row.
        Phase::Starting | Phase::Working | Phase::Unknown => Style::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The palette these colours are read out of: what the tests are about is
    /// which role a thing is painted in, and the values are the theme's
    /// business.
    fn theme() -> Theme {
        Theme::default()
    }

    /// Every standing there is, so a table over them cannot quietly miss one.
    const EVERY_STANDING: [Standing; 8] = [
        Standing::Merged,
        Standing::Closed,
        Standing::Draft,
        Standing::Failing,
        Standing::Changes,
        Standing::Running,
        Standing::Ready,
        Standing::Open,
    ];

    #[test]
    fn rows_a_name_takes_two_colours_and_no_weight() {
        // The colour is the state's and only two states have one; what is left
        // to say which row a person is working with is strength, and it says
        // the same thing on every one of them.
        for phase in [Phase::Waiting, Phase::Failed] {
            assert!(
                name_colour(theme(), phase, false, false).fg.is_some(),
                "{phase} is a name worth finding down a column of them"
            );
        }
        assert_eq!(
            name_colour(theme(), Phase::Waiting, false, false).fg,
            Some(theme().waiting),
            "a question the cursor is not on is still a question"
        );
        for phase in [
            Phase::Starting,
            Phase::Working,
            Phase::Idle,
            Phase::Done,
            Phase::Stopped,
            Phase::Unknown,
        ] {
            assert_eq!(
                name_colour(theme(), phase, false, false).fg,
                None,
                "{phase} has said what it has to say on the glyph"
            );
        }
        for phase in [Phase::Waiting, Phase::Done, Phase::Failed] {
            for bright in [true, false] {
                assert!(
                    !name_colour(theme(), phase, bright, false)
                        .add_modifier
                        .contains(Modifier::BOLD),
                    "{phase} carries no weight either way"
                );
            }
            assert!(
                name_colour(theme(), phase, false, false)
                    .add_modifier
                    .contains(Modifier::DIM),
                "{phase} is as quiet as the summary beside it"
            );
            assert!(
                !name_colour(theme(), phase, true, false)
                    .add_modifier
                    .contains(Modifier::DIM),
                "{phase}, under the cursor or the pointer, comes up to the \
                 terminal's own strength"
            );
        }
    }

    #[test]
    fn rows_the_accent_marks_a_name_the_state_left_alone() {
        // Where the terminal has been is the third thing a name can say, and
        // the quietest of the three: it goes on the states that had nothing
        // to say for themselves.
        for phase in [
            Phase::Starting,
            Phase::Working,
            Phase::Idle,
            Phase::Done,
            Phase::Stopped,
            Phase::Unknown,
        ] {
            assert_eq!(
                name_colour(theme(), phase, false, true).fg,
                Some(theme().accent),
                "{phase} is the row the terminal came back from"
            );
        }
        for (phase, colour) in [
            (Phase::Waiting, theme().waiting),
            (Phase::Failed, theme().failed),
        ] {
            assert_eq!(
                name_colour(theme(), phase, false, true).fg,
                Some(colour),
                "{phase} says what it is before it says where somebody has been"
            );
        }
        assert_eq!(
            name_colour(theme(), Phase::Done, true, true).fg,
            Some(theme().accent),
            "and the mark survives the row coming up under the cursor"
        );
    }

    #[test]
    fn pr_every_standing_has_a_word_and_a_colour() {
        // Eight standings and eight words, so a card never says one thing for
        // two of them. The colours are five and are meant to be shared: they
        // answer how it is going, and two standings can have the same answer.
        let said: Vec<&str> = EVERY_STANDING.into_iter().map(Standing::says).collect();
        assert_eq!(
            said.iter().collect::<std::collections::BTreeSet<_>>().len(),
            EVERY_STANDING.len(),
            "{said:?}"
        );
        for standing in EVERY_STANDING {
            assert_eq!(
                request_colour(theme(), standing).bg,
                None,
                "{standing:?} is a word on a row, not a bar under one"
            );
        }
        assert_eq!(
            request_colour(theme(), Standing::Merged).fg,
            Some(theme().done)
        );
        assert_eq!(
            request_colour(theme(), Standing::Failing).fg,
            Some(theme().failed)
        );
        assert_eq!(
            request_colour(theme(), Standing::Changes).fg,
            Some(theme().waiting)
        );
        assert_eq!(
            request_colour(theme(), Standing::Closed).fg,
            Some(theme().stopped)
        );
        assert_eq!(
            request_colour(theme(), Standing::Open).fg,
            None,
            "a request nobody has read yet has nothing to say about how it went"
        );
    }
}
