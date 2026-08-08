//! The layout file itself: what it says, how it is read, how it is written.
//!
//! One workspace per `[[workspace]]`, one pane per `[[workspace.pane]]`, in
//! the order a session builds them. The first pane of a workspace is the one
//! `workspace.create` already made; every pane after it names the pane it was
//! split off, which way the cut ran and what share the source kept. That is a
//! *recipe*, not a picture — and it is the same recipe [`build`](super::build)
//! replays through the public verbs, which is why the two directions cannot
//! drift.
//!
//! ```toml
//! version = 1
//!
//! [[workspace]]
//! label = "dev"
//!
//! [[workspace.pane]]
//! label = "edit"
//!
//! [[workspace.pane]]
//! from = 0
//! direction = "vertical"
//! ratio = 0.620
//! agent = "claude"
//! ```
//!
//! # What is deliberately not in it
//!
//! **Session refs.** A layout is a shape, not a conversation (D-M3-11): a ref
//! is machine-specific and secrets-adjacent, and a file you cannot safely mail
//! to a colleague is not a layout file. There is no key for one, so an export
//! cannot leak one by omission of care.
//!
//! **Ids.** Panes address each other by their index in the workspace's own
//! list, so the file is identical whichever session produced it — which is what
//! makes "export → apply → export is the same bytes" a check worth running.
//!
//! **Focus and zoom.** Both are where a person is *looking*, not how the
//! session is *built*; applying a file must not move anybody's focus.
//!
//! # Errors name a line
//!
//! Every refusal carries the line it is about ([`ParseError`]), and every one
//! of them happens before [`super::build::plan`] is called, let alone before a
//! verb is: a half-applied layout is worse than a refused one.

use std::fmt::{self, Display, Formatter, Write as _};
use std::path::PathBuf;

use amx_core::agent::AgentKind;
use amx_proto::control::pane::SplitDirection;
use serde::Deserialize;
use toml::Spanned;

/// The file format version this build reads and writes.
///
/// Bumped only by a change that an older amx could misread. A file from the
/// future is refused by name rather than parsed optimistically, because the
/// failure of a half-understood layout is a session built wrong.
pub const VERSION: u32 = 1;

/// The share of a split the server can actually hold.
///
/// `Layout::resize` clamps to this band, so a file asking for anything outside
/// it would describe a session that cannot exist. Refusing is honest; clamping
/// silently would make apply build something the file does not say.
pub const RATIO_MIN: f32 = 0.05;
/// See [`RATIO_MIN`].
pub const RATIO_MAX: f32 = 0.95;

/// The share a fresh split gives its source pane.
///
/// `pane.split` always cuts in half, so a pane at this ratio needs no `resize`
/// after it and no `ratio` key in the file.
pub const RATIO_DEFAULT: f32 = 0.5;

/// How many decimals a ratio is written with.
///
/// A layout is a shape a person reads, and three decimals is a tenth of a
/// percent of a workspace — far finer than a terminal cell. Rounding here is
/// also what makes the round trip *exact*: apply reaches a ratio by nudging a
/// half-split by a delta, which lands within a rounding error of the written
/// value, and the next export writes the same three decimals back.
const RATIO_DECIMALS: usize = 3;

/// A whole layout file.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct LayoutFile {
    /// Its workspaces, in the order they are built.
    pub workspaces: Vec<WorkspaceSpec>,
}

/// One workspace, and the panes it is built from.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct WorkspaceSpec {
    /// Its label, if it has one.
    pub label: Option<String>,
    /// Its panes, in creation order. The first one is the workspace's root.
    pub panes: Vec<PaneSpec>,
}

/// One pane.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct PaneSpec {
    /// The split that made it, or `None` for a workspace's first pane.
    pub split: Option<SplitSpec>,
    /// Its label, if it has one. An agent pane's label is the agent's name.
    pub label: Option<String>,
    /// The agent to start in it, by registry id.
    pub agent: Option<AgentKind>,
    /// Where to start it. Absent means the same default a split takes.
    pub cwd: Option<PathBuf>,
}

/// The cut that produced a pane.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SplitSpec {
    /// The pane that was split, by index in this workspace's list. Always
    /// earlier than the pane that names it.
    pub from: usize,
    /// Which way the cut ran.
    pub direction: SplitDirection,
    /// The share the *source* pane kept, in [`RATIO_MIN`]..=[`RATIO_MAX`].
    pub ratio: f32,
}

