//! W12's guard: no test in this rig waits on the wall clock.
//!
//! The failure mode being kept out: a test that naps a fixed interval and
//! then asserts, passing on a fast machine and flaking on a loaded one. Every
//! wait in this package is a condition plus a deadline — the deadline's expiry
//! is a *failure*, never the green path — and this suite enforces that shape
//! mechanically, because a convention nothing checks is a convention on its
//! way out.
//!
//! The second guard here is the same idea one milestone up: a *seam* — a
//! method that landed in the shared table before its implementation — is
//! allowed to exist only while its milestone is being built, and the way that
//! stops being permanent is a test that fails once the milestone ships. M2
//! opened twelve, M3 one, and both ledgers emptied on schedule; **M4's is open
//! with one row**, and X02 opened it beside a second ledger of the same kind.
//!
//! The second ledger is D-M4-10, which is the seam rule applied to *fields*: a
//! field frozen ahead of its reader costs nothing to freeze and does not make
//! the reader's design right, and M3 found three fields whose readers were
//! wrong or absent. So M4 counts fields alongside handlers, and neither list
//! empties until every named reader has landed and the integration owner has
//! watched it work over a socket rather than in a type.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::fs;
use std::path::Path;

use rig::Env;

/// The forbidden token, assembled so this file does not match itself.
fn needle() -> String {
    ["sl", "eep"].concat()
}

#[test]
fn no_test_depends_on_wall_clock_sleep() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let needle = needle();

    // The suites: no sleeping at all, under any name. Waiting is done through
    // the support crate's condition-driven helpers, and observation windows
    // (the flood test) tick an interval instead of napping blind.
    //
    // Recursive, because suites grew module directories — `persistence/`,
    // `seams/` — and a scan of the package root alone had quietly stopped
    // covering most of the rig's test code. `support/` is walked separately
    // below under its own, looser rule.
    let mut suites = 0;
    for path in rust_files(root) {
        if path.starts_with(root.join("support")) {
            continue;
        }
        // This file is exempt for one reason only: the acceptance-test name
        // itself contains the forbidden token.
        if path.file_name().is_some_and(|name| name == "hygiene.rs") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("read the suite");
        assert!(
            !text.contains(&needle),
            "{} calls {needle}; wait on a condition instead (rig::wait_until, \
             Terminal::wait_output, or an interval tick)",
            path.display()
        );
        suites += 1;
    }
    assert!(suites >= 10, "the scan found too few suites to be believed");

    // The support crate: the poll tick inside its deadline loops is the one
    // place a nap is allowed, and every such line names TICK so a bare
    // interval cannot hide there.
    let mut waits = 0;
    for entry in fs::read_dir(root.join("support")).expect("read support/") {
        let path = entry.expect("a directory entry").path();
        if !path.extension().is_some_and(|ext| ext == "rs") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("read the support module");
        for (n, line) in text.lines().enumerate() {
            if line.contains(&needle) {
                assert!(
                    line.contains("(TICK)"),
                    "{}:{} naps outside a TICK-paced deadline loop: {line}",
                    path.display(),
                    n + 1
                );
                waits += 1;
            }
        }
    }
    assert!(
        waits > 0,
        "the support crate's wait helpers have gone missing; this guard checks them"
    );
}

