//! The completion band under the typed line.
//!
//! One candidate per row, under the line and over the keys, aligned with the
//! line's text. The band takes its rows from the list and shows at most
//! [`SHOWN`] candidates, scrolling to keep the chosen one visible. Each row is
//! the word a vendor answers to and the description from the file it came
//! from; the chosen word is styled as [`prospective`].

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::input::GUTTER;
use super::style::{dim, prospective};
use super::text::{fit, width_of};
use crate::catalog::Entry;
use crate::theme::Theme;
use crate::tui::act::Suggest;
use crate::tui::grid;

/// How many candidates the band shows at once.
const SHOWN: usize = 6;

/// Columns between a word and its description.
const GAP: usize = 2;

/// Rows the band wants: one per candidate, at most [`SHOWN`], none when
/// nothing is offered.
pub(super) fn rows_wanted(suggest: Option<&Suggest>) -> u16 {
    suggest.map_or(0, |suggest| suggest.entries.len().min(SHOWN) as u16)
}

/// The band's rows.
///
/// Shows the first [`SHOWN`] candidates until the choice moves past them, then
/// the [`SHOWN`] ending on the choice.
pub(super) fn band(suggest: &Suggest, width: usize, theme: Theme) -> Vec<Line<'static>> {
    let from = suggest
        .chosen
        .saturating_sub(SHOWN - 1)
        .min(suggest.entries.len().saturating_sub(SHOWN));
    let shown = &suggest.entries[from..(from + SHOWN).min(suggest.entries.len())];
    // Descriptions line up after the widest word shown.
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

/// One candidate: the word, then its description cut to fit.
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

    fn entry(spelled: &str, about: &str) -> Entry {
        Entry {
            spelled: spelled.to_string(),
            kind: Kind::Skill,
            about: about.to_string(),
        }
    }

    /// These words offered, with `chosen` selected.
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

    /// The band's rows as text.
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

    /// More words than the band shows at once.
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

        // A narrow screen cuts the description, never the word.
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

        // Past the sixth, the band scrolls to keep the choice on it.
        let down = said(&offering(&twenty(), 8), 60);
        assert!(down[0].starts_with("  /word-04"), "{down:?}");
        assert!(down[5].starts_with("  /word-09"), "{down:?}");

        let last = said(&offering(&twenty(), 19), 60);
        assert!(last[5].starts_with("  /word-20"), "{last:?}");
    }
}
