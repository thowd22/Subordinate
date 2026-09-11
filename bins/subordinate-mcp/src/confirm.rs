//! Asking before something is destroyed.
//!
//! Most of what an agent does through the bridge is undoable: every mutation
//! is a Command, and `edit_undo` puts the project back. A few calls are not
//! that kind of mistake. Deleting a sequence throws away everything on it,
//! removing a media item drops a source the edit may still be using, and an
//! export writes a file — over whatever was already at that path. Those three
//! ask first (docs/PLAN.md §7).
//!
//! Asking is the multi round-trip request pattern of the 2026-07-28 protocol
//! (SEP-2322). The first `tools/call` answers `input_required` rather than a
//! result: it carries an `elicitation/create` request naming exactly what is
//! about to go, and an opaque `requestState`. The client puts the question to
//! the user — in Claude Code, its elicitation dialog — and retries the same
//! call with the answer in `inputResponses` and the state echoed back. An
//! accepted answer runs the call; a declined or cancelled one fails it with
//! `mcp.confirmation_declined`, and nothing was touched in between.
//!
//! The state is a handle, not a message: the pending call — its tool and its
//! arguments — stays here, so a client that edits the string it echoes back
//! cannot use it to run something else.
//!
//! A client with no user to ask passes `confirm: true` and gets the call
//! straight away. Every destructive tool declares that argument in its own
//! input schema ([`augment_schema`]), and it is stripped before the call is
//! forwarded, because the Command API method itself has no such parameter.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicU64, Ordering};

use rmcp::model::{
    CallToolRequestParams, ElicitRequest, ElicitRequestParams, ElicitResult, ElicitationAction,
    ElicitationSchema, InputRequest, InputRequests, InputRequiredResult, JsonObject,
};
use serde_json::Value;
use sub_core::SubError;

use crate::codes;

/// The argument a caller sets to say it has already confirmed.
pub const CONFIRM_ARGUMENT: &str = "confirm";

/// The key the confirmation question travels under, in both the
/// `inputRequests` the bridge sends and the `inputResponses` it reads back.
pub const CONFIRM_KEY: &str = "confirm";

/// How many unanswered questions are remembered at once.
///
/// A confirmation is answered in the next round trip or not at all, so the
/// store is small by nature; the cap is only there so a client that asks and
/// never answers cannot grow it without bound. The oldest is forgotten first,
/// and a client that retries against a forgotten state is simply asked again.
const MAX_PENDING: usize = 16;

/// What a destructive call is about to do, in words the user can act on.
///
/// Built from the arguments of the call itself, so the question names the
/// sequence, the item or the file rather than the tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question(String);

impl Question {
    /// The question, as the elicitation dialog shows it.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.0
    }
}

