//! M4's two ledgers: the handler seam, closed by X10, and the fields still
//! frozen ahead of a reader.
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

/// M4's seam ledger, **closed**: no `seam(…)` may live in `crates/*/src`.
///
/// A row that lands before its wiring is answered through a `seam` helper
/// rather than `METHOD_NOT_FOUND`, because telling a client a method is unknown
/// tells it to stop offering it — and three of D15's surfaces are built on
/// `agent.list`. The helper is therefore a milestone's tool, and this test is
/// what keeps it one: while a milestone is being built the ledger is non-empty
/// and every call site must live in a file it names; when the last row is
/// answered, the helper, the list and the exemption go together.
///
/// That has now happened four times. U01 introduced the helper with M1's two
/// rows, U06 and U07 closed them. V02 brought both back for M2's twelve; V12
/// closed four, V09 one, V11 three, V13 two, and V17 closed the last two. W03
/// reopened it for M3's one, `session.handoff`, and W06 wired it. X02 reopened
/// it for M4's one, in `actor/core/route.rs` rather than in the dispatch tree —
/// the dispatch arm was *finished* and it was the answer that was owed — and
/// **X10 wired it**, which took the helper and the exemption with it.
///
/// So the assertion below has no exemption left to make. The next milestone
/// that tables a row ahead of its handler writes its own list here.
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
                found.push(format!("{where_it_is}:{}: {}", n + 1, line.trim()));
            }
        }
    }

    assert!(
        found.is_empty(),
        "M4's seam ledger is closed, so a `seam(…)` call site can only be a row \
         that landed without wiring. Implement it, or reopen the ledger above \
         with the file and the task that owes it:\n{}",
        found.join("\n")
    );
    assert!(
        scanned >= 50,
        "the crates scan read too few source files ({scanned}) to be believed"
    );
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
