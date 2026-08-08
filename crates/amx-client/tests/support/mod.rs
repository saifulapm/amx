//! Shared scaffolding for the T13 acceptance suite.
//!
//! [`Server`] mirrors `amx-server`'s own `tests/support::Server` (T10's
//! harness) so these tests drive the *real* `Gateway`/`Core`/`Runtime` over a
//! real socket, not a stand-in. [`Pty`] is unrelated: the raw-mode guard
//! tests need a real tty (`tcgetattr`/`tcsetattr` refuse a regular file or a
//! pipe), so it opens one with `rustix::pty`.

#![allow(dead_code, reason = "each test binary uses a subset of the harness")]
#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use amx_core::{Bus, Ctx, Scheduled, SessionName};
use amx_server::actor::CoreHandle;
use amx_server::actor::core::Core;
use amx_server::actor::gateway::{Gateway, GatewayControl, GatewayProbe, GatewayReport};
use amx_server::runtime::{Runtime, ShutdownReport};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// A directory under `$TMPDIR`, removed when the test ends.
pub struct TempDir(PathBuf);

impl TempDir {
    /// A directory nobody else in this process will pick.
    pub fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        // Kept short deliberately: this directory prefixes a unix socket path,
        // and darwin's $TMPDIR alone eats half the sun_path budget (~104
        // bytes). A four-char tag survives for debuggability.
        let brief: String = tag.chars().take(4).collect();
        let path = std::env::temp_dir().join(format!("a{}-{n}-{brief}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create the temp dir");
        Self(path)
    }

    /// Where it is.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A `Ctx` whose paths all live under `root`, with a private bus and token.
pub fn ctx_under(root: &Path) -> Ctx {
    let runtime_dir = root.join("run/amx/test");
    Ctx {
        session: SessionName::new("test").expect("valid session name"),
        socket: runtime_dir.join("sock"),
        runtime_dir,
        state_dir: root.join("state"),
        config_path: root.join("config/amx/config.toml"),
        bus: Arc::new(Bus::new(64)),
        cancel: CancellationToken::new(),
    }
}

/// A real session: a real `Core` actor and a real `Gateway`, on a real
/// socket under a temp runtime dir.
pub struct Server {
    /// The context both actors share.
    pub ctx: Ctx,
    /// Live connection accounting.
    pub probe: GatewayProbe,
    /// Retire and restore this session's socket — W06's handoff seam, and the
    /// only way a test can take a client's connection away without taking the
    /// panes behind it away too.
    pub control: GatewayControl,
    runtime: Runtime,
    report: oneshot::Receiver<GatewayReport>,
    _dir: TempDir,
}

impl Server {
    /// Bind a session socket under a fresh temp directory and start serving.
    pub async fn start(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let ctx = ctx_under(dir.path());
        let mut runtime = Runtime::new(ctx.clone());
        let (probe, control, report) = serve(&mut runtime, &ctx);
        Self {
            ctx,
            probe,
            control,
            runtime,
            report,
            _dir: dir,
        }
    }

    /// The session socket.
    pub fn socket(&self) -> &Path {
        &self.ctx.socket
    }

    /// Retire this session's socket and bind a **different** session on it.
    ///
    /// A fresh `Core` mints a fresh `SessionId` (`core/mod.rs`), so what a
    /// client finds when it redials is a server that has never heard of it —
    /// the other half of the `Welcome.session` branch, and the only half a
    /// retire-and-restore cannot produce.
    ///
    /// The old `Core` is left running and unreachable rather than shut down:
    /// what this models is the socket changing hands, and joining an actor tree
    /// mid-test would be testing `Runtime::shutdown` instead.
    pub async fn usurp(&mut self) {
        self.control.retire().await.expect("retire the socket");
        let (probe, control, report) = serve(&mut self.runtime, &self.ctx);
        self.probe = probe;
        self.control = control;
        self.report = report;
    }

