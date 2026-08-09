//! What the `amx agents` table says about a reply, over the public renderer.
//!
//! Nothing here talks to a server. `agents_cli.rs` is the other half — the real
//! binary against a real session — and this is the half that can put a reply
//! into a shape a live session takes hours to reach: an agent blocked for four
//! days, a `since` nobody stamped, a screen line carrying a control byte.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::agent::{AgentState, AgentWorkspace, EpochMillis};
use amx_core::{PaneId, WorkspaceId};
use amx_proto::control::agent::{AgentEntry, ListReply};

/// A moment to date every fixture against: 2026-08-09, in epoch milliseconds.
const NOW: EpochMillis = 1_786_000_000_000;

/// One second, in the units `since` is spelled in.
const SECOND: EpochMillis = 1_000;

/// A reply carrying `agents`, with the blocked ones queued oldest first.
fn reply(agents: Vec<AgentEntry>) -> ListReply {
    let mut blocked: Vec<&AgentEntry> = agents
        .iter()
        .filter(|entry| entry.status == AgentState::Blocked)
        .collect();
    blocked.sort_by_key(|entry| entry.since);
    ListReply {
        seq: 41_023,
        now: NOW,
        attention: blocked.iter().map(|entry| entry.pane).collect(),
        agents,
    }
}

/// A workspace whose id follows from its label, so two rows written as being
/// in `api` really are in one workspace.
///
/// Grouping is by id and never by label — a session may hold two workspaces
/// with the same name — so a fixture that spelled the label twice and let the
/// ids differ would be testing the wrong thing and would pass anyway.
fn workspace(label: &str) -> AgentWorkspace {
    let tag = label
        .bytes()
        .fold(0_u32, |sum, byte| sum * 31 + u32::from(byte));
    let id: WorkspaceId = format!("00000000-0000-4000-8000-{tag:012x}")
        .parse()
        .expect("a workspace id");
    AgentWorkspace {
        id,
        name: Some(label.to_owned()),
    }
}

/// One row. `ago` is how long ago it entered its state, in seconds.
fn agent(label: &str, name: &str, status: AgentState, ago: u64) -> AgentEntry {
    AgentEntry {
        workspace: workspace(label),
        pane: PaneId::new_v4(),
        name: Some(name.to_owned()),
        kind: None,
        status,
        reason: None,
        since: Some(NOW - ago * SECOND),
        last_line: String::new(),
    }
}

/// The rows of a rendered table, without its count line or its header.
fn rows(lines: &[String]) -> &[String] {
    assert!(
        lines.len() >= 2,
        "a table has a count and a header: {lines:?}"
    );
    &lines[2..]
}

/// Whether the header row carries a column called `name`.
///
/// By word and not by substring: `AGENT` contains `AGE`, and a test that asked
/// the lazy way would say the age column is present in every table there is.
fn has_column(header: &str, name: &str) -> bool {
    header.split("  ").any(|cell| cell.trim() == name)
}

/// The `AGENT` cell of every row, in the order they were rendered.
fn names(lines: &[String]) -> Vec<String> {
    rows(lines)
        .iter()
        .map(|line| {
            line.split_whitespace()
                .next()
                .expect("every row names an agent")
                .to_owned()
        })
        .collect()
}

// ------------------------------------------------------------------ ordering

#[test]
fn blocked_agents_come_first_in_the_queues_own_order() {
    // Deliberately not in `since` order on the wire: the reply answers in the
    // server's pane order, and the queue is a separate list.
    let reply = reply(vec![
        agent("api", "tests", AgentState::Working, 120),
        agent("api", "backend", AgentState::Blocked, 240),
        agent("infra", "deploy", AgentState::Idle, 660),
        agent("docs", "writer", AgentState::Blocked, 420),
    ]);
    let lines = amx::agents::table::render(&reply, NOW, None);

    // The two blocked rows, longest-waiting first, are the top two — which is
    // the order `attention` carries and therefore the order `agent.next` walks.
    assert_eq!(
        names(&lines),
        ["docs/writer", "api/backend", "api/tests", "infra/deploy"],
        "{lines:#?}"
    );
}

