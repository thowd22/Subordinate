//! Track and sequence commands, exercised the way the Command API will drive
//! them: applied through the [`History`], undone, redone, and checked against
//! the bytes of the project file rather than against field reads.
//!
//! Every command in the set is proved to have an exact inverse — undo lands on
//! the file the project had before it ran, redo lands back on the file it had
//! after — because that byte equality is what makes a `.sub` file survive an
//! edit/undo cycle with an empty git diff (docs/PLAN.md §5.6).

use serde::{Deserialize, Serialize};
use sub_core::{SubResult, codes as core_codes};
use sub_edit::commands::{
    AddTrack, CreateSequence, DeleteSequence, InsertSequence, InsertTrack, RemoveTrack,
    RenameSequence, RenameTrack, ReorderTrack, SetSequenceSettings, SetTrackLocked, SetTrackMuted,
    builtin_registry, track_for_clip_edit,
};
use sub_edit::{AnyCommand, Command, CommandEnvelope, History, Inverse, codes};
use sub_model::{
    Clip, ColorTags, MediaId, MediaItem, MediaPath, Project, Resolution, Sequence, SequenceId,
    SequenceSettings, Track, TrackId, TrackKind, json,
};
use sub_time::{Rational, RationalTime, TimeRange};

/// A project with one sequence, two video tracks and one audio track. The
/// first video track holds a clip; the others are empty.
fn fixture() -> (Project, SequenceId, Vec<TrackId>, MediaId) {
    let rate = Rational::FPS_24;
    let mut project = Project::new("Doc cut");

    let media = MediaItem::new(MediaPath::new("footage/interview.mp4").unwrap());
    let media_id = media.id;
    project.media.push(media);

    let source = TimeRange::new(RationalTime::zero(rate), RationalTime::new(48, rate)).unwrap();
    let mut v1 = Track::new("V1", TrackKind::Video);
    v1.items.push(Clip::new("shot 1", media_id, source).into());
    let v2 = Track::new("V2", TrackKind::Video);
    let a1 = Track::new("A1", TrackKind::Audio);
    let ids = vec![v1.id, v2.id, a1.id];

    let mut sequence = Sequence::new("Main", SequenceSettings::default());
    sequence.tracks.push(v1);
    sequence.tracks.push(v2);
    sequence.tracks.push(a1);
    let sequence_id = sequence.id;
    project.sequences.push(sequence);

    (project, sequence_id, ids, media_id)
}

/// Applies `command`, then undoes and redoes it, asserting that the project
/// file is byte-identical before and after each hop.
///
/// Returns the file text the command produced, so a caller can also assert on
/// what it changed.
fn round_trip<C: Command>(project: &mut Project, command: C) -> String {
    let before = json::to_json(project).unwrap();
    let mut history = History::new();
    history.apply(project, command).unwrap();
    let after = json::to_json(project).unwrap();
    assert_ne!(after, before, "the command changed nothing");

    history.undo(project).unwrap();
    assert_eq!(
        json::to_json(project).unwrap(),
        before,
        "undo was not exact"
    );

    history.redo(project).unwrap();
    assert_eq!(json::to_json(project).unwrap(), after, "redo was not exact");
    after
}

#[test]
fn a_new_track_is_appended_and_undone_exactly() {
    let (mut project, sequence, _, _) = fixture();
    round_trip(
        &mut project,
        AddTrack::new(sequence, "A2", TrackKind::Audio),
    );
    let tracks = &project.sequences[0].tracks;
    assert_eq!(tracks.len(), 4);
    assert_eq!(tracks[3].name, "A2");
    assert_eq!(tracks[3].kind, TrackKind::Audio);
    assert!(!tracks[3].muted);
    assert!(!tracks[3].locked);
}