/// The same convention where the naps used to live: `crates/*/tests`.
///
/// Those suites legitimately *say* the forbidden word — spawned shells run
/// `sleep 60` to stay alive, and that is program text under test, not a test
/// waiting — so the scan matches the Rust call (`thread::sleep`,
/// `tokio::time::sleep`) rather than the word. Every call must either pace a
/// deadline loop at a named `TICK`-family constant (the line names `(TICK)`)
/// or be a scheduling window the test opens on purpose — a controlled ingest
/// rate, an adversarial hold — marked `// deliberate` on the same line, with
/// the justification above it. A deliberate window must never be load-bearing
/// for the green path: expiring long or short may weaken the test, never
/// redden it.
#[test]
fn crate_tests_wait_on_conditions_not_wall_clock() {
    // `<workspace>/tests/../crates`: this package sits beside `crates/`.
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates");
    let call = ["::sl", "eep"].concat();

    let mut suites = 0;
    let mut flagged = Vec::new();
    for krate in fs::read_dir(&crates).expect("read crates/") {
        let tests = krate.expect("a directory entry").path().join("tests");
        if !tests.is_dir() {
            continue;
        }
        for path in rust_files(&tests) {
            suites += 1;
            let text = fs::read_to_string(&path).expect("read the suite");
            for (n, line) in text.lines().enumerate() {
                if line.contains(&call)
                    && !line.contains("(TICK)")
                    && !line.trim_end().ends_with("// deliberate")
                {
                    flagged.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        flagged.is_empty(),
        "wall-clock naps outside a TICK-paced loop; wait on a condition, or \
         mark a justified scheduling window `// deliberate`:\n{}",
        flagged.join("\n")
    );
    assert!(
        suites >= 10,
        "the crates scan found too few suites ({suites}) to be believed"
    );
}

/// M4's seam ledger: the files a `seam(…)` may live in, and who owes each.
///
/// A row that lands before its wiring is answered through a `seam` helper
/// rather than `METHOD_NOT_FOUND`, because telling a client a method is unknown
/// tells it to stop offering it — and three of D15's surfaces are built on
/// `agent.list`. The helper is therefore a milestone's tool, and the test below
/// is what keeps it one: while a milestone is being built this list is
/// non-empty and every call site must live in a file it names; when the last
/// row is answered, the helper, this list and the exemption go together.
///
/// That has happened three times. U01 introduced the helper with M1's two rows,
/// U06 and U07 closed them. V02 brought both back for M2's twelve; V12 closed
/// four, V09 one, V11 three, V13 two, and V17 closed the last two. W03 reopened
/// it for M3's one, `session.handoff`, and W06 wired it.
///
/// **X02 reopens it for M4's one.** The helper is in `actor/core/route.rs` and
/// not in the dispatch tree, because the dispatch arm is *finished* — it routes
/// the call to the actor D-M4-2 puts the answer in — and it is the answer that
/// is owed. X10 replaces the arm and deletes the helper, this row and the
/// exemption together.
const SEAM_LEDGER: &[(&str, &str)] = &[("amx-server/src/actor/core/route.rs", "X10")];

/// D-M4-10's other half: a *field* frozen in M4 names the task that reads it.
///
/// R-M3-12 recorded the qualified version — "freezing a field ahead of its
/// reader costs nothing, and it does not make the reader's design right" — and
/// M3 then found three fields whose readers were wrong or absent. M4 freezes
/// six additive fields across three surfaces, so the ledger this milestone
/// keeps counts fields as well as handlers, and it does not empty until every
/// named reader has landed.
///
/// What this test can check mechanically is that each frozen field still exists
/// where the ledger says it does. What it cannot check is that the reader
/// arrived — that is the integration owner's, and this list is what X00 walks.
/// A row is deleted when its reader lands, not when the field does.
const FIELD_LEDGER: &[(&str, &str, &str)] = &[
    (
        "amx-core/src/agent/mod.rs",
        "pub reason: Option<String>",
        "X06 writes it; X10, X11, X14 and X16 read it",
    ),
    (
        "amx-core/src/agent/mod.rs",
        "pub since: Option<EpochMillis>",
        "X06 stamps it; X10, X11, X14 and X16 read it",
    ),
    (
        "amx-core/src/event/mod.rs",
        "workspace: Option<AgentWorkspace>",
        "X06 folds the names mirror; X16's --watch and examples/notify.sh read it",
    ),
    (
        "amx-proto/src/control/session.rs",
        "pub mouse: Option<MouseMode>",
        "X13 fills it from the pane's own terminal and reads it client-side",
    ),
    (
        "amx-proto/src/control/agent/list.rs",
        "pub now: EpochMillis",
        "X10 answers it; X14 and X16 render every age against it",
    ),
    (
        "amx-proto/src/control/agent/verbs.rs",
        "pub workspace: Option<WorkspaceId>",
        "X17 reads the scope on agent.next",
    ),
];

#[test]
fn no_dispatch_seam_outlives_the_milestone_that_opened_it() {
    // `<workspace>/tests/../crates`: the shipped code, not the suites, since a
    // test harness may legitimately name the concept.
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates");
    // A call or a definition, not the word: `seam` is the tree's ordinary
    // noun for a trait boundary (`platform.rs`, `persist/io.rs`) and banning
    // the word would ban the vocabulary.
    let call = "seam(";

    let mut found = Vec::new();
    let mut scanned = 0;
    for krate in fs::read_dir(&crates).expect("read crates/") {
        let src = krate.expect("a directory entry").path().join("src");
        if !src.is_dir() {
            continue;
        }
        for path in rust_files(&src) {
            scanned += 1;
            let text = fs::read_to_string(&path).expect("read a source file");
            let where_it_is = path.display().to_string().replace('\\', "/");
            for (n, line) in text.lines().enumerate() {
                // Prose says "the seam (`Pty`, `Ipc`)"; code says `seam(…)`.
                if !line.contains(call) || line.trim_start().starts_with("//") {
                    continue;
                }
                if SEAM_LEDGER
                    .iter()
                    .any(|(file, _)| where_it_is.ends_with(file))
                {
                    continue;
                }
                found.push(format!("{where_it_is}:{}: {}", n + 1, line.trim()));
            }
        }
    }

    assert!(
        found.is_empty(),
        "a `seam(…)` call site outside M4's ledger can only be a row that \
         landed without wiring. Implement it, or add its file and its owner to \
         SEAM_LEDGER:\n{}",
        found.join("\n")
    );
    assert!(
        scanned >= 50,
        "the crates scan read too few source files ({scanned}) to be believed"
    );

    // The other direction: a ledger row whose file no longer holds a seam is a
    // row somebody closed and forgot to strike, and a ledger that lies about
    // what is open is worse than no ledger.
    for (file, owner) in SEAM_LEDGER {
        let path = crates.join(file);
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("SEAM_LEDGER names {file}, which cannot be read: {e}"));
        assert!(
            text.lines()
                .any(|line| line.contains(call) && !line.trim_start().starts_with("//")),
            "SEAM_LEDGER says {owner} owes a seam in {file} and there is none \
             left; strike the row, delete the helper, and close the milestone's \
             ledger",
        );
    }
}

/// D-M4-10: every field M4 froze still exists where the ledger says it does.
///
/// The mechanical half of "no field ships without its reader inside the same
/// milestone". A field renamed or dropped while the ledger still claims a
/// reader for it fails here, which is the failure mode R-M4-14 records four
/// instances of: `workspace.create`'s `focus`, `Hello.resume`'s `generations`,
/// `client::Viewport.panes` and `client::Keybindings`, all frozen ahead of a
/// reader that never came.
///
/// The judgement half — whether the reader actually landed and works over a
/// socket — belongs to the integration owner, and this list is what it walks.
#[test]
fn every_field_m4_froze_is_still_where_its_ledger_row_says() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates");

    for (file, declaration, reader) in FIELD_LEDGER {
        let path = crates.join(file);
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("FIELD_LEDGER names {file}, which cannot be read: {e}"));
        assert!(
            text.contains(declaration),
            "FIELD_LEDGER says {file} declares `{declaration}` and {reader}; it \
             does not. A field that moved has a ledger row to move with it, and \
             a field that was dropped has a reader to tell",
        );
    }
    assert_eq!(
        FIELD_LEDGER.len(),
        6,
        "docs/11-m4-plan.md §3 freezes six additive fields; this list must be \
         all of them until their readers land",
    );
}

