//! The [`Command`] trait, its type-erased form and the registry that turns
//! logged or transmitted JSON back into commands.
//!
//! A command is a small, serialisable, self-contained mutation of a
//! [`Project`]. Applying one returns its [`Inverse`], which is itself a
//! command: undo is nothing more than applying the inverse, and the inverse of
//! the inverse is the redo. That symmetry is what keeps `undo` then `redo`
//! byte-identical in the project file.
//!
//! Two rules every implementation must keep:
//!
//! - **Atomic.** A command either applies completely or leaves the project
//!   untouched and returns a [`SubError`]. Nothing rolls back a half-applied
//!   command, so validate first and mutate afterwards.
//! - **Exact.** The returned inverse must restore the project to the state it
//!   had before the command ran, field for field, including identifiers.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sub_core::{SubError, SubResult};
use sub_model::Project;

use crate::codes;

/// A single undoable mutation of a project.
///
/// Implementors are plain serde types: the Command API sends them as JSON, the
/// history logs them as JSON, and plugins build them as JSON. See the module
/// documentation for the atomicity and exactness rules.
///
/// ```
/// use serde::{Deserialize, Serialize};
/// use sub_core::SubResult;
/// use sub_edit::{Command, Inverse};
/// use sub_model::Project;
///
/// #[derive(Debug, Serialize, Deserialize)]
/// struct RenameProject {
///     name: String,
/// }
///
/// impl Command for RenameProject {
///     const KIND: &'static str = "project.rename";
///
///     fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
///         let previous = std::mem::replace(&mut project.name, self.name.clone());
///         Ok(Inverse::new(RenameProject { name: previous }))
///     }
/// }
///
/// let mut project = Project::new("Untitled");
/// let inverse = RenameProject { name: "Doc cut".to_owned() }
///     .apply(&mut project)
///     .unwrap();
/// assert_eq!(project.name, "Doc cut");
/// inverse.command().apply_erased(&mut project).unwrap();
/// assert_eq!(project.name, "Untitled");
/// ```
pub trait Command: Serialize + DeserializeOwned + fmt::Debug + Send + Sync + 'static {
    /// The stable wire name of this command, such as `clip.trim_in`.
    ///
    /// It is part of the public contract with agents and plugins: an existing
    /// kind is never renamed or given a new meaning.
    const KIND: &'static str;

    /// Applies the command and returns the command that undoes it.
    ///
    /// # Errors
    ///
    /// Returns a [`SubError`] when the command cannot be applied. The project
    /// must be left untouched in that case.
    fn apply(&self, project: &mut Project) -> SubResult<Inverse>;

    /// The label shown in the undo menu and the history panel.
    ///
    /// Defaults to [`Command::KIND`]; override it for something a human reads,
    /// like `Trim clip in`.
    fn label(&self) -> String {
        Self::KIND.to_owned()
    }
}

/// The type-erased form of a [`Command`], used by the history and the registry.
///
/// It is implemented for every [`Command`] by a blanket impl; never implement
/// it directly.
pub trait AnyCommand: fmt::Debug + Send + Sync + 'static {
    /// The command's stable wire name, [`Command::KIND`].
    fn kind(&self) -> &'static str;

    /// The command's human label, [`Command::label`].
    fn label_erased(&self) -> String;

    /// Applies the command, as [`Command::apply`].
    ///
    /// # Errors
    ///
    /// Whatever [`Command::apply`] returns.
    fn apply_erased(&self, project: &mut Project) -> SubResult<Inverse>;

    /// The command's parameters as JSON.
    ///
    /// # Errors
    ///
    /// Returns `edit.invalid_command` if the command cannot be serialised,
    /// which would be a bug in the command rather than in the caller.
    fn to_value(&self) -> SubResult<Value>;

    /// The command as the envelope sent over the Command API and written to
    /// the log.
    ///
    /// # Errors
    ///
    /// The same as [`AnyCommand::to_value`].
    fn to_envelope(&self) -> SubResult<CommandEnvelope> {
        Ok(CommandEnvelope {
            kind: self.kind().to_owned(),
            params: self.to_value()?,
        })
    }
}

impl<C: Command> AnyCommand for C {
    fn kind(&self) -> &'static str {
        C::KIND
    }

    fn label_erased(&self) -> String {
        Command::label(self)
    }

    fn apply_erased(&self, project: &mut Project) -> SubResult<Inverse> {
        Command::apply(self, project)
    }

    fn to_value(&self) -> SubResult<Value> {
        serde_json::to_value(self).map_err(|err| {
            SubError::wrap(
                codes::INVALID_COMMAND,
                "command could not be serialised",
                &err,
            )
            .with_detail("kind", C::KIND)
        })
    }
}

