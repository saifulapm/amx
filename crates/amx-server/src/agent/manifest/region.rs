//! The whitelisted region vocabulary, and the screen text it slices.
//!
//! D-M2-3 fixes both halves of this file. The vocabulary is a *whitelist* —
//! `whole_recent`, `bottom_lines(N)`, `bottom_non_empty_lines(N)`, `title` —
//! "so manifest rules cannot drift into whole-scrollback greps", and it starts
//! minimal: herdr's structural prompt-marker regions arrive only if a shipped
//! manifest needs one.
//!
//! What amx does not need at all is herdr's scroll anchor. herdr pins its
//! detection buffer to the scrollback bottom and regression-tests that a
//! scrolled viewport never moves it, because its server owns the scrolled view.
//! amx's published snapshot **is** the live visible grid — scrollback and scroll
//! position are client-side (04 §3) — so there is no anchor to keep and no test
//! to write: the property holds by construction.

use std::fmt;

use thiserror::Error;

/// The largest `N` a counted region may name.
///
/// A screen is a grid, so a count past its height already means "all of it",
/// which `whole_recent` spells. The cap exists so an override manifest cannot
/// name a number that only looks like a region.
pub const MAX_REGION_LINES: u16 = 512;

/// Which part of a pane's screen a rule looks at.
///
/// Bias state rules towards the bottom-anchored variants: herdr's changelog
/// records `whole_recent` matching a stale "esc to interrupt" left in
/// scrollback as a real production bug, and amx inherits the hazard wherever a
/// screen still holds the previous turn's output above the prompt.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Region {
    /// The whole visible grid.
    WholeRecent,
    /// The last `N` lines of it, blank or not.
    ///
    /// Counted after the grid's blank tail has been dropped — see
    /// [`ScreenBuf`], which is where that normalisation lives — so this is the
    /// last `N` lines of *painted* screen rather than `N` empty rows.
    BottomLines(u16),
    /// From the `N`-th non-empty line from the bottom, to the end.
    ///
    /// Contiguous text, not a filter: blank lines *between* the two ends stay
    /// in the region, because a rule matching across a gap in a dialog box has
    /// to see the gap.
    BottomNonEmptyLines(u16),
    /// The pane's title, as the program last set it.
    ///
    /// Fed from pane title state rather than from the grid — herdr's
    /// `osc_title` analog. Agents paint their state into the title (a spinner
    /// while working, a marker when idle), which is the one region that says
    /// something the grid does not.
    Title,
}

impl Region {
    /// The region `spec` names, in the manifest's own spelling.
    ///
    /// # Errors
    ///
    /// [`RegionError`] naming the whole whitelist, because the message a
    /// manifest author sees is the only documentation they are guaranteed to
    /// read.
    pub fn parse(spec: &str) -> Result<Self, RegionError> {
        match spec {
            "whole_recent" => return Ok(Self::WholeRecent),
            "title" => return Ok(Self::Title),
            _ => {}
        }
        if let Some(count) = counted(spec, "bottom_lines") {
            return count.map(Self::BottomLines);
        }
        if let Some(count) = counted(spec, "bottom_non_empty_lines") {
            return count.map(Self::BottomNonEmptyLines);
        }
        Err(RegionError::Unknown {
            spec: spec.to_owned(),
        })
    }

    /// This region's slice of `screen`.
    ///
    /// Borrowed, never built: the detection path runs per damage batch, and a
    /// region that allocated would allocate on every frame of a busy pane.
    #[must_use]
    pub fn slice<'a>(self, screen: &ScreenText<'a>) -> &'a str {
        match self {
            Self::WholeRecent => screen.grid,
            Self::Title => screen.title,
            Self::BottomLines(count) => bottom_lines(screen.grid, count.into()),
            Self::BottomNonEmptyLines(count) => bottom_non_empty_lines(screen.grid, count.into()),
        }
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WholeRecent => f.write_str("whole_recent"),
            Self::Title => f.write_str("title"),
            Self::BottomLines(count) => write!(f, "bottom_lines({count})"),
            Self::BottomNonEmptyLines(count) => write!(f, "bottom_non_empty_lines({count})"),
        }
    }
}

/// Why a region spec is not a region.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum RegionError {
    /// The spec is not in the whitelist.
    #[error(
        "unknown region {spec:?}; the whitelist is whole_recent, bottom_lines(N), \
         bottom_non_empty_lines(N), title"
    )]
    Unknown {
        /// What the manifest said.
        spec: String,
    },
    /// A counted region's `N` was zero, not a number, or past the cap.
    #[error("region {spec:?} needs a line count between 1 and {max}", max = MAX_REGION_LINES)]
    BadCount {
        /// What the manifest said.
        spec: String,
    },
}

