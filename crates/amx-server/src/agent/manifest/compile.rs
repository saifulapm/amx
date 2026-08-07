//! Compiled gates: what a parsed rule becomes, and how it is evaluated.
//!
//! Every regex compiles **once, at load**, and is stored beside its rule.
//! D-M2-3 is explicit about why: evaluation is driven by the `PaneDamage`
//! stream, so a pattern rebuilt per evaluation would be rebuilt on every frame
//! of a busy pane. Nothing in [`CompiledGate::matches`] allocates either — the
//! case-insensitive `contains` is a windowed byte compare rather than a
//! lowercased copy of the region, which is the one place herdr's engine pays
//! per evaluation.
//!
//! Evidence is the cold path and is allowed to allocate: it is built by
//! `agent explain`, at human speed, and by nothing else.

use regex::{Regex, RegexBuilder};

use super::rule::{GateFile, MAX_MATCHER_CHARS, RuleError};

/// How large a single compiled pattern may get.
///
/// A regex's compiled size is not its source length — `(a|b){20}` is short and
/// enormous — so the cap that matters is here rather than in
/// [`MAX_MATCHER_CHARS`]. One megabyte is far past anything a screen rule
/// needs and far below anything that would hurt a session.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// One node of a gate tree, ready to evaluate.
#[derive(Debug)]
pub struct CompiledGate {
    /// Needles, lowercased at compile so the hot path never lowercases the
    /// region.
    contains: Vec<String>,
    regex: Vec<Regex>,
    line_regex: Vec<Regex>,
    all: Vec<CompiledGate>,
    any: Vec<CompiledGate>,
    not: Vec<CompiledGate>,
}

impl CompiledGate {
    /// Compile `gate`, reporting failures against `rule`'s name.
    ///
    /// # Errors
    ///
    /// [`RuleError::Regex`] for a pattern that does not compile or is too
    /// large, and [`RuleError::MatcherTooLong`] for one past
    /// [`MAX_MATCHER_CHARS`].
    pub fn build(gate: &GateFile, rule: &str) -> Result<Self, RuleError> {
        for matcher in gate.matchers() {
            if matcher.chars().count() > MAX_MATCHER_CHARS {
                return Err(RuleError::MatcherTooLong {
                    rule: rule.to_owned(),
                });
            }
        }
        Ok(Self {
            contains: gate
                .contains
                .iter()
                .map(|needle| needle.to_lowercase())
                .collect(),
            regex: compile_all(&gate.regex, rule)?,
            line_regex: compile_all(&gate.line_regex, rule)?,
            all: build_all(&gate.all, rule)?,
            any: build_all(&gate.any, rule)?,
            not: build_all(&gate.not, rule)?,
        })
    }

    /// Whether this gate holds over `text`.
    ///
    /// The AND of every conjunct, in the cheapest-first order: substrings
    /// before regexes, this level before the nesting below it.
    #[must_use]
    pub fn matches(&self, text: &str) -> bool {
        self.contains
            .iter()
            .all(|needle| contains_ignore_case(text, needle))
            && self.regex.iter().all(|regex| regex.is_match(text))
            && self
                .line_regex
                .iter()
                .all(|regex| text.lines().any(|line| regex.is_match(line)))
            && self.all.iter().all(|gate| gate.matches(text))
            && (self.any.is_empty() || self.any.iter().any(|gate| gate.matches(text)))
            && !self.not.iter().any(|gate| gate.matches(text))
    }

    /// Why this gate held over `text`, or why it did not.
    ///
    /// One line, naming the decisive matcher: the first conjunct that failed,
    /// or the first positive matcher that held. `agent explain` reports it per
    /// rule — including for the rules that did *not* match, which is the half
    /// that tells a user why the screen they are looking at was read the way it
    /// was (04 §5: a detection you cannot interrogate is one you cannot fix).
    #[must_use]
    pub fn evidence(&self, text: &str) -> String {
        match self.first_failure(text) {
            Some(reason) => reason,
            None => self
                .first_positive(text)
                .unwrap_or_else(|| "matched: nothing to check".to_owned()),
        }
    }

