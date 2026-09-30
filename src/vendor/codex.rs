//! codex's entry in the vendor table.
//!
//! Every value is codex's own spelling, checked against codex-cli 0.157.1's
//! `--help` and source (tag rust-v0.157.1; paths below are relative to its
//! `codex-rs/`). Re-check on every vendor bump: a renamed flag turns a dial
//! into a spawn that fails.

use super::{
    Capability, Catalog, DEFAULT, DialSpec, ForkSpec, Hooks, Models, Moment, Place, Resume,
    SessionSpec, Transcript, Vendor, Wire, Wiring,
};

/// codex's entry in the table.
pub const VENDOR: Vendor = Vendor {
    name: "codex",
    // Open: `-m, --model <MODEL>` is a free string
    // (utils/cli/src/shared_options.rs:22-23) and codex names no aliases.
    model: Some(DialSpec::open("--model", &[DEFAULT])),
    // `codex debug models` prints the account's catalog as JSON
    // (cli/src/main.rs:258, 2068-2095): `models[]` with a `slug` and a
    // `visibility`; the picker offers those whose visibility is `list`.
    models: Models::Json(&["debug", "models"]),
    // Closed: `-s, --sandbox <SANDBOX_MODE>` takes exactly these
    // (utils/cli/src/sandbox_mode_cli_arg.rs:12-17). The approval flag `-a` is
    // the other half of codex's permissions and is left to the agent command.
    permission: Some(DialSpec::closed(
        "--sandbox",
        &[
            DEFAULT,
            "read-only",
            "workspace-write",
            "danger-full-access",
        ],
    )),
    // No effort flag, only `-c model_reasoning_effort=<v>`
    // (config/src/config_toml.rs:392). Closed over the levels the catalog's
    // models support between them (protocol/src/openai_models.rs:59-89); each
    // model's `supported_reasoning_levels` is a subset, and codex refuses a
    // level the model lacks.
    effort: Some(DialSpec {
        key: Some("model_reasoning_effort"),
        ..DialSpec::closed("-c", &[DEFAULT, "low", "medium", "high", "xhigh", "max"])
    }),
    // codex mints its own ids and has no flag to take one (tui/src/cli.rs:11-88),
    // so SessionStart names the session at the first turn. `codex resume <id>`
    // (cli/src/main.rs:350-373) and `codex fork <id>` (cli/src/main.rs:412-431)
    // are subcommands right after the program. `--last`, `--all` and
    // `--include-non-interactive` also pick a session for both.
    session: Some(SessionSpec {
        start: None,
        resume: Resume::Subcommand("resume"),
        conflicts: &["--last", "--all", "--include-non-interactive"],
        fork: Some(ForkSpec::Subcommand("fork")),
    }),
    // Set on every command the shell tool runs to the root session's id
    // (protocol/src/shell_environment.rs:6, core/src/exec_env.rs:40-50), which
    // equals the SessionStart payload's `session_id`.
    session_env: Some("CODEX_SESSION_ID"),
    // What codex sets for every process it starts: session, thread and
    // version (core/src/exec_env.rs:16, protocol/src/shell_environment.rs:6-7),
    // permission profile (core/src/exec_env.rs:20), sandbox markers
    // (core/src/spawn.rs:21, 26), proxy marker (network-proxy/src/proxy.rs:626),
    // apply-patch line endings (apply-patch/src/lib.rs:59) and the plugin
    // metrics sidecar (core-plugins/src/plugin_metrics_sidecar.rs:26).
    // `CODEX_EXEC_SERVER_URL` would put an inheriting codex on another
    // machine's executor (tui/src/daemon_startup.rs:37-38).
    not_inherited: &[
        "CODEX_SESSION_ID",
        "CODEX_THREAD_ID",
        "CODEX_VERSION",
        "CODEX_PERMISSION_PROFILE",
        "CODEX_SANDBOX",
        "CODEX_SANDBOX_NETWORK_DISABLED",
        "CODEX_NETWORK_PROXY_ACTIVE",
        "CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS",
        "CODEX_PLUGIN_METRICS_OUTPUT",
        "CODEX_EXEC_SERVER_URL",
    ],
    // Hook reports name the rollout file, which gives Transcript. No Trust:
    // codex keys a folder's trust on the main checkout's root
    // (config/src/state.rs:233-243), so trusting an amx worktree would trust
    // the person's whole repository.
    capabilities: &[
        Capability::Hooks,
        Capability::Transcript,
        Capability::Resume,
        Capability::Fork,
        Capability::Adopt,
    ],
    hooks: Some(HOOKS),
    // Read off live panes under `--no-daemon` at four widths.
    screens: Some(include_str!("../../assets/screen-rules-codex.toml")),
    // The rollout under `$CODEX_HOME/sessions/`, named on every hook payload
    // as `transcript_path`. Samples are in tests/codex/rollouts.
    transcript: Some(Transcript::Codex),
    // A skill runs as a bare `$name` anywhere in a message
    // (skills/src/mentions.rs:41); `/name` runs no skill. The skill places are
    // the person's and the project's roots from ext/skills/src/host_roots.rs
    // (:86-108, 137-185), with `$CODEX_HOME/skills` taken as `~/.codex/skills`.
    // codex reads the project places in every directory from the project root
    // down; amx reads them only where the agent runs. No commands (custom
    // prompts are gone), no agents (roles have no CLI flag) and no built-ins
    // (a slash command as the first prompt is sent as text).
    //
    // codex finds a SKILL.md up to depth 6 (ext/skills/src/loader/mod.rs:31)
    // and names it by its frontmatter `name`. `crate::catalog` reads one level
    // deep and names a skill by its directory, so deeper or renamed skills are
    // not offered.
    catalog: Some(Catalog {
        skills: &[
            Place::Person(".agents/skills"),
            Place::Person(".codex/skills"),
            Place::Project(".codex/skills"),
            Place::Project(".agents/skills"),
        ],
        commands: &[],
        agents: &[],
        agent_flag: None,
        builtins: &[],
        sigil: '$',
        skill_prefix: "",
    }),
    // clap reads a message word starting with `-` as a flag, and a
    // subcommand's name as that subcommand, until `--` (cli/src/main.rs:125-140):
    // `codex --no-daemon -- resume` sends the prompt `resume`.
    ends_options: Some("--"),
    // The argv prompt is sent as is, with no `@` expansion
    // (tui/src/chatwidget/user_messages.rs:196-221).
    attaches_at: false,
    // Esc restores Tab-queued messages, but amx never queues with Tab. amx
    // sends steers (bracketed paste and Enter, which submit mid-turn), and a
    // steer still pending at Esc is resubmitted as a new turn rather than
    // restored (tui/src/chatwidget/input_restore.rs:316-378).
    restores_queued_on_cancel: false,
    // The approval box marks `y` on its first row, and `y` approves.
    menus_take_letters: true,
    prompt_flag: None,
    popups: &[],
    cancel_presses: 1,
    interrupt_signal: None,
    // Each codex gets its own app server. Under the shared daemon hooks run in
    // the daemon's environment, so `AMX_ID` never reaches `amx _hook`, and a
    // killed pane's turn keeps running (tui/src/cli.rs:85,
    // hooks/src/engine/command_runner.rs:426-432).
    launch: &["--no-daemon"],
};

