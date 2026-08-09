//! The adversarial suite behind M0's exit criteria (05 M0, 06 §4 T18).
//!
//! Everything here is process-level and drives the real `amx` binary over the
//! real socket: kill -9 is a real `SIGKILL`, the second terminal is a second
//! pseudoterminal, the flood is a real `cat /dev/urandom` in a real pane.
//!
//! Grid bytes cross the socket in this build, so "identical grid" and
//! "uncorrupted grid" are claims about content: every compared screen first
//! shows a marker typed into the pane and echoed back through its shell, so
//! two equally blank frames can never satisfy an equality assertion.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::time::{Duration, Instant};

use amx_proto::version::window;
use amx_server::session::probe::{Probe, probe};
use rig::env::processes_with_arg;
use rig::screen::render;
use rig::wire::{split_running, workspace_with_root};
use rig::{
    ALT_ENTER, ALT_LEAVE, Env, PATIENCE, Screen, Wire, rasterize, result_of, shows, wait_until,
};
use serde_json::json;

const ROWS: u16 = 24;
const COLS: u16 = 80;

/// Whether a rasterized screen shows a fully painted frame: the status line
/// padded across the whole bottom row.
fn painted(screen: &Screen) -> bool {
    (0..COLS).all(|col| screen.contains_key(&(ROWS - 1, col)))
}

/// A rasterized screen without its status line: the content area, which is the
/// whole terminal minus one row of chrome.
fn panes_of(screen: &Screen) -> Screen {
    screen
        .iter()
        .filter(|&(&(row, _), _)| row + 1 < ROWS)
        .map(|(&at, &ch)| (at, ch))
        .collect()
}

/// The session id `amx ping` reports for this environment.
fn session_id(env: &Env) -> String {
    let reply: serde_json::Value =
        serde_json::from_str(env.run(&["ping"]).ok()).expect("ping prints the reply as JSON");
    reply["session"]
        .as_str()
        .expect("the reply names the session")
        .to_owned()
}

/// A marker string no other test's processes carry.
fn marker(tag: &str) -> String {
    format!("amx-rig-{tag}-{}", std::process::id())
}

// ------------------------------------------------------------------ kill -9

#[tokio::test]
async fn kill_dash_9_the_client_then_reattach_yields_an_identical_grid() {
    let env = Env::new("kill9");

    // The first client daemonizes the server and paints its frame.
    let mut first = env.attach_on_tty(&[], ROWS, COLS);
    first.wait_for(ALT_ENTER);
    first.wait_output("a fully painted frame", |seen| painted(&rasterize(seen)));
    let session = session_id(&env);

    // Give the session something to lose — a process started by typing into
    // the seeded shell — and the grid something no blank frame fakes. `read`
    // keeps the shell, and the marker in its argv, alive. The command spells
    // its own output `he\ld` so the pty echo of the typed line cannot satisfy
    // a wait for `held` — only the child running proves anything, and the
    // wait must keep draining `first` while it polls: blind, it wedges the
    // client on darwin's shallow pty queue before the typed line finishes
    // arriving (see `Terminal::try_wait_until`).
    let marker = marker("kill9");
    first.type_line(&format!("sh -c 'echo he\\ld; read _' {marker}"));
    first.wait_output("the child's output to render", |seen| shows(seen, "held"));
    first.wait_until("the typed process appears", || {
        processes_with_arg(&marker) == 1
    });
    let before = first.wait_settled();
    assert!(
        render(&before).contains(&marker),
        "the baseline grid is non-blank and holds the marker:\n{}",
        render(&before)
    );

    // kill -9. No detach, no goodbye, no chance to clean up.
    first.kill_dash_9();

    // The server shrugs: still answering, same session, pane untouched.
    assert_eq!(probe(&env.socket()).expect("probe"), Probe::Running);
    assert_eq!(
        session_id(&env),
        session,
        "the same server instance answers"
    );
    assert_eq!(
        processes_with_arg(&marker),
        1,
        "the pane's process survived its client"
    );

    // Reattach from a fresh terminal of the same size: the same screen, cell
    // for cell — not merely "a" screen.
    let mut second = env.attach_on_tty(&[], ROWS, COLS);
    second.wait_for(ALT_ENTER);
    second.wait_output("the reattached screen to match the first", |seen| {
        rasterize(seen) == before
    });
    assert_eq!(
        rasterize(second.output()),
        before,
        "reattach must land on the identical screen:\nfirst:\n{}\nsecond:\n{}",
        render(&before),
        render(&rasterize(second.output()))
    );

    // And this client can leave the civilised way.
    second.chord(b'd');
    assert_eq!(second.wait(), Some(0), "detach is a clean exit");
    assert_eq!(
        processes_with_arg(&marker),
        1,
        "detaching leaves the pane running"
    );
}

