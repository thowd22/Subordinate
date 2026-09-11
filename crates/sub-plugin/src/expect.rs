//! What a fixture says the timeline should look like once a command plugin
//! has run.
//!
//! The harness's `project_state` check proves a command plugin *changed* the
//! fixture and that one undo reversed it, but it compares counts: a plugin
//! that removed 25 frames instead of 15 passes it exactly like one that got
//! the edit right (TASK-129, found by the TASK-102 runbook run). A fixture
//! that carries an expectation turns "something changed" into "this is what it
//! should be".
//!
//! The expectation is a sidecar beside the fixture project: for
//! `fixture/fixture.sub` it is `fixture/fixture.expect.json`
//! ([`sidecar_for`]). A fixture that ships no sidecar declares nothing and the
//! run behaves exactly as it did before — the timeline check is skipped, not
//! failed.
//!
//! The document names the track layout expected *after* the run, as clips in
//! sequence time:
//!
//! ```json
//! {
//!   "sequences": [
//!     {
//!       "name": "Main",
//!       "tracks": [
//!         {
//!           "name": "V1",
//!           "kind": "video",
//!           "clips": [
//!             {
//!               "name": "shot-a",
//!               "start": { "value": 0, "rate": { "numerator": 24, "denominator": 1 } },
//!               "duration": { "value": 15, "rate": { "numerator": 24, "denominator": 1 } }
//!             }
//!           ]
//!         }
//!       ]
//!     }
//!   ]
//! }
//! ```
//!
//! Times are [`RationalTime`] in its own serde form — an integer `value` at an
//! exact rational `rate` — never seconds and never a float, so an expectation
//! means one instant rather than a rounded one (docs/PLAN.md §5.1). Comparison
//! is [`RationalTime`]'s own: 1 frame at 24 fps *is* 2 frames at 48 fps, so a
//! fixture may state its times at whatever timebase reads best.
//!
//! Everything but a clip's `start` and `duration` is optional. A sequence or
//! track that names nothing is matched by position; one that carries a `name`
//! or a `kind` must also match it, which is what keeps an expectation readable
//! when the plugin under test reorders nothing.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult, codes};
use sub_model::{Project, TrackKind};
use sub_time::RationalTime;

/// The suffix a fixture's expectation sidecar carries.
///
/// `fixture.sub` declares its expectation in `fixture.expect.json`.
pub const SIDECAR_SUFFIX: &str = "expect.json";

/// The sidecar path for `fixture`: its file stem plus `.expect.json`.
///
/// The extension is replaced rather than appended, so `fixture.sub` maps to
/// `fixture.expect.json` and a fixture with no extension at all to
/// `fixture.expect.json` beside it.
#[must_use]
pub fn sidecar_for(fixture: &Path) -> PathBuf {
    fixture.with_extension(SIDECAR_SUFFIX)
}

/// The timeline a fixture expects once a command plugin has run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineExpectation {
    /// One entry per sequence, in the project's own order.
    pub sequences: Vec<SequenceExpectation>,
}

/// One sequence's expected tracks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SequenceExpectation {
    /// The sequence's display name, checked when it is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// One entry per track, bottom-most first, as the model orders them.
    pub tracks: Vec<TrackExpectation>,
}

/// One track's expected clips.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackExpectation {
    /// The track's display name, checked when it is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether the lane carries picture or sound, checked when it is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<TrackKind>,
    /// Every clip the track should hold afterwards, in playback order. Gaps
    /// are not listed: a clip's `start` already says where the gaps put it.
    pub clips: Vec<ClipExpectation>,
}

/// One clip's expected place on the timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipExpectation {
    /// The clip's display name, checked when it is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Where the clip starts in sequence time.
    pub start: RationalTime,
    /// How long it runs for in sequence time.
    pub duration: RationalTime,
}

