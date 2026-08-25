//! What the view is painted in.
//!
//! Six roles, named by what they mean rather than by what they are, because
//! that is how the view already asks for a colour: a row is painted for having
//! failed, not for being red. A theme is the answer to those six questions and
//! nothing else — no per-widget keys, no styles, no glyphs — so a person can
//! read one at a glance and write one in a minute.
//!
//! A theme is a convenience under the same law as [`crate::config`]: a file
//! that cannot be read or cannot be understood degrades to the built-in
//! palette with a warning, because a view painted in the wrong colours is a
//! view, and no view at all is not.

use anyhow::{Context, Result, anyhow};
use ratatui::style::Color;
use std::str::FromStr;

/// Every role a theme file may name. Anything else is warned about and ignored.
pub const ROLES: [&str; 6] = ["waiting", "done", "failed", "stopped", "accent", "cursor"];

/// The themes that ship inside the binary, in the order `theme` may name them.
pub const NAMES: [&str; 2] = ["default", "terminal"];

/// Their text, by the same names.
const SHIPPED: [(&str, &str); 2] = [
    ("default", include_str!("../assets/themes/default.toml")),
    ("terminal", include_str!("../assets/themes/terminal.toml")),
];

/// The colours the view paints with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Something is waiting on a person.
    pub waiting: Color,
    /// It went the way it was meant to.
    pub done: Color,
    /// It was attempted and it failed.
    pub failed: Color,
    /// It was ended by hand, and nothing more is coming.
    pub stopped: Color,
    /// What the next agent will be started with.
    pub accent: Color,
    /// The line the cursor is on, as a background.
    pub cursor: Color,
}

impl Default for Theme {
    /// The values `assets/themes/default.toml` spells out, kept here as well so
    /// that a theme naming five roles still has a sixth. The two are held
    /// together by a test, not by anybody remembering.
    fn default() -> Self {
        Self {
            waiting: Color::Rgb(255, 193, 7),
            done: Color::Rgb(78, 186, 101),
            failed: Color::Rgb(255, 107, 128),
            stopped: Color::Rgb(153, 153, 153),
            accent: Color::Cyan,
            cursor: Color::Rgb(55, 55, 55),
        }
    }
}

impl Theme {
    /// Where a role's colour is kept, for the parser to write into.
    fn slot(&mut self, role: &str) -> Option<&mut Color> {
        Some(match role {
            "waiting" => &mut self.waiting,
            "done" => &mut self.done,
            "failed" => &mut self.failed,
            "stopped" => &mut self.stopped,
            "accent" => &mut self.accent,
            "cursor" => &mut self.cursor,
            _ => return None,
        })
    }
}

/// The text of a theme that ships with amx.
pub fn shipped(name: &str) -> Option<&'static str> {
    SHIPPED
        .iter()
        .find(|(shipped, _)| *shipped == name)
        .map(|(_, text)| *text)
}

/// Parse theme text, returning the theme and any warnings about it.
///
/// A role left out keeps its default, so a file may say only what it wants
/// changed. An unknown role is a warning and nothing more: theme files outlive
/// the versions that wrote them, the same as config files do.
///
/// A value nothing can read is an error, which costs the whole file. Half a
/// theme is a view painted in two people's decisions, and which half survived
/// would depend on the order the keys happen to be written in.
pub fn parse(text: &str) -> Result<(Theme, Vec<String>)> {
    let table: toml::Table = text.parse().context("not valid TOML")?;
    let mut theme = Theme::default();
    let mut warnings = Vec::new();

    for (key, value) in &table {
        let Some(slot) = theme.slot(key) else {
            warnings.push(format!("ignoring unknown key `{key}`"));
            continue;
        };
        let said = value
            .as_str()
            .with_context(|| format!("{key}: a colour is written as a string"))?;
        *slot = colour(said).with_context(|| key.to_owned())?;
    }

    Ok((theme, warnings))
}

