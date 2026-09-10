//! Everything the exporter does that is not a host call.
//!
//! Keeping it here means the whole of the plugin's behaviour — argument
//! handling, sequence selection, the document itself and the report — is
//! tested on the host triple, and the component is left with two host calls
//! and a file write.

use otio_core::error::{Error, Result, codes};
use otio_core::{Arguments, parse_arguments, parse_project_get};
use serde_json::{Value, json};

/// A finished export, before anything has been written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    /// The identifier of the sequence that was exported.
    pub sequence_id: String,
    /// Its display name.
    pub sequence_name: String,
    /// The OTIO document.
    pub document: String,
    /// Where the caller asked for it to be written, if anywhere.
    pub path: Option<String>,
}

/// Exports one sequence from the answer `project.get` gave.
///
/// `args` is the plugin's `args` string: a JSON object with an optional
/// `sequence` (identifier or name, defaulting to the first) and an optional
/// `path` (inside a folder the manifest granted write access to, so
/// `/project/edit.otio` for the open project's own folder).
///
/// # Errors
///
/// Returns `otio.invalid_arguments` when the arguments are not that shape,
/// `otio.invalid_project` when the host's answer is not a project,
/// `otio.unknown_sequence` when nothing matches, and `otio.invalid_time` when
/// a time in the sequence cannot be written exactly.
pub fn export(project_get_answer: &str, args: &str) -> Result<Export> {
    let Arguments { sequence, path } = parse_arguments(args)?;
    let project = parse_project_get(project_get_answer)?;
    let sequence = match sequence.as_deref() {
        Some(wanted) => project.sequence(wanted)?,
        None => project.first_sequence()?,
    };
    let timeline = otio_core::export_sequence(&project, sequence)?;
    Ok(Export {
        sequence_id: sequence.id.clone(),
        sequence_name: sequence.name.clone(),
        document: timeline.to_json()?,
        path,
    })
}

/// The JSON document the plugin returns.
///
/// The document itself is included only when it was not written to a file: a
/// caller that asked for a file wants the path, and an agent that did not is
/// holding the only copy.
#[must_use]
pub fn report(export: &Export) -> String {
    let mut report = json!({
        "sequence": export.sequence_id,
        "name": export.sequence_name,
        "bytes": export.document.len(),
    });
    match &export.path {
        Some(path) => report["path"] = Value::String(path.clone()),
        None => report["otio"] = Value::String(export.document.clone()),
    }
    report.to_string()
}

/// The error a failed file write reports.
#[must_use]
pub fn write_failed(path: &str, message: &str) -> Error {
    Error::new(
        codes::WRITE_FAILED,
        format!("could not write the export: {message}"),
    )
    .with("path", path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT: &str = include_str!("../../otio-core/tests/fixtures/project.json");

    /// `project.get` as the dispatcher really answers it: the project inside
    /// the document the project file holds, inside the result.
    fn answer() -> String {
        json!({
            "revision": 7,
            "project": {
                "project": serde_json::from_str::<Value>(PROJECT).unwrap(),
                "schema_version": 1,
            },
        })
        .to_string()
    }

    #[test]
    fn an_unwrapped_project_is_read_too() {
        let unwrapped =
            json!({ "revision": 7, "project": serde_json::from_str::<Value>(PROJECT).unwrap() })
                .to_string();
        assert_eq!(
            export(&unwrapped, "").unwrap().document,
            export(&answer(), "").unwrap().document
        );
    }

    #[test]
    fn no_arguments_export_the_first_sequence() {
        let export = export(&answer(), "").unwrap();
        assert_eq!(export.sequence_name, "Edit");
        assert!(export.path.is_none());
        assert!(export.document.starts_with('{'));
        assert!(export.document.ends_with('\n'));
    }

    #[test]
    fn a_sequence_can_be_named_by_identifier_or_by_name() {
        let by_name = export(&answer(), r#"{"sequence":"Edit"}"#).unwrap();
        let by_id = export(
            &answer(),
            &json!({ "sequence": by_name.sequence_id }).to_string(),
        )
        .unwrap();
        assert_eq!(by_id.document, by_name.document);
    }

    #[test]
    fn an_unknown_sequence_and_a_bad_argument_are_told_apart_by_code() {
        assert_eq!(
            export(&answer(), r#"{"sequence":"nope"}"#)
                .unwrap_err()
                .code,
            codes::UNKNOWN_SEQUENCE
        );
        assert_eq!(
            export(&answer(), r#"{"seqence":"typo"}"#).unwrap_err().code,
            codes::INVALID_ARGUMENTS
        );
        assert_eq!(
            export("not json", "").unwrap_err().code,
            codes::INVALID_PROJECT
        );
    }

    #[test]
    fn the_report_carries_the_document_only_when_no_file_was_written() {
        let mut export = export(&answer(), "").unwrap();
        let returned: Value = serde_json::from_str(&report(&export)).unwrap();
        assert_eq!(returned["otio"].as_str().unwrap(), export.document);
        assert!(returned.get("path").is_none());

        export.path = Some("/project/edit.otio".to_owned());
        let written: Value = serde_json::from_str(&report(&export)).unwrap();
        assert_eq!(written["path"], "/project/edit.otio");
        assert!(written.get("otio").is_none());
        assert_eq!(written["bytes"], export.document.len());
    }
}
