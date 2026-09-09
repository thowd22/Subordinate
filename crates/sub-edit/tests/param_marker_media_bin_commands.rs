//! Parameter, marker, media and bin commands, driven the way the Command API
//! will drive them: applied through the [`History`], undone, redone, and
//! checked against the bytes of the project file rather than against field
//! reads.
//!
//! Every command in this set is proved to have an exact inverse — undo lands
//! on the file the project had before it ran, redo lands back on the file it
//! had after — because that byte equality is what makes a `.sub` file survive
//! an edit/undo cycle with an empty git diff (docs/PLAN.md §5.6).

use sub_edit::commands::{
    AddMarker, CreateBin, Filing, ImportMedia, InsertBin, InsertMedia, MarkerTarget, MoveBin,
    MoveMarker, MoveToBin, RelinkMedia, RemoveBin, RemoveMarker, RemoveMedia, RenameBin,
    SetClipParams, SetTrackLocked, builtin_registry,
};
use sub_edit::{AnyCommand, Command, CommandEnvelope, History, codes};
use sub_model::{
    Bin, BinId, Clip, ClipId, ContentHash, Fixed6, GainDb, MarkerId, MediaId, MediaItem, MediaPath,
    Opacity, Point2, Project, Scale2, Sequence, SequenceId, SequenceSettings, Track, TrackId,
    TrackKind, Transform, codes as model_codes, json,
};
use sub_time::{Rational, RationalTime, TimeRange};

const RATE: Rational = Rational::FPS_24;

/// The identifiers the fixture hands back.
struct Fixture {
    sequence: SequenceId,
    track: TrackId,
    clip: ClipId,
    /// The media the clip is cut from, filed in the root bin.
    used: MediaId,
    /// A media item nothing uses, filed in the `Interviews` bin.
    spare: MediaId,
    /// The `Interviews` bin.
    bin: BinId,
}

/// A project with two media items, two bins and one 48-frame clip.
fn fixture() -> (Project, Fixture) {
    let mut project = Project::new("Doc cut");

    let used = MediaItem::new(MediaPath::new("footage/interview.mp4").unwrap());
    let used_id = used.id;
    project.media.push(used);
    project.root_bin.media.push(used_id);

    let spare = MediaItem::new(MediaPath::new("footage/broll.mp4").unwrap());
    let spare_id = spare.id;
    project.media.push(spare);

    let mut interviews = Bin::new("Interviews");
    let bin_id = interviews.id;
    interviews.media.push(spare_id);
    project.root_bin.children.push(interviews);

    let source = TimeRange::new(RationalTime::zero(RATE), RationalTime::new(48, RATE)).unwrap();
    let clip = Clip::new("shot 1", used_id, source);
    let clip_id = clip.id;
    let mut track = Track::new("V1", TrackKind::Video);
    track.items.push(clip.into());
    let track_id = track.id;

    let mut sequence = Sequence::new("Main", SequenceSettings::default());
    sequence.tracks.push(track);
    let sequence_id = sequence.id;
    project.sequences.push(sequence);

    (
        project,
        Fixture {
            sequence: sequence_id,
            track: track_id,
            clip: clip_id,
            used: used_id,
            spare: spare_id,
            bin: bin_id,
        },
    )
}

/// Applies `command`, then undoes and redoes it, asserting that the project
/// file is byte-identical before and after each hop.
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

/// Asserts that `command` leaves the project exactly as it found it, and
/// returns the error it produced.
fn refused<C: Command>(project: &mut Project, command: C) -> sub_core::SubError {
    let before = json::to_json(project).unwrap();
    let mut history = History::new();
    let err = history.apply(project, command).unwrap_err();
    assert_eq!(
        json::to_json(project).unwrap(),
        before,
        "a refused command changed the project"
    );
    err
}