#[test]
fn a_new_track_may_name_its_position() {
    let (mut project, sequence, _, _) = fixture();
    round_trip(
        &mut project,
        AddTrack::new(sequence, "V0", TrackKind::Video).at(0),
    );
    assert_eq!(project.sequences[0].tracks[0].name, "V0");

    let err = AddTrack::new(sequence, "nope", TrackKind::Video)
        .at(99)
        .apply(&mut project)
        .unwrap_err();
    assert_eq!(err.code, codes::INVALID_INDEX);
    assert_eq!(project.sequences[0].tracks.len(), 4, "nothing was inserted");
}

#[test]
fn redoing_an_added_track_keeps_its_identifier() {
    let (mut project, sequence, _, _) = fixture();
    let mut history = History::new();
    history
        .apply(
            &mut project,
            AddTrack::new(sequence, "A2", TrackKind::Audio),
        )
        .unwrap();
    let id = project.sequences[0].tracks[3].id;

    history.undo(&mut project).unwrap();
    history.redo(&mut project).unwrap();
    assert_eq!(project.sequences[0].tracks[3].id, id);
}

#[test]
fn an_empty_track_is_removed_but_one_holding_clips_needs_force() {
    let (mut project, sequence, tracks, _) = fixture();

    // The empty V2 goes without argument.
    round_trip(&mut project, RemoveTrack::new(sequence, tracks[1]));
    assert_eq!(project.sequences[0].tracks.len(), 2);

    // V1 still holds a clip, so an unforced removal is refused and changes
    // nothing.
    let before = json::to_json(&project).unwrap();
    let err = RemoveTrack::new(sequence, tracks[0])
        .apply(&mut project)
        .unwrap_err();
    assert_eq!(err.code, codes::TRACK_NOT_EMPTY);
    assert_eq!(err.details.get("clips"), Some(&serde_json::json!(1)));
    assert_eq!(json::to_json(&project).unwrap(), before);
}

#[test]
fn a_forced_removal_takes_the_clips_with_it_and_undoes_them_back() {
    let (mut project, sequence, tracks, _) = fixture();
    let text = round_trip(&mut project, RemoveTrack::forced(sequence, tracks[0]));
    assert!(!text.contains("shot 1"), "the clip went with the track");

    // The undo inside round_trip already proved the clip comes back; check the
    // restored track sits at the index it left from.
    let mut history = History::new();
    history
        .apply(
            &mut project,
            InsertTrack::new(sequence, 0, {
                let mut track = Track::new("V1 again", TrackKind::Video);
                track.locked = true;
                track
            }),
        )
        .unwrap();
    assert_eq!(project.sequences[0].tracks[0].name, "V1 again");
    history.undo(&mut project).unwrap();
    assert_eq!(project.sequences[0].tracks[0].name, "V2");
}

#[test]
fn inserting_a_track_twice_is_refused() {
    let (mut project, sequence, tracks, _) = fixture();
    let existing = project.sequences[0].tracks[1].clone();
    assert_eq!(existing.id, tracks[1]);

    let err = InsertTrack::new(sequence, 0, existing)
        .apply(&mut project)
        .unwrap_err();
    assert_eq!(err.code, codes::DUPLICATE_TRACK);
    assert_eq!(project.sequences[0].tracks.len(), 3);
}

#[test]
fn a_track_moves_and_moves_back() {
    let (mut project, sequence, tracks, _) = fixture();
    round_trip(&mut project, ReorderTrack::new(sequence, tracks[2], 0));
    let names: Vec<&str> = project.sequences[0]
        .tracks
        .iter()
        .map(|track| track.name.as_str())
        .collect();
    assert_eq!(names, ["A1", "V1", "V2"]);

    let err = ReorderTrack::new(sequence, tracks[2], 3)
        .apply(&mut project)
        .unwrap_err();
    assert_eq!(err.code, codes::INVALID_INDEX);
}

