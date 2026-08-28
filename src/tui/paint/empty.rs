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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::{Evidence, Verdict, View};
    use crate::store::{Meta, Phase, State};
    use crate::tmux::{PaneId, Socket};
    use crate::tui::Screen;
    use crate::tui::paint::{Card, draw};
    use crate::tui::rows::{Group, Narrow};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use std::path::PathBuf;

    fn view(id: &str, phase: Phase, said: Option<&str>, age: u64) -> View {
        View {
            meta: Meta {
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                dir: PathBuf::from("/srv/app"),
                worktree: None,
                branch: None,
                base: None,
                socket: Socket::Name("amx".to_string()),
                pane: PaneId::new("%1").unwrap(),
                bg: false,
                session: None,
                transcript: None,
                created: 1,
            },
            state: State {
                state: phase,
                summary: said.map(str::to_string),
                since: 1,
                last_event: 1,
                ..State::default()
            },
            verdict: Verdict {
                phase,
                evidence: Evidence::Hooks,
                rule: None,
                age,
                // The rows print the worked seconds; most of these tests only
                // care that a number is where the column is, so the helper
                // hands both clocks the same one.
                worked: age,
            },
        }
    }

    /// The view, with a reading in it. The card is read as it is planted,
    /// the way the view itself builds one.
    fn showing(views: Vec<View>, card: Option<Card>) -> Screen {
        let mut screen = Screen::default();
        screen.list.show(views);
        screen.card = card.map(Card::read);
        screen
    }

    /// What a view of this size draws, cell by cell.
    fn cells(screen: &Screen, size: (u16, u16)) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
        terminal.draw(|frame| draw(frame, screen)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// What a view of this size puts on the screen, line by line.
    fn painted(screen: &Screen, size: (u16, u16)) -> Vec<String> {
        let buffer = cells(screen, size);
        (0..size.1)
            .map(|row| {
                (0..size.0)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    /// What the view puts on a screen of this size, line by line.
    fn drawn(views: Vec<View>, card: Option<Card>, size: (u16, u16)) -> Vec<String> {
        painted(&showing(views, card), size)
    }

    /// What a heading line says in front of the rule that carries it out to
    /// the edge: the label, and how many failed under it where any did.
    fn heading_of(line: &str) -> &str {
        line.split('─').next().unwrap_or_default().trim()
    }

    /// A screen with room for the bands above and below the list, the space
    /// between the header and it, and a group or two under that.
    const WALL: (u16, u16) = (80, 12);

    #[test]
    fn axis_says_nothing_matches_rather_than_claiming_there_are_no_agents() {
        let mut screen = showing(vec![view("busy-a1b", Phase::Working, None, 3)], None);
        screen
            .list
            .narrow(vec![Narrow::Name(Some("nobody".to_string()))]);

        assert_eq!(painted(&screen, (60, 8))[1], "nothing matches a:nobody");
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
        assert_eq!(screen[1], "no agents");
    }

    #[test]
    fn a_wall_nobody_has_put_anything_on_says_so_in_one_line_of_its_own() {
        let screen = drawn(Vec::new(), None, WALL);

        assert_eq!(screen[3], WELCOME, "{screen:?}");
        // Everything under it down to the keys is the empty wall itself: one
        // line where the four groups used to have a sentence each.
        assert!(
            screen[4..screen.len() - 1].iter().all(String::is_empty),
            "one line, and no more: {screen:?}"
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
    fn the_wall_says_it_plainly_where_the_line_of_its_own_will_not_fit() {
        // Said whole or not at all: a sentence cut by the terminal reads as a
        // sentence that ends where the screen does, and this one is a joke as
        // well, which is worse to be handed two thirds of.
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
        assert_eq!(heading_of(&one[3]), "COMPLETED");
        assert!(
            !one.iter().any(|line| line.contains("nobody asking")),
            "one agent and there is something to read off the rows: {one:?}"
        );

        // A fleet somebody narrowed to nothing is not a fleet nobody started,
        // and the view owes them the words they typed rather than a joke.
        let mut screen = showing(Vec::new(), None);
        screen
            .list
            .narrow(vec![Narrow::Name(Some("nobody".to_string()))]);
        assert_eq!(painted(&screen, WALL)[3], "nothing matches a:nobody");

        // And the project axis is a list of places, which nobody arrives at
        // with nothing to arrange.
        let mut screen = showing(Vec::new(), None);
        screen.list.turn();
        assert_eq!(painted(&screen, WALL)[3], "no agents");
    }
}
