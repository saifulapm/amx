//! Reading an agent's screen when its hooks have gone quiet.
//!
//! Hooks win while they flow. When they stop (the vendor was interrupted, or
//! nothing has happened for a while) the pane is the only evidence left, and
//! this matches it against the rules in the vendor's screens document, e.g.
//! `assets/screen-rules.toml`, reading only the bottom of the capture.
//!
//! - The ruleset is small on purpose. A screen no rule claims is `unknown`,
//!   which beats a confident wrong answer: naming a screen also clears any
//!   question off the row.
//! - Every string in a document is that vendor's own, and the document is
//!   picked by the program the agent command runs. No vendor's screen is
//!   spelled out in Rust.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::sync::OnceLock;

use crate::furniture::Furniture;
use crate::registry;
use crate::store::{Phase, Question};

/// How many rows up from the bottom of the capture a rule may see. The chrome
/// is at the bottom; the rest is the agent's output and says nothing about the
/// vendor's state.
pub const FLOOR_LINES: usize = 24;

/// Seconds a screen must hold still before a quiescent rule may end a turn
/// the record says is running.
///
/// The idle screen and a mid-turn pause are the same bytes, so only time
/// tells them apart. The longest mid-turn stillness seen was ten seconds,
/// between the answer finishing and the Stop hook; this is three times that.
/// Readers look once a second, so looks and seconds are the same wait. See
/// [`crate::store::Still`].
pub const SETTLED_LOOKS: u64 = 30;

/// Everything amx can read on one vendor's screens: the rules, in the order
/// they are tried, and the chrome under them.
#[derive(Debug, Deserialize)]
pub struct Ruleset {
    #[serde(default)]
    furniture: Furniture,
    #[serde(default)]
    placeholders: Vec<String>,
    #[serde(default, rename = "rule")]
    rules: Vec<Rule>,
}

/// One screen amx can recognise. The fields are the keys of a `[[rule]]` table.
#[derive(Debug, PartialEq, Eq, Deserialize)]
pub struct Rule {
    /// The screen's name, which `status` reports as its evidence.
    pub name: String,
    /// The phase the screen means.
    pub state: Phase,
    /// Every one of these must appear.
    #[serde(default)]
    pub all: Vec<String>,
    /// At least one of these must appear: the widget that makes prose a
    /// prompt.
    #[serde(default)]
    pub any: Vec<String>,
    /// Most rows the matched anchors may span. A blocking prompt is one box,
    /// not two strings that happen to share a screen.
    #[serde(default)]
    pub within: Option<usize>,
    /// Fewest rows the matched anchors may span.
    ///
    /// Anchors closer than the vendor's chrome is tall have not found all of
    /// it. A box taller than the floor leaves only its bottom border in view,
    /// and no choice of rows spans enough, so the rule refuses it.
    #[serde(default)]
    pub apart: Option<usize>,
    /// None of these may appear below the match. claude draws no composer
    /// under a blocking prompt, so a widget with the mode footer under it is a
    /// quotation of one.
    #[serde(default)]
    pub not_below: Vec<String>,
    /// None of these may appear anywhere in the floor. For screens that say
    /// what they are (a transcript viewer, an overlay in the composer's slot)
    /// but carry the chrome of the screen underneath, which no anchor window
    /// tells apart.
    #[serde(default)]
    pub not: Vec<String>,
    /// Whether every anchor must be on the row directly above the composer,
    /// as found by the furniture walk. That is where a vendor that spins a
    /// line above its box spins it; the same words elsewhere are elision or
    /// agent output.
    #[serde(default)]
    pub over_composer: bool,
    /// Whether `any` anchors count only where they open a row, past indent or
    /// past the rule on a border row with a status drawn into it. The same
    /// glyph mid-row is somebody's text.
    #[serde(default)]
    pub any_opens: bool,
    /// Whether the screen must hold still before this rule may end a running
    /// turn.
    #[serde(default)]
    pub quiescent: bool,
    /// Where this screen keeps its question, when not in the usual place.
    #[serde(default)]
    pub asks: Asks,
    /// The glyph drawn in front of the cursor row, for a vendor that marks a
    /// choice instead of numbering it.
    ///
    /// Without it only numbered choices are read. With it the run of rows
    /// around the mark is the list, and amx numbers it itself (see
    /// [`Screen::marked_below`]) so a person has a key to press.
    #[serde(default)]
    pub marks: Option<String>,
    /// What this screen wants back, which decides what may be sent to it.
    /// Every blocking screen has one; a screen that is a state leaves it out.
    #[serde(default)]
    pub kind: Option<crate::store::Kind>,
    /// Whether the vendor puts this screen in front of the work: a gate only
    /// the person at the keyboard can get past, which no waiting ends.
    ///
    /// Read by `doctor`, so which screens gate a run lives in the vendor's own
    /// document and no verb keeps a list of screen names.
    #[serde(default)]
    pub setup: bool,
}

/// What a ruleset made of a screen.
#[derive(Debug, Clone, PartialEq)]
pub enum Claim<'a> {
    /// This rule claims the screen and may decide.
    Ruled(&'a Rule),
    /// This rule claims the screen, but a turn is on the record as running and
    /// the screen has not held still long enough to end it.
    Unsettled(&'a Rule),
    /// No rule accounts for this screen.
    Unclaimed,
}

#[cfg(test)]
impl Claim<'_> {
    /// The phase to report, if the rule may decide.
    pub fn phase(&self) -> Option<Phase> {
        match self {
            Claim::Ruled(rule) => Some(rule.state),
            _ => None,
        }
    }

    /// The name of the rule that claimed the screen.
    pub fn rule_name(&self) -> Option<&str> {
        match self {
            Claim::Ruled(rule) | Claim::Unsettled(rule) => Some(rule.name.as_str()),
            Claim::Unclaimed => None,
        }
    }
}

/// Every registered vendor's screens, parsed once, keyed by vendor name. A
/// vendor that declares none is absent.
fn parsed() -> &'static [(&'static str, Ruleset)] {
    static PARSED: OnceLock<Vec<(&'static str, Ruleset)>> = OnceLock::new();
    PARSED.get_or_init(|| {
        registry::entries()
            .iter()
            .filter_map(|vendor| {
                let screens = Ruleset::parse(vendor.screens?)
                    .expect("a vendor's screens are part of the binary");
                Some((vendor.name, screens))
            })
            .collect()
    })
}

/// The screens read on the pane of an agent running `agent`.
///
/// An unregistered command reads the default vendor's screens: it is usually
/// a wrapper around that vendor, and every anchor is the vendor's own, so a
/// pane it was not drawn for is claimed by nothing rather than claimed
/// wrongly. A vendor whose screens are unmeasured gets an empty ruleset that
/// claims nothing.
pub fn of(agent: &str) -> &'static Ruleset {
    let vendor = registry::read_as(agent);
    select(parsed(), vendor.map_or("", |vendor| vendor.name)).unwrap_or_else(unmeasured)
}

/// The parsed screens of `vendor`.
fn select<'a>(parsed: &'a [(&'static str, Ruleset)], vendor: &str) -> Option<&'a Ruleset> {
    parsed
        .iter()
        .find(|(name, _)| *name == vendor)
        .map(|(_, screens)| screens)
}

/// An empty ruleset, for a vendor with no measured screens.
fn unmeasured() -> &'static Ruleset {
    static NOTHING: OnceLock<Ruleset> = OnceLock::new();
    NOTHING.get_or_init(|| Ruleset::parse("").expect("no rules at all is a ruleset"))
}

impl Ruleset {
    pub fn parse(text: &str) -> Result<Ruleset> {
        toml::from_str(text).context("reading the screen rules")
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// The chrome this vendor draws under its panes. See [`crate::furniture`].
    pub fn furniture(&self) -> &Furniture {
        &self.furniture
    }

    /// Whether `sentence` is one the vendor sends in place of a question,
    /// saying a dialog is up without saying what it asks.
    ///
    /// Matched whole. The vendor also sends a longer sentence naming the tool,
    /// which a caller can act on.
    pub fn placeholder(&self, sentence: &str) -> bool {
        self.placeholders.iter().any(|said| said == sentence)
    }

    /// Which rule claims the capture, and whether it may decide.
    ///
    /// `recorded` is the phase on file and `held` is how many seconds the
    /// screen has held still; together they decide whether a quiescent rule
    /// may end a turn.
    pub fn claim(&self, capture: &str, recorded: Phase, held: u64) -> Claim<'_> {
        let Some(rule) = self.ruling(capture) else {
            return Claim::Unclaimed;
        };
        if rule.may_decide(recorded, held) {
            Claim::Ruled(rule)
        } else {
            Claim::Unsettled(rule)
        }
    }

    /// What the screen is asking, whatever the record says.
    ///
    /// For a record that says waiting without saying what for. Same rule
    /// order as [`claim`](Ruleset::claim); the quiescence gate does not apply,
    /// since no rule that asks a question is quiescent.
    pub fn asking(&self, capture: &str) -> Option<Question> {
        self.ruling(capture)?.question(capture)
    }

    /// The first rule in document order that holds on the capture. Order
    /// matters: a screen a specific rule names is not also the chrome under
    /// it.
    fn ruling(&self, capture: &str) -> Option<&Rule> {
        let screen = Screen::new(capture);
        self.rules
            .iter()
            .find(|rule| rule.holds(&screen, &self.furniture))
    }
}

impl Rule {
    /// Whether this rule's box is on the screen.
    ///
    /// Every row an anchor is on is a candidate, and the rule holds when some
    /// choice of one row per anchor fits its window. The topmost row alone is
    /// not enough: pi draws an Update Available box above its composer with
    /// the composer's own borders, and anchors that found the notice lost
    /// their window. `apart` still refuses a lone bottom border.
    fn holds(&self, screen: &Screen, furniture: &Furniture) -> bool {
        // A screen that names itself as something else is refused outright.
        if screen.carries_any(&self.not) {
            return false;
        }

        // The one row every anchor must be on, for an `over_composer` rule.
        let over = match self.over_composer {
            true => match furniture.spinner_row(&screen.rows()) {
                Some(row) => Some(row),
                None => return false,
            },
            false => None,
        };
        let placed = |rows: Vec<usize>| -> Vec<usize> {
            rows.into_iter()
                .filter(|&row| over.is_none_or(|over| row == over))
                .collect()
        };

        let mut anchors: Vec<Vec<usize>> = Vec::with_capacity(self.all.len() + 1);
        for needle in &self.all {
            let rows = placed(screen.rows_of(needle));
            if rows.is_empty() {
                return false;
            }
            anchors.push(rows);
        }

        if !self.any.is_empty() {
            // The widget, wherever any of them is. Below it is the rest of
            // the box or, on a quotation, the vendor's chrome that `not_below`
            // looks for.
            let rows: Vec<usize> = self
                .any
                .iter()
                .flat_map(|n| match self.any_opens {
                    true => screen.rows_opening(n, furniture),
                    false => screen.rows_of(n),
                })
                .collect();
            let rows = placed(rows);
            if rows.is_empty() {
                return false;
            }
            anchors.push(rows);
        }

        // A rule with no conditions claims nothing.
        !anchors.is_empty()
            && one_from_each(&anchors)
                .iter()
                .any(|rows| self.fits(rows, screen))
    }

    /// Whether these rows, one per anchor, span no more than `within`, no less
    /// than `apart`, and have nothing from `not_below` under the lowest.
    fn fits(&self, rows: &[usize], screen: &Screen) -> bool {
        let (Some(&first), Some(&last)) = (rows.iter().min(), rows.iter().max()) else {
            return false;
        };
        if let Some(within) = self.within
            && last - first > within
        {
            return false;
        }
        if let Some(apart) = self.apart
            && last - first < apart
        {
            return false;
        }
        !screen.any_below(last, &self.not_below)
    }

    /// The question this screen asks, read off a capture this rule claimed.
    ///
    /// Only a `waiting` rule asks anything. Where the question sits is the
    /// rule's [`Asks`].
    ///
    /// With [`marks`](Rule::marks), the list is the run of rows around the
    /// mark and the question is the sentence above the run, not above the
    /// mark: a person can move the cursor before amx looks.
    pub fn question(&self, capture: &str) -> Option<Question> {
        if self.state != Phase::Waiting {
            return None;
        }

        let screen = Screen::new(capture);
        let marked = self
            .marks
            .as_deref()
            .and_then(|mark| Some((screen.run_of(mark)?, mark)))
            // A run the vendor numbered is read by its numbers even under a
            // mark: they are keys a caller can press. claude drew a cursor over
            // a numbered list until 2.1.259.
            .filter(|&(run, _)| !screen.numbered(run));
        let choices = marked
            .map(|(run, _)| run.0)
            .or_else(|| screen.first_option());
        let (from, to) = match (&self.asks, marked) {
            (Asks::Sentence(anchor), _) => screen.sentence_at(screen.row_above(choices, anchor)?),
            // A screen asking above its anchor numbers no choices, so a
            // numbered row higher up is agent output, not a ceiling.
            (Asks::Above(anchor), None) => {
                screen.sentence_above(screen.row_above(None, anchor)?)?
            }
            // On a marked screen the run's first row is the anchor row.
            (Asks::Above(_) | Asks::AboveOptions, _) => screen.sentence_above(choices?)?,
        };

        let text = screen.joined(from, to);
        let (options, at) = match marked {
            Some((run, mark)) => screen.marked_below(run, mark),
            None => (screen.options_below(to), None),
        };
        (!text.is_empty()).then(|| Question {
            // Only a list amx numbered itself is walked.
            walked: marked.is_some() && !options.is_empty(),
            marked: at,
            options,
            text,
        })
    }

    /// Whether this rule may decide, given the recorded phase.
    ///
    /// A quiescent rule may end a turn only after [`SETTLED_LOOKS`] seconds of
    /// stillness. From `starting` or `unknown` there is no turn to end, so it
    /// decides at once; that is what names a parked agent.
    fn may_decide(&self, recorded: Phase, held: u64) -> bool {
        if !self.quiescent {
            return true;
        }
        match recorded {
            // Nothing outstanding, so no turn to end.
            Phase::Starting | Phase::Unknown => true,
            _ => held >= SETTLED_LOOKS,
        }
    }
}

/// Where a claimed screen keeps its question.
///
/// Blocking screens differ, so each rule says which reading its screen needs:
/// `asks = { sentence = "do you want to" }` or `asks = { above = "→" }`, or
/// nothing for the default.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Asks {
    /// The sentence ending just above the first numbered choice. The default,
    /// and the likeliest place on a screen not met yet.
    #[default]
    AboveOptions,
    /// The whole sentence, wrap included, that the lowest row carrying this
    /// string belongs to. For screens that draw something under the question:
    /// the tool a request is about, a note on what the vendor may do, a link.
    Sentence(String),
    /// The sentence ending just above the lowest row carrying this string.
    /// [`Asks::AboveOptions`] for a screen whose choices are not numbered: the
    /// string is the selection glyph or the input row.
    Above(String),
}

/// Every way of taking one row from each list, in order.
///
/// Small in practice: a rule has at most three anchors, and each has at most
/// [`FLOOR_LINES`] rows to be on.
fn one_from_each(lists: &[Vec<usize>]) -> Vec<Vec<usize>> {
    lists.iter().fold(vec![Vec::new()], |chosen, rows| {
        chosen
            .iter()
            .flat_map(|prefix| {
                rows.iter().map(move |&row| {
                    let mut next = prefix.clone();
                    next.push(row);
                    next
                })
            })
            .collect()
    })
}

/// The rows a rule may see: the bottom [`FLOOR_LINES`] of the capture,
/// case-folded for matching and as drawn for reading questions.
struct Screen {
    folded: Vec<String>,
    shown: Vec<String>,
}

impl Screen {
    fn new(capture: &str) -> Screen {
        let all: Vec<&str> = capture.lines().collect();
        let floor = all.len().saturating_sub(FLOOR_LINES);
        Screen {
            folded: all[floor..].iter().map(|row| row.to_lowercase()).collect(),
            shown: all[floor..].iter().map(|row| row.to_string()).collect(),
        }
    }

    /// Every row carrying `needle`, top to bottom.
    fn rows_of(&self, needle: &str) -> Vec<usize> {
        self.folded
            .iter()
            .enumerate()
            .filter(|(_, row)| row.contains(needle))
            .map(|(at, _)| at)
            .collect()
    }

    /// Every row `needle` opens, past indent and past the rule on a border
    /// row, top to bottom.
    fn rows_opening(&self, needle: &str, furniture: &Furniture) -> Vec<usize> {
        self.folded
            .iter()
            .enumerate()
            .filter(|(_, row)| furniture.unruled(row).starts_with(needle))
            .map(|(at, _)| at)
            .collect()
    }

    /// The rows as drawn, for the furniture walk.
    fn rows(&self) -> Vec<&str> {
        self.shown.iter().map(String::as_str).collect()
    }

    /// Whether any of `needles` is on any row.
    fn carries_any(&self, needles: &[String]) -> bool {
        self.folded
            .iter()
            .any(|row| needles.iter().any(|needle| row.contains(needle)))
    }

