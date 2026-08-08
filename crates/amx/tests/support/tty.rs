//! A pseudoterminal, and a child process running on it.
//!
//! The half of the harness that is about the terminal itself: the window the
//! client lays out against, the `termios` it borrows and must give back, and
//! the bytes it paints.

use std::fs::File;
use std::io::{ErrorKind, Read as _, Write as _};
use std::path::PathBuf;
use std::process::Child;
use std::time::Instant;

use super::env::{PATIENCE, TICK, window};

/// The alt-screen sequences the client writes on the way in and out.
pub const ALT_ENTER: &[u8] = b"\x1b[?1049h";
/// See [`ALT_ENTER`].
pub const ALT_LEAVE: &[u8] = b"\x1b[?1049l";

/// `ctrl+a`, the prefix key (04 §7).
pub const PREFIX: u8 = 0x01;

/// A pseudoterminal pair.
pub struct Pty {
    /// The side a test reads from and writes to.
    pub master: File,
    /// The side the child process uses as its terminal.
    pub slave: File,
}

/// Open a pty pair sized `rows` by `cols`.
///
/// A freshly opened pty reports a 0x0 window, which starves every layout
/// computation of any area to tile, so the size is set before anything is
/// spawned onto it.
///
/// **Both halves are close-on-exec** (W01), for the reasons the rig's own
/// `open_pty` states at length (`tests/support/term.rs`): without it this
/// harness's terminals are inherited by every `amx` it spawns, and a server
/// that seeded one pane is found holding six pty masters — which is how the
/// shutdown wedge acquired an imaginary "the pane spawn repeated" symptom
/// (`docs/notes/m3-shutdown-wedge.md` §3). The daemonized server the client
/// starts inherits them too, so the leak survives the client.
pub fn open_pty(rows: u16, cols: u16) -> Pty {
    let master = open_master().expect("openpt");
    rustix::pty::grantpt(&master).expect("grantpt");
    rustix::pty::unlockpt(&master).expect("unlockpt");
    let name = rustix::pty::ptsname(&master, Vec::new()).expect("ptsname");
    let slave = File::from(
        rustix::fs::open(
            PathBuf::from(name.to_string_lossy().into_owned()),
            rustix::fs::OFlags::RDWR | rustix::fs::OFlags::NOCTTY | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .expect("open the pty slave"),
    );
    // Sized through the slave, the way openpty(3) does it: the window lives on
    // the terminal, not on a descriptor, and the slave takes TIOCSWINSZ on
    // every platform where darwin's master refuses it until the slave is open.
    set_size(&slave, rows, cols);
    // Non-blocking on the master: a test reads what is there and moves on
    // rather than blocking on a child that has nothing more to say.
    let flags = rustix::fs::fcntl_getfl(&master).expect("getfl");
    rustix::fs::fcntl_setfl(&master, flags | rustix::fs::OFlags::NONBLOCK).expect("setfl");
    Pty {
        master: File::from(master),
        slave,
    }
}

/// The master descriptor, close-on-exec from its first instant where the
/// platform allows it.
///
/// Linux takes the flag in `posix_openpt` itself; elsewhere it goes on with a
/// second call, leaving the same narrow window the server's own
/// `platform::pty::open_master` documents.
#[cfg(target_os = "linux")]
fn open_master() -> Result<std::os::fd::OwnedFd, rustix::io::Errno> {
    rustix::pty::openpt(
        rustix::pty::OpenptFlags::RDWR
            | rustix::pty::OpenptFlags::NOCTTY
            | rustix::pty::OpenptFlags::CLOEXEC,
    )
}

/// See the Linux arm.
#[cfg(not(target_os = "linux"))]
fn open_master() -> Result<std::os::fd::OwnedFd, rustix::io::Errno> {
    let master =
        rustix::pty::openpt(rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY)?;
    rustix::io::fcntl_setfd(&master, rustix::io::FdFlags::CLOEXEC)?;
    Ok(master)
}

fn set_size<Fd: std::os::fd::AsFd>(fd: Fd, rows: u16, cols: u16) {
    rustix::termios::tcsetwinsize(
        fd,
        rustix::termios::Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )
    .expect("set the pty window size");
}

/// The `Debug` form of `fd`'s terminal attributes.
///
/// `Termios` has no `PartialEq`, but its `Debug` form is complete, so two of
/// these compare exactly what "the terminal is how you found it" means.
pub fn termios_of<Fd: std::os::fd::AsFd>(fd: Fd) -> String {
    format!("{:?}", rustix::termios::tcgetattr(fd).expect("tcgetattr"))
}

/// A child process running on a pseudoterminal.
pub struct Terminal {
    pty: Pty,
    child: Child,
    initial: String,
    seen: Vec<u8>,
}

impl Terminal {
    /// Take a freshly spawned child and the terminal it runs on.
    pub fn new(pty: Pty, child: Child, initial: String) -> Self {
        Self {
            pty,
            child,
            initial,
            seen: Vec::new(),
        }
    }

    /// The child's pid.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Everything the child has written so far.
    pub fn output(&self) -> &[u8] {
        &self.seen
    }

    /// The terminal's current attributes.
    ///
    /// Read from the test's own slave descriptor, which is the same terminal
    /// the child put into raw mode.
    pub fn termios(&self) -> String {
        termios_of(&self.pty.slave)
    }

    /// The terminal's attributes before the child was started.
    pub fn initial_termios(&self) -> &str {
        &self.initial
    }

    /// Resize the terminal, which sends the child a `SIGWINCH`.
    pub fn resize(&self, rows: u16, cols: u16) {
        set_size(&self.pty.slave, rows, cols);
    }

    /// Send bytes to the child's stdin.
    pub fn send(&mut self, bytes: &[u8]) {
        self.pty.master.write_all(bytes).expect("write to the pty");
        self.pty.master.flush().expect("flush the pty");
    }

    /// Send the prefix key and then `key`.
    pub fn chord(&mut self, key: u8) {
        self.send(&[PREFIX, key]);
    }

    /// Read whatever is waiting, appending it to [`Self::output`].
    pub fn drain(&mut self) {
        let mut buf = [0_u8; 4096];
        loop {
            match self.pty.master.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => self.seen.extend_from_slice(&buf[..n]),
                // WouldBlock: nothing more right now. Other errors are the
                // child having closed the slave, which is equally "no more".
                Err(err) if err.kind() == ErrorKind::WouldBlock => return,
                Err(_) => return,
            }
        }
    }

    /// Read until the output contains `needle`, or fail.
    pub fn wait_for(&mut self, needle: &[u8]) {
        self.wait_output(&String::from_utf8_lossy(needle), |seen| {
            window(seen, needle)
        });
    }

    /// Read until `seen` holds, or fail naming `what` and what was read.
    pub fn wait_output(&mut self, what: &str, mut seen: impl FnMut(&[u8]) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.drain();
            if seen(&self.seen) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what:?} in {} bytes of output:\n{}",
                self.seen.len(),
                String::from_utf8_lossy(&self.seen),
            );
            std::thread::sleep(TICK);
        }
    }

    /// Wait for the child to exit and return its status code.
    pub fn wait(&mut self) -> Option<i32> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.drain();
            match self.child.try_wait().expect("wait") {
                Some(status) => {
                    self.drain();
                    return status.code();
                }
                None => assert!(Instant::now() < deadline, "the client did not exit"),
            }
            std::thread::sleep(TICK);
        }
    }

    /// Kill the child, for a test that has finished with it.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
