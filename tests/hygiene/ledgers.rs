//! M4's two ledgers: the handler seam that is open, and the fields frozen
//! ahead of their readers.
//!
//! A child module of [`super`] rather than a second suite, because both guards
//! are the same idea at two granularities and both are struck by the same task
//! — X00 walks this file and nothing else. Split out by X02, which opened them
//! and pushed the parent past the module budget doing it.
//!
//! The parent's guards are about *test discipline*; these are about *milestone
//! discipline*, and they are the only two things in this package that are meant
//! to be deleted rather than kept.

use std::fs;
use std::path::Path;

use super::rust_files;

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
