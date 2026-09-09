//! The exported JSON Schema of the whole Command API.
//!
//! The MCP bridge turns this document into tools and the plugin SDK generates
//! bindings from it, so it is produced by the code rather than written by hand:
//! every method's `params` schema comes from the same serde type the
//! dispatcher decodes, and every command's description comes from the
//! [`sub_edit::Command`] implementation itself (docs/PLAN.md §7).
//!
//! The document is one JSON object:
//!
//! ```json
//! {
//!   "$schema": "https://json-schema.org/draft/2020-12/schema",
//!   "title": "Subordinate Command API",
//!   "methods": [
//!     {
//!       "name": "bin.rename",
//!       "kind": "command",
//!       "description": "Rename a bin.",
//!       "params": { "$ref": "#/$defs/RenameBin" },
//!       "result": { "$ref": "#/$defs/AppliedResult" }
//!     }
//!   ],
//!   "$defs": { "RenameBin": { "type": "object" } }
//! }
//! ```
//!
//! `methods` is sorted by name and every object's keys are sorted, so the
//! committed copy at `docs/schema/command-api.json` is byte-stable. The
//! `committed_schema_is_up_to_date` test in this module fails when the two
//! drift, which is what makes the CI run a schema check.
//!
//! ```
//! use sub_command::{Dispatcher, schema};
//! use sub_edit::Engine;
//! use sub_model::Project;
//!
//! let engine = Engine::spawn(Project::new("Doc cut")).unwrap();
//! let document = schema::document(&Dispatcher::new(engine.handle().clone()));
//! let methods = document["methods"].as_array().unwrap();
//! let rename = methods
//!     .iter()
//!     .find(|method| method["name"] == "bin.rename")
//!     .unwrap();
//! assert_eq!(rename["kind"], "command");
//! assert_eq!(rename["description"], "Rename a bin.");
//! engine.shutdown().unwrap();
//! ```

use schemars::generate::SchemaSettings;
use serde_json::{Map, Value};

use crate::dispatch::Dispatcher;

/// The meta-schema the exported document conforms to.
pub const META_SCHEMA: &str = "https://json-schema.org/draft/2020-12/schema";

/// The title of the exported document.
pub const TITLE: &str = "Subordinate Command API";

/// The path of the committed copy, relative to the repository root.
pub const COMMITTED_PATH: &str = "docs/schema/command-api.json";

/// The JSON Schema of every method `dispatcher` serves.
///
/// One [`schemars::SchemaGenerator`] serves the whole document, so a type used
/// by several methods — `RationalTime`, `Clip` — is defined once in `$defs`
/// and referred to from each of them.
#[must_use]
pub fn document(dispatcher: &Dispatcher) -> Value {
    let mut generator = SchemaSettings::draft2020_12().into_generator();
    let methods = dispatcher.method_schemas(&mut generator);
    let defs = generator.take_definitions(true);

    let document = serde_json::json!({
        "$schema": META_SCHEMA,
        "title": TITLE,
        "description": "Every method of the Subordinate Command API, with the JSON Schema of \
                        its parameters and of its result. Generated from the Rust command set; \
                        do not edit by hand.",
        "methods": methods,
        "$defs": defs,
    });
    sorted(document)
}

/// The document as the text committed under `docs/schema/`.
#[must_use]
pub fn document_text(dispatcher: &Dispatcher) -> String {
    let mut text = serde_json::to_string_pretty(&document(dispatcher)).unwrap_or_default();
    text.push('\n');
    text
}