/// Why a layout file could not be read, and where.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParseError {
    /// The line it is about, counted from one.
    ///
    /// `None` only for a failure with no position at all — an empty document
    /// missing its `version`, say.
    pub line: Option<usize>,
    /// What was wrong, in one line.
    pub message: String,
}

impl Display for ParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for ParseError {}

impl ParseError {
    /// A refusal about `span`, resolved against the document it came from.
    fn at(text: &str, span: Option<std::ops::Range<usize>>, message: impl Into<String>) -> Self {
        Self {
            line: span.map(|span| line_of(text, span.start)),
            message: message.into(),
        }
    }
}

/// Which line `offset` falls on, counted from one.
fn line_of(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    1 + text[..offset].bytes().filter(|byte| *byte == b'\n').count()
}

/// Read a layout file.
///
/// The whole document is parsed and validated here, so a caller holding a
/// [`LayoutFile`] holds one that describes a session that can be built.
///
/// # Errors
///
/// [`ParseError`], naming the line: malformed TOML, an unknown key, a version
/// this build does not read, a pane naming a source that does not precede it,
/// or a ratio outside the band the server can hold.
pub fn parse(text: &str) -> Result<LayoutFile, ParseError> {
    let raw: RawFile = toml::from_str(text).map_err(|err| ParseError {
        line: err.span().map(|span| line_of(text, span.start)),
        // The TOML crate's own message already names the line and column; the
        // line is lifted out so every refusal in this module reads the same
        // way, and the rest of the sentence is kept because it is the part
        // that says what was wrong.
        message: strip_position(&err.to_string()),
    })?;

    let (version, version_span) = (raw.version.get_ref(), raw.version.span());
    if *version != VERSION {
        return Err(ParseError::at(
            text,
            Some(version_span),
            format!("this build reads layout version {VERSION}, not {version}"),
        ));
    }

    let mut workspaces = Vec::with_capacity(raw.workspaces.len());
    for workspace in &raw.workspaces {
        workspaces.push(workspace_of(text, workspace.get_ref())?);
    }
    Ok(LayoutFile { workspaces })
}

/// Validate one `[[workspace]]` block.
fn workspace_of(text: &str, raw: &RawWorkspace) -> Result<WorkspaceSpec, ParseError> {
    let mut panes = Vec::with_capacity(raw.panes.len());
    for (index, pane) in raw.panes.iter().enumerate() {
        panes.push(pane_of(text, index, pane)?);
    }
    Ok(WorkspaceSpec {
        label: raw.label.clone(),
        panes,
    })
}

/// Validate one `[[workspace.pane]]` block.
///
/// The rule that makes a pane list a buildable recipe: the first pane is the
/// one the workspace comes with, and every later pane splits one that already
/// exists. A forward reference is refused rather than reordered — a file that
/// has to be sorted before it means anything is a file that means two things.
fn pane_of(text: &str, index: usize, raw: &Spanned<RawPane>) -> Result<PaneSpec, ParseError> {
    let span = raw.span();
    let raw = raw.get_ref();
    let at = |message: String| ParseError::at(text, Some(span.clone()), message);

    let split = match (index, raw.from) {
        (0, None) => None,
        (0, Some(_)) => {
            return Err(at(
                "the first pane of a workspace is the one it is created with; it splits nothing"
                    .to_owned(),
            ));
        }
        (_, None) => {
            return Err(at(format!(
                "pane {index} needs `from`: every pane but the first is split off another"
            )));
        }
        (_, Some(from)) if from >= index => {
            return Err(at(format!(
                "pane {index} splits pane {from}, which it does not follow"
            )));
        }
        (_, Some(from)) => Some(SplitSpec {
            from,
            direction: raw.direction.unwrap_or(SplitDirection::Vertical),
            ratio: ratio_of(raw.ratio, &at)?,
        }),
    };
    if split.is_none() && (raw.direction.is_some() || raw.ratio.is_some()) {
        return Err(at(
            "`direction` and `ratio` describe a split, and the first pane of a workspace is not \
             one"
            .to_owned(),
        ));
    }
    Ok(PaneSpec {
        split,
        label: raw.label.clone(),
        agent: raw.agent.clone(),
        cwd: raw.cwd.clone(),
    })
}

/// Validate a written ratio, or take the default.
fn ratio_of(written: Option<f64>, at: &impl Fn(String) -> ParseError) -> Result<f32, ParseError> {
    let Some(written) = written else {
        return Ok(RATIO_DEFAULT);
    };
    let ratio = written as f32;
    if !ratio.is_finite() || !(RATIO_MIN..=RATIO_MAX).contains(&ratio) {
        return Err(at(format!(
            "a ratio is between {RATIO_MIN} and {RATIO_MAX}, got {written}"
        )));
    }
    Ok(ratio)
}

