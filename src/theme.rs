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
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Every role a theme file may name. Anything else is warned about and ignored.
pub const ROLES: [&str; 6] = ["waiting", "done", "failed", "stopped", "accent", "cursor"];

/// The themes that ship inside the binary, by the name `theme` may call them.
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

/// What a theme file is called on disk.
const EXTENSION: &str = ".toml";

/// The text of a theme that ships with amx.
pub fn shipped(name: &str) -> Option<&'static str> {
    SHIPPED
        .iter()
        .find(|(shipped, _)| *shipped == name)
        .map(|(_, text)| *text)
}

/// Where a theme name pointed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A palette in the binary. Nothing on disk, so nothing to watch.
    Shipped(&'static str),
    /// A file, which is a file somebody may edit while the view is open.
    File(PathBuf),
}

impl Source {
    /// The file this came out of, when it came out of one.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Source::Shipped(_) => None,
            Source::File(path) => Some(path),
        }
    }
}

/// What `named` names, given the directory amx keeps themes in.
///
/// Three answers, in the order they are tried. A name amx ships is amx's own
/// word and is answered out of the binary. A name with a path separator in it
/// is a path, taken exactly as written, which is how a theme kept beside a
/// project or shared between machines is reached. Anything else is a file of
/// that name in the themes directory, with the extension added if it was left
/// off, because `mine` and `mine.toml` are the same wish.
pub fn source_in(themes: &Path, named: &str) -> Source {
    if let Some((name, _)) = SHIPPED.iter().find(|(name, _)| *name == named) {
        return Source::Shipped(name);
    }
    if named.contains('/') {
        return Source::File(PathBuf::from(named));
    }
    match named.ends_with(EXTENSION) {
        true => Source::File(themes.join(named)),
        false => Source::File(themes.join(format!("{named}{EXTENSION}"))),
    }
}

/// What `named` names, in the themes directory this machine keeps.
pub fn source(named: &str) -> Result<Source> {
    Ok(source_in(&themes_dir()?, named))
}

/// The theme `named`, with warnings for the caller to print.
pub fn load(named: &str) -> (Theme, Vec<String>) {
    match themes_dir() {
        Ok(themes) => load_in(&themes, named),
        Err(e) => (
            Theme::default(),
            vec![format!("using the default theme: {e}")],
        ),
    }
}

/// `~/.config/amx/themes`, beside the config file that names the theme.
fn themes_dir() -> Result<PathBuf> {
    let config = crate::paths::config_file()?;
    let dir = config
        .parent()
        .context("no directory to keep themes in")?
        .join("themes");
    Ok(dir)
}