/// The clip the fixture put on the track.
fn clip_of(project: &Project, at: &Fixture) -> Clip {
    project.sequences[0].tracks[0]
        .clip(at.clip)
        .unwrap()
        .clone()
}

// -- clip parameters ------------------------------------------------------

#[test]
fn every_clip_parameter_is_set_and_undone_exactly() {
    let (mut project, at) = fixture();
    let transform = Transform::new(
        Point2::new(Fixed6::from_units(2), Fixed6::from_units(-3)),
        Scale2::uniform(Fixed6::from_micros(500_000)).unwrap(),
        Fixed6::from_units(90),
    );

    round_trip(
        &mut project,
        SetClipParams::new(at.sequence, at.track, at.clip)
            .with_opacity(Opacity::from_f64(0.5).unwrap())
            .with_transform(transform)
            .with_gain(GainDb::new(Fixed6::from_units(-6)).unwrap())
            .with_fade_in(RationalTime::new(12, RATE))
            .with_fade_out(RationalTime::new(6, RATE)),
    );

    let clip = clip_of(&project, &at);
    assert_eq!(clip.opacity, Opacity::from_f64(0.5).unwrap());
    assert_eq!(clip.transform, transform);
    assert_eq!(clip.gain, GainDb::new(Fixed6::from_units(-6)).unwrap());
    assert_eq!(clip.fade_in, RationalTime::new(12, RATE));
    assert_eq!(clip.fade_out, RationalTime::new(6, RATE));
}

#[test]
fn an_inverse_puts_back_only_the_parameters_the_command_named() {
    let (mut project, at) = fixture();
    let mut history = History::new();

    history
        .apply(
            &mut project,
            SetClipParams::new(at.sequence, at.track, at.clip)
                .with_gain(GainDb::new(Fixed6::from_units(-6)).unwrap()),
        )
        .unwrap();
    history
        .apply(
            &mut project,
            SetClipParams::new(at.sequence, at.track, at.clip).with_opacity(Opacity::TRANSPARENT),
        )
        .unwrap();

    // Undoing the opacity edit must not restore the gain the earlier command
    // changed.
    history.undo(&mut project).unwrap();
    let clip = clip_of(&project, &at);
    assert_eq!(clip.opacity, Opacity::OPAQUE);
    assert_eq!(clip.gain, GainDb::new(Fixed6::from_units(-6)).unwrap());
}

#[test]
fn a_parameter_command_that_names_nothing_is_a_no_op_with_an_empty_inverse() {
    let (mut project, at) = fixture();
    let command = SetClipParams::new(at.sequence, at.track, at.clip);
    assert!(command.is_empty());

    let before = json::to_json(&project).unwrap();
    let inverse = command.apply(&mut project).unwrap();
    assert_eq!(json::to_json(&project).unwrap(), before);
    assert_eq!(inverse.command().kind(), "clip.set_params");
}

#[test]
fn parameters_are_validated_before_they_reach_the_clip() {
    let (mut project, at) = fixture();

    // Fades longer together than the clip: Clip::validate refuses them, and
    // the clip on the track is untouched.
    let err = refused(
        &mut project,
        SetClipParams::new(at.sequence, at.track, at.clip)
            .with_fade_in(RationalTime::new(30, RATE))
            .with_fade_out(RationalTime::new(30, RATE)),
    );
    assert_eq!(err.code, model_codes::INVALID_CLIP);
    assert_eq!(clip_of(&project, &at).fade_in, RationalTime::zero(RATE));

    let err = refused(
        &mut project,
        SetClipParams::new(at.sequence, at.track, at.clip)
            .with_fade_out(RationalTime::new(-1, RATE)),
    );
    assert_eq!(err.code, model_codes::INVALID_CLIP);
}

