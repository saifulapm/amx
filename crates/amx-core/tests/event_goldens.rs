//! Checked-in byte goldens for every event envelope on the bus.
//!
//! The control channel has had goldens since T04 and the streams since T18;
//! the bus has not, because until M1 nothing outside the server ever saw it.
//! `Event::SessionRestored` and `Event::ConfigReloaded` change that: M2's
//! `amx events --json` publishes this enum verbatim, and the plan
//! (`docs/07-m1-plan.md` §3) freezes the shapes *before* that surface exists
//! rather than after someone depends on them.
//!
//! The goldens live in the repository's one golden tree, `tests/goldens/`,
//! next to the protocol's — the same pretty-printed one-message-per-file
//! format, regenerated with:
//!
//! ```text
//! AMX_UPDATE_GOLDENS=1 cargo test -p amx-core --test event_goldens
//! ```
//!
//! They live *here* rather than in the rig because the goldens are named for
//! `Event::tag`, whose exhaustive match inside the library is what makes a new
//! variant a compile error until it says what it is called — a test crate
//! cannot match `#[non_exhaustive]` exhaustively, not even this package's own.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use amx_core::agent::{AgentKind, AgentState, AgentWorkspace, EpochMillis, StatusCause};
use amx_core::event::{Envelope, Event, Seq};
use amx_core::{ClientId, GridGeneration, InvalidationCause, PaneId, RowId, RowRange, WorkspaceId};

/// The sequence number every golden envelope carries.
///
/// One value for all of them: the envelope's shape is what is frozen here,
/// and a per-event counter would make an inserted variant rewrite every
/// golden after it.
const SEQ: Seq = 41;

/// The wall-clock instant every golden that carries one carries.
///
/// One value for the same reason [`SEQ`] is one value, and a real one rather
/// than a round number: it is 2025-08-08T11:26:40Z, so a reader of the golden
/// can tell at a glance that the units are milliseconds and not seconds.
const SINCE: EpochMillis = 1_754_650_000_000;

fn goldens_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/goldens/event")
}

fn pane_id() -> PaneId {
    "00000000-0000-0000-0000-0000000000a1".parse().unwrap()
}

fn workspace_id() -> WorkspaceId {
    "00000000-0000-0000-0000-0000000000b1".parse().unwrap()
}

fn client_id() -> ClientId {
    "00000000-0000-0000-0000-0000000000d1".parse().unwrap()
}

/// One example of every event the bus can carry.
///
/// Hand-built rather than derived, like the stream goldens' own fixture list
/// (`tests/protocol.rs::grid_message_goldens`): the values are documentation
/// of what each variant looks like in the field, and a generator would produce
/// zeros. A new variant belongs here *and* in `Event::tag`, which is where the
/// compiler insists.
fn every_event() -> Vec<Event> {
    let pane = pane_id();
    let workspace = workspace_id();
    vec![
        Event::PaneCreated { pane, workspace },
        Event::PaneExited {
            pane,
            status: Some(0),
        },
        Event::PaneResized {
            pane,
            rows: 24,
            cols: 80,
            generation: GridGeneration::FIRST.next(),
        },
        Event::PaneDamage {
            pane,
            generation: GridGeneration::FIRST.next(),
        },
        Event::PaneRenamed {
            pane,
            label: "editor".to_owned(),
        },
        Event::PaneTitle {
            pane,
            title: "amx — build".to_owned(),
        },
        Event::HistoryCommitted {
            pane,
            range: RowRange::new(RowId::from_raw(1), RowId::from_raw(3)),
        },
        Event::HistoryInvalidated {
            pane,
            from_row: RowId::from_raw(2),
            cause: InvalidationCause::WidthReflow,
        },
        Event::HistoryEvicted {
            pane,
            oldest_row: RowId::from_raw(2),
        },
        Event::WorkspaceCreated { workspace },
        Event::WorkspaceRenamed {
            workspace,
            label: "build".to_owned(),
        },
        Event::WorkspaceClosed { workspace },
        Event::FocusChanged {
            workspace,
            pane: Some(pane),
        },
        Event::LayoutChanged { workspace },
        Event::ClientAttached {
            client: client_id(),
        },
        Event::ClientDetached {
            client: client_id(),
        },
        Event::SessionRestored {
            workspaces: 2,
            panes: 4,
            lost: 1,
            degraded: 1,
        },
        Event::ConfigReloaded {
            rejected_sections: 1,
        },
        // M2's four. `agent_status` is the one that carries shape: a `from`
        // that is present here and absent on a pane's first status, and a
        // `cause` that is the whole of 04 §5's precedence made visible to
        // `amx events --json`.
        Event::AgentStatus {
            pane,
            from: Some(AgentState::Working),
            to: AgentState::Blocked,
            cause: StatusCause::Hook,
        },
        Event::AgentIdentified {
            pane,
            kind: AgentKind::new("claude").unwrap(),
        },
        // M4's identity block, frozen with values rather than as absences: a
        // shape nothing ever populates is a shape nobody has read, and the
        // whole point of the block is that a notifier can render a line from
        // the event alone (D15). The absent shape — a hub that has seen no
        // label — is asserted in `contracts.rs`, where both directions of the
        // additive rule belong.
        Event::AttentionEnqueued {
            pane,
            workspace: Some(AgentWorkspace {
                id: workspace,
                name: Some("api".to_owned()),
            }),
            name: Some("backend".to_owned()),
            reason: Some("permission_dialog".to_owned()),
            since: Some(SINCE),
        },
        Event::AttentionDequeued {
            pane,
            workspace: Some(AgentWorkspace {
                id: workspace,
                name: Some("api".to_owned()),
            }),
            name: Some("backend".to_owned()),
            reason: Some("prompt_box_idle".to_owned()),
            since: Some(SINCE),
        },
    ]
}

