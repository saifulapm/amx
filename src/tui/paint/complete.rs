//! What the word under the cursor could be, in a band under the line.
//!
//! A list rather than a hint. What `/rev` could mean is two things or twenty,
//! and somebody choosing between them is reading a column of words — so they
//! stand one to a row, under the line and over the keys, in the column the
//! line's own text starts in. The band takes those rows off the list of agents
//! the way the composer above it does, and gives them back the moment the word
//! is finished.
//!
//! Six of them at most. Past that the choice walks the list inside the band
//! rather than the band growing to meet it, which is what the composer does
//! with the rows of one long line, and for the same reason: the wall is what
//! the view is for.
//!
//! Each row is the word a vendor answers to and, behind it, what the file it
//! was read out of says about itself. The one the choice is standing on wears
//! the accent and the weight every prospective thing on this screen wears: it
//! is what the line will say if tab is pressed, and it has not happened yet.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::input::GUTTER;
use super::style::{dim, prospective};
use super::text::{fit, width_of};
use crate::catalog::Entry;
use crate::theme::Theme;
use crate::tui::act::Suggest;
use crate::tui::grid;

/// How many words the band shows at once.
const SHOWN: usize = 6;

/// What stands between a word and what it is for.
const GAP: usize = 2;

/// How many rows the band wants: one for each word offered, six at most, and
/// none at all where the line is offering nothing.
pub(super) fn rows_wanted(suggest: Option<&Suggest>) -> u16 {
    suggest.map_or(0, |suggest| suggest.entries.len().min(SHOWN) as u16)
}

/// The words themselves, with the one the choice is standing on among them.
///
/// Which six, where there are more than six: the first six until the choice
/// has walked past them, and the six ending on it after that. A band that
/// always showed the first six would hide the one thing the two keys over it
/// move.
pub(super) fn band(suggest: &Suggest, width: usize, theme: Theme) -> Vec<Line<'static>> {
    let from = suggest
        .chosen
        .saturating_sub(SHOWN - 1)
        .min(suggest.entries.len().saturating_sub(SHOWN));
    let shown = &suggest.entries[from..(from + SHOWN).min(suggest.entries.len())];
    // The column what they are for starts in: as wide as the widest word on
    // the band, so the sentences read as a column of their own rather than as
    // one hung off the end of each word.
    let column = shown
        .iter()
        .map(|entry| width_of(&entry.spelled))
        .max()
        .unwrap_or(0);
    shown
        .iter()
        .enumerate()
        .map(|(down, entry)| row(entry, from + down == suggest.chosen, column, width, theme))
        .collect()
}

/// One of them: the word, and what the thing it names says about itself.
fn row(entry: &Entry, chosen: bool, column: usize, width: usize, theme: Theme) -> Line<'static> {
    let indent = width_of(GUTTER);
    let says = width.saturating_sub(indent + column + GAP);
    let word = match chosen {
        true => prospective(theme),
        false => Style::new(),
    };
    Line::from(vec![
        Span::raw(" ".repeat(indent)),
        Span::styled(grid::pad(&entry.spelled, column), word),
        Span::raw(" ".repeat(GAP)),
        Span::styled(fit(&entry.about, says), dim()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Kind;

    /// A word a vendor answers to, and what it says about itself.
    fn entry(spelled: &str, about: &str) -> Entry {
        Entry {
            spelled: spelled.to_string(),
            kind: Kind::Skill,
            about: about.to_string(),
        }
    }

    /// What the line is offering, with the choice standing where it is.
    fn offering(words: &[impl AsRef<str>], chosen: usize) -> Suggest {
        Suggest {
            word: 0..4,
            entries: words
                .iter()
                .map(|word| entry(word.as_ref(), "what it is for"))
                .collect(),
            chosen,
        }
    }

    /// The band's rows as text, which is what a person reads off it.
    fn said(suggest: &Suggest, width: usize) -> Vec<String> {
        band(suggest, width, Theme::default())
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    /// Twenty words, which is more than any band shows at once.
    fn twenty() -> Vec<String> {
        (1..=20).map(|n| format!("/word-{n:02}")).collect()
    }

    #[test]
    fn composer_the_band_wants_a_row_for_each_word_and_never_more_than_six() {
        assert_eq!(
            rows_wanted(None),
            0,
            "a line offering nothing takes no rows"
        );
        assert_eq!(rows_wanted(Some(&offering(&["/review"], 0))), 1);
        assert_eq!(
            rows_wanted(Some(&offering(&["/review", "/revise"], 0))),
            2,
            "a row each, so the band is as tall as what it has to say"
        );

        assert_eq!(
            rows_wanted(Some(&offering(&twenty(), 0))),
            SHOWN as u16,
            "and it stops there rather than taking the wall"
        );
    }

    #[test]
    fn composer_the_band_says_each_word_and_what_it_is_for() {
        let suggest = Suggest {
            word: 0..4,
            entries: vec![
                entry("/review", "Read the diff."),
                entry("/revise", "Say it again."),
            ],
            chosen: 0,
        };
        assert_eq!(
            said(&suggest, 60),
            ["  /review  Read the diff.", "  /revise  Say it again."],
            "in the column the line's own text starts in, with what each of \
             them is for lined up behind them"
        );

        // A narrow screen keeps the words whole and cuts the sentence, which
        // is the half somebody can do without: the word is what tab writes.
        let narrow = said(&suggest, 20);
        assert_eq!(narrow[0], "  /review  Read the…");
    }

    #[test]
    fn composer_the_band_carries_the_weight_on_the_word_the_choice_is_on() {
        let suggest = offering(&["/review", "/revise"], 1);
        let rows = band(&suggest, 60, Theme::default());
        let word = |row: &Line<'static>| row.spans[1].style;

        assert_eq!(
            word(&rows[1]),
            prospective(Theme::default()),
            "the word the line would take is the one thing here that has not \
             happened yet"
        );
        assert_eq!(
            word(&rows[0]),
            Style::new(),
            "and the ones it is being chosen from are words on a row"
        );
        assert_eq!(
            rows[0].spans[3].style,
            dim(),
            "what a word is for stands behind it, on every row there is"
        );
    }

    #[test]
    fn composer_the_band_keeps_the_choice_in_the_six_words_it_shows() {
        let top = said(&offering(&twenty(), 0), 60);
        assert_eq!(top.len(), SHOWN);
        assert!(top[0].starts_with("  /word-01"), "{top:?}");

        // Walked past the sixth, the run under the band moves rather than the
        // choice walking off the foot of it.
        let down = said(&offering(&twenty(), 8), 60);
        assert!(down[0].starts_with("  /word-04"), "{down:?}");
        assert!(down[5].starts_with("  /word-09"), "{down:?}");

        let last = said(&offering(&twenty(), 19), 60);
        assert!(last[5].starts_with("  /word-20"), "{last:?}");
    }
}
