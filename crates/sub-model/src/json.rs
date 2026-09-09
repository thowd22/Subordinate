//! Reading and writing the project file: deterministic JSON with a
//! `schema_version`.
//!
//! A `.sub` file must be diffable in git (docs/PLAN.md §3), so the bytes are a
//! pure function of the model: keys are sorted at every level, indentation is
//! two spaces, and the file ends in a newline. Saving the same project twice
//! produces byte-identical files, and an edit shows up in a diff as the lines
//! it actually changed.
//!
//! The file is an object with exactly two members:
//!
//! ```json
//! {
//!   "project": { "id": "…", "media": [], "name": "Doc cut", … },
//!   "schema_version": 1
//! }
//! ```
//!
//! [`SCHEMA_VERSION`] is the version this build writes. A file from a newer
//! build is refused with `model.unsupported_schema_version` rather than being
//! read as if its fields still meant the same thing. An older file is brought
//! up to date first by the [`crate::migrate`] registry, so loading always goes
//! JSON text -> `serde_json::Value` -> migrations -> model, never text ->
//! model directly.
//!
//! ```
//! use sub_model::{Project, json};
//!
//! let project = Project::new("Doc cut");
//! let text = json::to_json(&project).unwrap();
//! assert!(text.starts_with("{\n  \"project\": {\n"));
//! assert!(text.ends_with("\"schema_version\": 1\n}\n"));
//! assert_eq!(json::from_json(&text).unwrap(), project);
//! ```

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sub_core::{SubError, SubResult};

use crate::codes;
use crate::migrate::{LoadReport, MigrationRegistry};
use crate::project::Project;

/// The project schema version this build writes.
///
/// Bump it whenever the on-disk shape changes in a way an older build would
/// misread, and register the matching migration in
/// [`MigrationRegistry::current`].
pub const SCHEMA_VERSION: u32 = 1;

/// A project file: the model plus the schema version it was written at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectFile {
    /// The version of the on-disk schema these fields follow.
    pub schema_version: u32,
    /// The project itself.
    pub project: Project,
}

impl ProjectFile {
    /// Wraps `project` at the current [`SCHEMA_VERSION`].
    #[must_use]
    pub fn new(project: Project) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            project,
        }
    }
}

/// Serialises `project` as the deterministic text of a `.sub` file.
///
/// Keys are sorted at every level, values are indented by two spaces and the
/// text ends in a newline.
///
/// # Errors
///
/// Returns `core.internal` if the model cannot be serialised, which would be a
/// bug in this crate rather than anything the caller did.
pub fn to_json(project: &Project) -> SubResult<String> {
    let value = serde_json::to_value(ProjectFile::new(project.clone())).map_err(|err| {
        SubError::wrap(
            sub_core::codes::INTERNAL,
            "could not serialise the project",
            &err,
        )
    })?;
    let mut text = serde_json::to_string_pretty(&sorted(value)).map_err(|err| {
        SubError::wrap(
            sub_core::codes::INTERNAL,
            "could not render the project as JSON",
            &err,
        )
    })?;
    text.push('\n');
    Ok(text)
}

/// Parses the text of a `.sub` file, migrating an older schema version on the
/// way in.
///
/// # Errors
///
/// - `model.unsupported_schema_version` when the file was written by a newer
///   build, or by an older one this build has no migration for.
/// - `model.migration_failed` when a migration could not be applied.
/// - `model.invalid_project_file` when the text is not JSON, is not a project
///   file, or carries a value the model rejects (a zero frame rate, a negative
///   duration, an absolute media path).
pub fn from_json(text: &str) -> SubResult<Project> {
    from_json_with_report(text).map(|(project, _)| project)
}

/// Parses the text of a `.sub` file and reports what the load did: the version
/// the file declared and the migrations that ran.
///
/// # Errors
///
/// The same as [`from_json`].
pub fn from_json_with_report(text: &str) -> SubResult<(Project, LoadReport)> {
    from_json_with_registry(text, &MigrationRegistry::current())
}

