//! `amx attach --pane`: one pane, full screen, no chrome (04 §1).
//!
//! "Single-pane attach: `amx attach --pane <target> [--takeover]` attaches the
//! current terminal full-screen to one pane with no chrome (a degenerate
//! one-pane client viewport), detach with prefix+q — herdr's direct
//! terminal-attach mode, kept. This is how you hand one agent's terminal to a
//! plain SSH window or another tool."
//!
//! "No chrome" is a property of [`paint`] and is the whole point of the mode:
//! no border, no status line, no inset. The pane's grid is blitted against the
//! terminal rect itself, and every byte typed — minus the one detach chord —
//! goes to the pane's PTY verbatim over the bound raw stream.
//!
//! `--takeover` claims size authority: the terminal's own size is declared via
//! `client.viewport`, so the pane's PTY resizes to it (04 §3's
//! most-recently-active-client rule). Without it this client observes at the
//! pane's current size, letterboxed or clipped.
//!
//! # Riding a swap (DR-16)
//!
//! This is an attached client, so a handoff takes its socket away exactly as it
//! takes the full client's — §3 step 11 retires the gateway and the successor
//! binds the path a moment later. M3 taught the full client to redial and left
//! this one ending on the `Closed`, which is the register's second reconnect
//! residual. [`reopen`] is the answer, and it is deliberately the *small* one:
//! the redial itself is [`reconnect::dial_until`], shared with the full client
//! so both agree on how long a swap may take, and everything after it is a
//! fresh bind rather than a resumed one.
//!
//! Nothing is claimed across the redial. A single-pane attach holds no bus
//! cursor to resume from and presents no grid generation, so the successor
//! answers the rebind with a `First` keyframe and this terminal is repainted
//! whole — one keyframe to be sure instead of a claim nothing here is in a
//! position to make.
//!
//! What *is* checked is that the pane is still there, by the same
//! `session.state` read the first connect makes. A handoff keeps pane ids, and
//! so does a restore from disk, so "the same pane" survives both — but a
//! successor that came back without it is a session this terminal has no
//! business drawing, and saying so beats a blank screen.

use std::collections::HashMap;
use std::process::ExitCode;

use amx_client::app::reconnect::{self, ReconnectPolicy};
use amx_client::model::{ClientModel, PaneGrid};
use amx_client::net::{self, NetError, Session};
use amx_client::render::{FrameWriter, grid};
use amx_client::stream::{self, Bindings};
use amx_client::term::{Sigwinch, TerminalGuard, window_size};
use amx_core::{Ctx, PaneId, Rect};
use amx_proto::control::{client as client_proto, session, stream as stream_proto};
use amx_proto::stream::{RawDirection, RawPaneIo, StreamKind};
use amx_proto::{ClientInfo, Resume};
use anyhow::Context as _;
use serde_json::json;
use tokio::io::AsyncReadExt as _;

use crate::cmd::attach::{client_info, flush};
use crate::cmd::detach::{Chord, DETACH_PANE, PREFIX};

/// Paint `grid` into `area` with no chrome at all.
///
/// The cursor is placed where the pane's cursor is and shown or hidden as the
/// pane asks, because a viewport with no chrome *is* the pane: there is no
/// status line for the cursor to be parked on instead.
pub fn paint(writer: &mut FrameWriter, grid: &PaneGrid, area: Rect) {
    writer.begin_frame();
    grid::blit(writer, grid, area);

    let cursor = grid.cursor();
    let rows = grid.rows().min(area.h);
    let cols = grid.cols().min(area.w);
    if cursor.visible && cursor.row < rows && cursor.col < cols {
        writer.move_to(area.y + cursor.row, area.x + cursor.col);
        writer.set_cursor_visible(true);
    } else {
        writer.set_cursor_visible(false);
    }
}

/// Everything one connection to the session owns: the socket, and the channels
/// it handed out.
///
/// A struct because a redial replaces all three at once and nothing may
/// survive it — the channel numbers are the *server's* to hand out, and a
/// successor's have no relation to the ones a dead table holds.
struct Wired {
    session: Session,
    bindings: Bindings,
    raw_channel: u8,
}

/// Connect, negotiate, verify the pane still exists, and bind its two streams.
///
/// `resume` is `Resume::default()` on every connection this command makes:
/// see the module header.
async fn wire(ctx: &Ctx, pane: PaneId, client: &ClientInfo) -> anyhow::Result<Wired> {
    let socket = net::connect(&ctx.socket)
        .await
        .context("connect to the session")?;
    let (session, _welcome) = Session::attach(socket, client.clone(), true, None)
        .await
        .context("negotiate with the session")?;
    wire_onto(session, pane).await
}

