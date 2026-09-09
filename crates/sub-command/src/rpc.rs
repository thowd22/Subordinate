//! The JSON-RPC 2.0 wire types the Command API speaks.
//!
//! The Command API is one JSON-RPC 2.0 endpoint serving the GUI, the CLI, the
//! MCP bridge and plugins (docs/PLAN.md §4, decision-7). This module is only
//! the message layer: [`Request`], [`Notification`], [`Response`] and
//! [`RpcError`], the [`RequestId`] forms, and the [`Incoming`] / [`Outgoing`]
//! batch envelopes. What a method *does* lives in [`crate::dispatch`], and how
//! bytes reach it lives in the transport.
//!
//! Everything here is transport-agnostic, so the same types serve a socket, an
//! in-process caller and a test.
//!
//! ```
//! use sub_command::rpc::{Request, RequestId, Response, error_codes};
//!
//! let request: Request = serde_json::from_str(
//!     r#"{"jsonrpc":"2.0","method":"project.get","id":1}"#,
//! )
//! .unwrap();
//! assert_eq!(request.id, RequestId::Number(1));
//!
//! let response = Response::result(request.id.clone(), serde_json::json!({ "revision": 0 }));
//! assert_eq!(
//!     serde_json::to_string(&response).unwrap(),
//!     r#"{"jsonrpc":"2.0","result":{"revision":0},"id":1}"#,
//! );
//!
//! let failed = Response::error(Some(request.id), error_codes::method_not_found("nope"));
//! assert_eq!(failed.error_ref().unwrap().code, error_codes::METHOD_NOT_FOUND);
//! ```

use std::fmt;

use serde::de::{Error as DeError, Unexpected};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use sub_core::SubError;

/// The only `jsonrpc` version string this endpoint accepts.
pub const VERSION: &str = "2.0";

/// The `"jsonrpc": "2.0"` member, as a type that cannot hold anything else.
///
/// Deserialising a message whose version is missing or different fails, which
/// is what turns it into an invalid-request error.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Version;

impl Serialize for Version {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(VERSION)
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text == VERSION {
            Ok(Self)
        } else {
            Err(D::Error::invalid_value(
                Unexpected::Str(&text),
                &"the string \"2.0\"",
            ))
        }
    }
}

/// The identifier of a call, which the response echoes back unchanged.
///
/// JSON-RPC 2.0 allows a string or a number; fractional numbers are
/// discouraged by the specification and are not accepted here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    /// A numeric id, as most clients send.
    Number(i64),
    /// A string id.
    Text(String),
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(number) => write!(f, "{number}"),
            Self::Text(text) => write!(f, "{text}"),
        }
    }
}

impl From<i64> for RequestId {
    fn from(value: i64) -> Self {
        Self::Number(value)
    }
}

impl From<String> for RequestId {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for RequestId {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

/// A call that expects a response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Always `"2.0"`.
    pub jsonrpc: Version,
    /// The method name, such as `clip.add`.
    pub method: String,
    /// The method's parameters; absent when the method takes none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    /// The id the response echoes.
    pub id: RequestId,
}

impl Request {
    /// Builds a request.
    #[must_use]
    pub fn new(id: impl Into<RequestId>, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: Version,
            method: method.into(),
            params,
            id: id.into(),
        }
    }
}

/// A call that expects no response.
///
/// A request without an `id` member is a notification: the server runs it and
/// stays silent, even when it fails.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notification {
    /// Always `"2.0"`.
    pub jsonrpc: Version,
    /// The method name.
    pub method: String,
    /// The method's parameters; absent when the method takes none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Notification {
    /// Builds a notification.
    #[must_use]
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: Version,
            method: method.into(),
            params,
        }
    }
}

/// One inbound message: a [`Request`] or a [`Notification`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Call {
    /// A call with an id, which is answered.
    Request(Request),
    /// A call without an id, which is not.
    Notification(Notification),
}