#[test]
fn parameters_name_a_clip_on_an_unlocked_track() {
    let (mut project, at) = fixture();

    let err = refused(
        &mut project,
        SetClipParams::new(at.sequence, at.track, ClipId::new()).with_opacity(Opacity::TRANSPARENT),
    );
    assert_eq!(err.code, codes::CLIP_NOT_FOUND);

    let err = refused(
        &mut project,
        SetClipParams::new(at.sequence, TrackId::new(), at.clip).with_opacity(Opacity::TRANSPARENT),
    );
    assert_eq!(err.code, codes::TRACK_NOT_FOUND);

    let mut history = History::new();
    history
        .apply(
            &mut project,
            SetTrackLocked::new(at.sequence, at.track, true),
        )
        .unwrap();
    let err = refused(
        &mut project,
        SetClipParams::new(at.sequence, at.track, at.clip).with_opacity(Opacity::TRANSPARENT),
    );
    assert_eq!(err.code, codes::TRACK_LOCKED);
}

// -- markers --------------------------------------------------------------

#[test]
fn sequence_markers_are_added_moved_and_removed_exactly() {
    let (mut project, at) = fixture();
    let target = MarkerTarget::sequence(at.sequence);
    let marker =
        sub_model::Marker::new("cut here", TimeRange::empty_at(RationalTime::new(24, RATE)));
    let marker_id = marker.id;

    round_trip(&mut project, AddMarker::new(target, marker));
    assert_eq!(project.sequences[0].markers.len(), 1);

    let moved = TimeRange::new(RationalTime::new(36, RATE), RationalTime::new(12, RATE)).unwrap();
    round_trip(&mut project, MoveMarker::new(target, marker_id, moved));
    assert_eq!(
        project.sequences[0].marker(marker_id).unwrap().marked_range,
        moved
    );

    round_trip(&mut project, RemoveMarker::new(target, marker_id));
    assert!(project.sequences[0].markers.is_empty());
}

#[test]
fn a_removed_marker_comes_back_at_the_index_it_sat_at() {
    let (mut project, at) = fixture();
    let target = MarkerTarget::sequence(at.sequence);
    let mut history = History::new();
    for name in ["one", "two", "three"] {
        history
            .apply(
                &mut project,
                AddMarker::new(
                    target,
                    sub_model::Marker::new(name, TimeRange::empty_at(RationalTime::zero(RATE))),
                ),
            )
            .unwrap();
    }
    let middle = project.sequences[0].markers[1].id;

    round_trip(&mut project, RemoveMarker::new(target, middle));
    let names: Vec<&str> = project.sequences[0]
        .markers
        .iter()
        .map(|marker| marker.name.as_str())
        .collect();
    assert_eq!(names, ["one", "three"]);
}

#[test]
fn clip_markers_use_the_same_commands_and_respect_the_track_lock() {
    let (mut project, at) = fixture();
    let target = MarkerTarget::clip(at.sequence, at.track, at.clip);
    let marker = sub_model::Marker::new("sync", TimeRange::empty_at(RationalTime::new(6, RATE)));
    let marker_id = marker.id;

    round_trip(&mut project, AddMarker::new(target, marker.clone()).at(0));
    assert_eq!(clip_of(&project, &at).markers.len(), 1);

    let mut history = History::new();
    history
        .apply(
            &mut project,
            SetTrackLocked::new(at.sequence, at.track, true),
        )
        .unwrap();
    let err = refused(&mut project, RemoveMarker::new(target, marker_id));
    assert_eq!(err.code, codes::TRACK_LOCKED);
}

