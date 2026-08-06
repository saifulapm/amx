//! Core types for amx: identity, the event bus, render effects, the session
//! context, scrollback identity and the platform seam.
//!
//! This crate is the shared vocabulary of the server and the client and has no
//! I/O of its own. Everything public here is a contract: the doc comment on a
//! type states the invariant it encodes, quoting `docs/04-architecture.md`
//! where the invariant comes from there.

#![forbid(unsafe_code)]

pub mod ctx;
pub mod effect;
pub mod error;
pub mod event;
pub mod id;
pub mod layout;
pub mod platform;
pub mod scrollback;

pub use ctx::{Ctx, Env, SessionName};
pub use effect::{Effect, EffectSet, Level, Scheduled};
pub use error::{CtxError, IdParseError, SessionNameError};
pub use event::{Bus, Delivery, Envelope, Event, Seq, Subscription};
pub use id::{ClientId, GridGeneration, PaneId, SessionId, ShortNumber, ShortNumbers, WorkspaceId};
pub use layout::{Axis, Direction, Layout, LayoutError, Rect};
pub use scrollback::{EvictionFloor, InvalidationCause, RowHash, RowId, RowRange};
