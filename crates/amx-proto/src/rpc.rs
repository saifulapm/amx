//! The JSON-RPC 2.0 control envelope (channel 0).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The only `jsonrpc` value this protocol accepts.
pub const JSONRPC_VERSION: &str = "2.0";

/// A JSON-RPC request identifier.
///
/// Numbers and strings are both legal per JSON-RPC 2.0 and both are accepted;
/// amx generates numbers. Untagged, so the id round-trips as whatever the peer
/// sent it as.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    /// A numeric id.
    Number(u64),
    /// A string id.
    Text(String),
}

impl From<u64> for RequestId {
    fn from(value: u64) -> Self {
        Self::Number(value)
    }
}

/// A call that expects a reply.
///
/// `params` stays a `Value` at the envelope layer: the envelope is decoded once
/// and the method table decides what the payload should be, so an unknown
/// method is a clean `MethodNotFound` reply rather than a decode failure that
/// takes the connection with it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Request {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// The correlation id.
    pub id: RequestId,
    /// The method's wire name, e.g. `pane.split`.
    pub method: String,
    /// Method parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Request {
    /// A request with this build's `jsonrpc` version.
    #[must_use]
    pub fn new(id: RequestId, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_owned(),
            id,
            method: method.into(),
            params,
        }
    }

    /// Whether the envelope declares the JSON-RPC version we speak.
    #[must_use]
    pub fn is_jsonrpc_2(&self) -> bool {
        self.jsonrpc == JSONRPC_VERSION
    }
}

/// A call that expects no reply.
///
/// Server-to-client events ride on notifications; they carry no id precisely
/// because there is nothing to correlate them with.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Notification {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// The method's wire name.
    pub method: String,
    /// Method parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Notification {
    /// A notification with this build's `jsonrpc` version.
    #[must_use]
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_owned(),
            method: method.into(),
            params,
        }
    }
}

/// The reply to a [`Request`].
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Response {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// The id of the request being answered.
    pub id: RequestId,
    /// Exactly one of `result` or `error`, as JSON-RPC 2.0 requires.
    #[serde(flatten)]
    pub outcome: RpcOutcome,
}

impl Response {
    /// A successful reply.
    #[must_use]
    pub fn ok(id: RequestId, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_owned(),
            id,
            outcome: RpcOutcome::Result(result),
        }
    }

    /// A failed reply.
    #[must_use]
    pub fn err(id: RequestId, error: RpcError) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_owned(),
            id,
            outcome: RpcOutcome::Error(error),
        }
    }
}

/// Success or failure of one call.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RpcOutcome {
    /// The method's reply payload.
    Result(Value),
    /// Why the method failed.
    Error(RpcError),
}

/// A JSON-RPC error object.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RpcError {
    /// JSON-RPC error code.
    pub code: i32,
    /// Human-readable summary.
    pub message: String,
    /// Structured detail, when there is any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// The JSON-RPC 2.0 reserved code for a malformed envelope.
    pub const INVALID_REQUEST: i32 = -32600;
    /// The JSON-RPC 2.0 reserved code for an unknown method.
    ///
    /// This is the skew-tolerant failure: a peer calling a method this build
    /// does not have gets a typed error back and keeps its connection.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// The JSON-RPC 2.0 reserved code for parameters that do not fit.
    pub const INVALID_PARAMS: i32 = -32602;
    /// The JSON-RPC 2.0 reserved code for a handler failure.
    pub const INTERNAL_ERROR: i32 = -32603;
    /// A long poll the session ended before it could answer.
    ///
    /// The first code of amx's own, and it sits in −32000..=−32099, the range
    /// JSON-RPC 2.0 reserves for implementation-defined server errors. It is
    /// separate from [`INTERNAL_ERROR`](Self::INTERNAL_ERROR) because it is not
    /// a failure: a `wait` cut short because the session is handing over or
    /// shutting down was never answered at all, and the caller's question is
    /// still open. A client that recognises this code redials and asks again
    /// (D-M3-7); one that does not sees an ordinary error, exactly as every
    /// client did before the code existed.
    pub const WAIT_ABANDONED: i32 = -32000;

    /// An error with a code and message and no data.
    #[must_use]
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// `METHOD_NOT_FOUND` for `method`.
    #[must_use]
    pub fn method_not_found(method: &str) -> Self {
        Self::new(Self::METHOD_NOT_FOUND, format!("unknown method: {method}"))
    }
}