    /// The first conjunct that does not hold, described.
    fn first_failure(&self, text: &str) -> Option<String> {
        for needle in &self.contains {
            if !contains_ignore_case(text, needle) {
                return Some(format!("no {needle:?} in the region"));
            }
        }
        for regex in &self.regex {
            if !regex.is_match(text) {
                return Some(format!("region does not match /{}/", regex.as_str()));
            }
        }
        for regex in &self.line_regex {
            if !text.lines().any(|line| regex.is_match(line)) {
                return Some(format!("no line matches /{}/", regex.as_str()));
            }
        }
        for gate in &self.all {
            if let Some(reason) = gate.first_failure(text) {
                return Some(reason);
            }
        }
        if !self.any.is_empty() && !self.any.iter().any(|gate| gate.matches(text)) {
            return Some(format!("none of {} alternatives hold", self.any.len()));
        }
        for gate in &self.not {
            if gate.matches(text) {
                return Some(match gate.first_positive(text) {
                    Some(reason) => format!("excluded: {reason}"),
                    None => "excluded by a `not` gate".to_owned(),
                });
            }
        }
        None
    }

    /// The first positive matcher that holds, described.
    fn first_positive(&self, text: &str) -> Option<String> {
        if let Some(needle) = self.contains.first() {
            return Some(format!("{needle:?} in the region"));
        }
        if let Some(regex) = self.regex.first() {
            return Some(format!("region matches /{}/", regex.as_str()));
        }
        if let Some(regex) = self.line_regex.first() {
            let line = text.lines().find(|line| regex.is_match(line));
            return Some(match line {
                Some(line) => format!("{:?} matches /{}/", line.trim(), regex.as_str()),
                None => format!("a line matches /{}/", regex.as_str()),
            });
        }
        self.all
            .iter()
            .find_map(|gate| gate.first_positive(text))
            .or_else(|| {
                self.any
                    .iter()
                    .find(|gate| gate.matches(text))
                    .and_then(|gate| gate.first_positive(text))
            })
    }
}

/// Compile every gate in `gates`.
fn build_all(gates: &[GateFile], rule: &str) -> Result<Vec<CompiledGate>, RuleError> {
    gates
        .iter()
        .map(|gate| CompiledGate::build(gate, rule))
        .collect()
}

/// Compile every pattern in `patterns`.
fn compile_all(patterns: &[String], rule: &str) -> Result<Vec<Regex>, RuleError> {
    patterns
        .iter()
        .map(|pattern| {
            RegexBuilder::new(pattern)
                .size_limit(REGEX_SIZE_LIMIT)
                .build()
                .map_err(|err| RuleError::Regex {
                    rule: rule.to_owned(),
                    pattern: pattern.clone(),
                    message: err.to_string(),
                })
        })
        .collect()
}

/// Whether `haystack` holds `needle`, ignoring ASCII case.
///
/// `needle` is already lowercase — [`CompiledGate::build`] does that once, at
/// load. Only ASCII case is folded, and deliberately so: folding Unicode case
/// changes byte lengths, which would mean either allocating a folded copy of
/// the region per evaluation (herdr's cost) or a matcher that cannot say what
/// it matched. A rule that needs Unicode case-insensitivity writes `(?i)` in a
/// `regex`, where the regex engine does it properly.
fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    let needle = needle.as_bytes();
    let haystack_bytes = haystack.as_bytes();
    // Char boundaries only: a window starting inside a multi-byte character
    // could otherwise "match" a needle that is not on screen at all.
    haystack.char_indices().any(|(start, _)| {
        haystack_bytes
            .get(start..start + needle.len())
            .is_some_and(|window| window.eq_ignore_ascii_case(needle))
    })
}