/// The four agent events, by the name their variants carry in the tree.
///
/// Named rather than derived: this guard is about who may *construct* them, so
/// it has to know the spelling, and a fifth variant added without being listed
/// here is a fifth variant nothing guards.
const AGENT_EVENTS: &[&str] = &[
    "Event::AgentStatus",
    "Event::AgentIdentified",
    "Event::AttentionEnqueued",
    "Event::AttentionDequeued",
];

/// The seven pane-thread events, by the name their variants carry in the tree.
///
/// Named for the same reason [`AGENT_EVENTS`] is: an eighth kind added without
/// being listed here is an eighth kind nothing guards.
const PANE_EVENTS: &[&str] = &[
    "Event::PaneDamage",
    "Event::PaneTitle",
    "Event::PaneResized",
    "Event::HistoryCommitted",
    "Event::HistoryInvalidated",
    "Event::HistoryEvicted",
    "Event::PaneExited",
];

/// Where in the shipped code any of `events` is handed to a bus or an event
/// list — `publish(Event::…` or `push(Event::…`.
///
/// Naming the *verbs* rather than the variants is what keeps this from
/// flagging every consumer: a `match` arm reading `Event::AgentStatus { .. }`
/// is a subscriber, and a rule that could not tell one from a publisher would
/// have to be turned off the first time somebody wrote a second consumer.
///
/// The limit, stated rather than hidden: a publisher that binds an event to a
/// local first (`let e = Event::AgentStatus { … }; bus.publish(e);`) slips
/// past. These are tripwires over a rule the tree is *known* to have broken
/// once already — they exist to make the ordinary way of breaking it again
/// fail a test, not to be a proof.
fn announcements_of(events: &[&str]) -> Vec<String> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates");
    let verbs = ["publish(", "push("];

    let mut announced = Vec::new();
    for krate in fs::read_dir(&crates).expect("read crates/") {
        let src = krate.expect("a directory entry").path().join("src");
        if !src.is_dir() {
            continue;
        }
        for path in rust_files(&src) {
            let text = fs::read_to_string(&path).expect("read a source file");
            for (n, line) in text.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                let names_one = events.iter().any(|event| line.contains(event));
                if names_one && verbs.iter().any(|verb| line.contains(verb)) {
                    announced.push(format!("{}:{}: {}", path.display(), n + 1, code));
                }
            }
        }
    }
    announced
}