    /// Whether any of `needles` is on a row below `row`.
    fn any_below(&self, row: usize, needles: &[String]) -> bool {
        self.folded
            .iter()
            .skip(row + 1)
            .any(|line| needles.iter().any(|needle| line.contains(needle)))
    }

    /// The row the choices start on: the lowest first choice, since agent
    /// output above a blocking screen often has numbered lists of its own.
    fn first_option(&self) -> Option<usize> {
        self.shown
            .iter()
            .rposition(|row| matches!(option_on(row), Some((1, _))))
    }

    /// The lowest row above the choices that carries `needle`.
    fn row_above(&self, choices: Option<usize>, needle: &str) -> Option<usize> {
        let ceiling = choices.unwrap_or(self.folded.len());
        self.folded[..ceiling]
            .iter()
            .rposition(|row| row.contains(needle))
    }

    /// The rows of the sentence that `row` belongs to.
    fn sentence_at(&self, row: usize) -> (usize, usize) {
        let mut from = row;
        while from > 0 && wrapped(&self.shown[from]) && content(&self.shown[from - 1]) {
            from -= 1;
        }

        let mut to = row;
        while to + 1 < self.shown.len()
            && content(&self.shown[to + 1])
            && option_on(&self.shown[to + 1]).is_none()
        {
            to += 1;
        }
        (from, to)
    }

    /// The rows of the sentence ending above the choices.
    fn sentence_above(&self, choices: usize) -> Option<(usize, usize)> {
        let mut to = choices.checked_sub(1)?;
        while !content(&self.shown[to]) {
            to = to.checked_sub(1)?;
        }

        let mut from = to;
        while from > 0 && content(&self.shown[from - 1]) {
            from -= 1;
        }
        Some((from, to))
    }

    /// Rows `from` to `to` joined back into one sentence.
    fn joined(&self, from: usize, to: usize) -> String {
        self.shown[from..=to]
            .iter()
            .map(|row| row.trim())
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string()
    }

    /// The choices under `row`, in order.
    ///
    /// Only rows numbered one, two, three and on in sequence count, so a
    /// description under a label, a rule through the list or stray prose
    /// cannot join in.
    fn options_below(&self, row: usize) -> Vec<String> {
        let mut options: Vec<String> = Vec::new();
        for line in self.shown.iter().skip(row + 1) {
            if let Some((number, label)) = option_on(line)
                && number == options.len() + 1
            {
                options.push(label.to_string());
            }
        }
        options
    }

    /// The run of non-blank rows around the lowest row carrying `mark`.
    ///
    /// The lowest, for the same reason as [`first_option`](Screen::first_option).
    /// Blank rows and the box's rule end the run.
    fn run_of(&self, mark: &str) -> Option<(usize, usize)> {
        let at = self.shown.iter().rposition(|row| row.contains(mark))?;

        let mut from = at;
        while from > 0 && content(&self.shown[from - 1]) {
            from -= 1;
        }

        let mut to = at;
        while to + 1 < self.shown.len() && content(&self.shown[to + 1]) {
            to += 1;
        }
        Some((from, to))
    }

    /// Whether the rows in `run` carry the vendor's own numbers.
    fn numbered(&self, (from, to): (usize, usize)) -> bool {
        self.shown[from..=to]
            .iter()
            .any(|row| matches!(option_on(row), Some((1, _))))
    }

    /// The choices in `run`, and which of them (from one) carries the mark.
    ///
    /// A row whose words start at the marked row's label column is a choice;
    /// one starting further left is the wrapped rest of the choice above. A
    /// row opening in lower case is also a wrap: claude hangs the wrap at the
    /// label's column on the 24-column trust gate (`Yes, I trust this` over
    /// `folder`), and labels start with a capital.
    fn marked_below(&self, run: (usize, usize), mark: &str) -> (Vec<String>, Option<usize>) {
        let (from, to) = run;
        let rows = &self.shown[from..=to];
        let Some(column) = rows
            .iter()
            .filter_map(|row| column_of(row, mark))
            .next_back()
        else {
            return (Vec::new(), None);
        };

        let mut options: Vec<String> = Vec::new();
        let mut marked = None;
        for row in rows {
            let (at, label) = labelled(row, mark);
            if label.is_empty() {
                continue;
            }
            if at >= column + 2 && !wrapped(row) {
                options.push(label.to_string());
                if row.contains(mark) {
                    marked = Some(options.len());
                }
            } else if let Some(above) = options.last_mut() {
                above.push(' ');
                above.push_str(label);
            }
        }
        (options, marked)
    }
}

/// The character column `mark` is drawn at.
fn column_of(row: &str, mark: &str) -> Option<usize> {
    row.find(mark).map(|at| row[..at].chars().count())
}

/// Where a row's words start, in characters, and the words: the row trimmed,
/// with a leading mark and the space after it counted as indent.
fn labelled<'a>(row: &'a str, mark: &str) -> (usize, &'a str) {
    let words = row.trim_start();
    let mut at = row.chars().count() - words.chars().count();

    let words = match words.strip_prefix(mark) {
        Some(rest) => {
            let label = rest.trim_start();
            at += mark.chars().count() + rest.chars().count() - label.chars().count();
            label
        }
        None => words,
    };
    (at, words.trim_end())
}

/// One numbered choice as drawn: `❯ 1. Yes` under the cursor, `  2. No`
/// otherwise.
///
/// A wrapped label is read to the end of its own row only. The rows under it
/// are where menus put descriptions, at the same indent.
fn option_on(row: &str) -> Option<(usize, &str)> {
    let row = row.trim_start();
    let row = row.strip_prefix('❯').map_or(row, str::trim_start);
    let (number, label) = row.split_once(". ")?;
    let number = number.parse().ok()?;
    let label = label.trim();
    (!label.is_empty()).then_some((number, label))
}

/// Whether a row has anything on it but a rule.
fn content(row: &str) -> bool {
    let row = row.trim();
    !row.is_empty() && !row.chars().all(|glyph| glyph == '─' || glyph == '-')
}

