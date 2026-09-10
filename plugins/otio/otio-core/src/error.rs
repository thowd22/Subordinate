//! The failure type both OTIO plugins report, and its code catalogue.
//!
//! A plugin's errors cross the component boundary as the WIT `error` record —
//! a stable machine-readable `code`, a human message and a details map — which
//! mirrors `sub_core::SubError`. `otio-core` knows nothing about WIT, so it
//! carries the same three fields itself and each component converts.
//!
//! Codes are lowercase dot-separated segments, at least two deep, and are
//! never renamed or given a new meaning: agents match on them.

use std::collections::BTreeMap;
use std::fmt;

/// The error codes the OTIO plugins report.
pub mod codes {
    /// The document is not an OTIO timeline this plugin can read: not JSON, no
    /// `OTIO_SCHEMA`, a schema whose name is not one of OTIO's, or a required
    /// field missing.
    pub const INVALID_DOCUMENT: &str = "otio.invalid_document";
    /// The document is valid OTIO but uses something the MVP model has no
    /// place for, such as a nested stack inside a track.
    pub const UNSUPPORTED: &str = "otio.unsupported";
    /// A `RationalTime` rate is not a rate an exact fraction can represent, or
    /// a value is not a whole number of ticks.
    pub const INVALID_TIME: &str = "otio.invalid_time";
    /// The arguments handed to the exporter are not a JSON object of the shape
    /// it documents.
    pub const INVALID_ARGUMENTS: &str = "otio.invalid_arguments";
    /// The project holds no sequence, or none matching the one asked for.
    pub const UNKNOWN_SEQUENCE: &str = "otio.unknown_sequence";
    /// The host's answer to a Command API query was not the shape the Command
    /// API schema says it is.
    pub const INVALID_PROJECT: &str = "otio.invalid_project";
    /// The exporter could not write the file it was asked to write.
    pub const WRITE_FAILED: &str = "otio.write_failed";
}

/// A failure, in the same three parts the WIT `error` record has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// Stable machine-readable code from [`codes`].
    pub code: &'static str,
    /// Human-readable, non-localised message.
    pub message: String,
    /// Structured context for tooling; sorted, so a message is reproducible.
    pub details: BTreeMap<String, String>,
}

impl Error {
    /// Creates an error with `code` and `message` and no details.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: BTreeMap::new(),
        }
    }

    /// The same error with one more detail.
    #[must_use]
    pub fn with(mut self, key: &str, value: impl fmt::Display) -> Self {
        self.details.insert(key.to_owned(), value.to_string());
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Error {}

/// What every fallible conversion in this crate returns.
pub type Result<T> = std::result::Result<T, Error>;

/// An `otio.invalid_document` naming the field that was wrong.
pub fn invalid_document(message: impl Into<String>, field: &str) -> Error {
    Error::new(codes::INVALID_DOCUMENT, message).with("field", field)
}
