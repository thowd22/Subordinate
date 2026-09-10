//! Calling the host: typed commands, queries, project metadata and logging.
//!
//! The generated `command-api` import speaks JSON strings. These helpers put
//! the serde layer around it, so a plugin writes a parameter struct and reads a
//! result type instead of building and parsing documents:
//!
//! ```ignore
//! use subordinate_sdk::params::TrimClipIn;
//! use subordinate_sdk::time::{frames, rate};
//! use subordinate_sdk::{Project, Result};
//!
//! fn shorten(project: &Project, sequence: SequenceId, track: TrackId, clip: ClipId) -> Result<()> {
//!     let applied = project.run(&TrimClipIn {
//!         sequence,
//!         track,
//!         clip,
//!         delta: frames(1, rate::FPS_24),
//!     })?;
//!     subordinate_sdk::info(&format!("undo step {:?} at revision {}", applied.label, applied.revision));
//!     Ok(())
//! }
//! ```
//!
//! Each successful [`Project::run`] is one undoable command on the host's undo
//! stack — the same stack the GUI, the CLI and the MCP bridge push onto — so a
//! plugin edit is undone with Ctrl+Z like any other. [`Project::query`] cannot
//! mutate anything.
//!
//! # Failures
//!
//! Everything returns [`Result`], whose error is the WIT [`Error`] record: a
//! stable `code`, a one-line message and details. A host failure is passed
//! through unchanged, so matching on [`crate::errors`]' code constants works on
//! it. A failure inside the SDK — a `params` value that will not serialise, or
//! a `result` that does not match the type asked for — becomes
//! `plugin.invalid_argument` or `plugin.invalid_result`, with the method name
//! and the serde message in its details, because those are the two things
//! needed to find the mistake.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::bindings::subordinate::plugin::command_api::{
    self, ClipMetadata, LogLevel, MarkerMetadata, ProjectMetadata, SequenceMetadata, TrackMetadata,
};
use crate::bindings::subordinate::plugin::types::{
    Detail, Error, ProjectId, RationalTime, SequenceId, TrackId,
};
use crate::params::CommandParams;

/// What every SDK call returns: a value, or the error the host would have
/// reported.
pub type Result<T> = core::result::Result<T, Error>;

/// The code a plugin's own bad `params` gets.
const INVALID_ARGUMENT: &str = "plugin.invalid_argument";
/// The code a `result` that does not match the expected type gets.
const INVALID_RESULT: &str = "plugin.invalid_result";

/// Builds an [`Error`] with a stable code and a message.
///
/// Use it for a plugin's own failures, so they reach the host in the same
/// shape as the host's own: a code an agent can match on, a message a human can
/// read, and details a tool can use.
#[must_use]
pub fn error(code: impl Into<String>, message: impl Into<String>) -> Error {
    Error {
        code: code.into(),
        message: message.into(),
        details: Vec::new(),
    }
}

/// Adds one `key`/`value` detail to an error, returning it.
///
/// `value` is a JSON fragment, because WIT has no dynamic JSON type: a string
/// detail is quoted, `"\"clip-1\""`.
#[must_use]
pub fn with_detail(mut error: Error, key: impl Into<String>, value: impl Into<String>) -> Error {
    error.details.push(Detail {
        key: key.into(),
        value: value.into(),
    });
    error
}

/// Writes one line into the host's `tracing` subscriber.
pub fn log(level: LogLevel, message: &str) {
    command_api::log(level, message);
}

/// Logs at trace level.
pub fn trace(message: &str) {
    log(LogLevel::Trace, message);
}

/// Logs at debug level.
pub fn debug(message: &str) {
    log(LogLevel::Debug, message);
}

/// Logs at info level.
pub fn info(message: &str) {
    log(LogLevel::Info, message);
}

/// Logs at warn level.
pub fn warn(message: &str) {
    log(LogLevel::Warn, message);
}

/// Logs at error level.
pub fn error_log(message: &str) {
    log(LogLevel::Error, message);
}

