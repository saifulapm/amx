//! The view's colour themes: six semantic roles, loaded from a shipped palette
//! or a TOML file, and reloaded when that file changes.
//!
//! Like [`crate::config`], a theme never blocks the view: a file that cannot
//! be read or parsed falls back to the built-in palette with a warning.

use crate::paths::stamped;
use crate::shade::Shade;
use anyhow::{Context, Result, anyhow};
use ratatui::style::Color;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::SystemTime;

/// The themes built into the binary, by name.
const SHIPPED: [(&str, &str); 3] = [
    ("default", include_str!("../assets/themes/default.toml")),
    ("light", include_str!("../assets/themes/light.toml")),
    ("terminal", include_str!("../assets/themes/terminal.toml")),
];

/// The theme name that picks `light` or `default` from the terminal's
/// background.
///
/// Resolved by [`for_the_shade`] before anything is loaded, so there is no
/// `auto.toml`. It is the config default.
pub const AUTO: &str = "auto";

/// The palette [`AUTO`] resolves to on a terminal of this shade. Any other
/// name is returned unchanged.
pub fn for_the_shade(named: &str, shade: Shade) -> &str {
    match (named == AUTO, shade) {
        (false, _) => named,
        (true, Shade::Light) => "light",
        (true, Shade::Dark) => "default",
    }
}

/// Like [`for_the_shade`], calling `ask` for the shade only when the name is
/// [`AUTO`].
pub fn chosen(named: &str, ask: impl FnOnce() -> Shade) -> &str {
    match named == AUTO {
        true => for_the_shade(named, ask()),
        false => named,
    }
}

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
    /// The values in `assets/themes/default.toml`, which fill any role a theme
    /// file leaves out. A test keeps the two in step.
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
    /// The field for `role`, or `None` for an unknown role.
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

/// The extension of a theme file.
const EXTENSION: &str = ".toml";

/// The text of a shipped theme.
pub fn shipped(name: &str) -> Option<&'static str> {
    SHIPPED
        .iter()
        .find(|(shipped, _)| *shipped == name)
        .map(|(_, text)| *text)
}

/// Where a theme name resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A palette built into the binary.
    Shipped(&'static str),
    /// A theme file on disk.
    File(PathBuf),
}

impl Source {
    /// The file path, for a theme on disk.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Source::Shipped(_) => None,
            Source::File(path) => Some(path),
        }
    }
}

/// Resolve a theme name against the themes directory.
///
/// A shipped name is built in. A name containing `/` is a path used as
/// written. Anything else is a file in `themes`, with `.toml` added if
/// missing.
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

/// Load the theme `named`, with warnings for the caller to print.
pub fn load(named: &str) -> (Theme, Vec<String>) {
    match themes_dir() {
        Ok(themes) => load_in(&themes, named),
        Err(e) => (
            Theme::default(),
            vec![format!("using the default theme: {e}")],
        ),
    }
}

/// Polls a theme file for edits so the view can reload it.
///
/// A stat per view tick rather than a filesystem watcher: no thread or
/// descriptor to manage, at the cost of seeing an edit on the next tick.
#[derive(Debug, Default, Clone)]
pub struct Watch {
    /// The theme name from the config.
    named: String,
    /// The themes directory the name was resolved against.
    themes: PathBuf,
    /// The file to stat; `None` for a shipped palette.
    file: Option<PathBuf>,
    /// The file's length and mtime at the last read. `None` means no file
    /// existed, so one appearing counts as a change.
    seen: Option<(u64, SystemTime)>,
}

impl Watch {
    /// Watch `named` in this machine's themes directory.
    ///
    /// Call before loading the theme: stamping after the read could miss an
    /// edit made between the two.
    pub fn of(named: &str) -> Watch {
        // The load that follows reports a missing themes directory.
        match themes_dir() {
            Ok(themes) => Watch::of_in(&themes, named),
            Err(_) => Watch::default(),
        }
    }