/// An owned command of an unknown concrete type.
pub type BoxedCommand = Box<dyn AnyCommand>;

/// The command that undoes another command.
///
/// It is a command like any other, so the history can apply it to undo and
/// keep what that returns to redo.
#[derive(Debug)]
pub struct Inverse(BoxedCommand);

impl Inverse {
    /// Wraps a concrete command as an inverse.
    #[must_use]
    pub fn new<C: Command>(command: C) -> Self {
        Self(Box::new(command))
    }

    /// Wraps an already-boxed command as an inverse.
    #[must_use]
    pub fn from_boxed(command: BoxedCommand) -> Self {
        Self(command)
    }

    /// The command that performs the undo.
    #[must_use]
    pub fn command(&self) -> &dyn AnyCommand {
        self.0.as_ref()
    }

    /// Takes the command out of the inverse.
    #[must_use]
    pub fn into_command(self) -> BoxedCommand {
        self.0
    }
}

/// A command on the wire: its stable kind plus its parameters.
///
/// The JSON shape is stable, because it is what the Command API carries, what
/// the history log stores and what plugins build:
///
/// ```json
/// { "kind": "project.rename", "params": { "name": "Doc cut" } }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandEnvelope {
    /// The command's stable wire name, [`Command::KIND`].
    pub kind: String,
    /// The command's parameters, as its serde representation.
    pub params: Value,
}

impl CommandEnvelope {
    /// Builds an envelope from a kind and already-serialised parameters.
    #[must_use]
    pub fn new(kind: impl Into<String>, params: Value) -> Self {
        Self {
            kind: kind.into(),
            params,
        }
    }
}

type Decoder = fn(Value) -> SubResult<BoxedCommand>;

/// The set of command kinds this build can decode.
///
/// The Command API, the MCP bridge and the plugin host all receive commands as
/// JSON; the registry is what turns that JSON back into something applicable,
/// and what a `list methods` call enumerates.
///
/// ```
/// # use serde::{Deserialize, Serialize};
/// # use sub_core::SubResult;
/// # use sub_edit::{Command, CommandEnvelope, CommandRegistry, Inverse};
/// # use sub_model::Project;
/// # #[derive(Debug, Serialize, Deserialize)]
/// # struct RenameProject { name: String }
/// # impl Command for RenameProject {
/// #     const KIND: &'static str = "project.rename";
/// #     fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
/// #         let previous = std::mem::replace(&mut project.name, self.name.clone());
/// #         Ok(Inverse::new(RenameProject { name: previous }))
/// #     }
/// # }
/// let mut registry = CommandRegistry::new();
/// registry.register::<RenameProject>().unwrap();
///
/// let envelope = CommandEnvelope::new(
///     "project.rename",
///     serde_json::json!({ "name": "Doc cut" }),
/// );
/// let command = registry.decode(&envelope).unwrap();
///
/// let mut project = Project::new("Untitled");
/// command.apply_erased(&mut project).unwrap();
/// assert_eq!(project.name, "Doc cut");
/// ```
#[derive(Clone, Default)]
pub struct CommandRegistry {
    decoders: BTreeMap<&'static str, Decoder>,
}

impl CommandRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a command kind.
    ///
    /// # Errors
    ///
    /// Returns `edit.duplicate_command` if the kind is already registered:
    /// two commands sharing a wire name would make decoding ambiguous.
    pub fn register<C: Command>(&mut self) -> SubResult<()> {
        if self.decoders.contains_key(C::KIND) {
            return Err(SubError::new(
                codes::DUPLICATE_COMMAND,
                "command kind is already registered",
            )
            .with_detail("kind", C::KIND));
        }
        self.decoders.insert(C::KIND, decode_as::<C>);
        Ok(())
    }

    /// Every registered kind, in lexicographic order.
    pub fn kinds(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.decoders.keys().copied()
    }

    /// Whether `kind` is registered.
    #[must_use]
    pub fn contains(&self, kind: &str) -> bool {
        self.decoders.contains_key(kind)
    }

    /// The number of registered kinds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.decoders.len()
    }

    /// Whether nothing is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.decoders.is_empty()
    }

    /// Decodes an envelope into an applicable command.
    ///
    /// # Errors
    ///
    /// - `edit.unknown_command` when the kind is not registered.
    /// - `edit.invalid_command` when the parameters do not match the command.
    pub fn decode(&self, envelope: &CommandEnvelope) -> SubResult<BoxedCommand> {
        let decoder = self.decoders.get(envelope.kind.as_str()).ok_or_else(|| {
            SubError::new(codes::UNKNOWN_COMMAND, "no such command kind")
                .with_detail("kind", &envelope.kind)
        })?;
        decoder(envelope.params.clone())
    }

    /// Decodes a whole sequence of envelopes, such as one history entry.
    ///
    /// # Errors
    ///
    /// The same as [`CommandRegistry::decode`], with an `index` detail naming
    /// the envelope that failed.
    pub fn decode_all(&self, envelopes: &[CommandEnvelope]) -> SubResult<Vec<BoxedCommand>> {
        envelopes
            .iter()
            .enumerate()
            .map(|(index, envelope)| {
                self.decode(envelope)
                    .map_err(|err| err.with_detail("index", index))
            })
            .collect()
    }
}

