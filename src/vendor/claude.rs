//! claude, the vendor amx was written against.
//!
//! Everything here is the vendor's own words, measured against 2.1.237's
//! `--help`. Re-measure at every vendor bump: these are not amx's names to
//! choose, and a renamed mode or a dropped alias turns a dial into a spawn
//! that fails.

use super::{
    Capability, DEFAULT, DialSpec, ForkSpec, Hooks, Moment, SessionSpec, TOOL, Vendor, Wiring,
};

/// claude's entry in the table.
pub const VENDOR: Vendor = Vendor {
    name: "claude",
    // Open: `--help` says an alias "or a model's full name", so the cycle
    // lists the three aliases and the dial takes anything.
    model: Some(DialSpec {
        cycle: &[DEFAULT, "fable", "opus", "sonnet"],
        open: true,
        flag: "--model",
    }),
    // Closed, and the vendor enforces it: `--permission-mode nonsense` is a
    // hard error naming these six.
    permission: Some(DialSpec {
        cycle: &[
            DEFAULT,
            "acceptEdits",
            "auto",
            "bypassPermissions",
            "manual",
            "dontAsk",
            "plan",
        ],
        open: false,
        flag: "--permission-mode",
    }),
    // Closed by judgement rather than by the vendor: `--effort nonsense`
    // warns and falls back to the default rather than refusing. Five levels
    // are the whole documented set, so amx warns at the config it can see
    // instead of leaving the person to find the vendor's warning scrolled off
    // the top of a pane.
    effort: Some(DialSpec {
        cycle: &[DEFAULT, "low", "medium", "high", "xhigh", "max"],
        open: false,
        flag: "--effort",
    }),
    // amx never asks claude to open a session under an id amx chose: claude's
    // own SessionStart hook already names the one it opened
    // (src/hook.rs:311), and the id it wants there is a UUID, not the
    // <stem>-<suffix> amx mints. `--session-id` stays in the entry, but as a
    // flag `resume` and `fork` strip rather than one either ever writes.
    //
    // `--resume` carries an agent on, its value joined on with `=` because
    // that is the one spelling with no ambiguity about where the value is.
    // `--fork-session` is a bare marker written beside it: measured against
    // 2.1.237, it takes no value of its own and only says to branch rather
    // than continue.
    session: Some(SessionSpec {
        start: None,
        resume: "--resume",
        joined: true,
        conflicts: &["--session-id", "-r"],
        fork: Some(ForkSpec::Marker("--fork-session")),
    }),
    // Measured at 2.1.240 on 2026-08-24: every process the vendor starts, a
    // tool call or a hook alike, is handed this, holding the same session id
    // its hook payloads carry.
    session_env: Some("CLAUDE_CODE_SESSION_ID"),
    // The markers of the session a command was typed inside, which is a
    // session an agent amx starts is not in. Measured at 2.1.240 on
    // 2026-08-25: carrying them, the vendor came up with "Transcript saving
    // is off, inherited CLAUDE_CODE_CHILD_SESSION marker", and an agent with
    // no transcript is one `result` cannot quote and `resume` and `fork`
    // cannot continue.
    //
    // Preferences are about the person and ride along; these name the
    // session. CLAUDE_EFFORT is here because it is the spawner's own dial,
    // and a dial nobody turned on this agent is a flag amx does not pass.
    not_inherited: &[
        "CLAUDECODE",
        "CLAUDE_PID",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_CODE_CHILD_SESSION",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_EXECPATH",
        "CLAUDE_EFFORT",
    ],
    // All of them, because amx was written against this vendor: the hooks it
    // reports through, the transcript it keeps, `--resume` and
    // `--fork-session`, the session id it hands what it starts, and the
    // folder-trust screen amx answers for a tree it cut itself.
    capabilities: &[
        Capability::Hooks,
        Capability::Transcript,
        Capability::Resume,
        Capability::Fork,
        Capability::Adopt,
        Capability::Trust,
    ],
    hooks: Some(HOOKS),
    // The screens amx has measured off this vendor, every anchor in them with
    // the capture, the version and the date it was read at. The file is the
    // whole of what amx knows how to see on a claude pane.
    screens: Some(include_str!("../../assets/screen-rules.toml")),
};

/// How claude reports what it is doing, and where amx asks it to.
///
/// Seven events out of the vendor's own list, one line each in the settings
/// file. `PostToolUse` is deliberately not among them: nothing amx keeps needs
/// to know that a tool finished, and every tool call would cost a process.
///
/// Measured at 2.1.240 on 2026-08-25. Re-measure at every vendor bump the same
/// way as the dials above: a renamed event is hooks that never fire, and a
/// renamed notification type is a nudge amx reads as a question.
pub const HOOKS: Hooks = Hooks {
    settings: ".claude/settings.json",
    events: &[
        Wiring {
            moment: Moment::Started,
            event: "SessionStart",
            matched: false,
        },
        Wiring {
            moment: Moment::Prompted,
            event: "UserPromptSubmit",
            matched: false,
        },
        Wiring {
            moment: Moment::Calling,
            event: "PreToolUse",
            matched: true,
        },
        Wiring {
            moment: Moment::Asked,
            event: "PermissionRequest",
            matched: true,
        },
        Wiring {
            moment: Moment::Refused,
            event: "PermissionDenied",
            matched: true,
        },
        Wiring {
            moment: Moment::Notified,
            event: "Notification",
            matched: false,
        },
        Wiring {
            moment: Moment::Ended,
            event: "Stop",
            matched: false,
        },
    ],
    // The three events above that take one, and amx wants all of them: what a
    // tool call means for the record is decided by reading the payload, not by
    // asking the vendor to send only some.
    matcher: "*",
    question_tool: "AskUserQuestion",
    idle_notice: "idle_prompt",
    permission_notice: "permission_prompt",
    permission_sentence: "Claude needs your permission to use {tool}",
};

