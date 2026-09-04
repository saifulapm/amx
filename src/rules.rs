//! Reading an agent's screen when its hooks have gone quiet.
//!
//! Hooks are precise and win while they flow. When they stop — the vendor was
//! interrupted with Escape, or nothing has happened for a while — the pane
//! itself is the only witness left, and this is what amx knows how to see in
//! it: the rules in the vendor's own screens document — claude's is
//! `assets/screen-rules.toml` — matched against the bottom of the capture.
//!
//! The ruleset is small on purpose. A screen no rule claims is `unknown`, and
//! `unknown` with its age shown is a better answer than a confident wrong one:
//! naming a screen also clears any question off the row, so a wrong match can
//! delete a question a person is being asked.
//!
//! One document per vendor, and the vendor's own entry in the table is what
//! points at it. Every string in a document is that vendor's own — the words
//! in its widgets, the glyphs it draws its chrome with, the sentences it sends
//! about a dialog it will not describe — so which document is read follows
//! from the program an agent command runs, and no vendor's screen is spelled
//! out in Rust here.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::sync::OnceLock;

use crate::furniture::Furniture;
use crate::registry;
use crate::store::{Phase, Question};

/// How many rows up from the bottom of the capture a rule may see. The
/// vendor's chrome sits at the bottom; the rest is the agent's own output,
/// which is not evidence about the vendor's state.
pub const FLOOR_LINES: usize = 24;

/// How many consecutive unchanged looks a quiescent rule needs before it may
/// end a turn that is on the record as running.
///
/// The idle screen and a mid-turn pause are the same bytes, so only time tells
/// them apart. The longest mid-turn stillness measured at one look a second is
/// ten seconds, at the tail of a turn after the answer stopped streaming and
/// before the Stop hook arrived; this is three times that.
pub const SETTLED_LOOKS: usize = 30;

/// Everything amx knows how to read on one vendor's screens: the rules, in
/// the order they are asked, and the chrome underneath them.
#[derive(Debug, Deserialize)]
pub struct Ruleset {
    #[serde(default)]
    furniture: Furniture,
    #[serde(default)]
    placeholders: Vec<String>,
    #[serde(default, rename = "rule")]
    rules: Vec<Rule>,
}

/// One screen amx can recognise.
#[derive(Debug, PartialEq, Eq, Deserialize)]
pub struct Rule {
    /// What this screen is called, for `status` to name its evidence.
    pub name: String,
    /// What the screen means.
    pub state: Phase,
    /// Every one of these must appear.
    #[serde(default)]
    pub all: Vec<String>,
    /// At least one of these must appear — the widget that makes prose a
    /// prompt.
    #[serde(default)]
    pub any: Vec<String>,
    /// How many rows the matched strings may span. A blocking prompt is a box,
    /// not two things that happen to share a screen.
    #[serde(default)]
    pub within: Option<usize>,
    /// None of these may appear below the match. claude draws no composer
    /// under a blocking prompt, so a widget with the mode footer beneath it is
    /// a quotation of a widget rather than one.
    #[serde(default)]
    pub not_below: Vec<String>,
    /// Whether this rule needs the screen to have held still before it may end
    /// a running turn.
    #[serde(default)]
    pub quiescent: bool,
    /// Where this screen keeps the question it is asking, for the screens that
    /// do not keep it where a screen usually does.
    #[serde(default)]
    pub asks: Asks,
    /// What this screen wants back, which is what decides what may be sent to
    /// it. Every screen that blocks has one; a screen that is a state rather
    /// than a question wants nothing and says so by leaving this out.
    #[serde(default)]
    pub kind: Option<crate::store::Kind>,
}

/// What the screen had to say.
#[derive(Debug, Clone, PartialEq)]
pub enum Claim<'a> {
    /// This rule claims the screen, and may say so.
    Ruled(&'a Rule),
    /// This rule claims the screen, but a turn is on the record as running and
    /// the screen has not held still long enough to end it.
    Unsettled(&'a Rule),
    /// Nothing amx knows accounts for this screen.
    Unclaimed,
}

impl Claim<'_> {
    /// The state to report, when there is one.
    pub fn phase(&self) -> Option<Phase> {
        match self {
            Claim::Ruled(rule) => Some(rule.state),
            _ => None,
        }
    }

    /// Which rule spoke, for `status` to name.
    pub fn rule_name(&self) -> Option<&str> {
        match self {
            Claim::Ruled(rule) | Claim::Unsettled(rule) => Some(rule.name.as_str()),
            Claim::Unclaimed => None,
        }
    }
}

/// Every registered vendor's screens, parsed once and kept by the name of the
/// vendor that draws them.
///
/// A vendor that declares none is not in here at all, which is the difference
/// between screens amx has measured and screens it has not.
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