#[test]
fn a_track_is_renamed_muted_and_locked_reversibly() {
    let (mut project, sequence, tracks, _) = fixture();

    round_trip(&mut project, RenameTrack::new(sequence, tracks[0], "Bed"));
    assert_eq!(project.sequences[0].tracks[0].name, "Bed");

    round_trip(&mut project, SetTrackMuted::new(sequence, tracks[2], true));
    assert!(project.sequences[0].tracks[2].muted);

    round_trip(&mut project, SetTrackLocked::new(sequence, tracks[0], true));
    assert!(project.sequences[0].tracks[0].locked);
}

/// A stand-in for the clip commands of TASK-4.2: all it does is go through
/// [`track_for_clip_edit`], which is the lock check every one of them shares.
#[derive(Debug, Clone, schemars::JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppendGap {
    sequence: SequenceId,
    track: TrackId,
}

impl Command for AppendGap {
    const KIND: &'static str = "test.append_gap";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let track = track_for_clip_edit(project, self.sequence, self.track)?;
        track
            .items
            .push(sub_model::Gap::new(RationalTime::new(24, Rational::FPS_24)).into());
        Ok(Inverse::new(self.clone()))
    }
}

#[test]
fn a_locked_track_rejects_clip_commands_with_a_structured_error() {
    let (mut project, sequence, tracks, _) = fixture();
    let mut history = History::new();

    history
        .apply(
            &mut project,
            AppendGap {
                sequence,
                track: tracks[0],
            },
        )
        .unwrap();
    assert_eq!(project.sequences[0].tracks[0].items.len(), 2);

    history
        .apply(&mut project, SetTrackLocked::new(sequence, tracks[0], true))
        .unwrap();

    let before = json::to_json(&project).unwrap();
    let err = AppendGap {
        sequence,
        track: tracks[0],
    }
    .apply(&mut project)
    .unwrap_err();
    assert_eq!(err.code, codes::TRACK_LOCKED);
    assert_eq!(err.code.domain(), "edit");
    assert_eq!(
        err.details.get("track_id"),
        Some(&serde_json::json!(tracks[0].to_string()))
    );
    assert_eq!(
        err.details.get("track_name"),
        Some(&serde_json::json!("V1"))
    );
    assert_eq!(
        json::to_json(&project).unwrap(),
        before,
        "a refused command left the project alone"
    );

    // Unlocking is itself a command, and it opens the track again.
    history
        .apply(
            &mut project,
            SetTrackLocked::new(sequence, tracks[0], false),
        )
        .unwrap();
    assert!(
        AppendGap {
            sequence,
            track: tracks[0]
        }
        .apply(&mut project)
        .is_ok()
    );
}

#[test]
fn an_unknown_sequence_or_track_is_named_in_the_error() {
    let (mut project, sequence, tracks, _) = fixture();

    let err = RenameTrack::new(SequenceId::new(), tracks[0], "x")
        .apply(&mut project)
        .unwrap_err();
    assert_eq!(err.code, codes::SEQUENCE_NOT_FOUND);

    let err = SetTrackMuted::new(sequence, TrackId::new(), true)
        .apply(&mut project)
        .unwrap_err();
    assert_eq!(err.code, codes::TRACK_NOT_FOUND);

    let err = RemoveTrack::new(sequence, TrackId::new())
        .apply(&mut project)
        .unwrap_err();
    assert_eq!(err.code, codes::TRACK_NOT_FOUND);

    let err = ReorderTrack::new(sequence, TrackId::new(), 0)
        .apply(&mut project)
        .unwrap_err();
    assert_eq!(err.code, codes::TRACK_NOT_FOUND);

    let err = DeleteSequence::new(SequenceId::new())
        .apply(&mut project)
        .unwrap_err();
    assert_eq!(err.code, codes::SEQUENCE_NOT_FOUND);
}

