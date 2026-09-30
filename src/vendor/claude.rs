//! claude's entry in the vendor table.
//!
//! Every value is claude's own spelling, checked against claude 2.1.237 to
//! 2.1.270. Re-check on every vendor bump: a renamed mode or a dropped alias
//! turns a dial into a spawn that fails.

use super::{
    Capability, Catalog, DEFAULT, DialSpec, ForkSpec, Hooks, Models, Moment, Place, Resume,
    SessionSpec, Transcript, Vendor, Wire, Wiring,
};

/// claude's entry in the table.
pub const VENDOR: Vendor = Vendor {
    name: "claude",
    // Open: `--help` takes an alias "or a model's full name". The cycle lists
    // the aliases: fable, opus and sonnet from `--help` (2.1.263), plus haiku.
    model: Some(DialSpec::open(
        "--model",
        &[DEFAULT, "fable", "opus", "sonnet", "haiku"],
    )),
    // claude prints no model list, so the aliases above are all amx offers.
    models: Models::Cycle,
    // Closed by the vendor: `--permission-mode nonsense` is a hard error.
    permission: Some(DialSpec::closed(
        "--permission-mode",
        &[
            DEFAULT,
            "acceptEdits",
            "auto",
            "bypassPermissions",
            "manual",
            "dontAsk",
            "plan",
        ],
    )),
    // Closed by amx: `--effort nonsense` only warns and falls back, so amx
    // warns at config time instead of in a pane that may have scrolled.
    effort: Some(DialSpec::closed(
        "--effort",
        &[DEFAULT, "low", "medium", "high", "xhigh", "max"],
    )),
    // No start flag: the SessionStart hook names the session claude opened,
    // and `--session-id` wants a UUID rather than an amx id, so it is only
    // listed as a conflict for resume and fork to strip.
    //
    // `--resume=<id>` is joined with `=` so the value's position is never
    // ambiguous. `--fork-session` takes no value and marks the resume as a
    // branch (2.1.237).
    session: Some(SessionSpec {
        start: None,
        resume: Resume::Flag {
            flag: "--resume",
            joined: true,
        },
        conflicts: &["--session-id", "-r"],
        fork: Some(ForkSpec::Marker("--fork-session")),
    }),
    // Set on every process claude starts, tools and hooks alike, to the
    // session id its hook payloads carry (2.1.240).
    session_env: Some("CLAUDE_CODE_SESSION_ID"),
    // The spawning session's markers. Inherited, they make claude start with
    // "Transcript saving is off, inherited CLAUDE_CODE_CHILD_SESSION marker"
    // (2.1.240), and without a transcript `result`, `resume` and `fork` break.
    // CLAUDE_EFFORT is the spawner's dial, not this agent's. Preferences such
    // as CLAUDE_CODE_NO_FLICKER are inherited.
    not_inherited: &[
        "CLAUDECODE",
        "CLAUDE_PID",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_CODE_CHILD_SESSION",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_EXECPATH",
        "CLAUDE_EFFORT",
    ],
    capabilities: &[
        Capability::Hooks,
        Capability::Transcript,
        Capability::Resume,
        Capability::Fork,
        Capability::Adopt,
        Capability::Trust,
    ],
    hooks: Some(HOOKS),
    // Each rule in the file records the capture and version it was read from.
    screens: Some(include_str!("../../assets/screen-rules.toml")),
    // The session files under `~/.claude/projects/`.
    transcript: Some(Transcript::Claude),
    // Skills, commands and agents from the person's and the project's
    // `.claude`, plus plugin skills and commands under
    // `plugins/cache/<market>/<plugin>/<version>/` (2.1.263). A skill runs as
    // `/name`, hence the empty prefix.
    //
    // The built-ins are the words claude's agent view dispatches as a first
    // prompt: its bundled skills and the built-ins that expand into a prompt.
    // It refuses every other built-in with "attach to a session to run it".
    // Taken from code.claude.com/docs/en/commands at 2.1.263, not from the `/`
    // menu, which also lists built-ins a fresh session cannot run.
    catalog: Some(Catalog {
        skills: &[
            Place::Person(".claude/skills"),
            Place::Project(".claude/skills"),
            Place::Person(".claude/plugins/cache/*/*/*/skills"),
        ],
        commands: &[
            Place::Person(".claude/commands"),
            Place::Project(".claude/commands"),
            Place::Person(".claude/plugins/cache/*/*/*/commands"),
        ],
        agents: &[
            Place::Person(".claude/agents"),
            Place::Project(".claude/agents"),
        ],
        // `--agent <agent>` overrides the agent the settings name (2.1.263).
        agent_flag: Some("--agent"),
        builtins: &[
            "batch",
            "claude-api",
            "code-review",
            "dataviz",
            "debug",
            "deep-research",
            "design",
            "design-sync",
            "doctor",
            "fewer-permission-prompts",
            "init",
            "loop",
            "plan",
            "review",
            "run",
            "run-skill-generator",
            "security-review",
            "simplify",
            "verify",
            "workflow-authoring",
        ],
        sigil: '/',
        skill_prefix: "",
    }),
    // The last word is the prompt whatever it starts with.
    ends_options: None,
    attaches_at: false,
    restores_queued_on_cancel: false,
    // 2.1.284's permission menu ignores `y`; digits work.
    menus_take_letters: false,
    prompt_flag: None,
    popups: &[],
    cancel_presses: 1,
    interrupt_signal: None,
    launch: &[],
};

