//! opencode, the fourth vendor amx knows anything about.
//!
//! Everything here is the vendor's own words, measured against opencode
//! 2.0.16 and the source it was built from (tag v2.0.16, paths below relative
//! to its `packages/`), each value on the date it carries. Re-measure at every
//! vendor bump: these are not amx's names to choose, and a renamed flag turns
//! a dial into a spawn that fails.

use super::{
    Capability, Catalog, DEFAULT, DialSpec, Hooks, Models, Moment, Place, Resume, SessionSpec,
    Transcript, Vendor, Wire, Wiring,
};

/// opencode's entry in the table.
pub const VENDOR: Vendor = Vendor {
    name: "opencode",
    // The TUI takes no model flag: the root command's parameters are
    // standalone, server, auto, directory, continue, session and prompt
    // (cli/src/commands/commands.ts:26-58). So the model goes into the pane's
    // env as `OPENCODE_CONFIG_CONTENT`, which is loaded last, over every
    // config file (core/src/config.ts:230), on `new` only: a resumed session
    // keeps its own model (tui/src/context/local.tsx:252-260). Open, since a
    // model is `provider/model` and opencode names no aliases. Measured at
    // 2.0.16 on 2026-09-30 (plan opencode-lands Ruling 8).
    model: Some(DialSpec {
        key: Some("model"),
        env: Some("OPENCODE_CONFIG_CONTENT"),
        ..DialSpec::open("", &[DEFAULT])
    }),
    // `opencode models` is a subcommand that starts the shared service with
    // the caller's env (Ruling 1), so amx never runs it: the cycle is the
    // whole of what it offers.
    models: Models::Cycle,
    // `--auto` auto-approves every permission not explicitly denied
    // (cli/src/commands/commands.ts:27-30), a flag with no value. Measured at
    // 2.0.16 on 2026-09-30.
    permission: Some(DialSpec {
        bare: true,
        ..DialSpec::closed("--auto", &[DEFAULT, "auto"])
    }),
    // A `#variant` on the model loses to opencode's stored per-model variant
    // (tui/src/context/local.tsx:236), so there is no effort to turn (Ruling
    // 8).
    effort: None,
    // opencode mints its own ids and the TUI takes none to start under, so the
    // plugin's Started names the session. `-s, --session <id>` opens one that
    // exists (tui/src/app.tsx:655-661), as two words; `-c, --continue` opens
    // the newest in the cwd (app.tsx:664-690), and `--server` picks the
    // process whose sessions these are. The TUI has no fork flag
    // (cli/src/commands/handlers/default.ts:82-87). Measured at 2.0.16 on
    // 2026-09-30.
    session: Some(SessionSpec {
        start: None,
        resume: Resume::Flag {
            flag: "--session",
            joined: false,
        },
        conflicts: &["-s", "-c", "--continue", "--server"],
        fork: None,
    }),
    // No variable names the session in what opencode starts: its shell tool
    // adds only `TERM` and `OPENCODE_TERMINAL` (core/src/shell.ts:264-268).
    session_env: None,
    // What a pane typed inside opencode would carry into one amx starts, read
    // at 2.0.16 on 2026-09-30: the shell and PTY marker (core/src/shell.ts:
    // 264-268, core/src/pty.ts:173), the initial route and storybook the TUI
    // opens on (tui/src/app.tsx:343-356), the PTY handoff
    // (cli/src/server-process.ts:46-47), the server passwords
    // (cli/src/env.ts:10-13) and the app's name (default.ts:69).
    not_inherited: &[
        "OPENCODE_TERMINAL",
        "OPENCODE_ROUTE",
        "OPENCODE_STORY",
        "OPENCODE_PTY_HANDOFF",
        "OPENCODE_PASSWORD",
        "OPENCODE_SERVER_PASSWORD",
        "OPENCODE_CLIENT",
    ],
    // opencode reports through the TUI plugin `HOOKS` below places, and names
    // the message list it writes at each turn's end. No Fork (the TUI has no
    // fork flag), no Adopt (no session variable in children) and no Trust (no
    // trust screen): Ruling 9.
    capabilities: &[
        Capability::Hooks,
        Capability::Transcript,
        Capability::Resume,
    ],
    hooks: Some(HOOKS),
    // Driven live against 2.0.16 on 2026-09-30 under `--standalone`, at four
    // widths (docs/opencode-screens.md).
    screens: Some(include_str!("../../assets/screen-rules-opencode.toml")),
    // The message list the plugin writes to `$AMX_DIR/opencode-messages.jsonl`
    // and names as `transcript_path` (Ruling 5).
    transcript: Some(Transcript::Opencode),
    // A command runs as `/name` (tui/src/component/prompt/index.tsx:1141-1144),
    // named by its path under `{command,commands}/`
    // (core/src/config/plugin/command.ts:144,159), loaded from the config dir
    // and the project's `.opencode/` (core/src/config.ts:185-191,221-236).
    // Agents come from `{agent,agents}/` in the same two
    // (core/src/config/plugin/agent.ts:21-24), and the TUI has no flag to run
    // as one. A skill is an autocomplete part with no text spelling
    // (tui/src/component/prompt/autocomplete.tsx:406-440), so no skills. The
    // built-ins are the app's (tui/src/app.tsx:720-1112), the session's
    // (routes/session/index.tsx:871-1174), the prompt's, the feature
    // plugins' and the server's (core/src/plugin/command.ts:29-53), aliases
    // and all. Read at 2.0.16 on 2026-09-30.
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
    // The task rides on `--prompt=`, never as a bare word: the root command's
    // only positional is a directory (cli/src/commands/commands.ts:43-46).
    ends_options: None,
    // The v2 server parses no `@path` in text (core/src/session/prompt.ts:
    // 45-83); `@` opens a popup instead, which `popups` answers.
    attaches_at: false,
    // Esc cancels the turn and leaves nothing amx sent in the composer
    // (docs/opencode-screens.md, "Idle, Esc and error").
    restores_queued_on_cancel: false,
    // One `--prompt=<text>` word (Ruling 2): a task that opens with `-` would
    // otherwise be read as a flag.
    prompt_flag: Some("--prompt"),
    // `submit` refuses while the autocomplete popup is up
    // (tui/src/component/prompt/index.tsx:1112), and `@` and `/` open it at
    // the start of a word. Measured on `@README` on 2026-09-30 (Ruling 7).
    popups: &['@', '/'],
    // The first Escape only arms the cancel (Ruling 6, measured 2026-09-30).
    cancel_presses: 2,
    // The plugin hears SIGUSR2 and interrupts its session, which ends a turn
    // even under a permission card (Ruling 6, measured 2026-09-30).
    interrupt_signal: Some(nix::sys::signal::Signal::SIGUSR2),
    // Every opencode amx starts runs its own server. Without it the TUI joins
    // the shared service, started with the caller's env, and a killed pane's
    // turn runs on (cli/src/services/standalone.ts:22-24; Ruling 1).
    launch: &["--standalone"],
};