/// The `N` of a `name(N)` spec, if `spec` is one at all.
///
/// `None` means "not this region"; `Some(Err(_))` means "this region, spelled
/// wrong", which is the difference between falling through to the next arm and
/// reporting a broken manifest.
fn counted(spec: &str, name: &str) -> Option<Result<u16, RegionError>> {
    let count = spec
        .strip_prefix(name)?
        .strip_prefix('(')?
        .strip_suffix(')')?;
    let bad = || RegionError::BadCount {
        spec: spec.to_owned(),
    };
    Some(
        count
            .parse::<u16>()
            .map_err(|_| bad())
            .and_then(|count| match count {
                1..=MAX_REGION_LINES => Ok(count),
                _ => Err(bad()),
            }),
    )
}

/// The tail of `text` from `count` lines before its end.
fn bottom_lines(text: &str, count: usize) -> &str {
    let start = text.lines().count().saturating_sub(count);
    from_line(text, start)
}

/// The tail of `text` from the `count`-th non-empty line from its end.
///
/// Two forward passes rather than one reverse one, because [`str::lines`] is
/// not double-ended and the alternative is collecting the line indices — an
/// allocation, on the path that runs per damage batch.
fn bottom_non_empty_lines(text: &str, count: usize) -> &str {
    let is_content = |line: &&str| !line.trim().is_empty();
    let total = text.lines().filter(is_content).count();
    if total == 0 {
        // Every line is blank, so there is no `count`-th non-empty one and the
        // region is empty — not the whole screen, which is what returning
        // `text` here would silently make it.
        return "";
    }
    let start = text
        .lines()
        .enumerate()
        .filter(|(_, line)| is_content(line))
        .nth(total.saturating_sub(count))
        .map(|(index, _)| index);
    match start {
        Some(start) => from_line(text, start),
        None => "",
    }
}

/// `text` from the start of line `index`, or `""` if it has no such line.
///
/// Counted in newlines rather than by re-walking [`str::lines`], because the
/// two disagree about a `\r\n` and only this one has the byte offset.
fn from_line(text: &str, index: usize) -> &str {
    if index == 0 {
        return text;
    }
    let mut remaining = index;
    for (offset, byte) in text.bytes().enumerate() {
        if byte != b'\n' {
            continue;
        }
        remaining -= 1;
        if remaining == 0 {
            return &text[offset + 1..];
        }
    }
    ""
}

/// One evaluation's view of a pane: the visible grid, and the title.
///
/// Borrowed from a buffer the caller owns, because tier-2 evaluation is per
/// damage batch. `AgentHub` (V08) keeps one [`ScreenBuf`] per pane, refills it
/// from the snapshot's rows and hands the result here; nothing in this module
/// allocates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScreenText<'a> {
    grid: &'a str,
    title: &'a str,
}

impl<'a> ScreenText<'a> {
    /// A view over `grid` (newline-separated screen rows) and `title`.
    ///
    /// Both may be empty: a pane that has painted nothing and a program that
    /// has set no title are ordinary, and the rules simply do not match.
    #[must_use]
    pub const fn new(grid: &'a str, title: &'a str) -> Self {
        Self { grid, title }
    }

    /// The grid, as the rules see it.
    #[must_use]
    pub const fn grid(self) -> &'a str {
        self.grid
    }

    /// The title, as the rules see it.
    #[must_use]
    pub const fn title(self) -> &'a str {
        self.title
    }
}

/// A reusable buffer that turns a snapshot's rows into a [`ScreenText`].
///
/// Two normalisations happen here and nowhere else, so a fixture on disk and a
/// live pane are the same text:
///
/// - **Trailing whitespace goes, per line.** A terminal row is padded to the
///   grid's width with blank cells; a rule anchored with `$` would never match
///   one.
/// - **The blank tail goes, whole.** The rows below whatever the program
///   painted are grid, not content, and `bottom_lines(3)` that returned three
///   empty rows would be a region vocabulary that only worked on a full screen.
///
/// Interior blank lines are kept: the space between a dialog's question and its
/// options is part of what a rule reads.
#[derive(Clone, Debug, Default)]
pub struct ScreenBuf {
    text: String,
}

impl ScreenBuf {
    /// An empty buffer.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            text: String::new(),
        }
    }

    /// Replace the buffer's contents with `rows`, normalised.
    ///
    /// Reuses the allocation, so a pane evaluated a thousand times allocates
    /// once — the "no per-frame allocations on hot paths" rule of HACKING.md,
    /// kept where the frames actually are.
    pub fn fill<'a>(&mut self, rows: impl IntoIterator<Item = &'a str>) {
        self.text.clear();
        for row in rows {
            self.text.push_str(row.trim_end());
            self.text.push('\n');
        }
        let trimmed = self.text.trim_end_matches('\n').len();
        self.text.truncate(trimmed);
    }

    /// A view over what was last filled in, plus `title`.
    #[must_use]
    pub fn screen<'a>(&'a self, title: &'a str) -> ScreenText<'a> {
        ScreenText::new(&self.text, title)
    }

    /// The normalised grid text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}