/// claude's hook events and the words its payloads use.
///
/// `PostToolUse` is left out on purpose: amx needs nothing from it, and it
/// would cost a process per tool call. Checked against 2.1.240; a renamed
/// event means hooks that never fire, and a renamed notification type turns a
/// nudge into a question.
pub const HOOKS: Hooks = Hooks {
    // A plugin in the skills directory rather than entries in
    // `~/.claude/settings.json`: claude loads a directory there that has a
    // manifest as `<name>@skills-dir`, for every project, read live, and the
    // person's settings are never opened (2.1.270).
    wire: Wire::Plugin {
        dir: ".claude/skills/amx",
        files: &[
            (
                crate::install::MANIFEST,
                include_str!("../../assets/claude/plugin.json"),
            ),
            (
                "hooks/hooks.json",
                include_str!("../../assets/claude/hooks.json"),
            ),
            ("SKILL.md", include_str!("../../skill/amx/SKILL.md")),
        ],
    },
    opt_in: &[],
    events: &[
        Wiring::new(Moment::Started, "SessionStart"),
        Wiring::new(Moment::Prompted, "UserPromptSubmit"),
        Wiring::with_matcher(Moment::Calling, "PreToolUse"),
        Wiring::with_matcher(Moment::Asked, "PermissionRequest"),
        Wiring::with_matcher(Moment::Refused, "PermissionDenied"),
        Wiring::new(Moment::Notified, "Notification"),
        Wiring::new(Moment::Ended, "Stop"),
    ],
    // Every tool: the hook reads the payload to decide what a call means.
    matcher: "*",
    question_tool: "AskUserQuestion",
    idle_notice: "idle_prompt",
    permission_notice: "permission_prompt",
    permission_sentence: "Claude needs your permission to use {tool}",
    // A finished background task and a message from another agent, seen in
    // real UserPromptSubmit payloads.
    injected: &["<task-notification", "<agent-message"],
    // Resume, clear and compact report other sources
    // (code.claude.com/docs/en/hooks).
    fresh_start: Some("startup"),
    // Notices carry a type but no kind; questions come from the question tool.
    question_kinds: &[],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_words_a_permission_box_the_way_the_pane_does() {
        // Written when the box opens and quoted until claude's own
        // notification repeats it, so it must match the pane's wording.
        assert_eq!(
            HOOKS.permission_sentence("Bash").as_deref(),
            Some("Claude needs your permission to use Bash")
        );
        assert_eq!(
            HOOKS
                .permission_sentence("mcp__playwright__browser_click")
                .as_deref(),
            Some("Claude needs your permission to use Browser Click")
        );
    }

    #[test]
    fn claude_declares_a_model_a_permission_and_an_effort_dial() {
        // Checked against claude 2.1.237's `--help`.
        let model = VENDOR.model.expect("claude has a model dial");
        assert_eq!(model.flag, "--model");
        assert_eq!(model.cycle, ["default", "fable", "opus", "sonnet", "haiku"]);
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
    fn claude_lists_its_models_in_the_cycle_its_dial_already_names() {
        assert_eq!(VENDOR.models, Models::Cycle);
    }

    #[test]
    fn claude_declares_no_start_flag_and_a_resume_flag_joined_with_equals() {
        // SessionStart names the session, and `--session-id` wants a UUID, so
        // it is only a conflict for resume and fork to strip.
        let session = VENDOR
            .session
            .expect("claude declares a session vocabulary");
        assert_eq!(session.start, None);
        assert_eq!(
            session.resume,
            Resume::Flag {
                flag: "--resume",
                joined: true
            },
            "--resume=<id> is one word, not two"
        );
        assert_eq!(session.conflicts, ["--session-id", "-r"]);
        assert_eq!(session.fork, Some(ForkSpec::Marker("--fork-session")));
    }

    #[test]
    fn claude_names_the_session_every_process_it_starts_belongs_to() {
        // How an adopted agent's events find their record (2.1.240).
        assert_eq!(VENDOR.session_env, Some("CLAUDE_CODE_SESSION_ID"));
    }

    #[test]
    fn claude_keeps_the_markers_of_the_session_a_spawn_was_typed_inside() {
        // Inherited, these made claude 2.1.240 turn transcript saving off.
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

        // Preferences are the person's and are inherited.
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
    fn claude_names_every_event_amx_wires_into_its_plugin() {
        // Checked against claude 2.1.240's hook list. A renamed event fails
        // silently: the record just stops moving.
        let hooks = VENDOR.hooks.expect("claude reports through hooks");
        let Wire::Plugin { dir, files } = hooks.wire else {
            panic!("claude reports through a plugin amx writes");
        };
        assert_eq!(
            dir, ".claude/skills/amx",
            "the skills directory is where claude loads a plugin without a marketplace"
        );
        let shipped: Vec<&str> = files.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            shipped,
            [crate::install::MANIFEST, "hooks/hooks.json", "SKILL.md",],
            "the manifest claude loads it by, the file that wires the events, and amx's own skill"
        );

        // install.rs checks the shipped hooks file against this table.

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

        // Left out on purpose; the cost, a record that still reads waiting
        // after a box is approved, is handled in `hook`.
        assert_eq!(hooks.moment("PostToolUse"), None);
    }

    #[test]
    fn claude_names_the_tool_that_asks_and_the_notices_it_sends() {
        // AskUserQuestion draws a menu; a notification carries idle_prompt
        // when nothing is open and permission_prompt when a box is (2.1.240).
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
    fn claude_names_the_prompts_it_types_into_a_session_itself() {
        // A finished background task and a message from another agent arrive
        // as prompts nobody typed, each opening with its own tag.
        let hooks = VENDOR.hooks.expect("claude reports through hooks");
        assert_eq!(hooks.injected, ["<task-notification", "<agent-message"]);
    }

    #[test]
    fn claude_names_where_it_loads_skills_commands_and_agents_from() {
        // Checked against claude 2.1.263. A moved directory means suggestions
        // that never arrive.
        let catalog = VENDOR.catalog.expect("claude loads files by name");
        assert_eq!(
            catalog.skills,
            [
                Place::Person(".claude/skills"),
                Place::Project(".claude/skills"),
                Place::Person(".claude/plugins/cache/*/*/*/skills"),
            ]
        );
        assert_eq!(
            catalog.commands,
            [
                Place::Person(".claude/commands"),
                Place::Project(".claude/commands"),
                Place::Person(".claude/plugins/cache/*/*/*/commands"),
            ]
        );
        assert_eq!(
            catalog.agents,
            [
                Place::Person(".claude/agents"),
                Place::Project(".claude/agents"),
            ]
        );
        assert_eq!(
            catalog.agent_flag,
            Some("--agent"),
            "and it can be told to run a session as one of them"
        );
        assert_eq!(catalog.sigil, '/', "claude opens every word with a slash");
        assert_eq!(catalog.skill_prefix, "", "claude runs a skill by its name");
        // From code.claude.com/docs/en/commands at 2.1.263; `--help` lists
        // none of them.
        assert_eq!(
            catalog.builtins,
            [
                "batch",
                "claude-api",
                "code-review",
                "dataviz",
                "debug",
                "deep-research",
                "design",
                "design-sync",
                "doctor",
                "fewer-permission-prompts",
                "init",
                "loop",
                "plan",
                "review",
                "run",
                "run-skill-generator",
                "security-review",
                "simplify",
                "verify",
                "workflow-authoring",
            ]
        );
    }

    #[test]
    fn claude_can_do_everything_amx_knows_how_to_ask_a_vendor_for() {
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
