//! Tier 3: what is actually running in the pane's foreground.
//!
//! 04 §5: "Process-tree foreground job + prompt detection for unknown programs:
//! `busy/quiet`, **never fake `blocked`**." That last clause is already true by
//! typing — this tier's output is an
//! [`Activity`](amx_core::agent::Activity), which has no `blocked` to spell.
//!
//! What this module answers is the identification question: given a pane's
//! foreground process group, which registry stanza (if any) owns it. D-M2-4 and
//! V07 fix the method:
//!
//! - the foreground **group leader** first, then job members scored by
//!   unwrapped evidence;
//! - unwrapping `node`/`bun`/`python`/`sh -c` and path tokens, because an agent
//!   shipped as a Node script has `node` as its `comm`;
//! - **eval-flag arguments never identify**: `python -c "codex"` is not codex.
//!   herdr carries that negative test and V07 re-derives it.
//!
//! Probes are triggered, rate-limited work — a spawn, damage-after-quiet, an
//! `agent start` readiness check — and never a free-running scan loop.
//! [`ProbeGate`] is that rule as data, so the actor that schedules probes
//! cannot accidentally schedule a loop.
//!
//! Identity carries weight two other tiers do not, and V01 §4 is why: a pane
//! running an idle Codex has **no hook-borne identity at all** until the user
//! types their first prompt, because Codex fires no `SessionStart` before then.
//! For that pane, this module is the only thing that knows what it is looking
//! at — so `agent status` must not report it as unidentified and `agent start
//! codex` readiness cannot wait on a hook.
//!
//! # Pure over the seam
//!
//! Everything here takes a [`ProcessTree`] by reference and a
//! [`ProcessId`] by value; nothing opens a pty, reads a clock or owns a task.
//! The pid comes from the pane's own session
//! ([`PtySession::foreground_group`](amx_core::platform::PtySession::foreground_group)),
//! which only the pane host holds, so the caller supplies it. That is also what
//! lets the suite drive the whole walk over a fabricated tree and still test
//! the real reader against a real pty.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use amx_core::agent::{Activity, AgentKind, AgentState};
use amx_core::platform::{ProcessId, ProcessTree};

use super::registry::{AgentStanza, Registry};

/// Programs whose own name says nothing about what they are running.
///
/// An interpreter or a launcher: the agent is in its arguments, not in its
/// `comm`. Kept deliberately short — every entry is a program amx will look
/// *past*, and looking past something that was the answer is how a pane gets
/// identified as whatever it happened to invoke.
const WRAPPERS: &[&str] = &[
    "bash", "bun", "bunx", "dash", "deno", "env", "ksh", "node", "nodejs", "npx", "pnpm", "sh",
    "tsx", "uvx", "yarn", "zsh",
];

/// Flags whose operand is source text rather than a program.
///
/// The negative test's whole subject: `python -c "codex"` names no program at
/// all, it names an expression that happens to read like one. herdr carries the
/// same test and V07 re-derives it, because an agent's *name appearing in an
/// argument* is exactly the shape a wrapper's arguments have.
const EVAL_FLAGS: &[&str] = &["-c", "-e", "--eval", "--command"];

/// How many wrappers deep the unwrapping will go.
///
/// `env node /opt/agent/bin/agent` is two. Four is slack for a launcher chain
/// nobody has written yet and a hard stop for an argv that unwraps in a circle.
const UNWRAP_DEPTH: usize = 4;

/// How far below the foreground group leader the walk looks for the agent.
///
/// A leader that is a shell wrapper with the agent as its child is depth 1; an
/// agent that re-execs itself behind a supervisor is depth 2. Beyond that the
/// processes found are the agent's own workers, not the agent.
const MAX_DEPTH: u32 = 3;

/// The most processes one probe will read.
///
/// A bound, not a budget: an agent mid-turn can have dozens of children, and a
/// probe that walked all of them would turn a damage batch into a `/proc` scan.
/// The leader and its immediate job members are where the answer is.
const MAX_CANDIDATES: usize = 32;