    /// [`Watch::of`] with an explicit themes directory.
    pub fn of_in(themes: &Path, named: &str) -> Watch {
        let file = source_in(themes, named).path().map(Path::to_path_buf);
        let seen = file.as_deref().and_then(stamped);
        Watch {
            named: named.to_string(),
            themes: themes.to_path_buf(),
            file,
            seen,
        }
    }

    /// The reloaded theme if the file changed since the last read, else
    /// `None`.
    pub fn reread(&mut self) -> Option<(Theme, Vec<String>)> {
        let file = self.file.as_deref()?;
        let now = stamped(file);
        if now == self.seen {
            return None;
        }
        self.seen = now;
        Some(load_in(&self.themes, &self.named))
    }
}

/// `~/.config/amx/themes`, beside the config file.
fn themes_dir() -> Result<PathBuf> {
    let config = crate::paths::config_file()?;
    let dir = config
        .parent()
        .context("no directory to keep themes in")?
        .join("themes");
    Ok(dir)
}

/// [`load`] with an explicit themes directory.
///
/// Always returns a theme, falling back to the default with a warning that
/// names the file.
fn load_in(themes: &Path, named: &str) -> (Theme, Vec<String>) {
    let path = match source_in(themes, named) {
        // A file shadowing a shipped name is never read, so warn about it
        // rather than let edits to it go unnoticed.
        Source::Shipped(name) => {
            let text = shipped(name).expect("a shipped theme is part of the binary");
            let (theme, _) = parse(text).expect("a shipped theme is proved by its own test");
            let shadow = themes.join(format!("{name}{EXTENSION}"));
            let warnings = match shadow.exists() {
                true => vec![format!(
                    "{}: ignored: `{name}` is a built-in theme; give yours another name",
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
                "ignoring {}, using the default theme: {e:#}",
                path.display()
            )],
        ),
    }
}

/// Parse theme text, returning the theme and any warnings about it.
///
/// Missing roles keep their defaults and unknown roles are warnings. A value
/// that is not a colour fails the whole file, so a theme is never half
/// applied.
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
            .with_context(|| format!("{key}: a colour must be a string"))?;
        *slot = colour(said).with_context(|| key.to_owned())?;
    }

    Ok((theme, warnings))
}

