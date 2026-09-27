//! A second vendor, so that the first one cannot quietly become the shape of
//! everything.
//!
//! Test builds only, and never in the table: it is nobody's agent, nothing can
//! spawn it, and it answers to no command anybody would type. Its whole job is
//! to be unlike claude in every way the descriptor allows, so that a test
//! which passes for both is a test of the machinery rather than of claude.
//!
//! It declares a dial claude does not spell the same way, leaves out one
//! claude has, and its values are words claude has never heard of. When a
//! later field arrives on the descriptor, the way to keep it honest is to
//! answer it differently here.

use super::{
    Capability, DEFAULT, DialSpec, Hooks, Models, Moment, Resume, SessionSpec, Vendor, Wire, Wiring,
};

/// The fixture. Read the module docs before changing a value: each of these
/// disagrees with claude on purpose.
pub const SECOND: Vendor = Vendor {
    name: "second",
    // A short flag, a closed set, and values that are nobody else's words.
    model: Some(DialSpec {
        cycle: &[DEFAULT, "small", "large"],
        open: false,
        flag: "-m",
    }),
    // Its two words are the whole of what it offers, so they are found in the
    // cycle and nothing is ever run to ask: the shape a search over the table
    // has to answer out of the entry alone.
    models: Models::Cycle,
    // No permission dial at all, which is the difference between a dial
    // nobody has turned and a dial that does not exist.
    permission: None,
    // Open where claude's is closed, and under a flag of its own.
    effort: Some(DialSpec {
        cycle: &[DEFAULT, "quick", "thorough"],
        open: true,
        flag: "--care",
    }),
    // Unlike claude, it declares a start flag: proof that a vendor is free to
    // ask amx to open a session under an id amx chose. It resumes with a
    // subcommand rather than a flag, the way codex does, and it names no fork
    // shape at all, because it does not claim `Capability::Fork`.
    session: Some(SessionSpec {
        start: Some("--open"),
        resume: Resume::Subcommand("again"),
        conflicts: &["--open"],
        fork: None,
    }),
    // A session variable spelled nothing like the other one, so that a test
    // reading it is reading the descriptor.
    session_env: Some("SECOND_SESSION"),
    not_inherited: &["SECOND_SESSION", "SECOND_PARENT"],
    // Two of the six, so that half the questions a verb asks come back the
    // other way. It carries a session on and can be taken over, and it has no
    // hooks, no transcript, no way to branch and no trust screen: the shape of
    // a vendor amx has to refuse things for.
    capabilities: &[Capability::Resume, Capability::Adopt],
    // Nothing to wire and nothing to read: this vendor tells amx nothing about
    // what it is doing, which is the shape install has to leave alone and the
    // reason the capability above is asked before either is touched.
    hooks: None,
    // Screens of its own, drawn out of nothing claude draws: see [`SCREENS`].
    screens: Some(SCREENS),
    // And no conversation on disk to read, which is what a vendor that keeps
    // no transcript has to answer.
    transcript: None,
    // Nothing anybody can name on a line either: nobody has measured where
    // this vendor keeps its skills or what it answers itself, which is the
    // shape a reader of those places has to leave alone.
    catalog: None,
    // Its task is its last word, whatever that word opens with.
    ends_options: None,
};

/// Hooks in the second vendor's words, for a test that reads a payload.
///
/// Off the entry on purpose: [`SECOND`] is the vendor that reports nothing,
/// and a verb has to refuse things for it. These are what a vendor that did
/// report would say if every word of it were its own, so that a reading which
/// passes under them is a reading of the record's vendor and not of claude.
pub const HOOKS: Hooks = Hooks {
    wire: Wire::File {
        path: ".second/report.sh",
        body: "# installed by amx\namx _hook\n",
    },
    opt_in: &[],
    events: &[
        Wiring {
            moment: Moment::Started,
            event: "opened",
            matched: false,
        },
        Wiring {
            moment: Moment::Prompted,
            event: "told",
            matched: false,
        },
        Wiring {
            moment: Moment::Calling,
            event: "using",
            matched: false,
        },
        Wiring {
            moment: Moment::Asked,
            event: "may_i",
            matched: false,
        },
        Wiring {
            moment: Moment::Refused,
            event: "refused",
            matched: false,
        },
        Wiring {
            moment: Moment::Notified,
            event: "note",
            matched: false,
        },
        Wiring {
            moment: Moment::Ended,
            event: "finished",
            matched: false,
        },
    ],
    matcher: "",
    question_tool: "choose",
    idle_notice: "resting",
    permission_notice: "gatekeeping",
    permission_sentence: "second may not run {tool} yet",
    injected: &["[second]"],
    fresh_start: Some("new"),
    question_kinds: &["pick"],
};

/// The second vendor's screens.
///
/// A document rather than a file, because nothing ships it: it exists so that
/// a reader passing over both documents is reading the machinery and not
/// claude. Every string in it disagrees with the first document — a different
/// footer, a different rule glyph, a different spinner, choices with no cursor
/// glyph in front of them, and a question the vendor writes above its options
/// instead of on the anchor row.
///
/// The pane it describes:
///
/// ```text
///   It did the thing.
///
///  = compose =
///  >
///  ===========
///   model: small
///   mode: careful
/// ```
pub const SCREENS: &str = r#"
placeholders = ["it wants something"]

[furniture]
mode = ["mode:"]
spinner = ["thinking for "]
rule = "="
statusline = 1
bottom = 1

[[rule]]
name = "choice"
state = "waiting"
kind = "question"
all = ["pick one"]
any = ["1. "]
within = 4
not_below = ["mode:"]
asks = { sentence = "pick one" }

[[rule]]
name = "busy"
state = "working"
all = ["thinking for "]

[[rule]]
name = "prompt"
state = "idle"
quiescent = true
any = ["mode:"]
"#;
