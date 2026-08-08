//! Spike-only helper for X01, the mouse-path spike (`docs/notes/m4-mouse-path.md`).
//!
//! `raw_mode_probe` is the precedent: a small binary that takes a real
//! terminal and reports what it sees. This one asks the host terminal for
//! mouse reporting, prints every byte that comes back verbatim, and puts the
//! modes back the way it found them on the way out.
//!
//! It exists because the question X01 answers cannot be answered from a unit
//! test. `amx-client` never asks its host terminal for mouse reports at all
//! (`term.rs`'s `ALT_SCREEN_ENTER` is the whole of what entering writes), so
//! nothing in the tree has ever observed one arriving. This binary is the
//! observation.
//!
//! Usage:
//!
//! ```text
//! mouse_probe [--modes 1000,1006] [--seconds 20] [--log PATH] [--alt] [--query]
//! ```
//!
//! `--modes` is a comma-separated list of DEC private mode numbers to set on
//! entry and reset on exit, in the order given (reset happens in reverse); an
//! empty list asks for nothing, which is the baseline every other run is
//! measured against. `--alt` additionally enters the alternate screen, which
//! is what a real `amx attach` does; it is off by default so the transcript
//! survives the probe's exit on a terminal a human is watching. `--query`
//! issues DECRQM (`CSI ? Ps $ p`) three times — before the modes are set,
//! after, and after they are reset — so a terminal that answers says out loud
//! what its defaults were, whether it took the request, and whether it gave
//! the mode back. That is an observation a headless box can make of a real
//! emulator, because it needs no pointer: the window can open, answer and
//! close without anyone touching a mouse.
//!
//! Everything the probe writes to the terminal is also echoed to the log, so
//! the recorded transcript is self-describing: what was asked for, and what
//! came back.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "spike-only probe, never shipped"
)]

mod decode;
mod modes;

use std::io::Write as _;
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt as _;

use decode::{classify, escape, hex};
use modes::{Guard, disable_bytes, enable_bytes, query_bytes};

/// The default modes: button reporting plus the SGR encoding `mouse::scan`
/// recognises. `1002`/`1003` are opt-in because they add motion traffic.
const DEFAULT_MODES: &[u16] = &[1000, 1006];

/// How long the probe listens before restoring and exiting, by default.
const DEFAULT_SECONDS: u64 = 20;

/// How long a DECRQM phase waits for the terminal's answers.
const QUERY_WINDOW: Duration = Duration::from_millis(300);

/// What the command line asked for.
struct Args {
    modes: Vec<u16>,
    seconds: u64,
    log: Option<String>,
    alt: bool,
    query: bool,
}

/// Parse the command line, or explain what went wrong.
fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        modes: DEFAULT_MODES.to_vec(),
        seconds: DEFAULT_SECONDS,
        log: None,
        alt: false,
        query: false,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--modes" => {
                let value = argv.next().ok_or("--modes needs a value")?;
                // An empty list is a real request: it asks the terminal for
                // nothing at all, which is what `amx attach` does today and
                // therefore the baseline every other run is measured against.
                if value.is_empty() {
                    args.modes = Vec::new();
                    continue;
                }
                args.modes = value
                    .split(',')
                    .map(|mode| mode.trim().parse::<u16>().map_err(|_| "bad mode number"))
                    .collect::<Result<_, _>>()?;
            }
            "--seconds" => {
                let value = argv.next().ok_or("--seconds needs a value")?;
                args.seconds = value.parse().map_err(|_| "bad --seconds")?;
            }
            "--log" => args.log = Some(argv.next().ok_or("--log needs a path")?),
            "--alt" => args.alt = true,
            "--query" => args.query = true,
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

/// Both sinks: the terminal a human is watching and the log a run leaves
/// behind.
struct Transcript {
    log: Option<std::fs::File>,
}

