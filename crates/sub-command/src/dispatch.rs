//! The method dispatcher: JSON-RPC method names to engine work.
//!
//! One [`Dispatcher`] sits in front of one [`EngineHandle`] and answers
//! messages. Two kinds of method live in its table:
//!
//! - **Commands.** Every kind on the engine's [`CommandRegistry`] is a method
//!   of the same name — `clip.add`, `track.rename`, `media.import` — whose
//!   `params` are the command's own serde parameters. The dispatcher wraps
//!   them in a [`CommandEnvelope`] and hands them to
//!   [`EngineHandle::apply_envelope`], so a Command API call and a UI click
//!   take exactly the same undoable path (decision-7).
//! - **Queries and history control.** Typed methods the engine serves but that
//!   are not commands: [`PROJECT_GET`], [`PROJECT_REVISION`], [`HISTORY_GET`],
//!   [`EDIT_UNDO`], [`EDIT_REDO`], the group methods, and
//!   [`SYSTEM_LIST_METHODS`].
//! - **Session methods.** [`EVENTS_SUBSCRIBE`] and [`EVENTS_UNSUBSCRIBE`] act
//!   on the connection that called them rather than on the engine, so they are
//!   served only when a [`Session`] is passed in with the message — the
//!   `*_in` methods below — and answer `command.no_session` otherwise.
//!
//! Anything else — a plugin-contributed method, the transport's own
//! housekeeping — goes on with [`Dispatcher::register`], so later work extends
//! the surface without touching this module.
//!
//! Failures never escape as `Err`: every one becomes a [`Response`] whose
//! JSON-RPC number comes from the [`SubError`]'s stable code and whose `data`
//! carries that `SubError` whole (see [`error_codes::for_sub_error`]).
//!
//! ```
//! use sub_command::Dispatcher;
//! use sub_command::rpc::{Request, RequestId};
//! use sub_edit::Engine;
//! use sub_model::Project;
//!
//! let engine = Engine::spawn(Project::new("Doc cut")).unwrap();
//! let dispatcher = Dispatcher::new(engine.handle().clone());
//!
//! let response = dispatcher.call(&Request::new(
//!     1,
//!     "bin.create",
//!     Some(serde_json::json!({ "name": "Footage" })),
//! ));
//! assert_eq!(response.value().unwrap()["revision"], 1);
//! assert_eq!(engine.handle().snapshot().root_bin.children[0].name, "Footage");
//!
//! let response = dispatcher.call(&Request::new(RequestId::Number(2), "nope.method", None));
//! assert_eq!(response.error_ref().unwrap().code, -32601);
//! engine.shutdown().unwrap();
//! ```

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sub_core::{SubError, SubResult};
use sub_edit::{
    Applied, ChangeEvent, CommandEnvelope, CommandRegistry, EngineHandle, HistorySummary,
    builtin_registry,
};
use sub_model::json;

use crate::codes;
use crate::events::{
    EVENTS_SUBSCRIBE, EVENTS_UNSUBSCRIBE, Session, SubscribeResult, UnsubscribeParams,
    UnsubscribeResult,
};
use crate::rpc::{Call, Incoming, Outgoing, Request, RequestId, Response, RpcError, error_codes};

/// The whole project as JSON, with the revision it was read at.
pub const PROJECT_GET: &str = "project.get";
/// The current revision alone.
pub const PROJECT_REVISION: &str = "project.revision";
/// The state of the undo and redo stacks.
pub const HISTORY_GET: &str = "history.get";
/// Undo the most recent history step.
pub const EDIT_UNDO: &str = "edit.undo";
/// Redo the most recently undone step.
pub const EDIT_REDO: &str = "edit.redo";
/// Open a command group, so what follows undoes in one step.
pub const EDIT_BEGIN_GROUP: &str = "edit.begin_group";
/// Close the open command group.
pub const EDIT_COMMIT_GROUP: &str = "edit.commit_group";
/// Close the open command group and reverse everything in it.
pub const EDIT_ABORT_GROUP: &str = "edit.abort_group";
/// Every method this build serves.
pub const SYSTEM_LIST_METHODS: &str = "system.list_methods";

/// What a method call does, once the name has been resolved.
type Handler = Box<dyn Fn(&EngineHandle, Value) -> SubResult<Value> + Send + Sync>;

/// What a method that acts on the calling connection does.
type SessionHandler = Box<dyn Fn(&EngineHandle, &Session, Value) -> SubResult<Value> + Send + Sync>;

