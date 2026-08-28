//! What a wall with nothing on it says for itself.

use ratatui::text::{Line, Span};

use super::help::HELP;
use super::style::{bold, dim};
use crate::tui::grid;
use crate::tui::rows::List;

/// What a wall nobody has put anything on says for itself.
///
/// One line of amx's own, where four headings with a sentence each used to
/// stand: there is nothing to read off the rows, and a view that explains the
/// list before there is a list is doing the manual's job on the screen a
/// person came to work at. What is worth knowing about this wall is that it is
/// the good one.
pub(super) const WELCOME: &str = "nothing running, nothing broken, nobody asking. enjoy it";

/// What stands before an agent's name on a row: the two marks, the state glyph
/// and the space after it. The offers stand where a name would, so the empty
/// wall is the wall with its rows taken out rather than a screen of its own.
const NAME: usize = 4;

/// The key column those offers stand in, which is one key and the air that
/// holds what it does off it.
const KEY: usize = 4;

/// The rows a list holding nothing is drawn as.
///
/// Nothing to show is one thing while a narrowing is holding every agent back,
/// another while nobody has started one. The first is answered in the words
/// somebody typed and nothing else: they narrowed the wall themselves, and
/// they have agents the narrowing is holding back. The second is the sentence,
/// and under it the two keys that do something about it.
///
/// The sentence is said whole or not at all — a screen too narrow for it gets
/// the label instead of two thirds of a joke — but the two keys stand either
/// way, because they are the only thing on the screen that leads anywhere.
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
        // Every key but the one already offered above it.
        offer("?", format!("the other {} keys", HELP.len() - 1)),
    ]
}

/// A key and what pressing it would do, in the column a row's name stands in.
///
/// Two rather than the whole table, because a person looking at an empty wall
/// has one thing to decide, and the keys that arrange, page and stop things
/// have nothing to work on yet.
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
    use crate::derive::{Evidence, Verdict, View};
    use crate::store::{Meta, Phase, State};
    use crate::tmux::{PaneId, Socket};
    use crate::tui::Screen;
    use crate::tui::paint::{Card, draw};
    use crate::tui::rows::{Group, Narrow};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Modifier;
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
        line.split('┈').next().unwrap_or_default().trim()
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
    fn a_wall_nobody_has_put_anything_on_says_so_and_offers_the_two_keys_that_answer_it() {
        let screen = drawn(Vec::new(), None, WALL);

        // One line where the four groups used to have a sentence each, a row
        // of air, and the two keys that lead anywhere from here, standing
        // where a row's name would stand.
        assert_eq!(screen[3], WELCOME, "{screen:?}");
        assert_eq!(screen[4], "", "{screen:?}");
        assert_eq!(screen[5], "    n   start an agent", "{screen:?}");
        assert_eq!(
            screen[6],
            format!("    ?   the other {} keys", HELP.len() - 1),
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

        let key = buffer[(4, 5)].clone();
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
        let narrowed = painted(&screen, WALL);
        assert_eq!(narrowed[3], "nothing matches a:nobody");
        assert!(
            !narrowed.iter().any(|line| line.contains("start an agent")),
            "somebody who narrowed the wall themselves has agents already: {narrowed:?}"
        );

        // And the project axis is a list of places, which nobody arrives at
        // with nothing to arrange.
        let mut screen = showing(Vec::new(), None);
        screen.list.turn();
        assert_eq!(painted(&screen, WALL)[3], "no agents");
    }
}