/// R-M2-3: agent events have exactly one publisher, and it is the hub.
///
/// 04 §2 gives every transition one sequence number, which requires one
/// publisher per transition — read per event *kind*, which is the reading
/// `docs/09-m3-plan.md` D-M3-2 settled on and
/// [`pane_events_have_exactly_one_publisher`] is the other half of. The hub
/// owns the four agent kinds.
///
/// Two things are checked, because the rule has two halves:
///
/// - an agent event is announced in exactly one file,
///   `actor/agent_hub/commit.rs`, where the fusion machine's effects become
///   announcements;
/// - the bus sees that list only through `StatusView::commit`, whose one caller
///   is the hub — which is also what enforces §3's write-before-publish
///   ordering, since the view write and the publish are one call.
#[test]
fn agent_events_have_exactly_one_publisher() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates");
    let announced = announcements_of(AGENT_EVENTS);

    let mut committers = Vec::new();
    for krate in fs::read_dir(&crates).expect("read crates/") {
        let src = krate.expect("a directory entry").path().join("src");
        if !src.is_dir() {
            continue;
        }
        for path in rust_files(&src) {
            let text = fs::read_to_string(&path).expect("read a source file");
            let where_it_is = path.display().to_string();
            for (n, line) in text.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                // The server's own `.commit(` calls only: `commit` is an
                // ordinary word and the client's row cache has three of its
                // own. The type's definition carries the doctest that documents
                // the ordering, and a doctest is prose that happens to compile
                // rather than a publisher. `Exporter::commit` is §3 step 13 —
                // ownership of a whole session transferring to a successor
                // process — and shares nothing with this one but the English
                // word.
                //
                // The exemption is the *receiver*, not the directory. W06 wrote
                // it as "any file under `handoff/`", which is wider than the
                // fact it covers: the import half of that module seeds agent
                // statuses, and a `StatusView::commit` appearing there is
                // exactly the second publisher this test exists to catch.
                let server = where_it_is.contains("amx-server");
                let another_verb =
                    where_it_is.ends_with("actor/agent.rs") || code.contains("exporter.commit(");
                if server && line.contains(".commit(") && !another_verb {
                    committers.push(format!("{where_it_is}:{}: {code}", n + 1));
                }
            }
        }
    }

    let stray: Vec<&String> = announced
        .iter()
        .filter(|at| !at.contains("actor/agent_hub/commit.rs"))
        .collect();
    assert!(
        stray.is_empty(),
        "an agent event is announced outside the hub's commit path, which is \
         a second publisher of a transition that owes exactly one sequence \
         number (04 §2, R-M2-3):\n{stray:?}"
    );
    assert_eq!(
        announced.len(),
        AGENT_EVENTS.len(),
        "one announcement per event variant, all of them in the hub; found \
         {announced:?}"
    );

    let outside: Vec<&String> = committers
        .iter()
        .filter(|at| !at.contains("actor/agent_hub/"))
        .collect();
    assert!(
        outside.is_empty(),
        "StatusView::commit is the only path to the bus for an agent event, \
         and the hub is its only caller — a second one is a second publisher \
         (R-M2-3):\n{outside:?}"
    );
    assert_eq!(
        committers.len(),
        1,
        "exactly one commit call site, in the hub: {committers:?}"
    );
}

