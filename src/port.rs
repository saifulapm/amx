//! The TCP port each agent is given as `$AMX_PORT` and `$PORT`.
//!
//! A spawn picks the port under [`crate::store::spawn_lock`], so two spawns at
//! once never pick the same one, and keeps it in the agent's directory (see
//! [`crate::store::port`]) for the pane's boot to read.

use std::collections::BTreeSet;
use std::ops::RangeInclusive;

/// The variable that names the agent's port, beside the conventional `PORT`.
pub const ENV: &str = "AMX_PORT";

/// The conventional variable a dev server reads its port from.
pub const PORT_ENV: &str = "PORT";

/// Narrows the range, so a test can hold every port in it. Not for people.
const RANGE_ENV: &str = "AMX_PORT_RANGE";

/// The ports amx hands out.
const RANGE: RangeInclusive<u16> = 4100..=4999;

/// The ports to pick from: `$AMX_PORT_RANGE` as `<first>-<last>`, else
/// 4100-4999. A value that does not read as a range is ignored.
pub fn range() -> RangeInclusive<u16> {
    std::env::var(RANGE_ENV)
        .ok()
        .and_then(|value| {
            let (first, last) = value.split_once('-')?;
            Some(first.trim().parse().ok()?..=last.trim().parse().ok()?)
        })
        .filter(|range: &RangeInclusive<u16>| !range.is_empty())
        .unwrap_or(RANGE)
}

/// The warning for a spawn that found every port in `range` taken.
pub fn none_free(range: &RangeInclusive<u16>) -> String {
    format!(
        "no free port in {}-{}; the agent starts without $AMX_PORT and $PORT",
        range.start(),
        range.end()
    )
}

/// The first port in `range` no agent holds and no process listens on.
pub fn pick(range: RangeInclusive<u16>, held: &BTreeSet<u16>) -> Option<u16> {
    range
        .filter(|port| !held.contains(port))
        .find(|port| listens_nobody(*port))
}

/// Whether nothing listens on `port`. Binding every address also fails over a
/// listener on 127.0.0.1 alone.
fn listens_nobody(port: u16) -> bool {
    std::net::TcpListener::bind(("0.0.0.0", port)).is_ok()
}
