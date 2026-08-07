//! The manifest: what a session is, as one line of JSON.
//!
//! **W04 fills this.** `docs/09-m3-plan.md` D-M3-5 tables the whole inventory
//! and where each field is reachable from — schema version and read window,
//! exporter version and proto window, the `SessionId`, the bus head, the
//! persist `Snapshot` captured in memory, per-pane state from the parser
//! thread, and the hub's statuses and attention queue in block order. What is
//! deliberately *not* carried is tabled there too: client connections, in-flight
//! waits, damage accumulators, sidecar files.
//!
//! The manifest surface skews on its own N/N−1 window, separately from the
//! control protocol: D-M3-6 point 2 has the importer check the manifest
//! *window* rather than demanding version equality, which is what lets
//! self-update hand a session to any successor that can read manifest v1.
