//! pi, the second vendor amx knows anything about.
//!
//! Everything here is the vendor's own words, measured against 0.84.4's
//! `--help` and the unbundled JS shipped beside it, each value on the date it
//! carries. Re-measure at every vendor bump: these are not amx's names to
//! choose, and a renamed flag turns a dial into a spawn that fails.

use super::{
    Capability, Catalog, DEFAULT, DialSpec, ForkSpec, Hooks, Moment, Place, SessionSpec,
    Transcript, Vendor, Wire, Wiring,
};

/// pi's entry in the table.
pub const VENDOR: Vendor = Vendor {
    name: "pi",
    // Open: `--model <pattern>` takes a provider/id pattern of the caller's
    // choosing, and documents no aliases of its own the way claude's three
    // are. The cycle offers nothing beyond the sentinel. Measured at 0.84.4
    // on 2026-09-03.
    model: Some(DialSpec {
        cycle: &[DEFAULT],
        open: true,
        flag: "--model",
    }),
    // pi has no permission dial: nothing in `--help` asks it to run less
    // trusting than it otherwise would, so there is no flag to spell one
    // with. Measured at 0.84.4 on 2026-09-03.
    permission: None,
    // Closed: `--thinking <level>` documents exactly these seven, and a
    // value off that list is not one the vendor has a meaning for. Measured
    // at 0.84.4 on 2026-09-03.
    effort: Some(DialSpec {
        cycle: &[
            DEFAULT, "off", "minimal", "low", "medium", "high", "xhigh", "max",
        ],
        open: false,
        flag: "--thinking",
    }),
    // `--session-id <id>` is mint-or-open (dist/main.js:337-344): it opens
    // the project session already under that id, or creates one under it if
    // none exists. One flag does the work claude splits across a start flag
    // it does not have and a resume flag it does, so both point at it here.
    // Two words, not joined: `--help` shows `--session-id <id>`, and nothing
    // in dist/main.js reads a `=` spelling of it.
    //
    // It refuses to be combined with `--session`, `--continue` or `--resume`
    // (dist/main.js:237-247), and `-c`/`-r` are `--help`'s own short
    // spellings of the latter two. `--no-session` is the sixth, and it
    // refuses nothing: pi reads that branch before the one that writes a
    // session to disk (dist/main.js:278-280), so `--session-id` under it
    // names an in-memory session and no file is ever left behind. amx would
    // have put that id in `meta.session` and offered it back, telling
    // somebody a conversation was carried on that was never written. Its own
    // `validateForkFlags` groups `--no-session` with the other three
    // (dist/main.js:227-231). All six are what a minted id has to displace.
    //
    // `pi --fork <origin> --session-id <new>` branches into a chosen id
    // (dist/main.js:283-295): the origin rides on `--fork` itself rather than
    // beside `--session-id`, which is `ForkSpec::Origin`. Measured at 0.84.4
    // on 2026-09-04.
    session: Some(SessionSpec {
        start: Some("--session-id"),
        resume: "--session-id",
        joined: false,
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
    // Handed to every command pi's bash tool runs, measured at 0.84.4 on
    // 2026-09-03 in core/tools/bash.js's resolveSpawnContext: the id of the
    // session the agent has open, which is what lets `adopt` find its way
    // home the same way claude's CLAUDE_CODE_SESSION_ID does.
    session_env: Some("PI_SESSION_ID"),
    // The other four resolveSpawnContext strips and reissues alongside
    // PI_SESSION_ID, so an agent handed its spawner's copies would believe it
    // is that session rather than a new one, plus the two markers dist/cli.js
    // and dist/rpc-entry.js set on every process pi starts: AI_AGENT names
    // which vendor is running and PI_CODING_AGENT that pi in particular is.
    // Measured at 0.84.4 on 2026-09-03.
    not_inherited: &[
        "PI_SESSION_ID",
        "PI_SESSION_FILE",
        "PI_PROVIDER",
        "PI_MODEL",
        "PI_REASONING_LEVEL",
        "AI_AGENT",
        "PI_CODING_AGENT",
    ],
    // pi reports through the extension `HOOKS` below carries, which is what
    // gives it Hooks, and a report names the session file pi appends to as a
    // turn runs, which is what gives it Transcript — `crate::conversation`
    // reads that file by the shape named at the foot of this entry. Trust is
    // the one amx sends rather than writes: `--help` documents `--approve,
    // -a` as trusting project-local files for a run, which is a word on the
    // argv of the pane amx was starting anyway and leaves nothing behind in
    // anybody's files. `crate::trust` is where that answer is measured and
    // written down. Measured at 0.84.4 on 2026-09-05.
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
    // the capture, the version and the date it was read at. Driven live
    // against 0.84.4 on 2026-09-04: a dialog, a running turn and a prompt,
    // plus the chrome that gets cut off a capture before anybody reads it.
    screens: Some(include_str!("../../assets/screen-rules-pi.toml")),
    // The conversation pi writes under `~/.pi/agent/sessions/<cwd>/`, one
    // file per session named after the id it was opened under, in the shape
    // `crate::conversation` reads pi's by. The shape is measured; whether a
    // record ever names such a file is a question of the capabilities above.
    transcript: Some(Transcript::Pi),
    // Where pi loads what somebody can name on a line, measured at 0.85.1 on
    // 2026-09-07 against the README shipped beside it: four skill directories
    // in the order pi reads them, and two for the prompt templates it expands
    // with `/name`. A skill is run as `/skill:name`, which is the prefix.
    //
    // The built-ins are the 23 rows of that README's Commands table, in its
    // order, `/login` to `/quit`; the first row names two, so 23 rows are 24
    // words. pi ships no sub agents, so it has no agents places at all.
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
        skill_prefix: "skill:",
    }),
};