/// The screens amx reads on the pane of an agent running `agent`.
///
/// The vendor that command runs, and for a command amx has no entry for the
/// vendor it runs by default. An unregistered command is not another vendor:
/// it is a command line somebody wrote, routinely a wrapper around the vendor
/// amx was written against. Every anchor in that vendor's document is its own,
/// so a pane it was not drawn for is claimed by nothing rather than claimed
/// wrongly, and a wrapper keeps the reading it has always had.
///
/// A vendor whose screens nobody has measured reads nothing whatsoever, which
/// is the floor an entry stands on before anybody has sat in front of it: a
/// pane to watch, and no claim about what is in it.
pub fn of(agent: &str) -> &'static Ruleset {
    let vendor = registry::entry(agent).or_else(|| registry::entries().first());
    select(parsed(), vendor.map_or("", |vendor| vendor.name)).unwrap_or_else(unmeasured)
}

/// The screens `vendor` draws, out of the ones amx has parsed.
fn select<'a>(parsed: &'a [(&'static str, Ruleset)], vendor: &str) -> Option<&'a Ruleset> {
    parsed
        .iter()
        .find(|(name, _)| *name == vendor)
        .map(|(_, screens)| screens)
}

/// The screens of a vendor amx has measured none of: no rule, so no claim.
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

    /// The chrome this vendor draws under every pane it has the room for, as
    /// the anchors that find it. The rules read a screen; this is what a
    /// surface printing one cuts off it — see [`crate::furniture`].
    pub fn furniture(&self) -> &Furniture {
        &self.furniture
    }

    /// Whether this is one of the sentences the vendor sends in place of a
    /// question — a message about a dialog that says nothing about what the
    /// dialog is asking.
    ///
    /// A whole sentence and never the start of one. The vendor sends a longer
    /// one naming the tool it is about, and that one is something a caller can
    /// act on; which of the two lands last is the vendor's business.
    pub fn placeholder(&self, sentence: &str) -> bool {
        self.placeholders.iter().any(|said| said == sentence)
    }

    /// Ask the screen what it is.
    ///
    /// `recorded` is the state amx has on file and `still_looks` is how many
    /// consecutive looks have found this same screen — together they decide
    /// whether a quiescent rule is allowed to end a turn.
    pub fn claim(&self, capture: &str, recorded: Phase, still_looks: usize) -> Claim<'_> {
        let screen = Screen::new(capture);
        // Ordered: the first rule that holds decides, and the rest are not
        // asked. A screen the specific rules have named is not also the
        // furniture underneath them.
        let Some(rule) = self.rules.iter().find(|rule| rule.holds(&screen)) else {
            return Claim::Unclaimed;
        };
        if rule.may_decide(recorded, still_looks) {
            Claim::Ruled(rule)
        } else {
            Claim::Unsettled(rule)
        }
    }

    /// What the screen is asking, without asking it what the agent is doing.
    ///
    /// [`claim`](Ruleset::claim) weighs a screen against what amx already
    /// believes, because naming a screen can end a turn and that is a decision
    /// about the record. Reading the question off one is not: the caller here
    /// has a record that says the agent is waiting and cannot say what for.
    /// The same rule order decides, and the quiescence gate has nothing to say
    /// — it governs which rule may end a turn, and no rule that asks a
    /// question is quiescent.
    pub fn asking(&self, capture: &str) -> Option<Question> {
        let screen = Screen::new(capture);
        let rule = self.rules.iter().find(|rule| rule.holds(&screen))?;
        rule.question(capture)
    }
}

impl Rule {
    /// Whether this rule's conditions hold on the screen.
    fn holds(&self, screen: &Screen) -> bool {
        let mut matched = Vec::with_capacity(self.all.len() + 1);
        for needle in &self.all {
            match screen.row_of(needle) {
                Some(row) => matched.push(row),
                None => return false,
            }
        }

        if !self.any.is_empty() {
            // The affordance: the highest row any of them is on. Everything
            // below it is the rest of the box — or, on a quotation, the
            // vendor's own chrome, which is what gives the guard something to
            // find.
            match self.any.iter().filter_map(|n| screen.row_of(n)).min() {
                Some(row) => matched.push(row),
                None => return false,
            }
        }

        let (Some(&first), Some(&last)) = (matched.iter().min(), matched.iter().max()) else {
            // A rule with no conditions at all claims nothing.
            return false;
        };

        if let Some(within) = self.within
            && last - first > within
        {
            return false;
        }

        !screen.any_below(last, &self.not_below)
    }

    /// What this screen is asking, read off the capture this rule claimed.
    ///
    /// Only a blocking screen is asking anything: a spinner and an idle prompt
    /// are states, not questions. The rest is where each screen keeps its
    /// question, which is not the same place on any two of them — see
    /// [`Asks`].
    pub fn question(&self, capture: &str) -> Option<Question> {
        if self.state != Phase::Waiting {
            return None;
        }

        let screen = Screen::new(capture);
        let choices = screen.first_option();
        let (from, to) = match &self.asks {
            Asks::Sentence(anchor) => screen.sentence_at(screen.row_above(choices, anchor)?),
            Asks::Above(anchor) => screen.sentence_above(screen.row_above(choices, anchor)?)?,
            Asks::AboveOptions => screen.sentence_above(choices?)?,
        };

        let text = screen.joined(from, to);
        (!text.is_empty()).then(|| Question {
            options: screen.options_below(to),
            text,
        })
    }

