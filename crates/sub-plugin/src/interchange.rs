//! Host wiring for the `importer` and `exporter` worlds.
//!
//! Neither world touches the project directly. An importer parses a file and
//! describes what it found; this module turns that description into the
//! Command API calls the host makes on its behalf, so an import is an ordinary
//! undoable step and the identifiers of everything created are the host's to
//! mint. An exporter describes presets; this module validates them into the
//! [`ExportPreset`] rows the export panel lists.
//!
//! Both directions treat the plugin as untrusted input: a zero denominator, a
//! path escaping the project folder, a media reference pointing past the end of
//! the import, a preset id that is not in the stable form, or a setting whose
//! value is not JSON all come back as a [`SubError`] with a `plugin.*` code
//! rather than reaching the project.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use sub_core::{SubError, SubResult};
use sub_model::{
    BinId, Clip, ColorTags, Gap, Marker, MediaId, MediaItem, MediaPath, Resolution, Sequence,
    SequenceSettings, Track, TrackItem, TrackKind, Transition,
};
use sub_time::{Rational, RationalTime, TimeRange};

use crate::bindings::exporter::exports::subordinate::plugin::exporter_api as wit_exporter;
use crate::bindings::importer::exports::subordinate::plugin::importer_api as wit_importer;
use crate::codes;

/// One Command API call the host makes on a plugin's behalf.
///
/// `method` and `params` are exactly what the JSON-RPC dispatcher takes, which
/// is also what `command-api.run-command` takes: the host applies an import by
/// feeding these to the same dispatcher an external agent reaches, so every
/// call lands on the undo stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandCall {
    /// The Command API method name, e.g. `media.import`.
    pub method: &'static str,
    /// The method's `params` object.
    pub params: Value,
}

/// Where an import lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImportTarget {
    /// The bin imported media is filed in. The project's root bin when absent.
    pub bin: Option<BinId>,
    /// The index the first imported sequence is inserted at; further sequences
    /// follow it. Usually the project's current sequence count, which appends.
    pub sequence_index: usize,
}

impl ImportTarget {
    /// Files media in the root bin and appends sequences after the `existing`
    /// sequences a project already has.
    #[must_use]
    pub fn appending(existing: usize) -> Self {
        Self {
            bin: None,
            sequence_index: existing,
        }
    }

    /// The same target, filing media in `bin`.
    #[must_use]
    pub fn into_bin(mut self, bin: BinId) -> Self {
        self.bin = Some(bin);
        self
    }
}

/// Turns one importer result into the Command API calls that apply it.
///
/// Media comes first, whatever order the plugin listed things in: a sequence
/// may play media from the same import, and the clips referring to it need the
/// media items already in the project. Each media file becomes one
/// `media.import` and each sequence one `sequence.insert` carrying the whole
/// built sequence, so a multi-sequence import is a handful of undoable steps
/// rather than one per clip.
///
/// # Errors
///
/// Returns `plugin.invalid_spec` when the plugin described something the model
/// cannot hold — an out-of-range media reference, an unnamed clip on media the
/// import did not create, a zero sample rate — and the `plugin.invalid_*` or
/// `model.invalid_*` code of whichever conversion failed otherwise.
pub fn import_plan(
    specs: &[wit_importer::MediaOrSequenceSpec],
    target: ImportTarget,
) -> SubResult<Vec<CommandCall>> {
    let mut calls = Vec::with_capacity(specs.len());
    let mut imported: Vec<(MediaId, String)> = Vec::new();

    for spec in specs {
        let wit_importer::MediaOrSequenceSpec::Media(media) = spec else {
            continue;
        };
        let item = media_item(media)?;
        imported.push((item.id, item.name.clone()));
        let mut params =
            json!({ "item": serde_json::to_value(&item).map_err(|error| json_error(&error))? });
        if let Some(bin) = target.bin {
            params["bin"] = serde_json::to_value(bin).map_err(|error| json_error(&error))?;
        }
        calls.push(CommandCall {
            method: "media.import",
            params,
        });
    }

    let mut index = target.sequence_index;
    for spec in specs {
        let wit_importer::MediaOrSequenceSpec::Sequence(sequence) = spec else {
            continue;
        };
        let built = build_sequence(sequence, &imported)?;
        calls.push(CommandCall {
            method: "sequence.insert",
            params: json!({
                "index": index,
                "sequence": serde_json::to_value(&built).map_err(|error| json_error(&error))?,
            }),
        });
        index += 1;
    }

    Ok(calls)
}

