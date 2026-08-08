//! The rig M3's exit criterion runs on: a live session, an attached client, and
//! the two long-lived consumers that must not notice a swap.
//!
//! Split from the suite for the module budget, and along the seam that keeps
//! both readable: everything here is *scaffolding around a real session* — a
//! child process holding a standing call, a relay reading NDJSON, and the two
//! or three questions a test asks of the process table. The assertions live
//! next door.
//!
//! Nothing here models anything. The session is a real `amx server`, the
//! client is the real `amx attach` on a real pseudoterminal, the consumers are
//! the real `amx wait` and `amx events --json`, and the swap is the real
//! `amx session handoff` against the real binary.

#![allow(dead_code, reason = "the suite uses a subset of the rig")]

use std::fs::File;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::PathBuf;
use std::process::{Child, Stdio};

use rig::{Env, Wire};

/// A child `amx` whose output is captured to files under the environment root.
///
/// Files rather than pipes, for the reason the M3 client-reconnect suite gives:
/// a test that has to read a long-running child's output *while it runs* cannot
/// read a pipe without blocking on it, and polling a file is the same
/// observation without the deadlock.
#[derive(Debug)]
pub struct Standing {
    child: Child,
    out: PathBuf,
    err: PathBuf,
    what: String,
}

impl Standing {
    /// Start `args` in the background, capturing both streams.
    pub fn start(env: &Env, tag: &str, args: &[&str]) -> Self {
        let out = env.home().join(format!("{tag}.out"));
        let err = env.home().join(format!("{tag}.err"));
        let child = env
            .command()
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(File::create(&out).expect("create stdout")))
            .stderr(Stdio::from(File::create(&err).expect("create stderr")))
            .spawn()
            .expect("spawn amx");
        Self {
            child,
            out,
            err,
            what: format!("amx {}", args.join(" ")),
        }
    }

    /// Whether it is still running.
    pub fn running(&mut self) -> bool {
        self.child.try_wait().expect("try_wait").is_none()
    }

    /// What it has written to stdout so far.
    #[must_use]
    pub fn stdout(&self) -> String {
        std::fs::read_to_string(&self.out).unwrap_or_default()
    }

    /// What it has written to stderr so far.
    #[must_use]
    pub fn stderr(&self) -> String {
        std::fs::read_to_string(&self.err).unwrap_or_default()
    }

    /// Every complete line of stdout so far, as JSON.
    #[must_use]
    pub fn deliveries(&self) -> Vec<serde_json::Value> {
        parse(&self.stdout())
    }

    /// Wait until it exits, and answer with its status and both streams.
    pub fn finish(mut self) -> Finished {
        let mut done = None;
        {
            let (child, what, out, err) = (&mut self.child, &self.what, &self.out, &self.err);
            rig::wait_until_or(
                what,
                || {
                    done = child.try_wait().expect("try_wait");
                    done.is_some()
                },
                || {
                    format!(
                        "it has written {:?} and {:?}",
                        std::fs::read_to_string(out).unwrap_or_default(),
                        std::fs::read_to_string(err).unwrap_or_default(),
                    )
                },
            );
        }
        Finished {
            // Present: `wait_until_or` panics rather than return without it.
            code: done.and_then(|status| status.code()),
            stdout: self.stdout(),
            stderr: self.stderr(),
        }
    }

    /// Stop it and read what it wrote.
    pub fn stop(mut self) -> Finished {
        let _ = self.child.kill();
        let _ = self.child.wait();
        Finished {
            code: None,
            stdout: self.stdout(),
            stderr: self.stderr(),
        }
    }
}

/// What a [`Standing`] child did.
#[derive(Debug)]
pub struct Finished {
    /// Its exit code, or `None` if it was killed.
    pub code: Option<i32>,
    /// Everything it wrote to stdout.
    pub stdout: String,
    /// Everything it wrote to stderr.
    pub stderr: String,
}

/// Parse NDJSON, dropping a partial last line.
///
/// The file is being appended to while it is read, so a half-written delivery
/// is a fact about the reader's timing rather than about the stream.
#[must_use]
pub fn parse(text: &str) -> Vec<serde_json::Value> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if !text.ends_with('\n') {
        lines.pop();
    }
    lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|err| panic!("not NDJSON: {line:?} ({err})"))
        })
        .collect()
}

/// A protocol connection to `env`'s session, reached through `amx _bridge`.
///
/// Tier 1 of D-M3-9, exactly as W11 built it: one end of a socketpair is the
/// bridge child's stdin and stdout, the other is an ordinary stream, and
/// nothing above it knows the peer is a subprocess. The child is leaked into
/// the environment's lifetime deliberately — it dies when its socket does, and
/// the test's business is the protocol rather than the process.
pub async fn bridged(env: &Env) -> Wire {
    let (mine, theirs) = StdUnixStream::pair().expect("socketpair");
    let stdin = OwnedFd::from(theirs.try_clone().expect("dup"));
    let stdout = OwnedFd::from(theirs);
    let child = env
        .command()
        .arg("_bridge")
        .arg("--session")
        .arg(&env.session)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn amx _bridge");
    std::mem::forget(child);
    mine.set_nonblocking(true).expect("non-blocking");
    Wire::over(tokio::net::UnixStream::from_std(mine).expect("adopt the socketpair"))
}

/// The sequence numbers an `amx events --json` relay has printed, in order.
///
/// A `gap` contributes its `to`, because that is what the consumer's cursor
/// becomes: the contract is gapless *or gap-marked*, and a gap is a delivery
/// like any other rather than a hole in the record.
#[must_use]
pub fn seqs(deliveries: &[serde_json::Value]) -> Vec<u64> {
    deliveries
        .iter()
        .filter_map(|delivery| match delivery["delivery"].as_str() {
            Some("event") => delivery["seq"].as_u64(),
            Some("gap") => delivery["to"].as_u64(),
            other => panic!("unknown delivery kind {other:?} in {delivery}"),
        })
        .collect()
}

/// Whether any delivery in the relay's record is a gap.
#[must_use]
pub fn gapped(deliveries: &[serde_json::Value]) -> bool {
    deliveries
        .iter()
        .any(|delivery| delivery["delivery"] == "gap")
}