/// How a method reaches the engine.
enum Method {
    /// A registered command kind, applied through the engine's registry.
    Command,
    /// A query or history operation.
    Query(Handler),
    /// A method acting on the connection that called it.
    Session(SessionHandler),
}

impl Method {
    /// The word `system.list_methods` reports.
    fn kind(&self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Query(_) => "query",
            Self::Session(_) => "session",
        }
    }
}

/// What one applied, undone or redone command did.
///
/// The project itself is deliberately not included: a caller that wants it
/// asks for [`PROJECT_GET`], so a chatty edit session does not serialise the
/// whole timeline on every call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedResult {
    /// The revision the project reached.
    pub revision: u64,
    /// The label of the history step, as the undo menu shows it.
    pub label: String,
    /// What changed, in the order it changed.
    pub events: Vec<ChangeEvent>,
}

impl From<&Applied> for AppliedResult {
    fn from(applied: &Applied) -> Self {
        Self {
            revision: applied.revision,
            label: applied.label.clone(),
            events: applied.events.clone(),
        }
    }
}

/// The state of the undo and redo stacks, as [`HISTORY_GET`] returns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryResult {
    /// Whether there is a step to undo.
    pub can_undo: bool,
    /// Whether there is a step to redo.
    pub can_redo: bool,
    /// The label of the step an undo would reverse.
    pub undo_label: Option<String>,
    /// The label of the step a redo would replay.
    pub redo_label: Option<String>,
    /// The number of steps that can be undone.
    pub undo_len: usize,
    /// The number of steps that can be redone.
    pub redo_len: usize,
    /// Whether a command group is open.
    pub in_group: bool,
}

impl From<HistorySummary> for HistoryResult {
    fn from(summary: HistorySummary) -> Self {
        Self {
            can_undo: summary.can_undo,
            can_redo: summary.can_redo,
            undo_label: summary.undo_label,
            redo_label: summary.redo_label,
            undo_len: summary.undo_len,
            redo_len: summary.redo_len,
            in_group: summary.in_group,
        }
    }
}

/// One entry of [`SYSTEM_LIST_METHODS`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MethodInfo {
    /// The method name.
    pub name: String,
    /// `command` for an undoable mutation, `session` for a method acting on
    /// the calling connection, `query` for everything else.
    pub kind: String,
}

/// The parameters of [`EDIT_BEGIN_GROUP`].
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct BeginGroupParams {
    /// The label the grouped step gets in the undo menu.
    label: String,
}

/// The parameters of every method that takes none.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoParams {}

/// Turns JSON-RPC method calls into engine work.
///
/// Cloning the [`EngineHandle`] into one is enough to serve a client; a
/// dispatcher is `Send` and `Sync`, so one instance can serve every connection
/// the transport accepts.
pub struct Dispatcher {
    engine: EngineHandle,
    methods: BTreeMap<String, Method>,
}

impl std::fmt::Debug for Dispatcher {
    /// Handlers are closures with no useful `Debug`, so the table is shown as
    /// its size.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dispatcher")
            .field("engine", &self.engine)
            .field("methods", &self.methods.len())
            .finish()
    }
}

impl Dispatcher {
    /// A dispatcher serving the built-in command set and the queries.
    ///
    /// # Panics
    ///
    /// Never in practice: the built-in registry is the same one the engine
    /// builds, and failing to build it would already have stopped the engine.
    #[must_use]
    pub fn new(engine: EngineHandle) -> Self {
        let registry = builtin_registry().expect("the built-in command registry is well formed");
        Self::with_registry(engine, &registry)
    }

    /// A dispatcher whose command methods are the kinds in `registry`.
    ///
    /// The registry must be the one the engine decodes with — including any
    /// plugin-contributed kinds — or the dispatcher will offer methods the
    /// engine rejects, or hide methods it would accept.
    #[must_use]
    pub fn with_registry(engine: EngineHandle, registry: &CommandRegistry) -> Self {
        let mut methods = BTreeMap::new();
        for kind in registry.kinds() {
            methods.insert(kind.to_owned(), Method::Command);
        }
        let mut dispatcher = Self { engine, methods };
        dispatcher.install_events();
        dispatcher.install_queries();
        dispatcher
    }

    /// Adds a method that is neither a command nor one of the built-in
    /// queries, such as a transport or plugin method.
    ///
    /// # Errors
    ///
    /// Returns `command.duplicate_method` when the name is already served;
    /// silently shadowing a command would make the surface unpredictable.
    pub fn register<F>(&mut self, name: impl Into<String>, handler: F) -> SubResult<()>
    where
        F: Fn(&EngineHandle, Value) -> SubResult<Value> + Send + Sync + 'static,
    {
        let name = name.into();
        if self.methods.contains_key(&name) {
            return Err(
                SubError::new(codes::DUPLICATE_METHOD, "method is already registered")
                    .with_detail("method", name),
            );
        }
        self.methods.insert(name, Method::Query(Box::new(handler)));
        Ok(())
    }