/// Rebuilds `value` with every object's keys in sorted order.
///
/// The same guard `sub_model::json` uses: any crate in the build may turn on
/// `serde_json`'s `preserve_order` feature, and the committed file has to stay
/// byte-identical either way. Array order is left alone, so `methods` keeps
/// the dispatcher's lexicographic order.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<(String, Value)> = object.into_iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sorted(value)))
                    .collect::<Map<String, Value>>(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use sub_edit::Engine;
    use sub_model::Project;

    use super::{COMMITTED_PATH, META_SCHEMA, TITLE, document, document_text};
    use crate::Dispatcher;

    /// A dispatcher over an engine holding an empty project.
    fn fixture() -> (Engine, Dispatcher) {
        let engine = Engine::spawn(Project::new("Doc cut")).unwrap();
        let dispatcher = Dispatcher::new(engine.handle().clone());
        (engine, dispatcher)
    }

    #[test]
    fn every_served_method_is_in_the_document() {
        let (engine, dispatcher) = fixture();
        let document = document(&dispatcher);
        assert_eq!(document["$schema"], META_SCHEMA);
        assert_eq!(document["title"], TITLE);

        let names: Vec<&str> = document["methods"]
            .as_array()
            .unwrap()
            .iter()
            .map(|method| method["name"].as_str().unwrap())
            .collect();
        let mut served: Vec<&str> = dispatcher.method_names().collect();
        served.sort_unstable();
        assert_eq!(names, served);
        assert!(names.contains(&"clip.add"));
        assert!(names.contains(&"events.subscribe"));
        assert!(names.contains(&"system.list_methods"));
        engine.shutdown().unwrap();
    }

    #[test]
    fn every_method_carries_a_one_sentence_description() {
        let (engine, dispatcher) = fixture();
        let document = document(&dispatcher);
        for method in document["methods"].as_array().unwrap() {
            let name = method["name"].as_str().unwrap();
            let description = method["description"].as_str().unwrap();
            assert!(!description.is_empty(), "{name} has no description");
            assert!(
                description.ends_with('.'),
                "{name}: a description is one sentence ending in a full stop: {description:?}",
            );
            assert_eq!(
                description.matches(". ").count(),
                0,
                "{name}: a description is one sentence: {description:?}",
            );
            assert!(
                description.chars().next().is_some_and(char::is_uppercase),
                "{name}: a description starts with a capital: {description:?}",
            );
            assert_eq!(
                description,
                dispatcher.description(name).unwrap(),
                "{name}: the document and the dispatcher disagree",
            );
        }
        engine.shutdown().unwrap();
    }

    #[test]
    fn params_and_result_schemas_resolve_against_the_definitions() {
        let (engine, dispatcher) = fixture();
        let document = document(&dispatcher);
        let defs = document["$defs"].as_object().unwrap();
        for method in document["methods"].as_array().unwrap() {
            for slot in ["params", "result"] {
                let schema = &method[slot];
                assert!(schema.is_object(), "{slot} is not a schema: {schema}");
                if let Some(reference) = schema["$ref"].as_str() {
                    let name = reference.strip_prefix("#/$defs/").unwrap_or_else(|| {
                        panic!("{slot} points outside the document: {reference}")
                    });
                    assert!(defs.contains_key(name), "$defs has no {name}");
                }
            }
        }
        // The command result is the one shape every command shares.
        let add = document["methods"]
            .as_array()
            .unwrap()
            .iter()
            .find(|method| method["name"] == "clip.add")
            .unwrap();
        assert_eq!(add["result"]["$ref"], "#/$defs/AppliedResult");
        assert_eq!(add["params"]["$ref"], "#/$defs/AddClip");
        engine.shutdown().unwrap();
    }

    #[test]
    fn the_document_is_key_sorted_and_stable() {
        let (engine, dispatcher) = fixture();
        let text = document_text(&dispatcher);
        assert!(text.ends_with('\n'));
        assert_eq!(text, document_text(&dispatcher));
        engine.shutdown().unwrap();
    }

    /// The committed copy is what the MCP bridge and the plugin SDK read, so
    /// it must match what this build generates. `SUB_UPDATE_SCHEMA=1`
    /// rewrites it.
    #[test]
    fn committed_schema_is_up_to_date() {
        let (engine, dispatcher) = fixture();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(COMMITTED_PATH);
        let generated = document_text(&dispatcher);
        if std::env::var_os("SUB_UPDATE_SCHEMA").is_some() {
            std::fs::write(&path, &generated).unwrap();
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        engine.shutdown().unwrap();
        assert_eq!(
            committed, generated,
            "the committed {COMMITTED_PATH} is stale; regenerate it with \
             SUB_UPDATE_SCHEMA=1 cargo test -p sub-command committed_schema_is_up_to_date"
        );
    }
}