    /// Whether this rule may speak, given what amx already believes.
    ///
    /// A rule that is not quiescent always may. A quiescent one may end a turn
    /// only once the screen has held still; from a state with nothing
    /// outstanding it decides at once, which is what gets a parked agent named
    /// rather than left at `starting`.
    fn may_decide(&self, recorded: Phase, still_looks: usize) -> bool {
        if !self.quiescent {
            return true;
        }
        match recorded {
            // Nothing is outstanding, so there is no turn to end.
            Phase::Starting | Phase::Unknown => true,
            _ => still_looks >= SETTLED_LOOKS,
        }
    }
}

/// Where on a claimed screen its question is written.
///
/// The blocking screens do not all keep it in the same place and no one
/// reading finds it on every one of them, so each rule says which of these its
/// own screen wants. `asks = { sentence = "do you want to" }` or `asks =
/// { above = "→" }` in the document, or nothing at all for the usual place.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Asks {
    /// The sentence that ends just above the first choice, which is where a
    /// screen amx has not met yet is likeliest to keep it — so it is what a
    /// rule saying nothing gets.
    #[default]
    AboveOptions,
    /// The sentence the lowest row carrying this string belongs to, wrap and
    /// all. For the screens that draw something under their question that is
    /// not part of it: the tool a request is about, a sentence about what the
    /// vendor will be able to do, a link to a guide.
    Sentence(String),
    /// The sentence that ends just above the lowest row carrying this string.
    /// The same reading as [`Asks::AboveOptions`] on a screen whose choices
    /// are not numbered, so the rule has to say for itself what the question
    /// sits above: the glyph a vendor marks its selected choice with, or the
    /// row it draws for the words it is waiting to be given.
    Above(String),
}

/// The part of a capture a rule is allowed to look at: the bottom rows, twice
/// over — case folded for the anchors to match against, and as the pane drew
/// it for a question to be read out of.
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

    /// The topmost row carrying `needle`.
    fn row_of(&self, needle: &str) -> Option<usize> {
        self.folded.iter().position(|row| row.contains(needle))
    }

    /// Whether any of `needles` appears below `row`.
    fn any_below(&self, row: usize, needles: &[String]) -> bool {
        self.folded
            .iter()
            .skip(row + 1)
            .any(|line| needles.iter().any(|needle| line.contains(needle)))
    }

    /// The row the choices start on.
    ///
    /// The lowest one that reads as the first choice, not the topmost: a
    /// blocking screen is the last thing the vendor draws, and an agent's own
    /// output above it writes numbered lists every day.
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

    /// The rows this one is a sentence with.
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

    /// The rows of the sentence that ends above the choices.
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

    /// Rows `from` to `to` as the one sentence the vendor wrapped them out of.
    fn joined(&self, from: usize, to: usize) -> String {
        self.shown[from..=to]
            .iter()
            .map(|row| row.trim())
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string()
    }

    /// The choices under `row`, in the order they are drawn.
    ///
    /// Numbered from one and counting up, which is what makes a description
    /// under a label, a rule drawn through the list, or a stray line of the
    /// agent's own prose unable to join in.
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
}

/// One numbered choice, as the vendor draws it: `❯ 1. Yes` for the one under
/// the cursor and `  2. No` for the rest.
///
/// A label the vendor wrapped is read as far as its own row goes. The rows
/// under it cannot be joined on, because that is exactly where the menu keeps
/// the descriptions of its choices, at the same indent and telling nothing
/// apart.
fn option_on(row: &str) -> Option<(usize, &str)> {
    let row = row.trim_start();
    let row = row.strip_prefix('❯').map_or(row, str::trim_start);
    let (number, label) = row.split_once(". ")?;
    let number = number.parse().ok()?;
    let label = label.trim();
    (!label.is_empty()).then_some((number, label))
}

/// Whether a row has anything on it but the vendor's furniture.
fn content(row: &str) -> bool {
    let row = row.trim();
    !row.is_empty() && !row.chars().all(|glyph| glyph == '─' || glyph == '-')
}

/// Whether a row is the rest of the row above it. Word wrap breaks a sentence
/// mid-way, and the vendor's prose starts its sentences with a capital.
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

    // ── Screens measured off a live claude ───────────────────────────────────
    // Every capture below came off a running vendor, at the version, date and
    // width named with it. They are the evidence these rules are answerable
    // to: a rule that only matches a screen amx made up is a transcription,
    // not a measurement.

    /// The turn is over, claude's own summary line is still on the transcript,
    /// and the prompt is waiting for a person. v2.1.226, auto mode.
    const IDLE_SCREEN: &str = "\
  It ran for the full 40 seconds and exited cleanly.

✻ Worked for 2m 26s

