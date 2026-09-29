//! opencode's entry in the vendor table.
//!
//! Every value is opencode's own spelling, checked against opencode 2.0.16 and
//! its source (tag v2.0.16; paths below are relative to its `packages/`).
//! Re-check on every vendor bump: a renamed flag turns a dial into a spawn
//! that fails.

use super::{
    Capability, Catalog, DEFAULT, DialSpec, Hooks, Models, Moment, Place, Resume, SessionSpec,
    Transcript, Vendor, Wire, Wiring,
};

/// opencode's entry in the table.
pub const VENDOR: Vendor = Vendor {
    name: "opencode",
    // The TUI has no model flag (its root options are standalone, server,
    // auto, directory, continue, session and prompt;
    // cli/src/commands/commands.ts:26-58). The model goes into the pane's env
    // as `OPENCODE_CONFIG_CONTENT`, which is loaded over every config file
    // (core/src/config.ts:230). It only applies to `new`: a resumed session
    // keeps its model (tui/src/context/local.tsx:252-260). Open, since a model
    // is `provider/model` and opencode names no aliases.
    model: Some(DialSpec {
        key: Some("model"),
        env: Some("OPENCODE_CONFIG_CONTENT"),
        ..DialSpec::open("", &[DEFAULT])
    }),
    // `opencode models` starts the shared service with the caller's env, so
    // amx never runs it.
    models: Models::Cycle,
    // `--auto` approves every permission not explicitly denied, and takes no
    // value (cli/src/commands/commands.ts:27-30).
    permission: Some(DialSpec {
        bare: true,
        ..DialSpec::closed("--auto", &[DEFAULT, "auto"])
    }),
    // A `#variant` on the model loses to opencode's stored per-model variant
    // (tui/src/context/local.tsx:236), so there is no effort dial.
    effort: None,
    // opencode mints its own ids and the TUI takes none to start under, so the
    // plugin's Started names the session. `-s, --session <id>` opens an
    // existing session, as two words (tui/src/app.tsx:655-661). `-c,
    // --continue` opens the newest in the cwd (app.tsx:664-690), and
    // `--server` picks which process's sessions these are. The TUI has no fork
    // flag (cli/src/commands/handlers/default.ts:82-87).
    session: Some(SessionSpec {
        start: None,
        resume: Resume::Flag {
            flag: "--session",
            joined: false,
        },
        conflicts: &["-s", "-c", "--continue", "--server"],
        fork: None,
    }),
    // The shell tool adds only `TERM` and `OPENCODE_TERMINAL`
    // (core/src/shell.ts:264-268), so nothing names the session.
    session_env: None,
    // What a pane started inside opencode would inherit: the shell and PTY
    // markers (core/src/shell.ts:264-268, core/src/pty.ts:173), the TUI's
    // initial route and storybook (tui/src/app.tsx:343-356), the PTY handoff
    // (cli/src/server-process.ts:46-47), the server passwords
    // (cli/src/env.ts:10-13) and the app name (default.ts:69).
    not_inherited: &[
        "OPENCODE_TERMINAL",
        "OPENCODE_ROUTE",
        "OPENCODE_STORY",
        "OPENCODE_PTY_HANDOFF",
        "OPENCODE_PASSWORD",
        "OPENCODE_SERVER_PASSWORD",
        "OPENCODE_CLIENT",
    ],
    // Hooks and Transcript come from the plugin in `HOOKS`, which writes the
    // message list at each turn's end. No Fork (no fork flag), no Adopt (no
    // session variable in children) and no Trust (no trust screen).
    capabilities: &[
        Capability::Hooks,
        Capability::Transcript,
        Capability::Resume,
    ],
    hooks: Some(HOOKS),
    // Read off live panes under `--standalone` at four widths.
    screens: Some(include_str!("../../assets/screen-rules-opencode.toml")),
    // The message list the plugin writes to `$AMX_DIR/opencode-messages.jsonl`
    // and names as `transcript_path`.
    transcript: Some(Transcript::Opencode),
    // A command runs as `/name` (tui/src/component/prompt/index.tsx:1141-1144),
    // named by its path under `{command,commands}/`
    // (core/src/config/plugin/command.ts:144,159) in the config dir and the
    // project's `.opencode/` (core/src/config.ts:185-191,221-236). Agents come
    // from `{agent,agents}/` in the same two (core/src/config/plugin/agent.ts:21-24),
    // and the TUI has no flag to run as one. A skill is an autocomplete part
    // with no text spelling (tui/src/component/prompt/autocomplete.tsx:406-440),
    // so no skills. The built-ins, aliases included, come from the app
    // (tui/src/app.tsx:720-1112), the session (routes/session/index.tsx:871-1174),
    // the prompt, the feature plugins and the server
    // (core/src/plugin/command.ts:29-53).
    catalog: Some(Catalog {
        skills: &[],
        commands: &[
            Place::Person(".config/opencode/commands"),
            Place::Person(".config/opencode/command"),
            Place::Project(".opencode/commands"),
            Place::Project(".opencode/command"),
        ],
        agents: &[
            Place::Person(".config/opencode/agents"),
            Place::Project(".opencode/agents"),
        ],
        agent_flag: None,
        builtins: &[
            "sessions",
            "resume",
            "continue",
            "new",
            "clear",
            "open",
            "projects",
            "project",
            "models",
            "agents",
            "mcps",
            "variants",
            "thinking",
            "effort",
            "connect",
            "settings",
            "status",
            "update",
            "pair",
            "web",
            "restart",
            "reload",
            "debug",
            "themes",
            "help",
            "exit",
            "quit",
            "q",
            "share",
            "rename",
            "timeline",
            "fork",
            "compact",
            "unshare",
            "undo",
            "redo",
            "copy",
            "export",
            "cd",
            "editor",
            "skills",
            "worktrees",
            "btw",
            "diff",
            "stats",
            "plugins",
            "init",
            "review",
        ],
        sigil: '/',
        skill_prefix: "",
    }),
    // The task goes on `--prompt=`, since the root command's only positional
    // is a directory (cli/src/commands/commands.ts:43-46).
    ends_options: None,
    // The v2 server parses no `@path` in text (core/src/session/prompt.ts:45-83);
    // `@` opens a popup instead, see `popups`.
    attaches_at: false,
    // Esc cancels the turn and leaves nothing amx sent in the composer.
    restores_queued_on_cancel: false,
    // One `--prompt=<text>` word, so a task starting with `-` is not read as
    // a flag.
    prompt_flag: Some("--prompt"),
    // `submit` is refused while the autocomplete popup is open
    // (tui/src/component/prompt/index.tsx:1112), and `@` or `/` at the start
    // of a word opens it.
    popups: &['@', '/'],
    // The first Escape only arms the cancel.
    cancel_presses: 2,
    // The plugin interrupts its session on SIGUSR2, which ends a turn even
    // while a permission card is open.
    interrupt_signal: Some(nix::sys::signal::Signal::SIGUSR2),
    // Each opencode gets its own server. Otherwise the TUI joins the shared
    // service, which runs with the caller's env, and a killed pane's turn
    // keeps running (cli/src/services/standalone.ts:22-24).
    launch: &["--standalone"],
};

