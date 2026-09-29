//! Mapping what a thing means to the style it is painted in.
//!
//! This is the only place a role becomes a colour: each function takes the
//! screen's [`Theme`] and answers for one kind of thing, so the bands never
//! pick a colour themselves.

use ratatui::style::{Modifier, Style};

use crate::pr::Standing;
use crate::store::Phase;
use crate::theme::Theme;

/// The style of an agent's name on its row.
///
/// Only waiting and failed names take a colour; the rest of the states are
/// already said by the glyph. `lent` (the row the terminal was last lent to)
/// takes the accent, but never over those two colours. Names are dim unless
/// `bright`, which marks the row under the cursor or the pointer; the wall
/// uses no bold.
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

/// The colour a state is said in.
///
/// The glyph says whether a process is running, so the colour says how it
/// went: a live agent keeps the terminal's own colour until it ends.
pub(super) fn colour(theme: Theme, phase: Phase) -> Style {
    match phase {
        Phase::Waiting => Style::new().fg(theme.waiting),
        // An agent amx has lost track of is not asking anything, so it does
        // not get the waiting colour.
        Phase::Starting | Phase::Working | Phase::Unknown => Style::new(),
        // Idle and done both mean the turn is over.
        Phase::Idle | Phase::Done => Style::new().fg(theme.done),
        Phase::Failed => Style::new().fg(theme.failed),
        Phase::Stopped => Style::new().fg(theme.stopped),
    }
}

/// The colour a pull request's standing is said in, on the same roles as a
/// state. Running checks and an unreviewed request both keep the terminal's
/// own colour; the card says which in words.
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

/// The style of something that applies to the next agent and has not happened
/// yet: the accent, in bold so it still stands out without colour.
pub(super) fn prospective(theme: Theme) -> Style {
    Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::default()
    }

    /// Every standing, so a table over them cannot miss one.
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
        // Only two states colour a name; strength alone marks the row being
        // worked with.
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
        // The lent-to accent only goes on states that have no colour.
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
        // Eight standings, eight distinct words; the five colours are shared.
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