/// The half of [`wire`] that a redial repeats: the pane check and the binds.
async fn wire_onto(mut session: Session, pane: PaneId) -> anyhow::Result<Wired> {
    // The pane must exist before this terminal is touched: attaching to a
    // typo'd id must fail with an error, not a blank screen. The same read
    // answers the same question after a swap, where the honest failure is a
    // successor that came back without this pane.
    let state = session
        .call("session.state", json!({}))
        .await
        .context("read the session's state")?;
    let state: session::StateReply =
        serde_json::from_value(state).context("decode the session's state")?;
    anyhow::ensure!(
        state.panes.iter().any(|p| p.pane == pane),
        "no such pane in this session: {pane}"
    );

    let mut bindings = Bindings::new();
    bind(&mut session, &mut bindings, StreamKind::PaneGrid { pane })
        .await
        .context("bind the pane's grid stream")?;
    let raw_channel = bind(&mut session, &mut bindings, StreamKind::RawPaneIo { pane })
        .await
        .context("bind the pane's input stream")?;
    Ok(Wired {
        session,
        bindings,
        raw_channel,
    })
}

/// Redial the session and take the pane up again on the successor.
///
/// The deadline is [`ReconnectPolicy`]'s, the same one the full client waits
/// out, so a slow-but-successful upgrade does not end one kind of attach and
/// not the other.
async fn reopen(
    ctx: &Ctx,
    pane: PaneId,
    client: &ClientInfo,
    takeover: bool,
    area: Rect,
) -> anyhow::Result<Wired> {
    let (session, _welcome) = reconnect::dial_until(
        &ctx.socket,
        client,
        &Resume::default(),
        ReconnectPolicy::default(),
        &mut |_| {},
    )
    .await
    .context("the session stopped answering")?;
    let mut wired = wire_onto(session, pane).await?;
    if takeover {
        // The successor has never heard of this terminal, so the declaration
        // is owed again before its first frame rather than at the next winch.
        declare_viewport(&mut wired.session, pane, area.h, area.w).await?;
    }
    Ok(wired)
}

/// Attach this terminal to one pane until the detach chord (prefix+q).
pub async fn one_pane(ctx: &Ctx, pane: PaneId, takeover: bool) -> anyhow::Result<ExitCode> {
    let client = client_info();
    let mut wired = wire(ctx, pane, &client).await?;

    let mut model = ClientModel::new(0, 0);
    let mut caches = HashMap::new();

    let mut term = TerminalGuard::enter(std::io::stdin(), std::io::stdout())
        .context("amx attach needs a terminal on stdin")?;
    let mut out = std::io::stdout();
    let mut sigwinch = Sigwinch::install().context("watch for terminal resizes")?;
    let mut stdin = tokio::io::stdin();
    let mut chord = Chord::new(DETACH_PANE);
    let mut input = [0_u8; 1024];
    let mut frame = Vec::new();
    let mut writer = FrameWriter::new();

    let size = term.size().context("read the terminal size")?;
    let mut area = Rect::new(0, 0, size.cols, size.rows);
    if takeover {
        declare_viewport(&mut wired.session, pane, size.rows, size.cols).await?;
    }

    paint(&mut writer, model.pane_mut(pane, 0, 0), area);
    flush(&mut out, writer.bytes())?;

    loop {
        tokio::select! {
            read = stdin.read(&mut input) => {
                let Ok(n @ 1..) = read else { break };
                let (bytes, detach) = strip_chord(&mut chord, &input[..n]);
                if !bytes.is_empty() {
                    // A keystroke that fell into the swap is dropped, not
                    // replayed: the successor's raw stream is a different
                    // channel, and re-sending bytes whose fate is unknown is
                    // the one thing a terminal must never do.
                    match forward(&mut wired.session, pane, wired.raw_channel, &bytes).await {
                        Ok(()) => {}
                        Err(err) if is_transport(&err) => {
                            wired = reopen(ctx, pane, &client, takeover, area).await?;
                            frame.clear();
                        }
                        Err(err) => return Err(err),
                    }
                }
                if detach {
                    break;
                }
            }
            header = wired.session.read_frame_into(&mut frame) => {
                let header = match header {
                    Ok(header) => header,
                    // The socket ended under the read. Not the end of this
                    // attach: a handoff retires the gateway and the successor
                    // binds the same path a moment later.
                    Err(err) if err.is_transport() => {
                        wired = reopen(ctx, pane, &client, takeover, area).await?;
                        frame.clear();
                        continue;
                    }
                    Err(err) => return Err(anyhow::Error::new(err).context("read from the session")),
                };
                let changed = stream::apply(&mut model, &mut caches, &wired.bindings, header, &frame);
                if changed != stream::Applied::Nothing {
                    paint(&mut writer, model.pane_mut(pane, 0, 0), area);
                    flush(&mut out, writer.bytes())?;
                }
            }
            signal = sigwinch.recv() => {
                let Some(()) = signal else { break };
                let size = window_size(std::io::stdin()).context("read the terminal size")?;
                area = Rect::new(0, 0, size.cols, size.rows);
                if takeover {
                    match declare_viewport(&mut wired.session, pane, size.rows, size.cols).await {
                        Ok(()) => {}
                        Err(err) if is_transport(&err) => {
                            wired = reopen(ctx, pane, &client, takeover, area).await?;
                            frame.clear();
                        }
                        Err(err) => return Err(err),
                    }
                }
                paint(&mut writer, model.pane_mut(pane, 0, 0), area);
                flush(&mut out, writer.bytes())?;
            }
        }
    }

    term.restore();
    Ok(ExitCode::SUCCESS)
}

