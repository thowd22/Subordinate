//! Shared conventions for every Subordinate crate and binary.
//!
//! Two things live here because everything else depends on them and they must
//! not drift:
//!
//! - [`SubError`], the structured error that crosses every API boundary: a
//!   stable [`ErrorCode`], a human message, a details map and a flattened cause
//!   chain, serializable to a stable JSON shape for the Command API, the MCP
//!   bridge and plugins (docs/PLAN.md §6.4).
//! - [`logging`], the `tracing` setup used by all three binaries, with an
//!   env-filter and a JSON output option.
//! - [`jobs`], the background job service every long-running piece of work
//!   (thumbnails, waveforms, proxies, PTS indexes, plugin analyzers) is run
//!   through: priorities, cooperative cancellation and progress events
//!   (docs/PLAN.md §5.2).
//!
//! The convention itself — when to add a code, how to wrap lower-level errors —
//! is documented in `docs/DEVELOPMENT.md`.
//!
//! ```
//! use sub_core::{ResultExt, SubError, SubResult, codes};
//!
//! fn open(path: &str) -> SubResult<String> {
//!     std::fs::read_to_string(path)
//!         .sub_context_with(codes::IO, || format!("could not read {path}"))
//! }
//!
//! let err: SubError = open("/definitely/not/here.sub").unwrap_err();
//! assert_eq!(err.code.as_str(), "core.io");
//! assert!(err.cause.is_some());
//! ```

pub mod error;
pub mod jobs;
pub mod logging;

pub use error::{ErrorCode, InvalidErrorCode, ResultExt, SubError, SubResult, codes};
pub use jobs::{
    CancelToken, JobContext, JobEvent, JobHandle, JobId, JobOutcome, JobService, JobState, Priority,
};
pub use logging::{LogConfig, LogFormat};