// --------------------------------------------------------- second terminal

#[tokio::test]
async fn reattach_from_a_second_terminal_while_the_first_is_live() {
    let env = Env::new("two-terms");

    let mut first = env.attach_on_tty(&[], ROWS, COLS);
    first.wait_for(ALT_ENTER);
    first.wait_output("a fully painted frame", |seen| painted(&rasterize(seen)));
    first.type_line("printf 'ok-%s\\n' two-terms-mark");
    first.wait_output("the marker to render", |seen| {
        shows(seen, "ok-two-terms-mark")
    });
    let screen = first.wait_settled();
    assert!(
        render(&screen).contains("ok-two-terms-mark"),
        "the shared baseline is non-blank"
    );

    // The second terminal attaches while the first is still attached — no
    // takeover, no eviction, both connections live at once.
    let mut second = env.attach_on_tty(&[], ROWS, COLS);
    second.wait_for(ALT_ENTER);
    second.wait_output("the second terminal to paint the same screen", |seen| {
        rasterize(seen) == screen
    });
    assert!(first.alive(), "the first client must not be evicted");

    // The server still answers a third party while serving both.
    let mut wire = Wire::connect(&env.socket()).await;
    wire.hello(window()).await;
    let reply = wire.request("ping", json!({})).await;
    assert!(result_of(&reply)["seq"].is_u64());

    // Each detaches cleanly, in either order, restoring its own terminal.
    second.chord(b'd');
    assert_eq!(second.wait(), Some(0));
    assert!(
        rig::term::contains(second.output(), ALT_LEAVE),
        "the second client left the alternate screen"
    );
    assert_eq!(second.termios(), second.initial_termios());

    assert!(first.alive(), "one detach must not take the other client");
    first.chord(b'd');
    assert_eq!(first.wait(), Some(0));
    assert_eq!(first.termios(), first.initial_termios());

    assert_eq!(probe(&env.socket()).expect("probe"), Probe::Running);
}

// ------------------------------------------------------------- flow control

/// What "bounded" means for the whole server process under a flood: however
/// long the pane runs, resident memory stops growing once the scrollback
/// budget is full. Generous on purpose — the point is not the constant but
/// that it does not scale with the flood.
const RSS_GROWTH_BOUND: u64 = 64 * 1024 * 1024;

/// How fast the flood must be reaching the server, per second of *observed*
/// time, before its steady memory is worth asserting anything about.
///
/// A rate, not a quantity (DR-19). A quantity is a claim about the machine:
/// these suites run under `cargo test --workspace` and a flood given a sixth of
/// a core delivers a sixth of the bytes, so any fixed number of megabytes is a
/// throughput threshold wearing a memory bound's clothes. It failed as one —
/// `docs/notes/m3-shutdown-wedge.md` records 7923383 bytes against an 8 MiB
/// line on every round of a 35-minute field run under eight-way load.
///
/// The floor is what "the flood is flowing at all" costs. That same run put the
/// *loaded* rate at about 2.6 MB/s (its 7923383 bytes over a three-second
/// window), and this is twenty times under it. A slower machine observes for
/// longer and proves the same pair of facts; only a flood that has stopped
/// fails.
const FLOOD_RATE: u64 = 128 * 1024;

/// How long the memory bound is watched before the rate is judged.
const FLOOD_OBSERVE: Duration = Duration::from_secs(3);

/// Whether `ingested` bytes over `observed` time clears [`FLOOD_RATE`].
///
/// Split out so the decision can be exercised directly: the loop below cannot
/// be made to run at a chosen speed, and a predicate that is only ever read
/// through a live pty is a predicate nobody has checked.
fn flooding(ingested: u64, observed: Duration) -> bool {
    let micros = u64::try_from(observed.as_micros()).unwrap_or(u64::MAX);
    ingested.saturating_mul(1_000_000) >= FLOOD_RATE.saturating_mul(micros)
}