/// The sentence inside a TOML parse error.
///
/// The crate's own message is a three-part display: a `TOML parse error at
/// line N, column M` header, the offending source line with a caret under it,
/// and then what was actually wrong. [`ParseError`] already carries the
/// position and the caller already has the file, so only the last part is kept
/// — and every refusal in this module then reads the same way whether it came
/// from the parser or from the rules above.
fn strip_position(message: &str) -> String {
    let body: Vec<&str> = message
        .lines()
        .map(str::trim_end)
        .filter(|line| {
            let trimmed = line.trim_start();
            !(trimmed.is_empty()
                || trimmed.starts_with("TOML parse error at")
                // The snippet's own lines: `  |`, `6 | …`, `  | ^^^`.
                || trimmed.starts_with('|')
                || trimmed
                    .split_once(" |")
                    .is_some_and(|(head, _)| head.chars().all(|c| c.is_ascii_digit())))
        })
        .collect();
    // A refusal that is *only* a header still has to say something.
    if body.is_empty() {
        message.trim().to_owned()
    } else {
        body.join(" ")
    }
}

impl Display for LayoutFile {
    /// The file's text form: what `amx layout export` writes.
    ///
    /// Rendered by hand rather than by a serializer so that the key order, the
    /// comment at the top and the omission of every default are the format's
    /// design rather than a library's. Defaults are omitted on purpose: a file
    /// full of `ratio = 0.500` says nothing a reader wants to know.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("# An amx layout: the shape of a session, and nothing it contains.\n")?;
        f.write_str("# Build it with `amx apply <file>`.\n")?;
        writeln!(f, "version = {VERSION}")?;
        for workspace in &self.workspaces {
            f.write_str("\n[[workspace]]\n")?;
            if let Some(label) = &workspace.label {
                writeln!(f, "label = {}", quoted(label))?;
            }
            for pane in &workspace.panes {
                f.write_str("\n[[workspace.pane]]\n")?;
                if let Some(split) = &pane.split {
                    writeln!(f, "from = {}", split.from)?;
                    writeln!(f, "direction = {}", quoted(direction_name(split.direction)))?;
                    let ratio = format!("{:.RATIO_DECIMALS$}", split.ratio);
                    if ratio != format!("{RATIO_DEFAULT:.RATIO_DECIMALS$}") {
                        writeln!(f, "ratio = {ratio}")?;
                    }
                }
                if let Some(label) = &pane.label {
                    writeln!(f, "label = {}", quoted(label))?;
                }
                if let Some(agent) = &pane.agent {
                    writeln!(f, "agent = {}", quoted(agent.as_str()))?;
                }
                if let Some(cwd) = &pane.cwd {
                    writeln!(f, "cwd = {}", quoted(&cwd.to_string_lossy()))?;
                }
            }
        }
        Ok(())
    }
}

/// A split direction, as the wire and the file both spell it.
const fn direction_name(direction: SplitDirection) -> &'static str {
    match direction {
        SplitDirection::Vertical => "vertical",
        SplitDirection::Horizontal => "horizontal",
    }
}

/// `text` as a TOML basic string.
///
/// Everything a basic string must escape, and nothing else: a label holding a
/// quote, a backslash or a newline survives the round trip, and a path with a
/// space is not mangled into two tokens.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            // The rest of the C0 range and DEL have no short escape.
            ch if ch < ' ' || ch == '\u{7f}' => {
                // Writing into a `String` cannot fail.
                let _ = write!(out, "\\u{:04X}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// The document as TOML says it is, before any of the rules above.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    version: Spanned<u32>,
    #[serde(default, rename = "workspace")]
    workspaces: Vec<Spanned<RawWorkspace>>,
}

/// See [`RawFile`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWorkspace {
    #[serde(default)]
    label: Option<String>,
    #[serde(default, rename = "pane")]
    panes: Vec<Spanned<RawPane>>,
}

/// See [`RawFile`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPane {
    #[serde(default)]
    from: Option<usize>,
    #[serde(default)]
    direction: Option<SplitDirection>,
    #[serde(default)]
    ratio: Option<f64>,
    #[serde(default)]
    label: Option<String>,
    /// Validated as it is parsed: `AgentKind` deserializes through its own
    /// constructor, so a kind that reached here is one the registry could name.
    #[serde(default)]
    agent: Option<AgentKind>,
    #[serde(default)]
    cwd: Option<PathBuf>,
}
