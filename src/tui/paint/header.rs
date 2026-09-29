//! The header above the list, and the terminal title.
//!
//! The first row is the present: the name, the directory, the fleet counts
//! and the waiting badge. The second row holds the dials the next agent will
//! be started with; it hangs off the first on a branch glyph and its values
//! wear the accent. The title counts waiting agents the same way the badge
//! does.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::style::dim;
use super::text::{SEPARATOR, fit, said};
use crate::registry;
use crate::theme::Theme;
use crate::tui::rows::{Group, List};
use crate::tui::{Profile, Screen};

/// Below this many rows the header drops its dials row.
pub(super) const SHORT: usize = 10;

/// From this many rows up, a blank row separates the header from the list and
/// the list from the keys. These are the first rows to go on a short screen.
pub(super) const SPACED: usize = 12;

/// The fewest columns worth showing a directory in; below this it is dropped.
const SHORTEST_DIR: usize = 8;

/// Rows the header takes at this terminal height.
pub(super) fn header_rows(height: u16) -> u16 {
    match (height as usize) < SHORT {
        true => 1,
        false => 2,
    }
}

/// Blank rows above and below the list at this terminal height.
pub(super) fn space_rows(height: u16) -> u16 {
    u16::from((height as usize) >= SPACED)
}

/// The header rows: the present, then the dials when there is room for both.
pub(super) fn header(screen: &Screen, area: Rect) -> Vec<Line<'static>> {
    let width = area.width as usize;
    // The right-hand block is laid out first; the name and directory get what
    // is left.
    let fleet = fleet(screen, width);
    let room = width.saturating_sub(said(&fleet) + 1);
    let mut lines = vec![spread(here(&screen.profile, room), fleet, width)];
    if area.height >= 2 {
        lines.push(Line::from(dials(&screen.profile, width, screen.theme)));
    }
    lines
}

/// The right side of the first row: fleet counts, the active narrowing, and
/// the waiting badge. The counts are dropped first when the row is too narrow.
fn fleet(screen: &Screen, width: usize) -> Vec<Span<'static>> {
    // The narrowing, in the words it was typed with.
    let mut kept = match screen.list.narrowing() {
        Some(narrowing) => vec![Span::styled(format!("{narrowing}{APART}"), dim())],
        None => Vec::new(),
    };
    kept.extend(badge(&screen.list, screen.theme));

    let counts = counters(&screen.list, screen.profile.cap);
    let together = said(&counts) + APART.chars().count() + said(&kept) + NAME.chars().count() + 1;
    match together <= width {
        true => [counts, vec![Span::raw(APART)], kept].concat(),
        false => kept,
    }
}

/// The count of agents waiting on the user, in bold reverse video in the
/// waiting colour. At zero it reads [`NOBODY`], dim, in the same place.
fn badge(list: &List, theme: Theme) -> Vec<Span<'static>> {
    match list.waiting() {
        0 => vec![Span::styled(NOBODY, dim())],
        count => vec![Span::styled(
            format!(" {count} WAITING "),
            Style::new()
                .fg(theme.waiting)
                .add_modifier(Modifier::REVERSED | Modifier::BOLD),
        )],
    }
}

/// The badge's text when nothing is waiting.
const NOBODY: &str = "nothing waiting";

/// Two blocks on one row, left-aligned and right-aligned, with at least one
/// column between them.
///
/// When both do not fit the right block is dropped, unless the left one is
/// empty, in which case the right one gets the row.
fn spread(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut spans = left;
    match width.checked_sub(said(&spans) + said(&right)) {
        Some(gap) if gap >= 1 => {
            spans.push(Span::raw(" ".repeat(gap)));
            spans.extend(right);
        }
        _ if said(&spans) == 0 => return Line::from(right),
        _ => {}
    }
    Line::from(spans)
}

/// The left side of the first row: [`NAME`] in bold, then the directory the
/// view was opened on (where new agents run), dim. The directory is cut to
/// fit and dropped below [`SHORTEST_DIR`] columns.
fn here(profile: &Profile, room: usize) -> Vec<Span<'static>> {
    let name = Span::styled(fit(NAME, room), Style::new().add_modifier(Modifier::BOLD));
    let left = room.saturating_sub(NAME.chars().count() + BESIDE.chars().count());
    match !profile.dir.is_empty() && left >= SHORTEST_DIR {
        true => vec![
            name,
            Span::styled(format!("{BESIDE}{}", fit(&profile.dir, left)), dim()),
        ],
        false => vec![name],
    }
}