#[test]
fn the_blocked_band_follows_the_queue_and_not_the_stamps() {
    // A queue that disagrees with the `since` stamps — which is what a re-block
    // produces, since D-M2-8 puts a pane that unblocked and blocked again at
    // the tail. The queue is the fact and the table has to say so, or the top
    // row is not the pane `amx agent next` would jump to.
    let old = agent("api", "backend", AgentState::Blocked, 900);
    let new = agent("docs", "writer", AgentState::Blocked, 10);
    let (head, tail) = (new.pane, old.pane);
    let lines = amx::agents::table::render(
        &ListReply {
            seq: 1,
            now: NOW,
            attention: vec![head, tail],
            agents: vec![old, new],
        },
        NOW,
        None,
    );
    assert_eq!(names(&lines), ["docs/writer", "api/backend"], "{lines:#?}");
}

#[test]
fn a_projects_rows_stay_together_inside_a_band() {
    let reply = reply(vec![
        agent("api", "one", AgentState::Idle, 1),
        agent("docs", "two", AgentState::Idle, 2),
        agent("api", "three", AgentState::Idle, 3),
        agent("docs", "four", AgentState::Idle, 4),
    ]);
    let lines = amx::agents::table::render(&reply, NOW, None);
    assert_eq!(
        names(&lines),
        ["api/one", "api/three", "docs/two", "docs/four"],
        "grouped by workspace, and by creation order inside one: {lines:#?}"
    );
}

#[test]
fn the_two_tier_three_states_never_outrank_an_agent() {
    let reply = reply(vec![
        agent("w", "quiet", AgentState::Quiet, 1),
        agent("w", "idle", AgentState::Idle, 1),
        agent("w", "busy", AgentState::Busy, 1),
        agent("w", "working", AgentState::Working, 1),
    ]);
    assert_eq!(
        names(&amx::agents::table::render(&reply, NOW, None)),
        ["w/working", "w/busy", "w/idle", "w/quiet"],
    );
}

// ---------------------------------------------------------------------- ages

#[test]
fn an_age_is_measured_against_the_replys_clock_and_never_this_machines() {
    // `now` a whole day away from the fixture's stamps, which is what a server
    // whose clock differs from the reader's looks like (D-M4-4). Every age is
    // computed from the pair inside one reply, so the answer does not move.
    let reply = reply(vec![agent("api", "backend", AgentState::Working, 240)]);
    let here = amx::agents::table::render(&reply, NOW, None);
    let mut skewed = reply.clone();
    let day = 24 * 60 * 60 * SECOND;
    skewed.now += day;
    for entry in &mut skewed.agents {
        entry.since = entry.since.map(|since| since + day);
    }
    let there = amx::agents::table::render(&skewed, skewed.now, None);
    assert_eq!(here, there, "the same age on two machines' clocks");
    assert!(here[2].ends_with("4m"), "{:?}", here[2]);
}

#[test]
fn an_age_uses_the_coarsest_unit_that_still_says_something() {
    for (ago, shown) in [
        (0, "0s"),
        (59, "59s"),
        (60, "1m"),
        (59 * 60 + 59, "59m"),
        (60 * 60, "1h"),
        (24 * 60 * 60 - 1, "23h"),
        (24 * 60 * 60, "1d"),
        (4 * 24 * 60 * 60, "4d"),
    ] {
        assert_eq!(amx::agents::age(NOW, Some(NOW - ago * SECOND)), shown);
    }
}

#[test]
fn a_status_nobody_watched_start_has_a_blank_age_and_never_a_zero() {
    // A probe-derived status carries no `since` at all, and `0s` would be a
    // measurement nobody made.
    assert_eq!(amx::agents::age(NOW, None), "");

    let mut entry = agent("api", "backend", AgentState::Idle, 0);
    entry.since = None;
    let lines = amx::agents::table::render(&reply(vec![entry]), NOW, None);
    assert!(
        !has_column(&lines[1], "AGE"),
        "a column no row fills is not printed: {:?}",
        lines[1]
    );
}