#[test]
fn a_sequence_is_created_renamed_reconfigured_and_deleted_reversibly() {
    let (mut project, sequence, _, _) = fixture();

    round_trip(
        &mut project,
        CreateSequence::new("Titles", SequenceSettings::default()).at(0),
    );
    assert_eq!(project.sequences[0].name, "Titles");
    assert_eq!(project.sequences.len(), 2);

    round_trip(&mut project, RenameSequence::new(sequence, "Programme"));
    assert_eq!(project.sequences[1].name, "Programme");

    let settings = SequenceSettings::new(
        Resolution::UHD_2160,
        Rational::FPS_25,
        44_100,
        ColorTags::REC709,
    )
    .unwrap();
    round_trip(&mut project, SetSequenceSettings::new(sequence, settings));
    assert_eq!(project.sequences[1].settings, settings);

    // Deleting takes the tracks and their clips with it, and undo brings them
    // all back byte for byte.
    let text = round_trip(&mut project, DeleteSequence::new(sequence));
    assert!(!text.contains("shot 1"));
    assert_eq!(project.sequences.len(), 1);
}

#[test]
fn a_deleted_sequence_returns_to_its_tab_position() {
    let (mut project, sequence, _, _) = fixture();
    CreateSequence::new("Titles", SequenceSettings::default())
        .at(0)
        .apply(&mut project)
        .unwrap();
    assert_eq!(project.sequence_index(sequence), Some(1));

    let mut history = History::new();
    history
        .apply(&mut project, DeleteSequence::new(sequence))
        .unwrap();
    history.undo(&mut project).unwrap();
    assert_eq!(project.sequence_index(sequence), Some(1));

    let duplicate = project.sequences[1].clone();
    let err = InsertSequence::new(0, duplicate)
        .apply(&mut project)
        .unwrap_err();
    assert_eq!(err.code, codes::DUPLICATE_SEQUENCE);

    let err = InsertSequence::new(9, Sequence::new("x", SequenceSettings::default()))
        .apply(&mut project)
        .unwrap_err();
    assert_eq!(err.code, codes::INVALID_INDEX);
}

#[test]
fn creating_a_sequence_past_the_end_is_refused() {
    let (mut project, _, _, _) = fixture();
    let err = CreateSequence::new("Nope", SequenceSettings::default())
        .at(4)
        .apply(&mut project)
        .unwrap_err();
    assert_eq!(err.code, codes::INVALID_INDEX);
    assert_eq!(project.sequences.len(), 1);
}

#[test]
fn every_command_travels_as_an_envelope_and_comes_back() {
    let (mut project, sequence, tracks, _) = fixture();
    let registry = builtin_registry().unwrap();

    let envelopes = vec![
        AddTrack::new(sequence, "A2", TrackKind::Audio).to_envelope(),
        RemoveTrack::forced(sequence, tracks[1]).to_envelope(),
        ReorderTrack::new(sequence, tracks[2], 0).to_envelope(),
        RenameTrack::new(sequence, tracks[0], "Bed").to_envelope(),
        SetTrackMuted::new(sequence, tracks[0], true).to_envelope(),
        SetTrackLocked::new(sequence, tracks[0], true).to_envelope(),
        CreateSequence::new("Titles", SequenceSettings::default()).to_envelope(),
        RenameSequence::new(sequence, "Programme").to_envelope(),
        SetSequenceSettings::new(sequence, SequenceSettings::default()).to_envelope(),
        DeleteSequence::new(sequence).to_envelope(),
    ];

    let mut history = History::new();
    for envelope in &envelopes {
        let envelope = envelope.as_ref().unwrap();
        assert!(registry.contains(&envelope.kind), "{}", envelope.kind);
        let command = registry.decode(envelope).unwrap();
        history.apply_boxed(&mut project, command).unwrap();
    }

    while history.undo(&mut project).unwrap().is_some() {}
    assert_eq!(project.sequences.len(), 1);
    assert_eq!(project.sequences[0].tracks.len(), 3);
}