/// opencode's plugin events and the words its payloads use.
///
/// The TUI loads `plugins/<dir>/tui.*` under its config dir with no
/// registration (plugin/src/host.ts:17-44), and `OPENCODE_CONFIG_DIR` moves
/// that dir (util/src/global.ts:79). The plugin is amx's own file, runs in the
/// pane and reads what `amx _hook` prints.
///
/// The event names are the plugin's. Five are opencode's own:
/// `session.execution.started`, `session.inbox.delivered` (a message queued
/// during a turn), `session.tool.called`, `permission.asked` and
/// `form.created` (sent only for a form of kind `question`). Three are amx's:
/// `session.selected` for the first session route, `permission.rejected` for
/// a `reject` reply or a cancelled form, and `session.execution.ended` for a
/// turn that succeeded, failed or was interrupted.
pub const HOOKS: Hooks = Hooks {
    wire: Wire::Placed {
        dir_env: "OPENCODE_CONFIG_DIR",
        dir: ".config/opencode",
        path: "plugins/amx/tui.js",
        body: include_str!("../../assets/opencode/tui.js"),
    },
    opt_in: &[],
    events: &[
        Wiring::new(Moment::Started, "session.selected"),
        Wiring::new(Moment::Prompted, "session.execution.started"),
        Wiring::new(Moment::Taken, "session.inbox.delivered"),
        Wiring::new(Moment::Calling, "session.tool.called"),
        Wiring::new(Moment::Asked, "permission.asked"),
        Wiring::new(Moment::Refused, "permission.rejected"),
        Wiring::new(Moment::Notified, "form.created"),
        Wiring::new(Moment::Ended, "session.execution.ended"),
    ],
    // A plugin takes no matcher, opencode sends no notices of its own, payloads
    // carry no text from the permission card, and opencode starts no turns of
    // its own.
    matcher: "",
    idle_notice: "",
    permission_notice: "",
    permission_sentence: "",
    injected: &[],
    // The tool that opens a question form (core/src/tool/plugin/question.ts:10).
    question_tool: "question",
    // The plugin's Started carries `startup` for a new session and `resume`
    // for one the pane was opened on.
    fresh_start: Some("startup"),
    // `form.created` is only sent for `metadata.kind` `question`, and carries
    // it as `kind`.
    question_kinds: &["question"],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_is_fourth_in_the_table() {
        assert_eq!(super::super::find("opencode --standalone"), Some(&VENDOR));
        assert_eq!(super::super::table()[3], VENDOR);
    }

    #[test]
    fn opencode_places_its_plugin_where_the_tui_loads_one() {
        let Wire::Placed {
            dir_env,
            dir,
            path,
            body,
        } = HOOKS.wire
        else {
            panic!("opencode is wired through a file placed in its config dir");
        };
        assert_eq!(
            (dir_env, dir, path),
            (
                "OPENCODE_CONFIG_DIR",
                ".config/opencode",
                "plugins/amx/tui.js"
            ),
            "where the TUI loads a plugin"
        );
        assert!(HOOKS.wire.listens(), "the plugin reads the hook's answer");
        assert!(body.starts_with("// installed by amx\n"), "{body}");
        assert!(body.contains("\"_hook\""), "it reports through amx");
        for wiring in HOOKS.events {
            assert!(
                body.contains(&format!("\"{}\"", wiring.event)),
                "the plugin never reports {}",
                wiring.event
            );
        }
    }

    #[test]
    fn opencode_names_all_eight_moments_in_the_plugins_words() {
        assert_eq!(
            HOOKS
                .events
                .iter()
                .map(|w| (w.event, w.moment, w.matched))
                .collect::<Vec<_>>(),
            [
                ("session.selected", Moment::Started, false),
                ("session.execution.started", Moment::Prompted, false),
                ("session.inbox.delivered", Moment::Taken, false),
                ("session.tool.called", Moment::Calling, false),
                ("permission.asked", Moment::Asked, false),
                ("permission.rejected", Moment::Refused, false),
                ("form.created", Moment::Notified, false),
                ("session.execution.ended", Moment::Ended, false),
            ],
            "the plugin's event names"
        );
        assert_eq!(HOOKS.question_tool, "question");
        assert_eq!(HOOKS.fresh_start, Some("startup"));
        assert_eq!(HOOKS.question_kinds, ["question"]);
        assert_eq!(HOOKS.permission_sentence("bash"), None);
        assert!(HOOKS.opt_in.is_empty() && HOOKS.injected.is_empty());
        assert_eq!(
            (HOOKS.matcher, HOOKS.idle_notice, HOOKS.permission_notice),
            ("", "", "")
        );
    }

    #[test]
    fn opencode_carries_its_model_in_the_env_and_auto_as_a_bare_flag() {
        let model = VENDOR.model.expect("opencode has a model dial");
        assert_eq!(
            (model.cycle, model.open, model.flag, model.key, model.bare),
            (&["default"][..], true, "", Some("model"), false)
        );
        assert_eq!(
            model.env,
            Some("OPENCODE_CONFIG_CONTENT"),
            "the TUI takes no model flag"
        );
        assert_eq!(VENDOR.models, Models::Cycle);

        let permission = VENDOR.permission.expect("opencode has --auto");
        assert_eq!(
            (
                permission.cycle,
                permission.open,
                permission.flag,
                permission.key,
                permission.bare,
                permission.env
            ),
            (&["default", "auto"][..], false, "--auto", None, true, None)
        );
        assert_eq!(
            VENDOR.effort, None,
            "the stored per-model variant wins over one on the model"
        );
    }

    #[test]
    fn opencode_resumes_by_a_split_session_flag_and_never_forks() {
        let session = VENDOR
            .session
            .expect("opencode declares a session vocabulary");
        assert_eq!(session.start, None);
        assert_eq!(
            session.resume,
            Resume::Flag {
                flag: "--session",
                joined: false
            }
        );
        assert_eq!(session.conflicts, ["-s", "-c", "--continue", "--server"]);
        assert_eq!(session.fork, None);
        assert_eq!(session.resume_args("ses_1"), ["--session", "ses_1"]);
        assert_eq!(
            VENDOR.launch,
            ["--standalone"],
            "each opencode runs its own server"
        );
    }

    #[test]
    fn opencode_can_resume_and_report_and_nothing_else() {
        for can in [
            Capability::Hooks,
            Capability::Transcript,
            Capability::Resume,
        ] {
            assert!(VENDOR.can(can), "{can:?}");
        }
        for cannot in [Capability::Fork, Capability::Adopt, Capability::Trust] {
            assert!(!VENDOR.can(cannot), "{cannot:?}");
        }
        assert_eq!(VENDOR.hooks, Some(HOOKS));
        assert_eq!(VENDOR.transcript, Some(Transcript::Opencode));
        assert_eq!(VENDOR.session_env, None);
        assert_eq!(
            VENDOR.not_inherited,
            [
                "OPENCODE_TERMINAL",
                "OPENCODE_ROUTE",
                "OPENCODE_STORY",
                "OPENCODE_PTY_HANDOFF",
                "OPENCODE_PASSWORD",
                "OPENCODE_SERVER_PASSWORD",
                "OPENCODE_CLIENT",
            ]
        );
    }

    #[test]
    fn opencode_prompts_on_a_flag_and_cuts_a_turn_in_two_presses() {
        assert_eq!(
            VENDOR.prompt_flag,
            Some("--prompt"),
            "a task is one --prompt=<text> word"
        );
        assert_eq!(
            VENDOR.popups,
            ['@', '/'],
            "`@` and `/` open the autocomplete popup"
        );
        assert_eq!(
            VENDOR.cancel_presses, 2,
            "the first Escape only arms the cancel"
        );
        assert_eq!(
            VENDOR.interrupt_signal,
            Some(nix::sys::signal::Signal::SIGUSR2),
            "the plugin interrupts its session on SIGUSR2"
        );
        assert_eq!(VENDOR.ends_options, None);
        assert_eq!(
            (VENDOR.attaches_at, VENDOR.restores_queued_on_cancel),
            (false, false)
        );
    }

    #[test]
    fn opencode_runs_a_command_as_slash_name_and_offers_no_skill() {
        let catalog = VENDOR.catalog.expect("opencode's layout is measured");
        assert_eq!((catalog.sigil, catalog.skill_prefix), ('/', ""));
        assert!(catalog.skills.is_empty());
        assert_eq!(
            catalog.commands,
            [
                Place::Person(".config/opencode/commands"),
                Place::Person(".config/opencode/command"),
                Place::Project(".opencode/commands"),
                Place::Project(".opencode/command"),
            ]
        );
        assert_eq!(
            catalog.agents,
            [
                Place::Person(".config/opencode/agents"),
                Place::Project(".opencode/agents"),
            ]
        );
        assert_eq!(catalog.agent_flag, None);
        assert_eq!(catalog.builtins.len(), 48);
        for builtin in ["sessions", "exit", "q", "compact", "review", "init"] {
            assert!(catalog.builtins.contains(&builtin), "{builtin}");
        }
    }

    #[test]
    fn opencode_screens_parse() {
        let screens =
            crate::rules::Ruleset::parse(VENDOR.screens.expect("opencode declares screens"))
                .expect("opencode's screens parse");
        assert!(!screens.rules().is_empty());
    }
}