impl TimelineExpectation {
    /// Reads an expectation from `path`.
    ///
    /// # Errors
    ///
    /// [`codes::INVALID_ARGUMENT`] when the file cannot be read or is not an
    /// expectation document; the path is carried as a detail either way, so a
    /// hand-written sidecar with a typo names itself in the report.
    pub fn load(path: &Path) -> SubResult<Self> {
        let text = std::fs::read_to_string(path).map_err(|err| {
            SubError::wrap(
                codes::INVALID_ARGUMENT,
                "the fixture's timeline expectation could not be read",
                &err,
            )
            .with_detail("path", path.display().to_string())
        })?;
        serde_json::from_str(&text).map_err(|err| {
            SubError::wrap(
                codes::INVALID_ARGUMENT,
                "the fixture's timeline expectation is not a valid expectation document",
                &err,
            )
            .with_detail("path", path.display().to_string())
        })
    }

    /// The expectation beside `fixture`, or `None` when it declares one.
    ///
    /// # Errors
    ///
    /// What [`Self::load`] returns for a sidecar that is there but unreadable.
    /// A sidecar that is not there is not an error: it is a fixture that
    /// expects nothing.
    pub fn beside(fixture: &Path) -> SubResult<Option<(PathBuf, Self)>> {
        let path = sidecar_for(fixture);
        if !path.is_file() {
            return Ok(None);
        }
        Self::load(&path).map(|expectation| Some((path, expectation)))
    }

    /// Every way `project` differs from what this expects, in reading order.
    ///
    /// An empty answer is a match. Each entry names the path it is about —
    /// `sequences[0].tracks[1].clips[2].start` — and both values, so a report
    /// reader need not diff two documents by eye.
    #[must_use]
    pub fn mismatches(&self, project: &Project) -> Vec<String> {
        let mut found = Vec::new();
        if project.sequences.len() != self.sequences.len() {
            found.push(format!(
                "sequences: expected {} but the project has {}",
                self.sequences.len(),
                project.sequences.len(),
            ));
        }
        for (index, expected) in self.sequences.iter().enumerate() {
            let Some(sequence) = project.sequences.get(index) else {
                break;
            };
            let at = format!("sequences[{index}]");
            if let Some(name) = &expected.name
                && *name != sequence.name
            {
                found.push(format!(
                    "{at}.name: expected {name:?} but the project has {:?}",
                    sequence.name,
                ));
            }
            if sequence.tracks.len() != expected.tracks.len() {
                found.push(format!(
                    "{at}.tracks: expected {} but the project has {}",
                    expected.tracks.len(),
                    sequence.tracks.len(),
                ));
            }
            let rate = sequence.settings.frame_rate;
            for (index, expected) in expected.tracks.iter().enumerate() {
                let Some(track) = sequence.tracks.get(index) else {
                    break;
                };
                let at = format!("{at}.tracks[{index}]");
                if let Some(name) = &expected.name
                    && *name != track.name
                {
                    found.push(format!(
                        "{at}.name: expected {name:?} but the project has {:?}",
                        track.name,
                    ));
                }
                if let Some(kind) = expected.kind
                    && kind != track.kind
                {
                    found.push(format!(
                        "{at}.kind: expected {kind:?} but the project has {:?}",
                        track.kind,
                    ));
                }
                let clips: Vec<_> = track.clip_placements(rate).collect();
                if clips.len() != expected.clips.len() {
                    found.push(format!(
                        "{at}.clips: expected {} but the track holds {}",
                        expected.clips.len(),
                        clips.len(),
                    ));
                }
                for (index, expected) in expected.clips.iter().enumerate() {
                    let Some((clip, range)) = clips.get(index) else {
                        break;
                    };
                    let at = format!("{at}.clips[{index}]");
                    if let Some(name) = &expected.name
                        && *name != clip.name
                    {
                        found.push(format!(
                            "{at}.name: expected {name:?} but the clip is named {:?}",
                            clip.name,
                        ));
                    }
                    difference(&mut found, &at, "start", expected.start, range.start());
                    difference(
                        &mut found,
                        &at,
                        "duration",
                        expected.duration,
                        range.duration(),
                    );
                }
            }
        }
        found
    }
}