/// The load itself, with the themes directory as a parameter.
///
/// Whatever goes wrong, a theme comes back: the view is the thing amx is for,
/// and a view in the wrong colours beats no view at all. What went wrong is
/// said instead, naming the file, since a person who asked for a theme and got
/// the default one is owed the reason.
fn load_in(themes: &Path, named: &str) -> (Theme, Vec<String>) {
    let path = match source_in(themes, named) {
        // A file of a shipped name is a file amx will never read. Silence
        // there is a person editing a copy of default.toml and watching
        // nothing happen.
        Source::Shipped(name) => {
            let text = shipped(name).expect("a shipped theme is part of the binary");
            let (theme, _) = parse(text).expect("a shipped theme is proved by its own test");
            let shadow = themes.join(format!("{name}{EXTENSION}"));
            let warnings = match shadow.exists() {
                true => vec![format!(
                    "{}: not read, `{name}` is a theme amx ships; give yours another name",
                    shadow.display()
                )],
                false => Vec::new(),
            };
            return (theme, warnings);
        }
        Source::File(path) => path,
    };

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => {
            let said = format!("no theme `{named}`: {} ({e})", path.display());
            return (Theme::default(), vec![said]);
        }
    };

    match parse(&text) {
        Ok((theme, warnings)) => (
            theme,
            warnings
                .into_iter()
                .map(|w| format!("{}: {w}", path.display()))
                .collect(),
        ),
        Err(e) => (
            Theme::default(),
            vec![format!(
                "ignoring {}, painting the default theme: {e:#}",
                path.display()
            )],
        ),
    }
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
    use tempfile::TempDir;

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
        for (name, text) in SHIPPED {
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
            SHIPPED.len(),
            2,
            "a theme this file does not name is one nothing here proves"
        );
    }

    #[test]
    fn a_name_amx_ships_comes_out_of_the_binary() {
        let dir = TempDir::new().unwrap();
        for (name, _) in SHIPPED {
            assert_eq!(source_in(dir.path(), name), Source::Shipped(name));
        }
        let (t, w) = load_in(dir.path(), "default");
        assert_eq!(t, Theme::default());
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn a_bare_name_is_a_file_in_the_themes_directory() {
        let themes = Path::new("/cfg/amx/themes");
        assert_eq!(
            source_in(themes, "solarized"),
            Source::File(themes.join("solarized.toml"))
        );
        assert_eq!(
            source_in(themes, "solarized.toml"),
            Source::File(themes.join("solarized.toml")),
            "and writing the extension out is not a second one"
        );
    }

    #[test]
    fn a_name_with_a_path_in_it_is_that_path_as_written() {
        let themes = Path::new("/cfg/amx/themes");
        for said in ["/etc/amx/dark.toml", "./dark.toml", "shared/dark.toml"] {
            assert_eq!(source_in(themes, said), Source::File(PathBuf::from(said)));
        }
    }

    #[test]
    fn only_a_theme_on_disk_has_a_path_to_watch() {
        // What t13 stats to notice an edit. There is nothing to watch about a
        // palette that is part of the binary.
        assert_eq!(Source::Shipped("default").path(), None);
        let path = Path::new("/cfg/amx/themes/mine.toml");
        assert_eq!(Source::File(path.to_path_buf()).path(), Some(path));
    }

    #[test]
    fn a_theme_on_disk_is_read_and_its_warnings_name_the_file() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "mine",
            "accent = \"magenta\"\nbanner = \"blue\"\n",
        );

        let (t, w) = load_in(dir.path(), "mine");
        assert_eq!(t.accent, Color::Magenta);
        assert_eq!(
            t.waiting,
            Theme::default().waiting,
            "the rest is the default"
        );
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("mine.toml"), "{w:?}");
        assert!(w[0].contains("banner"), "{w:?}");
    }

    #[test]
    fn a_broken_theme_falls_back_whole_and_names_the_file() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "mine",
            "accent = \"magenta\"\ndone = \"chartreuse\"\n",
        );

        let (t, w) = load_in(dir.path(), "mine");
        assert_eq!(t, Theme::default(), "including the value that did read");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("mine.toml"), "{w:?}");
        assert!(w[0].contains("chartreuse"), "{w:?}");
    }

    #[test]
    fn a_theme_that_is_not_there_says_so_rather_than_painting_on_quietly() {
        // Unlike config.toml, which nobody has to write: a theme by name is
        // something a person asked for, so getting the default instead is news.
        let dir = TempDir::new().unwrap();
        let (t, w) = load_in(dir.path(), "solarized");
        assert_eq!(t, Theme::default());
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("solarized"), "{w:?}");
    }

    #[test]
    fn a_file_shadowing_a_name_amx_ships_is_said_out_loud() {
        // The alternative is a person editing a copy of default.toml and
        // watching nothing happen.
        let dir = TempDir::new().unwrap();
        write(dir.path(), "default", "accent = \"magenta\"\n");

        let (t, w) = load_in(dir.path(), "default");
        assert_eq!(t, Theme::default(), "the shipped one still wins");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("default.toml"), "{w:?}");
    }

    #[test]
    fn the_wrappers_look_for_a_theme_beside_the_config_file() {
        // Reads the ambient environment and never touches it: whichever home
        // it finds, a name amx ships is answered out of the binary and a name
        // it does not is a file in the themes directory next to config.toml.
        let Ok(shipped) = source("default") else {
            return;
        };
        assert_eq!(shipped, Source::Shipped("default"));

        let Ok(mine) = source("solarized") else {
            return;
        };
        let path = mine.path().expect("a name amx does not ship is a file");
        assert!(
            path.ends_with("amx/themes/solarized.toml"),
            "{}",
            path.display()
        );

        // Whatever this machine has in its themes directory, a shipped name
        // paints the palette out of the binary.
        assert_eq!(load("default").0, Theme::default());
    }

    /// A theme file in a themes directory that may not exist yet.
    fn write(themes: &Path, name: &str, text: &str) {
        std::fs::create_dir_all(themes).unwrap();
        std::fs::write(themes.join(format!("{name}.toml")), text).unwrap();
    }
}
