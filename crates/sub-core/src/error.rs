//! `SubError`: the one error type that crosses a Subordinate API boundary.
//!
//! Every failure that reaches the Command API, the MCP bridge or a plugin is a
//! [`SubError`]: a stable machine-readable [`ErrorCode`], a human message, an
//! optional details map and an optional rendered cause chain. Agents match on
//! the code, humans read the message, tooling reads the details.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Result alias for fallible Subordinate APIs.
pub type SubResult<T> = Result<T, SubError>;

/// A stable, machine-readable error code such as `media.decode_failed`.
///
/// Codes are lowercase dot-separated segments of `[a-z0-9_]`, at least two
/// segments deep: a domain (`model`, `media`, `plugin`, …) and a reason. They
/// are part of the public contract with agents and plugins, so an existing code
/// is never renamed or given a new meaning; a new situation gets a new code.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ErrorCode(Cow<'static, str>);

impl ErrorCode {
    /// Builds a code from a literal, without validation.
    ///
    /// Intended for the constants in [`codes`] and for other crates declaring
    /// their own code constants. `const` construction cannot validate, so the
    /// `code_constants_are_well_formed` tests guard the registry instead; use
    /// [`ErrorCode::parse`] for anything built at runtime.
    #[must_use]
    pub const fn from_static(code: &'static str) -> Self {
        Self(Cow::Borrowed(code))
    }

    /// Parses and validates a code produced at runtime (a plugin manifest, a
    /// deserialized message).
    ///
    /// # Errors
    ///
    /// Returns [`InvalidErrorCode`] if `code` is not two or more dot-separated
    /// segments of lowercase ASCII letters, digits and underscores.
    pub fn parse(code: impl Into<String>) -> Result<Self, InvalidErrorCode> {
        let code = code.into();
        if !is_well_formed(&code) {
            return Err(InvalidErrorCode(code));
        }
        Ok(Self(Cow::Owned(code)))
    }

    /// The code as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The domain segment: everything before the first dot.
    #[must_use]
    pub fn domain(&self) -> &str {
        self.0.split('.').next().unwrap_or(&self.0)
    }
}

/// Returns true if `code` is a well-formed [`ErrorCode`] string.
fn is_well_formed(code: &str) -> bool {
    let mut segments = 0_usize;
    for segment in code.split('.') {
        if segment.is_empty()
            || !segment
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return false;
        }
        segments += 1;
    }
    segments >= 2
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<ErrorCode> for String {
    fn from(code: ErrorCode) -> Self {
        code.0.into_owned()
    }
}

impl TryFrom<String> for ErrorCode {
    type Error = InvalidErrorCode;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

/// A string that is not a valid [`ErrorCode`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid error code {0:?}: expected two or more dot-separated segments of [a-z0-9_], \
     e.g. `media.decode_failed`"
)]
pub struct InvalidErrorCode(pub String);

/// The codes owned by the core layer.
///
/// Crates add their own constants in their own domain (`media.*`, `plugin.*`,
/// …); these are the ones shared by everything.
pub mod codes {
    use super::ErrorCode;

    /// A caller supplied an argument that is invalid on its face.
    pub const INVALID_ARGUMENT: ErrorCode = ErrorCode::from_static("core.invalid_argument");
    /// A named entity (clip, track, media item, plugin) does not exist.
    pub const NOT_FOUND: ErrorCode = ErrorCode::from_static("core.not_found");
    /// The request is well formed but not allowed in the current state.
    pub const INVALID_STATE: ErrorCode = ErrorCode::from_static("core.invalid_state");
    /// The operation is not implemented yet.
    pub const UNIMPLEMENTED: ErrorCode = ErrorCode::from_static("core.unimplemented");
    /// The operation was cancelled by the caller or by shutdown.
    pub const CANCELLED: ErrorCode = ErrorCode::from_static("core.cancelled");
    /// The operation exceeded its time budget.
    pub const TIMEOUT: ErrorCode = ErrorCode::from_static("core.timeout");
    /// An underlying I/O operation failed.
    pub const IO: ErrorCode = ErrorCode::from_static("core.io");
    /// A bug: an invariant the code is supposed to maintain did not hold.
    pub const INTERNAL: ErrorCode = ErrorCode::from_static("core.internal");
    /// Logging could not be initialised.
    pub const LOGGING_INIT: ErrorCode = ErrorCode::from_static("core.logging_init");
}