#[test]
fn the_wire_shape_of_a_command_is_stable() {
    let sequence = SequenceId::parse("018f0000-0000-7000-8000-000000000001").unwrap();
    let track = TrackId::parse("018f0000-0000-7000-8000-000000000002").unwrap();

    let envelope = SetTrackLocked::new(sequence, track, true)
        .to_envelope()
        .unwrap();
    assert_eq!(
        serde_json::to_string(&envelope).unwrap(),
        "{\"kind\":\"track.set_locked\",\"params\":{\"locked\":true,\"sequence\":\
         \"018f0000-0000-7000-8000-000000000001\",\"track\":\
         \"018f0000-0000-7000-8000-000000000002\"}}"
    );

    // An appended track omits `index` rather than writing a null.
    let envelope = AddTrack::new(sequence, "V1", TrackKind::Video)
        .to_envelope()
        .unwrap();
    assert_eq!(
        envelope.params,
        serde_json::json!({
            "sequence": "018f0000-0000-7000-8000-000000000001",
            "name": "V1",
            "kind": "video",
        })
    );

    // Unknown parameters are refused rather than silently ignored.
    let registry = builtin_registry().unwrap();
    let bad = CommandEnvelope::new(
        "track.rename",
        serde_json::json!({ "sequence": sequence, "track": track, "nmae": "V2" }),
    );
    assert_eq!(
        registry.decode(&bad).unwrap_err().code,
        codes::INVALID_COMMAND
    );
}

#[test]
fn a_removal_and_its_restore_group_into_one_undo_step() {
    let (mut project, sequence, tracks, _) = fixture();
    let before = json::to_json(&project).unwrap();

    let mut history = History::new();
    history.begin_group("Clear the video tracks").unwrap();
    history
        .apply(&mut project, RemoveTrack::forced(sequence, tracks[0]))
        .unwrap();
    history
        .apply(&mut project, RemoveTrack::forced(sequence, tracks[1]))
        .unwrap();
    assert!(history.commit_group().unwrap());

    assert_eq!(history.undo_len(), 1);
    assert_eq!(history.undo_label(), Some("Clear the video tracks"));
    assert_eq!(project.sequences[0].tracks.len(), 1);

    history.undo(&mut project).unwrap();
    assert_eq!(json::to_json(&project).unwrap(), before);
}

#[test]
fn labels_read_like_menu_entries() {
    let sequence = SequenceId::new();
    let track = TrackId::new();
    assert_eq!(
        AddTrack::new(sequence, "V2", TrackKind::Video).label(),
        "Add track V2"
    );
    assert_eq!(RemoveTrack::new(sequence, track).label(), "Remove track");
    assert_eq!(
        ReorderTrack::new(sequence, track, 0).label(),
        "Reorder track"
    );
    assert_eq!(
        RenameTrack::new(sequence, track, "Bed").label(),
        "Rename track to Bed"
    );
    assert_eq!(
        SetTrackMuted::new(sequence, track, true).label(),
        "Mute track"
    );
    assert_eq!(
        SetTrackMuted::new(sequence, track, false).label(),
        "Unmute track"
    );
    assert_eq!(
        SetTrackLocked::new(sequence, track, true).label(),
        "Lock track"
    );
    assert_eq!(
        SetTrackLocked::new(sequence, track, false).label(),
        "Unlock track"
    );
    assert_eq!(
        CreateSequence::new("Titles", SequenceSettings::default()).label(),
        "Create sequence Titles"
    );
    assert_eq!(DeleteSequence::new(sequence).label(), "Delete sequence");
    assert_eq!(
        RenameSequence::new(sequence, "Main").label(),
        "Rename sequence to Main"
    );
    assert_eq!(
        SetSequenceSettings::new(sequence, SequenceSettings::default()).label(),
        "Change sequence settings"
    );
    assert_eq!(
        InsertTrack::new(sequence, 0, Track::new("V1", TrackKind::Video)).label(),
        "Restore track V1"
    );
    assert_eq!(
        InsertSequence::new(0, Sequence::new("Main", SequenceSettings::default())).label(),
        "Restore sequence Main"
    );
    // A code from `sub-core` is still available to callers of these commands.
    assert_eq!(core_codes::INVALID_ARGUMENT.domain(), "core");
}
