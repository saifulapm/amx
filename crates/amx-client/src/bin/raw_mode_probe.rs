//! Test-only helper for `tests/term_guard.rs`.
//!
//! Enters raw mode on its stdin, prints a ready marker, and blocks for
//! `SIGTERM`. The point of the exercise is what happens *after* the signal:
//! nothing here catches it or calls `guard.restore()` explicitly — the
//! process returns from `main` normally once the signal future resolves, so
//! `TerminalGuard`'s own `Drop` is what restores the terminal. That is the
//! property `terminal_modes_are_restored_after_sigterm` checks from the
//! parent process, by inspecting the pty after this one has exited.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test-only helper, never shipped"
)]

use std::io::Write as _;

use amx_client::term::TerminalGuard;

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");
    rt.block_on(async {
        // Installed before raw mode is entered and before the ready marker is
        // printed: once this call returns, the process's disposition for
        // `SIGTERM` is already this handler, not the default terminate action
        // — otherwise a signal arriving in the window between printing ready
        // and reaching this line would kill the process outright and skip
        // `guard`'s `Drop` entirely, which is exactly the failure mode this
        // binary exists to prove does not happen.
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");

        let stdin = std::io::stdin();
        let guard = TerminalGuard::enter(stdin, std::io::stdout()).expect("enter raw mode");
        println!("ready");
        std::io::stdout().flush().expect("flush ready marker");

        sigterm.recv().await;
        drop(guard);
    });
}
