//! The control method table.
//!
//! 04 §4: "The control schema is one annotated Rust enum from which we
//! **derive**: serde wire names, method dispatch, the JSON Schema artifact, CLI
//! clap tree and parsing, and docs (fixes W6's four hand-synced lists — adding
//! a method is one enum variant + one handler)."
//!
//! That is what [`method_table!`] does. One row per method produces:
//!
//! - a [`Method`] variant and its wire name, in both directions;
//! - a [`Call`] variant carrying the method's parameter type;
//! - a [`Dispatch`] trait method, so a handler that is never written is a
//!   compile error rather than a method that silently 404s;
//! - a [`MethodSpec`] row in [`SPECS`], from which the CLI tree is built.
//!
//! Payload types live in the per-domain modules, so only the variant list lives
//! on the enum — which is what keeps the enum small as the surface grows.
//!
//! The JSON Schema artifact named in 04 §4 has no consumer until `amx api
//! schema`, so it is not generated here yet; [`SPECS`] is the table it will be
//! generated from.

#[cfg(feature = "cli")]
pub mod cli;
pub mod client;
pub mod pane;
pub mod session;
pub mod workspace;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::rpc::RpcError;

/// One row of the method table, as data.
///
/// The CLI tree, and later the schema artifact, are built by walking [`SPECS`]
/// rather than by repeating the method list — the repetition W6 is about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MethodSpec {
    /// The method.
    pub method: Method,
    /// Its wire name, e.g. `pane.split`.
    pub wire: &'static str,
    /// Its CLI path, e.g. `["pane", "split"]`.
    pub cli: &'static [&'static str],
    /// One-line description, used as the CLI `about` and in generated docs.
    pub about: &'static str,
}

macro_rules! method_table {
    ($(
        $(#[$doc:meta])*
        $variant:ident {
            wire: $wire:literal,
            handler: $handler:ident,
            params: $params:ty,
            reply: $reply:ty,
            cli: [$($seg:literal),+ $(,)?],
            about: $about:literal $(,)?
        }
    )*) => {
        /// Every control method, by name.
        ///
        /// Serializes as its wire name in both directions, so the name a peer
        /// sends and the name this table declares can never drift apart.
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
        pub enum Method {
            $(
                $(#[$doc])*
                #[serde(rename = $wire)]
                $variant,
            )*
        }

        impl Method {
            /// Every method in table order.
            pub const ALL: &'static [Method] = &[$(Method::$variant),*];

            /// This method's wire name.
            #[must_use]
            pub const fn wire_name(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)*
                }
            }

            /// The method with this wire name, if any.
            ///
            /// `None` is the skew-tolerant answer: an unknown method becomes a
            /// `METHOD_NOT_FOUND` reply, never a dropped connection.
            #[must_use]
            pub fn from_wire_name(name: &str) -> Option<Self> {
                match name {
                    $($wire => Some(Self::$variant),)*
                    _ => None,
                }
            }

            /// This method's table row.
            #[must_use]
            pub const fn spec(self) -> &'static MethodSpec {
                match self {
                    $(Self::$variant => &SPECS[Self::$variant as usize],)*
                }
            }
        }

        /// The method table, as data.
        pub const SPECS: &[MethodSpec] = &[
            $(MethodSpec {
                method: Method::$variant,
                wire: $wire,
                cli: &[$($seg),+],
                about: $about,
            },)*
        ];

        /// A control call with its parameters decoded.
        #[derive(Clone, PartialEq, Debug)]
        pub enum Call {
            $(
                $(#[$doc])*
                $variant($params),
            )*
        }

        impl Call {
            /// Which method this call is.
            #[must_use]
            pub const fn method(&self) -> Method {
                match self {
                    $(Self::$variant(_) => Method::$variant,)*
                }
            }

            /// Decode a call from an envelope's method name and params.
            ///
            /// Params default to `null` when absent, so a method whose
            /// parameter type is all-optional can be called with no `params`
            /// member at all.
            pub fn decode(method: &str, params: Option<Value>) -> Result<Self, RpcError> {
                let params = params.unwrap_or(Value::Null);
                match Method::from_wire_name(method) {
                    $(Some(Method::$variant) => serde_json::from_value::<$params>(params)
                        .map(Self::$variant)
                        .map_err(|e| RpcError::new(RpcError::INVALID_PARAMS, e.to_string())),)*
                    None => Err(RpcError::method_not_found(method)),
                }
            }

            /// Re-encode this call's parameters.
            pub fn params(&self) -> Result<Value, RpcError> {
                match self {
                    $(Self::$variant(p) => serde_json::to_value(p)
                        .map_err(|e| RpcError::new(RpcError::INTERNAL_ERROR, e.to_string())),)*
                }
            }
        }

        /// The server side of the method table.
        ///
        /// One method per table row: a new row does not compile until it has a
        /// handler, which is the property that replaces W6's hand-synced lists.
        /// Handlers return the method's reply type, and the server's own
        /// handlers additionally return an
        /// [`Effect`](amx_core::Effect) alongside it, so a state change that
        /// forgets to report itself is a type error (D2).
        #[allow(async_fn_in_trait, reason = "dispatch is used generically, never as a trait object")]
        pub trait Dispatch {
            $(
                #[doc = concat!("Handle `", $wire, "`.")]
                async fn $handler(&mut self, params: $params) -> Result<$reply, RpcError>;
            )*
        }

        /// Route one decoded call to its handler and serialize the reply.
        pub async fn dispatch<D: Dispatch>(target: &mut D, call: Call) -> Result<Value, RpcError> {
            match call {
                $(Call::$variant(params) => {
                    let reply: $reply = target.$handler(params).await?;
                    serde_json::to_value(reply)
                        .map_err(|e| RpcError::new(RpcError::INTERNAL_ERROR, e.to_string()))
                })*
            }
        }
    };
}