──────────────────────────────────────── amx ──
❯
────────────────────────────────────────────────
  Opus 5 (1M context) │ ◈ 4% │ fix-login-a1b (amx/fix-login-a1b) │ ◖ xhigh
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    /// The same screen in manual mode, captured 2026-08-12 at 220 columns —
    /// identical but for the footer, which carries no cycle hint in this mode
    /// at any width.
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

    /// claude mid-turn while an answer streams. The vendor drops its spinner
    /// line entirely while output flows, so with the control characters
    /// stripped this differs from the idle screen only in the transcript above
    /// it. Nothing here says whether a turn is running.
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

    /// A promptless boot left alone: the welcome box, an empty prompt, and
    /// rows of nothing between them. An earlier ruleset read this as `unknown`
    /// for 183 consecutive samples.
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

    /// Mid-turn with the spinner up, 1m 54s into a turn whose Bash call has
    /// been sleeping for ten seconds. The mode footer is on this screen too,
    /// which is why rule order and not the footer decides.
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
    /// wording the rule has to survive.
    const THINKING_SCREEN: &str = "\
✽ Nesting… (15s · still thinking with xhigh effort)
──────────────────────────────────────── amx ──
❯
────────────────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    /// The folder-trust screen as v2.1.226 renders it on a 220-column pane,
    /// captured 2026-08-12.
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

    /// The same screen on a 54-column pane — five agents tiled on one wall,
    /// which is a shape amx creates by itself. The vendor wraps the sentence
    /// across four rows, and the break falls between `you` and `trust`.
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

    /// The plan-mode approval screen claude's ExitPlanMode tool draws once a
    /// plan is ready, at 220 columns. v2.1.237, 2026-08-21, seen live by
    /// entering plan mode and asking for a plan.
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
    /// `execute.` — the fragment `ready to execute` cannot be the anchor, it
    /// breaks right here.
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

    /// The same screen at 24 columns. Here the vendor wraps the other way —
    /// between `to` and `proceed?` — so `would you like to proceed` cannot be
    /// the anchor either. Only single words survive both widths.
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

    /// A live permission box: v2.1.226, 2026-08-12, 220 columns, forced out of
    /// the vendor with manual permissions and an ask rule for Bash. A full
    /// width rule with the request under it, and no mode footer anywhere.
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

    /// The AskUserQuestion menu at 80 columns. v2.1.229, 2026-08-15.
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

    /// The same menu at 24 columns, where the footer wraps: `esc to cancel` is
    /// no longer a contiguous substring, and eighteen rows separate the
    /// selection marker from the footer.
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

    /// An agent quoting another agent's pane back as a tool result — which is
    /// what amx's own callers have agents do. The quotation is the widget,
    /// character for character.
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

    /// The same attack under a manual-mode footer, captured live 2026-08-12
    /// off the pane of an agent asked to quote a permission box. The cycle
    /// hint the layout guard once keyed on is absent here; the glyph is not.
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

    /// An ordinary answer that happens to carry `do you want to` above a
    /// markdown numbered list — the two things a markdown answer produces
    /// every day. The `❯` on its own row is the composer, which is on every
    /// screen this vendor draws.
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

    /// The other anchor in the same shape: an answer discussing trust.
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

    /// One plain English sentence carrying both of the trust rule's fragments
    /// on a single row. No widget, no list, and no string anchor can tell it
    /// from a prompt.
    const PROSE_WITH_A_CONFIRM_FOOTER_IN_IT: &str = "\
  Pick the folder you trust, press Enter to confirm, and it takes care of
  the rest.

──────────────────────────────────────── amx ──
❯
────────────────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    /// A numbered plan staged in the composer — what `send` leaves when the
    /// paste lands and the submit does not. The composer row then IS
    /// `❯ 1. …`, the widget's own option shape, carrying the anchor word too.
    const A_PLAN_STAGED_IN_THE_COMPOSER: &str = "\
  Working through the migration now.

──────────────────────────────────────── amx ──
❯ 1. Update the trust store before the rollout
────────────────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    /// The auto-mode footer on a 40-column pane — four agents on a 160-column
    /// terminal. The vendor truncates its own hint from the right.
    const FOOTER_AUTO_40: &str = "\
  Ran the migration.

──────────────────────────────────────
❯
──────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to      ·
";

    /// The second vendor stopped on a question, drawn the way its own
    /// document describes: its anchor on the row the question opens with, and
    /// choices numbered with no cursor glyph in front of them.
    const A_SECOND_VENDOR_ASKING: &str = "\
 pick one and I will carry on
 about the file you named
 1. keep it
 2. drop it
 answer with a number