// ------------------------------------------------------------------- columns

#[test]
fn a_column_no_row_fills_is_not_printed() {
    // Every status here is probe-derived, so no row carries a reason and no row
    // has printed anything. Eight columns of `REASON` over nothing is eight
    // columns a 45-column window does not have.
    let lines = amx::agents::table::render(
        &reply(vec![agent("api", "backend", AgentState::Quiet, 3)]),
        NOW,
        None,
    );
    assert!(has_column(&lines[1], "AGENT"), "{:?}", lines[1]);
    assert!(has_column(&lines[1], "STATUS"), "{:?}", lines[1]);
    assert!(has_column(&lines[1], "AGE"), "{:?}", lines[1]);
    assert!(!has_column(&lines[1], "REASON"), "{:?}", lines[1]);
    assert!(!has_column(&lines[1], "LAST LINE"), "{:?}", lines[1]);
}

#[test]
fn a_reason_is_printed_verbatim_whichever_tier_answered() {
    // D-M4-3: the detector's own name, and there are two vocabularies — the
    // manifest rule's for a screen-owned state, the hook event's for a
    // hook-asserted one. Nothing translates either.
    let mut hook = agent("api", "backend", AgentState::Blocked, 10);
    hook.reason = Some("PermissionRequest".to_owned());
    let mut screen = agent("docs", "writer", AgentState::Blocked, 20);
    screen.reason = Some("permission_dialog".to_owned());
    let lines = amx::agents::table::render(&reply(vec![hook, screen]), NOW, None);
    let text = lines.join("\n");
    assert!(text.contains("PermissionRequest"), "{text}");
    assert!(text.contains("permission_dialog"), "{text}");
}

#[test]
fn a_pane_nobody_named_says_so_rather_than_leaving_a_hole() {
    let mut entry = agent("api", "backend", AgentState::Idle, 5);
    entry.name = None;
    entry.workspace.name = None;
    let lines = amx::agents::table::render(&reply(vec![entry]), NOW, None);
    assert!(lines[2].starts_with("-/-"), "{:?}", lines[2]);
}

// --------------------------------------------------------------------- width

/// The blocked fixture the width tests measure, with a long last line.
fn wide_reply() -> ListReply {
    let mut blocked = agent("api", "backend", AgentState::Blocked, 240);
    blocked.reason = Some("permission_dialog".to_owned());
    blocked.last_line = "Allow Bash(git push origin main)? (y/n)".to_owned();
    reply(vec![blocked])
}

#[test]
fn nothing_is_truncated_when_the_output_is_not_going_to_a_terminal() {
    let lines = amx::agents::table::render(&wide_reply(), NOW, None);
    assert!(
        lines[2].ends_with("Allow Bash(git push origin main)? (y/n)"),
        "a redirected table keeps the whole line: {:?}",
        lines[2]
    );
}

#[test]
fn a_forty_five_column_window_still_names_the_agent_its_state_and_its_age() {
    // The window D14 exists for, and §5's own acceptance: the command works at
    // 45 columns. Not "fits by luck" — the columns that matter survive and no
    // line is wider than the terminal.
    let lines = amx::agents::table::render(&wide_reply(), NOW, Some(45));
    for line in &lines {
        assert!(line.chars().count() <= 45, "over 45 columns: {line:?}");
    }
    assert!(lines[2].starts_with("api/backend"), "{:?}", lines[2]);
    assert!(lines[2].contains("blocked"), "{:?}", lines[2]);
    assert!(lines[2].contains("4m"), "{:?}", lines[2]);
    // The reason is the first thing given up, because `blocked` already says
    // the half of it a narrow reader needs.
    assert!(!has_column(&lines[1], "REASON"), "{:?}", lines[1]);
    assert!(has_column(&lines[1], "LAST LINE"), "{:?}", lines[1]);
    assert!(
        lines[2].contains('…'),
        "the line was clipped: {:?}",
        lines[2]
    );
}