/// How opencode reports what it is doing, and where amx asks it to.
///
/// opencode's TUI loads `plugins/<dir>/tui.*` under its config dir with no
/// registration (plugin/src/host.ts:17-44, loaded live on 2026-09-29), and
/// `OPENCODE_CONFIG_DIR` replaces that dir (util/src/global.ts:79). The file
/// is amx's own, runs in the pane, and hears what `amx _hook` answers (Ruling
/// 3).
///
/// Every name is the plugin's, which is amx's to spell (Ruling 4). Four are
/// opencode's own events: `session.execution.started`,
/// `session.inbox.delivered` for a message enqueued while running,
/// `session.tool.called` and `permission.asked`. `form.created` is opencode's
/// too, sent only for a form of kind `question`. Three are coined:
/// `session.selected` for the first session route, `permission.rejected` for
/// a `reject` reply or a cancelled form, and `session.execution.ended` over
/// succeeded, failed and interrupted. Re-measure at every vendor bump: a
/// renamed event is a moment amx never hears.
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
    // None of these: a plugin takes no matcher, opencode sends no notice of
    // its own, and a payload carries no sentence off the permission card.
    // Nothing it runs as a turn is a prompt somebody did not type.
    matcher: "",
    idle_notice: "",
    permission_notice: "",
    permission_sentence: "",
    injected: &[],
    // The tool that opens a question form (core/src/tool/plugin/question.ts:
    // 10), measured on 2.0.16 on 2026-09-30.
    question_tool: "question",
    // The plugin's Started carries `startup` for a new session and `resume`
    // for one the pane was opened onto (Ruling 4).
    fresh_start: Some("startup"),
    // `form.created` is sent for a form of `metadata.kind` `question` alone,
    // and says so as its `kind`.
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
            "Ruling 3"
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
            "Ruling 4"
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
        assert_eq!(model.env, Some("OPENCODE_CONFIG_CONTENT"), "Ruling 8");
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
        assert_eq!(VENDOR.effort, None, "Ruling 8");
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
        assert_eq!(VENDOR.launch, ["--standalone"], "Ruling 1");
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
            assert!(!VENDOR.can(cannot), "Ruling 9: {cannot:?}");
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
        assert_eq!(VENDOR.prompt_flag, Some("--prompt"), "Ruling 2");
        assert_eq!(VENDOR.popups, ['@', '/'], "Ruling 7");
        assert_eq!(VENDOR.cancel_presses, 2, "Ruling 6");
        assert_eq!(
            VENDOR.interrupt_signal,
            Some(nix::sys::signal::Signal::SIGUSR2),
            "Ruling 6"
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