#[test]
fn marker_commands_name_a_marker_the_target_holds() {
    let (mut project, at) = fixture();
    let target = MarkerTarget::sequence(at.sequence);
    let empty = TimeRange::empty_at(RationalTime::zero(RATE));

    let err = refused(&mut project, RemoveMarker::new(target, MarkerId::new()));
    assert_eq!(err.code, codes::MARKER_NOT_FOUND);

    let err = refused(
        &mut project,
        MoveMarker::new(target, MarkerId::new(), empty),
    );
    assert_eq!(err.code, codes::MARKER_NOT_FOUND);

    let marker = sub_model::Marker::new("one", empty);
    let mut history = History::new();
    history
        .apply(&mut project, AddMarker::new(target, marker.clone()))
        .unwrap();

    let err = refused(&mut project, AddMarker::new(target, marker.clone()));
    assert_eq!(err.code, codes::DUPLICATE_MARKER);

    let err = refused(
        &mut project,
        AddMarker::new(target, sub_model::Marker::new("two", empty)).at(9),
    );
    assert_eq!(err.code, codes::INVALID_INDEX);

    let err = refused(
        &mut project,
        AddMarker::new(MarkerTarget::sequence(SequenceId::new()), marker),
    );
    assert_eq!(err.code, codes::SEQUENCE_NOT_FOUND);
}

// -- media ----------------------------------------------------------------

#[test]
fn media_is_imported_into_a_bin_and_undone_exactly() {
    let (mut project, at) = fixture();
    let item = MediaItem::new(MediaPath::new("footage/room-tone.wav").unwrap());
    let id = item.id;

    round_trip(
        &mut project,
        ImportMedia::new(item.clone()).into_bin(at.bin),
    );
    assert_eq!(project.bin_of(id), Some(at.bin));
    assert_eq!(project.media.len(), 3);

    let err = refused(&mut project, ImportMedia::new(item.clone()));
    assert_eq!(err.code, codes::DUPLICATE_MEDIA);

    let err = refused(
        &mut project,
        ImportMedia::new(MediaItem::new(MediaPath::new("x.mp4").unwrap())).into_bin(BinId::new()),
    );
    assert_eq!(err.code, codes::BIN_NOT_FOUND);
}

#[test]
fn media_in_use_is_removed_only_when_the_caller_forces_it() {
    let (mut project, at) = fixture();

    let err = refused(&mut project, RemoveMedia::new(at.used));
    assert_eq!(err.code, codes::MEDIA_IN_USE);
    assert_eq!(err.details.get("clips"), Some(&serde_json::json!(1)));

    // Unused media goes without a fight, and comes back in its own bin at the
    // position it held.
    round_trip(&mut project, RemoveMedia::new(at.spare));
    assert!(project.media_item(at.spare).is_none());
    assert_eq!(project.root_bin.children[0].media.len(), 0);

    round_trip(&mut project, RemoveMedia::forced(at.used));
    assert!(project.media_item(at.used).is_none());

    let err = refused(&mut project, RemoveMedia::new(MediaId::new()));
    assert_eq!(err.code, codes::MEDIA_NOT_FOUND);
}

#[test]
fn an_unfiled_media_item_is_restored_unfiled() {
    let (mut project, at) = fixture();
    project.root_bin.children[0].media.clear();
    assert_eq!(project.bin_of(at.spare), None);

    round_trip(&mut project, RemoveMedia::new(at.spare));
    assert_eq!(project.bin_of(at.spare), None);
    assert_eq!(project.media.len(), 1);
}

#[test]
fn a_restore_command_refuses_an_identifier_the_project_already_holds() {
    let (mut project, at) = fixture();
    let item = project.media_item(at.spare).unwrap().clone();

    let err = refused(
        &mut project,
        InsertMedia::new(0, item.clone(), Some(Filing::new(at.bin, 0))),
    );
    assert_eq!(err.code, codes::DUPLICATE_MEDIA);

    let mut history = History::new();
    history
        .apply(&mut project, RemoveMedia::new(at.spare))
        .unwrap();

    let err = refused(&mut project, InsertMedia::new(9, item.clone(), None));
    assert_eq!(err.code, codes::INVALID_INDEX);

    round_trip(
        &mut project,
        InsertMedia::new(0, item, Some(Filing::new(at.bin, 0))),
    );
    assert_eq!(project.media[0].id, at.spare);
    assert_eq!(project.bin_of(at.spare), Some(at.bin));
}

