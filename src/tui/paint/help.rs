//! The screen of keys, for whoever asked what they are.
//!
//! Not a band: it stands where the list stands, because a person who has asked
//! what the keys are is not reading the wall. The table itself is up in
//! [`super::HELP`]; what is here is how it is stood in columns on a terminal of
//! any shape, and what it gives up first when there is not room for all of it.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::HELP;
use super::style::dim;
use super::text::{fit, said};

/// What the keys are for, and how many of [`HELP`] each of those answers for.
///
/// A flat list of twenty-nine is a list somebody reads all of to find one, so
/// the table is cut into what a person is trying to do: get about the wall,
/// read one agent, put work in, arrange what is already there, and set what
/// the next agent runs. Five short lists are five places to not look.
///
/// Runs rather than tables of their own, so nothing here can hold a key twice
/// or drop one between two headings.
pub(super) const GROUPS: [(&str, usize); 5] = [
    ("walk", 5),
    ("look", 5),
    ("start", 5),
    ("arrange", 7),
    ("dials", 7),
];

/// Every key stands under exactly one heading.
const _: () = {
    let (mut under, mut at) = (0, 0);
    while at < GROUPS.len() {
        under += GROUPS[at].1;
        at += 1;
    }
    assert!(under == HELP.len());
};

/// How narrow a band may be before a screen has no room for another one beside
/// it: the widest key a column can hold, the air after it, and a character of
/// what it does.
///
/// A floor rather than a comfortable width, because of what the other end of
/// it costs. Short of a band the keys that would have gone in it are cut off
/// the bottom of the screen, and a key nobody can find is the one thing this
/// screen may not lose; a band this narrow is a key column with a stub against
/// it, and the key is the half somebody came here for.
const BAND: usize = 12;

/// How many bands the groups stand in wherever the width will take that many.
///
/// Two, because five short lists side by side is a wall of keys again and one
/// column of thirty-eight rows is the flat list the headings were put in to
/// break up. Two columns is what a page of keys looks like.
const COLUMNS: usize = 2;

/// One row of the overlay: a heading, a key with what it does, or the blank
/// that stands one group off from the next.
enum Told {
    Heading(String),
    Key(String, String),
    Air,
}

