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
//! terminal rect itself, so a tool on the other end of an SSH pipe sees the
//! pane's own output at the pane's own size and nothing amx drew around it.
//! [`amx_client::render::grid::blit`] still letterboxes or clips, because the
//! grid is the server's size and the terminal is the terminal's — what is
//! removed here is chrome, not the size contract.

use std::process::ExitCode;

use amx_client::model::PaneGrid;
use amx_client::net::{self, Session};
use amx_client::render::{FrameWriter, grid};
use amx_client::term::{Sigwinch, TerminalGuard, window_size};
use amx_core::{Ctx, PaneId, Rect};
use anyhow::Context as _;
use tokio::io::AsyncReadExt as _;

use crate::cmd::attach::{client_info, flush};
use crate::cmd::detach::{Chord, DETACH_PANE};

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

/// Attach this terminal to one pane until the detach chord.
///
/// The pane's cells are not fetched: the grid stream has no encoding yet
/// (`amx_proto::stream::grid::GridMessage::{encode,decode}` are `todo!()`, and
/// no control method binds a stream to a channel), so what this attaches is the
/// terminal, the connection and the render path — the three things that must be
/// right before there are cells to put through them. The blank grid it paints
/// is the one `apply_reset`/`apply_delta` fill in the moment that codec lands.
pub async fn one_pane(ctx: &Ctx, pane: PaneId, takeover: bool) -> anyhow::Result<ExitCode> {
    let stream = net::connect(&ctx.socket)
        .await
        .context("connect to the session")?;
    let (_session, _welcome) = Session::attach(stream, client_info(), None)
        .await
        .context("negotiate with the session")?;
    // `--takeover` has nothing to send yet: size authority rides
    // `client.viewport`, which is a declared payload type in `amx-proto` with
    // no row in the method table, so there is no call to make. Recorded rather
    // than silently dropped.
    let _ = (pane, takeover);

    let mut term = TerminalGuard::enter(std::io::stdin(), std::io::stdout())
        .context("amx attach needs a terminal on stdin")?;
    let mut out = std::io::stdout();
    let mut sigwinch = Sigwinch::install().context("watch for terminal resizes")?;
    let mut stdin = tokio::io::stdin();
    let mut chord = Chord::new(DETACH_PANE);
    let mut input = [0_u8; 1024];

    let size = term.size().context("read the terminal size")?;
    let mut area = Rect::new(0, 0, size.cols, size.rows);
    let cells = PaneGrid::blank(size.rows, size.cols);
    let mut writer = FrameWriter::new();

    paint(&mut writer, &cells, area);
    flush(&mut out, writer.bytes())?;

    loop {
        tokio::select! {
            read = stdin.read(&mut input) => {
                let Ok(n @ 1..) = read else { break };
                if chord.feed(&input[..n]) {
                    break;
                }
            }
            signal = sigwinch.recv() => {
                let Some(()) = signal else { break };
                let size = window_size(std::io::stdin()).context("read the terminal size")?;
                area = Rect::new(0, 0, size.cols, size.rows);
                paint(&mut writer, &cells, area);
                flush(&mut out, writer.bytes())?;
            }
        }
    }

    term.restore();
    Ok(ExitCode::SUCCESS)
}