    /// Cancel the session and join every task.
    pub async fn shutdown(self) -> (GatewayReport, ShutdownReport) {
        let shutdown = self.runtime.shutdown().await;
        let gateway = self.report.await.expect("the gateway reported");
        (gateway, shutdown)
    }
}

/// Spawn one `Core` and one `Gateway` on `ctx` into `runtime`.
fn serve(
    runtime: &mut Runtime,
    ctx: &Ctx,
) -> (
    GatewayProbe,
    GatewayControl,
    oneshot::Receiver<GatewayReport>,
) {
    let (core_tx, core_rx) = mpsc::channel(64);
    let core = Core::new(ctx.clone(), CoreHandle::new(core_tx.clone()));
    runtime.spawn("core", async move {
        let _ = core.run(core_rx, |_: &Scheduled| {}).await;
    });

    let gateway =
        Gateway::bind(ctx.clone(), CoreHandle::new(core_tx)).expect("bind the session socket");
    let probe = gateway.probe().clone();
    let control = gateway.control();
    let (report_tx, report) = oneshot::channel();
    runtime.spawn("gateway", async move {
        let _ = report_tx.send(gateway.run().await);
    });
    (probe, control, report)
}

/// A real pseudoterminal pair, opened with `rustix::pty`.
///
/// The raw-mode guard operates on a real tty fd (`tcgetattr`/`tcsetattr` fail
/// on a plain file or a pipe): `slave` is that fd, and `master` is kept alive
/// only because the slave is unusable once nothing holds the master open.
pub struct Pty {
    pub master: std::fs::File,
    pub slave: std::fs::File,
}

/// Open a fresh pty pair at 24x80 — a freshly `posix_openpt`'d pty otherwise
/// reports a 0x0 window, which starves every layout computation in this
/// crate of any area to tile.
pub fn open_pty() -> Pty {
    open_pty_sized(24, 80)
}

/// Open a fresh pty pair at a given size.
pub fn open_pty_sized(rows: u16, cols: u16) -> Pty {
    let master =
        rustix::pty::openpt(rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY)
            .expect("openpt");
    rustix::pty::grantpt(&master).expect("grantpt");
    rustix::pty::unlockpt(&master).expect("unlockpt");
    let name = rustix::pty::ptsname(&master, Vec::new()).expect("ptsname");
    let path = PathBuf::from(name.to_string_lossy().into_owned());
    let slave = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open the pty slave");
    // Sized through the slave, the way openpty(3) does it: the window lives on
    // the terminal, not on a descriptor, and the slave takes TIOCSWINSZ on
    // every platform where darwin's master refuses it until the slave is open.
    rustix::termios::tcsetwinsize(
        &slave,
        rustix::termios::Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )
    .expect("set the pty window size");
    Pty {
        master: std::fs::File::from(master),
        slave,
    }
}

/// The `Debug` form of `fd`'s current terminal attributes, for before/after
/// comparison — `Termios` has no `PartialEq`, but it does have `Debug`.
pub fn termios_snapshot<Fd: std::os::fd::AsFd>(fd: &Fd) -> String {
    format!("{:?}", rustix::termios::tcgetattr(fd).expect("tcgetattr"))
}

/// Reconstruct a `(row, col) -> char` map from `App::frame()`'s ANSI bytes,
/// tracking only the two sequences this crate's `FrameWriter` ever emits:
/// `CSI row;col H` (cursor position, 1-based) and `CSI ... m` (SGR, which
/// moves nothing). Anything that is not part of an escape sequence is a cell.
pub fn rasterize(bytes: &[u8]) -> HashMap<(u16, u16), char> {
    let text = std::str::from_utf8(bytes).expect("frame bytes are valid utf-8");
    let mut cells = HashMap::new();
    let mut row = 0_u16;
    let mut col = 0_u16;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            cells.insert((row, col), c);
            col += 1;
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
        }
        let mut params = String::new();
        let mut terminator = '\0';
        for c2 in chars.by_ref() {
            if c2.is_ascii_digit() || c2 == ';' {
                params.push(c2);
            } else {
                terminator = c2;
                break;
            }
        }
        if terminator == 'H' {
            let mut parts = params.split(';');
            let r: u16 = parts.next().unwrap_or("1").parse().unwrap_or(1);
            let c: u16 = parts.next().unwrap_or("1").parse().unwrap_or(1);
            row = r.saturating_sub(1);
            col = c.saturating_sub(1);
        }
    }
    cells
}