#[test]
fn event_goldens_cover_agent_status_and_attention_variants() {
    let owed = golden_every_event();

    // M2's four, named here so the acceptance this test carries is legible
    // without counting the fixture list. They are the shapes `amx events
    // --json` and the reference notifier are written against, frozen before
    // either exists rather than after somebody depends on them.
    for m2 in [
        "agent_status",
        "agent_identified",
        "attention_enqueued",
        "attention_dequeued",
    ] {
        assert!(owed.contains(m2), "M2's {m2} owes a golden");
    }
}

#[test]
fn event_envelope_goldens_cover_session_restored_and_config_reloaded() {
    let owed = golden_every_event();

    // Both of M1's variants, named here for the same reason M2's are above.
    for m1 in ["session_restored", "config_reloaded"] {
        assert!(owed.contains(m1), "M1's {m1} owes a golden");
    }

    assert_files(&goldens_dir(), &owed);
}

/// Golden every event in [`every_event`], returning the tags it covered.
fn golden_every_event() -> BTreeSet<String> {
    let mut owed = BTreeSet::new();
    for event in every_event() {
        let name = event.tag().to_owned();
        let envelope = Envelope { seq: SEQ, event };
        // The file name is the serde tag: a variant renamed on the wire but
        // not here would leave the golden filed under its old name, which is
        // exactly the drift a golden exists to catch.
        let json = serde_json::to_value(&envelope).unwrap();
        assert_eq!(
            json["event"]["event"].as_str(),
            Some(name.as_str()),
            "the golden's name must be the tag the bus sends",
        );
        assert!(
            owed.insert(name.clone()),
            "two variants claim the golden {name}",
        );
        check_golden(&name, &envelope);
    }
    owed
}

/// Check `value` against `goldens/event/<name>.json`, or regenerate it.
fn check_golden(name: &str, value: &impl serde::Serialize) {
    let json = format!("{}\n", serde_json::to_string_pretty(value).unwrap());
    let path = goldens_dir().join(format!("{name}.json"));

    if std::env::var_os("AMX_UPDATE_GOLDENS").is_some() {
        let dir = goldens_dir();
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("creating {dir:?}: {e}"));
        std::fs::write(&path, &json).unwrap_or_else(|e| panic!("writing golden {path:?}: {e}"));
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("reading golden {path:?}: {e} (run with AMX_UPDATE_GOLDENS=1 to create it)")
    });
    assert_eq!(
        json, expected,
        "golden {name} does not match; run with AMX_UPDATE_GOLDENS=1 to update it after an intentional change"
    );
}

/// Assert `dir` holds exactly `expected` goldens — nothing owed missing,
/// nothing stray left behind by a deleted variant.
fn assert_files(dir: &Path, expected: &BTreeSet<String>) {
    let present: BTreeSet<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading {dir:?}: {e}"))
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .map(|path| {
            path.file_stem()
                .expect("a file stem")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let missing: Vec<_> = expected.difference(&present).collect();
    let stray: Vec<_> = present.difference(expected).collect();
    assert!(
        missing.is_empty() && stray.is_empty(),
        "event goldens drifted from the bus surface;\n  missing: {missing:?}\n  stray: {stray:?}\n\
         (a missing file wants AMX_UPDATE_GOLDENS=1; a stray one wants deleting)"
    );
}
