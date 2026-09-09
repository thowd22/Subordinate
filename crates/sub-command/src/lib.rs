//! The Command API: JSON-RPC 2.0 dispatch and the local socket server.
//!
//! One internal API serves the GUI, the CLI, the MCP bridge and plugins so
//! the agent surface is exactly as capable as the UI. Also hosts the engine
//! thread that owns project state. See docs/PLAN.md §4 and §7.
//!
//! - [`rpc`] — the JSON-RPC 2.0 message types: requests, notifications,
//!   responses, errors and batches.
//! - [`dispatch`] — the [`Dispatcher`], which maps method names to engine
//!   commands and queries and turns every failure into the standard JSON-RPC
//!   error wrapping a [`sub_core::SubError`].
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

pub mod dispatch;
pub mod rpc;

pub use dispatch::{AppliedResult, Dispatcher, HistoryResult, MethodInfo};
pub use rpc::{
    Call, Incoming, Notification, Outgoing, Payload, Request, RequestId, Response, RpcError,
    Version, error_codes,
};

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
