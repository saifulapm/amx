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