/// Whether a call needs confirming, and what to ask.
///
/// `export.render` is the interesting one: writing a file is destructive only
/// when a file is already there, so the path decides. The bridge runs beside
/// the editor it drives, which is what makes that check meaningful.
#[must_use]
pub fn question(method: &str, arguments: Option<&JsonObject>) -> Option<Question> {
    let text = |field: &str| -> Option<&str> {
        arguments
            .and_then(|arguments| arguments.get(field))
            .and_then(Value::as_str)
    };
    let flag = |field: &str| -> bool {
        arguments
            .and_then(|arguments| arguments.get(field))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    match method {
        "sequence.delete" => Some(Question(format!(
            "Delete the sequence {} and every track, clip and marker on it? \
             The deletion is undoable with edit_undo.",
            named(text("sequence")),
        ))),
        "media.remove" => Some(Question(if flag("force") {
            format!(
                "Remove the media item {} from the project even though clips still use it? \
                 The removal is undoable with edit_undo.",
                named(text("media")),
            )
        } else {
            format!(
                "Remove the media item {} from the project? \
                 The removal is undoable with edit_undo.",
                named(text("media")),
            )
        })),
        "export.render" => {
            let output = text("output")?;
            Path::new(output).exists().then(|| {
                Question(format!(
                    "Overwrite the existing file {output}? \
                     An export cannot be undone."
                ))
            })
        }
        _ => None,
    }
}

/// How an argument that should name something reads when it does not.
fn named(value: Option<&str>) -> &str {
    value.unwrap_or("the call names")
}

/// Whether a Command API method is one that ever asks.
///
/// [`question`] decides call by call — an export that overwrites nothing does
/// not ask — but the tool's input schema is the same for every call, so this
/// is what says which tools declare [`CONFIRM_ARGUMENT`].
#[must_use]
pub fn is_destructive(method: &str) -> bool {
    matches!(method, "sequence.delete" | "media.remove" | "export.render")
}

/// Adds the `confirm` argument to a destructive tool's input schema.
///
/// The schema is the method's own parameter schema, which does not have the
/// argument and refuses unknown ones, so the property is added here and
/// [`Confirmations::gate`] takes it out again before the call is forwarded.
pub fn augment_schema(method: &str, schema: &mut JsonObject) {
    if !is_destructive(method) {
        return;
    }
    let properties = schema
        .entry("properties")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let Some(properties) = properties.as_object_mut() {
        properties.insert(
            CONFIRM_ARGUMENT.to_owned(),
            serde_json::json!({
                "type": "boolean",
                "default": false,
                "description":
                    "Whether the caller has already confirmed this destructive call. \
                     Left unset, the call answers input_required and the user is asked \
                     first; a client with no user to ask sets it to true.",
            }),
        );
    }
}

/// What the bridge does with a call that reached the gate.
#[derive(Debug)]
pub enum Gate {
    /// Nothing to ask, or the answer was yes: run the call.
    Proceed,
    /// Ask the user, and wait for the retry.
    Ask(Box<InputRequiredResult>),
    /// The user said no, or the client answered something else: fail the call.
    Refused(Box<SubError>),
}

/// One unanswered question.
#[derive(Debug, Clone)]
struct Pending {
    /// The tool the question was asked about.
    tool: String,
    /// The arguments it was asked about, with `confirm` already out.
    arguments: Option<JsonObject>,
}

/// The questions this session has asked and not yet had answered.
#[derive(Debug, Default)]
pub struct Confirmations {
    /// The pending questions, by the state handed to the client.
    pending: Mutex<BTreeMap<u64, Pending>>,
    /// The next handle. Handles are never reused within a session.
    next: AtomicU64,
}

impl Confirmations {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Decides what to do with `request`, stripping `confirm` from its
    /// arguments so the Command API sees only its own parameters.
    ///
    /// `method` is the Command API method the tool calls; a tool that is not
    /// destructive proceeds untouched.
    pub fn gate(&self, method: &str, request: &mut CallToolRequestParams) -> Gate {
        let confirmed = take_confirm(request.arguments.as_mut());
        let Some(question) = question(method, request.arguments.as_ref()) else {
            return Gate::Proceed;
        };
        if confirmed {
            return Gate::Proceed;
        }
        match request.request_state.clone() {
            Some(state) => self.resume(&state, &question, request),
            None => Gate::Ask(Box::new(self.ask(&question, request))),
        }
    }

    /// Forgets a question that will not be asked after all.
    ///
    /// The bridge asks before it knows whether the client can be asked: a peer
    /// on a protocol older than 2026-07-28 has no way to carry an
    /// `input_required` result, and is told to pass `confirm` instead. The
    /// entry that was made for it is dropped here rather than waiting to be
    /// evicted.
    pub fn forget(&self, state: &str) {
        if let Ok(handle) = state.parse::<u64>() {
            self.pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&handle);
        }
    }

    /// How many questions are waiting for an answer.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Records the question and builds the `input_required` result that asks
    /// it.
    fn ask(&self, question: &Question, request: &CallToolRequestParams) -> InputRequiredResult {
        let handle = self.next.fetch_add(1, Ordering::Relaxed);
        {
            let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            while pending.len() >= MAX_PENDING {
                let Some(oldest) = pending.keys().next().copied() else {
                    break;
                };
                pending.remove(&oldest);
            }
            pending.insert(
                handle,
                Pending {
                    tool: request.name.to_string(),
                    arguments: request.arguments.clone(),
                },
            );
        }
        let mut requests = InputRequests::new();
        requests.insert(
            CONFIRM_KEY.to_owned(),
            InputRequest::Elicitation(elicitation(question)),
        );
        InputRequiredResult::new(Some(requests), Some(handle.to_string()))
    }

    /// Reads the answer to a question this store asked.
    ///
    /// A state this session did not hand out, or one handed out for a
    /// different call, is not an answer to the call being made now, so the
    /// question is put again rather than taken as a yes.
    fn resume(&self, state: &str, question: &Question, request: &CallToolRequestParams) -> Gate {
        let asked = state.parse::<u64>().ok().and_then(|handle| {
            self.pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&handle)
        });
        let answers = asked.is_some_and(|pending| {
            pending.tool == request.name.as_ref() && pending.arguments == request.arguments
        });
        if !answers {
            return Gate::Ask(Box::new(self.ask(question, request)));
        }
        match answer(request.input_responses.as_ref()) {
            Answer::Yes => Gate::Proceed,
            Answer::No(reason) => Gate::Refused(Box::new(
                SubError::new(codes::CONFIRMATION_DECLINED, reason)
                    .with_detail("tool", request.name.to_string()),
            )),
        }
    }
}

