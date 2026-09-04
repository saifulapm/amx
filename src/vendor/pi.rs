//! pi, the second vendor amx knows anything about.
//!
//! Everything here is the vendor's own words, measured against 0.84.4's
//! `--help` and the unbundled JS shipped beside it, each value on the date it
//! carries. Re-measure at every vendor bump: these are not amx's names to
//! choose, and a renamed flag turns a dial into a spawn that fails.

use super::{Capability, DEFAULT, DialSpec, ForkSpec, SessionSpec, Vendor};

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
    // pi's extension events are JS callbacks inside its own process, not
    // command entries a settings file can name, so there is nothing for
    // `install` to write and nothing for `hook` to read: Hooks and
    // Transcript are both left off. Trust is on, and it is the one amx sends
    // rather than writes: `--help` documents `--approve, -a` as trusting
    // project-local files for a run, which is a word on the argv of the pane
    // amx was starting anyway and leaves nothing behind in anybody's files.
    // `crate::trust` is where that answer is measured and written down. So
    // this is what pi does today: carry a session on, branch one, be adopted
    // by way of PI_SESSION_ID, and take its folder-trust answer from amx.
    // Measured at 0.84.4 on 2026-09-05.
    capabilities: &[
        Capability::Resume,
        Capability::Fork,
        Capability::Adopt,
        Capability::Trust,
    ],
    hooks: None,
    // The screens amx has measured off this vendor, every anchor in them with
    // the capture, the version and the date it was read at. Driven live
    // against 0.84.4 on 2026-09-04: a dialog, a running turn and a prompt,
    // plus the chrome that gets cut off a capture before anybody reads it.
    screens: Some(include_str!("../../assets/screen-rules-pi.toml")),
};

#[cfg(test)]
mod tests {
    use super::*;

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
    fn pi_can_resume_fork_be_adopted_and_have_its_trust_screen_answered() {
        // Trust is the one pi claims without reporting anything: the answer is
        // a flag on the argv rather than an entry in a file, so a vendor with
        // no hooks can still have its screen taken off a person's hands. Which
        // flag, and that pi is the vendor answered that way, is asserted in
        // src/trust.rs, where it was measured.
        for can in [
            Capability::Resume,
            Capability::Fork,
            Capability::Adopt,
            Capability::Trust,
        ] {
            assert!(VENDOR.can(can), "{can:?}");
        }
        for cannot in [Capability::Hooks, Capability::Transcript] {
            assert!(!VENDOR.can(cannot), "{cannot:?}");
        }
    }

    #[test]
    fn pi_reports_through_no_hooks_and_reads_its_state_off_the_pane() {
        // Its extension events are JS callbacks, not settings-file entries,
        // so install has nothing to write and hook has nothing to read. The
        // pane is not the last witness on this vendor, it is the only one, and
        // the screens document is where what amx can see on it is written
        // down.
        //
        // Which screens that document names, and in which order, is asserted
        // in src/rules.rs and only there. A second copy here would be a second
        // place to edit every time a screen is measured, and the two would
        // disagree the first time somebody edited one of them.
        assert!(VENDOR.hooks.is_none());
        assert!(VENDOR.screens.is_some(), "pi declares screens");
    }
}
