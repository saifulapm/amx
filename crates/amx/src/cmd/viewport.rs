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

use std::collections::HashMap;
use std::process::ExitCode;

use amx_client::model::{ClientModel, PaneGrid};
use amx_client::net::{self, Session};
use amx_client::render::{FrameWriter, grid};
use amx_client::stream::{self, Bindings};
use amx_client::term::{Sigwinch, TerminalGuard, window_size};
use amx_core::{Ctx, PaneId, Rect};
use amx_proto::control::{client as client_proto, session, stream as stream_proto};
use amx_proto::stream::{RawDirection, RawPaneIo, StreamKind};
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

/// Attach this terminal to one pane until the detach chord (prefix+q).
pub async fn one_pane(ctx: &Ctx, pane: PaneId, takeover: bool) -> anyhow::Result<ExitCode> {
    let socket = net::connect(&ctx.socket)
        .await
        .context("connect to the session")?;
    let (mut session, _welcome) = Session::attach(socket, client_info(), true, None)
        .await
        .context("negotiate with the session")?;

    // The pane must exist before this terminal is touched: attaching to a
    // typo'd id must fail with an error, not a blank screen.
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

    let mut model = ClientModel::new(0, 0);
    let mut caches = HashMap::new();
    let mut bindings = Bindings::new();
    bind(&mut session, &mut bindings, StreamKind::PaneGrid { pane })
        .await
        .context("bind the pane's grid stream")?;
    let raw_channel = bind(&mut session, &mut bindings, StreamKind::RawPaneIo { pane })
        .await
        .context("bind the pane's input stream")?;

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
        declare_viewport(&mut session, pane, size.rows, size.cols).await?;
    }

    paint(&mut writer, model.pane_mut(pane, 0, 0), area);
    flush(&mut out, writer.bytes())?;

    loop {
        tokio::select! {
            read = stdin.read(&mut input) => {
                let Ok(n @ 1..) = read else { break };
                let (bytes, detach) = strip_chord(&mut chord, &input[..n]);
                if !bytes.is_empty() {
                    forward(&mut session, pane, raw_channel, &bytes).await?;
                }
                if detach {
                    break;
                }
            }
            header = session.read_frame_into(&mut frame) => {
                let header = header.context("read from the session")?;
                let changed = stream::apply(&mut model, &mut caches, &bindings, header, &frame);
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
                    declare_viewport(&mut session, pane, size.rows, size.cols).await?;
                }
                paint(&mut writer, model.pane_mut(pane, 0, 0), area);
                flush(&mut out, writer.bytes())?;
            }
        }
    }

    term.restore();
    Ok(ExitCode::SUCCESS)
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