    /// The engine this dispatcher serves.
    #[must_use]
    pub fn engine(&self) -> &EngineHandle {
        &self.engine
    }

    /// Whether `method` is served.
    #[must_use]
    pub fn contains(&self, method: &str) -> bool {
        self.methods.contains_key(method)
    }

    /// Every method served, in lexicographic order.
    pub fn method_names(&self) -> impl Iterator<Item = &str> {
        self.methods.keys().map(String::as_str)
    }

    /// Every method served, with its kind.
    #[must_use]
    pub fn methods(&self) -> Vec<MethodInfo> {
        self.methods
            .iter()
            .map(|(name, method)| MethodInfo {
                name: name.clone(),
                kind: method.kind().to_owned(),
            })
            .collect()
    }

    /// Runs one request and builds its response.
    #[must_use]
    pub fn call(&self, request: &Request) -> Response {
        self.call_in(None, request)
    }

    /// Runs one request on behalf of `session`, which may serve the session
    /// methods.
    #[must_use]
    pub fn call_in(&self, session: Option<&Session>, request: &Request) -> Response {
        match self.invoke_in(session, &request.method, request.params.clone()) {
            Ok(value) => Response::result(request.id.clone(), value),
            Err(error) => {
                Response::error(Some(request.id.clone()), RpcError::from_sub_error(&error))
            }
        }
    }

    /// Runs one call, answering only if it was a request.
    #[must_use]
    pub fn handle_call(&self, call: &Call) -> Option<Response> {
        self.handle_call_in(None, call)
    }

    /// Runs one call on behalf of `session`, answering only if it was a
    /// request.
    #[must_use]
    pub fn handle_call_in(&self, session: Option<&Session>, call: &Call) -> Option<Response> {
        match call {
            Call::Request(request) => Some(self.call_in(session, request)),
            Call::Notification(notification) => {
                let _ = self.invoke_in(session, &notification.method, notification.params.clone());
                None
            }
        }
    }

    /// Runs a whole message, single or batch, and returns what to send back.
    ///
    /// Returns `None` when nothing is to be sent: a batch of notifications, or
    /// a single notification.
    #[must_use]
    pub fn handle_incoming(&self, incoming: &Incoming) -> Option<Outgoing> {
        self.handle_incoming_in(None, incoming)
    }

    /// Runs a whole message on behalf of `session`.
    #[must_use]
    pub fn handle_incoming_in(
        &self,
        session: Option<&Session>,
        incoming: &Incoming,
    ) -> Option<Outgoing> {
        match incoming {
            Incoming::Single(call) => self.handle_call_in(session, call).map(Outgoing::Single),
            Incoming::Batch(calls) => {
                let responses: Vec<Response> = calls
                    .iter()
                    .filter_map(|call| self.handle_call_in(session, call))
                    .collect();
                (!responses.is_empty()).then_some(Outgoing::Batch(responses))
            }
        }
    }

    /// Runs one already-parsed JSON message, handling malformed messages the
    /// way the specification requires.
    ///
    /// An element of a batch that is not a valid call becomes its own
    /// invalid-request response with a `null` id, so one bad member does not
    /// discard the rest.
    #[must_use]
    pub fn handle_value(&self, message: &Value) -> Option<Outgoing> {
        self.handle_value_in(None, message)
    }

    /// Runs one already-parsed JSON message on behalf of `session`.
    #[must_use]
    pub fn handle_value_in(&self, session: Option<&Session>, message: &Value) -> Option<Outgoing> {
        match message {
            Value::Array(elements) if elements.is_empty() => Some(Outgoing::Single(
                Response::error(None, error_codes::invalid_request("the batch is empty")),
            )),
            Value::Array(elements) => {
                let responses: Vec<Response> = elements
                    .iter()
                    .filter_map(|element| match parse_call(element) {
                        Ok(call) => self.handle_call_in(session, &call),
                        Err(error) => Some(Response::error(recover_id(element), error)),
                    })
                    .collect();
                (!responses.is_empty()).then_some(Outgoing::Batch(responses))
            }
            other => match parse_call(other) {
                Ok(call) => self.handle_call_in(session, &call).map(Outgoing::Single),
                Err(error) => Some(Outgoing::Single(Response::error(recover_id(other), error))),
            },
        }
    }