/// D-M3-2: pane-thread events have exactly one publisher, and it is the pane
/// actor that owns those threads.
///
/// This is the rule the agent guard above was written *around*: until M3,
/// `Core` republished six of the seven kinds from the report that followed
/// each one, so a title change or an exit burned two sequence numbers. M2
/// could live with that because its consumers read the bus at-least-once; the
/// reconnect-resync cannot, because the duplicates halve the replay ring a
/// resuming client spends.
///
/// `Core` stays the publisher of everything it owns — panes created, renamed
/// and closed, workspaces, focus, layout, restore, config — which is why this
/// is a list of *kinds* and not a ban on `Core` publishing.
#[test]
fn pane_events_have_exactly_one_publisher() {
    let announced = announcements_of(PANE_EVENTS);

    let stray: Vec<&String> = announced
        .iter()
        .filter(|at| !at.contains("actor/pane_host/actor.rs"))
        .collect();
    assert!(
        stray.is_empty(),
        "a pane event is announced outside the pane actor, which is a second \
         publisher of a transition that owes exactly one sequence number \
         (04 §2, `docs/09-m3-plan.md` D-M3-2). A `Core` fold beside a report \
         is fine; a publish is not:\n{stray:?}"
    );
    assert_eq!(
        announced.len(),
        PANE_EVENTS.len(),
        "one announcement per event variant, all of them in the pane actor; \
         found {announced:?}"
    );
}

// ------------------------------------------------- the socket-path budget

/// The other convention nothing used to check: a session tag long enough to
/// push its socket past darwin's `sun_path`.
///
/// The guard lives in [`rig::env::assert_sun_path_fits`] and is charged against
/// darwin's `$TMPDIR` on every platform, which is the only way a Linux
/// developer finds out. This is the proof it still bites — and it is the shape
/// the flags suite arrived in: a readable tag, a socket four bytes too long,
/// green everywhere but on a macOS runner.
#[test]
#[should_panic(expected = "bytes below $TMPDIR")]
fn a_tag_too_long_to_bind_on_darwin_is_refused_here_too() {
    let over = "a-session-tag-nobody-would-choose-but-somebody-eventually-will";
    rig::env::assert_sun_path_fits(Path::new(&std::env::temp_dir()), over);
}