impl fmt::Debug for CommandRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandRegistry")
            .field("kinds", &self.decoders.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// The decoder stored for one command kind.
fn decode_as<C: Command>(params: Value) -> SubResult<BoxedCommand> {
    let command: C = serde_json::from_value(params).map_err(|err| {
        SubError::wrap(
            codes::INVALID_COMMAND,
            "command parameters do not match the command",
            &err,
        )
        .with_detail("kind", C::KIND)
    })?;
    Ok(Box::new(command))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_commands::{Rename, SetName};

    #[test]
    fn applying_a_command_returns_an_exact_inverse() {
        let mut project = Project::new("Untitled");
        let inverse = SetName::new("Doc cut").apply(&mut project).unwrap();
        assert_eq!(project.name, "Doc cut");
        assert_eq!(inverse.command().kind(), SetName::KIND);

        inverse.command().apply_erased(&mut project).unwrap();
        assert_eq!(project.name, "Untitled");
    }

    #[test]
    fn a_command_round_trips_through_its_envelope() {
        let mut registry = CommandRegistry::new();
        registry.register::<SetName>().unwrap();

        let envelope = SetName::new("Doc cut").to_envelope().unwrap();
        assert_eq!(envelope.kind, "test.set_name");
        assert_eq!(
            serde_json::to_string(&envelope).unwrap(),
            r#"{"kind":"test.set_name","params":{"name":"Doc cut"}}"#
        );

        let decoded = registry.decode(&envelope).unwrap();
        let mut project = Project::new("Untitled");
        decoded.apply_erased(&mut project).unwrap();
        assert_eq!(project.name, "Doc cut");
    }

    #[test]
    fn registering_the_same_kind_twice_is_refused() {
        let mut registry = CommandRegistry::new();
        registry.register::<SetName>().unwrap();
        let err = registry.register::<SetName>().unwrap_err();
        assert_eq!(err.code, codes::DUPLICATE_COMMAND);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn decoding_reports_unknown_kinds_and_bad_parameters() {
        let mut registry = CommandRegistry::new();
        registry.register::<SetName>().unwrap();
        assert!(registry.contains("test.set_name"));
        assert!(!registry.is_empty());
        assert_eq!(registry.kinds().collect::<Vec<_>>(), ["test.set_name"]);

        let unknown = CommandEnvelope::new("test.nope", serde_json::json!({}));
        let err = registry.decode(&unknown).unwrap_err();
        assert_eq!(err.code, codes::UNKNOWN_COMMAND);

        let bad = CommandEnvelope::new("test.set_name", serde_json::json!({ "name": 7 }));
        let err = registry.decode(&bad).unwrap_err();
        assert_eq!(err.code, codes::INVALID_COMMAND);

        let err = registry
            .decode_all(&[SetName::new("a").to_envelope().unwrap(), bad])
            .unwrap_err();
        assert_eq!(err.details.get("index"), Some(&serde_json::json!(1)));
    }

    #[test]
    fn an_empty_registry_decodes_nothing() {
        let registry = CommandRegistry::default();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(registry.kinds().next().is_none());
        assert!(format!("{registry:?}").contains("CommandRegistry"));
    }

    #[test]
    fn a_command_may_carry_a_human_label() {
        let command = Rename::new("Doc cut");
        assert_eq!(command.label_erased(), "Rename project");
        assert_eq!(SetName::new("x").label_erased(), "test.set_name");
        let boxed: BoxedCommand = Inverse::new(command).into_command();
        assert_eq!(boxed.kind(), "test.rename");
    }
}