/// Builds the media item one `media-spec` describes.
fn media_item(spec: &wit_importer::MediaSpec) -> SubResult<MediaItem> {
    let path = MediaPath::new(&spec.path)?;
    let mut item = MediaItem::new(path);
    if !spec.name.is_empty() {
        spec.name.clone_into(&mut item.name);
    }
    Ok(item)
}

/// Builds the sequence one `sequence-spec` describes.
fn build_sequence(
    spec: &wit_importer::SequenceSpec,
    imported: &[(MediaId, String)],
) -> SubResult<Sequence> {
    let settings = SequenceSettings::new(
        Resolution::try_from(spec.resolution)?,
        Rational::try_from(spec.frame_rate)?,
        spec.sample_rate,
        ColorTags::default(),
    )?;
    let mut sequence = Sequence::new(spec.name.clone(), settings);
    for track in &spec.tracks {
        sequence.tracks.push(build_track(track, imported)?);
    }
    for marker in &spec.markers {
        sequence.markers.push(build_marker(marker)?);
    }
    Ok(sequence)
}

/// Builds one lane and everything on it.
fn build_track(spec: &wit_importer::TrackSpec, imported: &[(MediaId, String)]) -> SubResult<Track> {
    let mut track = Track::new(spec.name.clone(), TrackKind::from(spec.kind));
    for item in &spec.items {
        track.items.push(match item {
            wit_importer::TrackItemSpec::Clip(clip) => TrackItem::Clip(build_clip(clip, imported)?),
            wit_importer::TrackItemSpec::Gap(duration) => {
                let duration = RationalTime::try_from(*duration)?;
                if duration.is_negative() {
                    return Err(invalid_spec("a gap must last a non-negative time")
                        .with_detail("duration", duration.to_string()));
                }
                TrackItem::Gap(Gap::new(duration))
            }
            wit_importer::TrackItemSpec::Transition(transition) => {
                TrackItem::Transition(Transition::crossfade(
                    RationalTime::try_from(transition.in_offset)?,
                    RationalTime::try_from(transition.out_offset)?,
                ))
            }
        });
    }
    Ok(track)
}

/// Builds one clip, resolving the media it plays.
fn build_clip(spec: &wit_importer::ClipSpec, imported: &[(MediaId, String)]) -> SubResult<Clip> {
    let (media, default_name) = match &spec.media {
        wit_importer::MediaRef::Imported(index) => {
            let entry = usize::try_from(*index)
                .ok()
                .and_then(|index| imported.get(index))
                .ok_or_else(|| {
                    invalid_spec("a clip refers to a media item this import did not produce")
                        .with_detail("index", u64::from(*index))
                        .with_detail("imported", imported.len())
                })?;
            (entry.0, Some(entry.1.as_str()))
        }
        wit_importer::MediaRef::Existing(id) => (MediaId::try_from(id)?, None),
    };

    let name = if spec.name.is_empty() {
        default_name
            .ok_or_else(|| {
                invalid_spec("a clip playing media already in the project must be named")
                    .with_detail("media", media)
            })?
            .to_owned()
    } else {
        spec.name.clone()
    };

    let mut clip = Clip::new(name, media, TimeRange::try_from(spec.source_range)?);
    for marker in &spec.markers {
        clip.markers.push(build_marker(marker)?);
    }
    clip.validate()?;
    Ok(clip)
}

/// Builds one marker, on a sequence or on a clip.
fn build_marker(spec: &wit_importer::MarkerSpec) -> SubResult<Marker> {
    let mut marker = Marker::new(spec.name.clone(), TimeRange::try_from(spec.marked_range)?);
    spec.note.clone_into(&mut marker.note);
    Ok(marker)
}

