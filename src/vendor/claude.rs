//! claude, the vendor amx was written against.
//!
//! Everything here is the vendor's own words, measured against 2.1.237's
//! `--help`. Re-measure at every vendor bump: these are not amx's names to
//! choose, and a renamed mode or a dropped alias turns a dial into a spawn
//! that fails.

use super::{Capability, DEFAULT, DialSpec, Vendor};

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
};

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn nothing_else_in_amx_has_measured_these_differently() {
        // spawn and adopt each keep a copy of the measurement above, and two
        // copies of a measurement is one too many. This is what holds them
        // together while there are two of them.
        let ships = |source: &str| {
            source
                .split("#[cfg(test)]")
                .next()
                .unwrap_or(source)
                .to_string()
        };

        let spawn = ships(include_str!("../spawn.rs"));
        for name in VENDOR.not_inherited {
            assert!(
                spawn.contains(&format!("\"{name}\"")),
                "spawn lets {name} travel to a pane it starts"
            );
        }

        let adopt = ships(include_str!("../verbs/adopt.rs"));
        let session = VENDOR.session_env.expect("claude names its session");
        assert!(
            adopt.contains(&format!("\"{session}\"")),
            "adopt looks for a session variable claude does not name"
        );
    }
}