/// How pi reports what it is doing, and where amx asks it to.
///
/// pi has no settings file a hook command can be named in: its events are
/// callbacks inside its own process, handed to whatever extension asks for
/// them. So the wire is a file — `assets/pi/amx.ts`, written whole where pi
/// loads extensions from — and that file is what runs `amx _hook`, one
/// invocation per moment with the payload on stdin, under pi's own event
/// names and the keys claude's payloads carry.
///
/// Six moments out of pi's list, measured against 0.84.4's extension API on
/// 2026-09-05. `ui_prompt_start` is a stop on a question the way claude's
/// `Notification` is, and `ui_prompt_end` the prompt closing and the turn
/// going on, which is what `Refused` means to the record. There is no `Asked`:
/// pi asks leave for nothing. Re-measure at every vendor bump: a renamed event
/// is a moment amx never hears.
///
/// `tests/mock_pi/pi` delivers these the way the extension does, step by step
/// out of a scenario, so the suite drives pi's entry through them on a machine
/// with no pi on it.
pub const HOOKS: Hooks = Hooks {
    wire: Wire::File {
        path: ".pi/agent/extensions/amx.ts",
        body: include_str!("../../assets/pi/amx.ts"),
    },
    events: &[
        Wiring {
            moment: Moment::Started,
            event: "session_start",
            matched: false,
        },
        Wiring {
            moment: Moment::Prompted,
            event: "agent_start",
            matched: false,
        },
        Wiring {
            moment: Moment::Calling,
            event: "tool_execution_start",
            matched: false,
        },
        Wiring {
            moment: Moment::Notified,
            event: "ui_prompt_start",
            matched: false,
        },
        Wiring {
            moment: Moment::Refused,
            event: "ui_prompt_end",
            matched: false,
        },
        Wiring {
            moment: Moment::Ended,
            event: "agent_settled",
            matched: false,
        },
    ],
    // None of these: pi's extension takes every event without a matcher, has
    // no tool that draws a menu and waits on it, sends no typed notices about
    // an idle session or a permission box, and draws no permission box to
    // write a sentence on.
    matcher: "",
    question_tool: "",
    idle_notice: "",
    permission_notice: "",
    permission_sentence: "",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_is_wired_through_a_file_pi_loads_as_an_extension() {
        // Where pi 0.84.4 finds a global extension, per its own docs: one
        // `.ts` file under the agent directory's `extensions/`.
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
        // Every event it wires is one the file listens for, under that name.
        for wiring in HOOKS.events {
            assert!(
                body.contains(&format!("pi.on(\"{}\"", wiring.event)),
                "the extension never listens for {}",
                wiring.event
            );
            assert_eq!(HOOKS.moment(wiring.event), Some(wiring.moment));
        }
        assert!(body.contains("pi.on(\"message_update\""), "and it streams");
    }

    #[test]
    fn pi_asks_leave_for_nothing_and_says_so_by_naming_no_such_moment() {
        assert!(
            !HOOKS.events.iter().any(|w| w.moment == Moment::Asked),
            "pi draws no permission box"
        );
        assert_eq!(HOOKS.moment("ui_prompt_start"), Some(Moment::Notified));
        assert_eq!(HOOKS.moment("ui_prompt_end"), Some(Moment::Refused));
        assert_eq!(HOOKS.moment("Stop"), None, "claude's names are not pi's");
    }

    #[test]
    fn pi_declares_a_model_and_an_effort_dial_and_no_permission_dial() {
        // Measured against pi 0.84.4's `--help`. Re-measure at every vendor
        // bump: a renamed flag or a changed level list turns a dial into a
        // spawn that fails.
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
    fn pi_mints_or_opens_a_session_with_the_same_flag_written_two_words() {
        // `--session-id <id>` opens the id if it exists and creates it if it
        // does not, so amx offers the same flag whether this spawn is
        // starting a session or carrying one on.
        let session = VENDOR.session.expect("pi declares a session vocabulary");
        assert_eq!(session.start, Some("--session-id"));
        assert_eq!(session.resume, "--session-id");
        assert!(!session.joined, "--session-id <id> is two words, not one");
        assert_eq!(session.fork, Some(ForkSpec::Origin("--fork")));
    }

    #[test]
    fn pi_lists_every_flag_that_would_ignore_or_refuse_a_minted_id() {
        // Six, not the five that refuse. `--session`, `--continue`,
        // `--resume` and the two short spellings make pi exit rather than
        // take an id amx chose. `--no-session` takes it and throws it away:
        // its branch is read before the one that writes a session file, so
        // the id names a conversation that only ever existed in memory, and
        // amx would have recorded it as one somebody can come back to.
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
        // Measured at 0.84.4 in core/tools/bash.js's resolveSpawnContext.
        assert_eq!(VENDOR.session_env, Some("PI_SESSION_ID"));
    }

    #[test]
    fn pi_keeps_the_markers_of_the_session_a_spawn_was_typed_inside() {
        // The four other variables resolveSpawnContext strips and reissues
        // alongside PI_SESSION_ID, plus the two process markers dist/cli.js
        // and dist/rpc-entry.js set on every process pi starts.
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
        // Measured at 0.85.1 on 2026-09-07 against the README shipped beside
        // the installed pi: four skill directories in the order it reads them,
        // two for the prompt templates it calls up with `/name`, and the 23
        // rows of the Commands table for what pi answers out of itself,
        // `/login` to `/quit`. The first row names two commands, so 23 rows
        // are 24 words. Re-measure at every vendor bump: a moved directory is
        // a suggestion that never arrives, and a dropped command is one amx
        // offers after pi has stopped answering it.
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
        // Trust is the one pi claims that no report carries: the answer is a
        // flag on the argv rather than an entry in a file. Which flag, and
        // that pi is the vendor answered that way, is asserted in
        // src/trust.rs, where it was measured. The other five are claimed too,
        // which is every capability the table names.
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
        // The entry carries the hooks the extension delivers through, and the
        // two capabilities that come of them: what it is doing, in its own
        // word, and the session file that word names, read back as the
        // conversation. The screens document stays beside them, because a pi
        // whose extension is not installed, and every gate pi draws before a
        // turn, are read off the pane the way they always were.
        //
        // Which screens that document names, and in which order, is asserted
        // in src/rules.rs and only there. A second copy here would be a second
        // place to edit every time a screen is measured, and the two would
        // disagree the first time somebody edited one of them.
        assert_eq!(VENDOR.hooks, Some(HOOKS));
        assert!(VENDOR.can(Capability::Hooks));
        assert!(VENDOR.can(Capability::Transcript));
        assert_eq!(VENDOR.transcript, Some(Transcript::Pi));
        assert!(VENDOR.screens.is_some(), "pi declares screens");
    }
}