/// Every project open in the host, in the order it opened them.
///
/// A plugin normally acts on the project it was handed rather than one it went
/// looking for; this is for the tools that legitimately span projects, such as
/// an MCP tool that reports on the whole session.
#[must_use]
pub fn open_projects() -> Vec<Project> {
    command_api::open_projects()
        .into_iter()
        .map(Project)
        .collect()
}

/// One open project: the handle every host call hangs off.
///
/// It is a thin wrapper around a [`ProjectId`] — the identifier is what crosses
/// the boundary — so it is free to clone and to keep.
#[derive(Debug, Clone)]
pub struct Project(ProjectId);

impl Project {
    /// The handle for `id`.
    #[must_use]
    pub const fn new(id: ProjectId) -> Self {
        Self(id)
    }

    /// The project's identifier.
    #[must_use]
    pub const fn id(&self) -> &ProjectId {
        &self.0
    }

    /// Runs one undoable command and returns what it did.
    ///
    /// The method name comes from `P`, so it cannot be paired with the wrong
    /// parameters.
    ///
    /// # Errors
    ///
    /// Returns the host's own error when the command is rejected —
    /// `edit.unknown_clip`, `plugin.capability_denied` and the rest — and
    /// `plugin.invalid_argument` or `plugin.invalid_result` when the SDK cannot
    /// encode the parameters or decode the result.
    pub fn run<P: CommandParams>(&self, params: &P) -> Result<P::Output> {
        let encoded = encode(P::METHOD, params)?;
        let result = command_api::run_command(&self.0, P::METHOD, &encoded)?;
        decode(P::METHOD, &result)
    }

    /// Runs one read-only query and returns its result.
    ///
    /// # Errors
    ///
    /// As [`Project::run`].
    pub fn query<P: CommandParams>(&self, params: &P) -> Result<P::Output> {
        let encoded = encode(P::METHOD, params)?;
        let result = command_api::query(&self.0, P::METHOD, &encoded)?;
        decode(P::METHOD, &result)
    }

    /// Runs one undoable command whose parameters the SDK does not type.
    ///
    /// This is the escape hatch for the commands that carry a whole project
    /// entity, and for the methods a plugin itself contributed. `params` is any
    /// serialisable value, `R` any deserialisable one — `serde_json::Value` at
    /// both ends when nothing better fits.
    ///
    /// # Errors
    ///
    /// As [`Project::run`].
    pub fn run_json<P: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<R> {
        let encoded = encode(method, params)?;
        let result = command_api::run_command(&self.0, method, &encoded)?;
        decode(method, &result)
    }

    /// Runs one read-only query whose parameters the SDK does not type.
    ///
    /// # Errors
    ///
    /// As [`Project::run`].
    pub fn query_json<P: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<R> {
        let encoded = encode(method, params)?;
        let result = command_api::query(&self.0, method, &encoded)?;
        decode(method, &result)
    }

    /// The project's name, path and timebase.
    ///
    /// # Errors
    ///
    /// Returns the host's error when the project is not open.
    pub fn info(&self) -> Result<ProjectMetadata> {
        command_api::project_info(&self.0)
    }

    /// Every sequence in the project.
    ///
    /// # Errors
    ///
    /// Returns the host's error when the project is not open.
    pub fn sequences(&self) -> Result<Vec<SequenceMetadata>> {
        command_api::sequences(&self.0)
    }

    /// Every track in `sequence`, in composite order.
    ///
    /// # Errors
    ///
    /// Returns the host's error when the sequence is unknown.
    pub fn tracks(&self, sequence: &SequenceId) -> Result<Vec<TrackMetadata>> {
        command_api::tracks(&self.0, sequence)
    }

    /// Every clip on `track`, in timeline order.
    ///
    /// # Errors
    ///
    /// Returns the host's error when the sequence or the track is unknown.
    pub fn clips(&self, sequence: &SequenceId, track: &TrackId) -> Result<Vec<ClipMetadata>> {
        command_api::clips(&self.0, sequence, track)
    }