/// The shortest gap between two damage-triggered probes of one pane.
///
/// Derived from the two numbers V01 §6 measured rather than chosen: a probe
/// must not outpace the screen evaluation it corroborates (`CONFIRMATION_
/// SPACING`, 100 ms) and must resolve identity well inside the shortest startup
/// grace the registry ships (3 s, for Claude Code — Codex takes 5 s). 250 ms is
/// one probe per two-and-a-half screen evaluations and at most twelve across
/// the shortest grace, which is ample for a fact that changes only when a
/// program starts or exits.
pub const PROBE_SPACING: Duration = Duration::from_millis(250);

/// Why a probe is being asked for.
///
/// The three triggers D-M2-4 names, and the reason this is an enum rather than
/// a bool: two of them are events that *just changed what is running*, so
/// delaying them would delay the answer they exist to produce, while the third
/// is a stream and needs a floor under it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ProbeTrigger {
    /// A pane's process just started. Always admitted: this is the probe that
    /// gives a pane its first identity.
    Spawn,
    /// The pane painted something after a quiet stretch. Rate-limited — this is
    /// the trigger that arrives in bursts.
    Damage,
    /// `agent start` is waiting to call a pane ready. Always admitted: the
    /// caller is blocked on the answer and has its own timeout.
    Readiness,
}

/// The rate limit on a pane's identity probes.
///
/// "Probes are triggered, rate-limited work … never a free-running scan loop."
/// herdr's equivalent is a permanent 300–500 ms scan over every pane; amx has
/// no timer here at all, only a floor that a burst of damage cannot cross.
///
/// No clock inside: `now` arrives as an argument, which is what lets the suite
/// step time rather than wait for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ProbeGate {
    last: Option<Instant>,
}

impl ProbeGate {
    /// A gate that has admitted nothing yet.
    #[must_use]
    pub const fn new() -> Self {
        Self { last: None }
    }

    /// Whether a probe for `trigger` may run at `now`, recording it if so.
    ///
    /// A refused damage probe is *not* remembered: the floor is measured from
    /// the last probe that ran, so a burst of damage does not push the next
    /// admitted probe further and further away.
    pub fn admit(&mut self, trigger: ProbeTrigger, now: Instant) -> bool {
        let due = match trigger {
            ProbeTrigger::Spawn | ProbeTrigger::Readiness => true,
            ProbeTrigger::Damage => self
                .last
                .is_none_or(|last| now.saturating_duration_since(last) >= PROBE_SPACING),
        };
        if due {
            self.last = Some(now);
        }
        due
    }
}

/// How an identification was reached.
///
/// Carried so `agent explain` can say *which* process answered — a detection
/// you cannot interrogate is a detection you can only distrust.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Evidence {
    /// Nothing in the foreground job named an agent this registry knows.
    Unidentified,
    /// The foreground process group's leader is the agent.
    GroupLeader,
    /// A member of the foreground job, this many links below the leader, is.
    JobMember {
        /// Links below the group leader.
        depth: u32,
    },
}

/// What tier 3 concluded about a pane's foreground job.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Identification {
    kind: Option<AgentKind>,
    process: Option<ProcessId>,
    program: Option<String>,
    argv: Vec<String>,
    evidence: Evidence,
}

impl Identification {
    /// Nothing was readable at all — the job exited between the trigger and the
    /// probe, or this platform has no process-tree reader.
    #[must_use]
    pub const fn unreadable() -> Self {
        Self {
            kind: None,
            process: None,
            program: None,
            argv: Vec::new(),
            evidence: Evidence::Unidentified,
        }
    }

    /// The agent this pane is running, if the walk named one.
    #[must_use]
    pub const fn kind(&self) -> Option<&AgentKind> {
        self.kind.as_ref()
    }

    /// The process the answer came from.
    #[must_use]
    pub const fn process(&self) -> Option<ProcessId> {
        self.process
    }

    /// The program name the walk settled on, after unwrapping.
    ///
    /// Present for an unidentified job too, which is the point: a pane running
    /// `vim` is not an agent, and saying "vim" is more useful than saying
    /// nothing.
    #[must_use]
    pub fn program(&self) -> Option<&str> {
        self.program.as_deref()
    }