    /// Runs one message as it arrived on the wire, from bytes to bytes.
    ///
    /// This is the whole pipeline a transport needs: invalid JSON becomes a
    /// parse error, everything else is dispatched, and `None` means send
    /// nothing.
    ///
    /// # Panics
    ///
    /// Never in practice: a [`Response`] is plain JSON-compatible data.
    #[must_use]
    pub fn handle_text(&self, message: &str) -> Option<String> {
        self.handle_text_in(None, message)
    }

    /// Runs one message as it arrived on the wire on behalf of `session`.
    ///
    /// This is what the transport calls: the connection's session is what
    /// makes [`EVENTS_SUBSCRIBE`] and [`EVENTS_UNSUBSCRIBE`] serviceable.
    ///
    /// # Panics
    ///
    /// Never in practice: a [`Response`] is plain JSON-compatible data.
    #[must_use]
    pub fn handle_text_in(&self, session: Option<&Session>, message: &str) -> Option<String> {
        let outgoing = match serde_json::from_str::<Value>(message) {
            Ok(value) => self.handle_value_in(session, &value),
            Err(error) => Some(Outgoing::Single(Response::error(
                None,
                error_codes::parse_error(error.to_string()),
            ))),
        }?;
        Some(serde_json::to_string(&outgoing).expect("responses are always serialisable"))
    }

    /// Resolves a method name and runs it.
    ///
    /// # Errors
    ///
    /// `command.unknown_method`, `command.invalid_params`, or whatever the
    /// engine returns.
    pub fn invoke(&self, method: &str, params: Option<Value>) -> SubResult<Value> {
        self.invoke_in(None, method, params)
    }

    /// Resolves a method name and runs it on behalf of `session`.
    ///
    /// # Errors
    ///
    /// `command.unknown_method`, `command.invalid_params`,
    /// `command.no_session` when a session method is called without a
    /// connection, or whatever the engine returns.
    pub fn invoke_in(
        &self,
        session: Option<&Session>,
        method: &str,
        params: Option<Value>,
    ) -> SubResult<Value> {
        let Some(entry) = self.methods.get(method) else {
            return Err(SubError::new(codes::UNKNOWN_METHOD, "no such method")
                .with_detail("method", method));
        };
        let params = params.unwrap_or_else(|| Value::Object(Map::new()));
        match entry {
            Method::Command => {
                let params = command_params(method, params)?;
                let applied = self
                    .engine
                    .apply_envelope(CommandEnvelope::new(method, params))?;
                to_value(&AppliedResult::from(&applied))
            }
            Method::Query(handler) => handler(&self.engine, params),
            Method::Session(handler) => {
                let session = session.ok_or_else(|| {
                    SubError::new(
                        codes::NO_SESSION,
                        "this method is only served on a client connection",
                    )
                    .with_detail("method", method)
                })?;
                handler(&self.engine, session, params)
            }
        }
    }

    /// Puts the per-connection event methods on the table.
    fn install_events(&mut self) {
        self.add_session(EVENTS_SUBSCRIBE, |engine, session, params| {
            typed::<NoParams>(params)?;
            let subscription = session.subscribe(engine)?;
            to_value(&SubscribeResult { subscription })
        });
        self.add_session(EVENTS_UNSUBSCRIBE, |_, session, params| {
            let params: UnsubscribeParams = typed(params)?;
            session.unsubscribe(&params.subscription)?;
            to_value(&UnsubscribeResult {
                subscription: params.subscription,
            })
        });
    }

