//! A pseudoterminal to run the real client on.
//!
//! Same shape as the T17 harness's terminal: the test owns the master side,
//! the client under test gets the slave as its stdin/stdout/stderr, and every
//! wait is a condition over what the client has painted so far.

use std::fs::File;
use std::io::{ErrorKind, Read as _, Write as _};
use std::path::PathBuf;
use std::process::Child;
use std::time::Instant;

use crate::env::{PATIENCE, TICK};

/// The alt-screen sequence the client writes on the way in.
pub const ALT_ENTER: &[u8] = b"\x1b[?1049h";
/// The alt-screen sequence the client writes on the way out.
pub const ALT_LEAVE: &[u8] = b"\x1b[?1049l";

/// `ctrl+a`, the prefix key (04 §7).
pub const PREFIX: u8 = 0x01;

/// A pseudoterminal pair.
#[derive(Debug)]
pub struct Pty {
    /// The side a test reads from and writes to.
    pub master: File,
    /// The side the child process uses as its terminal.
    pub slave: File,
}

/// Open a pty pair sized `rows` by `cols`.
///
/// The size is set before anything is spawned onto it: a freshly opened pty
/// reports 0x0, which starves every layout computation.
#[must_use]
pub fn open_pty(rows: u16, cols: u16) -> Pty {
    let master =
        rustix::pty::openpt(rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY)
            .expect("openpt");
    rustix::pty::grantpt(&master).expect("grantpt");
    rustix::pty::unlockpt(&master).expect("unlockpt");
    let name = rustix::pty::ptsname(&master, Vec::new()).expect("ptsname");
    let slave = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(PathBuf::from(name.to_string_lossy().into_owned()))
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
    // Non-blocking master: a test reads what is there and moves on rather than
    // blocking on a child with nothing more to say.
    let flags = rustix::fs::fcntl_getfl(&master).expect("getfl");
    rustix::fs::fcntl_setfl(&master, flags | rustix::fs::OFlags::NONBLOCK).expect("setfl");
    Pty {
        master: File::from(master),
        slave,
    }
}

/// The `Debug` form of `fd`'s terminal attributes.
///
/// `Termios` has no `PartialEq`, but its `Debug` form is complete, so two of
/// these compare exactly what "the terminal is how you found it" means.
#[must_use]
pub fn termios_of<Fd: std::os::fd::AsFd>(fd: Fd) -> String {
    format!("{:?}", rustix::termios::tcgetattr(fd).expect("tcgetattr"))
}

/// A child process running on a pseudoterminal.
#[derive(Debug)]
pub struct Terminal {
    pty: Pty,
    child: Child,
    initial: String,
    seen: Vec<u8>,
}

impl Terminal {
    /// Wrap a spawned child and the pty it runs on.
    #[must_use]
    pub fn new(pty: Pty, child: Child, initial: String) -> Self {
        Self {
            pty,
            child,
            initial,
            seen: Vec::new(),
        }
    }

    /// The child's pid.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Everything the child has written so far.
    #[must_use]
    pub fn output(&self) -> &[u8] {
        &self.seen
    }

    /// The terminal's current attributes, read from the shared slave.
    #[must_use]
    pub fn termios(&self) -> String {
        termios_of(&self.pty.slave)
    }

    /// The terminal's attributes before the child started.
    #[must_use]
    pub fn initial_termios(&self) -> &str {
        &self.initial
    }

    /// Send bytes to the child's stdin.
    pub fn send(&mut self, bytes: &[u8]) {
        self.pty.master.write_all(bytes).expect("write to the pty");
        self.pty.master.flush().expect("flush the pty");
    }

    /// Type a line of shell input, newline included.
    pub fn type_line(&mut self, line: &str) {
        self.send(line.as_bytes());
        self.send(b"\r");
    }

    /// Resize the terminal the child runs on.
    ///
    /// The winsize change is followed by an explicit `SIGWINCH` to the child:
    /// a real emulator's kernel-side delivery goes to the pty's foreground
    /// process group, but the harness spawns the client without a controlling
    /// tty (no `setsid`), so that group is empty here and the signal must be
    /// sent by hand — same information, same order.
    pub fn resize(&self, rows: u16, cols: u16) {
        rustix::termios::tcsetwinsize(
            &self.pty.slave,
            rustix::termios::Winsize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )
        .expect("resize the pty");
        let pid = rustix::process::Pid::from_raw(self.pid().cast_signed()).expect("a live pid");
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::WINCH);
    }

    /// Read until the rasterized screen holds still: the same cells across
    /// several consecutive polls. Returns that settled screen.
    ///
    /// For "identical grid" comparisons: capturing while a shell is still
    /// printing its prompt would freeze a half-drawn frame as the baseline.
    pub fn wait_settled(&mut self) -> crate::screen::Screen {
        let deadline = Instant::now() + PATIENCE;
        let mut last = crate::screen::rasterize(self.output());
        let mut stable = 0;
        loop {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the screen to settle"
            );
            std::thread::sleep(TICK);
            self.drain();
            let now = crate::screen::rasterize(self.output());
            if now == last && !now.is_empty() {
                stable += 1;
                if stable >= 10 {
                    return now;
                }
            } else {
                stable = 0;
                last = now;
            }
        }
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
                // WouldBlock: nothing more right now. Anything else is the
                // child having closed the slave, which is equally "no more".
                Err(err) if err.kind() == ErrorKind::WouldBlock => return,
                Err(_) => return,
            }
        }
    }

    /// Read until the output contains `needle`, or fail.
    pub fn wait_for(&mut self, needle: &[u8]) {
        self.wait_output(
            &format!(
                "{:?} in the client's output",
                String::from_utf8_lossy(needle)
            ),
            |seen| contains(seen, needle),
        );
    }

    /// Read until `cond` holds over everything painted so far, or fail.
    pub fn wait_output(&mut self, what: &str, mut cond: impl FnMut(&[u8]) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.drain();
            if cond(&self.seen) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what} ({} bytes seen)",
                self.seen.len()
            );
            std::thread::sleep(TICK);
        }
    }

    /// Wait for the child to exit and return its status code.
    pub fn wait(&mut self) -> Option<i32> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.drain();
            match self.child.try_wait().expect("wait for the client") {
                Some(status) => {
                    self.drain();
                    return status.code();
                }
                None => assert!(Instant::now() < deadline, "the client did not exit"),
            }
            std::thread::sleep(TICK);
        }
    }

    /// Whether the child is still running.
    pub fn alive(&mut self) -> bool {
        self.child.try_wait().expect("try_wait").is_none()
    }

    /// Kill the child with `SIGKILL` — the "kill -9 the client" of 05 M0.
    pub fn kill_dash_9(&mut self) {
        self.child.kill().expect("SIGKILL the client");
        let _ = self.child.wait();
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Whether `haystack` contains `needle`.
#[must_use]
pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}