#[test]
fn the_flood_bound_is_a_rate_and_not_a_machine_speed() {
    // A sixth of a core still delivers: slow, steady, and over the floor for
    // however long the observation runs.
    let slow = FLOOD_RATE * 2;
    for seconds in [3, 10, 60, 600] {
        assert!(
            flooding(slow * seconds, Duration::from_secs(seconds)),
            "a flood at {slow} B/s must clear the floor over {seconds}s"
        );
    }

    // A flood that stopped does not become acceptable by being waited on —
    // which is what a fixed quantity plus a deadline lets happen.
    let burst = 64 * 1024 * 1024;
    assert!(flooding(burst, FLOOD_OBSERVE));
    assert!(!flooding(burst, Duration::from_secs(10_000)));

    // And nothing arriving is nothing arriving, at any duration.
    assert!(!flooding(0, FLOOD_OBSERVE));
}

#[tokio::test]
async fn flow_control_urandom_pane_with_stalled_client_bounds_memory_and_preserves_grids() {
    let env = Env::new("flood");
    let server = env.server();

    // A real client watching the seeded workspace throughout, with content of
    // its own on screen so "uncorrupted" is a claim about something. It
    // attaches before the flood's workspace exists, so its screen owes the
    // flood pane nothing.
    let mut live = env.attach_on_tty(&[], ROWS, COLS);
    live.wait_for(ALT_ENTER);
    live.wait_output("a fully painted frame", |seen| painted(&rasterize(seen)));
    live.type_line("printf 'ok-%s\\n' flood-mark");
    live.wait_output("the marker to render", |seen| shows(seen, "ok-flood-mark"));
    let baseline_screen = live.wait_settled();
    assert!(
        render(&baseline_screen).contains("ok-flood-mark"),
        "the watched screen is non-blank before the flood"
    );

    // A control-plane client that keeps behaving.
    let mut control = Wire::connect(&env.socket()).await;
    control.hello(window()).await;
    let (workspace, root) = workspace_with_root(&mut control).await;

    // The adversarial pane from the M0 exit criterion, in its own workspace.
    let marker = marker("flood");
    let _flood = split_running(
        &mut control,
        root,
        &["sh", "-c", "cat /dev/urandom; true", &marker],
    )
    .await;
    wait_until("the flood starts", || processes_with_arg(&marker) == 1);

    // A stalled client: it attaches, fires off calls, and then reads nothing
    // at all while the flood runs.
    let mut stalled = Wire::connect(&env.socket()).await;
    stalled.hello(window()).await;
    for id in 0..32_u64 {
        stalled.send_request(100 + id, "ping", json!({})).await;
    }

    // Observe: one second of warm-up, then the bound holds while the flood
    // keeps running. The interval is a sampling cadence, not a wait — every
    // tick re-checks real state, and the control connection must answer with
    // monotonic sequence numbers the entire time.
    //
    // Two things end the loop, and neither is a clock alone: three seconds of
    // observation, *and* a flood still flowing over that window at a rate no
    // machine speed decides ([`FLOOD_RATE`]). Both halves have to be there — a
    // window alone measures throughput, and a quantity alone lets a flood that
    // stopped a minute ago satisfy the bound it is supposed to make mean
    // something.
    let mut ticker = tokio::time::interval(Duration::from_millis(50));
    let start = Instant::now();
    let ingested_before = server.read_bytes();
    // Where the kernel keeps no io accounting the number does not exist, so
    // the observation window is all there is; say so out loud rather than
    // failing on a probe gap.
    let accounted = ingested_before.is_some();
    if !accounted {
        eprintln!("io accounting unavailable on this platform; ingestion bound not checked");
    }
    let deadline = Instant::now() + PATIENCE;
    let mut baseline_rss = None;
    let mut peak: u64 = 0;
    let mut last_seq = 0_u64;
    let mut ingested = 0_u64;
    loop {
        ticker.tick().await;
        let rss = server.rss_bytes();
        if start.elapsed() >= Duration::from_secs(1) {
            let base = *baseline_rss.get_or_insert(rss);
            peak = peak.max(rss);
            assert!(
                peak.saturating_sub(base) < RSS_GROWTH_BOUND,
                "server RSS grew {} bytes past its post-warm-up baseline of {base}",
                peak - base
            );
        }
        let reply = control.request("ping", json!({})).await;
        let seq = result_of(&reply)["seq"].as_u64().expect("ping carries seq");
        assert!(
            seq >= last_seq,
            "sequence numbers went backwards under load"
        );
        last_seq = seq;
        assert_eq!(
            processes_with_arg(&marker),
            1,
            "the flood must keep running"
        );

        // The bound above means nothing unless the flood really flowed: the
        // server must have read megabytes off the pane's pty while its memory
        // stood still.
        if let (Some(before), Some(after)) = (ingested_before, server.read_bytes()) {
            ingested = after.saturating_sub(before);
        }
        let observed = start.elapsed();
        let flowing = !accounted || flooding(ingested, observed);
        if observed >= FLOOD_OBSERVE && flowing {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "over {observed:?} the server ingested {ingested} bytes of the flood, \
             under the {FLOOD_RATE} B/s that says it is flowing at all; the flood \
             never reached it",
        );
    }

    // The stalled client was neither disconnected nor corrupted: every reply
    // it never read is still there, in order, when it finally reads.
    for _ in 0..32 {
        let reply = stalled.read_response().await;
        assert!(
            result_of(&reply)["seq"].is_u64(),
            "a stalled client's replies must survive the flood intact"
        );
    }

    // No attached client's grid was corrupted by three seconds of urandom: the
    // live client still shows exactly the panes it painted, and detaches
    // cleanly, terminal restored.
    //
    // The panes and not the whole screen, because the status line is
    // session-wide by design: D15's breakdown names *every* workspace, so the
    // flood's own workspace joins it on every attached client the moment it is
    // created (`amx-client/src/app/status.rs`). That is a chrome line following
    // session state, which is what it is for; what owes the flood pane nothing
    // is the grid this client is drawing, which is what "uncorrupted" has
    // always been about here.
    live.drain();
    assert_eq!(
        panes_of(&rasterize(live.output())),
        panes_of(&baseline_screen),
        "the flood must not leak into another client's panes"
    );
    live.chord(b'd');
    assert_eq!(live.wait(), Some(0), "the live client detaches cleanly");
    assert_eq!(live.termios(), live.initial_termios());

    // Tear the flood down through the same wire surface and prove it died.
    let killed = control
        .request("workspace.kill", json!({ "workspace": workspace }))
        .await;
    assert!(result_of(&killed)["panes"].is_array());
    wait_until("the flood is reaped", || processes_with_arg(&marker) == 0);

    server.shutdown();
}