/// codex's hook events and the words its payloads use.
///
/// codex reads `hooks.json` beside `config.toml` in `$CODEX_HOME`
/// (hooks/src/engine/discovery.rs:339-380) and runs each handler with the
/// payload on stdin. amx merges its groups into that file and trusts each in
/// the config. A handler's stdout is fed to the model
/// (hooks/src/events/user_prompt_submit.rs:157-225), so this wire never
/// listens.
///
/// Four of codex's twelve events (hooks/src/lib.rs:23-36); samples are in
/// tests/codex/hooks. No `Taken`: a steered message is a second
/// `UserPromptSubmit` with the same `turn_id` (core/src/session/turn.rs:427-445),
/// read as `Prompted`, which leaves a working record working. The approval box
/// is read off the screen, because `PermissionRequest` fires before any box
/// and may be resolved without one. An interrupted or failed turn sends no
/// `Stop`; the pane reader closes it.
pub const HOOKS: Hooks = Hooks {
    wire: Wire::Hooks {
        dir_env: "CODEX_HOME",
        dir: ".codex",
        body: include_str!("../../assets/codex/hooks.json"),
    },
    opt_in: &[],
    events: &[
        Wiring::new(Moment::Started, "SessionStart"),
        Wiring::new(Moment::Prompted, "UserPromptSubmit"),
        Wiring::new(Moment::Calling, "PreToolUse"),
        Wiring::new(Moment::Ended, "Stop"),
    ],
    // A group without a matcher takes every tool (hooks/src/lib.rs:38-53).
    // codex has no notice hook, no permission sentence in payloads and no
    // turns of its own.
    matcher: "",
    idle_notice: "",
    permission_notice: "",
    permission_sentence: "",
    injected: &[],
    // Arrives as a PreToolUse with `tool_input.questions[]`
    // (core/src/tools/registry.rs:130-139).
    question_tool: "request_user_input",
    // `source` is `startup`, `resume`, `clear`, `compact` or `fork`
    // (hooks/src/schema.rs:854).
    fresh_start: Some("startup"),
    // Notices are not hooks, so no payload says what a question is.
    question_kinds: &[],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_is_wired_through_its_own_hooks_file() {
        let Wire::Hooks { dir_env, dir, body } = HOOKS.wire else {
            panic!("codex is wired through a hooks file it shares");
        };
        assert_eq!(dir_env, "CODEX_HOME");
        assert_eq!(dir, ".codex");
        assert!(
            !HOOKS.wire.listens(),
            "codex feeds a hook's stdout to the model"
        );
        let file: serde_json::Value = serde_json::from_str(body).expect("the body is JSON");
        let groups = file["hooks"].as_object().expect("a hooks object");
        assert_eq!(
            groups.keys().map(String::as_str).collect::<Vec<_>>(),
            ["PreToolUse", "SessionStart", "Stop", "UserPromptSubmit"]
        );
        for wiring in HOOKS.events {
            let group = &file["hooks"][wiring.event][0];
            assert_eq!(
                group["hooks"][0]["command"], "amx _hook",
                "{}",
                wiring.event
            );
            assert!(group.get("matcher").is_none(), "{}", wiring.event);
        }
    }

    #[test]
    fn codex_names_four_moments_and_hears_a_steer_as_a_prompt() {
        assert_eq!(
            HOOKS
                .events
                .iter()
                .map(|w| (w.event, w.moment, w.matched))
                .collect::<Vec<_>>(),
            [
                ("SessionStart", Moment::Started, false),
                ("UserPromptSubmit", Moment::Prompted, false),
                ("PreToolUse", Moment::Calling, false),
                ("Stop", Moment::Ended, false),
            ]
        );
        assert_eq!(
            HOOKS.moment("PermissionRequest"),
            None,
            "the approval box is read off the screen"
        );
        assert_eq!(
            HOOKS.moment("Interrupt"),
            None,
            "the pane reader closes an interrupted turn"
        );
        assert_eq!(HOOKS.question_tool, "request_user_input");
        assert_eq!(HOOKS.fresh_start, Some("startup"));
        assert_eq!(HOOKS.permission_sentence("Bash"), None);
        assert!(HOOKS.opt_in.is_empty() && HOOKS.injected.is_empty());
        assert!(HOOKS.question_kinds.is_empty());
        assert_eq!(
            (HOOKS.matcher, HOOKS.idle_notice, HOOKS.permission_notice),
            ("", "", "")
        );
    }

    #[test]
    fn codex_declares_a_model_a_sandbox_and_a_keyed_effort_dial() {
        let model = VENDOR.model.expect("codex has a model dial");
        assert_eq!(
            (model.flag, model.cycle, model.open, model.key),
            ("--model", &["default"][..], true, None)
        );
        assert_eq!(VENDOR.models, Models::Json(&["debug", "models"]));

        let permission = VENDOR.permission.expect("codex has a sandbox dial");
        assert_eq!(
            (
                permission.flag,
                permission.cycle,
                permission.open,
                permission.key
            ),
            (
                "--sandbox",
                &[
                    "default",
                    "read-only",
                    "workspace-write",
                    "danger-full-access"
                ][..],
                false,
                None
            )
        );

        let effort = VENDOR.effort.expect("codex has an effort setting");
        assert_eq!(
            (effort.flag, effort.cycle, effort.open, effort.key),
            (
                "-c",
                &["default", "low", "medium", "high", "xhigh", "max"][..],
                false,
                Some("model_reasoning_effort")
            )
        );
    }

    #[test]
    fn codex_resumes_and_forks_by_subcommand_and_starts_under_no_id_of_amxs() {
        let session = VENDOR.session.expect("codex declares a session vocabulary");
        assert_eq!(session.start, None);
        assert_eq!(session.resume, Resume::Subcommand("resume"));
        assert_eq!(session.fork, Some(ForkSpec::Subcommand("fork")));
        assert_eq!(
            session.conflicts,
            ["--last", "--all", "--include-non-interactive"]
        );
        assert_eq!(
            VENDOR.launch,
            ["--no-daemon"],
            "each codex runs its own app server"
        );
    }

    #[test]
    fn codex_keeps_the_variables_it_hands_its_children_to_itself() {
        assert_eq!(VENDOR.session_env, Some("CODEX_SESSION_ID"));
        assert_eq!(
            VENDOR.not_inherited,
            [
                "CODEX_SESSION_ID",
                "CODEX_THREAD_ID",
                "CODEX_VERSION",
                "CODEX_PERMISSION_PROFILE",
                "CODEX_SANDBOX",
                "CODEX_SANDBOX_NETWORK_DISABLED",
                "CODEX_NETWORK_PROXY_ACTIVE",
                "CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS",
                "CODEX_PLUGIN_METRICS_OUTPUT",
                "CODEX_EXEC_SERVER_URL",
            ]
        );
    }

    #[test]
    fn codex_can_do_all_but_have_its_trust_screen_answered() {
        for can in [
            Capability::Hooks,
            Capability::Transcript,
            Capability::Resume,
            Capability::Fork,
            Capability::Adopt,
        ] {
            assert!(VENDOR.can(can), "{can:?}");
        }
        assert!(
            !VENDOR.can(Capability::Trust),
            "codex keys trust on the main checkout"
        );
        assert_eq!(VENDOR.hooks, Some(HOOKS));
        assert_eq!(VENDOR.transcript, Some(Transcript::Codex));
    }

    #[test]
    fn codex_runs_a_skill_as_dollar_name_out_of_four_places() {
        assert_eq!(
            VENDOR.catalog,
            Some(Catalog {
                skills: &[
                    Place::Person(".agents/skills"),
                    Place::Person(".codex/skills"),
                    Place::Project(".codex/skills"),
                    Place::Project(".agents/skills"),
                ],
                commands: &[],
                agents: &[],
                agent_flag: None,
                builtins: &[],
                sigil: '$',
                skill_prefix: "",
            }),
            "`$` opens a codex word, and a bare name follows it"
        );
    }

    #[test]
    fn codex_reads_a_message_after_the_options_and_restores_no_steer() {
        assert_eq!(VENDOR.ends_options, Some("--"));
        assert_eq!(
            (VENDOR.attaches_at, VENDOR.restores_queued_on_cancel),
            (false, false)
        );
    }

    #[test]
    fn codex_screens_parse() {
        let screens = crate::rules::Ruleset::parse(VENDOR.screens.expect("codex declares screens"))
            .expect("codex's screens parse");
        assert!(!screens.rules().is_empty());
    }
}
