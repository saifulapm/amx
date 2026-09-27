//! codex, the third vendor amx knows anything about.
//!
//! Everything here is the vendor's own words, measured against codex-cli
//! 0.157.1's `--help` and the source it was built from (tag rust-v0.157.1,
//! paths below relative to its `codex-rs/`), each value on the date it
//! carries. Re-measure at every vendor bump: these are not amx's names to
//! choose, and a renamed flag turns a dial into a spawn that fails.

use super::{
    Capability, DEFAULT, DialSpec, ForkSpec, Hooks, Models, Moment, Resume, SessionSpec,
    Transcript, Vendor, Wire, Wiring,
};

/// codex's entry in the table.
pub const VENDOR: Vendor = Vendor {
    name: "codex",
    // Open: `-m, --model <MODEL>` is a free string
    // (utils/cli/src/shared_options.rs:22-23), and codex documents no aliases
    // of its own, so the cycle offers nothing beyond the sentinel. Measured at
    // 0.157.1 on 2026-09-28.
    model: Some(DialSpec {
        cycle: &[DEFAULT],
        open: true,
        flag: "--model",
        key: None,
    }),
    // `codex debug models` prints the account's catalog as JSON
    // (cli/src/main.rs:258, 2068-2095): `models[]`, each with a `slug` and a
    // `visibility`, and the picker offers the ones whose visibility is
    // `list`. Measured at 0.157.1 on 2026-09-28.
    models: Models::Json(&["debug", "models"]),
    // Closed: `-s, --sandbox <SANDBOX_MODE>` names exactly these three in
    // `--help` (utils/cli/src/sandbox_mode_cli_arg.rs:12-17), and clap refuses
    // any other. The approval flag `-a` is the other half of codex's
    // permissions and is left to the agent command: amx has one dial, and the
    // sandbox is the half every run is under. Measured at 0.157.1 on
    // 2026-09-28.
    permission: Some(DialSpec {
        cycle: &[
            DEFAULT,
            "read-only",
            "workspace-write",
            "danger-full-access",
        ],
        open: false,
        flag: "--sandbox",
        key: None,
    }),
    // codex has no effort flag, only the setting
    // `-c model_reasoning_effort=<v>` (config/src/config_toml.rs:392). Closed
    // over the levels the catalog's models support between them
    // (protocol/src/openai_models.rs:59-89), and knowingly partial: which of
    // them a model takes is per model, in its `supported_reasoning_levels`,
    // and a level the model lacks is the vendor's to refuse. Measured at
    // 0.157.1 on 2026-09-28.
    effort: Some(DialSpec {
        cycle: &[DEFAULT, "low", "medium", "high", "xhigh", "max"],
        open: false,
        flag: "-c",
        key: Some("model_reasoning_effort"),
    }),
    // codex mints its own ids and takes none (tui/src/cli.rs:11-88 names no
    // session flag), so nothing starts a session under an id amx chose: the
    // SessionStart hook names it, at the first turn. `codex resume <id>`
    // carries one on (cli/src/main.rs:350-373) and `codex fork <id>` branches
    // one (cli/src/main.rs:412-431), each a subcommand right after the
    // program. `--last`, `--all` and `--include-non-interactive` are the
    // other words both subcommands pick a session by (`codex resume --help`,
    // `codex fork --help`). Measured at 0.157.1 on 2026-09-28.
    session: Some(SessionSpec {
        start: None,
        resume: Resume::Subcommand("resume"),
        conflicts: &["--last", "--all", "--include-non-interactive"],
        fork: Some(ForkSpec::Subcommand("fork")),
    }),
    // Handed to every command codex's shell tool runs
    // (protocol/src/shell_environment.rs:6, core/src/exec_env.rs:40-50): the
    // root session's id, which is the pane's. Measured on 0.157.1 on
    // 2026-09-28: it equalled the SessionStart payload's `session_id`
    // (docs/codex-screens.md, open question 2).
    session_env: Some("CODEX_SESSION_ID"),
    // What codex puts in the environment of every process it starts, read at
    // 0.157.1 on 2026-09-28: the session, thread and version
    // (core/src/exec_env.rs:16, protocol/src/shell_environment.rs:6-7), the
    // permission profile (core/src/exec_env.rs:20), the sandbox markers
    // (core/src/spawn.rs:21, 26), the proxy marker
    // (network-proxy/src/proxy.rs:626), apply-patch's line endings
    // (apply-patch/src/lib.rs:59) and the plugin metrics sidecar
    // (core-plugins/src/plugin_metrics_sidecar.rs:26). And
    // `CODEX_EXEC_SERVER_URL`, which puts a codex that inherits it on another
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
    // codex reports through the hooks file `HOOKS` below merges into, and a
    // report names the rollout it appends to, which `crate::conversation`
    // reads by the shape at the foot of this entry. No Trust: codex keys a
    // folder's trust on the main checkout's root (config/src/state.rs:233-243),
    // so trusting a worktree amx cut would trust the person's repository. Its
    // trust screen is theirs to answer. Measured at 0.157.1 on 2026-09-28.
    capabilities: &[
        Capability::Hooks,
        Capability::Transcript,
        Capability::Resume,
        Capability::Fork,
        Capability::Adopt,
    ],
    hooks: Some(HOOKS),
    // Driven live against 0.157.1 on 2026-09-28 under `--no-daemon`, at four
    // widths: every gate codex draws before a turn, a running turn, a box, a
    // question, the prompt and an Esc'd turn (docs/codex-screens.md).
    screens: Some(include_str!("../../assets/screen-rules-codex.toml")),
    // The rollout codex writes under `$CODEX_HOME/sessions/`, named on every
    // hook payload as `transcript_path`. Measured off 0.157.1's rollouts on
    // 2026-09-28 (tests/codex/rollouts).
    transcript: Some(Transcript::Codex),
    // Not measured in this entry's shape yet: codex runs a skill as `$name`
    // rather than behind a `/`, and a catalog that says so is plan codex-lands
    // t8's. Until then a line for codex offers nothing.
    catalog: None,
    // clap reads a message word opening with `-` as a flag, and one that is a
    // subcommand's name as that subcommand (cli/src/main.rs:125-140), until
    // `--`. Measured on 0.157.1 on 2026-09-28: `codex --no-daemon -- resume`
    // started a session with the prompt `resume`, and `-- -v` sent `-v`.
    ends_options: Some("--"),
    // The argv prompt is submitted as it stands, with no `@` expansion
    // (tui/src/chatwidget/user_messages.rs:196-221). Read at 0.157.1 on
    // 2026-09-28.
    attaches_at: false,
    // Esc puts Tab-queued messages back in the composer, unsent — measured on
    // 0.157.1 on 2026-09-28 — but amx never Tabs. Its sends are steers:
    // bracketed paste and Enter, measured to submit mid-turn on 0.157.1 on
    // 2026-09-28, and a steer still pending at Esc is resubmitted as a fresh
    // turn rather than restored (tui/src/chatwidget/input_restore.rs:316-378).
    // So nothing amx sent is ever waiting in the composer after a cancel.
    restores_queued_on_cancel: false,
    // Every codex amx starts runs its own app server. Under the shared daemon
    // a hook runs in the daemon's environment, so `AMX_ID` never reaches
    // `amx _hook`, and a killed pane's turn runs on (tui/src/cli.rs:85,
    // hooks/src/engine/command_runner.rs:426-432; plan codex-lands Ruling 1).
    // Measured on 0.157.1 on 2026-09-28: the hook's environment carried the
    // pane's `AMX_ID` under `--no-daemon`.
    launch: &["--no-daemon"],
};