#[test]
fn relinking_keeps_the_identity_and_undoes_exactly() {
    let (mut project, at) = fixture();
    project.media_item_mut(at.used).unwrap().offline = true;
    let hash = ContentHash::from_bytes([7; 32]);

    round_trip(
        &mut project,
        RelinkMedia::new(at.used, MediaPath::new("moved/interview.mp4").unwrap()).with_hash(hash),
    );

    let item = project.media_item(at.used).unwrap();
    assert_eq!(item.path.as_str(), "moved/interview.mp4");
    assert_eq!(item.hash, Some(hash));
    assert!(!item.offline);
    // The clip cut from it is untouched.
    assert_eq!(clip_of(&project, &at).media, at.used);

    round_trip(
        &mut project,
        RelinkMedia::new(at.used, MediaPath::new("gone/interview.mp4").unwrap()).offline(),
    );
    assert!(project.media_item(at.used).unwrap().offline);

    let err = refused(
        &mut project,
        RelinkMedia::new(MediaId::new(), MediaPath::new("a.mp4").unwrap()),
    );
    assert_eq!(err.code, codes::MEDIA_NOT_FOUND);
}

// -- bins -----------------------------------------------------------------

#[test]
fn bins_are_created_renamed_and_removed_exactly() {
    let (mut project, at) = fixture();

    round_trip(&mut project, CreateBin::new("B-roll").inside(at.bin).at(0));
    let created = project.root_bin.children[0].children[0].id;
    assert_eq!(project.root_bin.find(created).unwrap().name, "B-roll");

    round_trip(&mut project, RenameBin::new(created, "Cutaways"));
    assert_eq!(project.root_bin.find(created).unwrap().name, "Cutaways");

    round_trip(&mut project, RemoveBin::new(created));
    assert!(project.root_bin.find(created).is_none());

    let root = project.root_bin.id;
    round_trip(&mut project, RenameBin::new(root, "Archive"));
    assert_eq!(project.root_bin.name, "Archive");
}

#[test]
fn a_bin_holding_anything_is_removed_only_when_forced() {
    let (mut project, at) = fixture();

    let err = refused(&mut project, RemoveBin::new(at.bin));
    assert_eq!(err.code, codes::BIN_NOT_EMPTY);
    assert_eq!(err.details.get("media"), Some(&serde_json::json!(1)));

    // Forcing it takes the whole subtree, media filing included, and undo
    // brings every identifier back.
    round_trip(&mut project, RemoveBin::forced(at.bin));
    assert!(project.root_bin.find(at.bin).is_none());
    assert!(project.media_item(at.spare).is_some());
    assert_eq!(project.bin_of(at.spare), None);
}

#[test]
fn the_root_bin_is_not_removable_and_bins_are_named_by_identifier() {
    let (mut project, at) = fixture();
    let root = project.root_bin.id;

    let err = refused(&mut project, RemoveBin::forced(root));
    assert_eq!(err.code, codes::ROOT_BIN);

    let err = refused(&mut project, RemoveBin::new(BinId::new()));
    assert_eq!(err.code, codes::BIN_NOT_FOUND);

    let err = refused(&mut project, RenameBin::new(BinId::new(), "nope"));
    assert_eq!(err.code, codes::BIN_NOT_FOUND);

    let err = refused(&mut project, CreateBin::new("nope").inside(BinId::new()));
    assert_eq!(err.code, codes::BIN_NOT_FOUND);

    let err = refused(&mut project, CreateBin::new("nope").inside(at.bin).at(4));
    assert_eq!(err.code, codes::INVALID_INDEX);
}

