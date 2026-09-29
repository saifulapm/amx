//! pi's entry in the vendor table.
//!
//! Every value is pi's own spelling, checked against pi 0.84.4 to 0.87.1: its
//! `--help`, its README and the unbundled JS shipped with it. Re-check on every
//! vendor bump: a renamed flag turns a dial into a spawn that fails.

use super::{
    Capability, Catalog, DEFAULT, DialSpec, ForkSpec, Hooks, Models, Moment, Place, Resume,
    SessionSpec, Transcript, Vendor, Wire, Wiring,
};

/// pi's entry in the table.
pub const VENDOR: Vendor = Vendor {
    name: "pi",
    // Open: `--model <pattern>` takes any provider/id pattern and pi names no
    // aliases, so the cycle is only the sentinel (0.84.4).
    model: Some(DialSpec::open("--model", &[DEFAULT])),
    // `--list-models` prints a header line, then one row per model with the
    // provider and id first (0.85.1).
    models: Models::Printed(&["--list-models"]),
    // No flag in `--help` restricts what pi may do (0.84.4).
    permission: None,
    // Closed: `--thinking <level>` documents exactly these levels (0.84.4).
    effort: Some(DialSpec::closed(
        "--thinking",
        &[
            DEFAULT, "off", "minimal", "low", "medium", "high", "xhigh", "max",
        ],
    )),
    // `--session-id <id>` opens the project session with that id, or creates
    // it (dist/main.js:337-344), so it is both the start and the resume flag.
    // Two words: nothing in dist/main.js reads `--session-id=<id>`.
    //
    // Conflicts: pi refuses `--session-id` with `--session`, `--continue` or
    // `--resume` (dist/main.js:237-247); `-c` and `-r` are their short forms.
    // `--no-session` accepts it but keeps the session in memory only
    // (dist/main.js:278-280), so amx would record a session that was never
    // written; `validateForkFlags` groups it with the other three
    // (dist/main.js:227-231).
    //
    // `pi --fork <origin> --session-id <new>` branches into a chosen id, with
    // the origin on `--fork` (dist/main.js:283-295, 0.84.4).
    session: Some(SessionSpec {
        start: Some("--session-id"),
        resume: Resume::Flag {
            flag: "--session-id",
            joined: false,
        },
        conflicts: &[
            "-c",
            "-r",
            "--continue",
            "--no-session",
            "--resume",
            "--session",
        ],
        fork: Some(ForkSpec::Origin("--fork")),
    }),
    // Set on every command pi's bash tool runs, to the open session's id
    // (core/tools/bash.js, resolveSpawnContext, 0.84.4).
    session_env: Some("PI_SESSION_ID"),
    // The session variables resolveSpawnContext strips and reissues, plus the
    // markers dist/cli.js and dist/rpc-entry.js set on every process pi starts
    // (0.84.4). Inherited, they make the new agent think it is the spawner's
    // session.
    not_inherited: &[
        "PI_SESSION_ID",
        "PI_SESSION_FILE",
        "PI_PROVIDER",
        "PI_MODEL",
        "PI_REASONING_LEVEL",
        "AI_AGENT",
        "PI_CODING_AGENT",
    ],
    // Hooks and Transcript come from the extension in `HOOKS`, whose reports
    // name the session file. Trust is `--approve` on the pane's argv, which
    // trusts project-local files for one run and writes nothing; see
    // `crate::trust`.
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
    screens: Some(include_str!("../../assets/screen-rules-pi.toml")),
    // One file per session under `~/.pi/agent/sessions/<cwd>/`, named by
    // session id.
    transcript: Some(Transcript::Pi),
    // From the README shipped with 0.85.1: four skill directories in pi's
    // reading order and two prompt-template directories. A skill runs as
    // `/skill:name`. pi has no subagents, so no agent places.
    //
    // The built-ins are the README's Commands table in order, `/login` to
    // `/quit`; its first row names two commands.
    catalog: Some(Catalog {
        skills: &[
            Place::Person(".pi/agent/skills"),
            Place::Person(".agents/skills"),
            Place::Project(".pi/skills"),
            Place::Project(".agents/skills"),
        ],
        commands: &[
            Place::Person(".pi/agent/prompts"),
            Place::Project(".pi/prompts"),
        ],
        agents: &[],
        agent_flag: None,
        builtins: &[
            "login",
            "logout",
            "llama",
            "model",
            "thinking",
            "scoped-models",
            "settings",
            "resume",
            "new",
            "name",
            "session",
            "tree",
            "trust",
            "fork",
            "clone",
            "compact",
            "copy",
            "export",
            "import",
            "share",
            "reload",
            "hotkeys",
            "changelog",
            "quit",
        ],
        sigil: '/',
        skill_prefix: "skill:",
    }),
    // A message word starting with `-` is read as a flag until `--`
    // (dist/cli/args.js:23, 0.87.1).
    ends_options: Some("--"),
    // A word starting with `@` is a file to attach on either side of `--`
    // (args.js:25 and :224); a leading space makes it plain text:
    // `pi -p -- ' @README say hello'` says hello (0.87.1).
    attaches_at: true,
    // Escape with steering or follow-up messages queued puts them back in the
    // editor, joined and unsent (interactive-mode.js:2332 and 3761-3778,
    // 0.87.1).
    restores_queued_on_cancel: true,
    prompt_flag: None,
    popups: &[],
    cancel_presses: 1,
    interrupt_signal: None,
    launch: &[],
};

