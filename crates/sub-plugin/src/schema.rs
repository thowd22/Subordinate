//! The exported JSON Schema of `plugin.toml`.
//!
//! A plugin scaffold is written by an agent, so the manifest shape has to be
//! readable without building the host: the schema is generated from the same
//! [`crate::manifest::Manifest`] types the host deserializes, and committed at
//! [`COMMITTED_PATH`]. It describes the TOML file's data model — TOML tables
//! are JSON objects, so a manifest converted to JSON validates against it.
//!
//! Every object's keys are sorted so the committed copy is byte-stable, the
//! same guard `sub_command::schema` uses. The `committed_schema_is_up_to_date`
//! test below fails when the two drift; `SUB_UPDATE_SCHEMA=1` rewrites it.
//!
//! ```
//! let document = sub_plugin::schema::document();
//! assert_eq!(document["title"], "Subordinate plugin manifest");
//! assert_eq!(document["properties"]["plugin"]["$ref"], "#/$defs/PluginSection");
//! ```

use schemars::generate::SchemaSettings;
use serde_json::{Map, Value};

use crate::manifest::Manifest;

/// The meta-schema the exported document conforms to.
pub const META_SCHEMA: &str = "https://json-schema.org/draft/2020-12/schema";

/// The title of the exported document.
pub const TITLE: &str = "Subordinate plugin manifest";

/// The path of the committed copy, relative to the repository root.
pub const COMMITTED_PATH: &str = "docs/schema/plugin-manifest.json";

/// The JSON Schema of a `plugin.toml`.
#[must_use]
pub fn document() -> Value {
    let settings = SchemaSettings::draft2020_12().with(|settings| {
        settings.meta_schema = Some(META_SCHEMA.into());
    });
    let schema = settings.into_generator().into_root_schema_for::<Manifest>();
    let mut document = schema.to_value();
    if let Some(object) = document.as_object_mut() {
        object.insert("title".to_owned(), Value::String(TITLE.to_owned()));
        object.insert(
            "description".to_owned(),
            Value::String(
                "The data model of a Subordinate plugin's plugin.toml (docs/PLAN.md §6.3). \
                 Generated from the Rust manifest types; do not edit by hand."
                    .to_owned(),
            ),
        );
    }
    sorted(document)
}

/// The document as the text committed under `docs/schema/`.
#[must_use]
pub fn document_text() -> String {
    let mut text = serde_json::to_string_pretty(&document()).unwrap_or_default();
    text.push('\n');
    text
}

/// Rebuilds `value` with every object's keys in sorted order, so the committed
/// file is identical whether or not some crate in the build turns on
/// `serde_json`'s `preserve_order` feature.
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
    use super::{COMMITTED_PATH, META_SCHEMA, TITLE, document, document_text};

    #[test]
    fn the_document_describes_every_manifest_section() {
        let document = document();
        assert_eq!(document["$schema"], META_SCHEMA);
        assert_eq!(document["title"], TITLE);
        let properties = document["properties"].as_object().unwrap();
        for section in ["plugin", "capabilities", "mcp"] {
            assert!(properties.contains_key(section), "no {section} section");
        }
        assert_eq!(document["required"].as_array().unwrap(), &["plugin"]);
    }

    #[test]
    fn the_identity_fields_carry_their_own_constraints() {
        let document = document();
        let defs = document["$defs"].as_object().unwrap();
        let plugin = &defs["PluginSection"];
        let required: Vec<&str> = plugin["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        for field in ["id", "name", "version", "api", "worlds"] {
            assert!(required.contains(&field), "{field} is not required");
        }
        assert_eq!(defs["PluginId"]["type"], "string");
        assert!(defs["PluginId"]["pattern"].is_string());
        assert_eq!(defs["ApiVersion"]["pattern"], r"^\d+\.\d+$");
        let worlds: Vec<&str> = defs["World"]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|variant| variant["const"].as_str().unwrap())
            .collect();
        assert!(worlds.contains(&"mcp-tools"));
        assert_eq!(worlds.len(), crate::manifest::World::ALL.len());
    }

    #[test]
    fn the_document_is_key_sorted_and_stable() {
        let text = document_text();
        assert!(text.ends_with('\n'));
        assert_eq!(text, document_text());
    }

    /// The committed copy is what a scaffolding agent and external tooling
    /// read, so it must match what this build generates. `SUB_UPDATE_SCHEMA=1`
    /// rewrites it.
    #[test]
    fn committed_schema_is_up_to_date() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(COMMITTED_PATH);
        let generated = document_text();
        if std::env::var_os("SUB_UPDATE_SCHEMA").is_some() {
            std::fs::write(&path, &generated).unwrap();
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            committed, generated,
            "the committed {COMMITTED_PATH} is stale; regenerate it with \
             SUB_UPDATE_SCHEMA=1 cargo test -p sub-plugin committed_schema_is_up_to_date"
        );
    }
}