    /// Puts the query and history methods on the table.
    fn install_queries(&mut self) {
        self.add(PROJECT_GET, |engine, params| {
            typed::<NoParams>(params)?;
            let project = engine.snapshot();
            let text = json::to_json(&project)?;
            let project = serde_json::from_str::<Value>(&text).map_err(|err| {
                SubError::wrap(
                    sub_core::codes::INTERNAL,
                    "the project did not round-trip to JSON",
                    &err,
                )
            })?;
            Ok(serde_json::json!({
                "revision": engine.revision(),
                "project": project,
            }))
        });
        self.add(PROJECT_REVISION, |engine, params| {
            typed::<NoParams>(params)?;
            Ok(serde_json::json!({ "revision": engine.revision() }))
        });
        self.add(HISTORY_GET, |engine, params| {
            typed::<NoParams>(params)?;
            to_value(&HistoryResult::from(engine.history()?))
        });
        self.add(EDIT_UNDO, |engine, params| {
            typed::<NoParams>(params)?;
            Ok(applied_option(engine.undo()?.as_ref()))
        });
        self.add(EDIT_REDO, |engine, params| {
            typed::<NoParams>(params)?;
            Ok(applied_option(engine.redo()?.as_ref()))
        });
        self.add(EDIT_BEGIN_GROUP, |engine, params| {
            let params: BeginGroupParams = typed(params)?;
            engine.begin_group(params.label)?;
            Ok(serde_json::json!({ "in_group": true }))
        });
        self.add(EDIT_COMMIT_GROUP, |engine, params| {
            typed::<NoParams>(params)?;
            Ok(serde_json::json!({ "committed": engine.commit_group()? }))
        });
        self.add(EDIT_ABORT_GROUP, |engine, params| {
            typed::<NoParams>(params)?;
            engine.abort_group()?;
            Ok(serde_json::json!({ "aborted": true }))
        });

        let methods = self.methods();
        self.add(SYSTEM_LIST_METHODS, move |_, params| {
            typed::<NoParams>(params)?;
            let mut methods = methods.clone();
            methods.push(MethodInfo {
                name: SYSTEM_LIST_METHODS.to_owned(),
                kind: "query".to_owned(),
            });
            methods.sort_by(|left, right| left.name.cmp(&right.name));
            to_value(&serde_json::json!({ "methods": methods }))
        });
    }

    /// Adds a built-in query, which cannot collide because the names are
    /// distinct literals and command kinds are namespaced by their own crate.
    fn add<F>(&mut self, name: &str, handler: F)
    where
        F: Fn(&EngineHandle, Value) -> SubResult<Value> + Send + Sync + 'static,
    {
        let previous = self
            .methods
            .insert(name.to_owned(), Method::Query(Box::new(handler)));
        debug_assert!(previous.is_none(), "built-in method {name} collides");
    }

    /// Adds a built-in method that acts on the calling connection.
    fn add_session<F>(&mut self, name: &str, handler: F)
    where
        F: Fn(&EngineHandle, &Session, Value) -> SubResult<Value> + Send + Sync + 'static,
    {
        let previous = self
            .methods
            .insert(name.to_owned(), Method::Session(Box::new(handler)));
        debug_assert!(previous.is_none(), "built-in method {name} collides");
    }
}

/// Serialises a result value.
fn to_value<T: Serialize>(value: &T) -> SubResult<Value> {
    serde_json::to_value(value).map_err(|err| {
        SubError::wrap(
            sub_core::codes::INTERNAL,
            "the result could not be serialised",
            &err,
        )
    })
}

/// Wraps the optional result of an undo or a redo.
fn applied_option(applied: Option<&Applied>) -> Value {
    let applied = applied.map(AppliedResult::from);
    serde_json::json!({ "applied": applied })
}

/// Decodes typed parameters, reporting a mismatch as `command.invalid_params`.
fn typed<T: DeserializeOwned>(params: Value) -> SubResult<T> {
    serde_json::from_value(params).map_err(|err| {
        SubError::wrap(
            codes::INVALID_PARAMS,
            "parameters do not match the method",
            &err,
        )
    })
}

/// Checks that a command's parameters are an object before they become a
/// [`CommandEnvelope`], so a bare array or string is an invalid-params error
/// rather than a decode failure deeper down.
fn command_params(method: &str, params: Value) -> SubResult<Value> {
    if params.is_object() {
        Ok(params)
    } else {
        Err(SubError::new(
            codes::INVALID_PARAMS,
            "command parameters must be an object",
        )
        .with_detail("method", method))
    }
}

/// Parses one element of a message into a call.
fn parse_call(element: &Value) -> Result<Call, RpcError> {
    serde_json::from_value(element.clone())
        .map_err(|err| error_codes::invalid_request(err.to_string()))
}