impl Transcript {
    fn line(&mut self, text: &str) {
        // `\r\n`: raw mode means no output post-processing, so a bare `\n`
        // would stairstep down the screen.
        print!("{text}\r\n");
        let _ = std::io::stdout().flush();
        if let Some(log) = self.log.as_mut() {
            let _ = writeln!(log, "{text}");
        }
    }
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("mouse_probe: {message}");
            std::process::exit(2);
        }
    };

    let log = args.log.as_ref().map(|path| {
        std::fs::File::create(path).unwrap_or_else(|err| panic!("create {path}: {err}"))
    });
    let mut transcript = Transcript { log };

    let enable = enable_bytes(&args.modes);
    let disable = disable_bytes(&args.modes);
    let query = query_bytes(&args.modes);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");

    rt.block_on(async {
        let stdin = std::io::stdin();
        let mut guard = Guard::enter(&stdin, args.alt);

        transcript.line(&format!("modes:   {:?}", args.modes));
        transcript.line(&format!("enable:  {}  ({})", escape(&enable), hex(&enable)));
        transcript.line(&format!(
            "disable: {}  ({})",
            escape(&disable),
            hex(&disable)
        ));
        if args.query {
            transcript.line(&format!("query:   {}  ({})", escape(&query), hex(&query)));
        }

        let started = Instant::now();
        let mut input = tokio::io::stdin();

        // Phase 1: what the terminal's defaults are, before anything is asked
        // of it. `1007` is the interesting one — a terminal that reports it
        // set is already translating the wheel to arrow keys on the alternate
        // screen, which is a byte arriving with no mouse mode requested at all.
        if args.query {
            write_out(&query);
            transcript.line("-- DECRQM before any mode is set --");
            listen(&mut input, &mut transcript, started, QUERY_WINDOW).await;
        }

        write_out(&enable);
        if args.query {
            write_out(&query);
            transcript.line("-- DECRQM after the modes are set --");
            listen(&mut input, &mut transcript, started, QUERY_WINDOW).await;
        }

        transcript.line(&format!(
            "listening for {}s; press q to stop. Scroll, click, and try to \
             drag-select this text.",
            args.seconds
        ));
        listen(
            &mut input,
            &mut transcript,
            started,
            Duration::from_secs(args.seconds),
        )
        .await;

        write_out(&disable);
        if args.query {
            write_out(&query);
            transcript.line("-- DECRQM after the modes are reset --");
            listen(&mut input, &mut transcript, started, QUERY_WINDOW).await;
        }
        transcript.line("restored");
        guard.restore();
    });

    // The last `listen` almost always leaves a read parked on the blocking
    // pool — `tokio::io::stdin` reads on a blocking thread and a cancelled
    // timeout cancels the future, not the syscall. Dropping the runtime waits
    // for that thread, so the probe would hang after restoring the terminal
    // rather than exiting. Shutting the runtime down without waiting is the
    // whole fix: the terminal is already restored above, so there is nothing
    // left that a parked read could still be owed.
    rt.shutdown_background();
}

/// Write `bytes` to the terminal and flush.
fn write_out(bytes: &[u8]) {
    let mut out = std::io::stdout();
    let _ = out.write_all(bytes);
    let _ = out.flush();
}

/// Record everything the terminal sends for `window`, or until `q`.
///
/// Returns once the window closes, the input ends, or a quit byte arrives.
/// Every read is one transcript line: the raw bytes are the record and the
/// classification beside them is this probe's reading of them, kept visibly
/// separate so the note can quote the bytes rather than the reading.
async fn listen(
    input: &mut tokio::io::Stdin,
    transcript: &mut Transcript,
    started: Instant,
    window: Duration,
) {
    let until = Instant::now() + window;
    let mut buf = [0u8; 512];
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        let read = match tokio::time::timeout(left, input.read(&mut buf)).await {
            Err(_) => return,
            Ok(Ok(0)) => return,
            Ok(Ok(n)) => n,
            Ok(Err(err)) => {
                transcript.line(&format!("read error: {err}"));
                return;
            }
        };
        let chunk = &buf[..read];
        transcript.line(&format!(
            "[{:>7.3}s] {read:>3}B  {}  \"{}\"  {}",
            started.elapsed().as_secs_f64(),
            hex(chunk),
            escape(chunk),
            classify(chunk),
        ));
        if chunk.contains(&b'q') || chunk.contains(&0x03) {
            return;
        }
    }
}
