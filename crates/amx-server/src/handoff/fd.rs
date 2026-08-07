//! The descriptor transfer: one pty master per message.
//!
//! **W05 fills this**, with rustix's `sendmsg`/`recvmsg` and
//! `SendAncillaryBuffer`/`ScmRights` — the `net` feature W03 enabled is for
//! exactly this and nothing else.
//!
//! One fd per message, whose 1-byte payload is the pane's manifest index
//! (D-M3-6 point 3). herdr batches every descriptor into a single `sendmsg`
//! capped at 64 panes, under the kernel's SCM_MAX_FD of 253, so a 65-pane
//! session cannot upgrade at all; per-pane messages have no such cliff, pair
//! descriptors with entries deterministically, and name the exact pane when a
//! receive fails. They also make one misconfiguration structurally impossible:
//! unix(7) has the kernel *close* the excess descriptors when the receiver's
//! ancillary buffer is too small, and a message that carries exactly one fd
//! cannot have an excess. The cost is a `sendmsg` per pane rather than one for
//! all of them (R-M3-11).
//!
//! The descriptors themselves come from
//! [`PtyActorHandle::dup_fd`](crate::pty::PtyActorHandle::dup_fd), which
//! answers on the actor thread and only in `Quiesced` (D-M3-3).