/// The name in the top left corner.
const NAME: &str = "AMX";

/// Gap between a label and its value, and between the name and directory.
const BESIDE: &str = "  ";

/// Gap between items on the header rows.
const APART: &str = "   ";

/// The second row: the agent command and every dial the next agent will be
/// started with.
///
/// Starts with [`BRANCH`]. Labels are dim, values wear the accent. A dial the
/// vendor does not declare is left out; one at the vendor's default reads
/// `default`. When the labelled row does not fit, every label but `next` is
/// dropped and [`MARKED`] separates the values. The agent command takes the
/// columns left over, but never fewer than [`SHORTEST_AGENT`].
fn dials(profile: &Profile, width: usize, theme: Theme) -> Vec<Span<'static>> {
    let mut pairs: Vec<(&'static str, String)> = Vec::new();
    if profile.model_dial().is_some() {
        pairs.push(("model", profile.model.clone()));
    }
    if profile.permission_dial().is_some() {
        pairs.push(("permission", profile.permission.clone()));
    }
    pairs.push((
        "worktree",
        match profile.worktree {
            true => TREE,
            false => NO_TREE,
        }
        .to_string(),
    ));

    // Columns the row takes besides the agent command, with or without labels.
    let chrome = |pairs: &[(&'static str, String)], labelled: bool| {
        BRANCH.chars().count()
            + NEXT.chars().count()
            + BESIDE.chars().count()
            + pairs
                .iter()
                .map(|(label, at)| {
                    at.chars().count()
                        + match labelled {
                            true => {
                                APART.chars().count()
                                    + label.chars().count()
                                    + BESIDE.chars().count()
                            }
                            false => MARKED.chars().count(),
                        }
                })
                .sum::<usize>()
    };

    // Effort at its default is left out rather than cost the other dials
    // their labels. Once set, it is always shown.
    if profile.effort_dial().is_some() {
        pairs.push(("effort", profile.effort.clone()));
        if profile.effort == registry::DEFAULT && chrome(&pairs, true) + SHORTEST_AGENT > width {
            pairs.pop();
        }
    }

    let labelled = chrome(&pairs, true) + SHORTEST_AGENT <= width;
    let room = width
        .saturating_sub(chrome(&pairs, labelled))
        .max(SHORTEST_AGENT);

    let turned = Style::new().fg(theme.accent);
    let mut spans = vec![
        Span::styled(format!("{BRANCH}{NEXT}{BESIDE}"), dim()),
        Span::styled(fit(&profile.agent, room), turned),
    ];
    for (label, at) in pairs {
        spans.push(Span::styled(
            match labelled {
                true => format!("{APART}{label}{BESIDE}"),
                false => MARKED.to_string(),
            },
            dim(),
        ));
        spans.push(Span::styled(at, turned));
    }
    clipped(spans, width)
}

/// The fewest columns the agent command keeps; the labels go first.
const SHORTEST_AGENT: usize = 8;

/// The glyph the dials row hangs off.
const BRANCH: &str = "└ ";

/// The agent command's label, the one label never dropped.
const NEXT: &str = "next";

/// The worktree dial's two values.
const TREE: &str = "new";
const NO_TREE: &str = "none";

/// Separator between dial values when the labels are dropped.
const MARKED: &str = "  ·  ";

/// Spans cut to `width` columns; the last span kept ends in an ellipsis.
fn clipped(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    if said(&spans) <= width {
        return spans;
    }
    let mut left = width;
    let mut kept = Vec::new();
    for span in spans {
        if left == 0 {
            break;
        }
        let taken = span.width();
        if taken <= left {
            left -= taken;
            kept.push(span);
            continue;
        }
        kept.push(Span::styled(fit(&span.content, left), span.style));
        left = 0;
    }
    kept
}

/// The fleet counts, dim: one per group in the word the list can be narrowed
/// by, except waiting (the badge counts that), then the running count.
fn counters(list: &List, cap: Option<usize>) -> Vec<Span<'static>> {
    let mut said: Vec<String> = list
        .counts()
        .iter()
        .filter(|(group, _)| *group != Group::NeedsInput)
        .map(|&(group, count)| format!("{count} {}", group.state()))
        .collect();

    // Running agents over the spawn cap, when the view has one. A machine-wide
    // view with no `max_total` has no cap to show.
    said.push(match cap {
        Some(cap) => format!("{}/{cap} running", list.live()),
        None => format!("{} running", list.live()),
    });
    vec![Span::styled(said.join(APART), dim())]
}