/// A structured, serializable error crossing a Subordinate API boundary.
///
/// The JSON shape is stable:
///
/// ```json
/// {
///   "code": "media.decode_failed",
///   "message": "could not decode frame 42",
///   "details": { "path": "/tmp/a.mp4" },
///   "cause": "gstreamer: pipeline failed to start"
/// }
/// ```
///
/// `details` and `cause` are omitted when empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubError {
    /// Stable machine-readable code.
    pub code: ErrorCode,
    /// Human-readable, one line, no trailing period.
    pub message: String,
    /// Machine-readable specifics: field paths, ids, file names.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, serde_json::Value>,
    /// The rendered display chain of the lower-level error this wraps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
}

impl SubError {
    /// Creates an error with a code and a message.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: BTreeMap::new(),
            cause: None,
        }
    }

    /// Creates an error that wraps a lower-level error as its cause.
    ///
    /// The cause is flattened to its full `source()` chain at construction
    /// time, so the result stays `Clone`, `Send` and serializable.
    #[must_use]
    pub fn wrap(code: ErrorCode, message: impl Into<String>, source: &dyn StdError) -> Self {
        Self::new(code, message).with_cause(source)
    }

    /// Attaches (or replaces) the rendered cause chain of `source`.
    #[must_use]
    pub fn with_cause(mut self, source: &dyn StdError) -> Self {
        self.cause = Some(render_chain(source));
        self
    }

    /// Attaches one detail, serializing `value` as JSON.
    ///
    /// A value that cannot be serialized is stored as its `Debug`-free error
    /// string rather than losing the detail or failing the call.
    #[must_use]
    pub fn with_detail(mut self, key: impl Into<String>, value: impl Serialize) -> Self {
        let value = serde_json::to_value(value)
            .unwrap_or_else(|err| serde_json::Value::String(err.to_string()));
        self.details.insert(key.into(), value);
        self
    }

    /// Serializes to the stable JSON object.
    ///
    /// # Panics
    ///
    /// Never in practice: every field is plain JSON-compatible data.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("SubError is always serializable")
    }
}

impl fmt::Display for SubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)?;
        if let Some(cause) = &self.cause {
            write!(f, ": {cause}")?;
        }
        Ok(())
    }
}

impl StdError for SubError {}

/// Renders `source` and its `source()` chain as `a: b: c`.
fn render_chain(source: &dyn StdError) -> String {
    let mut rendered = source.to_string();
    let mut next = source.source();
    while let Some(err) = next {
        rendered.push_str(": ");
        rendered.push_str(&err.to_string());
        next = err.source();
    }
    rendered
}

/// Adds `SubError` context to any `Result` whose error implements [`StdError`].
///
/// ```
/// use sub_core::{ResultExt, codes};
///
/// let parsed: Result<i32, _> = "x".parse::<i32>();
/// let err = parsed
///     .sub_context(codes::INVALID_ARGUMENT, "frame rate must be an integer")
///     .unwrap_err();
/// assert_eq!(err.code.as_str(), "core.invalid_argument");
/// assert!(err.cause.is_some());
/// ```
pub trait ResultExt<T> {
    /// Converts the error into a [`SubError`] with `code` and `message`,
    /// keeping the original as the cause chain.
    ///
    /// # Errors
    ///
    /// Returns the converted [`SubError`] when `self` is `Err`.
    fn sub_context(self, code: ErrorCode, message: impl Into<String>) -> SubResult<T>;

    /// Like [`ResultExt::sub_context`] but builds the message lazily.
    ///
    /// # Errors
    ///
    /// Returns the converted [`SubError`] when `self` is `Err`.
    fn sub_context_with<F, S>(self, code: ErrorCode, message: F) -> SubResult<T>
    where
        F: FnOnce() -> S,
        S: Into<String>;
}