";

    /// Nothing this ruleset knows: an ordinary shell, which is what a pane
    /// shows when the vendor has exited.
    const A_SHELL: &str = "\
$ ls
Cargo.toml  README.md  src  tests
$
";

    // ── Screens measured off a live pi ───────────────────────────────────────
    // pi 0.84.4, driven on 2026-09-04 in a tmux pane captured the way
    // `src/tmux.rs` captures one, at the width named with each. The dialogs
    // were raised by an extension gating the bash tool with `ctx.ui.select`,
    // which is how a caller asks pi a question. Trailing spaces are off the
    // rows and nothing else is: every reading here trims or asks whether a row
    // contains something, so the pane's own padding changes no answer. They
    // are raw strings rather than continued ones because a leading space and a
    // leading blank row are both things `\` at the end of a line eats, and
    // both are on these captures.
    //
    // These are checked in so the suite runs on a machine with no pi on it.
    // Re-measure at every vendor bump — see `assets/screen-rules-pi.toml`.

    /// A pi nobody has typed into yet: the banner, the box, and a footer whose
    /// stats line has nothing but the context window to say. 100 columns, and
    /// the vendor pads the rest of the pane out rather than sitting at the
    /// bottom of it.
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

    /// The same pane with a turn running on it. pi spins its line above the
    /// box and keeps it there for the whole turn, streaming answer and all.
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

    /// pi compacting the context at 100 columns, raised with `/compact`.
    /// `Working...` is off the pane while this is up: the vendor takes the
    /// working indicator down and puts this one where it was, on the same row
    /// with the same frame in front of it.
    ///
    /// Measured 2026-09-05 against a provider that takes the summarisation
    /// request and never answers it, which is what holds the screen still long
    /// enough to read. The footer's first row is `~` because the run was made
    /// from a home of its own.
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

    /// The same screen at 20 columns, where the message wraps across three
    /// rows and the frame stays on the first of them. This is the widest span
    /// measured between a frame and the box below it, and it is what the
    /// spinner rule's `within` is counted off.
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

    /// A turn that lost its provider and is waiting to try again, at 100
    /// columns. Measured 2026-09-05 against a provider answering 503 to every
    /// call, which is the error pi retries.
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

    /// A turn running under a working message an extension wrote, at 100
    /// columns. `ctx.ui.setWorkingMessage` takes `Working...` off the row and
    /// leaves everything else about it alone; what the extension puts there is
    /// its own, and this one says `Reviewing the diff`.
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

    /// A shell command somebody ran in the pane with `!cmd`, at 100 columns.
    /// pi draws a box of its own for it in the transcript with the composer
    /// still under that, and spins the same frame on the row inside it.
    /// Measured 2026-09-05 with `!sleep 20`.
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

    /// pi blocked on a dialog at 100 columns. The dialog is drawn inside the
    /// composer box, the working directory and the stats line are under it as
    /// on any other screen, and the spinner is still up above it.
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

    /// The same dialog at 20 columns, the narrowest pane pi draws its box wide
    /// enough for. The hint row wraps across four rows and `enter select`
    /// breaks between its two words; `↑↓ navigate` is what the row still opens
    /// with.
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

    /// pi stopped on `ctx.ui.input` at 100 columns: the caller's title, the row
    /// pi draws for the line it is waiting for, and a hint row of its own.
    /// Measured 2026-09-05 on a fresh `--offline` boot with an extension that
    /// does nothing but raise the dialog.
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
    /// hint row across three; `enter submit` is what the row still opens with.
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

    /// pi stopped on `ctx.ui.editor` at 100 columns. The same box, with a
    /// second one inside it for the block of text being asked for, and a hint
    /// row that says how to end a line as well as how to submit.
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

    /// The same screen at 20 columns, where the hint row wraps across six rows
    /// and `shift+enter/ctrl+j` is still whole on one of them.
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

    /// `ctx.ui.confirm` at 100 columns, which draws the same box with two
    /// choices and the caller's message on the row under its title. Measured
    /// 2026-09-05 with an extension that does nothing but raise the dialog.
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

    /// pi asking whether this folder is one to trust, at 100 columns, raised
    /// with `/trust` on a worktree the vendor has nothing saved about. The
    /// same box the dialog above is drawn in, ending in the same hint row,
    /// with nothing running over it: a person raises this screen before a turn
    /// rather than a tool call raising it inside one.
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

    /// The same question at 20 columns. This is the tallest box pi draws, and
    /// on a tree the depth amx cuts its own it is taller than the rows a rule
    /// may see: the title and the top border are both above the floor, and
    /// what is left to read the screen by is the hint row and the one border
    /// under it.
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

    /// The gate pi puts in front of a first run, at 100 columns on a pane of
    /// 30 rows, with `PI_EXPERIMENTAL=1` and an agent directory with no
    /// `settings.json` in it. Its own startup screen rather than the pane a
    /// session runs in: the box is the whole of it, and the rows under it are
    /// the empty pane. Six of those rows are why there is no `within` on this
    /// rule — they push the box's top border above the floor, and the topmost
    /// border left to find is the box's own bottom.
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

    /// The second step of the same gate at 20 columns. Four rows of prose about
    /// what sharing usage data means, wrapped until the box is taller than the
    /// pane: the banner has scrolled off with the top border, and what is left
    /// is the hint row and the border under it.
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

    /// pi waiting for the key a provider wants, at 100 columns, driven with
    /// `/login` through the two selectors in front of it. The box is in the
    /// composer's slot with the footer under it, and the rows above it are the
    /// vendor's own warning that it has no models to run a turn with — the row
    /// that spells `/login to log into`, which is why this rule cannot anchor
    /// on the title.
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

    /// The same screen at 20 columns, where the hint row takes a second row and
    /// the span from the box's top border to it is 6 — the widest measured, and
    /// what `within` on that rule is.
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

    /// The turn is over and the prompt is waiting for a person. The line pi
    /// spins is off the screen; the box, the working directory and the stats
    /// line are where they were.
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

    /// The same idle screen at 24 columns. The stats line is truncated from
    /// the right and the context indicator the wide screens carry is gone,
    /// which is why the tokens count has to be an anchor of its own.
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

    fn claim<'a>(rules: &'a Ruleset, screen: &str, recorded: Phase) -> Claim<'a> {
        rules.claim(screen, recorded, SETTLED_LOOKS)
    }

    /// claude's screens, read the way a reader reaches them: by naming the
    /// vendor whose pane the capture came off.
    fn claude() -> &'static Ruleset {
        of("claude")
    }

    /// The documents these tests weigh against each other: the two amx ships
    /// and the second vendor's, which shares no string with either. A law that
    /// holds for all three is a law about the machinery.
    fn documents() -> Vec<(&'static str, Ruleset)> {
        [claude::VENDOR, pi::VENDOR, SECOND]
            .iter()
            .map(|vendor| {
                let screens = vendor.screens.expect("each of these has screens");
                (vendor.name, Ruleset::parse(screens).expect(vendor.name))
            })
            .collect()
    }

    /// The names of the rules in a document, which is what says which document
    /// it is.
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
        // The floor an entry stands on before anybody has sat in front of it.
        // Claiming nothing is what `unknown` is made of, and it is the right
        // answer about a screen amx has never seen.
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
                "idle_prompt"
            ],
            "order decides, so it is part of the data"
        );
    }

    #[test]
    fn rules_every_anchor_is_folded_the_way_the_screen_is() {
        // Matching folds the capture's case; an anchor with a capital in it
        // could never match, and would fail silently. True of every document,
        // because it is the matching that folds and not the vendor.
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
        // A match arm on a rule name reads one vendor's document with
        // another's names in hand, and the second vendor's screens are called
        // something else entirely: every arm would miss them without a word.
        // What a screen wants — where its question is, what it asks for — is
        // written beside the rule, and the name a verdict carries is only ever
        // looked up in the document it came out of.
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
            ("a live permission box", PERMISSION_BOX),
            ("the ask menu at 80 columns", ASK_MENU_80),
            ("the ask menu at 24 columns", ASK_MENU_24),
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
        // No anchor string can tell these from the real thing; the layout can.
        // claude draws no composer under a blocking prompt, so a widget with
        // the mode footer beneath it is text about a widget.
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

    /// What the screen says it is asking, once a rule has claimed it.
    fn asked(rules: &Ruleset, screen: &str) -> Question {
        let Claim::Ruled(rule) = claim(rules, screen, Phase::Working) else {
            panic!("no rule claims this screen");
        };
        rule.question(screen)
            .expect("a screen that blocks says what it is blocking on")
    }

    #[test]
    fn rules_where_a_screen_keeps_its_question_is_the_documents_to_say() {
        // Two of claude's blocking screens draw something under their question
        // that is not part of it, so on those the question is the sentence the
        // rule's own anchor is on rather than the one above the choices. Which
        // of the two a screen wants is a fact about that screen: written in
        // Rust it would be one vendor's rule names deciding what is read off
        // another's pane.
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
            Some(Asks::Sentence("trust".to_string()))
        );
        assert_eq!(
            asks("ask_menu"),
            Some(Asks::AboveOptions),
            "a screen that says nothing keeps it above the choices"
        );
    }

    #[test]
    fn rules_a_second_vendors_question_is_read_where_its_own_document_says() {
        // The same machinery over a document that shares no string with
        // claude's: an anchor of its own, choices with no cursor glyph in
        // front of them, and a sentence the vendor wrapped across two rows.
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
    fn rules_the_trust_screen_asks_one_question_at_both_widths() {
        // The vendor wraps this sentence across five rows at 54 columns and
        // breaks it between `you` and `trust`. Both widths are the same
        // question, and the record must not be able to tell which one was read.
        let whole = "Quick safety check: Is this a project you created or one you \
             trust? (Like your own code, a well-known open source project, or work \
             from your team). If not, take a moment to review what's in this folder \
             first.";
        for (width, screen) in [("220", TRUST_SCREEN_220), ("54", TRUST_SCREEN_54)] {
            let asked = asked(claude(), screen);
            assert_eq!(asked.text, whole, "at {width} columns");
            assert_eq!(
                asked.options,
                ["Yes, I trust this folder", "No, exit"],
                "at {width} columns"
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

        // The same menu at 24 columns. The choices survive the wrap; the first
        // row of the question does not survive the floor, which only reads the
        // bottom of the pane, so what is recorded is what could be seen.
        let narrow = asked(claude(), ASK_MENU_24);
        assert_eq!(narrow.options, wide.options);
        assert_eq!(narrow.text, "indented with spaces or tabs?");
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
        // A reader that wants a question already has a record saying the agent
        // is waiting; what it has not got is what for. So this door takes
        // neither a state nor a count of looks, and it answers with what the
        // claimed screens above said.
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
        ] {
            assert_eq!(
                claim(rules, screen, Phase::Idle).phase(),
                Some(Phase::Working),
                "{what} must rule working"
            );
        }

        // The line claude leaves behind when the turn is over is the same
        // glyph without the ellipsis and the parenthesis.
        assert_ne!(
            claim(rules, IDLE_SCREEN, Phase::Idle).phase(),
            Some(Phase::Working),
            "`✻ Worked for 2m 26s` is not a spinner"
        );
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

        // Mid-turn and idle are the same bytes, so a turn on the record is not
        // ended by the screen until it has held still.
        assert_eq!(
            rules.claim(STREAMING_SCREEN, Phase::Working, 1).rule_name(),
            Some("idle_prompt")
        );
        assert!(
            rules
                .claim(STREAMING_SCREEN, Phase::Working, 1)
                .phase()
                .is_none(),
            "one look at a screen that looks idle must not end a turn"
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

        // With nothing outstanding there is no turn to end, so it decides at
        // once — which is what gets a parked agent out of `starting`.
        for recorded in [Phase::Starting, Phase::Unknown] {
            assert_eq!(
                rules.claim(PARKED_SCREEN, recorded, 1).phase(),
                Some(Phase::Idle),
                "{recorded} has nothing to wait for"
            );
        }

        // A rule that is not quiescent never waits.
        assert_eq!(
            rules.claim(WORKING_SCREEN, Phase::Idle, 1).phase(),
            Some(Phase::Working)
        );
    }

    #[test]
    fn rules_only_the_bottom_of_the_capture_is_evidence() {
        // The agent's own output scrolls; the vendor's chrome does not. A
        // spinner line far above the floor is transcript, not state.
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
        // `within` is what keeps two unrelated things on one screen from
        // adding up to a prompt.
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

    /// pi's screens, read the way a reader reaches them: through the entry in
    /// the table, which is what proves the document is in the binary and
    /// parses.
    fn pi() -> &'static Ruleset {
        of("pi")
    }

    #[test]
    fn rules_pi_reads_the_eight_screens_it_draws() {
        assert_eq!(
            named(pi()),
            [
                "first_time_setup",
                "project_trust",
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
        // Every screen here came off a live pi. What each one means is the
        // whole of what amx knows about this vendor: it reports through no
        // hooks, so the pane is not the last witness but the only one.
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
        // pi raises a dialog from a tool call without taking its spinner down,
        // so both rules hold on that screen and only the order decides. A
        // screen that blocks is what the row has to say.
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
        // pi has four status lines and only one of them says `Working...`.
        // Compaction and a retry each take the working indicator down and put
        // their own where it was, and an extension rewrites the message on the
        // one that is up — so the word the spinner rule used to stand on is
        // off the pane on three screens where a turn is running. The frame pi
        // spins in front of the message is on all four.
        for (what, screen) in [
            ("a turn under the vendor's own word", A_PI_WORKING),
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

        // What anchoring on the frame costs, measured rather than argued
        // about: `!cmd` spins the same frame in a box of its own three rows
        // above pi's, and this rule takes it. A command somebody ran in the
        // pane is not a turn the agent is taking, and `working` is still the
        // better of the two answers on offer — the pane is busy, and what it
        // read before was `unknown`.
        assert_eq!(
            claim(pi(), A_PI_RUNNING_A_COMMAND, Phase::Idle).rule_name(),
            Some("spinner"),
            "a shell command running in the pane"
        );

        // And the screens with no turn running under them still have none: the
        // frame is the anchor, and a pane pi is not spinning anything on
        // carries no frame to find.
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
        // pi draws its folder-trust question in the same box a gated tool call
        // is drawn in and ends it in the same `↑↓ navigate` hint row, so the
        // dialog rule holds on this screen too and only the order decides.
        // Both say a person is needed. What a person is being asked for is not
        // the same thing, and the kind is where that is written down.
        let Claim::Ruled(rule) = claim(pi(), A_PI_TRUST, Phase::Starting) else {
            panic!("pi's own rule claims pi's own screen");
        };
        assert_eq!(rule.name, "project_trust");
        assert_eq!(rule.state, Phase::Waiting);
        assert_eq!(rule.kind, Some(crate::store::Kind::Trust));

        // And at 20 columns the box is taller than the rows a rule may see, so
        // the title is out of reach and the screen falls to the rule below.
        // Still waiting, and asked about the way a tool call would be: quiet
        // in the direction of the weaker claim, which is what the fall-through
        // is for and what this screen read before the rule above existed.
        let Claim::Ruled(narrow) = claim(pi(), A_PI_TRUST_20, Phase::Starting) else {
            panic!("something still claims the screen at 20 columns");
        };
        assert_eq!(narrow.name, "dialog");
        assert_eq!(narrow.state, Phase::Waiting);
        assert_eq!(narrow.kind, Some(crate::store::Kind::Question));
    }

    #[test]
    fn rules_the_screens_a_fresh_pi_stops_on_are_screens_that_say_so() {
        // Two ways a pi nobody has set up stops before it can do anything, and
        // both were read as something else. The first-run gate ends in the
        // dialog rule's own hint row, so it was reported as a tool call
        // waiting on an answer; the login dialog is short enough that the box
        // and the stats line under it added up to `prompt`, and a card said
        // idle over a pi that cannot take a turn until somebody types a key
        // into it.
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

        // The gate's second step wraps four rows of prose about usage data
        // until the box is taller than the pane, and at 20 columns the banner
        // has scrolled off with the top border. The screen falls to the dialog
        // rule: still waiting, and asked about the way a tool call would be,
        // which is what it read before this rule existed.
        let Claim::Ruled(narrow) = claim(pi(), A_PI_SETUP_ANALYTICS_20, Phase::Starting) else {
            panic!("something still claims the screen at 20 columns");
        };
        assert_eq!(narrow.name, "dialog");
        assert_eq!(narrow.state, Phase::Waiting);
    }

    #[test]
    fn rules_a_setup_gate_with_pis_own_footer_under_it_is_a_quotation() {
        // The one screen on this vendor where the layout tells a widget from a
        // quotation of one: pi draws no composer and no footer under its
        // first-run gate, so a stats line below that banner says the box is
        // text on somebody else's pane. This is not a capture — it is the
        // measured gate with a measured footer written under it, which is the
        // shape a quotation of it has on a pane that is running a session.
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
        // An extension stops pi for a person three ways, and until now one of
        // them was the only one amx could see. A permission gate is as easily
        // written with `ctx.ui.input` as with `ctx.ui.select` — same caller,
        // same stop, and the row said idle or unknown while somebody waited to
        // be typed at. Each of the three draws a hint row of its own and all
        // three keep the caller's title in the same place, at the top of the
        // box.
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
    fn rules_a_pi_dialog_carries_the_callers_question_and_none_of_its_choices() {
        // The sentence a gated tool call asks is whatever its caller passed,
        // and pi draws it at the top of the box with the choices under it. The
        // choices are the half of this the reading cannot have: pi marks the
        // selected one with a leading arrow and numbers nothing, so there is
        // no first option to walk up from and no telling a choice from the
        // description under one. The arrow is what the question is read above.
        let Claim::Ruled(rule) = claim(pi(), A_PI_DIALOG, Phase::Working) else {
            panic!("pi's own rule claims pi's own screen");
        };
        assert_eq!(rule.kind, Some(crate::store::Kind::Question));

        for (what, screen, sentence) in [
            ("a gated tool call", A_PI_DIALOG, "Run echo hi?"),
            ("the same at 20 columns", A_PI_DIALOG_20, "Run echo hi?"),
            (
                "a confirm, which draws its message under its title",
                A_PI_CONFIRM,
                "Push to origin? This rewrites the remote branch.",
            ),
        ] {
            let asked = pi()
                .asking(screen)
                .unwrap_or_else(|| panic!("{what} says what it is blocking on"));
            assert_eq!(asked.text, sentence, "{what}");
            assert!(
                asked.options.is_empty(),
                "{what}: pi numbers none of these, so none of them is read: {:?}",
                asked.options
            );
        }
    }

    #[test]
    fn rules_pi_and_claude_claim_nothing_on_each_others_panes() {
        // Every anchor in a document is its own vendor's. On somebody else's
        // chrome they are not nearly right, they are absent — which is what
        // keeps a wrapper around one vendor from being read with the other's
        // document and told a confident wrong thing.
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
        // The anchors that find pi's furniture are in the same document as the
        // rules and measured off the same panes. What the walk takes is the
        // box, the working directory, the stats line, and the line pi spins
        // above them — and on this vendor the dialog too, because pi draws it
        // between the box's own borders.
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
        // The rule above reads all four of pi's status lines off the frame,
        // and the walk under it has to cut the same four rows. Anchored on the
        // one message, it cut the row while `Working...` was on it and left it
        // for the other three: a compacting turn's own status line went onto
        // the card and into `amx logs` as work the agent had done.
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

        // What the walk still cannot do, and it is the narrow panes. The frame
        // is on the FIRST row of a message that wraps and the walk reads the
        // LAST row above the box, so at 20 columns a compacting turn keeps its
        // whole status line. Cutting the rest of a wrap means taking rows by
        // position with nothing under them to stop on, and furniture left on
        // the screen is the direction this walk is built to be wrong in.
        assert!(
            cut(A_PI_COMPACTING_20)
                .iter()
                .any(|row| row.contains("Compacting")),
            "a wrapped status line is left whole"
        );
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
}