// ------------------------------------------------------------ 0×0 viewport

#[tokio::test]
async fn a_zero_size_viewport_still_gets_a_live_projection() {
    let env = Env::new("zeroview");
    let server = env.server();

    // The degenerate declaration a client on a 0×0 pty really sends —
    // python's `pty.fork` reports exactly this size. It must be clamped into
    // a live projection, not dropped: dropped, the session sits dark with no
    // indication until a real size happens to arrive.
    let mut wire = Wire::connect(&env.socket()).await;
    wire.hello(window()).await;
    let (_workspace, _root) = workspace_with_root(&mut wire).await;
    let reply = wire
        .request(
            "client.viewport",
            json!({"rows": 0, "cols": 0, "panes": []}),
        )
        .await;
    assert!(
        result_of(&reply)["seq"].is_u64(),
        "a degenerate viewport is clamped, not refused"
    );

    // The root pane is projected under the clamped default grid: the
    // interior a 24×80 client gets (one status line, a one-cell border).
    let want = (24 - 1 - 2, 80 - 2);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let state = wire.request("session.state", json!({})).await;
        let panes = result_of(&state)["panes"]
            .as_array()
            .expect("state lists panes")
            .clone();
        if panes.iter().any(|pane| {
            (pane["rows"].as_u64(), pane["cols"].as_u64()) == (Some(want.0), Some(want.1))
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the seeded pane never took the clamped projection: {panes:?}"
        );
    }

    server.shutdown();
}

// ------------------------------------------------------------ mid-frame close

#[tokio::test]
async fn server_survives_a_client_that_closes_mid_frame() {
    let env = Env::new("midframe");
    let mut server = env.server();

    // Gone mid-payload, after a clean handshake.
    let mut wire = Wire::connect(&env.socket()).await;
    wire.hello(window()).await;
    wire.send_partial_frame(4096, &[b'{'; 64]).await;
    wire.abandon();

    // Gone mid-header, before ever saying hello.
    let mut wire = Wire::connect(&env.socket()).await;
    wire.send_bytes(&[0xff, 0x00, 0x00]).await;
    wire.abandon();

    // Gone mid-hello: the declared payload never finishes arriving.
    let mut wire = Wire::connect(&env.socket()).await;
    wire.send_partial_frame(512, br#"{"proto":[1,1],"#).await;
    wire.abandon();

    // After all three, the next client gets a full-service session.
    let mut next = Wire::connect(&env.socket()).await;
    let welcome = next.hello(window()).await;
    assert_eq!(welcome.proto, amx_proto::version::PROTO_MAX);
    let reply = next.request("ping", json!({})).await;
    assert!(result_of(&reply)["seq"].is_u64());
    assert!(server.alive(), "the server outlived every rude client");

    server.shutdown();
}