/// One value as a colour: a name, a 256-colour index, or a hex.
///
/// ratatui's own reading of the three, rather than a second one here. What the
/// terminal will do with the answer is that crate's business, and a parser of
/// amx's own would be a slightly different set of spellings for a person to
/// find out about the hard way.
fn colour(said: &str) -> Result<Color> {
    Color::from_str(said).map_err(|_| {
        anyhow!(
            "{said:?} is not a colour: a name like \"cyan\", an index like \"134\", or \"#4eba65\""
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn the_defaults_are_the_colours_the_view_paints_today() {
        let t = Theme::default();
        assert_eq!(t.waiting, Color::Rgb(255, 193, 7));
        assert_eq!(t.done, Color::Rgb(78, 186, 101));
        assert_eq!(t.failed, Color::Rgb(255, 107, 128));
        assert_eq!(t.stopped, Color::Rgb(153, 153, 153));
        assert_eq!(t.accent, Color::Cyan);
        assert_eq!(t.cursor, Color::Rgb(55, 55, 55));
    }

    #[test]
    fn a_value_is_a_name_an_index_or_a_hex() {
        let (t, w) = parse("waiting = \"cyan\"\ndone = \"134\"\nfailed = \"#ff0000\"\n").unwrap();
        assert_eq!(t.waiting, Color::Cyan);
        assert_eq!(t.done, Color::Indexed(134));
        assert_eq!(t.failed, Color::Rgb(255, 0, 0));
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn a_role_left_out_keeps_the_default_it_had() {
        let (t, w) = parse("accent = \"magenta\"").unwrap();
        assert_eq!(t.accent, Color::Magenta);
        assert_eq!(t.waiting, Theme::default().waiting);
        assert_eq!(t.cursor, Theme::default().cursor);
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn an_empty_file_is_the_defaults_and_says_nothing() {
        let (t, w) = parse("").unwrap();
        assert_eq!(t, Theme::default());
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn an_unknown_role_is_named_in_a_warning_and_the_rest_still_applies() {
        let (t, w) = parse("done = \"green\"\nbanner = \"blue\"\n").unwrap();
        assert_eq!(t.done, Color::Green);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("banner"), "{w:?}");
    }

    #[test]
    fn a_colour_nothing_can_read_costs_the_whole_file() {
        // Half a theme is a view painted in two people's decisions. The file
        // is the unit, so the one bad value takes the rest of it with it.
        let e = parse("done = \"green\"\nfailed = \"burnt sienna\"\n").unwrap_err();
        let said = format!("{e:#}");
        assert!(said.contains("failed"), "names the role: {said}");
        assert!(said.contains("burnt sienna"), "and the value: {said}");
    }

    #[test]
    fn a_role_given_something_that_is_not_a_string_is_an_error() {
        let e = parse("done = 134").unwrap_err();
        assert!(format!("{e:#}").contains("done"), "{e:#}");
    }

    #[test]
    fn malformed_toml_is_an_error() {
        assert!(parse("done = ").is_err());
    }

    #[test]
    fn the_default_theme_file_is_the_struct_default() {
        // Two statements of the same palette, and the file is the one a person
        // copies to start their own. They drift apart the day nothing checks.
        let (t, w) = parse(shipped("default").unwrap()).unwrap();
        assert_eq!(t, Theme::default());
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn every_shipped_theme_parses_and_names_every_role() {
        for name in NAMES {
            let text = shipped(name).unwrap();
            let (_, w) = parse(text).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert!(w.is_empty(), "{name}: {w:?}");
            for role in ROLES {
                assert!(
                    text.contains(&format!("{role} = ")),
                    "{name} leaves {role} to the default, where a change to the \
                     default moves it without a word"
                );
            }
        }
    }

    #[test]
    fn the_terminal_theme_names_colours_and_never_measures_them() {
        // The point of it: the person's own palette decides, and an RGB value
        // in here would be amx overruling the terminal it was chosen for.
        let (t, _) = parse(shipped("terminal").unwrap()).unwrap();
        for colour in [t.waiting, t.done, t.failed, t.stopped, t.accent, t.cursor] {
            assert!(
                matches!(
                    colour,
                    Color::Reset
                        | Color::Black
                        | Color::Red
                        | Color::Green
                        | Color::Yellow
                        | Color::Blue
                        | Color::Magenta
                        | Color::Cyan
                        | Color::Gray
                        | Color::DarkGray
                        | Color::LightRed
                        | Color::LightGreen
                        | Color::LightYellow
                        | Color::LightBlue
                        | Color::LightMagenta
                        | Color::LightCyan
                        | Color::White
                ),
                "{colour:?} is a value, not a name"
            );
        }
    }

    #[test]
    fn a_name_nothing_ships_is_not_a_theme_amx_has() {
        assert!(shipped("solarized").is_none());
        assert_eq!(
            NAMES.len(),
            2,
            "a theme this file does not name is untested"
        );
    }
}
