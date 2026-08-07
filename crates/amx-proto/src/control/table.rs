//! The method table macro: one row in, five surfaces out.
//!
//! Split out of [`super`] by V02 (`docs/08-m2-plan.md` R-M2-5) before M2's
//! twelve new rows landed: the macro definition and the table it expands sat in
//! one 371-line file at the soft budget, and the two halves change for
//! different reasons — the macro when a *surface* is added (the JSON Schema
//! artifact of 04 §4 is the next one), the table when a *method* is. The move
//! is mechanical: not a line of the expansion changed.
//!
//! The macro is `pub(crate) use`d rather than `#[macro_export]`ed. It is this
//! crate's own generator, not a public one, and exporting it would put an
//! unstable expansion into the crate's API surface.

use super::Method;

/// One row of the method table, as data.
///
/// The CLI tree, and later the schema artifact, are built by walking
/// [`SPECS`](super::SPECS) rather than by repeating the method list — the
/// repetition W6 is about.
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

pub(crate) use method_table;