impl Call {
    /// The method name of either form.
    #[must_use]
    pub fn method(&self) -> &str {
        match self {
            Self::Request(request) => &request.method,
            Self::Notification(notification) => &notification.method,
        }
    }

    /// The parameters of either form.
    #[must_use]
    pub fn params(&self) -> Option<&Value> {
        match self {
            Self::Request(request) => request.params.as_ref(),
            Self::Notification(notification) => notification.params.as_ref(),
        }
    }

    /// The id, or `None` for a notification.
    #[must_use]
    pub fn id(&self) -> Option<&RequestId> {
        match self {
            Self::Request(request) => Some(&request.id),
            Self::Notification(_) => None,
        }
    }
}

impl From<Request> for Call {
    fn from(request: Request) -> Self {
        Self::Request(request)
    }
}

impl From<Notification> for Call {
    fn from(notification: Notification) -> Self {
        Self::Notification(notification)
    }
}

/// The `result` or `error` half of a [`Response`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Payload {
    /// The method succeeded, with this value.
    Result(Value),
    /// The method, or the message itself, failed.
    Error(RpcError),
}

/// The answer to one [`Request`].
///
/// The `id` is `null` only when the request could not be parsed far enough to
/// recover one, as the specification requires.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// Always `"2.0"`.
    pub jsonrpc: Version,
    /// Exactly one of `result` or `error`.
    #[serde(flatten)]
    pub payload: Payload,
    /// The id of the request being answered, or `null`.
    pub id: Option<RequestId>,
}

impl Response {
    /// A successful response.
    #[must_use]
    pub fn result(id: RequestId, value: Value) -> Self {
        Self {
            jsonrpc: Version,
            payload: Payload::Result(value),
            id: Some(id),
        }
    }

    /// A failed response.
    #[must_use]
    pub fn error(id: Option<RequestId>, error: RpcError) -> Self {
        Self {
            jsonrpc: Version,
            payload: Payload::Error(error),
            id,
        }
    }

    /// The result value, if this response succeeded.
    #[must_use]
    pub fn value(&self) -> Option<&Value> {
        match &self.payload {
            Payload::Result(value) => Some(value),
            Payload::Error(_) => None,
        }
    }

    /// The error, if this response failed.
    #[must_use]
    pub fn error_ref(&self) -> Option<&RpcError> {
        match &self.payload {
            Payload::Result(_) => None,
            Payload::Error(error) => Some(error),
        }
    }

    /// Whether this response carries an error.
    #[must_use]
    pub fn is_error(&self) -> bool {
        matches!(self.payload, Payload::Error(_))
    }
}

/// The `error` member of a failed [`Response`].
///
/// `data` carries the [`SubError`] in its stable JSON shape whenever the
/// failure came from the engine, so an agent can match on the `code` there
/// while the JSON-RPC `code` stays one of the standard numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    /// One of the numbers in [`error_codes`].
    pub code: i64,
    /// A short human-readable message.
    pub message: String,
    /// The `SubError` JSON, or a detail string, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// Builds an error with no data.
    #[must_use]
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Attaches the data member.
    #[must_use]
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// Wraps a [`SubError`], choosing the JSON-RPC number its stable code maps
    /// to and keeping the whole `SubError` in `data`.
    #[must_use]
    pub fn from_sub_error(error: &SubError) -> Self {
        Self::new(error_codes::for_sub_error(error), error.message.clone())
            .with_data(error.to_json())
    }

    /// The `SubError` behind this error, when `data` holds one.
    #[must_use]
    pub fn sub_error(&self) -> Option<SubError> {
        serde_json::from_value(self.data.clone()?).ok()
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl From<&SubError> for RpcError {
    fn from(error: &SubError) -> Self {
        Self::from_sub_error(error)
    }
}

/// One inbound message: a single [`Call`] or a batch of them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Incoming {
    /// A single call.
    Single(Call),
    /// A batch of calls.
    Batch(Vec<Call>),
}

/// One outbound message: a single [`Response`] or a batch of them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Outgoing {
    /// The answer to a single call.
    Single(Response),
    /// The answers to a batch, in the order the calls arrived.
    Batch(Vec<Response>),
}