/// One export preset a plugin contributes, validated for the export panel.
///
/// The frame rate is a [`Rational`] and never a float, and the settings stay
/// open: the panel hands them to the encoder as they are, so a new encoder
/// option needs no change here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportPreset {
    /// Stable identifier, unique within the plugin, e.g. `youtube.1080p`.
    pub id: String,
    /// Display name for the export panel.
    pub name: String,
    /// One-line description; empty when the plugin gave none.
    pub description: String,
    /// The container extension written, without the dot, e.g. `mp4`.
    pub container: String,
    /// The rate the preset encodes at. `None` means the sequence's own rate.
    pub frame_rate: Option<Rational>,
    /// Encoder settings, keyed and ordered by key.
    pub settings: BTreeMap<String, Value>,
}

impl TryFrom<&wit_exporter::PresetDesc> for ExportPreset {
    type Error = SubError;

    /// # Errors
    ///
    /// Returns `plugin.invalid_preset` when the id is not in the stable form,
    /// the name or container is empty or the container is not a bare lowercase
    /// extension, a setting key repeats or its value is not JSON, and
    /// `plugin.invalid_rational` when the frame rate has a zero term.
    fn try_from(desc: &wit_exporter::PresetDesc) -> SubResult<Self> {
        if !is_stable_id(&desc.id) {
            return Err(invalid_preset(
                "a preset id must be two or more dot-separated segments of [a-z0-9_]",
            )
            .with_detail("id", desc.id.clone()));
        }
        if desc.name.is_empty() {
            return Err(invalid_preset("a preset must have a display name")
                .with_detail("id", desc.id.clone()));
        }
        if desc.container.is_empty() || !is_extension(&desc.container) {
            return Err(
                invalid_preset("a preset container must be a bare lowercase extension")
                    .with_detail("id", desc.id.clone())
                    .with_detail("container", desc.container.clone()),
            );
        }

        let mut settings = BTreeMap::new();
        for setting in &desc.settings {
            let value: Value = serde_json::from_str(&setting.value).map_err(|error| {
                invalid_preset("a preset setting value must be a JSON fragment")
                    .with_detail("id", desc.id.clone())
                    .with_detail("key", setting.key.clone())
                    .with_detail("error", error.to_string())
            })?;
            if settings.insert(setting.key.clone(), value).is_some() {
                return Err(invalid_preset("a preset setting key is repeated")
                    .with_detail("id", desc.id.clone())
                    .with_detail("key", setting.key.clone()));
            }
        }

        Ok(Self {
            id: desc.id.clone(),
            name: desc.name.clone(),
            description: desc.description.clone(),
            container: desc.container.clone(),
            frame_rate: desc.frame_rate.map(Rational::try_from).transpose()?,
            settings,
        })
    }
}

/// Validates everything one exporter offers, in the order it listed them.
///
/// # Errors
///
/// Returns `plugin.invalid_preset` when a preset is malformed or two share an
/// id: the panel lists presets by id, and a duplicate would make the choice
/// the project file remembers ambiguous.
pub fn export_presets(descs: &[wit_exporter::PresetDesc]) -> SubResult<Vec<ExportPreset>> {
    let mut presets = Vec::with_capacity(descs.len());
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for desc in descs {
        let preset = ExportPreset::try_from(desc)?;
        if !seen.insert(desc.id.as_str()) {
            return Err(
                invalid_preset("two presets share an id").with_detail("id", desc.id.clone())
            );
        }
        presets.push(preset);
    }
    Ok(presets)
}

/// Whether `id` is two or more dot-separated segments of `[a-z0-9_]`, the same
/// stable form an error code takes.
fn is_stable_id(id: &str) -> bool {
    let mut segments = 0;
    for segment in id.split('.') {
        if segment.is_empty()
            || !segment
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return false;
        }
        segments += 1;
    }
    segments >= 2
}