    /// The argv the answer was read out of, as data.
    #[must_use]
    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    /// How the walk got here.
    #[must_use]
    pub const fn evidence(&self) -> Evidence {
        self.evidence
    }

    /// The state tier 3 asserts for this pane, given what its output is doing.
    ///
    /// `None` once an agent has been identified: from then on the pane's state
    /// is the fusion machine's, taken from hook edges and the screen, and a
    /// probe that also asserted one would be a fourth authority nobody asked
    /// for.
    ///
    /// For everything else the answer is an [`Activity`], and 04 §5's "never
    /// fake `blocked`" holds by construction — `Activity` has two variants and
    /// neither of them is a block.
    #[must_use]
    pub const fn assert_state(&self, activity: Activity) -> Option<AgentState> {
        match self.kind {
            Some(_) => None,
            None => Some(match activity {
                Activity::Busy => AgentState::Busy,
                Activity::Quiet => AgentState::Quiet,
            }),
        }
    }
}

/// Identify the agent in a pane's foreground job, starting at its group leader.
///
/// The group leader answers first because it is what the terminal itself says
/// is in the foreground; only when it names nothing the registry knows does the
/// walk descend into the job, breadth first, so the shallowest evidence wins.
/// The bounds ([`MAX_DEPTH`], [`MAX_CANDIDATES`]) are what keep a probe a probe
/// and not a scan.
///
/// A job that names no known agent still comes back with the leader's program
/// and argv, because "this pane is running `vim`" is an answer tier 3 owes the
/// status line.
#[must_use]
pub fn identify<T>(tree: &T, registry: &Registry, leader: ProcessId) -> Identification
where
    T: ProcessTree + ?Sized,
{
    let mut seen = HashSet::new();
    let mut queue = vec![(leader, 0_u32)];
    let mut read = 0_usize;
    let mut fallback: Option<Identification> = None;

    while let Some((process, depth)) = queue.pop() {
        if !seen.insert(process) || read >= MAX_CANDIDATES {
            continue;
        }
        read += 1;

        let (argv, program) = read_program(tree, process);
        if let Some(program) = program.as_deref()
            && let Some(stanza) = stanza_for(registry, program)
        {
            return Identification {
                kind: Some(stanza.id.clone()),
                process: Some(process),
                program: Some(program.to_owned()),
                argv,
                evidence: if depth == 0 {
                    Evidence::GroupLeader
                } else {
                    Evidence::JobMember { depth }
                },
            };
        }
        // The leader's own reading is what an unidentified pane reports; a
        // descendant's would name a helper process the user never started.
        // Nothing readable at all is not a reading: a job that exited between
        // the trigger and the probe reports `unreadable`, not a pid.
        if depth == 0 && (program.is_some() || !argv.is_empty()) {
            fallback = Some(Identification {
                kind: None,
                process: Some(process),
                program,
                argv,
                evidence: Evidence::Unidentified,
            });
        }

        if depth < MAX_DEPTH {
            let children = tree.children(process).unwrap_or_default();
            // Reversed, because the queue is popped from the back: the walk
            // stays breadth-first *and* visits each generation in pid order,
            // which is what makes a two-candidate job resolve the same way
            // twice.
            queue.splice(0..0, children.into_iter().rev().map(|kid| (kid, depth + 1)));
        }
    }

    fallback.unwrap_or_else(Identification::unreadable)
}

/// One process's argv and the program name it resolves to.
///
/// `exe` is the fallback and not the primary: `argv[0]` is whatever the parent
/// wrote there, but it is also the only place a wrapper's real target appears.
/// When argv is unreadable or empty — a zombie, a process that exited under the
/// walk — the executable the kernel mapped is all there is, and it cannot be
/// unwrapped because it carries no arguments.
fn read_program<T>(tree: &T, process: ProcessId) -> (Vec<String>, Option<String>)
where
    T: ProcessTree + ?Sized,
{
    let argv: Vec<String> = tree
        .argv(process)
        .unwrap_or_default()
        .iter()
        .map(|word| word.to_string_lossy().into_owned())
        .collect();
    if !argv.is_empty() {
        return (argv.clone(), program_named(&argv).map(str::to_owned));
    }
    let program = tree
        .exe(process)
        .ok()
        .and_then(|exe| Some(exe.file_name()?.to_string_lossy().into_owned()));
    (Vec::new(), program)
}