/// Whether `err` is the connection itself going away, wherever in the chain
/// the `NetError` ended up.
///
/// The calls in this file wear an `anyhow` context, so the question has to be
/// asked of the cause rather than of the top: a swap must reach the redial
/// whether it was noticed by the read, by a keystroke on its way out, or by a
/// viewport declaration a resize made.
fn is_transport(err: &anyhow::Error) -> bool {
    err.chain()
        .filter_map(|cause| cause.downcast_ref::<NetError>())
        .any(NetError::is_transport)
}

/// Bind one stream for `pane`, recording the channel both ways.
async fn bind(
    session: &mut Session,
    bindings: &mut Bindings,
    kind: StreamKind,
) -> anyhow::Result<u8> {
    let params = serde_json::to_value(stream_proto::BindParams {
        kind,
        // A single-pane attach binds once on a fresh connection and has no
        // generation to claim (D-M3-7).
        generation: None,
    })
    .context("encode the bind")?;
    let value = session
        .call("stream.bind", params)
        .await
        .context("bind a stream")?;
    let reply: stream_proto::BindReply =
        serde_json::from_value(value).context("decode the bind reply")?;
    session.bind_channel(reply.channel, reply.max_frame);
    match kind {
        StreamKind::PaneGrid { pane } => bindings.bind_grid(pane, reply.channel),
        StreamKind::RawPaneIo { pane } => bindings.bind_raw(pane, reply.channel),
        StreamKind::History { pane } => bindings.bind_history(pane, reply.channel),
        StreamKind::Graphics { .. } => {}
    }
    Ok(reply.channel)
}

/// Declare this terminal as the pane's size authority.
async fn declare_viewport(
    session: &mut Session,
    pane: PaneId,
    rows: u16,
    cols: u16,
) -> anyhow::Result<()> {
    let params = serde_json::to_value(client_proto::Viewport {
        rows,
        cols,
        panes: vec![pane],
    })
    .context("encode the viewport")?;
    session
        .call("client.viewport", params)
        .await
        .context("declare the viewport")?;
    Ok(())
}

/// Write `bytes` into the pane's PTY over the bound raw stream.
async fn forward(
    session: &mut Session,
    pane: PaneId,
    channel: u8,
    bytes: &[u8],
) -> anyhow::Result<()> {
    let mut payload = Vec::with_capacity(bytes.len() + 17);
    RawPaneIo {
        pane,
        direction: RawDirection::ToPane,
        bytes,
    }
    .encode(&mut payload);
    session
        .write_stream(channel, &payload)
        .await
        .context("forward input to the pane")
}

/// Run the chord over `bytes`, returning the bytes that should still reach
/// the pane and whether the chord completed.
///
/// A pending prefix is held back until the next byte decides it: prefix+q
/// detaches, prefix+anything-else releases both bytes to the pane, and a
/// doubled prefix sends one literal prefix on — the same rules every
/// multiplexer's direct-attach mode uses.
fn strip_chord(chord: &mut Chord, bytes: &[u8]) -> (Vec<u8>, bool) {
    let mut keep = Vec::with_capacity(bytes.len());
    for &byte in bytes {
        let was_armed = chord.is_armed();
        if chord.feed(&[byte]) {
            return (keep, true);
        }
        match (was_armed, chord.is_armed()) {
            // The prefix is pending: hold it until the next byte decides.
            (_, true) if byte == PREFIX && !was_armed => {}
            // A doubled prefix: one literal prefix goes to the pane.
            (true, true) if byte == PREFIX => keep.push(PREFIX),
            // The pending prefix was for the pane after all.
            (true, false) => {
                keep.push(PREFIX);
                keep.push(byte);
            }
            _ => keep.push(byte),
        }
    }
    (keep, false)
}