/// What the user answered.
enum Answer {
    /// Run it.
    Yes,
    /// Do not, and say why.
    No(&'static str),
}

/// Reads the elicitation result the client sent back.
///
/// Anything that is not an accepted answer carrying `confirm: true` — a
/// decline, a cancel, a missing response, a malformed one — is a no. The
/// asymmetry is deliberate: only an explicit yes destroys anything.
fn answer(responses: Option<&BTreeMap<String, Value>>) -> Answer {
    let Some(response) = responses.and_then(|responses| responses.get(CONFIRM_KEY)) else {
        return Answer::No("the confirmation was not answered");
    };
    let Ok(result) = serde_json::from_value::<ElicitResult>(response.clone()) else {
        return Answer::No("the confirmation answer could not be read");
    };
    if result.action != ElicitationAction::Accept {
        return Answer::No("the confirmation was declined");
    }
    let confirmed = result
        .content
        .as_ref()
        .and_then(|content| content.get(CONFIRM_KEY))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if confirmed {
        Answer::Yes
    } else {
        Answer::No("the confirmation was answered with no")
    }
}

/// Takes `confirm` out of a call's arguments, and says what it was.
///
/// The argument never reaches the Command API: the method's own parameter
/// schema does not have it and refuses unknown fields.
fn take_confirm(arguments: Option<&mut JsonObject>) -> bool {
    arguments
        .and_then(|arguments| arguments.remove(CONFIRM_ARGUMENT))
        .as_ref()
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The `elicitation/create` request that puts `question` to the user.
fn elicitation(question: &Question) -> ElicitRequest {
    let schema = ElicitationSchema::builder()
        .required_bool_with(CONFIRM_ARGUMENT, |field| {
            field
                .title("Confirm")
                .description("Yes, go ahead with this.")
        })
        .build_unchecked();
    ElicitRequest::new(ElicitRequestParams::FormElicitationParams {
        meta: None,
        message: question.0.clone(),
        requested_schema: schema,
    })
}

#[cfg(test)]
mod tests {
    use super::{CONFIRM_ARGUMENT, Confirmations, Gate, augment_schema, is_destructive, question};
    use crate::codes;
    use rmcp::model::{CallToolRequestParams, JsonObject};
    use serde_json::{Value, json};

    /// A call to `tool` with `arguments`.
    fn call(tool: &str, arguments: &Value) -> CallToolRequestParams {
        let mut request = CallToolRequestParams::new(tool.to_owned());
        request.arguments = arguments.as_object().cloned();
        request
    }

    /// The arguments of a value, as a tool call carries them.
    fn object(value: &Value) -> JsonObject {
        value.as_object().cloned().expect("an object")
    }

    #[test]
    fn the_destructive_methods_are_the_ones_that_ask() {
        for method in ["sequence.delete", "media.remove", "export.render"] {
            assert!(is_destructive(method), "{method}");
        }
        for method in [
            "clip.remove",
            "track.remove",
            "bin.remove",
            "project.save",
            "export.progress",
        ] {
            assert!(!is_destructive(method), "{method}");
        }
    }

    #[test]
    fn a_question_names_what_is_about_to_go() {
        let sequence = question(
            "sequence.delete",
            Some(&object(&json!({ "sequence": "s1" }))),
        )
        .expect("a question");
        assert!(sequence.message().contains("s1"), "{sequence:?}");
        let media = question(
            "media.remove",
            Some(&object(&json!({ "media": "m1", "force": true }))),
        )
        .expect("a question");
        assert!(media.message().contains("m1"), "{media:?}");
        assert!(media.message().contains("still use it"), "{media:?}");
        assert!(question("clip.remove", None).is_none());
    }

    #[test]
    fn an_export_asks_only_when_it_would_overwrite() {
        let directory = std::env::temp_dir().join(format!("sub-confirm-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("a test directory");
        let fresh = directory.join("fresh.mp4");
        let _ = std::fs::remove_file(&fresh);
        let arguments = json!({ "preset": "h264-mp4", "output": fresh });
        assert!(question("export.render", Some(&object(&arguments))).is_none());

        std::fs::write(&fresh, b"an earlier render").expect("a file to overwrite");
        let asked = question("export.render", Some(&object(&arguments))).expect("a question");
        assert!(asked.message().contains("Overwrite"), "{asked:?}");
        let _ = std::fs::remove_file(&fresh);
    }

    #[test]
    fn the_confirm_argument_is_declared_and_never_forwarded() {
        let mut schema = object(&json!({
            "type": "object",
            "additionalProperties": false,
            "properties": { "sequence": { "type": "string" } },
        }));
        augment_schema("sequence.delete", &mut schema);
        assert_eq!(schema["properties"][CONFIRM_ARGUMENT]["type"], "boolean");
        assert!(schema["properties"]["sequence"].is_object());
        assert!(
            !schema
                .get("required")
                .and_then(Value::as_array)
                .is_some_and(|required| required.iter().any(|name| name == CONFIRM_ARGUMENT)),
            "confirming must stay optional",
        );

        let mut untouched = object(&json!({ "type": "object", "properties": {} }));
        augment_schema("clip.remove", &mut untouched);
        assert!(
            untouched["properties"]
                .as_object()
                .expect("an object")
                .is_empty()
        );
    }

    #[test]
    fn a_confirmed_call_runs_without_a_round_trip() {
        let confirmations = Confirmations::new();
        let mut request = call(
            "sequence_delete",
            &json!({ "sequence": "s1", CONFIRM_ARGUMENT: true }),
        );
        assert!(matches!(
            confirmations.gate("sequence.delete", &mut request),
            Gate::Proceed
        ));
        assert_eq!(confirmations.pending(), 0);
        assert!(
            !request
                .arguments
                .expect("arguments")
                .contains_key(CONFIRM_ARGUMENT),
            "confirm is not a Command API parameter",
        );
    }

    #[test]
    fn an_unconfirmed_call_asks_and_the_answer_decides() {
        let confirmations = Confirmations::new();
        let mut request = call("sequence_delete", &json!({ "sequence": "s1" }));
        let Gate::Ask(asked) = confirmations.gate("sequence.delete", &mut request) else {
            panic!("an unconfirmed deletion must ask");
        };
        assert_eq!(confirmations.pending(), 1);
        let state = asked.request_state.clone().expect("a request state");
        let requests = asked.input_requests.as_ref().expect("an elicitation");
        assert_eq!(requests.len(), 1);

        let mut retry = call("sequence_delete", &json!({ "sequence": "s1" }));
        retry.request_state = Some(state.clone());
        retry.input_responses = Some(
            [(
                super::CONFIRM_KEY.to_owned(),
                json!({ "action": "accept", "content": { "confirm": true } }),
            )]
            .into_iter()
            .collect(),
        );
        assert!(matches!(
            confirmations.gate("sequence.delete", &mut retry),
            Gate::Proceed
        ));
        assert_eq!(confirmations.pending(), 0, "an answered question is spent");
    }

    #[test]
    fn a_declined_confirmation_fails_the_call() {
        for response in [
            json!({ "action": "decline" }),
            json!({ "action": "cancel" }),
            json!({ "action": "accept", "content": { "confirm": false } }),
            json!({ "action": "accept" }),
        ] {
            let confirmations = Confirmations::new();
            let mut request = call("media_remove", &json!({ "media": "m1" }));
            let Gate::Ask(asked) = confirmations.gate("media.remove", &mut request) else {
                panic!("an unconfirmed removal must ask");
            };
            let mut retry = call("media_remove", &json!({ "media": "m1" }));
            retry.request_state = asked.request_state.clone();
            retry.input_responses = Some(
                [(super::CONFIRM_KEY.to_owned(), response.clone())]
                    .into_iter()
                    .collect(),
            );
            let Gate::Refused(error) = confirmations.gate("media.remove", &mut retry) else {
                panic!("{response} must refuse the call");
            };
            assert_eq!(error.code, codes::CONFIRMATION_DECLINED);
        }
    }

    #[test]
    fn a_state_from_another_call_is_not_an_answer() {
        let confirmations = Confirmations::new();
        let mut asked_about = call("sequence_delete", &json!({ "sequence": "s1" }));
        let Gate::Ask(asked) = confirmations.gate("sequence.delete", &mut asked_about) else {
            panic!("an unconfirmed deletion must ask");
        };

        // The same state, echoed back against a different sequence.
        let mut elsewhere = call("sequence_delete", &json!({ "sequence": "s2" }));
        elsewhere.request_state = asked.request_state.clone();
        elsewhere.input_responses = Some(
            [(
                super::CONFIRM_KEY.to_owned(),
                json!({ "action": "accept", "content": { "confirm": true } }),
            )]
            .into_iter()
            .collect(),
        );
        let Gate::Ask(again) = confirmations.gate("sequence.delete", &mut elsewhere) else {
            panic!("a confirmation for one call must not carry another");
        };
        assert_ne!(again.request_state, asked.request_state);
    }

    #[test]
    fn a_forgotten_state_is_asked_again() {
        let confirmations = Confirmations::new();
        let mut request = call("sequence_delete", &json!({ "sequence": "s1" }));
        let Gate::Ask(asked) = confirmations.gate("sequence.delete", &mut request) else {
            panic!("an unconfirmed deletion must ask");
        };
        confirmations.forget(&asked.request_state.clone().expect("a state"));
        assert_eq!(confirmations.pending(), 0);

        let mut retry = call("sequence_delete", &json!({ "sequence": "s1" }));
        retry.request_state = asked.request_state.clone();
        retry.input_responses = Some(
            [(
                super::CONFIRM_KEY.to_owned(),
                json!({ "action": "accept", "content": { "confirm": true } }),
            )]
            .into_iter()
            .collect(),
        );
        assert!(matches!(
            confirmations.gate("sequence.delete", &mut retry),
            Gate::Ask(_)
        ));
    }

    #[test]
    fn unanswered_questions_do_not_pile_up() {
        let confirmations = Confirmations::new();
        for index in 0..super::MAX_PENDING * 2 {
            let mut request = call(
                "sequence_delete",
                &json!({ "sequence": format!("s{index}") }),
            );
            assert!(matches!(
                confirmations.gate("sequence.delete", &mut request),
                Gate::Ask(_)
            ));
        }
        assert_eq!(confirmations.pending(), super::MAX_PENDING);
    }
}