/// Whether a row continues the one above: wrapped prose, since the vendor's
/// sentences start with a capital.
fn wrapped(row: &str) -> bool {
    row.trim_start()
        .chars()
        .next()
        .is_some_and(|glyph| glyph.is_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vendor::second::SECOND;
    use crate::vendor::{claude, pi};

    // claude screens, captured off a live vendor at the version and width
    // named with each.

    /// A finished turn at the prompt: claude's summary line is still on the
    /// transcript. v2.1.226, auto mode.
    const IDLE_SCREEN: &str = "\
  It ran for the full 40 seconds and exited cleanly.

✻ Worked for 2m 26s

──────────────────────────────────────── amx ──
❯
────────────────────────────────────────────────
  Opus 5 (1M context) │ ◈ 4% │ fix-login-a1b (amx/fix-login-a1b) │ ◖ xhigh
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    /// The same screen in manual mode at 220 columns. The footer carries no
    /// cycle hint in this mode at any width.
    const IDLE_SCREEN_MANUAL: &str = "\
● I'll run that command.
  Ran 1 shell command

✻ Cooked for 1m 23s

────────────────────────────────────── round5 ──
❯
────────────────────────────────────────────────
  Opus 5 (1M context) │ ◈ 2% │ scratch2 (HEAD*) │ ◖ xhigh
  ⏸ manual mode on · ← for agents
";

    /// Mid-turn while an answer streams. claude drops its spinner line while
    /// output flows, so with control characters stripped this differs from
    /// the idle screen only in the transcript.
    const STREAMING_SCREEN: &str = "\
  5. BBR (2016): Loss Is the Wrong Signal

  Tahoe, Reno and CUBIC share an assumption: loss means congestion.

  First, bufferbloat. Router buffers grew to hundreds of milliseconds.

──────────────────────────────────────── amx ──
❯
────────────────────────────────────────────────
  Opus 5 (1M context) │ ◈ 2% │ fix-login-a1b (amx/fix-login-a1b) │ ◖ xhigh
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    /// A boot left alone: the welcome box, an empty prompt and blank rows
    /// between them.
    const PARKED_SCREEN: &str = "\
╭─── Claude Code v2.1.226 ─────────────────────╮
│             Welcome back Saiful Islam!       │
│   Opus 5 (1M context) with xhig… · Claude    │
│          /…/repo/.amx/worktrees/agent-ivu    │
╰──────────────────────────────────────────────╯




──────────────────────────────────────── amx ──
❯
────────────────────────────────────────────────
  Opus 5 (1M context) │ ◈ 0% │ agent-ivu (amx/agent-ivu) │ ◖ xhigh
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    /// Mid-turn with the spinner up, during a Bash call. The mode footer is on
    /// this screen too, so rule order decides, not the footer.
    const WORKING_SCREEN: &str = "\
  Now running the command you asked for:
● Running 1 shell command · 12s…
  ⎿  $ bash -c \"sleep 40; echo finished-sleeping\" (10s)
✢ Infusing… (1m 54s · ↓ 6.9k tokens)
  ⎿  Tip: Did you know you can drag and drop image files into your terminal?
──────────────────────────────────────── amx ──
❯
────────────────────────────────────────────────
  Opus 5 (1M context) │ ◈ 3% │ fix-login-a1b (amx/fix-login-a1b) │ ◖ xhigh
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    /// The same turn thinking rather than running a tool: the other spinner
    /// wording.
    const THINKING_SCREEN: &str = "\
✽ Nesting… (15s · still thinking with xhigh effort)
──────────────────────────────────────── amx ──
❯
────────────────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    /// A running turn at 30 columns, v2.1.259. claude truncates the spinner
    /// row from the right, and the whole parenthesis is gone. The mode footer
    /// is under it, as under every running turn.
    const NARROW_TURN_30: &str = "\
● Finagling… thinking
──────────────────────────────
❯
──────────────────────────────
  ⏵⏵ auto mode on
";

    /// The same turn at 24 columns: the parenthesis is back and its tail after
    /// the elapsed time is gone.
    const NARROW_TURN_24: &str = "\
● Finagling… (2s)
────────────────────────
❯
────────────────────────
  ⏵⏵ auto mode on
";

    /// The idle screen at 40 columns, v2.1.259: the line the finished turn
    /// left, the composer, the elided statusline and the footer. It carries
    /// both an ellipsis and `s · `, so neither alone identifies the spinner.
    const IDLE_SCREEN_259_40: &str = "\
✻ Cogitated for 2m 6s · done 10:09 AM

───────────────────── execute t1 brief ─
❯
────────────────────────────────────────
  Opus 5 (1M context) (1M context) │ …
  ⏵⏵ auto mode on (shift+tab to cycle)
";

    // claude 2.1.270 screens at 100, 54, 40, 30 and 24 columns, each from the
    // top of the screen it is about down to the bottom of the pane. Trailing
    // spaces are trimmed.

    /// The transcript viewer (ctrl+o) at 100 columns, claude 2.1.270. It has
    /// its own footer and no mode row, so the only anchor a document had on it
    /// was the older `? for shortcuts` hint, and it read as idle mid-turn.
    const TRANSCRIPT_270_100: &str = "\
────────────────────────────────────────────────────────────────────────────────────────────────────
  Showing detailed transcript · ctrl+o to toggle · ? for shortcuts                          verbose
";

    /// The same viewer at 54 columns. The footer truncates from the middle, so
    /// only the fragment the row opens with survives every width.
    const TRANSCRIPT_270_54: &str = "\
──────────────────────────────────────────────────────
  Showing detailed transcript · ctrl+o to togg…verbose
";

    /// At 40 columns: the middle of the footer is gone.
    const TRANSCRIPT_270_40: &str = "\
────────────────────────────────────────
  Showing detailed transcript · …verbose
";

    /// At 30 columns: the first fragment is truncated too, and the last letter
    /// of `verbose` wraps onto its own row.
    const TRANSCRIPT_270_30: &str = "\
──────────────────────────────
  Showing detailed tran…verbos
                        e
";

    /// At 24 columns: only `Showing detaile` is left.
    const TRANSCRIPT_270_24: &str = "\
────────────────────────
  Showing detaile…verbos
                  e
";

    /// The /btw overlay after it answered, at 100 columns. claude draws it in
    /// the composer's slot with no mode row under it.
    ///
    /// On the pane it came from, the turn's `✻ Waiting for 1 background agent
    /// to finish` sat eleven rows above, inside the floor at 100, 54 and 40
    /// columns. Only the overlay is kept here; whether an overlay should
    /// outrank the turn behind it is undecided.
    const BTW_ANSWERED_270_100: &str = "\
▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔

    /btw how many files are in this folder

      I can't tell you — that needs an actual directory listing, and I have no tools available in
      a side question.

      Nothing in the conversation so far has enumerated the contents of /tmp/measure-270. Ask in
      the main conversation and it can run ls there.

    ↑/↓ to scroll · c to copy · f to fork · Esc to close
";

    /// The same overlay while it works. `· Answering…` is not on the row over
    /// a composer, so the spinner rule does not claim it.
    const BTW_ANSWERING_270_100: &str = "\
▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔

    /btw how many files are in this folder

      · Answering…

    Esc to close
";

    /// A turn that answered and waits on a background subagent, at 100
    /// columns. The Stop hook has fired, but the line above the composer says
    /// the turn goes on. The mode row and an agents panel are under it.
    ///
    /// A finished turn's line also opens with `✻`, so the words after the
    /// glyph are the anchor.
    const BACKGROUND_270_100: &str = "\
✻ Waiting for 1 background agent to finish
                                                                                  ● high · /effort
────────────────────────────────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────────────────────────────────
  Opus 5 (1M context) (1M context) │ ◈ 2% │ measure-270 │ ◖ high
  ⏵⏵ auto mode on (shift+tab to cycle) · ← 2 agents

  ● main
  ◯ general-purpose  Preparing to run `sleep 45`                              48s · ↓ 10.2k tokens
";

    /// At 54 columns. The agents panel elides its subagent's label to
    /// `Preparing…`, the spinner rule's anchor, but under the footer rather
    /// than over the composer, so the background rule names the screen.
    const BACKGROUND_270_54: &str = "\
✻ Waiting for 1 background agent to finish
                                    ● high · /effort
──────────────────────────────────────────────────────
❯
──────────────────────────────────────────────────────
  Opus 5 (1M context) (1M context) │ ◈ 2% │ measure…
  ⏵⏵ auto mode on (shift+tab to cycle) · ← 2 agents

  ● main
  ◯ general-purpose  Preparing… 49s · ↓ 10.2k tokens
";

    /// At 40 columns: the line wraps after `to`.
    const BACKGROUND_270_40: &str = "\
✻ Waiting for 1 background agent to
  finish
                      ● high · /effort
────────────────────────────────────────
❯
────────────────────────────────────────
  Opus 5 (1M context) (1M context) │ …
  ⏵⏵ auto mode on (shift+tab to cycle)

  ● main
  ◯ general-purpose 50s · ↓ 10.2k tokens
";

    /// At 30 columns: it wraps after `background`.
    const BACKGROUND_270_30: &str = "\
✻ Waiting for 1 background
  agent to finish
            ● high · /effort
──────────────────────────────
❯
──────────────────────────────
  Opus 5 (1M context) (1M c…
  ⏵⏵ auto mode on (shift+tab

  ● main
  ◯ general-purpose 51s · ↓
";

    /// At 24 columns: it wraps after `1` and takes three rows. One row is the
    /// widest the two anchors were measured apart.
    const BACKGROUND_270_24: &str = "\
✻ Waiting for 1
  background agent to
  finish
      ● high · /effort
────────────────────────
❯
────────────────────────
  Opus 5 (1M context)…
  ⏵⏵ auto mode on

  ● main
  ◯ general-purpose 51s
";

    /// The folder-trust screen, v2.1.226, 220 columns.
    const TRUST_SCREEN_220: &str = "\
────────────────────────────────────────────────
 Accessing workspace:

 /tmp/amx-repo/repo/.amx/worktrees/fix-login-a1b

 Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source project, or work from your team). If not, take a moment to review what's in this folder first.

 Claude Code'll be able to read, edit, and execute files here.

 Security guide

 ❯ 1. Yes, I trust this folder
   2. No, exit

 Enter to confirm · Esc to cancel
";

    /// The same screen at 54 columns, as on a wall of five tiled agents. The
    /// sentence wraps across four rows, breaking between `you` and `trust`.
    const TRUST_SCREEN_54: &str = "\
──────────────────────────────────────────────────────
 Accessing workspace:

 /tmp/amx-repo/repo/.amx/worktrees/fix-login-a1b

 Quick safety check: Is this a project you created or
 one you trust? (Like your own code, a well-known
 open source project, or work from your team). If
 not, take a moment to review what's in this folder
 first.

 Claude Code'll be able to read, edit, and execute
 files here.

 Security guide

 ❯ 1. Yes, I trust this folder
   2. No, exit

 Enter to confirm · Esc to cancel
";

    /// The same screen, v2.1.259 at 54 columns. The wording is unchanged from
    /// 2.1.240, but the choices lost their numbers and swapped places: the
    /// only `❯` is the cursor on the exit, and the confirm footer is the only
    /// widget.
    const TRUST_SCREEN_259_54: &str = "\
──────────────────────────────────────────────────────
 Accessing workspace:

 /home/saiful/.claude/jobs/dfc82656/tmp/scratch2

 Quick safety check: Is this a project you created or
 one you trust? (Like your own code, a well-known
 open source project, or work from your team). If
 not, take a moment to review what's in this folder
 first.

 Claude Code'll be able to read, edit, and execute
 files here.

 Security guide

 ❯ No, exit
   Yes, I trust this folder

 Enter to confirm · Esc to cancel
";

    /// The same screen at 24 columns on a 30-row pane. The box is taller than
    /// the pane, so the floor starts on the question's first row. Nineteen
    /// rows separate the topmost `trust` from the confirm footer, which sets
    /// this rule's `within`. A raw string because it opens with a blank row.
    const TRUST_SCREEN_259_24: &str = r"
 /home/saiful/.claude/j
 obs/dfc82656/tmp/scrat
 ch2

 Quick safety check: Is
 this a project you
 created or one you
 trust? (Like your own
 code, a well-known
 open source project,
 or work from your
 team). If not, take a
 moment to review
 what's in this folder
 first.

 Claude Code'll be able
 to read, edit, and
 execute files here.

 Security guide

 ❯ No, exit
   Yes, I trust this
   folder

 Enter to confirm · Esc
 to cancel
";

    /// The same screen, v2.1.276 at 220 columns, on a folder with no saved
    /// decision. Unchanged since 2.1.259; the same version at 54 and 24
    /// columns matched the two captures above row for row.
    const TRUST_SCREEN_276_220: &str = "\
────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────
 Accessing workspace:

 /tmp/amx-trust-276b

 Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source project, or work from your team). If not, take a moment to review what's in this folder first.

 Claude Code'll be able to read, edit, and execute files here.

 Security guide

 ❯ No, exit
   Yes, I trust this folder

 Enter to confirm · Esc to cancel
";

    /// The plan approval screen ExitPlanMode draws, v2.1.237 at 220 columns.
    const PLAN_APPROVAL_220: &str = "\
────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────
 Claude has written up a plan and is ready to execute. Would you like to proceed?

 ❯ 1. Yes, and use auto mode
   2. Yes, manually approve edits
   3. Tell Claude what to change
      shift+tab to approve with this feedback

 ctrl+g to edit in Kak · ~/.claude/plans/write-a-one-paragraph-plan-snug-russell.md
";

    /// The same screen at 54 columns. The message wraps between `to` and
    /// `execute.`, so `ready to execute` cannot be an anchor.
    const PLAN_APPROVAL_54: &str = "\
──────────────────────────────────────────────────
 Claude has written up a plan and is ready to
 execute. Would you like to proceed?

 ❯ 1. Yes, and use auto mode
   2. Yes, manually approve edits
   3. Tell Claude what to change
      shift+tab to approve with this feedback

 ctrl+g to edit in Kak · ~/.claude/plans/write-a-
 one-paragraph-plan-snug-russell.md
";

    /// The same screen at 24 columns, wrapping between `to` and `proceed?`, so
    /// `would you like to proceed` cannot be one either. Only single words
    /// survive both widths.
    const PLAN_APPROVAL_24: &str = "\
────────────────────
 Claude has written
 up a plan and is
 ready to execute.
 Would you like to
 proceed?

 ❯ 1. Yes, and use
      auto mode
   2. Yes, manually
      approve edits
   3. Tell Claude
      what to change
      shift+tab to
      approve with
      this feedback

 ctrl+g to edit in
 Kak · ~/.claude/pl
 ans/write-a-one-pa
 ragraph-plan-snug-
 russell.md
";

    /// A permission box, v2.1.226 at 220 columns, with manual permissions and
    /// an ask rule for Bash. A full-width rule with the request under it and
    /// no mode footer.
    const PERMISSION_BOX: &str = "\
────────────────────────────────────────────────
 Bash command
   rm -f b.txt
   Remove b.txt
 Permission rule Bash requires confirmation for this command.
 /permissions to update rules
 Do you want to proceed?
 ❯ 1. Yes
   2. No
 Esc to cancel · Tab to amend · ctrl+e to explain
";

    /// The AskUserQuestion menu at 80 columns, v2.1.229.
    const ASK_MENU_80: &str = "\
────────────────────────────────────────────────────────────────────────────────
 ☐ Indentation

Should this project be indented with spaces or tabs?

❯ 1. Spaces
     Indent with spaces (most common default across JS/TS, PHP/Laravel, and
     Python codebases).
  2. Tabs
     Indent with tab characters — accessible, since each reader can set their
     own display width.
  3. Type something.
────────────────────────────────────────────────────────────────────────────────
  4. Chat about this

Enter to select · ↑/↓ to navigate · Esc to cancel
";

    /// The same menu at 24 columns. The footer wraps, so `esc to cancel` is
    /// not contiguous, and eighteen rows separate the marker from the footer.
    const ASK_MENU_24: &str = "\
────────────────────────
 ☐ Indentation

Should this project be
indented with spaces or
tabs?

❯ 1. Spaces
     Indent with spaces
     (most common
     default across
     JS/TS, PHP/Laravel,
     and Python
     codebases).
  2. Tabs
     Indent with tab
     characters —
     accessible, since
     each reader can set
     their own display
     width.
  3. Type something.
────────────────────────
  4. Chat about this

Enter to select · ↑/↓ to
navigate · Esc to
cancel
";

    /// The same menu, v2.1.259 at 24 columns on a 30-row pane. Two short
    /// descriptions make the box taller than the floor: `❯ 1. Spaces` is the
    /// sixth row and the floor starts on the seventh, and the footer wraps in
    /// three. Only the bottom of the box is in view. A raw string because it
    /// opens with a blank row.
    const ASK_MENU_259_24: &str = r"
Should this project be
indented with spaces or
tabs?

❯ 1. Spaces
     Fixed-width
     indentation that
     renders identically
     everywhere; the
     common default for
     most language style
     guides and
     formatters.
  2. Tabs
     One tab character
     per level, so each
     reader's editor
     controls the
     visible width;
     better for
     accessibility and
     smaller files.
  3. Type something.
────────────────────────
  4. Chat about this

Enter to select · ↑/↓ to
navigate · Esc to
cancel
";

    /// An agent quoting another agent's pane back as a tool result, as amx's
    /// callers have agents do. The quotation is the widget, character for
    /// character.
    const QUOTED_PERMISSION_BOX: &str = "\
  I read the other agent's pane and it is asking this:

  ╭──────────────────────────────────────────╮
  │ Bash command                             │
  │ Do you want to proceed?                  │
  │ ❯ 1. Yes                                 │
  │   2. No, and tell Claude what to do      │
  ╰──────────────────────────────────────────╯
    esc to cancel

  I will answer it once you say which.

──────────────────────────────────────── amx ──
❯
────────────────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    /// The same quotation under a manual-mode footer. The cycle hint is absent
    /// in this mode; the glyph is not.
    const QUOTED_BOX_UNDER_A_MANUAL_FOOTER: &str = "\
❯ Print this block back to me verbatim inside a fenced code block.

● ╭────────────╮
  │ Bash command │
  │ rm -rf build │
  │ Do you want to proceed? │
  │ ❯ 1. Yes │
  │   2. No │
  ╰────────────╯
  Esc to cancel

────────────────────────────────────── round5 ──
❯
────────────────────────────────────────────────
  Opus 5 (1M context) │ ◈ 2% │ scratch2 (HEAD*) │ ◖ xhigh
  ⏸ manual mode on · ← for agents
";

    /// An ordinary answer with `do you want to` above a markdown numbered
    /// list. The `❯` on its own row is the composer, which is on every claude
    /// screen.
    const PROSE_THAT_LOOKS_LIKE_A_QUESTION: &str = "\
  Here is the plan I would follow.

  1. Add the parser
  2. Wire the CLI

  Do you want to proceed with this plan, or should I keep going?

──────────────────────────────────────── amx ──
❯
────────────────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    /// The same shape for the trust rule: an answer discussing trust.
    const PROSE_ABOUT_TRUST: &str = "\
  The lockfile is only as good as the registry you trust, so I would
  pin the digest rather than the tag.

  1. Pin the digest
  2. Leave the tag

──────────────────────────────────────── amx ──
❯
────────────────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    /// One plain sentence carrying both of the trust rule's fragments on one
    /// row. No string anchor can tell it from a prompt.
    const PROSE_WITH_A_CONFIRM_FOOTER_IN_IT: &str = "\
  Pick the folder you trust, press Enter to confirm, and it takes care of
  the rest.

──────────────────────────────────────── amx ──
❯
────────────────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    /// A numbered plan staged in the composer, as `send` leaves it when the
    /// paste lands and the submit does not. The composer row is `❯ 1. …`, the
    /// widget's own option shape, and carries the anchor word.
    const A_PLAN_STAGED_IN_THE_COMPOSER: &str = "\
  Working through the migration now.

──────────────────────────────────────── amx ──
❯ 1. Update the trust store before the rollout
────────────────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    /// The auto-mode footer on a 40-column pane. claude truncates its own
    /// hint from the right.
    const FOOTER_AUTO_40: &str = "\
  Ran the migration.

──────────────────────────────────────
❯
──────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to      ·
";

    /// The second vendor stopped on a question, as its document describes: the
    /// anchor on the question's row and choices numbered with no cursor glyph.
    const A_SECOND_VENDOR_ASKING: &str = "\
 pick one and I will carry on
 about the file you named
 1. keep it
 2. drop it
 answer with a number
";

    /// An ordinary shell, as a pane shows once the vendor has exited.
    const A_SHELL: &str = "\
$ ls
Cargo.toml  README.md  src  tests
$
";

    // pi screens, 0.84.4 unless marked 0.85.1, at the width named with each.
    // Dialogs are raised by an extension gating the bash tool with
    // `ctx.ui.select`, which is how a caller asks pi a question. Trailing
    // spaces are trimmed. Raw strings, because a trailing `\` would eat the
    // leading spaces and blank rows these captures carry. Re-measure on every
    // vendor bump; see `assets/screen-rules-pi.toml`.

    /// A fresh pi at 100 columns: the banner, the box, and a stats line with
    /// only the context window on it. pi pads the rest of the pane.
    const A_PI_BOOT: &str = r"
 pi v0.84.4
 escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more
 Press ctrl+o to show full startup help and loaded resources.

 Pi can explain its own features and look up its docs. Ask it how to use or extend Pi.


[Context]
  ~/.claude/CLAUDE.md

[Extensions]
  gate2.js

[Themes]
  qshell


────────────────────────────────────────────────────────────────────────────────────────────────────

────────────────────────────────────────────────────────────────────────────────────────────────────
~/.claude/jobs/eef72778/tmp/pipane
$0.000 (sub) 0.0%/264k (auto)                                  (github-copilot) gpt-5-mini • minimal







";

    /// The same pane mid-turn. pi spins its line above the box for the whole
    /// turn.
    const A_PI_WORKING: &str = r"
 pi v0.84.4
 escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more
 Press ctrl+o to show full startup help and loaded resources.

 Pi can explain its own features and look up its docs. Ask it how to use or extend Pi.


[Context]
  ~/.claude/CLAUDE.md

[Extensions]
  gate2.js

[Themes]
  qshell


 Run this exact bash command and nothing else: echo hi


 ⠼ Working...

────────────────────────────────────────────────────────────────────────────────────────────────────

────────────────────────────────────────────────────────────────────────────────────────────────────
~/.claude/jobs/eef72778/tmp/pipane
$0.000 (sub) 0.0%/264k (auto)                                  (github-copilot) gpt-5-mini • minimal


";

    /// A running turn on pi 0.85.1 at 100 columns. The working indicator is in
    /// the composer's top border (`── `, the frame, the message, the rule) and
    /// no status row sits above the box.
    const A_PI_WORKING_0851: &str = r"
 $ uname -a 2>&1 | head -n 5; echo ---; echo $XDG_SESSION_TYPE $WAYLAND_DISPLAY $DISPLAY; echo ---;
 ls /sys/class/drm 2>&1 | head -n 20; echo ---; cat /sys/class/drm/*/modes 2>&1 | head -n 20

 ... (10 earlier lines, ctrl+o to expand)
 renderD128
 version
 ---
 60x2008
 2560x1600

 Took 0.0s



 Run sleep 75 with the bash tool, then reply with the single word done.


── ⠼ Working ───────────────────────────────────────────────────────────────────────────────────────

────────────────────────────────────────────────────────────────────────────────────────────────────
/tmp/rig/work
↑20k ↓682 R3.8k CH9.6% 1.9%/1.0M (auto)            (opencode) muse-spark-1.3-contributor-free • high
";

    /// The same turn at 20 columns. The top border has room for only seven
    /// rule characters after the message, so the twenty the rule asks for are
    /// on the bottom border alone. The frame is two rows above it.
    const A_PI_WORKING_0851_20: &str = r"
 Run sleep 75 with
 the bash tool,
 then reply with
 the single word
 done.



 $ sleep 75
 (timeout 90s)

 Elapsed 13.0s


── ⠇ Working ───────

────────────────────
/tmp/rig/work
↑20k ↓823 R23k CH...
";

    /// pi compacting the context at 100 columns (`/compact`). The compaction
    /// message replaces `Working...` on the same row with the same frame.
    ///
    /// Held still by a provider that never answers the summarisation request.
    /// The footer's first row is `~` because the run used its own home.
    const A_PI_COMPACTING: &str = r"
 pi v0.84.4
 escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more
 Press ctrl+o to show full startup help and loaded resources.

 Pi can explain its own features and look up its docs. Ask it how to use or extend Pi.

[Extensions]
  rig.js


 what did you do


 The rig answered.


 and then


 The rig answered.

 ⠼ Compacting context... (escape to cancel)

────────────────────────────────────────────────────────────────────────────────────────────────────

────────────────────────────────────────────────────────────────────────────────────────────────────
~
0.0%/264k (auto)                                                                         (bench) rig
";

    /// The same screen at 20 columns. The message wraps across three rows with
    /// the frame on the first. This is the widest span measured between a
    /// frame and the box, and it sets the spinner rule's `within`.
    const A_PI_COMPACTING_20: &str = r" Pi can explain its
 own features and
 look up its docs.
 Ask it how to use
 or extend Pi.

[Extensions]
  rig.js


 what did you do


 The rig answered.


 and then


 The rig answered.

 ⠸ Compacting
 context... (escape
 to cancel)

────────────────────

────────────────────
~
0.0%/264k (auto)  ri
";

    /// A turn waiting to retry after its provider answered 503, at 100
    /// columns.
    const A_PI_RETRYING: &str = r#"
 pi v0.84.4
 escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more
 Press ctrl+o to show full startup help and loaded resources.

 Pi can explain its own features and look up its docs. Ask it how to use or extend Pi.

[Extensions]
  rig.js


 what did you do


 Error: 503: {"message":"upstream overloaded","type":"overloaded_error"}

 ⠙ Retrying (1/3) in 2s... (escape to cancel)

────────────────────────────────────────────────────────────────────────────────────────────────────

────────────────────────────────────────────────────────────────────────────────────────────────────
~
0.0%/264k (auto)                                                                         (bench) rig
"#;

    /// A turn under an extension's working message, at 100 columns.
    /// `ctx.ui.setWorkingMessage` replaces `Working...` and leaves the rest of
    /// the row alone.
    const A_PI_RENAMED: &str = r"
 pi v0.84.4
 escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more
 Press ctrl+o to show full startup help and loaded resources.

 Pi can explain its own features and look up its docs. Ask it how to use or extend Pi.

[Extensions]
  rig.js


 hold the line


 ⠙ Reviewing the diff

────────────────────────────────────────────────────────────────────────────────────────────────────

────────────────────────────────────────────────────────────────────────────────────────────────────
~
0.0%/264k (auto)                                                                         (bench) rig
";

    /// A `!cmd` shell command (`!sleep 20`) at 100 columns. pi draws its own
    /// box for it in the transcript, with the composer still under it, and
    /// spins the same frame inside it.
    const A_PI_RUNNING_A_COMMAND: &str = r"
 pi v0.84.4
 escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more
 Press ctrl+o to show full startup help and loaded resources.

 Pi can explain its own features and look up its docs. Ask it how to use or extend Pi.

[Extensions]
  rig.js


────────────────────────────────────────────────────────────────────────────────────────────────────
 $ sleep 20

 ⠏ Running... (escape/ctrl+c to cancel)
────────────────────────────────────────────────────────────────────────────────────────────────────

────────────────────────────────────────────────────────────────────────────────────────────────────

────────────────────────────────────────────────────────────────────────────────────────────────────
~
0.0%/264k (auto)                                                                         (bench) rig
";

    /// pi blocked on a dialog at 100 columns. The dialog is inside the
    /// composer box, the directory and stats line are under it, and the
    /// spinner is still up above it.
    const A_PI_DIALOG: &str = r"
 Run this exact bash command and nothing else: echo hi


 Executing command via bash

 I’m thinking about how to use the functions.bash tool to run a command. I’ll need to incorporate a
 timeout, just in case the command takes too long. My plan is to call bash with the command “echo
 hi” and set the timeout as part of the call. Once it's executed, I’ll return the output. I just
 want to make sure this runs smoothly and effectively!


 $ echo hi (timeout 10s)


 ⠧ Working...

────────────────────────────────────────────────────────────────────────────────────────────────────

 Run echo hi?

 → Allow once
   Allow always
   Deny

 ↑↓ navigate  enter select  escape/ctrl+c cancel

────────────────────────────────────────────────────────────────────────────────────────────────────
~/.claude/jobs/eef72778/tmp/pipane
↑1.3k ↓64 $0.000 (sub) 0.5%/264k (auto)                        (github-copilot) gpt-5-mini • minimal
";

    /// The same dialog at 20 columns, the narrowest pane pi draws its box in.
    /// The hint row wraps across four rows and `enter select` breaks between
    /// its words; the row still opens with `↑↓ navigate`.
    const A_PI_DIALOG_20: &str = r" it's executed,
 I’ll return the
 output. I just
 want to make sure
 this runs smoothly
 and effectively!


 $ echo hi (timeout
 10s)


 ⠹ Working...

────────────────────

 Run echo hi?

 → Allow once
   Allow always
   Deny

 ↑↓ navigate  enter
  select
 escape/ctrl+c
 cancel

────────────────────
~/.claude/jobs/ee...
↑1.3k ↓64 $0.000 ...
";

    /// `ctx.ui.input` at 100 columns: the caller's title, the input row and
    /// pi's hint row.
    const A_PI_INPUT: &str = r"
 pi v0.84.4
 escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more
 Press ctrl+o to show full startup help and loaded resources.

 Pi can explain its own features and look up its docs. Ask it how to use or extend Pi.

[Extensions]
  screens.js


────────────────────────────────────────────────────────────────────────────────────────────────────

 Which branch should I push to?

>

 enter submit  escape/ctrl+c cancel

────────────────────────────────────────────────────────────────────────────────────────────────────
~/.claude/jobs/3876e46d/tmp/pane
0.0%/1.0M (auto)                                   (opencode) muse-spark-1.3-contributor-free • high








";

    /// The same screen at 20 columns. The title wraps across two rows and the
    /// hint row across three; the hint still opens with `enter submit`.
    const A_PI_INPUT_20: &str = r" · ctrl+o more
 Press ctrl+o to
 show full startup
 help and loaded
 resources.

 Pi can explain its
 own features and
 look up its docs.
 Ask it how to use
 or extend Pi.

[Extensions]
  screens.js


────────────────────

 Which branch
 should I push to?

>

 enter submit
 escape/ctrl+c
 cancel

────────────────────
~/.claude/jobs/38...
0.0%/1.0M (auto)  mu
";

    /// `ctx.ui.editor` at 100 columns: the same box with a second box inside
    /// for the text, and a hint row covering newlines as well as submit.
    const A_PI_EDITOR: &str = r"
 pi v0.84.4
 escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more
 Press ctrl+o to show full startup help and loaded resources.

 Pi can explain its own features and look up its docs. Ask it how to use or extend Pi.

[Extensions]
  screens.js


────────────────────────────────────────────────────────────────────────────────────────────────────

 Write the commit message

────────────────────────────────────────────────────────────────────────────────────────────────────

────────────────────────────────────────────────────────────────────────────────────────────────────

 enter submit  shift+enter/ctrl+j newline  escape/ctrl+c cancel  ctrl+g external editor

────────────────────────────────────────────────────────────────────────────────────────────────────
~/.claude/jobs/3876e46d/tmp/pane
0.0%/1.0M (auto)                                   (opencode) muse-spark-1.3-contributor-free • high






";

    /// The same screen at 20 columns. The hint row wraps across six rows and
    /// `shift+enter/ctrl+j` stays whole.
    const A_PI_EDITOR_20: &str = r"
 Pi can explain its
 own features and
 look up its docs.
 Ask it how to use
 or extend Pi.

[Extensions]
  screens.js


────────────────────

 Write the commit
 message

────────────────────

────────────────────

 enter submit
 shift+enter/ctrl+j
  newline
 escape/ctrl+c
 cancel  ctrl+g
 external editor

────────────────────
~/.claude/jobs/38...
0.0%/1.0M (auto)  mu
";

    /// `ctx.ui.confirm` at 100 columns: the same box with two choices and the
    /// caller's message under the title.
    const A_PI_CONFIRM: &str = r"
 pi v0.84.4
 escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more
 Press ctrl+o to show full startup help and loaded resources.

 Pi can explain its own features and look up its docs. Ask it how to use or extend Pi.

[Extensions]
  screens.js


────────────────────────────────────────────────────────────────────────────────────────────────────

 Push to origin?
 This rewrites the remote branch.

 → Yes
   No

 ↑↓ navigate  enter select  escape/ctrl+c cancel

────────────────────────────────────────────────────────────────────────────────────────────────────
~/.claude/jobs/3876e46d/tmp/pane
0.0%/1.0M (auto)                                   (opencode) muse-spark-1.3-contributor-free • high






";

    /// `/trust` at 100 columns, on a worktree with nothing saved. The same box
    /// and hint row as the dialog above, with nothing running over it: a
    /// person raises this before a turn.
    const A_PI_TRUST: &str = r"
 pi v0.84.4
 escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more
 Press ctrl+o to show full startup help and loaded resources.

 Pi can explain its own features and look up its docs. Ask it how to use or extend Pi.


────────────────────────────────────────────────────────────────────────────────────────────────────

 Project trust
 /home/saiful/.claude/jobs/1e9e9b98/tmp/worktrees/fix-login-a1b

 Saved decision: none
 Current session: trusted

 → Trust
   Trust parent folder (/home/saiful/.claude/jobs/1e9e9b98/tmp/worktrees)
   Do not trust

 ↑↓ navigate  enter save  escape/ctrl+c cancel

────────────────────────────────────────────────────────────────────────────────────────────────────
~/.claude/jobs/1e9e9b98/tmp/worktrees/fix-login-a1b
0.0%/1.0M (auto)                                   (opencode) muse-spark-1.3-contributor-free • high
";

    /// The same question at 20 columns. The tallest box pi draws: at amx's
    /// worktree depth the title and top border are above the floor, leaving
    /// the hint row and the border under it.
    const A_PI_TRUST_20: &str = r"
────────────────────

 Project trust
 /home/saiful/.clau
 de/jobs/1e9e9b98/t
 mp/worktrees/fix-l
 ogin-a1b

 Saved decision:
 none
 Current session:
 trusted

 → Trust
   Trust parent
 folder
 (/home/saiful/.cla
 ude/jobs/1e9e9b98/
 tmp/worktrees)
   Do not trust

 ↑↓ navigate  enter
  save
 escape/ctrl+c
 cancel

────────────────────
~/.claude/jobs/1e...
0.0%/1.0M (auto)  mu
";

    /// pi 0.85.1 asking about the folder unprompted, at 100 columns on a
    /// 30-row pane, in a checkout with a `.pi/` and no saved decision. Unlike
    /// `/trust`: a different title, a sentence about what trusting allows,
    /// five choices instead of three, and `enter select` in the hint row.
    const A_PI_FOLDER_TRUST: &str = r"
────────────────────────────────────────────────────────────────────────────────────────────────────

 Trust project folder?
 /home/saiful/Sites/tries/pi-src

 This allows pi to load .pi settings and resources, install missing project packages, and execute
 project extensions.

 → Trust
   Trust parent folder (/home/saiful/Sites/tries)
   Trust (this session only)
   Do not trust
   Do not trust (this session only)

 ↑↓ navigate  enter select  escape/ctrl+c cancel

────────────────────────────────────────────────────────────────────────────────────────────────────
";

    /// The same at 20 columns. The title is above the floor and the folder is
    /// the first row left, so the dialog rule claims it as it does `/trust`.
    const A_PI_FOLDER_TRUST_20: &str = r" /home/saiful/Sites
 /tries/pi-src

 This allows pi to
 load .pi settings
 and resources,
 install missing
 project packages,
 and execute
 project
 extensions.

 → Trust
   Trust parent
 folder
 (/home/saiful/Site
 s/tries)
   Trust (this
 session only)
   Do not trust
   Do not trust
 (this session
 only)

 ↑↓ navigate  enter
  select
 escape/ctrl+c
 cancel

────────────────────
";

    /// pi's first-run gate at 100 columns on a 30-row pane
    /// (`PI_EXPERIMENTAL=1`, no `settings.json`). The box is the whole screen
    /// and the rows under it are the empty pane. Those rows push the box's top
    /// border above the floor, which is why this rule has no `within`.
    const A_PI_SETUP: &str = r"
────────────────────────────────────────────────────────────────────────────────────────────────────

 ██████
 ██  ██
 ████  ██
 ██    ██

 Welcome to pi, the minimal coding agent.

 Pick a theme.
 Detected system appearance: dark

 → Dark
   Light

 ↑↓ navigate  enter continue  escape/ctrl+c skip setup

────────────────────────────────────────────────────────────────────────────────────────────────────












";

    /// The gate's second step at 20 columns. The usage-data prose wraps until
    /// the box is taller than the pane, leaving the hint row and the border
    /// under it.
    const A_PI_SETUP_ANALYTICS_20: &str = r" anonymous usage
 data sharing?
 Opting in stores a
 tracking
 identifier in
 settings.json and
 enables anonymous
 usage analytics.
 This helps us to
 better debug,
 reproduce, and
 resolve issues
 and bugs within
 Pi. You can
 observe what is
 shared using
 /privacy and make
 changes anytime in
 settings.json.

 → Share anonymous
 usage data
   Don't share

 ↑↓ navigate  enter
  finish
 escape/ctrl+c skip
 setup

────────────────────
";

    /// pi waiting for a provider key at 100 columns, reached with `/login`
    /// through two selectors. The box is in the composer's slot with the
    /// footer under it. The warning above it spells `/login to log into`,
    /// which is why the rule cannot anchor on the title.
    const A_PI_LOGIN: &str = r"
 Pi can explain its own features and look up its docs. Ask it how to use or extend Pi.


 Warning: No models available. Use /login to log into a provider via OAuth or API key. See:

 /home/saiful/.local/share/mise/installs/node/24.20.0/lib/node_modules/@earendil-works/pi-coding-ag
 ent/docs/providers.md

 /home/saiful/.local/share/mise/installs/node/24.20.0/lib/node_modules/@earendil-works/pi-coding-ag
 ent/docs/models.md

────────────────────────────────────────────────────────────────────────────────────────────────────
 Login to Cerebras

 Enter Cerebras API key
>
 (escape/ctrl+c to cancel, enter to submit)
────────────────────────────────────────────────────────────────────────────────────────────────────
/home/saiful/.claude/jobs/6bbcd928/tmp/rig/work
0.0%/0 (auto)                                                                                unknown
";

    /// The same screen at 20 columns. The hint row takes a second row and the
    /// span from the top border is 6, the widest measured and the rule's
    /// `within`.
    const A_PI_LOGIN_20: &str = r" oding-agent/docs/p
 roviders.md

 /home/saiful/.loca
 l/share/mise/insta
 lls/node/24.20.0/l
 ib/node_modules/@e
 arendil-works/pi-c
 oding-agent/docs/m
 odels.md

────────────────────
 Login to Cerebras

 Enter Cerebras API
 key
>
 (escape/ctrl+c to
 cancel, enter to
 submit)
────────────────────
/home/saiful/.cla...
0.0%/0 (auto)  unkno
";

    /// The login dialog on pi 0.85.1 at 100 columns, with models configured so
    /// no warning is above it. The hint row is still 5 rows under the top
    /// border.
    const A_PI_LOGIN_0851: &str = r"
 (no output)

 Took 75.0s


 done

 Refreshing model catalogs…

────────────────────────────────────────────────────────────────────────────────────────────────────
 Login to Cerebras

 Enter Cerebras API key
>
 (escape/ctrl+c to cancel, enter to submit)
────────────────────────────────────────────────────────────────────────────────────────────────────
/tmp/rig/work
↑41k ↓847 R24k CH0.6% 1.9%/1.0M (auto)             (opencode) muse-spark-1.3-contributor-free • high
";

    /// The same at 20 columns: the hint row breaks after each `to`, the span
    /// is 6, and `(escape/ctrl+c to` is whole on its first row.
    const A_PI_LOGIN_0851_20: &str = r"
 catalogs…

────────────────────
 Login to Cerebras

 Enter Cerebras API
 key
>
 (escape/ctrl+c to
 cancel, enter to
 submit)
────────────────────
/tmp/rig/work
↑41k ↓847 R24k CH...
";

    /// A finished turn at the prompt. The spinner line is gone; the box, the
    /// directory and the stats line are where they were.
    const A_PI_IDLE: &str = r"
[Themes]
  qshell


 Run this exact bash command and nothing else: echo hi


 Executing command via bash

 I’m thinking about how to use the functions.bash tool to run a command. I’ll need to incorporate a
 timeout, just in case the command takes too long. My plan is to call bash with the command “echo
 hi” and set the timeout as part of the call. Once it's executed, I’ll return the output. I just
 want to make sure this runs smoothly and effectively!


 $ echo hi (timeout 10s)

 hi

 Took 15.2s


 hi

────────────────────────────────────────────────────────────────────────────────────────────────────

────────────────────────────────────────────────────────────────────────────────────────────────────
~/.claude/jobs/eef72778/tmp/pipane
↑1.5k ↓69 R1.3k CH90.3% $0.001 (sub) 0.5%/264k (auto)          (github-copilot) gpt-5-mini • minimal
";

    /// The idle screen at 24 columns. The stats line is truncated and the
    /// context indicator is gone, so the token count needs its own anchor.
    const A_PI_IDLE_24: &str = r" need to incorporate a
 timeout, just in case
 the command takes too
 long. My plan is to
 call bash with the
 command “echo hi” and
 set the timeout as
 part of the call. Once
 it's executed, I’ll
 return the output. I
 just want to make sure
 this runs smoothly and
 effectively!


 $ echo hi (timeout
 10s)

 hi

 Took 15.2s


 hi

────────────────────────

────────────────────────
~/.claude/jobs/eef727...
↑1.5k ↓69 R1.3k CH90....
";

    /// The show-images selector, the shortest widget pi draws in the
    /// composer's slot, at 100 columns on a 30-row pane. Raised through
    /// `ctx.ui.custom`, the only way to reach it; the component is pi's own.
    ///
    /// Five rows from the box's top border to the stats line, a span the idle
    /// rule once allowed, so a pi waiting on a menu choice read as idle.
    const A_PI_SELECTOR: &str = r"
 pi v0.84.4
 escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more
 Press ctrl+o to show full startup help and loaded resources.

 Pi can explain its own features and look up its docs. Ask it how to use or extend Pi.

[Extensions]
  screens.ts


────────────────────────────────────────────────────────────────────────────────────────────────────
  Yes         Show images inline in terminal
→ No          Show text placeholder instead
────────────────────────────────────────────────────────────────────────────────────────────────────
~/.claude/jobs/3bd43671/tmp/measure
0.0%/1.0M (auto)                                   (opencode) muse-spark-1.3-contributor-free • high












";

    /// The same selector under a transcript, after `!seq 1 60`. The widget is
    /// unchanged, but `!cmd` leaves its own box's bottom border two rows above
    /// the widget's. The stats line is 7 rows from that border, 5 from the
    /// widget's, and neither is the composer's 4. A rule that read only the
    /// topmost border turned on what had scrolled by.
    const A_PI_SELECTOR_UNDER_A_TRANSCRIPT: &str = r" 42
 43
 44
 45
 46
 47
 48
 49
 50
 51
 52
 53
 54
 55
 56
 57
 58
 59
 60


 ... 41 more lines (ctrl+o to expand)
────────────────────────────────────────────────────────────────────────────────────────────────────

────────────────────────────────────────────────────────────────────────────────────────────────────
  Yes         Show images inline in terminal
→ No          Show text placeholder instead
────────────────────────────────────────────────────────────────────────────────────────────────────
~/.claude/jobs/3bd43671/tmp/measure
0.0%/1.0M (auto)                                   (opencode) muse-spark-1.3-contributor-free • high
";

    /// pi's model selector (`/model`). The box is taller than the floor, so
    /// only its bottom border is in view: three rows of screen, a span of 2,
    /// and it once read as a prompt.
    const A_PI_MODEL_SELECTOR: &str = r"
 Pi can explain its own features and look up its docs. Ask it how to use or extend Pi.


────────────────────────────────────────────────────────────────────────────────────────────────────

Only showing models from configured providers. Use /login to add providers.

>

→ muse-spark-1.3-contributor-free [opencode] · default ✓
  claude-haiku-4.5 [github-copilot]
  claude-sonnet-4.6 [github-copilot]
  gemini-3.5-flash [github-copilot]
  gpt-5-mini [github-copilot]
  gpt-5.3-codex [github-copilot]
  gpt-5.4 [github-copilot]
  gpt-5.4-mini [github-copilot]
  mai-code-1-flash-picker [github-copilot]
  mai-code-1.1-flash [github-copilot]
  (1/100)

  Model Name: Muse Spark 1.3 Free

  Model catalogs refreshed.

  Enter to select · Ctrl+S to set as default · Esc to cancel
────────────────────────────────────────────────────────────────────────────────────────────────────
~/.claude/jobs/3bd43671/tmp/measure
0.0%/1.0M (auto)                                   (opencode) muse-spark-1.3-contributor-free • high
";

    /// The model selector on pi 0.85.1 at 100 columns. The hint row now spells
    /// the cancel key in full, `Escape/Ctrl+C to cancel` where 0.84.4 wrote
    /// `Esc to cancel`, which is the login rule's anchor on a screen that is
    /// not the login dialog. The current model's mark moved before its name.
    const A_PI_MODEL_SELECTOR_0851: &str = r"
 Error: Failed to save API key for Cerebras: This operation was aborted

────────────────────────────────────────────────────────────────────────────────────────────────────

Only showing models from configured providers. Use /login to add providers.

>

→ ✓ muse-spark-1.3-contributor-free [opencode] · default
    claude-haiku-4.5 [github-copilot]
    claude-sonnet-4.6 [github-copilot]
    gemini-3.5-flash [github-copilot]
    gpt-5-mini [github-copilot]
    gpt-5.3-codex [github-copilot]
    gpt-5.4 [github-copilot]
    gpt-5.4-mini [github-copilot]
    mai-code-1-flash-picker [github-copilot]
    mai-code-1.1-flash [github-copilot]
  (1/477)

  Model Name: Muse Spark 1.3 Free

  Model catalogs refreshed.

  Enter to select · Ctrl+S to set as default · Escape/Ctrl+C to cancel
────────────────────────────────────────────────────────────────────────────────────────────────────
/tmp/rig/work
↑41k ↓847 R24k CH0.6% 2.0%/1.0M (auto)             (opencode) muse-spark-1.3-contributor-free • high
";

    fn claim<'a>(rules: &'a Ruleset, screen: &str, recorded: Phase) -> Claim<'a> {
        rules.claim(screen, recorded, SETTLED_LOOKS)
    }

    /// claude's screens, looked up by vendor name as a reader does.
    fn claude() -> &'static Ruleset {
        of("claude")
    }

    /// Every document amx ships plus the second vendor's, which shares no
    /// string with any of them. A property that holds for all of them is about
    /// the machinery.
    fn documents() -> Vec<(&'static str, Ruleset)> {
        [claude::VENDOR, pi::VENDOR, SECOND]
            .iter()
            .map(|vendor| {
                let screens = vendor.screens.expect("each of these has screens");
                (vendor.name, Ruleset::parse(screens).expect(vendor.name))
            })
            .collect()
    }

    /// The rule names in a document.
    fn named(screens: &Ruleset) -> Vec<&str> {
        screens
            .rules()
            .iter()
            .map(|rule| rule.name.as_str())
            .collect()
    }

    #[test]
    fn rules_the_screens_read_are_the_ones_the_vendor_draws() {
        let documents = documents();
        assert_eq!(
            named(select(&documents, "second").expect("the second vendor's screens")),
            ["choice", "busy", "prompt"]
        );
        assert_eq!(
            select(&documents, "claude").map(named),
            Some(named(claude()))
        );
        assert!(
            select(&documents, "nobody").is_none(),
            "a vendor nobody has measured has no screens to read"
        );
    }

    /// The rules a document marks as gates in front of the work.
    fn gates(screens: &Ruleset) -> Vec<&str> {
        screens
            .rules()
            .iter()
            .filter(|rule| rule.setup)
            .map(|rule| rule.name.as_str())
            .collect()
    }

    #[test]
    fn rules_each_document_names_the_screens_its_vendor_gates_a_run_with() {
        // Each document names its own gates, since which screens gate a run
        // is a fact about the vendor.
        assert_eq!(gates(claude()), ["folder_trust"]);
        assert_eq!(
            gates(pi()),
            ["first_time_setup", "project_trust", "folder_trust", "login"],
            "the gate in front of a first run, the question a folder nobody \
             has decided about raises — asked with `/trust` or by pi itself \
             on the way in — and a pi with no key to call a provider with"
        );

        // And a gate is a screen somebody is standing in front of. One marked
        // on a rule that means anything else would be a check reporting an
        // agent parked on a turn that is running.
        for (vendor, screens) in documents() {
            for rule in screens.rules().iter().filter(|rule| rule.setup) {
                assert_eq!(rule.state, Phase::Waiting, "{vendor}'s {}", rule.name);
            }
        }
    }

    #[test]
    fn rules_an_agent_command_is_read_as_the_vendor_it_runs() {
        // `agent` is a command line rather than a program name, and a command
        // amx has no entry for is a wrapper around one it has: both are read
        // with the screens of the vendor that ends up drawing them.
        for agent in ["claude", "claude --add-dir ..", "my-claude", ""] {
            assert_eq!(named(of(agent)), named(claude()), "{agent:?}");
        }

        // And what a command with no entry falls back to is the vendor amx
        // would run if nobody had configured one, which is the first in the
        // table. Two ways of saying the default, and they have to agree.
        assert_eq!(
            registry::program(&crate::config::Config::default().agent),
            registry::entries().first().map_or("", |vendor| vendor.name)
        );
    }

    #[test]
    fn rules_a_sentence_that_stands_in_for_a_question_is_the_vendors_own() {
        // The vendor sends these about a dialog it will not describe, and a
        // reader that took one for an answer would leave somebody reading the
        // pane themselves. Which sentences they are is the vendor's own
        // wording and nobody else's.
        assert!(claude().placeholder("Claude needs your permission"));
        assert!(
            !claude().placeholder("Claude needs your permission to use Bash"),
            "a whole sentence and never the start of one: the one that names \
             the tool is something a caller can act on"
        );

        let second = Ruleset::parse(SECOND.screens.unwrap()).unwrap();
        assert!(second.placeholder("it wants something"));
        assert!(
            !second.placeholder("Claude needs your permission"),
            "another vendor's sentence is not this vendor's"
        );
        assert!(
            !unmeasured().placeholder("it wants something"),
            "a vendor nobody has measured has none of these either"
        );
    }

    #[test]
    fn rules_a_vendor_nobody_has_measured_claims_nothing() {
        // An unmeasured vendor claims nothing, which reads as `unknown`.
        let none = unmeasured();
        assert!(none.rules().is_empty());
        assert_eq!(
            none.claim(PERMISSION_BOX, Phase::Working, 1),
            Claim::Unclaimed
        );
        assert_eq!(none.asking(PERMISSION_BOX), None);
    }

    #[test]
    fn rules_the_bundled_file_is_the_ruleset() {
        let rules = claude();
        let names: Vec<_> = rules.rules().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "folder_trust",
                "permission_prompt",
                "ask_menu",
                "plan_approval",
                "spinner",
                "background_agents",
                "idle_prompt"
            ],
            "order decides, so it is part of the data"
        );
    }

    #[test]
    fn rules_every_anchor_is_folded_the_way_the_screen_is() {
        // Matching folds the capture's case, so an anchor with a capital in it
        // could never match and would fail silently.
        for (vendor, screens) in documents() {
            for rule in screens.rules() {
                let asks = match &rule.asks {
                    Asks::Sentence(anchor) | Asks::Above(anchor) => Some(anchor),
                    Asks::AboveOptions => None,
                };
                for needle in rule
                    .all
                    .iter()
                    .chain(&rule.any)
                    .chain(&rule.not_below)
                    .chain(&rule.not)
                    .chain(asks)
                {
                    assert_eq!(
                        needle,
                        &needle.to_lowercase(),
                        "{vendor}'s {}: {needle:?} must be written folded",
                        rule.name
                    );
                }
            }
        }
    }

    #[test]
    fn rules_no_screen_is_named_in_rust_to_decide_anything() {
        // A match on a rule name would read one vendor's document with another
        // vendor's names, and the second vendor's screens are named
        // differently, so every arm would silently miss. What a screen wants is
        // written beside its rule, and a verdict's rule name is only looked up
        // in the document it came from.
        let ships = |source: &str| {
            source
                .split("#[cfg(test)]")
                .next()
                .unwrap_or(source)
                .to_string()
        };
        let reading = [
            ships(include_str!("rules.rs")),
            ships(include_str!("derive.rs")),
            // Tests included: a fixture spelling out a screen name would be
            // the same list of names in a second place.
            include_str!("verbs/doctor.rs").to_string(),
        ];

        for (vendor, screens) in documents() {
            for rule in screens.rules() {
                for source in &reading {
                    assert!(
                        !source.contains(&format!("\"{}\"", rule.name)),
                        "{vendor}'s {} is spelled out in Rust",
                        rule.name
                    );
                }
            }
        }
    }

    #[test]
    fn rules_the_four_blocking_prompts_rule_waiting() {
        let rules = claude();
        for (what, screen) in [
            ("trust at 220 columns", TRUST_SCREEN_220),
            ("trust at 54 columns", TRUST_SCREEN_54),
            ("2.1.259 trust at 54 columns", TRUST_SCREEN_259_54),
            ("2.1.259 trust at 24 columns", TRUST_SCREEN_259_24),
            ("a live permission box", PERMISSION_BOX),
            ("the ask menu at 80 columns", ASK_MENU_80),
            ("the ask menu at 24 columns", ASK_MENU_24),
            ("2.1.259's ask menu at 24 columns", ASK_MENU_259_24),
            ("the plan approval screen at 220 columns", PLAN_APPROVAL_220),
            ("the plan approval screen at 54 columns", PLAN_APPROVAL_54),
            ("the plan approval screen at 24 columns", PLAN_APPROVAL_24),
        ] {
            let claimed = claim(rules, screen, Phase::Working);
            assert_eq!(
                claimed.phase(),
                Some(Phase::Waiting),
                "{what} must rule waiting, ruled {claimed:?}"
            );
        }
    }

    #[test]
    fn rules_a_quotation_of_a_widget_is_not_a_widget() {
        // No anchor tells these from the real thing; the layout does. claude
        // draws no composer under a blocking prompt, so a widget with the mode
        // footer under it is text about a widget.
        let rules = claude();
        for (what, screen) in [
            ("a quoted permission box", QUOTED_PERMISSION_BOX),
            (
                "the same under a manual footer",
                QUOTED_BOX_UNDER_A_MANUAL_FOOTER,
            ),
            (
                "prose that reads like a question",
                PROSE_THAT_LOOKS_LIKE_A_QUESTION,
            ),
            ("prose about trust", PROSE_ABOUT_TRUST),
            (
                "a sentence with a confirm footer in it",
                PROSE_WITH_A_CONFIRM_FOOTER_IN_IT,
            ),
            (
                "a plan staged in the composer",
                A_PLAN_STAGED_IN_THE_COMPOSER,
            ),
        ] {
            let claimed = claim(rules, screen, Phase::Working);
            assert_ne!(
                claimed.phase(),
                Some(Phase::Waiting),
                "{what} must not rule waiting, ruled {claimed:?}"
            );
        }
    }

    /// The question a claimed screen is asking.
    fn asked(rules: &Ruleset, screen: &str) -> Question {
        let Claim::Ruled(rule) = claim(rules, screen, Phase::Working) else {
            panic!("no rule claims this screen");
        };
        rule.question(screen)
            .expect("a screen that blocks says what it is blocking on")
    }

    #[test]
    fn rules_where_a_screen_keeps_its_question_is_the_documents_to_say() {
        // Two of claude's blocking screens draw something under the question,
        // so there the question is the sentence the anchor is on rather than
        // the one above the choices. The document says which, per rule.
        let asks = |name: &str| {
            let rule = claude().rules().iter().find(|rule| rule.name == name);
            rule.map(|rule| rule.asks.clone())
        };
        assert_eq!(
            asks("permission_prompt"),
            Some(Asks::Sentence("do you want to".to_string()))
        );
        assert_eq!(
            asks("folder_trust"),
            Some(Asks::Sentence("quick".to_string())),
            "the word the question opens with, since the word the rule stands \
             on is on one of the choices too"
        );
        assert_eq!(
            asks("ask_menu"),
            Some(Asks::AboveOptions),
            "a screen that says nothing keeps it above the choices"
        );
    }

    #[test]
    fn rules_a_second_vendors_question_is_read_where_its_own_document_says() {
        // The same machinery over a document sharing no string with claude's:
        // its own anchor, choices with no cursor glyph, and a sentence wrapped
        // across two rows.
        let screens = Ruleset::parse(SECOND.screens.unwrap()).unwrap();
        let Claim::Ruled(rule) = screens.claim(A_SECOND_VENDOR_ASKING, Phase::Working, 1) else {
            panic!("the second vendor's own rule claims its own screen");
        };
        assert_eq!(rule.name, "choice");

        let asked = rule
            .question(A_SECOND_VENDOR_ASKING)
            .expect("a screen that blocks says what it is blocking on");
        assert_eq!(
            asked.text,
            "pick one and I will carry on about the file you named"
        );
        assert_eq!(asked.options, ["keep it", "drop it"]);
    }

    #[test]
    fn rules_a_permission_box_carries_the_question_and_the_two_answers() {
        let asked = asked(claude(), PERMISSION_BOX);
        assert_eq!(asked.text, "Do you want to proceed?");
        assert_eq!(
            asked.options,
            ["Yes", "No"],
            "the request above the question is what it is about, not what it asks"
        );
    }

    #[test]
    fn rules_the_trust_screen_asks_one_question_at_every_width() {
        // At 54 columns the sentence wraps across five rows, breaking between
        // `you` and `trust`; at 24 it takes eleven and the floor opens on the
        // first. Every width and both versions must read the same question,
        // though the later version has no numbered choice to stop above.
        let whole = "Quick safety check: Is this a project you created or one you \
             trust? (Like your own code, a well-known open source project, or work \
             from your team). If not, take a moment to review what's in this folder \
             first.";
        let numbered: &[&str] = &["Yes, I trust this folder", "No, exit"];
        // 2.1.259 and 2.1.276 number nothing here, so the rows are read off the
        // cursor glyph and amx numbers them in drawn order. The exit comes
        // first, which a caller pressing a digit needs to know.
        let marked: &[&str] = &["No, exit", "Yes, I trust this folder"];
        for (what, screen, options, walked) in [
            ("2.1.226 at 220 columns", TRUST_SCREEN_220, numbered, false),
            ("2.1.226 at 54 columns", TRUST_SCREEN_54, numbered, false),
            ("2.1.259 at 54 columns", TRUST_SCREEN_259_54, marked, true),
            ("2.1.259 at 24 columns", TRUST_SCREEN_259_24, marked, true),
            ("2.1.276 at 220 columns", TRUST_SCREEN_276_220, marked, true),
        ] {
            let asked = asked(claude(), screen);
            assert_eq!(asked.text, whole, "{what}");
            assert_eq!(asked.options, options, "{what}");
            assert_eq!(
                asked.walked, walked,
                "{what}: a list amx numbered off the mark is walked, and one \
                 the vendor numbered itself is not"
            );
        }
    }

    #[test]
    fn rules_a_menu_carries_the_agents_own_question_and_every_choice() {
        let wide = asked(claude(), ASK_MENU_80);
        assert_eq!(
            wide.text,
            "Should this project be indented with spaces or tabs?"
        );
        assert_eq!(
            wide.options,
            ["Spaces", "Tabs", "Type something.", "Chat about this"],
            "the descriptions under the labels are not choices, and the rule \
             between the third and the fourth does not end the list"
        );

        // At 24 columns the choices survive the wrap, but the question's first
        // row is above the floor, so only what could be seen is recorded.
        let narrow = asked(claude(), ASK_MENU_24);
        assert_eq!(narrow.options, wide.options);
        assert_eq!(narrow.text, "indented with spaces or tabs?");
    }

    #[test]
    fn rules_a_menu_taller_than_the_floor_is_claimed_by_its_own_bottom() {
        // At 24 columns the top of this box (header, question, first choice
        // and its marker) is above the floor, and both of the rule's old
        // anchors went with it, one out of the floor and one to the wrap.
        let screen = Screen::new(ASK_MENU_259_24);
        assert!(
            screen.rows_of("❯ 1.").is_empty(),
            "the marker is above the floor"
        );
        assert!(
            screen.rows_of("esc to cancel").is_empty(),
            "the footer wrapped"
        );

        // The bottom of the box is what the rule now stands on.
        assert_eq!(
            claim(claude(), ASK_MENU_259_24, Phase::Working).rule_name(),
            Some("ask_menu")
        );
    }

    #[test]
    fn rules_the_plan_approval_asks_one_question_at_every_width() {
        let whole = "Claude has written up a plan and is ready to execute. \
             Would you like to proceed?";
        for (width, screen) in [
            ("220", PLAN_APPROVAL_220),
            ("54", PLAN_APPROVAL_54),
            ("24", PLAN_APPROVAL_24),
        ] {
            assert_eq!(asked(claude(), screen).text, whole, "at {width} columns");
        }

        assert_eq!(
            asked(claude(), PLAN_APPROVAL_220).options,
            [
                "Yes, and use auto mode",
                "Yes, manually approve edits",
                "Tell Claude what to change"
            ]
        );
        assert_eq!(
            asked(claude(), PLAN_APPROVAL_24).options,
            ["Yes, and use", "Yes, manually", "Tell Claude"],
            "a label the vendor wrapped is read as far as its own row goes: the \
             rows under it are where the menu keeps its descriptions"
        );
    }

    #[test]
    fn rules_a_screen_says_what_it_is_asking_whatever_amx_believes() {
        // The caller already has a record saying waiting and lacks only the
        // question, so this takes no phase and no stillness.
        let rules = claude();
        let permission = rules
            .asking(PERMISSION_BOX)
            .expect("a box that blocks is asking something");
        assert_eq!(permission.text, "Do you want to proceed?");
        assert_eq!(permission.options, ["Yes", "No"]);
        assert_eq!(
            rules.asking(ASK_MENU_80).map(|asked| asked.text),
            Some("Should this project be indented with spaces or tabs?".to_string())
        );

        for (what, screen) in [
            ("an idle prompt", IDLE_SCREEN),
            ("a running turn", WORKING_SCREEN),
            ("a shell", A_SHELL),
            ("a quotation of a box", QUOTED_PERMISSION_BOX),
        ] {
            assert_eq!(rules.asking(screen), None, "{what} is asking nothing");
        }
    }

    #[test]
    fn rules_a_screen_that_is_not_blocking_asks_no_question() {
        let rules = claude();
        for (what, screen) in [
            ("an idle prompt", IDLE_SCREEN),
            ("a running turn", WORKING_SCREEN),
            ("a promptless boot", PARKED_SCREEN),
        ] {
            let Claim::Ruled(rule) = claim(rules, screen, Phase::Starting) else {
                panic!("{what} is claimed by a rule");
            };
            assert_eq!(rule.question(screen), None, "{what} is not asking");
        }
    }

    #[test]
    fn rules_the_spinner_rules_working() {
        let rules = claude();
        for (what, screen) in [
            ("a tool call", WORKING_SCREEN),
            ("thinking", THINKING_SCREEN),
            ("a turn at 30 columns", NARROW_TURN_30),
            ("a turn at 24 columns", NARROW_TURN_24),
        ] {
            assert_eq!(
                claim(rules, screen, Phase::Idle).phase(),
                Some(Phase::Working),
                "{what} must rule working"
            );
        }

        // The line claude leaves after a turn has the same glyph and a past
        // tense gerund, with no ellipsis.
        for (what, screen) in [
            ("`✻ Worked for 2m 26s`", IDLE_SCREEN),
            (
                "`✻ Cogitated for 2m 6s · done 10:09 AM`",
                IDLE_SCREEN_259_40,
            ),
        ] {
            assert_eq!(
                claim(rules, screen, Phase::Idle).phase(),
                Some(Phase::Idle),
                "{what} is not a spinner"
            );
        }
    }

    #[test]
    fn rules_a_turn_waiting_on_a_background_agent_is_still_a_turn() {
        // claude 2.1.270 ends the turn when the answer stops and keeps working
        // on the subagents it started, drawing this line above the composer.
        // The hooks said the turn is over and the mode row is under the line,
        // so the screen read idle over a busy agent.
        //
        // A `working` claim over a record the hooks left idle is only a
        // reading; claude reports, and what it reported stays on the record.
        let rules = claude();
        for (what, screen, named) in [
            ("at 100 columns", BACKGROUND_270_100, "background_agents"),
            // At 54 columns the agents panel shows `Preparing…`, the spinner
            // rule's anchor, on a row under the footer rather than over the
            // composer, so the spinner rule does not hold.
            (
                "at 54, under an elided panel",
                BACKGROUND_270_54,
                "background_agents",
            ),
            (
                "at 40, wrapped after `to`",
                BACKGROUND_270_40,
                "background_agents",
            ),
            (
                "at 30, wrapped after `background`",
                BACKGROUND_270_30,
                "background_agents",
            ),
            (
                "at 24, wrapped after `1`",
                BACKGROUND_270_24,
                "background_agents",
            ),
        ] {
            let claimed = claim(rules, screen, Phase::Idle);
            assert_eq!(
                claimed.phase(),
                Some(Phase::Working),
                "{what} must rule working, ruled {claimed:?}"
            );
            assert_eq!(claimed.rule_name(), Some(named), "{what}");
        }
    }

    #[test]
    fn rules_idle_furniture_rules_idle_in_every_mode_and_width() {
        let rules = claude();
        for (what, screen) in [
            ("auto mode", IDLE_SCREEN),
            ("manual mode, no cycle hint", IDLE_SCREEN_MANUAL),
            ("auto mode truncated at 40 columns", FOOTER_AUTO_40),
            ("a promptless boot", PARKED_SCREEN),
        ] {
            assert_eq!(
                claim(rules, screen, Phase::Starting).phase(),
                Some(Phase::Idle),
                "{what} must rule idle"
            );
        }
    }

    #[test]
    fn rules_a_screen_drawn_over_the_prompt_is_not_the_prompt() {
        // Two claude 2.1.270 screens with no mode row. The transcript viewer
        // ends in `? for shortcuts`, kept for an older vendor, so reading the
        // transcript during a long tool call said the turn had ended. The /btw
        // overlay sits in the composer's slot.
        //
        // Unclaimed is right for both: an idle or waiting record keeps its
        // word, and a record mid-turn reads unknown rather than being ended.
        let rules = claude();
        assert!(
            TRANSCRIPT_270_100.contains("? for shortcuts"),
            "the anchor that claimed this screen is still drawn on it, and \
             `not` is what keeps the rule off it"
        );
        for (what, screen) in [
            ("the viewer at 100 columns", TRANSCRIPT_270_100),
            ("at 54", TRANSCRIPT_270_54),
            ("at 40", TRANSCRIPT_270_40),
            ("at 30", TRANSCRIPT_270_30),
            ("at 24", TRANSCRIPT_270_24),
            ("the answered /btw overlay", BTW_ANSWERED_270_100),
        ] {
            assert_eq!(
                claim(rules, screen, Phase::Working),
                Claim::Unclaimed,
                "{what} must claim nothing"
            );
        }

        // The overlay while it answers has claude's own gerund and ellipsis,
        // but in the composer's slot with no composer under it, so it is not
        // the spinner row.
        assert_eq!(
            claim(rules, BTW_ANSWERING_270_100, Phase::Idle),
            Claim::Unclaimed,
            "a side question being answered is not the row over the composer"
        );
    }

    #[test]
    fn rules_a_screen_nothing_knows_claims_nothing() {
        let rules = claude();
        for (what, screen) in [
            ("a shell", A_SHELL),
            ("an empty pane", ""),
            (
                "a pager",
                "  1 use std::io;\n  2 fn main() {}\n~\n~\n\"src/main.rs\" 2L\n",
            ),
        ] {
            assert_eq!(
                claim(rules, screen, Phase::Working),
                Claim::Unclaimed,
                "{what} must claim nothing"
            );
        }
    }

    #[test]
    fn rules_a_still_screen_is_what_ends_a_running_turn() {
        let rules = claude();

        // Mid-turn and idle are the same bytes, so a running turn is not ended
        // until the screen holds still.
        assert_eq!(
            rules.claim(STREAMING_SCREEN, Phase::Working, 0).rule_name(),
            Some("idle_prompt")
        );
        assert!(
            rules
                .claim(STREAMING_SCREEN, Phase::Working, 0)
                .phase()
                .is_none(),
            "a screen that looks idle and has only just gone up ends no turn"
        );
        assert!(
            rules
                .claim(STREAMING_SCREEN, Phase::Working, SETTLED_LOOKS - 1)
                .phase()
                .is_none()
        );
        assert_eq!(
            rules
                .claim(STREAMING_SCREEN, Phase::Working, SETTLED_LOOKS)
                .phase(),
            Some(Phase::Idle)
        );

        // With nothing outstanding it decides at once, which gets a parked
        // agent out of `starting`.
        for recorded in [Phase::Starting, Phase::Unknown] {
            assert_eq!(
                rules.claim(PARKED_SCREEN, recorded, 0).phase(),
                Some(Phase::Idle),
                "{recorded} has nothing to wait for"
            );
        }

        // A rule that is not quiescent never waits.
        assert_eq!(
            rules.claim(WORKING_SCREEN, Phase::Idle, 0).phase(),
            Some(Phase::Working)
        );
    }

    #[test]
    fn rules_only_the_bottom_of_the_capture_is_evidence() {
        // A spinner line far above the floor is transcript, not state.
        let rules = claude();
        let old_news = format!(
            "✢ Infusing… (1m 54s · ↓ 6.9k)\n{}{}",
            "\n".repeat(40),
            A_SHELL
        );
        assert_eq!(claim(rules, &old_news, Phase::Idle), Claim::Unclaimed);
    }

    #[test]
    fn rules_matching_folds_case() {
        let rules = claude();
        let shouted = PERMISSION_BOX.to_uppercase();
        assert_eq!(
            claim(rules, &shouted, Phase::Working).phase(),
            Some(Phase::Waiting)
        );
    }

    #[test]
    fn rules_a_box_and_a_widget_have_to_share_a_screen() {
        // `within` stops two unrelated strings on one screen adding up to a
        // prompt.
        let ruleset = Ruleset::parse(
            r#"
            [[rule]]
            name = "boxed"
            state = "waiting"
            all = ["do you want to"]
            any = ["❯ 1."]
            within = 3
            "#,
        )
        .unwrap();

        let together = "do you want to proceed?\n❯ 1. yes\n";
        let apart = format!("do you want to proceed?\n{}❯ 1. yes\n", "\n".repeat(6));
        assert_eq!(
            ruleset.claim(together, Phase::Working, 1).phase(),
            Some(Phase::Waiting)
        );
        assert_eq!(
            ruleset.claim(&apart, Phase::Working, 1),
            Claim::Unclaimed,
            "six rows apart is not one box"
        );
    }

    #[test]
    fn rules_a_screen_that_says_what_it_is_refuses_the_rule() {
        // `not` asks only whether a string is on the screen, not where: the
        // screens it is for (a viewer, an overlay) draw the chrome of the
        // screen under them anywhere.
        let ruleset = Ruleset::parse(
            r#"
            [[rule]]
            name = "prompt"
            state = "idle"
            any = ["mode:"]
            not = ["showing"]
            "#,
        )
        .unwrap();

        assert_eq!(
            ruleset
                .claim("here\nmode: careful\n", Phase::Starting, 1)
                .phase(),
            Some(Phase::Idle)
        );
        for (where_it_is, screen) in [
            ("above the match", "showing the transcript\nmode: careful\n"),
            ("below it", "mode: careful\nshowing the transcript\n"),
            ("on the same row", "mode: careful, showing the transcript\n"),
        ] {
            assert_eq!(
                ruleset.claim(screen, Phase::Starting, 1),
                Claim::Unclaimed,
                "a screen naming itself {where_it_is} is not this rule's screen"
            );
        }
    }

    #[test]
    fn rules_a_lone_border_is_the_bottom_of_a_box_and_not_a_box() {
        // `apart` is for a box taller than the floor: its top is out of view,
        // leaving its bottom border and the chrome under it, the same rows
        // every screen ends in. No choice of rows spans enough.
        let ruleset = Ruleset::parse(
            r#"
            [[rule]]
            name = "boxed"
            state = "idle"
            all = ["---"]
            any = ["mode:"]
            within = 4
            apart = 4
            "#,
        )
        .unwrap();

        let whole_box = "---\n\n---\nhere\nmode: careful\n";
        let bottom_of_one = "---\nhere\nmode: careful\n";
        assert_eq!(
            ruleset.claim(whole_box, Phase::Starting, 1).phase(),
            Some(Phase::Idle)
        );
        assert_eq!(
            ruleset.claim(bottom_of_one, Phase::Starting, 1),
            Claim::Unclaimed,
            "one border with a footer under it is not the box it is the end of"
        );
    }

    #[test]
    fn rules_a_box_under_another_box_is_still_the_box() {
        // pi draws an Update Available box above its composer when a newer pi
        // exists: five rows, a blank one, then the composer, with the
        // composer's own borders. Anchoring on the topmost border found the
        // notice and lost every window.
        let ruleset = Ruleset::parse(
            r#"
            [[rule]]
            name = "boxed"
            state = "idle"
            all = ["---"]
            any = ["mode:"]
            within = 4
            apart = 4
            "#,
        )
        .unwrap();

        let under_a_notice = "---\nUpdate Available\nNew version 0.85.1 is available. Run pi update\n\
                              Changelog: https://pi.dev/changelog\n---\n\n---\n\n---\nhere\nmode: careful\n";
        assert_eq!(
            ruleset.claim(under_a_notice, Phase::Starting, 1).phase(),
            Some(Phase::Idle),
            "the composer's own border is four rows from the footer"
        );

        // A lone bottom border is still not a box.
        assert_eq!(
            ruleset.claim("---\nhere\nmode: careful\n", Phase::Starting, 1),
            Claim::Unclaimed
        );
    }

    /// pi's screens, reached through the vendor table, which also shows the
    /// document is in the binary and parses.
    fn pi() -> &'static Ruleset {
        of("pi")
    }

    #[test]
    fn rules_pi_reads_the_nine_screens_it_draws() {
        assert_eq!(
            named(pi()),
            [
                "first_time_setup",
                "project_trust",
                "folder_trust",
                "dialog",
                "editor",
                "input",
                "spinner",
                "login",
                "prompt"
            ],
            "order decides, so it is part of the data"
        );
    }

    #[test]
    fn rules_pi_names_a_dialog_a_running_turn_and_a_prompt() {
        // pi reports no turns through hooks, so the pane is its only witness.
        for (what, screen, recorded, means) in [
            (
                "a dialog gating a tool call",
                A_PI_DIALOG,
                Phase::Working,
                Phase::Waiting,
            ),
            (
                "the same dialog at 20 columns",
                A_PI_DIALOG_20,
                Phase::Working,
                Phase::Waiting,
            ),
            ("a turn running", A_PI_WORKING, Phase::Idle, Phase::Working),
            (
                "a turn running on 0.85.1, the frame in the box's top border",
                A_PI_WORKING_0851,
                Phase::Idle,
                Phase::Working,
            ),
            (
                "the same at 20 columns, where only the bottom border is twenty rules",
                A_PI_WORKING_0851_20,
                Phase::Idle,
                Phase::Working,
            ),
            ("a finished turn", A_PI_IDLE, Phase::Starting, Phase::Idle),
            (
                "the same at 24 columns, the context indicator truncated away",
                A_PI_IDLE_24,
                Phase::Starting,
                Phase::Idle,
            ),
            (
                "a pi nobody has typed into yet",
                A_PI_BOOT,
                Phase::Starting,
                Phase::Idle,
            ),
        ] {
            let claimed = claim(pi(), screen, recorded);
            assert_eq!(claimed.phase(), Some(means), "{what}, ruled {claimed:?}");
        }

        assert_eq!(claim(pi(), A_SHELL, Phase::Working), Claim::Unclaimed);
    }

    #[test]
    fn rules_a_pi_dialog_outranks_the_turn_it_went_up_in() {
        // pi raises a dialog from a tool call with the spinner still up, so
        // both rules hold and order decides. The blocking screen wins.
        assert!(
            A_PI_DIALOG.contains("Working..."),
            "the line pi spins is on this screen too"
        );
        assert_eq!(
            claim(pi(), A_PI_DIALOG, Phase::Working).rule_name(),
            Some("dialog")
        );
    }

    #[test]
    fn rules_a_turn_pi_has_stopped_calling_working_is_still_a_turn() {
        // pi has four status lines and only one says `Working...`: compaction
        // and retry replace it, and an extension can rewrite it. The frame in
        // front of the message is on all four.
        for (what, screen) in [
            ("a turn under the vendor's own word", A_PI_WORKING),
            (
                "the same on 0.85.1, the word and its frame in the top border",
                A_PI_WORKING_0851,
            ),
            ("a compacting turn", A_PI_COMPACTING),
            (
                "the same at 20 columns, the message wrapped across three rows",
                A_PI_COMPACTING_20,
            ),
            ("a retrying turn", A_PI_RETRYING),
            (
                "a turn under an extension's own working message",
                A_PI_RENAMED,
            ),
        ] {
            let claimed = claim(pi(), screen, Phase::Idle);
            assert_eq!(
                claimed.phase(),
                Some(Phase::Working),
                "{what} must rule working, ruled {claimed:?}"
            );
            assert_eq!(claimed.rule_name(), Some("spinner"), "{what}");
        }

        for (what, screen) in [
            ("a compacting turn", A_PI_COMPACTING),
            ("a retrying turn", A_PI_RETRYING),
            ("a turn under an extension's own message", A_PI_RENAMED),
        ] {
            assert!(
                !screen.to_lowercase().contains("working..."),
                "{what} carries no working line for a rule to find"
            );
        }

        // The cost of anchoring on the frame: `!cmd` spins the same frame in
        // its own box three rows above pi's, and this rule claims it. A shell
        // command is not the agent's turn, but `working` beats the `unknown`
        // it read before: the pane is busy.
        assert_eq!(
            claim(pi(), A_PI_RUNNING_A_COMMAND, Phase::Idle).rule_name(),
            Some("spinner"),
            "a shell command running in the pane"
        );

        // Screens with no turn running carry no frame.
        for (what, screen) in [
            ("a finished turn", A_PI_IDLE),
            ("a pi nobody has typed into yet", A_PI_BOOT),
            ("a caller waiting to be typed at", A_PI_INPUT),
        ] {
            assert_ne!(
                claim(pi(), screen, Phase::Idle).rule_name(),
                Some("spinner"),
                "{what} is not a turn"
            );
        }
    }

    #[test]
    fn rules_pi_asking_about_the_folder_is_not_pi_asking_about_a_tool_call() {
        // pi draws its folder-trust question in the same box and hint row as a
        // gated tool call, so the dialog rule holds too and order decides.
        // Both need a person; the kind says what for.
        let Claim::Ruled(rule) = claim(pi(), A_PI_TRUST, Phase::Starting) else {
            panic!("pi's own rule claims pi's own screen");
        };
        assert_eq!(rule.name, "project_trust");
        assert_eq!(rule.state, Phase::Waiting);
        assert_eq!(rule.kind, Some(crate::store::Kind::Trust));

        // The question is the title and the folder under it. The two rows
        // right above the choices are pi's saved decision, so `asks` names
        // the title and the sentence runs on to the folder.
        let asked = pi()
            .asking(A_PI_TRUST)
            .expect("the screen says what it is blocking on");
        assert_eq!(
            asked.text,
            "Project trust /home/saiful/.claude/jobs/1e9e9b98/tmp/worktrees/fix-login-a1b"
        );
        assert_eq!(
            asked.options,
            [
                "Trust",
                "Trust parent folder (/home/saiful/.claude/jobs/1e9e9b98/tmp/worktrees)",
                "Do not trust"
            ],
            "the three rows of the run the arrow is in, in the order pi draws \
             them"
        );
        assert!(asked.walked, "read off the marks, and numbered here");

        // At 20 columns the title is above the floor and the screen falls to
        // the dialog rule: still waiting, asked about like a tool call.
        let Claim::Ruled(narrow) = claim(pi(), A_PI_TRUST_20, Phase::Starting) else {
            panic!("something still claims the screen at 20 columns");
        };
        assert_eq!(narrow.name, "dialog");
        assert_eq!(narrow.state, Phase::Waiting);
        assert_eq!(narrow.kind, Some(crate::store::Kind::Question));
    }

    #[test]
    fn rules_pi_asking_about_the_folder_on_its_way_in_is_the_same_kind_of_question() {
        // Started in a folder with a `.pi/`, pi asks before the turn in the
        // dialog's box and without the `/trust` selector's anchors, so the
        // dialog rule read it as a tool call. It is the same decision: `trust`,
        // a gate, and the title with the folder under it.
        let Claim::Ruled(rule) = claim(pi(), A_PI_FOLDER_TRUST, Phase::Starting) else {
            panic!("pi's own rule claims pi's own screen");
        };
        assert_eq!(rule.name, "folder_trust");
        assert_eq!(rule.state, Phase::Waiting);
        assert_eq!(rule.kind, Some(crate::store::Kind::Trust));
        assert!(rule.setup);
        let asked = pi()
            .asking(A_PI_FOLDER_TRUST)
            .expect("the screen says what it is blocking on");
        assert_eq!(
            asked.text,
            "Trust project folder? /home/saiful/Sites/tries/pi-src"
        );
        assert_eq!(
            asked.options,
            [
                "Trust",
                "Trust parent folder (/home/saiful/Sites/tries)",
                "Trust (this session only)",
                "Do not trust",
                "Do not trust (this session only)"
            ],
            "the five rows of the run the arrow is in, in the order pi draws \
             them"
        );
        assert!(asked.walked, "read off the marks, and numbered here");

        // The `/trust` selector keeps its own rule; neither takes the other's
        // screen.
        assert_eq!(
            claim(pi(), A_PI_TRUST, Phase::Starting).rule_name(),
            Some("project_trust")
        );

        // At 20 columns the title is above the floor, so the dialog rule takes
        // it, as it does the selector.
        let Claim::Ruled(narrow) = claim(pi(), A_PI_FOLDER_TRUST_20, Phase::Starting) else {
            panic!("something still claims the screen at 20 columns");
        };
        assert_eq!(narrow.name, "dialog");
        assert_eq!(narrow.kind, Some(crate::store::Kind::Question));
    }

    #[test]
    fn rules_the_screens_a_fresh_pi_stops_on_are_screens_that_say_so() {
        // Two ways an unconfigured pi stops before it can work, both once read
        // as something else. The first-run gate ends in the dialog rule's hint
        // row and read as a tool call; the login dialog is short enough that
        // the box and stats line under it read as `prompt`.
        for (what, screen, named, sentence) in [
            (
                "the gate a first run stops at",
                A_PI_SETUP,
                "first_time_setup",
                "Pick a theme. Detected system appearance: dark",
            ),
            (
                "a pi waiting for a provider's key",
                A_PI_LOGIN,
                "login",
                "Enter Cerebras API key",
            ),
            (
                "the same login at 20 columns, its hint row wrapped in three",
                A_PI_LOGIN_20,
                "login",
                "Enter Cerebras API key",
            ),
            (
                "the login dialog on 0.85.1",
                A_PI_LOGIN_0851,
                "login",
                "Enter Cerebras API key",
            ),
            (
                "the login dialog on 0.85.1 at 20 columns",
                A_PI_LOGIN_0851_20,
                "login",
                "Enter Cerebras API key",
            ),
        ] {
            let Claim::Ruled(rule) = claim(pi(), screen, Phase::Starting) else {
                panic!("{what} is claimed by a rule");
            };
            assert_eq!(rule.name, named, "{what}");
            assert_eq!(rule.state, Phase::Waiting, "{what}");
            assert_eq!(rule.kind, Some(crate::store::Kind::Question), "{what}");
            assert_eq!(
                pi().asking(screen).map(|asked| asked.text),
                Some(sentence.to_string()),
                "{what}"
            );
        }

        // The gate's second step at 20 columns has lost its banner and top
        // border off the pane, so it falls to the dialog rule: still waiting.
        let Claim::Ruled(narrow) = claim(pi(), A_PI_SETUP_ANALYTICS_20, Phase::Starting) else {
            panic!("something still claims the screen at 20 columns");
        };
        assert_eq!(narrow.name, "dialog");
        assert_eq!(narrow.state, Phase::Waiting);
    }

    #[test]
    fn rules_a_setup_gate_with_pis_own_footer_under_it_is_a_quotation() {
        // pi draws no composer or footer under its first-run gate, so a stats
        // line below the banner means the box is quoted text. Not a capture:
        // the measured gate with a measured footer under it, the shape a
        // quotation has on a pane running a session.
        let quoted = format!(
            "{}\n~/.claude/jobs/eef72778/tmp/pipane\n\
             ↑1.5k ↓69 R1.3k CH90.3% $0.001 (sub) 0.5%/264k (auto)\n",
            A_PI_SETUP.trim_end()
        );

        let Claim::Ruled(rule) = claim(pi(), &quoted, Phase::Starting) else {
            panic!("the dialog rule still has the screen");
        };
        assert_ne!(
            rule.name, "first_time_setup",
            "a gate with a session's chrome under it is not the gate"
        );
    }

    #[test]
    fn rules_every_way_a_caller_stops_pi_is_a_screen_that_says_so() {
        // An extension can stop pi for a person three ways: `select`, `input`
        // and `editor`. Each draws its own hint row, and all three keep the
        // caller's title at the top of the box.
        for (what, screen, named, sentence) in [
            (
                "a caller asking for a choice",
                A_PI_DIALOG,
                "dialog",
                "Run echo hi?",
            ),
            (
                "a caller asking for a line",
                A_PI_INPUT,
                "input",
                "Which branch should I push to?",
            ),
            (
                "the same at 20 columns, the title wrapped in two",
                A_PI_INPUT_20,
                "input",
                "Which branch should I push to?",
            ),
            (
                "a caller asking for a block",
                A_PI_EDITOR,
                "editor",
                "Write the commit message",
            ),
            (
                "the same at 20 columns",
                A_PI_EDITOR_20,
                "editor",
                "Write the commit message",
            ),
        ] {
            let Claim::Ruled(rule) = claim(pi(), screen, Phase::Working) else {
                panic!("{what} is claimed by a rule");
            };
            assert_eq!(rule.name, named, "{what}");
            assert_eq!(rule.state, Phase::Waiting, "{what}");
            assert_eq!(rule.kind, Some(crate::store::Kind::Question), "{what}");
            assert_eq!(
                pi().asking(screen).map(|asked| asked.text),
                Some(sentence.to_string()),
                "{what}"
            );
        }
    }

    #[test]
    fn rules_a_pi_dialog_carries_the_callers_question_and_the_choices_it_marks() {
        // A gated tool call's question is whatever its caller passed, drawn
        // at the top of the box with the choices under it. The choices are
        // marked, not numbered (an arrow on the cursor row, two spaces on the
        // rest), so amx numbers the run in drawn order and records that it did.
        let Claim::Ruled(rule) = claim(pi(), A_PI_DIALOG, Phase::Working) else {
            panic!("pi's own rule claims pi's own screen");
        };
        assert_eq!(rule.kind, Some(crate::store::Kind::Question));

        let gated: &[&str] = &["Allow once", "Allow always", "Deny"];
        for (what, screen, sentence, options) in [
            ("a gated tool call", A_PI_DIALOG, "Run echo hi?", gated),
            (
                "the same at 20 columns",
                A_PI_DIALOG_20,
                "Run echo hi?",
                gated,
            ),
            (
                "a confirm, which draws its message under its title",
                A_PI_CONFIRM,
                "Push to origin? This rewrites the remote branch.",
                &["Yes", "No"],
            ),
        ] {
            let asked = pi()
                .asking(screen)
                .unwrap_or_else(|| panic!("{what} says what it is blocking on"));
            assert_eq!(asked.text, sentence, "{what}");
            assert_eq!(asked.options, options, "{what}");
            assert!(
                asked.walked,
                "{what}: the numbers are amx's own, and the record says so"
            );
        }
    }

    #[test]
    fn rules_a_cursor_somebody_moved_leaves_the_question_where_it_is() {
        // A person can move the cursor before amx looks, so the question is
        // the sentence above the run, not above the mark. Anchored on the
        // mark, a cursor one row down would quote the choice above it.
        //
        // Not a capture: the measured dialog with its arrow moved down one row,
        // which is what `Down` does on pi 0.85.1.
        let moved = A_PI_DIALOG
            .replace(" → Allow once", "   Allow once")
            .replace("   Allow always", " → Allow always");

        let asked = pi().asking(&moved).expect("the screen still blocks");
        assert_eq!(asked.text, "Run echo hi?");
        assert_eq!(
            asked.options,
            ["Allow once", "Allow always", "Deny"],
            "the list is the run, and the cursor is not part of what it says"
        );
    }

    #[test]
    fn rules_which_screens_mark_a_choice_is_the_documents_to_say() {
        // Which screens draw a marked list is the vendor's document to say.
        // claude numbers everything but its trust gate, which has drawn a
        // cursor and no digits since 2.1.259.
        let marks = |screens: &'static Ruleset| -> Vec<&'static str> {
            screens
                .rules()
                .iter()
                .filter(|rule| rule.marks.is_some())
                .map(|rule| rule.name.as_str())
                .collect()
        };
        assert_eq!(
            marks(pi()),
            [
                "first_time_setup",
                "project_trust",
                "folder_trust",
                "dialog"
            ],
            "every screen pi draws a selector on"
        );
        assert_eq!(
            marks(claude()),
            ["folder_trust"],
            "the one screen claude draws a cursor on instead of numbers"
        );
    }

    #[test]
    fn rules_a_widget_in_the_slot_pis_composer_had_is_not_pis_prompt() {
        // pi draws every widget a person opens between the composer's two
        // borders with the same footer under it, so the idle rule (a box, a
        // footer and the rows between) held on all of them. Its window was
        // counted off an empty composer.
        for (what, screen) in [
            ("a selector with nothing above it", A_PI_SELECTOR),
            (
                "the same selector under a transcript",
                A_PI_SELECTOR_UNDER_A_TRANSCRIPT,
            ),
            (
                "a selector taller than the rows a rule may see",
                A_PI_MODEL_SELECTOR,
            ),
            // 0.85.1 spells this selector's cancel key in full, and the login
            // rule read `escape/ctrl+c to` on it. The dialog's parentheses are
            // what the selector's row lacks.
            (
                "the same selector on 0.85.1, its hint row naming both keys",
                A_PI_MODEL_SELECTOR_0851,
            ),
        ] {
            assert_eq!(
                claim(pi(), screen, Phase::Starting),
                Claim::Unclaimed,
                "{what} is not a pi waiting for somebody to type"
            );
        }

        // The two captures of one widget differ only by the transcript above,
        // so they must read the same.
        assert_eq!(
            claim(pi(), A_PI_SELECTOR, Phase::Starting),
            claim(pi(), A_PI_SELECTOR_UNDER_A_TRANSCRIPT, Phase::Starting),
            "the same widget, read the same way"
        );

        // The screen the rule was measured on is still claimed, at both widths
        // and on a fresh pi.
        for (what, screen) in [
            ("a finished turn", A_PI_IDLE),
            ("the same at 24 columns", A_PI_IDLE_24),
            ("a pi nobody has typed into yet", A_PI_BOOT),
        ] {
            assert_eq!(
                claim(pi(), screen, Phase::Starting).rule_name(),
                Some("prompt"),
                "{what}"
            );
        }
    }

    #[test]
    fn rules_pi_and_claude_claim_nothing_on_each_others_panes() {
        // Each vendor's anchors are absent from the other's chrome, so a
        // wrapper around one vendor is never read confidently wrong with the
        // other's document.
        for (what, screen) in [
            ("a claude idle prompt", IDLE_SCREEN),
            ("a claude turn running", WORKING_SCREEN),
            ("a claude permission box", PERMISSION_BOX),
            ("a claude ask menu", ASK_MENU_80),
            ("a claude plan approval", PLAN_APPROVAL_220),
        ] {
            assert_eq!(
                pi().claim(screen, Phase::Working, SETTLED_LOOKS),
                Claim::Unclaimed,
                "pi's document claims {what}"
            );
        }

        for (what, screen) in [
            ("a pi prompt", A_PI_IDLE),
            ("a pi turn running", A_PI_WORKING),
            ("a pi dialog", A_PI_DIALOG),
            ("a pi asking for a line", A_PI_INPUT),
            ("a pi asking for a block", A_PI_EDITOR),
        ] {
            assert_eq!(
                claude().claim(screen, Phase::Working, SETTLED_LOOKS),
                Claim::Unclaimed,
                "claude's document claims {what}"
            );
        }
    }

    #[test]
    fn rules_pi_cuts_its_own_chrome_and_leaves_the_work() {
        // pi's furniture anchors are in the same document as its rules. The
        // walk takes the box, the directory, the stats line and the spinner
        // line above them, and the dialog too, since pi draws it inside the
        // box's borders.
        let cut = |screen: &'static str| -> Vec<&'static str> {
            let rows: Vec<&str> = screen.lines().collect();
            pi().furniture().cut(&rows).to_vec()
        };

        let idle = cut(A_PI_IDLE);
        assert_eq!(
            idle.last().map(|row| row.trim()),
            Some(""),
            "the walk stops at the box's top border"
        );
        assert!(
            !idle.iter().any(|row| row.contains("0.5%/264k")),
            "the stats line is chrome"
        );
        assert!(
            idle.iter().any(|row| row.contains("Took 15.2s")),
            "the rows the agent earned are not"
        );

        // 0.85.1 puts the word in the top border, which ends in the rule like
        // any top border.
        let working = cut(A_PI_WORKING_0851);
        assert!(
            !working.iter().any(|row| row.contains("Working")),
            "the top border goes with the word in it: {working:?}"
        );
        assert!(
            working.iter().any(|row| row.contains("single word done")),
            "and the prompt above it stays: {working:?}"
        );
        assert!(
            !cut(A_PI_WORKING)
                .iter()
                .any(|row| row.contains("Working...")),
            "so is the line a turn spins"
        );
        assert!(
            !cut(A_PI_DIALOG)
                .iter()
                .any(|row| row.contains("↑↓ navigate")),
            "and so is a dialog, which pi stages in the composer's own box"
        );
    }

    #[test]
    fn rules_the_walk_cuts_pis_status_line_whatever_it_is_saying() {
        // The walk must cut all four of pi's status lines, as the rule reads
        // them all off the frame. Anchored on one message, a compacting turn's
        // status line went onto the card and into `amx logs` as work.
        let cut = |screen: &'static str| -> Vec<&'static str> {
            let rows: Vec<&str> = screen.lines().collect();
            pi().furniture().cut(&rows).to_vec()
        };

        for (what, screen, message) in [
            (
                "a compacting turn",
                A_PI_COMPACTING,
                "Compacting context...",
            ),
            ("a retrying turn", A_PI_RETRYING, "Retrying (1/3)"),
            (
                "a turn under an extension's own message",
                A_PI_RENAMED,
                "Reviewing the diff",
            ),
        ] {
            let rows: Vec<&str> = screen.lines().collect();
            let line = rows
                .iter()
                .position(|row| row.contains(message))
                .unwrap_or_else(|| panic!("{what} has a status line"));
            assert_eq!(
                cut(screen).len(),
                line,
                "{what}: the status line goes with the chrome under it, and \
                 the rows above it stay"
            );
        }

        // Known limit at narrow widths: the frame is on the first row of a
        // wrapped message and the walk reads the last row above the box, so
        // at 20 columns a compacting turn keeps its status line. Leaving
        // chrome on screen is the direction the walk is built to err in.
        assert!(
            cut(A_PI_COMPACTING_20)
                .iter()
                .any(|row| row.contains("Compacting")),
            "a wrapped status line is left whole"
        );
    }

    /// claude 2.1.278 after an esc. The last tool result reads
    /// `Initializing…`, the spinner's own `ing…` on a transcript row. The gap
    /// after each `⎿` is a non-breaking space, as drawn.
    const INTERRUPTED_278: &str = "\
❯ Tell me about this project

● Skill(mem)
  ⎿ \u{a0}Initializing…
  ⎿ \u{a0}Error: Unknown skill: mem. Did you mean new?
  ⎿ \u{a0}Interrupted · What should Claude do instead?

────────────────────────────────────────
❯\u{a0}
────────────────────────────────────────
  Haiku 4.5 │ ◈ 9% │ amx (main) │ ◖ thinking
  ⏵⏵ bypass permissions on (shift+tab to cycle)
";

    /// The same pane at rest, with its statusline elided mid branch name,
    /// leaving a gerund ending and an ellipsis under the composer.
    const AN_ELIDED_STATUSLINE: &str = "\
● done

✻ Cogitated for 10s · done 2:19 PM

────────────────────────────────────────
❯
────────────────────────────────────────
  Haiku 4.5 │ ◈ 9% │ amx (fix-the-spinner-reading…
  ⏵⏵ bypass permissions on (shift+tab to cycle)
";

    /// claude's AskUserQuestion menu with a question that opens like a
    /// permission box's. `Enter to select` makes it the menu.
    const ASK_MENU_DO_YOU_WANT_TO: &str = "\
────────────────────────────────────────────────────────────────────────────────
 ☐ Indentation

Do you want to indent this project with spaces or tabs?

❯ 1. Spaces
  2. Tabs
  3. Type something.
────────────────────────────────────────────────────────────────────────────────
  4. Chat about this

Enter to select · ↑/↓ to navigate · Esc to cancel
";

    /// pi at rest after a tool call whose output drew a braille frame mid-row,
    /// two rows over the composer.
    const A_PI_FRAME_IN_TOOL_OUTPUT: &str = r"
 $ pnpm install

 resolving ⠏ 3/3 packages

────────────────────────────────────────────────────────────────────────────────────────────────────

────────────────────────────────────────────────────────────────────────────────────────────────────
~/.claude/jobs/eef72778/tmp/pipane
↑1.5k ↓69 R1.3k CH90.3% $0.001 (sub) 0.5%/264k (auto)          (github-copilot) gpt-5-mini • minimal
";

    /// pi's one-line input under a transcript ending in a numbered list of the
    /// agent's own.
    const A_PI_INPUT_UNDER_A_LIST: &str = r"
 Two branches are ahead of main:

 1. fix-login
 2. port-importer


────────────────────────────────────────────────────────────────────────────────────────────────────

 Which branch should I push to?

>

 enter submit  escape/ctrl+c cancel

────────────────────────────────────────────────────────────────────────────────────────────────────
~/.claude/jobs/3876e46d/tmp/pane
0.0%/1.0M (auto)                                   (opencode) muse-spark-1.3-contributor-free • high
";

    #[test]
    fn rules_claudes_spinner_is_the_row_over_its_composer_and_no_other() {
        // `ing…` is also the vendor's elision after a gerund: on a tool result
        // in the transcript, and on a truncated statusline under the composer.
        // Neither is the spinner row.
        for (what, screen) in [
            ("an interrupted turn", INTERRUPTED_278),
            ("an elided statusline", AN_ELIDED_STATUSLINE),
        ] {
            let claimed = claim(claude(), screen, Phase::Working);
            assert_eq!(claimed.rule_name(), Some("idle_prompt"), "{what}");
            assert_eq!(claimed.phase(), Some(Phase::Idle), "{what}");
        }
    }

    #[test]
    fn rules_a_frame_in_pis_tool_output_is_not_pis_spinner() {
        let claimed = claim(pi(), A_PI_FRAME_IN_TOOL_OUTPUT, Phase::Working);
        assert_eq!(claimed.rule_name(), Some("prompt"));
        assert_eq!(claimed.phase(), Some(Phase::Idle));
    }

    #[test]
    fn rules_a_menu_asking_whether_you_want_to_is_a_menu() {
        let claimed = claim(claude(), ASK_MENU_DO_YOU_WANT_TO, Phase::Working);
        assert_eq!(claimed.rule_name(), Some("ask_menu"));

        let asked = asked(claude(), ASK_MENU_DO_YOU_WANT_TO);
        assert_eq!(
            asked.text,
            "Do you want to indent this project with spaces or tabs?"
        );
        assert_eq!(
            asked.options,
            ["Spaces", "Tabs", "Type something.", "Chat about this"]
        );
    }

    #[test]
    fn rules_pis_input_under_a_numbered_list_keeps_its_question() {
        // The list is agent output above the box, not choices: the question is
        // read above the input row, however many numbered rows sit higher.
        let asked = asked(pi(), A_PI_INPUT_UNDER_A_LIST);
        assert_eq!(asked.text, "Which branch should I push to?");
        assert!(asked.options.is_empty(), "{:?}", asked.options);
    }

    #[test]
    fn rules_a_ruleset_that_is_not_one_is_an_error() {
        assert!(
            Ruleset::parse("[[rule]]\nname = \"x\"\n").is_err(),
            "no state"
        );
        assert!(
            Ruleset::parse("[[rule]]\nname = \"x\"\nstate = \"pondering\"\n").is_err(),
            "not a state amx has"
        );
        assert!(Ruleset::parse("rule = ").is_err(), "not TOML");
    }

    // codex 0.157.1. The captures under tests/codex/screens are from `codex
    // --no-daemon` with `capture-pane -p -J` at the width in each name and
    // forty rows; see docs/codex-screens.md.

    fn codex() -> Ruleset {
        Ruleset::parse(include_str!("../assets/screen-rules-codex.toml"))
            .expect("codex's screens parse")
    }

    #[test]
    fn rules_codex_is_the_document_a_codex_pane_is_read_by() {
        // amx starts codex with its launch word first.
        let read = of("codex --no-daemon --model gpt-6-luna -- fix the login bug");
        assert!(std::ptr::eq(read, of("codex")));
        assert!(std::ptr::eq(read, of("/usr/local/bin/codex --no-daemon")));
        assert_eq!(named(read), named(&codex()));
        assert!(!std::ptr::eq(read, of("claude")));
    }

    /// A capture file as amx reads a pane: trailing blank rows dropped (as
    /// `Server::run` does), then sanitized.
    fn as_read(capture: &str) -> String {
        crate::tmux::sanitize(capture.trim_end())
    }

    /// One screen at the four widths it was captured at.
    macro_rules! codex_widths {
        ($name:literal) => {
            [
                (
                    220,
                    include_str!(concat!("../tests/codex/screens/", $name, "-220.txt")),
                ),
                (
                    100,
                    include_str!(concat!("../tests/codex/screens/", $name, "-100.txt")),
                ),
                (
                    54,
                    include_str!(concat!("../tests/codex/screens/", $name, "-54.txt")),
                ),
                (
                    24,
                    include_str!(concat!("../tests/codex/screens/", $name, "-24.txt")),
                ),
            ]
        };
    }

    #[test]
    fn rules_codex_reads_the_screens_it_draws() {
        assert_eq!(
            named(&codex()),
            [
                "update",
                "hooks_review",
                "folder_trust",
                "cwd_prompt",
                "approval",
                "question",
                "working",
                "prompt",
                "interrupted"
            ],
            "order decides, so it is part of the data"
        );
    }

    #[test]
    fn rules_codex_gates_a_run_with_update_trust_hooks_and_the_resume_directory() {
        assert_eq!(
            gates(&codex()),
            ["update", "hooks_review", "folder_trust", "cwd_prompt"],
            "Ruling 7: amx answers none of them, the person does"
        );
    }

    #[test]
    fn rules_codex_names_every_screen_at_every_width() {
        let codex = codex();
        for (screen, captures, rule, means) in [
            ("idle", codex_widths!("idle"), "prompt", Phase::Idle),
            (
                "idle after a turn",
                codex_widths!("idle-after"),
                "prompt",
                Phase::Idle,
            ),
            (
                "a turn running",
                codex_widths!("working"),
                "working",
                Phase::Working,
            ),
            (
                "a command waiting for approval",
                codex_widths!("approval"),
                "approval",
                Phase::Waiting,
            ),
            (
                "request_user_input",
                codex_widths!("question"),
                "question",
                Phase::Waiting,
            ),
            (
                "folder trust",
                codex_widths!("trust"),
                "folder_trust",
                Phase::Waiting,
            ),
            (
                "hooks need review",
                codex_widths!("hooks-review"),
                "hooks_review",
                Phase::Waiting,
            ),
            (
                "the resume working directory",
                codex_widths!("cwd-prompt"),
                "cwd_prompt",
                Phase::Waiting,
            ),
            (
                "the update prompt",
                codex_widths!("update"),
                "update",
                Phase::Waiting,
            ),
            (
                "an Esc'd turn with a queued message put back in the composer",
                codex_widths!("interrupted"),
                "interrupted",
                Phase::Idle,
            ),
        ] {
            for (width, capture) in captures {
                let claimed = claim(&codex, &as_read(capture), Phase::Working);
                assert_eq!(
                    (claimed.rule_name(), claimed.phase()),
                    (Some(rule), Some(means)),
                    "{screen} at {width} columns"
                );
            }
        }
    }

    #[test]
    fn rules_codex_names_what_a_turn_draws_around_its_status_row() {
        let codex = codex();
        for (screen, capture, rule) in [
            (
                "a message steered into the turn, waiting for the next tool call",
                include_str!("../tests/codex/screens/steer-pending-100.txt"),
                "working",
            ),
            (
                "a message queued with Tab",
                include_str!("../tests/codex/screens/queued-100.txt"),
                "working",
            ),
            (
                "a turn that ended on an API error",
                include_str!("../tests/codex/screens/error-100.txt"),
                "prompt",
            ),
        ] {
            assert_eq!(
                claim(&codex, &as_read(capture), Phase::Working).rule_name(),
                Some(rule),
                "{screen}"
            );
        }
    }

    #[test]
    fn rules_codex_claims_nothing_on_a_shell_or_another_vendors_pane() {
        let codex = codex();
        for (what, screen) in [
            ("a shell", A_SHELL),
            ("claude at its prompt", IDLE_SCREEN),
            ("claude mid-turn", WORKING_SCREEN),
            ("pi at its prompt", A_PI_IDLE),
        ] {
            assert_eq!(
                claim(&codex, screen, Phase::Working),
                Claim::Unclaimed,
                "{what}"
            );
        }
    }

    // opencode 2.0.16. The captures under tests/opencode/screens are from
    // `opencode --standalone` with `capture-pane -p -J` at the width in each
    // name and forty rows; see docs/opencode-screens.md.

    fn opencode() -> Ruleset {
        Ruleset::parse(include_str!("../assets/screen-rules-opencode.toml"))
            .expect("opencode's screens parse")
    }

    #[test]
    fn rules_opencode_is_the_document_an_opencode_pane_is_read_by() {
        // amx starts opencode with its launch word first and the task on
        // `--prompt=`.
        let read = of("opencode --standalone --prompt=fix the login bug");
        assert!(std::ptr::eq(read, of("opencode")));
        assert!(std::ptr::eq(
            read,
            of("/usr/local/bin/opencode --standalone")
        ));
        assert_eq!(named(read), named(&opencode()));
        assert!(!std::ptr::eq(read, of("claude")));
        assert!(!std::ptr::eq(read, of("codex")));
    }

    /// One opencode screen at the four widths it was captured at.
    macro_rules! opencode_widths {
        ($name:literal) => {
            [
                (
                    220,
                    include_str!(concat!("../tests/opencode/screens/", $name, "-220.txt")),
                ),
                (
                    100,
                    include_str!(concat!("../tests/opencode/screens/", $name, "-100.txt")),
                ),
                (
                    54,
                    include_str!(concat!("../tests/opencode/screens/", $name, "-54.txt")),
                ),
                (
                    24,
                    include_str!(concat!("../tests/opencode/screens/", $name, "-24.txt")),
                ),
            ]
        };
    }

    #[test]
    fn rules_opencode_reads_the_screens_it_draws() {
        assert_eq!(
            named(&opencode()),
            [
                "connect",
                "no_provider",
                "permission",
                "question",
                "working",
                "prompt"
            ],
            "order decides, so it is part of the data"
        );
    }

    #[test]
    fn rules_opencode_gates_a_run_with_no_provider_to_call() {
        assert_eq!(
            gates(&opencode()),
            ["connect", "no_provider"],
            "only the person can connect a provider"
        );
    }

    #[test]
    fn rules_opencode_names_every_screen_at_every_width() {
        let opencode = opencode();
        for (screen, captures, rule, means) in [
            ("idle", opencode_widths!("idle"), "prompt", Phase::Idle),
            (
                "idle after a turn",
                opencode_widths!("idle-after"),
                "prompt",
                Phase::Idle,
            ),
            (
                "a turn running",
                opencode_widths!("working"),
                "working",
                Phase::Working,
            ),
            (
                "a turn running after one Esc",
                opencode_widths!("esc-once"),
                "working",
                Phase::Working,
            ),
            (
                "a turn interrupted with Esc twice",
                opencode_widths!("interrupted"),
                "prompt",
                Phase::Idle,
            ),
            (
                "a shell command waiting for permission",
                opencode_widths!("permission"),
                "permission",
                Phase::Waiting,
            ),
            (
                "the question tool's form",
                opencode_widths!("question"),
                "question",
                Phase::Waiting,
            ),
            (
                "no provider to call",
                opencode_widths!("no-provider"),
                "no_provider",
                Phase::Waiting,
            ),
            (
                "the connect dialog a send with no provider opens",
                opencode_widths!("connect"),
                "connect",
                Phase::Waiting,
            ),
        ] {
            for (width, capture) in captures {
                let claimed = claim(&opencode, &as_read(capture), Phase::Working);
                assert_eq!(
                    (claimed.rule_name(), claimed.phase()),
                    (Some(rule), Some(means)),
                    "{screen} at {width} columns"
                );
            }
        }
    }

    #[test]
    fn rules_opencode_names_what_a_turn_draws_around_its_footer() {
        let opencode = opencode();
        for (screen, capture, rule) in [
            (
                "a message steered into the turn, waiting for the next step",
                include_str!("../tests/opencode/screens/steer-pending-100.txt"),
                "working",
            ),
            (
                "a provider error being retried",
                include_str!("../tests/opencode/screens/retrying-100.txt"),
                "working",
            ),
            (
                "a turn that ended on a provider error",
                include_str!("../tests/opencode/screens/error-100.txt"),
                "prompt",
            ),
        ] {
            assert_eq!(
                claim(&opencode, &as_read(capture), Phase::Working).rule_name(),
                Some(rule),
                "{screen}"
            );
        }
    }

    #[test]
    fn rules_opencode_claims_nothing_on_a_shell_or_another_vendors_pane() {
        let opencode = opencode();
        for (what, screen) in [
            ("a shell", A_SHELL),
            ("claude at its prompt", IDLE_SCREEN),
            ("claude mid-turn", WORKING_SCREEN),
            ("pi at its prompt", A_PI_IDLE),
            (
                "codex at its prompt",
                include_str!("../tests/codex/screens/idle-100.txt"),
            ),
            (
                "codex mid-turn",
                include_str!("../tests/codex/screens/working-100.txt"),
            ),
        ] {
            assert_eq!(
                claim(&opencode, &as_read(screen), Phase::Working),
                Claim::Unclaimed,
                "{what}"
            );
        }
    }
}
