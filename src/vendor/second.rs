//! Fixture vendors for tests, deliberately unlike claude in every field.
//!
//! Test builds only and never in the table. A test that passes for claude and
//! for these is testing the table machinery rather than claude's values. When
//! a field is added to [`Vendor`], give it a non-claude value here.

use super::{
    Capability, DEFAULT, DialSpec, ForkSpec, Hooks, Models, Moment, Resume, SessionSpec, Vendor,
    Wire, Wiring,
};

/// The base fixture: no hooks, no transcript, no fork and no trust screen.
pub const SECOND: Vendor = Vendor {
    name: "second",
    model: Some(DialSpec::closed("-m", &[DEFAULT, "small", "large"])),
    models: Models::Cycle,
    permission: None,
    effort: Some(DialSpec::open("--care", &[DEFAULT, "quick", "thorough"])),
    // A start flag, which claude lacks, and a resume subcommand.
    session: Some(SessionSpec {
        start: Some("--open"),
        resume: Resume::Subcommand("again"),
        conflicts: &["--open"],
        fork: None,
    }),
    session_env: Some("SECOND_SESSION"),
    not_inherited: &["SECOND_SESSION", "SECOND_PARENT"],
    capabilities: &[Capability::Resume, Capability::Adopt],
    hooks: None,
    screens: Some(SCREENS),
    transcript: None,
    catalog: None,
    ends_options: None,
    attaches_at: false,
    restores_queued_on_cancel: false,
    prompt_flag: None,
    popups: &[],
    cancel_presses: 1,
    interrupt_signal: None,
    launch: &[],
};

/// [`SECOND`] with a fork subcommand, hooks, a JSON model listing, a keyed
/// effort dial, an options terminator and a launch flag.
///
/// With no start flag, it learns a fork's session id through [`HOOKS`].
pub const BRANCHING: Vendor = Vendor {
    models: Models::Json(&["list", "models"]),
    effort: Some(DialSpec {
        key: Some("care"),
        ..DialSpec::open("-c", &[DEFAULT, "quick", "thorough"])
    }),
    launch: &["--alone"],
    session: Some(SessionSpec {
        start: None,
        resume: Resume::Subcommand("again"),
        conflicts: &["--open"],
        fork: Some(ForkSpec::Subcommand("fork")),
    }),
    capabilities: &[
        Capability::Hooks,
        Capability::Resume,
        Capability::Fork,
        Capability::Adopt,
    ],
    hooks: Some(HOOKS),
    ends_options: Some("--"),
    ..SECOND
};

/// [`SECOND`] with the model dial carried in the environment, a bare
/// permission flag, a prompt flag, a `#` popup, three-press cancel and a
/// `SIGUSR1` interrupt.
pub const ELSEWHERE: Vendor = Vendor {
    model: Some(DialSpec {
        key: Some("size"),
        env: Some("SECOND_CONFIG"),
        ..DialSpec::open("", &[DEFAULT, "small", "large"])
    }),
    permission: Some(DialSpec {
        bare: true,
        ..DialSpec::closed("--loose", &[DEFAULT, "loose"])
    }),
    prompt_flag: Some("--say"),
    popups: &['#'],
    cancel_presses: 3,
    interrupt_signal: Some(nix::sys::signal::Signal::SIGUSR1),
    ..SECOND
};

/// Hooks with every name unlike claude's, for tests that read payloads.
///
/// Not on [`SECOND`], which must stay the vendor without hooks; [`BRANCHING`]
/// carries them.
pub const HOOKS: Hooks = Hooks {
    wire: Wire::File {
        path: ".second/report.sh",
        body: "# installed by amx\namx _hook\n",
    },
    opt_in: &[],
    events: &[
        Wiring::new(Moment::Started, "opened"),
        Wiring::new(Moment::Prompted, "told"),
        Wiring::new(Moment::Calling, "using"),
        Wiring::new(Moment::Asked, "may_i"),
        Wiring::new(Moment::Refused, "refused"),
        Wiring::new(Moment::Notified, "note"),
        Wiring::new(Moment::Ended, "finished"),
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

/// Screen rules for the fixture, differing from claude's in footer, rule
/// glyph, spinner, choice markers and where the question sits.
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
