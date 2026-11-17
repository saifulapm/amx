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
/// 4100-4999.
pub fn range() -> RangeInclusive<u16> {
    read_range(std::env::var(RANGE_ENV).ok().as_deref())
}

/// `value` as a range of ports, else 4100-4999. A value that does not read as
/// a range is ignored, and so is one holding port 0, which binds whatever
/// port the system picks and so always reads free.
fn read_range(value: Option<&str>) -> RangeInclusive<u16> {
    value
        .and_then(|value| {
            let (first, last) = value.split_once('-')?;
            Some(first.trim().parse().ok()?..=last.trim().parse().ok()?)
        })
        .filter(|range: &RangeInclusive<u16>| !range.is_empty() && *range.start() > 0)
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

/// Whether nothing listens on `port`. Binding every IPv4 address also fails
/// over a listener on 127.0.0.1 alone, but not over one on `[::1]` alone, so
/// that is bound too. Each socket closes as soon as it is bound.
fn listens_nobody(port: u16) -> bool {
    let ipv4 = std::net::TcpListener::bind(("0.0.0.0", port)).map(drop);
    let ipv6 = std::net::TcpListener::bind(("::1", port)).map(drop);
    free(ipv4, ipv6)
}

/// Whether a port whose IPv4 and IPv6 binds came to these is free. IPv6 holds
/// a port only when its bind says the address is in use: a machine with IPv6
/// off fails it for want of the family or of `::1`, and has the port free.
fn free(ipv4: std::io::Result<()>, ipv6: std::io::Result<()>) -> bool {
    ipv4.is_ok()
        && ipv6
            .err()
            .is_none_or(|e| e.kind() != std::io::ErrorKind::AddrInUse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    /// What `socket(2)` says on a kernel built without IPv6.
    const EAFNOSUPPORT: i32 = 97;

    /// What `bind(2)` says for `::1` with IPv6 turned off by sysctl.
    const EADDRNOTAVAIL: i32 = 99;

    #[test]
    fn a_range_holding_port_0_or_not_reading_as_one_is_the_usual_range() {
        assert_eq!(read_range(Some("4990-4991")), 4990..=4991);
        for value in [
            None,
            Some("abc"),
            Some("5000-4000"),
            Some("0-0"),
            Some("0-4990"),
        ] {
            assert_eq!(read_range(value), RANGE, "{value:?}");
        }
    }

    #[test]
    fn a_port_is_free_when_both_families_bind() {
        assert!(free(Ok(()), Ok(())));
    }

    #[test]
    fn a_port_in_use_on_either_family_is_taken() {
        assert!(!free(Err(ErrorKind::AddrInUse.into()), Ok(())));
        assert!(!free(Ok(()), Err(ErrorKind::AddrInUse.into())));
    }

    #[test]
    fn a_machine_without_ipv6_has_the_port_free() {
        for errno in [EAFNOSUPPORT, EADDRNOTAVAIL] {
            assert!(
                free(Ok(()), Err(Error::from_raw_os_error(errno))),
                "{errno}"
            );
        }
    }

    #[test]
    fn the_ipv6_check_leaves_the_port_for_the_agents_own_server() {
        let Ok(probe) = std::net::TcpListener::bind("[::1]:0") else {
            return;
        };
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        if listens_nobody(port) {
            assert!(
                std::net::TcpListener::bind(("::1", port)).is_ok(),
                "the check let go of [::1]:{port}"
            );
        }
    }
}