/// Recovers the id of a malformed request, so the error can be attributed.
fn recover_id(element: &Value) -> Option<RequestId> {
    serde_json::from_value(element.get("id")?.clone()).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use sub_edit::Engine;
    use sub_model::{Project, Sequence, SequenceId, SequenceSettings};

    use super::{Dispatcher, EDIT_UNDO, PROJECT_GET, SYSTEM_LIST_METHODS};
    use crate::events::{EVENTS_SUBSCRIBE, EVENTS_UNSUBSCRIBE, Session};
    use crate::rpc::{Notification, Outgoing, Request, RequestId, error_codes};

    /// A dispatcher over an engine holding a project with one sequence.
    fn fixture() -> (Engine, Dispatcher) {
        let mut project = Project::new("Doc cut");
        project
            .sequences
            .push(Sequence::new("Main", SequenceSettings::default()));
        let engine = Engine::spawn(project).unwrap();
        let dispatcher = Dispatcher::new(engine.handle().clone());
        (engine, dispatcher)
    }

    #[test]
    fn command_methods_reach_the_engine() {
        let (engine, dispatcher) = fixture();
        let response = dispatcher.call(&Request::new(
            1,
            "bin.create",
            Some(json!({ "name": "Footage" })),
        ));
        let value = response.value().unwrap();
        assert_eq!(value["revision"], 1);
        assert_eq!(value["events"].as_array().unwrap().len(), 1);
        assert_eq!(engine.handle().snapshot().root_bin.children.len(), 1);
    }

    #[test]
    fn every_registered_command_kind_is_a_method() {
        let (_engine, dispatcher) = fixture();
        for kind in [
            "track.add",
            "sequence.rename",
            "media.import",
            "marker.add",
            "bin.create",
            "clip.set_params",
        ] {
            assert!(dispatcher.contains(kind), "{kind}");
        }
    }

    #[test]
    fn unknown_method_is_method_not_found() {
        let (_engine, dispatcher) = fixture();
        let response = dispatcher.call(&Request::new(1, "nope.method", None));
        let error = response.error_ref().unwrap();
        assert_eq!(error.code, error_codes::METHOD_NOT_FOUND);
        assert_eq!(
            error.sub_error().unwrap().code.as_str(),
            "command.unknown_method",
        );
    }

    #[test]
    fn invalid_command_params_are_invalid_params() {
        let (_engine, dispatcher) = fixture();
        let response = dispatcher.call(&Request::new(1, "bin.create", Some(json!({}))));
        let error = response.error_ref().unwrap();
        assert_eq!(error.code, error_codes::INVALID_PARAMS);
        assert_eq!(
            error.sub_error().unwrap().code.as_str(),
            "edit.invalid_command",
        );
    }

    #[test]
    fn non_object_command_params_are_invalid_params() {
        let (_engine, dispatcher) = fixture();
        let response = dispatcher.call(&Request::new(1, "bin.create", Some(json!([1, 2]))));
        let error = response.error_ref().unwrap();
        assert_eq!(error.code, error_codes::INVALID_PARAMS);
        assert_eq!(
            error.sub_error().unwrap().code.as_str(),
            "command.invalid_params",
        );
    }

    #[test]
    fn invalid_query_params_are_invalid_params() {
        let (_engine, dispatcher) = fixture();
        let response = dispatcher.call(&Request::new(1, EDIT_UNDO, Some(json!({ "steps": 2 }))));
        let error = response.error_ref().unwrap();
        assert_eq!(error.code, error_codes::INVALID_PARAMS);
        assert_eq!(
            error.sub_error().unwrap().code.as_str(),
            "command.invalid_params",
        );
    }

    #[test]
    fn engine_failures_are_server_errors_wrapping_the_sub_error() {
        let (_engine, dispatcher) = fixture();
        let response = dispatcher.call(&Request::new(
            1,
            "sequence.rename",
            Some(json!({ "sequence": SequenceId::new(), "name": "Gone" })),
        ));
        let error = response.error_ref().unwrap();
        assert_eq!(error.code, error_codes::SERVER_ERROR);
        assert_eq!(
            error.sub_error().unwrap().code.as_str(),
            "edit.sequence_not_found",
        );
    }

    #[test]
    fn project_get_returns_the_project_and_revision() {
        let (_engine, dispatcher) = fixture();
        let value = dispatcher.invoke(PROJECT_GET, None).unwrap();
        assert_eq!(value["revision"], 0);
        assert_eq!(value["project"]["project"]["name"], "Doc cut");
    }

    #[test]
    fn undo_and_redo_report_what_they_did() {
        let (engine, dispatcher) = fixture();
        dispatcher
            .invoke("bin.create", Some(json!({ "name": "Footage" })))
            .unwrap();

        let history = dispatcher.invoke("history.get", None).unwrap();
        assert_eq!(history["can_undo"], true);
        assert_eq!(history["undo_len"], 1);

        let undone = dispatcher.invoke(EDIT_UNDO, None).unwrap();
        assert_eq!(undone["applied"]["revision"], 2);
        assert!(engine.handle().snapshot().root_bin.children.is_empty());

        let redone = dispatcher.invoke("edit.redo", None).unwrap();
        assert_eq!(redone["applied"]["revision"], 3);
        assert_eq!(engine.handle().snapshot().root_bin.children.len(), 1);

        let nothing = dispatcher.invoke("edit.redo", None).unwrap();
        assert_eq!(nothing["applied"], Value::Null);
    }

    #[test]
    fn groups_make_one_history_step() {
        let (_engine, dispatcher) = fixture();
        dispatcher
            .invoke("edit.begin_group", Some(json!({ "label": "Two bins" })))
            .unwrap();
        dispatcher
            .invoke("bin.create", Some(json!({ "name": "One" })))
            .unwrap();
        dispatcher
            .invoke("bin.create", Some(json!({ "name": "Two" })))
            .unwrap();
        let committed = dispatcher.invoke("edit.commit_group", None).unwrap();
        assert_eq!(committed["committed"], true);

        let history = dispatcher.invoke("history.get", None).unwrap();
        assert_eq!(history["undo_len"], 1);
        assert_eq!(history["undo_label"], "Two bins");
        assert_eq!(history["in_group"], false);
    }

    #[test]
    fn aborting_a_group_reverses_it() {
        let (engine, dispatcher) = fixture();
        dispatcher
            .invoke("edit.begin_group", Some(json!({ "label": "One bin" })))
            .unwrap();
        dispatcher
            .invoke("bin.create", Some(json!({ "name": "One" })))
            .unwrap();
        assert_eq!(
            dispatcher.invoke("edit.abort_group", None).unwrap()["aborted"],
            true,
        );
        assert!(engine.handle().snapshot().root_bin.children.is_empty());
        assert_eq!(
            dispatcher.invoke("history.get", None).unwrap()["undo_len"],
            0,
        );
    }

    #[test]
    fn list_methods_covers_commands_and_queries() {
        let (_engine, dispatcher) = fixture();
        let value = dispatcher.invoke(SYSTEM_LIST_METHODS, None).unwrap();
        let methods = value["methods"].as_array().unwrap();
        assert!(
            methods
                .iter()
                .any(|method| method["name"] == "bin.create" && method["kind"] == "command")
        );
        assert!(
            methods
                .iter()
                .any(|method| method["name"] == PROJECT_GET && method["kind"] == "query")
        );
        assert!(
            methods
                .iter()
                .any(|method| method["name"] == SYSTEM_LIST_METHODS)
        );
    }

    #[test]
    fn registering_a_duplicate_method_fails() {
        let (_engine, mut dispatcher) = fixture();
        dispatcher
            .register("plugin.ping", |_, _| Ok(json!("pong")))
            .unwrap();
        assert_eq!(
            dispatcher.invoke("plugin.ping", None).unwrap(),
            json!("pong"),
        );

        let err = dispatcher
            .register("plugin.ping", |_, _| Ok(Value::Null))
            .unwrap_err();
        assert_eq!(err.code.as_str(), "command.duplicate_method");
        let err = dispatcher
            .register("bin.create", |_, _| Ok(Value::Null))
            .unwrap_err();
        assert_eq!(err.code.as_str(), "command.duplicate_method");
    }

    #[test]
    fn notifications_run_but_answer_nothing() {
        let (engine, dispatcher) = fixture();
        let notification = Notification::new("bin.create", Some(json!({ "name": "Quiet" })));
        assert!(dispatcher.handle_call(&notification.into()).is_none());
        assert_eq!(
            engine.handle().snapshot().root_bin.children[0].name,
            "Quiet"
        );
    }

    #[test]
    fn a_failing_notification_still_answers_nothing() {
        let (_engine, dispatcher) = fixture();
        let notification = Notification::new("nope.method", None);
        assert!(dispatcher.handle_call(&notification.into()).is_none());
    }

    #[test]
    fn a_batch_answers_only_its_requests() {
        let (_engine, dispatcher) = fixture();
        let message = json!([
            { "jsonrpc": "2.0", "method": "bin.create", "params": { "name": "A" }, "id": 1 },
            { "jsonrpc": "2.0", "method": "project.revision" },
            { "jsonrpc": "2.0", "method": "project.revision", "id": "two" },
        ]);
        let Some(Outgoing::Batch(responses)) = dispatcher.handle_value(&message) else {
            panic!("expected a batch of responses");
        };
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0].id, Some(RequestId::Number(1)));
        assert_eq!(responses[1].id, Some(RequestId::Text("two".to_owned())));
        assert_eq!(responses[1].value().unwrap()["revision"], 1);
    }

    #[test]
    fn a_batch_of_notifications_answers_nothing() {
        let (_engine, dispatcher) = fixture();
        let message = json!([{ "jsonrpc": "2.0", "method": "project.revision" }]);
        assert!(dispatcher.handle_value(&message).is_none());
    }

    #[test]
    fn an_empty_batch_is_an_invalid_request() {
        let (_engine, dispatcher) = fixture();
        let Some(Outgoing::Single(response)) = dispatcher.handle_value(&json!([])) else {
            panic!("expected one response");
        };
        assert_eq!(
            response.error_ref().unwrap().code,
            error_codes::INVALID_REQUEST,
        );
        assert_eq!(response.id, None);
    }

    #[test]
    fn a_malformed_batch_member_fails_alone() {
        let (_engine, dispatcher) = fixture();
        let message = json!([
            { "jsonrpc": "1.0", "method": "project.revision", "id": 1 },
            { "jsonrpc": "2.0", "method": "project.revision", "id": 2 },
        ]);
        let Some(Outgoing::Batch(responses)) = dispatcher.handle_value(&message) else {
            panic!("expected a batch of responses");
        };
        assert_eq!(responses.len(), 2);
        assert_eq!(
            responses[0].error_ref().unwrap().code,
            error_codes::INVALID_REQUEST,
        );
        assert_eq!(responses[0].id, Some(RequestId::Number(1)));
        assert!(responses[1].value().is_some());
    }

    #[test]
    fn invalid_json_is_a_parse_error() {
        let (_engine, dispatcher) = fixture();
        let text = dispatcher.handle_text("{ not json").unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["error"]["code"], error_codes::PARSE_ERROR);
        assert_eq!(value["id"], Value::Null);
    }

    #[test]
    fn text_pipeline_answers_a_request() {
        let (_engine, dispatcher) = fixture();
        let text = dispatcher
            .handle_text(r#"{"jsonrpc":"2.0","method":"project.revision","id":9}"#)
            .unwrap();
        assert_eq!(text, r#"{"jsonrpc":"2.0","result":{"revision":0},"id":9}"#);
        assert!(
            dispatcher
                .handle_text(r#"{"jsonrpc":"2.0","method":"project.revision"}"#)
                .is_none()
        );
    }

    #[test]
    fn a_non_object_message_is_an_invalid_request() {
        let (_engine, dispatcher) = fixture();
        let Some(Outgoing::Single(response)) = dispatcher.handle_value(&json!(7)) else {
            panic!("expected one response");
        };
        assert_eq!(
            response.error_ref().unwrap().code,
            error_codes::INVALID_REQUEST,
        );
    }

    #[test]
    fn session_methods_need_a_connection() {
        let (_engine, dispatcher) = fixture();
        let response = dispatcher.call(&Request::new(1, EVENTS_SUBSCRIBE, None));
        let error = response.error_ref().unwrap();
        assert_eq!(
            error.sub_error().unwrap().code.as_str(),
            "command.no_session",
        );
    }

    #[test]
    fn a_session_subscribes_and_unsubscribes() {
        let (engine, dispatcher) = fixture();
        let session = Session::new(8);

        let subscribed = dispatcher
            .call_in(Some(&session), &Request::new(1, EVENTS_SUBSCRIBE, None))
            .value()
            .unwrap()
            .clone();
        let id = subscribed["subscription"].clone();
        assert_eq!(engine.handle().subscriber_count(), 1);

        let stopped = dispatcher.call_in(
            Some(&session),
            &Request::new(
                2,
                EVENTS_UNSUBSCRIBE,
                Some(json!({ "subscription": id.clone() })),
            ),
        );
        assert_eq!(stopped.value().unwrap()["subscription"], id);
        assert_eq!(session.subscription_count(), 0);
    }

    #[test]
    fn the_event_methods_are_listed_as_session_methods() {
        let (_engine, dispatcher) = fixture();
        let listed = dispatcher.invoke(SYSTEM_LIST_METHODS, None).unwrap();
        let methods = listed["methods"].as_array().unwrap().clone();
        let subscribe = methods
            .iter()
            .find(|method| method["name"] == EVENTS_SUBSCRIBE)
            .expect("events.subscribe is listed");
        assert_eq!(subscribe["kind"], "session");
        assert!(
            methods
                .iter()
                .any(|method| method["name"] == EVENTS_UNSUBSCRIBE)
        );
    }

    #[test]
    fn the_method_table_is_visible() {
        let (_engine, dispatcher) = fixture();
        assert!(format!("{dispatcher:?}").contains("methods"));
        assert!(dispatcher.method_names().count() > 20);
        assert_eq!(dispatcher.engine().revision(), 0);
    }
}