#[test]
fn restoring_a_bin_refuses_an_identifier_the_tree_already_holds() {
    let (mut project, at) = fixture();
    let existing = project.root_bin.find(at.bin).unwrap().clone();
    let root = project.root_bin.id;

    let err = refused(&mut project, InsertBin::new(root, 0, existing.clone()));
    assert_eq!(err.code, codes::DUPLICATE_BIN);

    let err = refused(&mut project, InsertBin::new(root, 7, Bin::new("Fresh")));
    assert_eq!(err.code, codes::INVALID_INDEX);

    let mut history = History::new();
    history
        .apply(&mut project, RemoveBin::forced(at.bin))
        .unwrap();
    round_trip(&mut project, InsertBin::new(root, 0, existing));
    assert!(project.root_bin.find(at.bin).is_some());
}

#[test]
fn media_moves_between_bins_and_back_to_the_index_it_held() {
    let (mut project, at) = fixture();

    round_trip(&mut project, MoveToBin::new(at.used, at.bin).at(0));
    assert_eq!(project.bin_of(at.used), Some(at.bin));
    assert_eq!(project.root_bin.children[0].media[0], at.used);
    assert!(project.root_bin.media.is_empty());

    // Reordering inside one bin is the same command.
    round_trip(&mut project, MoveToBin::new(at.spare, at.bin).at(0));
    assert_eq!(project.root_bin.children[0].media[0], at.spare);

    let err = refused(&mut project, MoveToBin::new(at.spare, BinId::new()));
    assert_eq!(err.code, codes::BIN_NOT_FOUND);

    let err = refused(&mut project, MoveToBin::new(MediaId::new(), at.bin));
    assert_eq!(err.code, codes::MEDIA_NOT_FOUND);

    let err = refused(&mut project, MoveToBin::new(at.spare, at.bin).at(5));
    assert_eq!(err.code, codes::INVALID_INDEX);
}

#[test]
fn an_unfiled_media_item_cannot_be_moved() {
    let (mut project, at) = fixture();
    project.root_bin.children[0].media.clear();

    let err = refused(&mut project, MoveToBin::new(at.spare, at.bin));
    assert_eq!(err.code, codes::BIN_NOT_FOUND);
}

#[test]
fn a_bin_is_reparented_with_everything_inside_it_and_put_back_exactly() {
    let (mut project, at) = fixture();
    let mut history = History::new();
    history
        .apply(&mut project, CreateBin::new("B-roll"))
        .unwrap();
    let broll = project.root_bin.children[1].id;

    // The `Interviews` bin, media and all, moves inside `B-roll`.
    round_trip(&mut project, MoveBin::new(at.bin, broll));
    let moved = project.root_bin.find(at.bin).unwrap();
    assert_eq!(moved.media, vec![at.spare]);
    assert_eq!(project.bin_of(at.spare), Some(at.bin));
    assert_eq!(project.root_bin.children.len(), 1);
    assert_eq!(project.root_bin.children[0].id, broll);

    // And back out to a chosen position in the root.
    let root = project.root_bin.id;
    round_trip(&mut project, MoveBin::new(at.bin, root).at(0));
    assert_eq!(project.root_bin.children[0].id, at.bin);
    assert_eq!(project.root_bin.children[1].id, broll);
}

#[test]
fn a_bin_cannot_be_moved_into_its_own_subtree_or_out_of_the_root() {
    let (mut project, at) = fixture();
    let root = project.root_bin.id;
    let mut history = History::new();
    history
        .apply(&mut project, CreateBin::new("Takes").inside(at.bin))
        .unwrap();
    let takes = project.root_bin.find(at.bin).unwrap().children[0].id;

    let err = refused(&mut project, MoveBin::new(root, at.bin));
    assert_eq!(err.code, codes::ROOT_BIN);

    let err = refused(&mut project, MoveBin::new(at.bin, at.bin));
    assert_eq!(err.code, codes::INVALID_INDEX);

    let err = refused(&mut project, MoveBin::new(at.bin, takes));
    assert_eq!(err.code, codes::INVALID_INDEX);

    let err = refused(&mut project, MoveBin::new(BinId::new(), root));
    assert_eq!(err.code, codes::BIN_NOT_FOUND);

    let err = refused(&mut project, MoveBin::new(at.bin, BinId::new()));
    assert_eq!(err.code, codes::BIN_NOT_FOUND);

    let err = refused(&mut project, MoveBin::new(at.bin, root).at(4));
    assert_eq!(err.code, codes::INVALID_INDEX);
}