/// Every key and what it does, under the heading that says what it is for, in
/// bands read down and then across.
///
/// The height decides how many bands there are and the width decides how much
/// of each description survives, because what this screen is for is being
/// complete: a key cut off the bottom is one the view has and nobody can find,
/// where a description cut short still leaves its key where it can be read.
///
/// Down before across for the reason a list is a column. Somebody looking for
/// one key reads the heading, runs their eye down the keys under it and on to
/// the next; a table filled the other way would put the second key beside the
/// first and the rest of them anywhere at all.
pub(super) fn help(frame: &mut Frame, area: Rect) {
    let bands = bands(area);
    let share = (area.width as usize / bands.len().max(1)).max(1);
    let deep = bands.iter().map(Vec::len).max().unwrap_or(0);

    let lines: Vec<Line> = (0..deep.min(area.height as usize))
        .map(|at| {
            let mut spans = Vec::new();
            let mut column = 0;
            for (n, band) in bands.iter().enumerate() {
                // A band that has run out of keys, or is standing one group
                // off from the next, leaves the ones beside it where they
                // were: the columns are what the eye follows down.
                let told = match band.get(at) {
                    Some(Told::Heading(name)) => vec![Span::styled(name.clone(), dim())],
                    Some(Told::Key(key, does)) => vec![
                        Span::styled(key.clone(), Style::new().add_modifier(Modifier::BOLD)),
                        Span::styled(does.clone(), dim()),
                    ],
                    Some(Told::Air) | None => continue,
                };
                if n * share > column {
                    spans.push(Span::raw(" ".repeat(n * share - column)));
                }
                column = n * share + said(&told);
                spans.extend(told);
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// The keys as the bands they are drawn in: the groups dealt into as few bands
/// as the height needs and the width will take, each key padded to line up
/// under the one above it, and each description cut to what its own band was
/// given.
fn bands(area: Rect) -> Vec<Vec<Told>> {
    let width = area.width as usize;
    let height = (area.height as usize).max(1);
    // A heading and the keys under it, which is what a group costs a band.
    let depths: Vec<usize> = GROUPS.iter().map(|(_, under)| under + 1).collect();
    // Two bands wherever there is width for two, and another whenever the
    // groups will not stand in the rows the band has. A group is what the eye
    // follows down, so it is never cut in half to make the columns even.
    let most = (width / BAND).max(1);
    let mut count = COLUMNS.min(most);
    while count < most && deepest(&depths, count) > height {
        count += 1;
    }
    let share = (width / count).max(1);

    let bands = dealt(&depths, count);
    bands
        .iter()
        .enumerate()
        .map(|(n, groups)| {
            // The key column is worked out over the whole band, so every key
            // in it lines up under the one above.
            let column = groups
                .iter()
                .flat_map(|group| under(*group))
                .map(|(key, _)| key.chars().count())
                .max()
                .unwrap_or(0)
                + 1;
            // The last band takes whatever the division left over, and every
            // other one keeps a column of air between it and the next.
            let room = match n + 1 == bands.len() {
                true => width.saturating_sub(n * share + column),
                false => share.saturating_sub(column + 1),
            };
            let mut told = Vec::new();
            for group in groups {
                if !told.is_empty() {
                    told.push(Told::Air);
                }
                told.push(Told::Heading(fit(GROUPS[*group].0, column + room)));
                told.extend(
                    under(*group)
                        .map(|(key, does)| Told::Key(format!("{key:<column$}"), fit(does, room))),
                );
            }
            told
        })
        .collect()
}

/// The keys one group stands over, which is its run of [`HELP`].
fn under(group: usize) -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    let from: usize = GROUPS[..group].iter().map(|(_, under)| under).sum();
    HELP[from..from + GROUPS[group].1].iter()
}

/// The shallowest a band can be when these groups are dealt into `count` of
/// them, which is what says whether that many bands will fit the screen.
fn deepest(depths: &[usize], count: usize) -> usize {
    let most = depths.iter().sum::<usize>() + depths.len().saturating_sub(1);
    let least = depths.iter().copied().max().unwrap_or(0);
    (least..=most)
        .find(|deep| taken(depths, *deep) <= count)
        .unwrap_or(most)
}

/// How many bands the groups take when none of them may be deeper than this.
fn taken(depths: &[usize], deep: usize) -> usize {
    let (mut bands, mut used) = (1, 0);
    for &depth in depths {
        let wanted = match used {
            0 => depth,
            used => used + 1 + depth,
        };
        match wanted <= deep || used == 0 {
            true => used = wanted,
            false => {
                bands += 1;
                used = depth;
            }
        }
    }
    bands
}

/// Which groups each band holds, in order: as level as groups this size deal,
/// and never a group in two bands.
fn dealt(depths: &[usize], count: usize) -> Vec<Vec<usize>> {
    let deep = deepest(depths, count);
    let mut bands: Vec<Vec<usize>> = vec![Vec::new()];
    let mut used = 0;
    for (group, &depth) in depths.iter().enumerate() {
        let wanted = match used {
            0 => depth,
            used => used + 1 + depth,
        };
        match wanted <= deep || used == 0 {
            true => used = wanted,
            false => {
                bands.push(Vec::new());
                used = depth;
            }
        }
        bands.last_mut().expect("a band to deal into").push(group);
    }
    bands
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::View;
    use crate::tui::paint::header::{header_rows, space_rows};
    use crate::tui::paint::{Card, draw};
    use crate::tui::{Mode, Screen};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

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

    /// Which column of a drawn line a word starts at, for the tests that ask
    /// what the view painted it in. Columns, not bytes: the separator between
    /// two things said on one row is two bytes wide and one column.
    fn column_of(line: &str, word: &str) -> u16 {
        let at = line.find(word).expect("the word is on the line");
        line[..at].chars().count() as u16
    }

    /// The overlay on a screen this size, and the rows it was drawn on.
    fn overlay(size: (u16, u16)) -> Vec<String> {
        let mut screen = showing(Vec::new(), None);
        screen.mode = Mode::Keys;
        painted(&screen, size)
    }

    #[test]
    fn view_lists_every_key_when_somebody_asks_for_them() {
        let mut screen = showing(Vec::new(), None);
        screen.mode = Mode::Keys;

        // Tall and wide enough for every key and every heading over them,
        // so each of them has the row to itself and every description is
        // whole.
        let tall = (HELP.len() + GROUPS.len()) as u16 + header_rows(24) + space_rows(24) + 1;
        let painted = painted(&screen, (140, tall)).join("\n");
        for (key, does) in HELP {
            assert!(painted.contains(key), "{key} is missing:\n{painted}");
            assert!(painted.contains(does), "{does} is missing:\n{painted}");
        }
    }

    #[test]
    fn keymap_stands_the_keys_under_headings_that_say_what_they_are_for() {
        // A screen with room for the groups in two columns, which is the
        // shape they are laid out in wherever the width will take it.
        let painted = overlay((140, 38));

        // Down before across: the second key is under the first rather than
        // beside it, and the second column starts where the first one's share
        // of the width ends.
        assert!(painted[3].starts_with("walk"), "{:?}", painted[3]);
        assert!(painted[4].starts_with(HELP[0].0), "{:?}", painted[4]);
        assert_eq!(
            column_of(&painted[3], "arrange"),
            70,
            "and the next column stands beside the first: {:?}",
            painted[3]
        );

        // A heading over every run of keys, a blank row between two groups,
        // and the groups themselves whole rather than split down the fold.
        assert!(
            painted[9].chars().take(70).all(char::is_whitespace),
            "one group stands off from the next: {:?}",
            painted[9]
        );
        assert!(painted[10].starts_with("look"), "{:?}", painted[10]);
        assert!(painted[17].starts_with("start"), "{:?}", painted[17]);
        assert_eq!(column_of(&painted[12], "dials"), 70, "{:?}", painted[12]);

        let all = painted.join("\n");
        for (key, does) in HELP {
            assert!(key.len() < 12, "{key} is wider than a band's key column");
            assert!(all.contains(key), "{key} is missing:\n{all}");
            assert!(all.contains(does), "{does} is missing:\n{all}");
        }
    }

    #[test]
    fn keymap_headings_are_the_quietest_thing_on_the_screen_of_keys() {
        let mut screen = showing(Vec::new(), None);
        screen.mode = Mode::Keys;
        let buffer = cells(&screen, (140, 38));

        let heading = buffer[(0, 3)].clone();
        assert!(
            heading.modifier.contains(Modifier::DIM),
            "a heading stands over the keys and is not one of them: {:?}",
            heading.modifier
        );
        let key = buffer[(0, 4)].clone();
        assert!(
            key.modifier.contains(Modifier::BOLD),
            "the key itself is what somebody came here to find: {:?}",
            key.modifier
        );
    }

    #[test]
    fn keymap_takes_another_column_when_the_rows_will_not_hold_a_group() {
        // Two rows of header, one of space and one of keys leave eleven for
        // the overlay, which is fewer rows than two columns of groups need:
        // rather than cut a group in half or run one off the bottom, the
        // groups deal into as many columns as the height asks for.
        let painted = overlay((140, 15));
        let all = painted.join("\n");
        for (key, _) in HELP {
            assert!(all.contains(key), "{key} is missing:\n{all}");
        }
        assert!(painted[3].starts_with("walk"), "{:?}", painted[3]);
        assert_eq!(
            column_of(&painted[3], "dials"),
            4 * (140 / GROUPS.len() as u16),
            "a column each, in the order the table has them: {:?}",
            painted[3]
        );
    }

    #[test]
    fn keymap_the_keys_give_up_what_they_say_before_they_give_up_a_key() {
        // The same screen with no room for two whole bands. Every key is
        // still on it, because a key nobody can find is worse than one whose
        // line was cut short.
        let painted = overlay((60, 15));
        let all = painted.join("\n");
        for (key, _) in HELP {
            assert!(all.contains(key), "{key} is missing:\n{all}");
        }
        for line in &painted {
            assert!(line.chars().count() <= 60, "{line:?}");
        }
        assert!(
            all.contains('…'),
            "and what was cut says it was cut:\n{all}"
        );
    }
}