/// Records one time that is not what the fixture asked for.
///
/// Both sides are printed as their exact `value/rate`, never as seconds: a
/// mismatch of one frame at 24 fps and one at 48 fps must not read as the same
/// number.
fn difference(
    found: &mut Vec<String>,
    at: &str,
    field: &str,
    expected: RationalTime,
    actual: RationalTime,
) {
    if expected == actual {
        return;
    }
    found.push(format!(
        "{at}.{field}: expected {} but the clip is at {}",
        exact(expected),
        exact(actual),
    ));
}

/// One time as a mismatch prints it: `15@24/1`.
fn exact(time: RationalTime) -> String {
    let rate = time.rate();
    format!(
        "{}@{}/{}",
        time.value(),
        rate.numerator(),
        rate.denominator(),
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sub_model::{
        Clip, MediaId, Project, Sequence, SequenceSettings, Track, TrackItem, TrackKind,
    };
    use sub_time::{Rational, RationalTime, TimeRange};

    use super::{TimelineExpectation, sidecar_for};

    /// 24 fps, the rate the fixtures below are cut at.
    const RATE: Rational = Rational::FPS_24;

    /// A project with one video track holding clips of the given frame
    /// durations, laid end to end.
    fn project(durations: &[i64]) -> Project {
        let settings = SequenceSettings::new(
            sub_model::Resolution::new(1920, 1080).expect("a canvas"),
            RATE,
            48_000,
            sub_model::ColorTags::default(),
        )
        .expect("settings");
        let mut sequence = Sequence::new("Main", settings);
        let mut track = Track::new("V1", TrackKind::Video);
        for (index, frames) in durations.iter().enumerate() {
            let range = TimeRange::new(
                RationalTime::from_frames(0, RATE),
                RationalTime::from_frames(*frames, RATE),
            )
            .expect("a source range");
            track.items.push(TrackItem::Clip(Clip::new(
                format!("shot-{index}"),
                MediaId::new(),
                range,
            )));
        }
        sequence.tracks.push(track);
        let mut project = Project::new("Fixture");
        project.sequences.push(sequence);
        project
    }

    /// An expectation over one video track of `(start, duration)` frame pairs.
    fn expectation(clips: &[(i64, i64)], rate: Rational) -> TimelineExpectation {
        TimelineExpectation {
            sequences: vec![super::SequenceExpectation {
                name: Some("Main".to_owned()),
                tracks: vec![super::TrackExpectation {
                    name: Some("V1".to_owned()),
                    kind: Some(TrackKind::Video),
                    clips: clips
                        .iter()
                        .map(|(start, duration)| super::ClipExpectation {
                            name: None,
                            start: RationalTime::from_frames(*start, rate),
                            duration: RationalTime::from_frames(*duration, rate),
                        })
                        .collect(),
                }],
            }],
        }
    }

    #[test]
    fn a_timeline_that_is_what_the_fixture_asked_for_has_no_mismatches() {
        let expected = expectation(&[(0, 15), (15, 30)], RATE);
        assert!(expected.mismatches(&project(&[15, 30])).is_empty());
    }

    /// The failure the harness could not see before: the plugin trimmed the
    /// wrong number of frames, so every later clip is in the wrong place.
    #[test]
    fn a_clip_of_the_wrong_length_is_reported_with_both_times() {
        let expected = expectation(&[(0, 15), (15, 30)], RATE);
        let found = expected.mismatches(&project(&[25, 30]));
        assert_eq!(found.len(), 2, "{found:#?}");
        assert!(
            found[0].contains("clips[0].duration: expected 15@24/1 but the clip is at 25@24/1"),
            "{found:#?}",
        );
        assert!(found[1].contains("clips[1].start"), "{found:#?}");
    }

    /// The comparison is exact time, not a raw representation: the same
    /// instants stated at 48 fps match a 24 fps timeline.
    #[test]
    fn times_are_compared_as_exact_instants_across_timebases() {
        let rate = Rational::new(48, 1).expect("48 fps");
        let expected = expectation(&[(0, 30), (30, 60)], rate);
        assert!(expected.mismatches(&project(&[15, 30])).is_empty());
        let wrong = expectation(&[(0, 31), (31, 60)], rate);
        assert!(!wrong.mismatches(&project(&[15, 30])).is_empty());
    }

    #[test]
    fn a_track_of_the_wrong_length_or_name_is_reported() {
        let expected = expectation(&[(0, 15)], RATE);
        let found = expected.mismatches(&project(&[15, 30]));
        assert_eq!(found.len(), 1, "{found:#?}");
        assert!(found[0].contains("clips: expected 1 but the track holds 2"));

        let mut named = expectation(&[(0, 15)], RATE);
        named.sequences[0].tracks[0].name = Some("A1".to_owned());
        named.sequences[0].tracks[0].kind = Some(TrackKind::Audio);
        let found = named.mismatches(&project(&[15]));
        assert_eq!(found.len(), 2, "{found:#?}");
        assert!(found[0].contains("tracks[0].name"), "{found:#?}");
        assert!(found[1].contains("tracks[0].kind"), "{found:#?}");
    }

    #[test]
    fn a_missing_sequence_is_reported_without_indexing_past_the_project() {
        let mut expected = expectation(&[(0, 15)], RATE);
        let second = expected.sequences[0].clone();
        expected.sequences.push(second);
        let found = expected.mismatches(&project(&[15]));
        assert_eq!(found.len(), 1, "{found:#?}");
        assert!(found[0].contains("sequences: expected 2 but the project has 1"));
    }

    /// The document is what a fixture author writes by hand: exact times, and
    /// nothing else required.
    #[test]
    fn the_document_round_trips_through_its_serde_form() {
        let text = r#"{
            "sequences": [
                {
                    "tracks": [
                        {
                            "kind": "video",
                            "clips": [
                                {
                                    "name": "shot-0",
                                    "start": { "value": 0, "rate": { "numerator": 24, "denominator": 1 } },
                                    "duration": { "value": 15, "rate": { "numerator": 24, "denominator": 1 } }
                                }
                            ]
                        }
                    ]
                }
            ]
        }"#;
        let expected: TimelineExpectation = serde_json::from_str(text).expect("an expectation");
        assert!(expected.mismatches(&project(&[15])).is_empty());
        let again: TimelineExpectation =
            serde_json::from_str(&serde_json::to_string(&expected).expect("json"))
                .expect("the same document");
        assert_eq!(again, expected);
    }

    #[test]
    fn a_field_that_is_not_in_the_document_is_refused_rather_than_ignored() {
        let text = r#"{ "sequences": [ { "tracks": [], "clips": [] } ] }"#;
        serde_json::from_str::<TimelineExpectation>(text).expect_err("clips is not a track field");
    }

    #[test]
    fn the_sidecar_of_a_fixture_replaces_its_extension() {
        assert_eq!(
            sidecar_for(&PathBuf::from("/edits/fixture/fixture.sub")),
            PathBuf::from("/edits/fixture/fixture.expect.json"),
        );
    }

    #[test]
    fn a_fixture_with_no_sidecar_expects_nothing() {
        let path = std::env::temp_dir().join("subordinate-expect-none.sub");
        let _ = std::fs::remove_file(sidecar_for(&path));
        assert!(
            TimelineExpectation::beside(&path)
                .expect("no sidecar is not an error")
                .is_none(),
        );
    }

    #[test]
    fn a_sidecar_that_is_not_an_expectation_names_itself() {
        let dir = std::env::temp_dir().join("subordinate-expect-bad");
        std::fs::create_dir_all(&dir).expect("a directory");
        let path = dir.join("fixture.sub");
        std::fs::write(sidecar_for(&path), "{ nonsense").expect("a sidecar");
        let error = TimelineExpectation::beside(&path).expect_err("not an expectation");
        assert_eq!(error.code.as_str(), "core.invalid_argument");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