/// The opt-in `subagent` tool, written by `amx setup pi --subagent`.
///
/// A second extension beside `amx.ts` so pi loads it separately. It reports
/// nothing: it registers a `subagent` tool that runs `amx sub` via `$AMX_BIN`
/// and returns the child's answer. pi loads every `.ts` file directly under
/// the extensions directory, so it is a single file rather than a directory.
pub const SUBAGENT: Wire = Wire::File {
    path: ".pi/agent/extensions/amx-subagent.ts",
    body: include_str!("../../assets/pi/amx-subagent.ts"),
};

/// pi's extension events and the words its payloads use.
///
/// pi has no settings file to name a hook command in: its events are
/// callbacks inside its own process. So the wire is `assets/pi/amx.ts`, an
/// extension that runs `amx _hook` once per event with the payload on stdin,
/// under pi's event names and claude's payload keys.
///
/// Checked against the 0.84.4 extension API. `ui_prompt_start` is a stop on a
/// question, like claude's `Notification`, and `ui_prompt_end` is the prompt
/// closing with the turn going on, which the record treats as `Refused`. There
/// is no `Asked`: pi asks permission for nothing. `tests/mock_pi/pi` delivers
/// these events the same way, so the suite runs without pi installed.
pub const HOOKS: Hooks = Hooks {
    wire: Wire::File {
        path: ".pi/agent/extensions/amx.ts",
        body: include_str!("../../assets/pi/amx.ts"),
    },
    opt_in: &[SUBAGENT],
    events: &[
        Wiring::new(Moment::Started, "session_start"),
        Wiring::new(Moment::Prompted, "agent_start"),
        Wiring::new(Moment::Calling, "tool_execution_start"),
        // User messages only (the extension filters the role): a message
        // steered into a running turn starts no new agent.
        Wiring::new(Moment::Taken, "message_start"),
        Wiring::new(Moment::Notified, "ui_prompt_start"),
        Wiring::new(Moment::Refused, "ui_prompt_end"),
        Wiring::new(Moment::Ended, "agent_settled"),
    ],
    // pi has no matcher, no menu tool, no typed notices, no permission box and
    // no turns of its own.
    matcher: "",
    question_tool: "",
    idle_notice: "",
    permission_notice: "",
    permission_sentence: "",
    injected: &[],
    // Session start carries no `source`.
    fresh_start: None,
    // Dialog kinds the extension reports with `ui_prompt_start`
    // (dist/core/extensions/runner.js:321-325, 0.87.1). `custom` is left out:
    // it is whatever an extension draws, not a question.
    question_kinds: &["input", "editor", "select", "confirm"],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_is_wired_through_a_file_pi_loads_as_an_extension() {
        // pi 0.84.4 loads global extensions from `extensions/` under its agent
        // directory.
        let Wire::File { path, body } = HOOKS.wire else {
            panic!("pi has no settings file to name a hook in");
        };
        assert_eq!(path, ".pi/agent/extensions/amx.ts");
        assert!(
            body.starts_with("// installed by amx\n"),
            "the first line is how uninstall knows the file is amx's"
        );
        assert!(
            body.contains("\"_hook\""),
            "it reports through amx's hook command"
        );
        for wiring in HOOKS.events {
            assert!(
                body.contains(&format!("pi.on(\"{}\"", wiring.event)),
                "the extension never listens for {}",
                wiring.event
            );
            assert_eq!(HOOKS.moment(wiring.event), Some(wiring.moment));
        }
        assert!(body.contains("pi.on(\"message_update\""), "and it streams");
        assert!(
            body.contains(&format!("\"{}\"", crate::store::HEARTBEAT)),
            "and it beats on the record while a turn runs"
        );
    }

    #[test]
    fn pi_ships_the_subagent_tool_as_a_wire_of_its_own() {
        // A separate file, so it is only there when asked for.
        let Wire::File { path, body } = SUBAGENT else {
            panic!("the tool is not a file pi loads")
        };
        assert_eq!(path, ".pi/agent/extensions/amx-subagent.ts");
        assert!(HOOKS.opt_in.contains(&SUBAGENT), "it is opted into by name");
        assert!(
            body.starts_with("// installed by amx\n"),
            "the first line is how uninstall knows the file is amx's"
        );
        assert!(body.contains("registerTool"), "it gives the agent a tool");
        assert!(body.contains("\"subagent\""), "named what it is");
        assert!(body.contains("\"sub\""), "and it runs the verb");
        assert!(
            body.contains("params.role") && body.contains("\"--role\""),
            "and it can name a role for the child: {body}"
        );
        assert!(
            !body.contains("\"_hook\""),
            "the tool reports nothing; that is the other wire's work"
        );
    }

    #[test]
    fn pi_asks_leave_for_nothing_and_says_so_by_naming_no_such_moment() {
        assert!(
            !HOOKS.events.iter().any(|w| w.moment == Moment::Asked),
            "pi draws no permission box"
        );
        assert_eq!(HOOKS.moment("ui_prompt_start"), Some(Moment::Notified));
        assert_eq!(HOOKS.moment("ui_prompt_end"), Some(Moment::Refused));
        // A steered message gets no new agent_start, only its user message.
        assert_eq!(HOOKS.moment("message_start"), Some(Moment::Taken));
        assert_eq!(HOOKS.moment("Stop"), None, "claude's names are not pi's");
    }

    #[test]
    fn pi_types_no_prompt_of_its_own_and_says_how_a_turn_ended() {
        assert!(HOOKS.injected.is_empty());
        // The extension reports the stop reason; a branch ending on a tool
        // result is a turn cut off after the tool ran.
        let Wire::File { body, .. } = HOOKS.wire else {
            unreachable!()
        };
        assert!(body.contains("fields.stop_reason ="), "{body}");
        assert!(body.contains("role === \"toolResult\""), "{body}");
    }

    #[test]
    fn pi_declares_a_model_and_an_effort_dial_and_no_permission_dial() {
        // Checked against pi 0.84.4's `--help`.
        let model = VENDOR.model.expect("pi has a model dial");
        assert_eq!(model.flag, "--model");
        assert_eq!(model.cycle, ["default"]);
        assert!(model.open, "--model takes any provider/id pattern");

        assert!(VENDOR.permission.is_none(), "pi has no permission flag");

        let effort = VENDOR.effort.expect("pi has an effort dial");
        assert_eq!(effort.flag, "--thinking");
        assert_eq!(
            effort.cycle,
            [
                "default", "off", "minimal", "low", "medium", "high", "xhigh", "max"
            ]
        );
        assert!(!effort.open, "--thinking is a closed set");
    }

    #[test]
    fn pi_lists_its_models_by_printing_them() {
        // pi's models are whatever its providers offer (0.85.1).
        assert_eq!(VENDOR.models, Models::Printed(&["--list-models"]));
    }

    #[test]
    fn pi_mints_or_opens_a_session_with_the_same_flag_written_two_words() {
        // `--session-id <id>` opens the session or creates it, so it serves
        // for both starting and resuming.
        let session = VENDOR.session.expect("pi declares a session vocabulary");
        assert_eq!(session.start, Some("--session-id"));
        assert_eq!(
            session.resume,
            Resume::Flag {
                flag: "--session-id",
                joined: false
            },
            "--session-id <id> is two words, not one"
        );
        assert_eq!(session.fork, Some(ForkSpec::Origin("--fork")));
    }

    #[test]
    fn pi_lists_every_flag_that_would_ignore_or_refuse_a_minted_id() {
        // Five make pi refuse the id. `--no-session` accepts it but never
        // writes the session, so amx would record one nobody can return to.
        let session = VENDOR.session.expect("pi declares a session vocabulary");
        assert_eq!(
            session.conflicts,
            [
                "-c",
                "-r",
                "--continue",
                "--no-session",
                "--resume",
                "--session"
            ]
        );
    }

    #[test]
    fn pi_names_the_session_every_command_its_bash_tool_runs_belongs_to() {
        // core/tools/bash.js, resolveSpawnContext (0.84.4).
        assert_eq!(VENDOR.session_env, Some("PI_SESSION_ID"));
    }

    #[test]
    fn pi_keeps_the_markers_of_the_session_a_spawn_was_typed_inside() {
        // resolveSpawnContext's session variables, plus the process markers
        // from dist/cli.js and dist/rpc-entry.js.
        assert_eq!(
            VENDOR.not_inherited,
            [
                "PI_SESSION_ID",
                "PI_SESSION_FILE",
                "PI_PROVIDER",
                "PI_MODEL",
                "PI_REASONING_LEVEL",
                "AI_AGENT",
                "PI_CODING_AGENT",
            ]
        );
    }

    #[test]
    fn pi_names_its_four_skill_places_its_two_prompt_places_and_its_own_commands() {
        // From the README shipped with pi 0.85.1. The Commands table's first
        // row names two commands.
        let catalog = VENDOR.catalog.expect("pi loads files by name");
        assert_eq!(
            catalog.skills,
            [
                Place::Person(".pi/agent/skills"),
                Place::Person(".agents/skills"),
                Place::Project(".pi/skills"),
                Place::Project(".agents/skills"),
            ]
        );
        assert_eq!(
            catalog.commands,
            [
                Place::Person(".pi/agent/prompts"),
                Place::Project(".pi/prompts"),
            ]
        );
        assert!(
            catalog.agents.is_empty(),
            "pi ships no sub agents, so nothing typed with @ names one"
        );
        assert_eq!(
            catalog.agent_flag, None,
            "and there is no agent for a line to ask pi to be"
        );
        assert_eq!(catalog.sigil, '/', "pi opens every word with a slash");
        assert_eq!(
            catalog.skill_prefix, "skill:",
            "pi runs a skill as /skill:name, which claude does not"
        );
        assert_eq!(
            catalog.builtins,
            [
                "login",
                "logout",
                "llama",
                "model",
                "thinking",
                "scoped-models",
                "settings",
                "resume",
                "new",
                "name",
                "session",
                "tree",
                "trust",
                "fork",
                "clone",
                "compact",
                "copy",
                "export",
                "import",
                "share",
                "reload",
                "hotkeys",
                "changelog",
                "quit",
            ]
        );
    }

    #[test]
    fn pi_can_resume_fork_be_adopted_and_have_its_trust_screen_answered() {
        // Trust is a flag on the argv; src/trust.rs tests which one.
        for can in [
            Capability::Hooks,
            Capability::Transcript,
            Capability::Resume,
            Capability::Fork,
            Capability::Adopt,
            Capability::Trust,
        ] {
            assert!(VENDOR.can(can), "{can:?}");
        }
    }

    #[test]
    fn pi_reports_through_its_extension_and_keeps_its_screens_for_when_it_is_quiet() {
        // Screens still matter: a pi without the extension, and the gates pi
        // draws before a turn, are read off the pane. src/rules.rs tests which
        // screens the document holds.
        assert_eq!(VENDOR.hooks, Some(HOOKS));
        assert!(VENDOR.can(Capability::Hooks));
        assert!(VENDOR.can(Capability::Transcript));
        assert_eq!(VENDOR.transcript, Some(Transcript::Pi));
        assert!(VENDOR.screens.is_some(), "pi declares screens");
    }
}
