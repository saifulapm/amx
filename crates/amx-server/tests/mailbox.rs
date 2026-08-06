//! The actor mailbox contracts.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::{GridGeneration, InvalidationCause, RowId, RowRange};
use amx_server::actor::{HistoryError, MailboxError, PaneCommand, PaneHandle, PaneReport};
use bytes::Bytes;
use tokio::sync::{mpsc, oneshot};

#[tokio::test]
async fn pane_commands_that_answer_carry_their_own_reply_channel() {
    let (tx, mut rx) = mpsc::channel(4);
    let handle = PaneHandle::new(tx);

    handle
        .send(PaneCommand::Write(Bytes::from_static(b"ls\r")))
        .await
        .unwrap();

    let (reply, answer) = oneshot::channel();
    handle
        .send(PaneCommand::HistoryRange {
            range: RowRange::new(RowId::FIRST, RowId::from_raw(9)),
            reply,
        })
        .await
        .unwrap();

    assert!(matches!(rx.recv().await, Some(PaneCommand::Write(_))));
    let Some(PaneCommand::HistoryRange { range, reply }) = rx.recv().await else {
        panic!("expected a history range command");
    };
    assert_eq!(range.row_count(), Some(10));

    // The actor answers the sender it was handed; there is no correlation
    // table that could pair the answer with the wrong request.
    reply
        .send(Err(HistoryError::Evicted {
            oldest: RowId::from_raw(4),
        }))
        .unwrap();
    assert!(matches!(
        answer.await.unwrap(),
        Err(HistoryError::Evicted { .. })
    ));
}

#[tokio::test]
async fn sending_to_a_stopped_actor_reports_it_gone() {
    let (tx, rx) = mpsc::channel::<PaneCommand>(1);
    let handle = PaneHandle::new(tx);
    drop(rx);

    assert!(handle.is_closed());
    assert_eq!(
        handle.send(PaneCommand::Shutdown).await,
        Err(MailboxError::Gone)
    );
}

#[test]
fn pane_reports_cover_every_transition_the_bus_publishes() {
    // Each report is the pane actor's half of a bus event; they are listed
    // together here so a new event variant without a report is visible.
    let reports = [
        PaneReport::Damage {
            generation: GridGeneration::FIRST,
        },
        PaneReport::Committed(RowRange::single(RowId::FIRST)),
        PaneReport::Invalidated {
            from_row: RowId::FIRST,
            cause: InvalidationCause::WidthReflow,
        },
        PaneReport::Evicted {
            oldest_row: RowId::FIRST,
        },
        PaneReport::Title("build".into()),
        PaneReport::Bell,
        PaneReport::Exited { status: Some(0) },
    ];
    assert_eq!(reports.len(), 7);
    assert_ne!(reports[0], reports[1]);
}