/// The JSON-RPC 2.0 error numbers, and the mapping from [`SubError`] codes.
pub mod error_codes {
    use sub_core::SubError;

    use super::RpcError;

    /// The bytes were not valid JSON.
    pub const PARSE_ERROR: i64 = -32700;
    /// The JSON was valid but is not a JSON-RPC 2.0 request.
    pub const INVALID_REQUEST: i64 = -32600;
    /// The method name is not one this build serves.
    pub const METHOD_NOT_FOUND: i64 = -32601;
    /// The parameters do not match the method.
    pub const INVALID_PARAMS: i64 = -32602;
    /// A bug on the server side.
    pub const INTERNAL_ERROR: i64 = -32603;
    /// An application-level failure: the request was well formed and the
    /// method ran, but the edit could not be made.
    ///
    /// `-32000` is inside the range the specification reserves for
    /// implementation-defined server errors.
    pub const SERVER_ERROR: i64 = -32000;

    /// The parse error, which always answers with a `null` id.
    #[must_use]
    pub fn parse_error(detail: impl Into<String>) -> RpcError {
        RpcError::new(PARSE_ERROR, "Parse error").with_data(detail.into().into())
    }

    /// The invalid-request error.
    #[must_use]
    pub fn invalid_request(detail: impl Into<String>) -> RpcError {
        RpcError::new(INVALID_REQUEST, "Invalid Request").with_data(detail.into().into())
    }

    /// The method-not-found error, naming the method that was asked for.
    #[must_use]
    pub fn method_not_found(method: &str) -> RpcError {
        RpcError::new(METHOD_NOT_FOUND, "Method not found")
            .with_data(serde_json::json!({ "method": method }))
    }