/// How codex reports what it is doing, and where amx asks it to.
///
/// codex reads a `hooks.json` beside its `config.toml` in `$CODEX_HOME`
/// (hooks/src/engine/discovery.rs:339-380), and runs each handler with the
/// payload on stdin. amx merges its groups into that file and trusts each in
/// the config (plan codex-lands Rulings 2-4). What a handler prints on stdout
/// is fed to the model (hooks/src/events/user_prompt_submit.rs:157-225), which
/// is why a hooks wire never listens.
///
/// Four moments out of codex's twelve (hooks/src/lib.rs:23-36), measured on
/// 0.157.1 on 2026-09-28 (tests/codex/hooks). There is no Taken: a message
/// steered into a running turn is a second `UserPromptSubmit` under the same
/// `turn_id` (core/src/session/turn.rs:427-445), read as Prompted, which
/// leaves a working record working (Ruling 6). The approval box is read off
/// the screen rather than `PermissionRequest`, which fires before any box and
/// may be answered with none; an Esc'd or errored turn sends no `Stop` and is
/// closed by the pane reader too (Ruling 5). Re-measure at every vendor bump:
/// a renamed event is a moment amx never hears.
pub const HOOKS: Hooks = Hooks {
    wire: Wire::Hooks {
        dir_env: "CODEX_HOME",
        dir: ".codex",
        body: include_str!("../../assets/codex/hooks.json"),
    },
    opt_in: &[],
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
            matched: false,
        },
        Wiring {
            moment: Moment::Ended,
            event: "Stop",
            matched: false,
        },
    ],
    // None of these: a group with no matcher takes every tool
    // (hooks/src/lib.rs:38-53), codex sends no notice hook at all, and it
    // writes no permission sentence a payload carries. Nothing it runs as a
    // turn is a prompt somebody did not type.
    matcher: "",
    idle_notice: "",
    permission_notice: "",
    permission_sentence: "",
    injected: &[],
    // The tool that draws a question and waits on it: `request_user_input`
    // (core/src/tools/registry.rs:130-139), measured on 0.157.1 on 2026-09-28
    // as a PreToolUse with `tool_input.questions[]`.
    question_tool: "request_user_input",
    // A session opening carries `source` `startup`, `resume`, `clear`,
    // `compact` or `fork` (hooks/src/schema.rs:854); `startup` is a new
    // conversation. Measured on 0.157.1 on 2026-09-28.
    fresh_start: Some("startup"),
    // Its notices are not hooks, so no payload says what one is waiting on.
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
        assert_eq!(HOOKS.moment("PermissionRequest"), None, "Ruling 5");
        assert_eq!(HOOKS.moment("Interrupt"), None, "Ruling 5");
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
        assert_eq!(VENDOR.launch, ["--no-daemon"], "Ruling 1");
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
        assert!(!VENDOR.can(Capability::Trust), "Ruling 7");
        assert_eq!(VENDOR.hooks, Some(HOOKS));
        assert_eq!(VENDOR.transcript, Some(Transcript::Codex));
        assert_eq!(VENDOR.catalog, None, "t8 measures codex's catalog");
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