#[test]
fn the_columns_are_dropped_in_the_order_a_narrow_reader_needs_them_least() {
    let reply = wide_reply();
    let header = |width: usize| amx::agents::table::render(&reply, NOW, Some(width))[1].clone();
    assert!(has_column(&header(120), "REASON"), "{}", header(120));
    assert!(!has_column(&header(45), "REASON"), "{}", header(45));
    assert!(!has_column(&header(30), "LAST LINE"), "{}", header(30));
    assert!(has_column(&header(30), "AGE"), "{}", header(30));
    let narrow = header(20);
    assert!(
        has_column(&narrow, "AGENT") && has_column(&narrow, "STATUS"),
        "{narrow}"
    );
    assert!(!has_column(&narrow, "AGE"), "{narrow}");
}

#[test]
fn a_terminal_narrower_than_one_agents_name_clips_rather_than_wrapping() {
    // Wrapping is what turns a monitor into a mess: one row becoming two moves
    // every row below it. A clipped line is still one line.
    for width in 1..=12 {
        for line in amx::agents::table::render(&wide_reply(), NOW, Some(width)) {
            assert!(
                line.chars().count() <= width,
                "{width} columns, got {line:?}"
            );
        }
    }
}

// -------------------------------------------------------------------- safety

#[test]
fn a_screen_line_carrying_control_bytes_cannot_reach_the_terminal() {
    // `last_line` comes off a cell grid and should hold none of these. But
    // `--watch` writes this table straight to a terminal, and the string
    // ultimately came from whatever an agent printed, so "should" is not a
    // property a monitor gets to rely on.
    let mut entry = agent("api", "backend", AgentState::Working, 30);
    entry.last_line = "before\x1b[2Jafter\r\ndone\u{7}".to_owned();
    let lines = amx::agents::table::render(&reply(vec![entry]), NOW, None);
    assert!(lines[2].contains("before"), "{:?}", lines[2]);
    assert!(lines[2].contains("after"), "{:?}", lines[2]);
    for line in &lines {
        assert!(
            !line.chars().any(char::is_control),
            "a control byte survived: {line:?}"
        );
    }
}

#[test]
fn an_empty_session_says_so_in_one_line_and_prints_no_header() {
    let lines = amx::agents::table::render(&reply(Vec::new()), NOW, Some(45));
    assert_eq!(lines, ["no agents"]);
}

#[test]
fn the_count_line_counts_the_queue_and_not_the_rows_that_look_blocked() {
    // The same discipline X11 pinned on the status line: the number a person
    // reads and the queue the jump key walks are one number. A row whose state
    // says `blocked` but which the queue does not name is a disagreement worth
    // seeing, not one worth papering over with a second tally.
    let blocked = agent("api", "backend", AgentState::Blocked, 60);
    let lines = amx::agents::table::render(
        &ListReply {
            seq: 1,
            now: NOW,
            attention: Vec::new(),
            agents: vec![blocked],
        },
        NOW,
        None,
    );
    assert_eq!(lines[0], "1 agent");
}

#[test]
fn a_scoped_table_counts_the_queue_it_is_showing_and_not_the_whole_of_it() {
    // `--workspace` narrows the rows and deliberately does *not* narrow the
    // queue: a filtered queue would answer a different question than the one
    // `agent.next` acts on. So the count line has to say how many of the panes
    // it is *showing* are waiting, or a two-row table reads "2 agents · 5
    // blocked" — two scopes in one sentence.
    let shown = agent("docs", "writer", AgentState::Blocked, 60);
    let elsewhere = agent("api", "backend", AgentState::Blocked, 120);
    let lines = amx::agents::table::render(
        &ListReply {
            seq: 1,
            now: NOW,
            attention: vec![elsewhere.pane, shown.pane],
            agents: vec![shown],
        },
        NOW,
        None,
    );
    assert_eq!(lines[0], "1 agent · 1 blocked");
}