/// The sentence claude puts on a permission box about `tool`.
///
/// The one place a tool name becomes the vendor's own words. What is written
/// when the box goes up has to be what the notification six seconds later will
/// repeat: it is the sentence every reader quotes until that echo lands, and
/// the echo writes the vendor's own words over it.
pub fn permission_sentence(tool: &str) -> String {
    HOOKS.permission_sentence.replace(TOOL, &rendered(tool))
}

/// A tool's name the way claude writes it into that sentence, measured at
/// 2.1.237: the last `__` segment — an MCP tool arrives as
/// `mcp__<server>__<tool>` — with underscores as spaces and a letter raised
/// wherever a word starts, which is after anything that is not a letter or a
/// digit (the vendor's `\b\w`), not only after an underscore. That carries a
/// kebab-case name past its dashes, leaves a built-in like `Bash` as it
/// stands, and keeps a digit's word one word.
fn rendered(tool: &str) -> String {
    let mut boundary = true;
    tool.rsplit("__")
        .next()
        .unwrap_or(tool)
        .chars()
        .map(|letter| {
            let letter = if letter == '_' { ' ' } else { letter };
            let raised = if boundary {
                letter.to_ascii_uppercase()
            } else {
                letter
            };
            boundary = !letter.is_ascii_alphanumeric();
            raised
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_words_a_permission_box_the_way_the_pane_does() {
        // The sentence written when the box goes up is what every reader
        // quotes for the six seconds until the vendor's own notification
        // repeats it. One worded any other way is a sentence nothing drew,
        // handed to whoever has to answer the box.
        assert_eq!(
            permission_sentence("Bash"),
            "Claude needs your permission to use Bash"
        );
        assert_eq!(
            permission_sentence("mcp__playwright__browser_click"),
            "Claude needs your permission to use Browser Click"
        );
    }

    #[test]
    fn claude_raises_a_tools_name_at_every_word_boundary() {
        // The vendor raises a letter wherever a word starts — after anything
        // that is not a letter or a digit — not only after an underscore.
        // Raised the underscore way, a kebab-case name reads
        // 'Resolve-library-id' against the pane's 'Resolve-Library-Id'.
        assert_eq!(
            rendered("mcp__context7__resolve-library-id"),
            "Resolve-Library-Id"
        );
        assert_eq!(rendered("mcp__playwright__browser_click"), "Browser Click");
        assert_eq!(rendered("mcp__acme__fs.read_file"), "Fs.Read File");
        // A digit neither opens a word nor ends one: nothing raises after it.
        assert_eq!(rendered("mcp__totp__get2fa-codes"), "Get2fa-Codes");
        assert_eq!(rendered("Bash"), "Bash");
    }

    #[test]
    fn claude_declares_a_model_a_permission_and_an_effort_dial() {
        // Measured against claude 2.1.237's `--help`. Re-measure at every
        // vendor bump: a renamed mode or a dropped alias turns a dial into a
        // spawn that fails.
        let model = VENDOR.model.expect("claude has a model dial");
        assert_eq!(model.flag, "--model");
        assert_eq!(model.cycle, ["default", "fable", "opus", "sonnet"]);
        assert!(model.open, "--model takes a full model name too");

        let permission = VENDOR.permission.expect("claude has a permission dial");
        assert_eq!(permission.flag, "--permission-mode");
        assert_eq!(
            permission.cycle,
            [
                "default",
                "acceptEdits",
                "auto",
                "bypassPermissions",
                "manual",
                "dontAsk",
                "plan"
            ]
        );
        assert!(!permission.open, "--permission-mode is a closed set");

        let effort = VENDOR.effort.expect("claude has an effort dial");
        assert_eq!(effort.flag, "--effort");
        assert_eq!(
            effort.cycle,
            ["default", "low", "medium", "high", "xhigh", "max"]
        );
        assert!(!effort.open, "--effort is a closed set");
    }

    #[test]
    fn claude_declares_no_start_flag_and_a_resume_flag_joined_with_equals() {
        // amx never asks claude to open a session under an id amx chose: its
        // own SessionStart hook already names the one it opened, and the id
        // it wants there is a UUID, not the id amx mints. `--session-id`
        // stays in the entry as a flag `resume` and `fork` strip rather than
        // write.
        let session = VENDOR
            .session
            .expect("claude declares a session vocabulary");
        assert_eq!(session.start, None);
        assert_eq!(session.resume, "--resume");
        assert!(session.joined, "--resume=<id> is one word, not two");
        assert_eq!(session.conflicts, ["--session-id", "-r"]);
        assert_eq!(session.fork, Some(ForkSpec::Marker("--fork-session")));
    }

    #[test]
    fn claude_names_the_session_every_process_it_starts_belongs_to() {
        // Measured against claude 2.1.240 on 2026-08-24: every process the
        // vendor starts, a tool call or a hook alike, is handed
        // CLAUDE_CODE_SESSION_ID holding the same session id its hook payloads
        // carry. Re-measure it at every vendor bump: it is the whole of how an
        // adopted agent's events find their way home.
        assert_eq!(VENDOR.session_env, Some("CLAUDE_CODE_SESSION_ID"));
    }

    #[test]
    fn claude_keeps_the_markers_of_the_session_a_spawn_was_typed_inside() {
        // Measured at 2.1.240 on 2026-08-25: a vendor handed its spawner's
        // markers believes it is a child of that session, and came up with
        // "Transcript saving is off, inherited CLAUDE_CODE_CHILD_SESSION
        // marker". An agent with no transcript is one `result` cannot quote
        // and `resume` and `fork` cannot continue.
        assert_eq!(
            VENDOR.not_inherited,
            [
                "CLAUDECODE",
                "CLAUDE_PID",
                "CLAUDE_CODE_SESSION_ID",
                "CLAUDE_CODE_CHILD_SESSION",
                "CLAUDE_CODE_ENTRYPOINT",
                "CLAUDE_CODE_EXECPATH",
                "CLAUDE_EFFORT",
            ]
        );

        // Preferences are about the person rather than the session, and they
        // ride along. CLAUDE_EFFORT is above with the markers because it is
        // the spawner's own dial, and a dial nobody turned on this agent is a
        // flag amx does not pass.
        for preference in [
            "CLAUDE_CODE_NO_FLICKER",
            "CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY",
        ] {
            assert!(
                !VENDOR.not_inherited.contains(&preference),
                "{preference} is the person's, not the session's"
            );
        }
    }

    #[test]
    fn claude_names_every_event_amx_wires_into_its_settings() {
        // Measured against claude 2.1.240's own hook list on 2026-08-25, and
        // the whole of what amx asks to be told. Re-measure at every vendor
        // bump: a renamed event is a hook that never fires again, and nothing
        // says so — the record simply stops moving.
        let hooks = VENDOR.hooks.expect("claude reports through hooks");
        assert_eq!(hooks.settings, ".claude/settings.json");

        let wired: Vec<(Moment, &str, bool)> = hooks
            .events
            .iter()
            .map(|wiring| (wiring.moment, wiring.event, wiring.matched))
            .collect();
        assert_eq!(
            wired,
            [
                (Moment::Started, "SessionStart", false),
                (Moment::Prompted, "UserPromptSubmit", false),
                (Moment::Calling, "PreToolUse", true),
                (Moment::Asked, "PermissionRequest", true),
                (Moment::Refused, "PermissionDenied", true),
                (Moment::Notified, "Notification", false),
                (Moment::Ended, "Stop", false),
            ]
        );
        assert_eq!(hooks.matcher, "*", "every tool, on the three that ask");

        // PostToolUse is the vendor's word for a tool that finished, and it is
        // left out on purpose: nothing amx keeps needs it, and it would cost a
        // process on every tool call. What that costs instead is written down
        // in `hook` — the record still reads waiting when a box is approved.
        assert_eq!(hooks.moment("PostToolUse"), None);
    }

    #[test]
    fn claude_names_the_tool_that_asks_and_the_notices_it_sends() {
        // Three words the vendor writes into payloads, and amx has no other
        // way to tell one screen from another. Measured at 2.1.240 on
        // 2026-08-25: AskUserQuestion draws a menu rather than doing work, and
        // a notification carries idle_prompt when nothing is open on the
        // session or permission_prompt when a box is.
        let hooks = VENDOR.hooks.expect("claude reports through hooks");
        assert_eq!(hooks.question_tool, "AskUserQuestion");
        assert_eq!(hooks.idle_notice, "idle_prompt");
        assert_eq!(hooks.permission_notice, "permission_prompt");
        assert_ne!(
            hooks.idle_notice, hooks.permission_notice,
            "a nudge about a session nobody is using is not a question"
        );
    }

    #[test]
    fn claude_can_do_everything_amx_knows_how_to_ask_a_vendor_for() {
        // The vendor amx was written against, so every capability in the list
        // is one it was written against too. A second vendor is where the
        // absences start, and where a verb's refusal has to say something.
        for what in [
            Capability::Hooks,
            Capability::Transcript,
            Capability::Resume,
            Capability::Fork,
            Capability::Adopt,
            Capability::Trust,
        ] {
            assert!(VENDOR.can(what), "{what:?}");
        }
    }
}