// -- the wire -------------------------------------------------------------

#[test]
fn every_new_kind_decodes_from_its_envelope() {
    let (mut project, at) = fixture();
    let registry = builtin_registry().unwrap();
    for kind in [
        "bin.create",
        "bin.insert",
        "bin.move",
        "bin.move_media",
        "bin.remove",
        "bin.rename",
        "clip.set_params",
        "marker.add",
        "marker.move",
        "marker.remove",
        "media.import",
        "media.insert",
        "media.relink",
        "media.remove",
    ] {
        assert!(registry.contains(kind), "{kind} is not registered");
    }

    let envelope = SetClipParams::new(at.sequence, at.track, at.clip)
        .with_opacity(Opacity::TRANSPARENT)
        .to_envelope()
        .unwrap();
    assert_eq!(envelope.kind, "clip.set_params");
    let decoded = registry.decode(&envelope).unwrap();
    decoded.apply_erased(&mut project).unwrap();
    assert!(clip_of(&project, &at).opacity.is_transparent());

    // A marker target is a tagged union on the wire.
    let target = MarkerTarget::clip(at.sequence, at.track, at.clip);
    let value = serde_json::to_value(target).unwrap();
    assert_eq!(value["on"], serde_json::json!("clip"));
    assert_eq!(
        serde_json::to_value(MarkerTarget::sequence(at.sequence)).unwrap()["on"],
        serde_json::json!("sequence")
    );

    let bad = CommandEnvelope::new("bin.rename", serde_json::json!({ "bin": 7 }));
    assert_eq!(
        registry.decode(&bad).unwrap_err().code,
        codes::INVALID_COMMAND
    );
}

#[test]
fn commands_carry_labels_for_the_undo_menu() {
    let (project, at) = fixture();
    let item = project.media_item(at.spare).unwrap().clone();
    let empty = TimeRange::empty_at(RationalTime::zero(RATE));

    assert_eq!(
        SetClipParams::new(at.sequence, at.track, at.clip).label(),
        "Change clip parameters"
    );
    assert_eq!(
        AddMarker::new(
            MarkerTarget::sequence(at.sequence),
            sub_model::Marker::new("cut", empty)
        )
        .label(),
        "Add marker cut"
    );
    assert_eq!(
        RemoveMarker::new(MarkerTarget::sequence(at.sequence), MarkerId::new()).label(),
        "Remove marker"
    );
    assert_eq!(
        MoveMarker::new(MarkerTarget::sequence(at.sequence), MarkerId::new(), empty).label(),
        "Move marker"
    );
    assert_eq!(ImportMedia::new(item.clone()).label(), "Import broll.mp4");
    assert_eq!(InsertMedia::new(0, item, None).label(), "Restore broll.mp4");
    assert_eq!(RemoveMedia::new(at.spare).label(), "Remove media");
    assert_eq!(
        RelinkMedia::new(at.spare, MediaPath::new("b.mp4").unwrap()).label(),
        "Relink to b.mp4"
    );
    assert_eq!(CreateBin::new("B-roll").label(), "Create bin B-roll");
    assert_eq!(
        InsertBin::new(at.bin, 0, Bin::new("B-roll")).label(),
        "Restore bin B-roll"
    );
    assert_eq!(RemoveBin::new(at.bin).label(), "Remove bin");
    assert_eq!(
        RenameBin::new(at.bin, "Cutaways").label(),
        "Rename bin to Cutaways"
    );
    assert_eq!(
        MoveToBin::new(at.spare, at.bin).label(),
        "Move media to bin"
    );
}