/// The terminal title: `amx`, plus the waiting count when it is not zero.
///
/// Counted over the list, as the badge is, so a view scoped to a directory
/// counts only its own agents.
pub fn title(list: &List) -> String {
    match list.waiting() {
        0 => "amx".to_string(),
        count => format!("amx{SEPARATOR}{count} waiting"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::View;
    use crate::store::Phase;
    use crate::tui::paint::fixtures::{
        a_fleet, cells, drawn, heading_of, launching, painted, showing, theme, view,
    };
    use crate::tui::rows::Narrow;
    use ratatui::style::Color;
    use std::path::PathBuf;

    /// Room for the whole header.
    const WIDE: (u16, u16) = (100, 12);

    /// One drawn row.
    fn screen_line(screen: &Screen, size: (u16, u16), row: usize) -> String {
        painted(screen, size)[row].clone()
    }

    /// The column `word` starts at in a drawn row (in chars, not bytes).
    fn column_of(line: &str, word: &str) -> u16 {
        let at = line.find(word).expect("the word is on the line");
        line[..at].chars().count() as u16
    }

    #[test]
    fn the_space_over_the_list_is_the_first_row_a_short_screen_takes_back() {
        let tall = drawn(a_fleet(), None, (60, SPACED as u16));
        assert_eq!(tall[2], "", "{tall:?}");
        assert_eq!(heading_of(&tall[3]), "Needs input", "{tall:?}");

        let short = drawn(a_fleet(), None, (60, SPACED as u16 - 1));
        assert_eq!(heading_of(&short[2]), "Needs input", "{short:?}");
        assert!(short[3].contains("ask-a1b"), "{short:?}");
    }

    /// More rows than a tall screen holds, over enough headings that no fold
    /// shortens it.
    fn a_full_wall() -> Vec<View> {
        (0..42)
            .map(|at| {
                let mut view = view(&format!("busy-{at:03}"), Phase::Working, None, 3);
                view.meta.dir = PathBuf::from(format!("/srv/app{}", at % 6));
                view
            })
            .collect()
    }

    #[test]
    fn the_keys_row_stands_off_the_list_on_a_screen_with_the_row_to_spare() {
        let mut screen = showing(a_full_wall(), None);
        screen.list.turn();

        let tall = painted(&screen, (60, 45));
        assert!(!tall[42].is_empty(), "the list fills the screen: {tall:?}");
        assert_eq!(tall[43], "", "{tall:?}");
        assert!(tall[44].starts_with("space card"), "{:?}", tall[44]);

        // A short screen gives the blank row back to the list.
        let short = painted(&screen, (60, SPACED as u16 - 1));
        assert!(!short[9].is_empty(), "{short:?}");
        assert!(short[10].starts_with("space card"), "{:?}", short[10]);
    }

    #[test]
    fn header_says_where_it_is_and_what_the_fleet_is_over_the_dials() {
        let screen = painted(
            &launching(vec![
                view("ask-a1b", Phase::Waiting, None, 30),
                view("busy-b2c", Phase::Working, Some("Running Bash"), 3),
            ]),
            WIDE,
        );

        assert!(
            screen[0].starts_with("AMX  ~/code/amx"),
            "whose screen this is and where it was opened: {:?}",
            screen[0]
        );
        assert!(
            !screen[0].contains(env!("CARGO_PKG_VERSION")),
            "which version this is says nothing about the fleet: {:?}",
            screen[0]
        );
        assert!(
            screen[0].ends_with("1 working   2/5 running    1 WAITING"),
            "what the fleet is, the gate the next one meets, and the one count \
             that wants somebody at the end of the row: {:?}",
            screen[0]
        );
        assert_eq!(
            screen[1],
            "└ next  claude   model  default   permission  default   worktree  new   \
             effort  default",
            "and under it every dial the next agent will be started with"
        );
        assert_eq!(screen[2], "", "a blank row stands the list off from it");
        assert_eq!(
            heading_of(&screen[3]),
            "Needs input",
            "and the list starts under that"
        );
    }

    #[test]
    fn header_spends_its_one_colour_on_the_count_that_wants_a_person() {
        let screen = launching(vec![
            view("ask-a1b", Phase::Waiting, None, 30),
            view("ask-b2c", Phase::Waiting, None, 10),
            view("busy-c3d", Phase::Working, Some("Running Bash"), 3),
        ]);
        let drawn = painted(&screen, WIDE);

        assert!(
            drawn[0].ends_with(" 2 WAITING"),
            "the one number the view was opened for, at the end of the row: {:?}",
            drawn[0]
        );
        assert!(
            drawn[0].contains("1 working   3/5 running"),
            "the counts beside it say the rest of the fleet, and say the \
             waiting one nowhere else: {:?}",
            drawn[0]
        );

        // Reverse video in the waiting colour through the row's last cell,
        // padding included.
        let buffer = cells(&screen, WIDE);
        for column in column_of(&drawn[0], " 2 WAITING")..WIDE.0 {
            let cell = buffer[(column, 0)].clone();
            assert_eq!(cell.fg, theme().waiting, "column {column}: {:?}", drawn[0]);
            assert!(
                cell.modifier.contains(Modifier::REVERSED | Modifier::BOLD),
                "column {column}: {:?}",
                cell.modifier
            );
        }
    }

    #[test]
    fn header_says_nothing_waiting_in_words_where_nobody_is() {
        let screen = launching(vec![view(
            "busy-a1b",
            Phase::Working,
            Some("Running Bash"),
            3,
        )]);
        let drawn = painted(&screen, WIDE);

        assert!(
            drawn[0].ends_with("nothing waiting"),
            "the answer stands where the answer always stands: {:?}",
            drawn[0]
        );

        let buffer = cells(&screen, WIDE);
        let cell = buffer[(column_of(&drawn[0], "nothing waiting"), 0)].clone();
        assert_eq!(
            cell.fg,
            Color::Reset,
            "nothing is asking, so nothing shouts"
        );
        assert!(cell.modifier.contains(Modifier::DIM), "{:?}", cell.modifier);
        assert!(
            !cell.modifier.contains(Modifier::REVERSED),
            "{:?}",
            cell.modifier
        );
    }

    #[test]
    fn header_hangs_the_dials_off_the_row_they_are_under() {
        let screen = launching(Vec::new());
        let drawn = painted(&screen, WIDE);
        assert_eq!(
            drawn[1],
            "└ next  claude   model  default   permission  default   worktree  new   \
             effort  default",
            "one glyph in the first column says the row is subordinate to the \
             one above it, without a word of explanation"
        );

        let buffer = cells(&screen, WIDE);
        for label in ["└", "next", "model", "permission", "worktree", "effort"] {
            let cell = buffer[(column_of(&drawn[1], label), 1)].clone();
            assert_eq!(cell.fg, Color::Reset, "{label}: {:?}", drawn[1]);
            assert!(
                cell.modifier.contains(Modifier::DIM),
                "{label}: {:?}",
                cell.modifier
            );
        }
        // Values wear the accent.
        for value in ["claude", "new"] {
            let cell = buffer[(column_of(&drawn[1], value), 1)].clone();
            assert_eq!(cell.fg, theme().accent, "{value}: {:?}", drawn[1]);
            assert!(
                !cell.modifier.contains(Modifier::DIM),
                "{value}: {:?}",
                cell.modifier
            );
        }
    }

    #[test]
    fn header_drops_the_dial_labels_before_it_cuts_what_they_are_set_to() {
        let screen = launching(Vec::new());
        assert_eq!(
            screen_line(&screen, (60, 12), 1),
            "└ next  claude  ·  default  ·  default  ·  new",
            "the value is the reading; the label is what a person already knows \
             the order of. Only `next` keeps its own, because it is what says \
             which half of the screen the row is about"
        );
    }

    #[test]
    fn header_names_a_dial_that_rests_where_the_vendor_left_it() {
        let mut screen = launching(Vec::new());
        assert_eq!(
            screen_line(&screen, WIDE, 1),
            "└ next  claude   model  default   permission  default   worktree  new   \
             effort  default",
            "the vendor's own answer said as a value, not a guess at which \
             model claude would have picked"
        );

        // Set dials show their values; the labels stay put.
        screen.profile.model = "opus".to_string();
        screen.profile.permission = "plan".to_string();
        screen.profile.effort = "high".to_string();
        screen.profile.worktree = false;
        assert_eq!(
            screen_line(&screen, WIDE, 1),
            "└ next  claude   model  opus   permission  plan   worktree  none   effort  high"
        );

        // An unknown agent declares no dials; only amx's worktree dial is left.
        screen.profile.agent = "mock-claude".to_string();
        assert_eq!(
            screen_line(&screen, WIDE, 1),
            "└ next  mock-claude   worktree  none"
        );
    }

    #[test]
    fn header_keeps_the_effort_dial_off_a_row_too_narrow_to_name_it() {
        // At 80 columns four labelled dials fill the row, so a default effort
        // is left off.
        let mut screen = launching(Vec::new());
        assert_eq!(
            screen_line(&screen, (80, 12), 1),
            "└ next  claude   model  default   permission  default   worktree  new"
        );

        // A set effort is shown even if the labels have to go.
        screen.profile.effort = "high".to_string();
        assert_eq!(
            screen_line(&screen, (80, 12), 1),
            "└ next  claude  ·  default  ·  default  ·  new  ·  high"
        );
    }

    #[test]
    fn header_counts_the_fleet_in_the_words_a_filter_takes() {
        let mut screen = launching(vec![
            view("ask-a1b", Phase::Waiting, None, 30),
            view("done-b2c", Phase::Done, Some("did it"), 60),
        ]);
        assert!(
            screen_line(&screen, WIDE, 0).ends_with("1 done   1/5 running    1 WAITING"),
            "the heading over the rows says `needs input`; the counter says \
             the word the list can be narrowed by, and says the waiting one \
             once, in the badge: {:?}",
            screen_line(&screen, WIDE, 0)
        );

        // The active narrowing is shown in the words it was typed with.
        screen
            .list
            .narrow(vec![Narrow::State(Some("waiting".to_string()))]);
        assert!(
            screen_line(&screen, WIDE, 0).ends_with("1/5 running   s:waiting    1 WAITING"),
            "{:?}",
            screen_line(&screen, WIDE, 0)
        );

        // A user-made group (pinned) is counted like any other.
        screen.list.narrow(vec![Narrow::State(None)]);
        for _ in 0..2 {
            screen.list.down();
        }
        assert!(screen.list.hold_or_let_go());
        assert!(
            screen_line(&screen, WIDE, 0).ends_with("1 pinned   1/5 running    1 WAITING"),
            "{:?}",
            screen_line(&screen, WIDE, 0)
        );
    }

    #[test]
    fn header_counts_a_sleeping_agent_in_its_own_word_and_among_the_ones_asking() {
        let mut screen = launching(vec![
            view("ask-a1b", Phase::Waiting, None, 30),
            view("busy-b2c", Phase::Working, Some("Running Bash"), 3),
        ]);

        // The cursor opens on the waiting agent, so that is the one put to
        // sleep.
        assert!(screen.list.sleep_or_wake());
        assert!(
            screen_line(&screen, WIDE, 0)
                .ends_with("1 working   1 asleep   2/5 running    1 WAITING"),
            "the group a person made is counted in the word that finds it \
             again, and the badge is the one number a mark on a row does not \
             move: an agent somebody put away is still asking them: {:?}",
            screen_line(&screen, WIDE, 0)
        );
    }

    #[test]
    fn header_says_the_cap_the_fleet_is_counted_against_before_it_refuses() {
        let mut screen = launching(vec![
            view("busy-a1b", Phase::Working, None, 3),
            view("busy-b2c", Phase::Working, None, 3),
            view("busy-c3d", Phase::Working, None, 3),
            view("done-d4e", Phase::Done, Some("did it"), 60),
        ]);
        screen.profile.cap = Some(5);
        assert!(
            screen_line(&screen, WIDE, 0).contains("3/5 running"),
            "an agent whose command has ended holds no slot: {:?}",
            screen_line(&screen, WIDE, 0)
        );

        // A machine-wide view with no `max_total` shows the bare count;
        // `max_agents` is per project and does not apply.
        screen.profile.cap = None;
        let line = screen_line(&screen, WIDE, 0);
        assert!(line.contains("3 running"), "{line:?}");
        assert!(!line.contains("3/"), "and no cap beside it: {line:?}");
    }

    #[test]
    fn header_sheds_the_dir_before_the_name_and_the_vendor_before_a_dial() {
        let mut screen = launching(vec![view("busy-a1b", Phase::Working, None, 3)]);

        let cramped = painted(&screen, (28, 12));
        assert!(
            cramped[0].starts_with("AMX"),
            "the name says what the screen is, and it is three columns: {:?}",
            cramped[0]
        );
        assert!(
            !cramped[0].contains("code/amx"),
            "a path cut to nothing is not a path: {:?}",
            cramped[0]
        );

        // A long agent command is cut before any dial is.
        screen.profile.agent = "claude --settings /etc/amx/every-hook.json".to_string();
        let long = painted(&screen, (80, 12));
        assert!(long[1].starts_with("└ next  claude --set"), "{:?}", long[1]);
        assert!(
            long[1].contains('…'),
            "and it says it was cut: {:?}",
            long[1]
        );
        assert!(
            long[1].ends_with("permission  default   worktree  new"),
            "{:?}",
            long[1]
        );

        // Narrower, the labels go before the command is cut further.
        assert_eq!(
            screen_line(&screen, (50, 12), 1),
            "└ next  claude --…  ·  default  ·  default  ·  new"
        );

        // Narrower still, the command keeps its floor and the row is cut.
        let narrow = screen_line(&screen, (36, 12), 1);
        assert!(narrow.starts_with("└ next  claude …"), "{narrow:?}");
        assert!(
            narrow.ends_with('…'),
            "and the end of the row is what says it was cut: {narrow:?}"
        );
    }

    #[test]
    fn header_sheds_the_counts_before_the_one_that_wants_a_person() {
        // Too many counts for a narrow row: the counts go, the badge stays.
        let screen = launching(vec![
            view("ask-a1b", Phase::Waiting, None, 30),
            view("busy-b2c", Phase::Working, None, 3),
            view("idle-c3d", Phase::Idle, None, 30),
            view("done-d4e", Phase::Done, Some("did it"), 60),
        ]);

        assert!(
            screen_line(&screen, (60, 12), 0).contains("1 working   2 done   3/5 running"),
            "{:?}",
            screen_line(&screen, (60, 12), 0)
        );

        let cramped = screen_line(&screen, (40, 12), 0);
        assert!(cramped.starts_with("AMX  ~/code/amx"), "{cramped:?}");
        assert!(cramped.ends_with(" 1 WAITING"), "{cramped:?}");
        assert!(
            !cramped.contains("running"),
            "and the counting is what gave the room up: {cramped:?}"
        );
    }

    #[test]
    fn header_measures_a_wide_directory_in_columns() {
        let mut screen = launching(vec![view("ask-a1b", Phase::Waiting, None, 30)]);
        screen.profile.dir = "~/code/日本語".to_string();
        let line = screen_line(&screen, WIDE, 0);
        assert!(line.ends_with(" 1 WAITING"), "{line:?}");
    }

    #[test]
    fn header_gives_the_row_back_to_the_list_on_a_short_screen() {
        let screen = launching(vec![view("busy-a1b", Phase::Working, None, 3)]);
        let short = painted(&screen, (60, SHORT as u16 - 1));

        assert!(
            short[0].starts_with("AMX  ~/code/amx"),
            "the row that says what there is stays; the dials are one \
             keypress from being read under the composer: {:?}",
            short[0]
        );
        assert!(
            short[0].ends_with("1 working   1/5 running   nothing waiting"),
            "{:?}",
            short[0]
        );
        assert!(!short.iter().any(|line| line.starts_with('└')), "{short:?}");
        assert_eq!(
            heading_of(&short[1]),
            "Working",
            "and the list starts a row sooner"
        );
    }
}
