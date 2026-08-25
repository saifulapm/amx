//! Reading an agent's screen when its hooks have gone quiet.
//!
//! Hooks are precise and win while they flow. When they stop — the vendor was
//! interrupted with Escape, or nothing has happened for a while — the pane
//! itself is the only witness left, and this is what amx knows how to see in
//! it: the rules in `assets/screen-rules.toml`, matched against the bottom of
//! the capture.
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
/// the order they are asked.
#[derive(Debug, Deserialize)]
pub struct Ruleset {
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

/// The screens amx reads when nothing has said which vendor's pane it is
/// looking at, which is every reader there is today: a record says which pane
/// an agent is in, not what is running in it. [`of`] is the door for anything
/// that does know.
pub fn bundled() -> &'static Ruleset {
    of(registry::entries().first().map_or("", |vendor| vendor.name))
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
        let (from, to) = match self.asks() {
            Asks::Sentence(anchor) => screen.sentence_at(screen.row_above(choices, anchor)?),
            Asks::AboveOptions => screen.sentence_above(choices?)?,
        };

        let text = screen.joined(from, to);
        (!text.is_empty()).then(|| Question {
            options: screen.options_below(to),
            text,
        })
    }

    /// Where this screen keeps the question it is asking.
    ///
    /// By name, because the blocking screens do not all keep it in the same
    /// place and no one reading finds it on every one of them. Both of these
    /// were chosen against the captures in this file's tests, at each of the
    /// widths those were measured at.
    fn asks(&self) -> Asks {
        match self.name.as_str() {
            // The question is the row the rule's own anchor is on. What is
            // above it is what the request is about — the tool, its arguments,
            // the rule that stopped it — and that is not what is being asked.
            "permission_prompt" => Asks::Sentence("do you want to"),
            // The same, and for the same reason: under this screen's question
            // sit a sentence about what claude will be able to do and a link
            // to the security guide, and neither is the question.
            "folder_trust" => Asks::Sentence("trust"),
            // Every other blocking screen puts the question straight above the
            // choices with a blank row between, which is also where a screen
            // amx has not met yet is likeliest to put it.
            _ => Asks::AboveOptions,
        }
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
enum Asks {
    /// The sentence the lowest row carrying this string belongs to, wrap and
    /// all. For the screens that draw something under their question that is
    /// not part of it.
    Sentence(&'static str),
    /// The sentence that ends just above the first choice.
    AboveOptions,
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
    use crate::vendor::claude;
    use crate::vendor::second::SECOND;

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

    /// Nothing this ruleset knows: an ordinary shell, which is what a pane
    /// shows when the vendor has exited.
    const A_SHELL: &str = "\
$ ls
Cargo.toml  README.md  src  tests
$
";

    fn claim<'a>(rules: &'a Ruleset, screen: &str, recorded: Phase) -> Claim<'a> {
        rules.claim(screen, recorded, SETTLED_LOOKS)
    }

    /// The two documents these tests weigh against each other: the one amx
    /// ships and the second vendor's, which shares no string with it. A law
    /// that holds for both is a law about the machinery.
    fn documents() -> Vec<(&'static str, Ruleset)> {
        [claude::VENDOR, SECOND]
            .iter()
            .map(|vendor| {
                let screens = vendor.screens.expect("both of these have screens");
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
            Some(named(bundled()))
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
            assert_eq!(named(of(agent)), named(bundled()), "{agent:?}");
        }
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
        let rules = bundled();
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
        // could never match, and would fail silently.
        for rule in bundled().rules() {
            for needle in rule.all.iter().chain(&rule.any).chain(&rule.not_below) {
                assert_eq!(
                    needle,
                    &needle.to_lowercase(),
                    "{}: {needle:?} must be written folded",
                    rule.name
                );
            }
        }
    }

    #[test]
    fn rules_the_four_blocking_prompts_rule_waiting() {
        let rules = bundled();
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
        let rules = bundled();
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
    fn rules_a_permission_box_carries_the_question_and_the_two_answers() {
        let asked = asked(bundled(), PERMISSION_BOX);
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
            let asked = asked(bundled(), screen);
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
        let wide = asked(bundled(), ASK_MENU_80);
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
        let narrow = asked(bundled(), ASK_MENU_24);
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
            assert_eq!(asked(bundled(), screen).text, whole, "at {width} columns");
        }

        assert_eq!(
            asked(bundled(), PLAN_APPROVAL_220).options,
            [
                "Yes, and use auto mode",
                "Yes, manually approve edits",
                "Tell Claude what to change"
            ]
        );
        assert_eq!(
            asked(bundled(), PLAN_APPROVAL_24).options,
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
        let rules = bundled();
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
        let rules = bundled();
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
        let rules = bundled();
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
        let rules = bundled();
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
        let rules = bundled();
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
        let rules = bundled();

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
        let rules = bundled();
        let old_news = format!(
            "✢ Infusing… (1m 54s · ↓ 6.9k)\n{}{}",
            "\n".repeat(40),
            A_SHELL
        );
        assert_eq!(claim(rules, &old_news, Phase::Idle), Claim::Unclaimed);
    }

    #[test]
    fn rules_matching_folds_case() {
        let rules = bundled();
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
