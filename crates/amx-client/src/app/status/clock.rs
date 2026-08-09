//! The clock every age on the status line is rendered against.
//!
//! D-M4-4: a wall-clock instant travels absolute
//! ([`AgentSnapshot::since`](amx_core::agent::AgentSnapshot::since)) and an age
//! is never computed against the reader's own clock, because a remote attach is
//! two machines and two clocks. The reply that carries a server `now` beside
//! `since` is `agent.list`, which the status line deliberately does not call
//! (D-M4-5) — so what stands in for it is this: the newest stamp any snapshot
//! carries, held with the monotonic instant this client read it at, and
//! advanced by monotonic elapsed time between repaints.
//!
//! The estimate is a *lower bound* on the server's clock, and the bound is
//! stated rather than hidden: an age is never overstated, and it is understated
//! by at most the time since the last transition this client has seen anywhere
//! in the session. A blocked agent in a live session is dated exactly — the
//! `attention_enqueued` that queued it carries the stamp and arrives at the
//! transition — and a session where nothing at all has moved since the attach
//! is where the understatement shows. Nothing here reads
//! [`SystemTime`](std::time::SystemTime), so nothing here can be wrong by the
//! skew between two machines.
//!
//! A live upgrade does not disturb it: a handoff is one machine handing panes
//! to another process on the same machine, so the successor's clock is the
//! clock this estimate was anchored on.

use std::fmt::Write as _;
use std::time::Instant;

use amx_core::agent::EpochMillis;

/// The client's own estimate of the server's wall clock.
#[derive(Debug, Default)]
pub(in crate::app) struct ServerClock {
    /// The newest server instant this client has been told about, and the local
    /// monotonic instant it was read at.
    anchor: Option<(EpochMillis, Instant)>,
}

impl ServerClock {
    /// What the server's clock read at `at`, as far as this client can tell.
    fn read_at(&self, at: Instant) -> Option<EpochMillis> {
        let (server, taken) = self.anchor?;
        let elapsed = EpochMillis::try_from(at.saturating_duration_since(taken).as_millis())
            .unwrap_or(EpochMillis::MAX);
        Some(server.saturating_add(elapsed))
    }

    /// Take `stamp`, read at `at`, as evidence of where the server's clock is.
    ///
    /// Only ever forwards. A stamp is a lower bound — the server's clock had
    /// reached it by the time the value arrived — so the useful anchor is the
    /// highest bound seen, and a stamp replayed out of an event ring after a
    /// gap is one this estimate has already passed.
    pub(in crate::app) fn observe_at(&mut self, stamp: EpochMillis, at: Instant) {
        if self.read_at(at).is_none_or(|now| stamp > now) {
            self.anchor = Some((stamp, at));
        }
    }

    /// How long ago `since` was, in milliseconds, if this clock is anchored.
    pub(in crate::app) fn age_at(&self, since: EpochMillis, at: Instant) -> Option<u64> {
        Some(self.read_at(at)?.saturating_sub(since))
    }
}

/// Write `ms` as the coarsest unit that says something: `12s`, `4m`, `2h`, `3d`.
///
/// One unit and no fractions. The question the line answers is "who has waited
/// longest", and a second digit of precision on a four-minute wait is width
/// spent on nothing.
pub(in crate::app) fn push_age(out: &mut String, ms: u64) {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    let secs = ms / 1000;
    let (n, unit) = if secs < MINUTE {
        (secs, 's')
    } else if secs < HOUR {
        (secs / MINUTE, 'm')
    } else if secs < DAY {
        (secs / HOUR, 'h')
    } else {
        (secs / DAY, 'd')
    };
    let _ = write!(out, "{n}{unit}");
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// A wall clock the server could plausibly have stamped a block at.
    const STAMP: EpochMillis = 1_754_650_000_000;

    /// Four minutes before it.
    const EARLIER: EpochMillis = STAMP - 240_000;

    #[test]
    fn an_age_is_the_coarsest_unit_that_says_something() {
        let mut out = String::new();
        for (ms, expected) in [
            (0_u64, "0s"),
            (12_400, "12s"),
            (59_999, "59s"),
            (60_000, "1m"),
            (240_000, "4m"),
            (3_600_000, "1h"),
            (90_000_000, "1d"),
        ] {
            out.clear();
            push_age(&mut out, ms);
            assert_eq!(out, expected, "{ms} ms");
        }
    }

    #[test]
    fn the_server_clock_takes_the_highest_bound_and_advances_monotonically() {
        let start = Instant::now();
        let mut clock = ServerClock::default();
        assert_eq!(
            clock.age_at(EARLIER, start),
            None,
            "unanchored dates nothing"
        );

        clock.observe_at(STAMP, start);
        assert_eq!(clock.age_at(EARLIER, start), Some(240_000));

        // Time passing is the whole of the advance: no new stamp, an older age.
        let later = start + Duration::from_secs(30);
        assert_eq!(clock.age_at(EARLIER, later), Some(270_000));

        // A stamp the estimate has already passed — a replay out of the event
        // ring after a gap — must not drag the clock backwards.
        clock.observe_at(STAMP, later);
        assert_eq!(clock.age_at(EARLIER, later), Some(270_000));

        // A newer one moves it, and the age moves with it.
        clock.observe_at(STAMP + 120_000, later);
        assert_eq!(clock.age_at(EARLIER, later), Some(360_000));

        // An age is never negative: a stamp from the future — a pane the hub
        // stamped between this client's read and its paint — reads as new
        // rather than as a wrap-around.
        assert_eq!(clock.age_at(STAMP + 600_000, later), Some(0));
    }
}