    /// Every marker on `sequence`.
    ///
    /// # Errors
    ///
    /// Returns the host's error when the sequence is unknown.
    pub fn markers(&self, sequence: &SequenceId) -> Result<Vec<MarkerMetadata>> {
        command_api::markers(&self.0, sequence)
    }

    /// Where the playhead is, in the active sequence's time base.
    ///
    /// # Errors
    ///
    /// Returns the host's error when the project is not open.
    pub fn playhead(&self) -> Result<RationalTime> {
        command_api::playhead(&self.0)
    }

    /// Every clip in `sequence`, paired with the track it sits on.
    ///
    /// The common shape of an analysis pass, which otherwise repeats the same
    /// two nested loops in every plugin.
    ///
    /// # Errors
    ///
    /// Returns the host's error when the sequence is unknown.
    pub fn clips_in(&self, sequence: &SequenceId) -> Result<Vec<(TrackId, ClipMetadata)>> {
        let mut all = Vec::new();
        for track in self.tracks(sequence)? {
            for clip in self.clips(sequence, &track.id)? {
                all.push((track.id.clone(), clip));
            }
        }
        Ok(all)
    }
}

impl From<ProjectId> for Project {
    fn from(id: ProjectId) -> Self {
        Self::new(id)
    }
}

/// Serialises `params`, turning a serde failure into a plugin error naming the
/// method it belongs to.
fn encode<P: Serialize + ?Sized>(method: &str, params: &P) -> Result<String> {
    serde_json::to_string(params).map_err(|failure| {
        with_detail(
            with_detail(
                error(
                    INVALID_ARGUMENT,
                    "the command parameters could not be encoded",
                ),
                "method",
                format!("{method:?}"),
            ),
            "reason",
            format!("{failure}"),
        )
    })
}

/// Deserialises a `result` document, turning a serde failure into a plugin
/// error naming the method and quoting what came back.
fn decode<R: DeserializeOwned>(method: &str, result: &str) -> Result<R> {
    serde_json::from_str(result).map_err(|failure| {
        with_detail(
            with_detail(
                with_detail(
                    error(INVALID_RESULT, "the command result did not match its type"),
                    "method",
                    format!("{method:?}"),
                ),
                "reason",
                format!("{failure}"),
            ),
            "result",
            format!("{result:?}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{Project, decode, encode, error, with_detail};
    use crate::bindings::subordinate::plugin::types::ProjectId;
    use crate::ids::Id;
    use crate::params::RevisionResult;

    #[test]
    fn a_handle_is_the_identifier_and_nothing_else() {
        let project = Project::from(ProjectId::parse("018f-proj"));
        assert_eq!(project.id().as_str(), "018f-proj");
        assert_eq!(project.clone().id().as_str(), "018f-proj");
    }

    #[test]
    fn an_error_carries_a_code_and_its_details() {
        let failure = with_detail(error("plugin.internal", "boom"), "clip", "\"c1\"");
        assert_eq!(failure.code, "plugin.internal");
        assert_eq!(failure.details.len(), 1);
        assert_eq!(failure.details[0].value, "\"c1\"");
    }

    #[test]
    fn a_result_that_does_not_match_its_type_names_the_method() {
        let failure = decode::<RevisionResult>("project.revision", "{}").unwrap_err();
        assert_eq!(failure.code, "plugin.invalid_result");
        let keys: Vec<&str> = failure.details.iter().map(|d| d.key.as_str()).collect();
        assert_eq!(keys, ["method", "reason", "result"]);
        assert_eq!(failure.details[0].value, "\"project.revision\"");
    }

    #[test]
    fn a_well_formed_result_decodes() {
        let decoded: RevisionResult = decode("project.revision", r#"{"revision":7}"#).unwrap();
        assert_eq!(decoded.revision, 7);
    }

    #[test]
    fn parameters_encode_to_the_json_the_socket_would_carry() {
        let encoded = encode(
            "edit.begin_group",
            &crate::params::BeginGroup {
                label: "Cut silence".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(encoded, r#"{"label":"Cut silence"}"#);
    }
}