/// A terminal this harness opens is the harness's, and stays the harness's.
///
/// W01's finding (`docs/notes/m3-shutdown-wedge.md` §3). Both halves of
/// [`rig::term::open_pty`] used to be inheritable, so every `amx` the rig
/// spawned — and every server those clients daemonized — kept a copy of every
/// terminal the test binary had open at that instant. A server that seeded one
/// pane was then found holding six pty masters, five of them belonging to other
/// tests, and the shutdown wedge acquired a symptom ("the pane spawn repeated")
/// that no code in the tree could produce.
///
/// Two assertions, because the mechanism and its consequence fail differently.
/// The flag is checked everywhere: it is what the fix actually is. The
/// inheritance is checked where the process table can be read — the same
/// honest degradation every probe in [`rig::platform`] makes — because a flag
/// that is set and a descriptor that does not travel are not the same claim,
/// and this test exists because the second one was the one that was false.
#[tokio::test]
async fn a_harness_terminal_does_not_leak_into_the_processes_it_spawns() {
    use std::os::fd::AsFd;

    let pty = rig::term::open_pty(24, 80);
    for (which, fd) in [("master", pty.master.as_fd()), ("slave", pty.slave.as_fd())] {
        let flags = rustix::io::fcntl_getfd(fd).expect("read the descriptor flags");
        assert!(
            flags.contains(rustix::io::FdFlags::CLOEXEC),
            "the harness's pty {which} is inheritable, so every process this rig \
             spawns from here on keeps a copy of this terminal"
        );
    }

    let Some(mine) = rig::platform::pty_masters(std::process::id()) else {
        // Loud, not silent: the flag assertion above still ran, and this line
        // is the record of which half of the claim this platform can make.
        println!(
            "this platform has no unprivileged reader for another process's \
             descriptor table; the inheritance half of this test did not run"
        );
        return;
    };
    assert!(
        mine.contains(&index_of(&pty)),
        "the probe cannot see this process's own terminal, so its silence about \
         the server's would mean nothing"
    );

    let env = Env::new("cloex");
    let server = env.server();
    let theirs = rig::platform::pty_masters(server.pid()).unwrap_or_default();
    let shared: Vec<u32> = theirs
        .iter()
        .copied()
        .filter(|i| mine.contains(i))
        .collect();
    server.shutdown();
    assert!(
        shared.is_empty(),
        "the server holds terminal(s) {shared:?} that belong to this test binary; \
         it should hold only its own panes' ({theirs:?} against the harness's {mine:?})"
    );
}

/// The slave index of `pty`'s master, so the probe can be checked against a
/// terminal whose owner is known.
#[cfg(target_os = "linux")]
fn index_of(pty: &rig::term::Pty) -> u32 {
    use std::os::fd::AsRawFd as _;

    let info = fs::read_to_string(format!("/proc/self/fdinfo/{}", pty.master.as_raw_fd()))
        .expect("read this descriptor's info");
    info.lines()
        .find_map(|line| line.strip_prefix("tty-index:"))
        .and_then(|value| value.trim().parse().ok())
        .expect("a pty master reports its slave index")
}

/// See the Linux arm; unreachable where [`rig::platform::pty_masters`] is
/// `None`, which is the only caller's guard.
#[cfg(not(target_os = "linux"))]
fn index_of(_pty: &rig::term::Pty) -> u32 {
    unreachable!("no index is asked for where the descriptor table cannot be read")
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let entries = fs::read_dir(dir).expect("read a test directory");
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            found.extend(rust_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
    found
}