    /// The JSON-RPC number a [`SubError`] maps to.
    ///
    /// The mapping is by stable error code, so the wire number and the
    /// `SubError` code never disagree:
    ///
    /// - `edit.unknown_command`, `command.unknown_method` — no such method:
    ///   `-32601`.
    /// - `edit.invalid_command`, `core.invalid_argument`,
    ///   `command.invalid_params` — the parameters are wrong: `-32602`.
    /// - `core.internal` — a server bug: `-32603`.
    /// - anything else — an application failure: `-32000`.
    #[must_use]
    pub fn for_sub_error(error: &SubError) -> i64 {
        match error.code.as_str() {
            "edit.unknown_command" | "command.unknown_method" => METHOD_NOT_FOUND,
            "edit.invalid_command" | "core.invalid_argument" | "command.invalid_params" => {
                INVALID_PARAMS
            }
            "core.internal" => INTERNAL_ERROR,
            _ => SERVER_ERROR,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sub_core::{ErrorCode, SubError};

    use super::{
        Call, Incoming, Notification, Outgoing, Request, RequestId, Response, RpcError, Version,
        error_codes,
    };

    #[test]
    fn request_round_trips() {
        let text = r#"{"jsonrpc":"2.0","method":"clip.add","params":{"track":"t"},"id":"a"}"#;
        let request: Request = serde_json::from_str(text).unwrap();
        assert_eq!(request.method, "clip.add");
        assert_eq!(request.id, RequestId::Text("a".to_owned()));
        assert_eq!(serde_json::to_string(&request).unwrap(), text);
    }

    #[test]
    fn notification_has_no_id() {
        let call: Call = serde_json::from_str(r#"{"jsonrpc":"2.0","method":"edit.undo"}"#).unwrap();
        assert!(matches!(call, Call::Notification(_)));
        assert_eq!(call.id(), None);
        assert_eq!(call.method(), "edit.undo");
        assert_eq!(call.params(), None);
    }

    #[test]
    fn call_with_id_is_a_request() {
        let call: Call =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"edit.undo","id":7}"#).unwrap();
        assert_eq!(call.id(), Some(&RequestId::Number(7)));
    }

    #[test]
    fn wrong_version_is_rejected() {
        assert!(
            serde_json::from_str::<Request>(r#"{"jsonrpc":"1.0","method":"a","id":1}"#).is_err()
        );
        assert!(serde_json::from_str::<Request>(r#"{"method":"a","id":1}"#).is_err());
    }

    #[test]
    fn unknown_members_are_rejected() {
        assert!(
            serde_json::from_str::<Request>(r#"{"jsonrpc":"2.0","method":"a","id":1,"x":2}"#)
                .is_err()
        );
    }

    #[test]
    fn fractional_id_is_rejected() {
        assert!(
            serde_json::from_str::<Request>(r#"{"jsonrpc":"2.0","method":"a","id":1.5}"#).is_err()
        );
    }

    #[test]
    fn batch_parses_as_incoming() {
        let incoming: Incoming = serde_json::from_str(
            r#"[{"jsonrpc":"2.0","method":"a","id":1},{"jsonrpc":"2.0","method":"b"}]"#,
        )
        .unwrap();
        let Incoming::Batch(calls) = incoming else {
            panic!("expected a batch");
        };
        assert_eq!(calls.len(), 2);
        assert!(matches!(calls[1], Call::Notification(_)));
    }

    #[test]
    fn outgoing_batch_serialises_as_an_array() {
        let outgoing = Outgoing::Batch(vec![Response::result(RequestId::Number(1), json!(true))]);
        assert_eq!(
            serde_json::to_string(&outgoing).unwrap(),
            r#"[{"jsonrpc":"2.0","result":true,"id":1}]"#,
        );
    }

    #[test]
    fn error_response_serialises_with_a_null_id() {
        let response = Response::error(None, error_codes::parse_error("bad json"));
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({
                "jsonrpc": "2.0",
                "error": { "code": -32700, "message": "Parse error", "data": "bad json" },
                "id": null,
            }),
        );
        assert!(response.is_error());
        assert_eq!(response.value(), None);
    }

    #[test]
    fn sub_errors_map_to_standard_numbers() {
        let cases = [
            ("edit.unknown_command", error_codes::METHOD_NOT_FOUND),
            ("command.unknown_method", error_codes::METHOD_NOT_FOUND),
            ("edit.invalid_command", error_codes::INVALID_PARAMS),
            ("core.invalid_argument", error_codes::INVALID_PARAMS),
            ("command.invalid_params", error_codes::INVALID_PARAMS),
            ("core.internal", error_codes::INTERNAL_ERROR),
            ("edit.clip_not_found", error_codes::SERVER_ERROR),
        ];
        for (code, expected) in cases {
            let error = SubError::new(ErrorCode::parse(code).unwrap(), "nope");
            assert_eq!(error_codes::for_sub_error(&error), expected, "{code}");
        }
    }

    #[test]
    fn rpc_error_keeps_the_sub_error_in_data() {
        let error = SubError::new(ErrorCode::parse("edit.clip_not_found").unwrap(), "no clip")
            .with_detail("clip", "c1");
        let rpc = RpcError::from_sub_error(&error);
        assert_eq!(rpc.code, error_codes::SERVER_ERROR);
        assert_eq!(rpc.message, "no clip");
        assert_eq!(rpc.sub_error(), Some(error));
        assert_eq!(rpc.to_string(), "[-32000] no clip");
    }

    #[test]
    fn ids_display_and_convert() {
        assert_eq!(RequestId::from(3).to_string(), "3");
        assert_eq!(RequestId::from("x").to_string(), "x");
        assert_eq!(
            RequestId::from("x".to_owned()),
            RequestId::Text("x".to_owned())
        );
    }

    #[test]
    fn notification_builder_matches_the_wire_form() {
        let notification = Notification::new("edit.undo", None);
        assert_eq!(
            serde_json::to_string(&notification).unwrap(),
            r#"{"jsonrpc":"2.0","method":"edit.undo"}"#,
        );
        assert_eq!(notification.jsonrpc, Version);
    }
}