impl<T, E: StdError> ResultExt<T> for Result<T, E> {
    fn sub_context(self, code: ErrorCode, message: impl Into<String>) -> SubResult<T> {
        self.map_err(|err| SubError::wrap(code, message, &err))
    }

    fn sub_context_with<F, S>(self, code: ErrorCode, message: F) -> SubResult<T>
    where
        F: FnOnce() -> S,
        S: Into<String>,
    {
        self.map_err(|err| SubError::wrap(code, message(), &err))
    }
}

impl From<std::io::Error> for SubError {
    fn from(err: std::io::Error) -> Self {
        Self::wrap(codes::IO, "I/O operation failed", &err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("inner boom")]
    struct Inner;

    #[derive(Debug, thiserror::Error)]
    #[error("outer boom")]
    struct Outer(#[source] Inner);

    #[test]
    fn code_constants_are_well_formed() {
        for code in [
            codes::INVALID_ARGUMENT,
            codes::NOT_FOUND,
            codes::INVALID_STATE,
            codes::UNIMPLEMENTED,
            codes::CANCELLED,
            codes::TIMEOUT,
            codes::IO,
            codes::INTERNAL,
            codes::LOGGING_INIT,
        ] {
            assert!(is_well_formed(code.as_str()), "malformed code {code}");
            assert_eq!(code.domain(), "core");
        }
    }

    #[test]
    fn parse_rejects_malformed_codes() {
        for bad in [
            "",
            "media",
            "media.",
            ".decode",
            "Media.Decode",
            "media..x",
            "media decode",
        ] {
            assert!(ErrorCode::parse(bad).is_err(), "accepted {bad:?}");
        }
        assert_eq!(
            ErrorCode::parse("plugin.host.trap").unwrap().as_str(),
            "plugin.host.trap"
        );
    }

    #[test]
    fn json_shape_is_stable_and_omits_empty_fields() {
        let err = SubError::new(codes::NOT_FOUND, "no such clip");
        assert_eq!(
            serde_json::to_string(&err).unwrap(),
            r#"{"code":"core.not_found","message":"no such clip"}"#
        );

        let err = err.with_detail("clip_id", "c-7").with_detail("track", 2);
        assert_eq!(
            serde_json::to_string(&err).unwrap(),
            r#"{"code":"core.not_found","message":"no such clip","details":{"clip_id":"c-7","track":2}}"#
        );
    }

    #[test]
    fn round_trips_through_json() {
        let err = SubError::wrap(codes::IO, "could not read project", &Outer(Inner))
            .with_detail("path", "/tmp/a.sub");
        let json = err.to_json();
        let back: SubError = serde_json::from_value(json).unwrap();
        assert_eq!(back, err);
    }

    #[test]
    fn deserializing_rejects_a_malformed_code() {
        let json = r#"{"code":"Bad Code","message":"x"}"#;
        assert!(serde_json::from_str::<SubError>(json).is_err());
    }

    #[test]
    fn wrapping_flattens_the_whole_source_chain() {
        let err = SubError::wrap(codes::INTERNAL, "load failed", &Outer(Inner));
        assert_eq!(err.cause.as_deref(), Some("outer boom: inner boom"));
        assert_eq!(
            err.to_string(),
            "[core.internal] load failed: outer boom: inner boom"
        );
    }

    #[test]
    fn result_ext_adds_context_lazily_only_on_error() {
        let ok: Result<i32, std::num::ParseIntError> = Ok(1);
        assert_eq!(
            ok.sub_context(codes::INVALID_ARGUMENT, "unused").unwrap(),
            1
        );

        let err = "nope"
            .parse::<i32>()
            .sub_context_with(codes::INVALID_ARGUMENT, || "frame rate must be an integer")
            .unwrap_err();
        assert_eq!(err.code, codes::INVALID_ARGUMENT);
        assert_eq!(err.message, "frame rate must be an integer");
        assert!(err.cause.is_some());
    }

    #[test]
    fn io_errors_convert_with_the_io_code() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing.mp4");
        let err = SubError::from(io);
        assert_eq!(err.code, codes::IO);
        assert_eq!(err.cause.as_deref(), Some("missing.mp4"));
    }
}