/// Parses the text of a `.sub` file against a specific migration chain.
///
/// Production loads use [`from_json`]; this is for tests and for tools that
/// need to migrate to a version other than [`SCHEMA_VERSION`].
///
/// # Errors
///
/// The same as [`from_json`]. Note that a file is only deserialised into the
/// model once the registry has brought it to its target version, so a registry
/// targeting a version this build does not model will fail with
/// `model.invalid_project_file`.
pub fn from_json_with_registry(
    text: &str,
    registry: &MigrationRegistry,
) -> SubResult<(Project, LoadReport)> {
    let value: Value = serde_json::from_str(text).map_err(|err| {
        SubError::wrap(
            codes::INVALID_PROJECT_FILE,
            "project file is not valid JSON",
            &err,
        )
        .with_detail("line", err.line())
        .with_detail("column", err.column())
    })?;

    let (value, report) = registry.migrate(value)?;

    let file: ProjectFile = serde_json::from_value(value).map_err(|err| {
        SubError::wrap(
            codes::INVALID_PROJECT_FILE,
            "project file does not match the project schema",
            &err,
        )
        .with_detail("schema_version", report.final_version)
    })?;
    Ok((file.project, report))
}

/// Rebuilds `value` with every object's keys in sorted order.
///
/// `serde_json` already sorts when its map is a `BTreeMap`, but any crate in
/// the build may turn on its `preserve_order` feature, which would silently
/// make the output follow field declaration order instead. Sorting here means
/// the file stays byte-identical either way.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<(String, Value)> = object.into_iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.cmp(b));
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

/// The JSON Schema of a project file, as `schemars` generates it.
///
/// The committed copy lives at `docs/schema/project-v1.schema.json`; the
/// `committed_schema_is_up_to_date` test keeps the two in step.
#[must_use]
pub fn schema() -> Value {
    let schema = schemars::schema_for!(ProjectFile);
    sorted(schema.to_value())
}

