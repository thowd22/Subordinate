//! The Command API: JSON-RPC 2.0 dispatch and the local socket server.
//!
//! One internal API serves the GUI, the CLI, the MCP bridge and plugins so
//! the agent surface is exactly as capable as the UI. Also hosts the engine
//! thread that owns project state. See docs/PLAN.md §4 and §7.
//!
//! - [`agent`] — the agent-facing tool families of docs/PLAN.md §7
//!   (`project.*`, `media.*`, `timeline.*`, `playback.*`) that the engine
//!   alone can serve, and [`host`] — the rest of them, which need decoders, a
//!   GPU and an encoder, so whoever is serving supplies a [`host::Services`].
//! - [`rpc`] — the JSON-RPC 2.0 message types: requests, notifications,
//!   responses, errors and batches.
//! - [`dispatch`] — the [`Dispatcher`], which maps method names to engine
//!   commands and queries and turns every failure into the standard JSON-RPC
//!   error wrapping a [`sub_core::SubError`].
//! - [`events`] — per-connection change-event subscriptions: `events.subscribe`,
//!   `events.unsubscribe` and the `events.changed` notifications a subscriber
//!   receives.
//! - [`endpoint`] — where the server listens: the per-user Unix socket or
//!   Windows named pipe, and the lock file that advertises it.
//! - [`schema`] — the exported JSON Schema of every method, generated from
//!   the command set and committed at `docs/schema/command-api.json`.
//! - [`transport`] — the [`transport::Server`] that serves that endpoint with
//!   newline-delimited JSON framing, and the [`transport::Client`] that speaks
//!   to it from another process.
//!
//! ```
//! use sub_command::Dispatcher;
//! use sub_edit::Engine;
//! use sub_model::Project;
//!
//! let engine = Engine::spawn(Project::new("Doc cut")).unwrap();
//! let dispatcher = Dispatcher::new(engine.handle().clone());
//!
//! let answer = dispatcher
//!     .handle_text(r#"{"jsonrpc":"2.0","method":"project.revision","id":1}"#)
//!     .unwrap();
//! assert_eq!(answer, r#"{"jsonrpc":"2.0","result":{"revision":0},"id":1}"#);
//! engine.shutdown().unwrap();
//! ```

pub mod agent;
pub mod dispatch;
pub mod endpoint;
pub mod events;
pub mod host;
pub mod rpc;
pub mod schema;
pub mod transport;

pub use dispatch::{AppliedResult, Dispatcher, HistoryResult, MethodInfo, MethodSchema};
pub use endpoint::{Address, Endpoint, LockFile, Transport};
pub use events::{ChangedParams, Outbox, Session, SubscriptionId};
pub use rpc::{
    Call, Incoming, Notification, Outgoing, Payload, Request, RequestId, Response, RpcError,
    Version, error_codes,
};
pub use transport::{Client, Server};

/// The error codes this crate produces.
///
/// Codes are part of the public contract with agents and plugins: an existing
/// one is never renamed or given a new meaning (see `sub_core::error`).
pub mod codes {
    use sub_core::ErrorCode;

    /// A request names a method this build does not serve.
    pub const UNKNOWN_METHOD: ErrorCode = ErrorCode::from_static("command.unknown_method");
    /// A request's parameters do not match the method it names.
    pub const INVALID_PARAMS: ErrorCode = ErrorCode::from_static("command.invalid_params");
    /// Two handlers claim the same method name.
    pub const DUPLICATE_METHOD: ErrorCode = ErrorCode::from_static("command.duplicate_method");
    /// An instance name is not usable in a socket path or a pipe name.
    pub const INVALID_INSTANCE: ErrorCode = ErrorCode::from_static("command.invalid_instance");
    /// This machine offers no usable per-user address for the Command API.
    pub const ENDPOINT_UNAVAILABLE: ErrorCode =
        ErrorCode::from_static("command.endpoint_unavailable");
    /// Another live instance is already listening on the endpoint.
    pub const ADDRESS_IN_USE: ErrorCode = ErrorCode::from_static("command.address_in_use");
    /// A lock file exists but is not one this build understands.
    pub const LOCK_FILE_INVALID: ErrorCode = ErrorCode::from_static("command.lock_file_invalid");
    /// No server is listening on the endpoint a client asked for.
    pub const NOT_RUNNING: ErrorCode = ErrorCode::from_static("command.not_running");
    /// The socket or pipe itself failed.
    pub const TRANSPORT_IO: ErrorCode = ErrorCode::from_static("command.transport_io");
    /// A method that acts on a client connection was called without one.
    pub const NO_SESSION: ErrorCode = ErrorCode::from_static("command.no_session");
    /// A subscription id is not one this connection holds.
    pub const UNKNOWN_SUBSCRIPTION: ErrorCode =
        ErrorCode::from_static("command.unknown_subscription");
}

#[cfg(test)]
mod tests {
    use super::codes;

    #[test]
    fn code_constants_are_well_formed() {
        for code in [
            codes::UNKNOWN_METHOD,
            codes::INVALID_PARAMS,
            codes::DUPLICATE_METHOD,
            codes::INVALID_INSTANCE,
            codes::ENDPOINT_UNAVAILABLE,
            codes::ADDRESS_IN_USE,
            codes::LOCK_FILE_INVALID,
            codes::NOT_RUNNING,
            codes::TRANSPORT_IO,
            codes::NO_SESSION,
            codes::UNKNOWN_SUBSCRIPTION,
        ] {
            assert_eq!(code.domain(), "command");
            assert_eq!(
                sub_core::ErrorCode::parse(code.as_str()).as_ref(),
                Ok(&code),
                "{code}",
            );
        }
    }
}