/// Parse a colour name, 256-colour index or hex value, using ratatui's own
/// parser.
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

    /// Every role a theme file may name.
    const ROLES: [&str; 6] = ["waiting", "done", "failed", "stopped", "accent", "cursor"];

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
        // One bad value rejects the whole file.
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
        // The file is what people copy to start their own theme.
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
        // Named colours only, so the terminal's own palette decides.
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
        assert!(
            shipped(AUTO).is_none(),
            "`auto` is a name resolved before anything is loaded, not a file: \
             a palette under it would be one amx never opens"
        );
        assert_eq!(
            SHIPPED.len(),
            3,
            "a theme this file does not name is one nothing here proves"
        );
    }

    #[test]
    fn auto_names_the_shipped_palette_for_the_shade_the_terminal_is() {
        assert_eq!(for_the_shade(AUTO, Shade::Light), "light");
        assert_eq!(for_the_shade(AUTO, Shade::Dark), "default");

        // Any other name is used as written, whatever the shade.
        for named in ["default", "light", "terminal", "solarized", "./mine.toml"] {
            for shade in [Shade::Light, Shade::Dark] {
                assert_eq!(for_the_shade(named, shade), named, "{named} {shade:?}");
            }
        }
    }

    #[test]
    fn the_light_theme_is_a_palette_for_a_page_rather_than_a_pane() {
        // Every role must be readable on a white background.
        let (light, w) = parse(shipped("light").unwrap()).unwrap();
        assert!(w.is_empty(), "{w:?}");
        for (role, colour) in [
            ("waiting", light.waiting),
            ("done", light.done),
            ("failed", light.failed),
            ("stopped", light.stopped),
            ("accent", light.accent),
        ] {
            let Color::Rgb(r, g, b) = colour else {
                panic!("{role} is a name, and a name is whatever the terminal says");
            };
            let against = 2126 * u32::from(r) + 7152 * u32::from(g) + 722 * u32::from(b);
            assert!(
                against < 10_000 * 128,
                "{role} is too light to read on a light background: {colour:?}"
            );
        }

        // The cursor bar is lighter than the text colours, so a row stays
        // readable under it.
        let Color::Rgb(r, g, b) = light.cursor else {
            panic!("the bar is measured, like the rest of them");
        };
        let bar = 2126 * u32::from(r) + 7152 * u32::from(g) + 722 * u32::from(b);
        assert!(bar > 10_000 * 128, "{:?}", light.cursor);
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
        // A shipped palette has no file to watch.
        assert_eq!(Source::Shipped("default").path(), None);
        let path = Path::new("/cfg/amx/themes/mine.toml");
        assert_eq!(Source::File(path.to_path_buf()).path(), Some(path));
    }

    #[test]
    fn a_theme_edited_while_it_is_being_watched_is_read_again() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "mine", "accent = \"magenta\"\n");

        let mut watch = Watch::of_in(dir.path(), "mine");
        assert!(
            watch.reread().is_none(),
            "a file nobody has touched is a file to leave alone"
        );

        write(dir.path(), "mine", "accent = \"blue\"\n");
        let (t, w) = watch.reread().expect("the edit");
        assert_eq!(t.accent, Color::Blue);
        assert!(w.is_empty(), "{w:?}");
        assert!(watch.reread().is_none(), "and once, not every pass after");
    }

    #[test]
    fn a_theme_written_after_the_watch_began_is_read_when_it_appears() {
        // A theme file created after the view opened is picked up without a
        // restart.
        let dir = TempDir::new().unwrap();
        let mut watch = Watch::of_in(dir.path(), "mine");
        assert!(watch.reread().is_none());

        write(dir.path(), "mine", "accent = \"magenta\"\n");
        let (t, w) = watch.reread().expect("the file appearing");
        assert_eq!(t.accent, Color::Magenta);
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn a_theme_edited_into_nonsense_falls_back_and_says_which_file() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "mine", "accent = \"magenta\"\n");
        let mut watch = Watch::of_in(dir.path(), "mine");

        write(dir.path(), "mine", "accent = \"chartreuse\"\n");
        let (t, w) = watch.reread().expect("the edit");
        assert_eq!(t, Theme::default());
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("mine.toml"), "{w:?}");
    }

    #[test]
    fn a_palette_in_the_binary_has_nothing_to_watch() {
        // Not even a file shadowing the shipped name: the warning was given
        // at load and is not repeated on every edit.
        let dir = TempDir::new().unwrap();
        let mut watch = Watch::of_in(dir.path(), "default");
        write(dir.path(), "default", "accent = \"magenta\"\n");
        assert!(watch.reread().is_none());
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
        // Unlike a missing config.toml, a missing named theme is worth a
        // warning.
        let dir = TempDir::new().unwrap();
        let (t, w) = load_in(dir.path(), "solarized");
        assert_eq!(t, Theme::default());
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("solarized"), "{w:?}");
    }

    #[test]
    fn a_file_shadowing_a_name_amx_ships_is_said_out_loud() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "default", "accent = \"magenta\"\n");

        let (t, w) = load_in(dir.path(), "default");
        assert_eq!(t, Theme::default(), "the shipped one still wins");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("default.toml"), "{w:?}");
    }

    #[test]
    fn the_wrappers_look_for_a_theme_beside_the_config_file() {
        // Reads the real environment without changing it.
        let Ok(themes) = themes_dir() else {
            return;
        };
        assert_eq!(source_in(&themes, "default"), Source::Shipped("default"));

        let mine = source_in(&themes, "solarized");
        let path = mine.path().expect("a name amx does not ship is a file");
        assert!(
            path.ends_with("amx/themes/solarized.toml"),
            "{}",
            path.display()
        );

        // A shipped name loads from the binary whatever is on disk.
        assert_eq!(load("default").0, Theme::default());
    }

    /// Write a theme file, creating the themes directory if needed.
    fn write(themes: &Path, name: &str, text: &str) {
        std::fs::create_dir_all(themes).unwrap();
        std::fs::write(themes.join(format!("{name}.toml")), text).unwrap();
    }
}