/// The JSON Schema of a project file as the text committed under `docs/schema/`.
#[must_use]
pub fn schema_text() -> String {
    let mut text = serde_json::to_string_pretty(&schema()).unwrap_or_default();
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{ContentHash, MediaPath};
    use crate::marker::Marker;
    use crate::media::{AudioStream, Bin, MediaItem, ProxyState, StreamInfo, VideoStream};
    use crate::params::{Fixed6, GainDb, Opacity, Scale2, Transform};
    use crate::sequence::{Sequence, SequenceSettings};
    use crate::track::{Clip, Gap, Track, TrackKind, Transition};
    use sub_time::{Rational, RationalTime, TimeRange};

    /// A project exercising every branch of the schema.
    fn populated_project() -> Project {
        let rate = Rational::FPS_23_976;
        let mut project = Project::new("Doc cut");

        let mut interview = MediaItem::new(MediaPath::new("footage/interview.mp4").unwrap());
        interview.hash = Some(ContentHash::from_bytes([7; 32]));
        interview.info = Some(StreamInfo {
            duration: Some(RationalTime::new(2400, rate)),
            video: vec![VideoStream {
                width: 3840,
                height: 2160,
                frame_rate: rate,
                sample_aspect: Rational::ONE,
                color: crate::sequence::ColorTags::REC709,
            }],
            audio: vec![AudioStream {
                channels: 2,
                sample_rate: 48_000,
            }],
        });
        interview.proxy = ProxyState::Ready(MediaPath::new("project.sub.d/proxy.mov").unwrap());
        interview.offline = true;
        let interview_id = interview.id;

        let mut broll = MediaItem::new(MediaPath::new("footage/broll.mov").unwrap());
        broll.proxy = ProxyState::Failed("encoder unavailable".to_owned());
        let broll_id = broll.id;

        let mut pending = MediaItem::new(MediaPath::new("footage/tone.wav").unwrap());
        pending.proxy = ProxyState::Pending;

        project.media.push(interview);
        project.media.push(broll);
        project.media.push(pending);

        let mut bin = Bin::new("Interviews");
        bin.media.push(interview_id);
        bin.children.push(Bin::new("Takes"));
        project.root_bin.media.push(broll_id);
        project.root_bin.children.push(bin);

        let source =
            TimeRange::new(RationalTime::new(24, rate), RationalTime::new(96, rate)).unwrap();
        let mut clip = Clip::new("shot 1", interview_id, source);
        clip.opacity = Opacity::new(Fixed6::from_micros(750_000)).unwrap();
        clip.gain = GainDb::new(Fixed6::from_micros(-6_000_000)).unwrap();
        clip.transform = Transform::new(
            crate::params::Point2::new(Fixed6::from_units(12), Fixed6::from_units(-4)),
            Scale2::uniform(Fixed6::from_micros(1_500_000)).unwrap(),
            Fixed6::from_micros(90_000_000),
        );
        clip.fade_in = RationalTime::new(6, rate);
        clip.fade_out = RationalTime::new(12, rate);
        clip.markers.push(Marker::new(
            "look here",
            TimeRange::empty_at(RationalTime::new(30, rate)),
        ));

        let mut video = Track::new("V1", TrackKind::Video);
        video.items.push(clip.into());
        video.items.push(
            Transition::crossfade(RationalTime::new(6, rate), RationalTime::new(6, rate)).into(),
        );
        video
            .items
            .push(Clip::new("shot 2", broll_id, source).into());

        let mut audio = Track::new("A1", TrackKind::Audio);
        audio
            .items
            .push(Gap::new(RationalTime::new(24, rate)).into());
        audio
            .items
            .push(Clip::new("room tone", broll_id, source).into());

        let mut sequence = Sequence::new(
            "Main",
            SequenceSettings::new(
                crate::sequence::Resolution::UHD_2160,
                rate,
                48_000,
                crate::sequence::ColorTags::REC709,
            )
            .unwrap(),
        );
        sequence.tracks.push(video);
        sequence.tracks.push(audio);
        sequence.markers.push(Marker::new(
            "act two",
            TimeRange::new(RationalTime::new(48, rate), RationalTime::new(24, rate)).unwrap(),
        ));
        project.sequences.push(sequence);
        project
    }

    #[test]
    fn a_populated_project_round_trips_unchanged() {
        let project = populated_project();
        let text = to_json(&project).unwrap();
        assert_eq!(from_json(&text).unwrap(), project);
        assert_eq!(to_json(&from_json(&text).unwrap()).unwrap(), text);
    }

    #[test]
    fn the_file_is_pretty_sorted_and_newline_terminated() {
        /// Walks every object, asserting its keys are in sorted order.
        fn assert_sorted(value: &Value) {
            if let Value::Object(object) = value {
                let keys: Vec<&String> = object.keys().collect();
                let mut expected = keys.clone();
                expected.sort();
                assert_eq!(keys, expected, "object keys must be sorted");
                for child in object.values() {
                    assert_sorted(child);
                }
            } else if let Value::Array(items) = value {
                for item in items {
                    assert_sorted(item);
                }
            }
        }

        let text = to_json(&populated_project()).unwrap();
        assert!(text.ends_with('\n'));
        assert!(!text.ends_with("\n\n"));
        assert!(text.contains("\n  \"schema_version\": 1"));
        assert!(!text.contains('\r'));
        assert_sorted(&serde_json::from_str::<Value>(&text).unwrap());
    }

    #[test]
    fn saving_twice_produces_identical_bytes() {
        let project = populated_project();
        assert_eq!(to_json(&project).unwrap(), to_json(&project).unwrap());
        assert_eq!(
            to_json(&project).unwrap(),
            to_json(&project.clone()).unwrap()
        );
    }

    #[test]
    fn no_float_ever_reaches_the_file() {
        /// Fails on any JSON number that is not an integer.
        fn assert_no_float(value: &Value) {
            match value {
                Value::Number(number) => {
                    assert!(number.is_i64() || number.is_u64(), "float in project file");
                }
                Value::Array(items) => items.iter().for_each(assert_no_float),
                Value::Object(object) => object.values().for_each(assert_no_float),
                _ => {}
            }
        }
        let text = to_json(&populated_project()).unwrap();
        assert_no_float(&serde_json::from_str::<Value>(&text).unwrap());
    }

    #[test]
    fn a_newer_schema_version_is_refused() {
        let text = to_json(&Project::new("Doc cut"))
            .unwrap()
            .replace("\"schema_version\": 1", "\"schema_version\": 2");
        let err = from_json(&text).unwrap_err();
        assert_eq!(err.code, codes::UNSUPPORTED_SCHEMA_VERSION);
        assert!(err.message.contains("newer version"), "{}", err.message);
        assert_eq!(err.details.get("schema_version"), Some(&Value::from(2_u32)));
        assert_eq!(
            err.details.get("supported_schema_version"),
            Some(&Value::from(SCHEMA_VERSION))
        );
    }

    #[test]
    fn an_older_schema_version_without_a_migration_is_refused() {
        let text = to_json(&Project::new("Doc cut"))
            .unwrap()
            .replace("\"schema_version\": 1", "\"schema_version\": 0");
        let err = from_json(&text).unwrap_err();
        assert_eq!(err.code, codes::UNSUPPORTED_SCHEMA_VERSION);
        assert!(err.message.contains("older schema"), "{}", err.message);
    }

    #[test]
    fn a_file_without_a_schema_version_is_refused() {
        let err = from_json(r#"{"project": {}}"#).unwrap_err();
        assert_eq!(err.code, codes::INVALID_PROJECT_FILE);
        assert!(err.message.contains("schema_version"), "{}", err.message);

        let err = from_json(r#"{"project": {}, "schema_version": "1"}"#).unwrap_err();
        assert_eq!(err.code, codes::INVALID_PROJECT_FILE);
    }

    #[test]
    fn malformed_json_is_refused_with_a_position() {
        let err = from_json("{ not json").unwrap_err();
        assert_eq!(err.code, codes::INVALID_PROJECT_FILE);
        assert_eq!(err.details.get("line"), Some(&Value::from(1_u64)));
        assert!(err.cause.is_some());
    }

    #[test]
    fn values_the_model_rejects_do_not_load() {
        let text = to_json(&populated_project()).unwrap();
        for (from, to) in [
            // A zero frame-rate numerator.
            ("\"numerator\": 24000", "\"numerator\": 0"),
            // An opacity beyond 1.
            ("\"opacity\": 750000", "\"opacity\": 2000000"),
            // A media path that escapes the project folder.
            ("footage/interview.mp4", "../../etc/passwd"),
            // An unknown field, which a typo in a hand edit would produce.
            ("\"name\": \"Doc cut\"", "\"nmae\": \"Doc cut\""),
        ] {
            let broken = text.replacen(from, to, 1);
            assert_ne!(broken, text, "test edit {from} did not apply");
            let err = from_json(&broken).unwrap_err();
            assert_eq!(err.code, codes::INVALID_PROJECT_FILE, "{to}");
        }
    }

    #[test]
    fn the_schema_describes_a_project_file() {
        let schema = schema();
        assert_eq!(
            schema.get("$schema").and_then(Value::as_str),
            Some("https://json-schema.org/draft/2020-12/schema")
        );
        let required: Vec<&str> = schema
            .get("required")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(required.contains(&"schema_version"), "{required:?}");
        assert!(required.contains(&"project"), "{required:?}");
        assert!(schema.get("$defs").and_then(Value::as_object).is_some());
    }

    #[test]
    fn committed_schema_is_up_to_date() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/schema")
            .join(format!("project-v{SCHEMA_VERSION}.schema.json"));
        let generated = schema_text();
        if std::env::var_os("SUB_UPDATE_SCHEMA").is_some() {
            std::fs::write(&path, &generated).unwrap();
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            committed, generated,
            "the committed docs/schema JSON Schema is stale; regenerate it with \
             SUB_UPDATE_SCHEMA=1 cargo test -p sub-model"
        );
    }
}
