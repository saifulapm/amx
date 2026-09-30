//! The list band when there are no rows to draw.

use ratatui::text::{Line, Span};

use super::help::HELP;
use super::style::{bold, dim};
use crate::tui::grid;
use crate::tui::rows::List;

/// The line shown when no agent has been started yet.
pub(super) const WELCOME: &str = "no agents yet";

/// Columns before an agent's name on a row: indent, glyph and a space. The
/// offered keys start there.
const NAME: usize = 3;

/// Width of the key column, key and padding.
const KEY: usize = 4;

/// The rows drawn for an empty list.
///
/// A narrowing that matched nothing says so in the words it was typed with.
/// Otherwise the list shows [`WELCOME`] and the two keys that lead somewhere.
/// [`WELCOME`] is only for a fleet nobody has started, on the state axis, and
/// only when it fits whole; else the line is a plain "no agents".
pub(super) fn nothing(list: &List, width: usize) -> Vec<Line<'static>> {
    if let Some(narrowing) = list.narrowing() {
        return vec![Line::styled(format!("nothing matches {narrowing}"), dim())];
    }
    let room = width >= WELCOME.chars().count();
    let said = match list.unstarted() && room {
        true => WELCOME,
        false => "no agents",
    };
    vec![
        Line::styled(said, dim()),
        Line::raw(""),
        offer("n", "start an agent".to_string()),
        // Every key but `n`, which is offered above.
        offer("?", format!("the other {} keys", HELP.len() - 1)),
    ]
}

/// A key and what it does, starting in the name column.
fn offer(key: &str, does: String) -> Line<'static> {
    Line::from(vec![
        Span::raw(" ".repeat(NAME)),
        Span::styled(grid::pad(key, KEY), bold()),
        Span::styled(does, dim()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Phase;
    use crate::tui::paint::fixtures::{cells, drawn, heading_of, painted, showing, view};
    use crate::tui::rows::{Group, Narrow};
    use ratatui::style::Modifier;

    /// Room for the header, the spacing rows and a group or two.
    const WALL: (u16, u16) = (80, 12);

    #[test]
    fn axis_says_nothing_matches_rather_than_claiming_there_are_no_agents() {
        let mut screen = showing(vec![view("busy-a1b", Phase::Working, None, 3)], None);
        screen
            .list
            .narrow(vec![Narrow::Name(Some("nobody".to_string()))]);

        assert_eq!(painted(&screen, (60, 8))[1], "nothing matches /nobody");
    }

    #[test]
    fn view_says_when_there_is_nothing_to_show() {
        let screen = drawn(Vec::new(), None, (40, 6));
        assert!(screen[0].starts_with("AMX"), "{:?}", screen[0]);
        assert!(
            screen[0].ends_with("0/5 running   nothing waiting"),
            "{:?}",
            screen[0]
        );
        assert_eq!(screen[1], WELCOME);
    }

    #[test]
    fn a_wall_nobody_has_put_anything_on_says_so_and_offers_the_two_keys_that_answer_it() {
        let screen = drawn(Vec::new(), None, WALL);

        // The welcome line, a blank row, and the two keys in the name column.
        assert_eq!(screen[3], WELCOME, "{screen:?}");
        assert_eq!(screen[4], "", "{screen:?}");
        assert_eq!(screen[5], "   n   start an agent", "{screen:?}");
        assert_eq!(
            screen[6],
            format!("   ?   the other {} keys", HELP.len() - 1),
            "{screen:?}"
        );
        assert!(
            screen[7..screen.len() - 1].iter().all(String::is_empty),
            "and nothing else: {screen:?}"
        );
        for group in Group::ALL {
            assert!(
                !screen.iter().any(|line| line.contains(group.title())),
                "{} stands over rows, and there are none: {screen:?}",
                group.title()
            );
        }
    }

    #[test]
    fn the_offers_on_an_empty_wall_carry_the_weight_on_the_key() {
        let buffer = cells(&showing(Vec::new(), None), WALL);

        let key = buffer[(3, 5)].clone();
        assert_eq!(key.symbol(), "n");
        assert!(
            key.modifier.contains(Modifier::BOLD),
            "the key is what there is to press: {:?}",
            key.modifier
        );
        let does = buffer[(8, 5)].clone();
        assert!(
            does.modifier.contains(Modifier::DIM),
            "and what it would do stands behind it: {:?}",
            does.modifier
        );
    }

    #[test]
    fn the_wall_says_it_plainly_where_the_line_of_its_own_will_not_fit() {
        // The welcome line is shown whole or not at all.
        let narrow = drawn(
            Vec::new(),
            None,
            (WELCOME.chars().count() as u16 - 1, WALL.1),
        );
        assert_eq!(narrow[3], "no agents");
        let wide = drawn(Vec::new(), None, (WELCOME.chars().count() as u16, WALL.1));
        assert_eq!(wide[3], WELCOME);
    }

    #[test]
    fn the_wall_has_its_line_to_itself_and_gives_it_up_to_the_first_row() {
        let one = drawn(
            vec![view("done-a1b", Phase::Done, Some("did it"), 60)],
            None,
            WALL,
        );
        assert_eq!(heading_of(&one[3]), "Completed");
        assert!(
            !one.iter().any(|line| line.contains("no agents yet")),
            "one agent and there is something to read off the rows: {one:?}"
        );

        // A narrowing that matched nothing is not an empty fleet.
        let mut screen = showing(Vec::new(), None);
        screen
            .list
            .narrow(vec![Narrow::Name(Some("nobody".to_string()))]);
        let narrowed = painted(&screen, WALL);
        assert_eq!(narrowed[3], "nothing matches /nobody");
        assert!(
            !narrowed.iter().any(|line| line.contains("start an agent")),
            "somebody who narrowed the wall themselves has agents already: {narrowed:?}"
        );

        // The project axis says the plain line.
        let mut screen = showing(Vec::new(), None);
        screen.list.turn();
        assert_eq!(painted(&screen, WALL)[3], "no agents");
    }
}