method_table! {
    /// Liveness and clock check; the connect probe's payload.
    Ping {
        wire: "ping",
        handler: ping,
        params: session::PingParams,
        reply: session::PingReply,
        cli: ["ping"],
        about: "Check that the session server is alive",
    }

    /// Create a workspace.
    WorkspaceCreate {
        wire: "workspace.create",
        handler: workspace_create,
        params: workspace::CreateParams,
        reply: workspace::CreateReply,
        cli: ["workspace", "create"],
        about: "Create a workspace",
    }

    /// Rename a workspace.
    WorkspaceRename {
        wire: "workspace.rename",
        handler: workspace_rename,
        params: workspace::RenameParams,
        reply: workspace::RenameReply,
        cli: ["workspace", "rename"],
        about: "Rename a workspace",
    }

    /// Kill a workspace and every pane it holds.
    WorkspaceKill {
        wire: "workspace.kill",
        handler: workspace_kill,
        params: workspace::KillParams,
        reply: workspace::KillReply,
        cli: ["workspace", "kill"],
        about: "Kill a workspace and every pane it holds",
    }

    /// Focus a workspace.
    WorkspaceSwitch {
        wire: "workspace.switch",
        handler: workspace_switch,
        params: workspace::SwitchParams,
        reply: workspace::SwitchReply,
        cli: ["workspace", "switch"],
        about: "Focus a workspace",
    }

    /// Split a pane, creating a new one beside it.
    PaneSplit {
        wire: "pane.split",
        handler: pane_split,
        params: pane::SplitParams,
        reply: pane::SplitReply,
        cli: ["pane", "split"],
        about: "Split a pane and run a command in the new one",
    }

    /// Toggle a pane between its layout slot and the full workspace.
    PaneZoom {
        wire: "pane.zoom",
        handler: pane_zoom,
        params: pane::ZoomParams,
        reply: pane::ZoomReply,
        cli: ["pane", "zoom"],
        about: "Toggle a pane between its layout slot and the full workspace",
    }

    /// Exchange two panes' positions in the layout.
    PaneSwap {
        wire: "pane.swap",
        handler: pane_swap,
        params: pane::SwapParams,
        reply: pane::SwapReply,
        cli: ["pane", "swap"],
        about: "Swap two panes' positions without restarting either",
    }

    /// Move a pane into a different workspace.
    PaneMove {
        wire: "pane.move",
        handler: pane_move,
        params: pane::MoveParams,
        reply: pane::MoveReply,
        cli: ["pane", "move"],
        about: "Move a pane into a different workspace",
    }

    /// Close a pane.
    PaneClose {
        wire: "pane.close",
        handler: pane_close,
        params: pane::CloseParams,
        reply: pane::CloseReply,
        cli: ["pane", "close"],
        about: "Close a pane",
    }

    /// Move a workspace's focus to the geometric neighbour in a direction.
    PaneFocus {
        wire: "pane.focus",
        handler: pane_focus,
        params: pane::FocusParams,
        reply: pane::FocusReply,
        cli: ["pane", "focus"],
        about: "Move focus to the neighbouring pane in a direction",
    }

    /// Nudge the split holding a pane by a ratio delta.
    PaneResize {
        wire: "pane.resize",
        handler: pane_resize,
        params: pane::ResizeParams,
        reply: pane::ResizeReply,
        cli: ["pane", "resize"],
        about: "Grow or shrink a pane's slot in a direction",
    }
}