/// The stanza that claims `program` as one of its executables.
fn stanza_for<'a>(registry: &'a Registry, program: &str) -> Option<&'a AgentStanza> {
    registry
        .stanzas()
        .iter()
        .find(|stanza| stanza.executables.iter().any(|exe| exe == program))
}

/// The program an argv actually names, after unwrapping.
///
/// The rules, in order:
///
/// 1. `argv[0]`'s **basename** — the path-token unwrapping, so
///    `/usr/local/bin/agent` and `agent` are one answer.
/// 2. If that basename is a [wrapper](WRAPPERS), look past it: skip its own
///    flags and any `NAME=value` assignments, honour `--`, and take the first
///    remaining word as the new `argv[0]`. Repeat, up to [`UNWRAP_DEPTH`].
/// 3. An [eval flag](EVAL_FLAGS) ends the walk. Its operand is source text, and
///    text is only allowed to name a program when the *whole* operand is a bare
///    path — which is what `sh -c /usr/local/bin/agent` is and what
///    `python -c "codex"` is not.
///
/// `None` means the argv names no program this can be sure of, which is a
/// perfectly ordinary answer and always safer than a guess: an unidentified
/// pane gets `busy`/`quiet`, a misidentified one gets a whole agent's worth of
/// wrong status.
#[must_use]
pub fn program_named(argv: &[String]) -> Option<&str> {
    let mut argv = argv;
    for _ in 0..UNWRAP_DEPTH {
        let name = basename(argv.first()?);
        if !is_wrapper(name) {
            return Some(name);
        }
        argv = wrapped_argv(&argv[1..])?;
    }
    None
}

/// What a wrapper runs, given everything after the wrapper's own name.
///
/// Returns the remaining argv positioned at the wrapped program, or `None` when
/// the wrapper names no program — it ran out of arguments, or the only thing it
/// was given was source text.
fn wrapped_argv(mut rest: &[String]) -> Option<&[String]> {
    loop {
        let arg = rest.first()?;
        if arg == "--" {
            // Everything after `--` is the wrapped command, verbatim.
            return rest.get(1..).filter(|tail| !tail.is_empty());
        }
        if EVAL_FLAGS.contains(&arg.as_str()) {
            // The operand is code. It names a program only if it is nothing but
            // a path — no spaces, no shell syntax, a `/` in it.
            let operand = rest.get(1)?;
            return path_operand(operand).then_some(&rest[1..2]);
        }
        // A flag, or an `env`-style assignment: neither is the program.
        if arg.starts_with('-') || is_assignment(arg) {
            rest = rest.get(1..)?;
            continue;
        }
        return Some(rest);
    }
}

/// Whether an eval flag's operand is a bare path and nothing else.
///
/// The one shape that survives rule 3: `sh -c /usr/local/bin/agent`. Anything
/// with whitespace or shell syntax in it is a script, and a script that
/// *mentions* an agent is not an agent — that is the whole of herdr's negative
/// test, re-derived.
fn path_operand(operand: &str) -> bool {
    operand.contains('/')
        && !operand
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || SHELL_SYNTAX.contains(c))
}

/// Characters that make an operand a script rather than a path.
const SHELL_SYNTAX: &str = "|&;<>()$`\\\"'*?[]{}!#";

/// Whether `word` is a `NAME=value` assignment, as `env` takes before the
/// program it runs.
fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// Whether a basename is an interpreter or launcher to look past.
///
/// `python`, `python3`, `python3.13` are all one wrapper: the version suffix is
/// packaging, not a different program.
fn is_wrapper(name: &str) -> bool {
    if WRAPPERS.contains(&name) {
        return true;
    }
    name.strip_prefix("python")
        .is_some_and(|version| version.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

/// The last component of a path token, or the token itself when it has none.
fn basename(token: &str) -> &str {
    Path::new(token)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or(token)
}