/// Whether `text` is a bare lowercase file extension: no dot, no separator.
fn is_extension(text: &str) -> bool {
    text.bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// A `plugin.invalid_spec` error.
fn invalid_spec(message: &str) -> SubError {
    SubError::new(codes::INVALID_SPEC, message)
}

/// A `plugin.invalid_preset` error.
fn invalid_preset(message: &str) -> SubError {
    SubError::new(codes::INVALID_PRESET, message)
}

/// Wraps a `serde_json` failure serialising a model value the host built.
fn json_error(error: &serde_json::Error) -> SubError {
    SubError::new(codes::INVALID_SPEC, "an imported item could not be encoded")
        .with_detail("error", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WitResolution;
    use crate::bindings::subordinate::plugin::command_api::TrackKind as WitTrackKind;
    use crate::bindings::subordinate::plugin::types as wit_types;

    /// 23.976, so nothing here can be done in whole ticks per second.
    fn rate() -> wit_types::Rational {
        wit_types::Rational {
            numerator: 24_000,
            denominator: 1_001,
        }
    }

    /// `frames` at [`rate`], the way a plugin would send it.
    fn time(frames: i64) -> wit_types::RationalTime {
        wit_types::RationalTime {
            value: frames,
            rate: rate(),
        }
    }

    /// The half-open span `[start, start + duration)` in frames.
    fn range(start: i64, duration: i64) -> wit_types::TimeRange {
        wit_types::TimeRange {
            start: time(start),
            duration: time(duration),
        }
    }

    fn media(path: &str, name: &str) -> wit_importer::MediaOrSequenceSpec {
        wit_importer::MediaOrSequenceSpec::Media(wit_importer::MediaSpec {
            path: path.to_owned(),
            name: name.to_owned(),
        })
    }

    fn clip(name: &str, media: wit_importer::MediaRef, start: i64) -> wit_importer::TrackItemSpec {
        wit_importer::TrackItemSpec::Clip(wit_importer::ClipSpec {
            name: name.to_owned(),
            media,
            source_range: range(start, 24),
            markers: Vec::new(),
        })
    }

    fn sequence(items: Vec<wit_importer::TrackItemSpec>) -> wit_importer::MediaOrSequenceSpec {
        wit_importer::MediaOrSequenceSpec::Sequence(wit_importer::SequenceSpec {
            name: "Reel 1".to_owned(),
            frame_rate: rate(),
            resolution: WitResolution {
                width: 1920,
                height: 1080,
            },
            sample_rate: 48_000,
            tracks: vec![wit_importer::TrackSpec {
                name: "V1".to_owned(),
                kind: WitTrackKind::Video,
                items,
            }],
            markers: vec![wit_importer::MarkerSpec {
                name: "sync".to_owned(),
                note: "clap".to_owned(),
                marked_range: range(12, 0),
            }],
        })
    }

    fn preset() -> wit_exporter::PresetDesc {
        wit_exporter::PresetDesc {
            id: "youtube.1080p".to_owned(),
            name: "YouTube 1080p".to_owned(),
            description: "H.264 in MP4".to_owned(),
            container: "mp4".to_owned(),
            frame_rate: Some(rate()),
            settings: vec![wit_exporter::PresetSetting {
                key: "bitrate".to_owned(),
                value: "12000000".to_owned(),
            }],
        }
    }

    #[test]
    fn media_is_imported_before_the_sequences_that_play_it() {
        // The plugin lists the sequence first; the plan still files the media
        // first, because `sequence.insert` refuses a clip whose media is
        // absent.
        let specs = vec![
            sequence(vec![clip("", wit_importer::MediaRef::Imported(0), 0)]),
            media("footage/a.mov", ""),
        ];
        let calls = import_plan(&specs, ImportTarget::appending(2)).unwrap();

        assert_eq!(
            calls.iter().map(|call| call.method).collect::<Vec<_>>(),
            ["media.import", "sequence.insert"],
        );
        // An empty name falls back to the file name, and no bin means the root.
        assert_eq!(calls[0].params["item"]["name"], "a.mov");
        assert_eq!(calls[0].params["item"]["path"], "footage/a.mov");
        assert!(calls[0].params.get("bin").is_none());

        let inserted = &calls[1].params;
        assert_eq!(inserted["index"], 2);
        assert_eq!(inserted["sequence"]["name"], "Reel 1");
        let clip = &inserted["sequence"]["tracks"][0]["items"][0]["clip"];
        // The clip plays the media the same import produced, and is named
        // after it.
        assert_eq!(clip["media"], calls[0].params["item"]["id"]);
        assert_eq!(clip["name"], "a.mov");
    }

    #[test]
    fn a_target_bin_and_a_sequence_index_are_carried_into_the_calls() {
        let bin = BinId::new();
        let specs = vec![
            media("a.mov", "A"),
            sequence(Vec::new()),
            sequence(Vec::new()),
        ];
        let calls = import_plan(&specs, ImportTarget::appending(1).into_bin(bin)).unwrap();

        assert_eq!(calls[0].params["bin"], serde_json::to_value(bin).unwrap());
        assert_eq!(calls[0].params["item"]["name"], "A");
        assert_eq!(calls[1].params["index"], 1);
        assert_eq!(calls[2].params["index"], 2);
    }

    #[test]
    fn gaps_transitions_and_markers_survive_as_exact_time() {
        let specs = vec![
            media("a.mov", ""),
            sequence(vec![
                wit_importer::TrackItemSpec::Gap(time(10)),
                clip("Take 2", wit_importer::MediaRef::Imported(0), 5),
                wit_importer::TrackItemSpec::Transition(wit_importer::TransitionSpec {
                    in_offset: time(2),
                    out_offset: time(3),
                }),
            ]),
        ];
        let calls = import_plan(&specs, ImportTarget::default()).unwrap();
        let items = &calls[1].params["sequence"]["tracks"][0]["items"];

        assert_eq!(items[0]["gap"]["duration"]["value"], 10);
        assert_eq!(items[0]["gap"]["duration"]["rate"]["numerator"], 24_000);
        assert_eq!(items[0]["gap"]["duration"]["rate"]["denominator"], 1_001);
        assert_eq!(items[1]["clip"]["source_range"]["start"]["value"], 5);
        assert_eq!(items[2]["transition"]["crossfade"]["in_offset"]["value"], 2);
        let marker = &calls[1].params["sequence"]["markers"][0];
        assert_eq!(marker["name"], "sync");
        assert_eq!(marker["note"], "clap");
    }

    #[test]
    fn an_existing_media_reference_is_resolved_and_must_be_named() {
        let id = MediaId::new();
        let reference = wit_importer::MediaRef::Existing(wit_types::MediaId {
            value: id.to_string(),
        });
        let calls = import_plan(
            &[sequence(vec![clip("Insert", reference.clone(), 0)])],
            ImportTarget::default(),
        )
        .unwrap();
        assert_eq!(
            calls[0].params["sequence"]["tracks"][0]["items"][0]["clip"]["media"],
            serde_json::to_value(id).unwrap(),
        );

        let error = import_plan(
            &[sequence(vec![clip("", reference, 0)])],
            ImportTarget::default(),
        )
        .unwrap_err();
        assert_eq!(error.code, codes::INVALID_SPEC);
    }

    #[test]
    fn a_media_reference_past_the_end_of_the_import_is_refused() {
        let specs = vec![
            media("a.mov", ""),
            sequence(vec![clip("X", wit_importer::MediaRef::Imported(1), 0)]),
        ];
        let error = import_plan(&specs, ImportTarget::default()).unwrap_err();
        assert_eq!(error.code, codes::INVALID_SPEC);
        assert_eq!(error.details.get("index").unwrap(), 1);
    }

    #[test]
    fn malformed_specs_come_back_as_stable_codes_and_never_reach_the_project() {
        // A gap that runs backwards.
        let specs = vec![sequence(vec![wit_importer::TrackItemSpec::Gap(time(-1))])];
        assert_eq!(
            import_plan(&specs, ImportTarget::default())
                .unwrap_err()
                .code,
            codes::INVALID_SPEC,
        );

        // A path escaping the project folder.
        let error =
            import_plan(&[media("../secrets.mov", "")], ImportTarget::default()).unwrap_err();
        assert_eq!(error.code.as_str(), "model.invalid_path");

        // A rate with a zero term.
        let wit_importer::MediaOrSequenceSpec::Sequence(mut spec) = sequence(Vec::new()) else {
            unreachable!("the fixture is a sequence");
        };
        spec.frame_rate = wit_types::Rational {
            numerator: 0,
            denominator: 1,
        };
        let error = import_plan(
            &[wit_importer::MediaOrSequenceSpec::Sequence(spec.clone())],
            ImportTarget::default(),
        )
        .unwrap_err();
        assert_eq!(error.code, codes::INVALID_RATIONAL);

        // A zero audio sample rate.
        spec.frame_rate = rate();
        spec.sample_rate = 0;
        let error = import_plan(
            &[wit_importer::MediaOrSequenceSpec::Sequence(spec)],
            ImportTarget::default(),
        )
        .unwrap_err();
        assert_eq!(error.code.as_str(), "model.invalid_settings");
    }

    #[test]
    fn a_preset_crosses_with_an_exact_rate_and_open_settings() {
        let presets = export_presets(&[preset()]).unwrap();
        assert_eq!(presets.len(), 1);
        let preset = &presets[0];
        assert_eq!(preset.id, "youtube.1080p");
        assert_eq!(preset.container, "mp4");
        assert_eq!(preset.frame_rate, Some(Rational::FPS_23_976));
        assert_eq!(preset.settings["bitrate"], serde_json::json!(12_000_000));
    }

    #[test]
    fn a_preset_without_a_rate_follows_the_sequence() {
        let mut desc = preset();
        desc.frame_rate = None;
        assert_eq!(export_presets(&[desc]).unwrap()[0].frame_rate, None);
    }

    #[test]
    fn malformed_presets_are_refused_before_the_panel_sees_them() {
        type Break = fn(&mut wit_exporter::PresetDesc);
        let cases: [(&str, Break); 7] = [
            ("Bad-Id", |desc: &mut wit_exporter::PresetDesc| {
                desc.id = "Not.Stable".to_owned();
            }),
            ("one segment", |desc: &mut wit_exporter::PresetDesc| {
                desc.id = "youtube".to_owned();
            }),
            ("no name", |desc: &mut wit_exporter::PresetDesc| {
                desc.name = String::new();
            }),
            ("dotted container", |desc: &mut wit_exporter::PresetDesc| {
                desc.container = ".mp4".to_owned();
            }),
            ("empty container", |desc: &mut wit_exporter::PresetDesc| {
                desc.container = String::new();
            }),
            (
                "value is not JSON",
                |desc: &mut wit_exporter::PresetDesc| {
                    desc.settings[0].value = "12000000!".to_owned();
                },
            ),
            ("repeated key", |desc: &mut wit_exporter::PresetDesc| {
                let repeat = desc.settings[0].clone();
                desc.settings.push(repeat);
            }),
        ];
        for (case, break_it) in cases {
            let mut desc = preset();
            break_it(&mut desc);
            let error = export_presets(&[desc]).unwrap_err();
            assert_eq!(error.code, codes::INVALID_PRESET, "{case}");
        }

        // Two well-formed presets sharing an id are ambiguous to the panel.
        let error = export_presets(&[preset(), preset()]).unwrap_err();
        assert_eq!(error.code, codes::INVALID_PRESET);
    }

    #[test]
    fn a_preset_rate_with_a_zero_term_is_refused() {
        let mut desc = preset();
        desc.frame_rate = Some(wit_types::Rational {
            numerator: 24,
            denominator: 0,
        });
        assert_eq!(
            export_presets(&[desc]).unwrap_err().code,
            codes::INVALID_RATIONAL,
        );
    }
}
