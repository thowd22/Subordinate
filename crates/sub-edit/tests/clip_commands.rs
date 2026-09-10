//! The clip commands: placement, the overwrite overlap policy, the errors at
//! clip boundaries, and undo and redo for each of them.
//!
//! Every test that mutates goes through [`History`], so every assertion about
//! a command is also an assertion that its inverse restores the project file
//! byte for byte.

use sub_core::SubResult;
use sub_edit::{
    AddClip, AnyCommand, Command, CommandEnvelope, CommandRegistry, History, InsertClip, MoveClip,
    RemoveClip, RestoreTrackItems, RippleDelete, SplitClip, TrimClipIn, TrimClipOut, clip, codes,
};
use sub_model::{
    Clip, ClipId, MediaId, MediaItem, MediaPath, Project, Sequence, SequenceId, SequenceSettings,
    StreamInfo, Track, TrackId, TrackItem, TrackKind, json,
};
use sub_time::{Rational, RationalTime, TimeRange};

const RATE: Rational = Rational::FPS_24;

/// A time in frames at the fixture's 24 fps timebase.
fn frames(value: i64) -> RationalTime {
    RationalTime::new(value, RATE)
}

/// A source range in frames.
fn source(start: i64, duration: i64) -> TimeRange {
    TimeRange::new(frames(start), frames(duration)).expect("valid source range")
}

/// The fixture: one media item lasting 1000 frames, one sequence, one video
/// track and one empty second track to move clips to.
struct Fixture {
    project: Project,
    sequence: SequenceId,
    track: TrackId,
    other_track: TrackId,
    media: MediaId,
}

impl Fixture {
    /// Builds the fixture with `clips` laid end to end from zero, each named
    /// and each `length` frames of source starting at `source_start`.
    fn new(clips: &[(&str, i64, i64)]) -> Self {
        let mut project = Project::new("Clip commands");
        let mut media = MediaItem::new(MediaPath::new("footage/a.mp4").expect("valid path"));
        media.info = Some(StreamInfo {
            duration: Some(frames(1000)),
            video: Vec::new(),
            audio: Vec::new(),
        });
        let media_id = media.id;
        project.media.push(media);

        let mut track = Track::new("V1", TrackKind::Video);
        for (name, source_start, length) in clips {
            track
                .items
                .push(Clip::new(*name, media_id, source(*source_start, *length)).into());
        }
        let track_id = track.id;
        let other = Track::new("V2", TrackKind::Video);
        let other_id = other.id;

        let mut sequence = Sequence::new("Main", SequenceSettings::default());
        sequence.tracks.push(track);
        sequence.tracks.push(other);
        let sequence_id = sequence.id;
        project.sequences.push(sequence);

        Self {
            project,
            sequence: sequence_id,
            track: track_id,
            other_track: other_id,
            media: media_id,
        }
    }

    /// A fresh clip of `length` frames, not yet on any track.
    fn clip(&self, name: &str, length: i64) -> Clip {
        Clip::new(name, self.media, source(0, length))
    }

    /// The identifier of the clip named `name`.
    fn id_of(&self, name: &str) -> ClipId {
        self.clips()
            .into_iter()
            .find(|(clip_name, ..)| clip_name == name)
            .map_or_else(|| panic!("no clip named {name}"), |(_, id, ..)| id)
    }

    /// The main track's clips as (name, id, start, duration) in frames.
    fn clips(&self) -> Vec<(String, ClipId, i64, i64)> {
        self.track_named(self.track)
            .clip_placements(RATE)
            .map(|(clip, range)| {
                (
                    clip.name.clone(),
                    clip.id,
                    range.start().rescaled_to(RATE).value(),
                    range.duration().rescaled_to(RATE).value(),
                )
            })
            .collect()
    }

    /// The same, as (name, start, duration), which is what most assertions
    /// compare.
    fn layout(&self) -> Vec<(String, i64, i64)> {
        self.clips()
            .into_iter()
            .map(|(name, _, start, duration)| (name, start, duration))
            .collect()
    }

    /// The source range of the clip named `name`, in frames.
    fn source_of(&self, name: &str) -> (i64, i64) {
        let clip = self
            .track_named(self.track)
            .clips()
            .find(|clip| clip.name == name)
            .unwrap_or_else(|| panic!("no clip named {name}"));
        (
            clip.source_range.start().rescaled_to(RATE).value(),
            clip.source_range.duration().rescaled_to(RATE).value(),
        )
    }

    /// The items of the main track, gaps included.
    fn items(&self) -> &[TrackItem] {
        &self.track_named(self.track).items
    }

    /// A track of the fixture's sequence.
    fn track_named(&self, id: TrackId) -> &Track {
        self.project
            .sequence(self.sequence)
            .expect("sequence")
            .track(id)
            .expect("track")
    }
}

/// Expected layouts read better as owned tuples.
fn expect(layout: &[(&str, i64, i64)]) -> Vec<(String, i64, i64)> {
    layout
        .iter()
        .map(|(name, start, duration)| ((*name).to_owned(), *start, *duration))
        .collect()
}

/// Applies a command through a history and proves undo and redo are exact.
fn round_trip<C: Command>(project: &mut Project, command: C) -> SubResult<()> {
    let before = json::to_json(project).expect("serialisable");
    let mut history = History::new();
    history.apply(project, command)?;
    let after = json::to_json(project).expect("serialisable");

    history.undo(project).expect("undo");
    assert_eq!(
        json::to_json(project).expect("serialisable"),
        before,
        "undo must restore the project byte for byte"
    );
    history.redo(project).expect("redo");
    assert_eq!(
        json::to_json(project).expect("serialisable"),
        after,
        "redo must restore the edit byte for byte"
    );
    Ok(())
}

#[test]
fn add_places_a_clip_and_pads_the_hole_before_it_with_a_gap() {
    let mut fixture = Fixture::new(&[]);
    let clip = fixture.clip("shot", 24);
    round_trip(
        &mut fixture.project,
        AddClip {
            sequence: fixture.sequence,
            track: fixture.track,
            start: frames(48),
            clip,
        },
    )
    .expect("add");

    assert_eq!(fixture.layout(), expect(&[("shot", 48, 24)]));
    let items = fixture.items();
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0].as_gap().map(|gap| gap.duration),
        Some(frames(48)),
        "the hole before the clip becomes a gap"
    );
}

#[test]
fn add_overwrites_covered_clips_and_trims_the_ones_it_only_reaches() {
    // 0..24 a, 24..48 b, 48..72 c; write 12..60.
    let mut fixture = Fixture::new(&[("a", 0, 24), ("b", 0, 24), ("c", 0, 24)]);
    let clip = fixture.clip("over", 48);
    round_trip(
        &mut fixture.project,
        AddClip {
            sequence: fixture.sequence,
            track: fixture.track,
            start: frames(12),
            clip,
        },
    )
    .expect("add");

    assert_eq!(
        fixture.layout(),
        expect(&[("a", 0, 12), ("over", 12, 48), ("c", 60, 12)]),
        "b is covered and removed, a and c are trimmed"
    );
    assert_eq!(fixture.source_of("a"), (0, 12), "a keeps its head frames");
    assert_eq!(
        fixture.source_of("c"),
        (12, 12),
        "c keeps its tail frames, so its picture does not slide"
    );
}

#[test]
fn add_inside_a_clip_splits_it_and_the_head_keeps_the_identity() {
    let mut fixture = Fixture::new(&[("a", 0, 48)]);
    let original = fixture.id_of("a");
    let clip = fixture.clip("over", 12);
    round_trip(
        &mut fixture.project,
        AddClip {
            sequence: fixture.sequence,
            track: fixture.track,
            start: frames(12),
            clip,
        },
    )
    .expect("add");

    assert_eq!(
        fixture.layout(),
        expect(&[("a", 0, 12), ("over", 12, 12), ("a", 24, 24)])
    );
    let clips = fixture.clips();
    assert_eq!(clips[0].1, original, "the head keeps the clip's identity");
    assert_ne!(clips[2].1, original, "the tail is a new clip");
    assert_eq!(fixture.source_of("a"), (0, 12));
}

#[test]
fn add_butt_joined_to_a_neighbour_touches_nothing() {
    let mut fixture = Fixture::new(&[("a", 0, 24)]);
    let clip = fixture.clip("b", 24);
    round_trip(
        &mut fixture.project,
        AddClip {
            sequence: fixture.sequence,
            track: fixture.track,
            start: frames(24),
            clip,
        },
    )
    .expect("add");

    assert_eq!(fixture.layout(), expect(&[("a", 0, 24), ("b", 24, 24)]));
    assert_eq!(fixture.items().len(), 2, "no gap between butt-joined clips");
}

#[test]
fn add_refuses_unknown_media_a_duplicate_id_and_a_negative_start() {
    let mut fixture = Fixture::new(&[("a", 0, 24)]);
    let mut clip = fixture.clip("b", 24);
    clip.media = MediaId::new();
    let err = AddClip {
        sequence: fixture.sequence,
        track: fixture.track,
        start: frames(0),
        clip,
    }
    .apply(&mut fixture.project)
    .unwrap_err();
    assert_eq!(err.code, codes::MEDIA_NOT_FOUND);

    let mut duplicate = fixture.clip("a again", 24);
    duplicate.id = fixture.id_of("a");
    let err = AddClip {
        sequence: fixture.sequence,
        track: fixture.track,
        start: frames(48),
        clip: duplicate,
    }
    .apply(&mut fixture.project)
    .unwrap_err();
    assert_eq!(err.code, codes::DUPLICATE_CLIP);

    let err = AddClip {
        sequence: fixture.sequence,
        track: fixture.track,
        start: frames(-1),
        clip: fixture.clip("early", 24),
    }
    .apply(&mut fixture.project)
    .unwrap_err();
    assert_eq!(err.code, codes::INVALID_TIME);

    assert_eq!(
        fixture.layout(),
        expect(&[("a", 0, 24)]),
        "a refused command leaves the project untouched"
    );
}

#[test]
fn add_reports_a_missing_sequence_track_and_clip() {
    let mut fixture = Fixture::new(&[("a", 0, 24)]);
    let err = AddClip {
        sequence: SequenceId::new(),
        track: fixture.track,
        start: frames(0),
        clip: fixture.clip("b", 24),
    }
    .apply(&mut fixture.project)
    .unwrap_err();
    assert_eq!(err.code, codes::SEQUENCE_NOT_FOUND);

    let err = AddClip {
        sequence: fixture.sequence,
        track: TrackId::new(),
        start: frames(0),
        clip: fixture.clip("b", 24),
    }
    .apply(&mut fixture.project)
    .unwrap_err();
    assert_eq!(err.code, codes::TRACK_NOT_FOUND);

    let err = RemoveClip {
        sequence: fixture.sequence,
        track: fixture.track,
        clip: ClipId::new(),
    }
    .apply(&mut fixture.project)
    .unwrap_err();
    assert_eq!(err.code, codes::CLIP_NOT_FOUND);
}

#[test]
fn insert_pushes_the_clips_after_the_insert_point_later() {
    let mut fixture = Fixture::new(&[("a", 0, 24), ("b", 0, 24)]);
    let clip = fixture.clip("new", 12);
    round_trip(
        &mut fixture.project,
        InsertClip {
            sequence: fixture.sequence,
            track: fixture.track,
            start: frames(24),
            clip,
            tail_id: None,
        },
    )
    .expect("insert");

    assert_eq!(
        fixture.layout(),
        expect(&[("a", 0, 24), ("new", 24, 12), ("b", 36, 24)]),
        "nothing is overwritten: b rides the ripple"
    );
}

#[test]
fn insert_inside_a_clip_splits_it_and_ripples_only_the_tail() {
    let mut fixture = Fixture::new(&[("a", 0, 48), ("b", 0, 24)]);
    let original = fixture.id_of("a");
    let clip = fixture.clip("new", 12);
    round_trip(
        &mut fixture.project,
        InsertClip {
            sequence: fixture.sequence,
            track: fixture.track,
            start: frames(12),
            clip,
            tail_id: None,
        },
    )
    .expect("insert");

    assert_eq!(
        fixture.layout(),
        expect(&[("a", 0, 12), ("new", 12, 12), ("a", 24, 36), ("b", 60, 24)])
    );
    let clips = fixture.clips();
    assert_eq!(clips[0].1, original, "the head keeps the clip's identity");
    assert_ne!(clips[2].1, original, "the tail is a new clip");
    assert_eq!(
        fixture.source_of("a"),
        (0, 12),
        "the head keeps its own source frames"
    );
}

#[test]
fn insert_past_the_end_of_the_track_moves_nothing_and_pads_with_a_gap() {
    let mut fixture = Fixture::new(&[("a", 0, 24)]);
    let clip = fixture.clip("new", 12);
    round_trip(
        &mut fixture.project,
        InsertClip {
            sequence: fixture.sequence,
            track: fixture.track,
            start: frames(48),
            clip,
            tail_id: None,
        },
    )
    .expect("insert");

    assert_eq!(fixture.layout(), expect(&[("a", 0, 24), ("new", 48, 12)]));
    assert_eq!(
        fixture.items()[1].as_gap().map(|gap| gap.duration),
        Some(frames(24)),
        "the hole before the inserted clip is a gap"
    );
}

#[test]
fn insert_refuses_unknown_media_and_a_duplicate_clip_id() {
    let mut fixture = Fixture::new(&[("a", 0, 24)]);
    let mut clip = fixture.clip("new", 12);
    clip.media = MediaId::new();
    let err = InsertClip {
        sequence: fixture.sequence,
        track: fixture.track,
        start: frames(0),
        clip,
        tail_id: None,
    }
    .apply(&mut fixture.project)
    .unwrap_err();
    assert_eq!(err.code, codes::MEDIA_NOT_FOUND);

    let mut clip = fixture.clip("new", 12);
    clip.id = fixture.id_of("a");
    let err = InsertClip {
        sequence: fixture.sequence,
        track: fixture.track,
        start: frames(0),
        clip,
        tail_id: None,
    }
    .apply(&mut fixture.project)
    .unwrap_err();
    assert_eq!(err.code, codes::DUPLICATE_CLIP);
}

#[test]
fn remove_leaves_a_gap_and_the_clips_after_it_stay_put() {
    let mut fixture = Fixture::new(&[("a", 0, 24), ("b", 0, 24), ("c", 0, 24)]);
    let clip = fixture.id_of("b");
    round_trip(
        &mut fixture.project,
        RemoveClip {
            sequence: fixture.sequence,
            track: fixture.track,
            clip,
        },
    )
    .expect("remove");

    assert_eq!(fixture.layout(), expect(&[("a", 0, 24), ("c", 48, 24)]));
    assert_eq!(
        fixture.items()[1].as_gap().map(|gap| gap.duration),
        Some(frames(24))
    );
}

#[test]
fn removing_the_last_clip_shortens_the_track() {
    let mut fixture = Fixture::new(&[("a", 0, 24), ("b", 0, 24)]);
    let clip = fixture.id_of("b");
    round_trip(
        &mut fixture.project,
        RemoveClip {
            sequence: fixture.sequence,
            track: fixture.track,
            clip,
        },
    )
    .expect("remove");

    assert_eq!(fixture.layout(), expect(&[("a", 0, 24)]));
    assert_eq!(fixture.items().len(), 1, "no trailing gap is stored");
}

#[test]
fn move_leaves_a_hole_and_overwrites_where_it_lands() {
    let mut fixture = Fixture::new(&[("a", 0, 24), ("b", 0, 24), ("c", 0, 24)]);
    let clip = fixture.id_of("a");
    round_trip(
        &mut fixture.project,
        MoveClip {
            sequence: fixture.sequence,
            track: fixture.track,
            clip,
            start: frames(36),
            to_track: None,
        },
    )
    .expect("move");

    assert_eq!(
        fixture.layout(),
        expect(&[("b", 24, 12), ("a", 36, 24), ("c", 60, 12)])
    );
}

#[test]
fn move_to_another_track_updates_both_tracks_and_undoes_as_one() {
    let mut fixture = Fixture::new(&[("a", 0, 24), ("b", 0, 24)]);
    let clip = fixture.id_of("a");
    let other = fixture.other_track;
    round_trip(
        &mut fixture.project,
        MoveClip {
            sequence: fixture.sequence,
            track: fixture.track,
            clip,
            start: frames(0),
            to_track: Some(other),
        },
    )
    .expect("move");

    assert_eq!(fixture.layout(), expect(&[("b", 24, 24)]));
    let landed = fixture.track_named(other);
    assert_eq!(landed.clips().count(), 1);
    assert_eq!(landed.clip(clip).map(|clip| clip.name.as_str()), Some("a"));
}

#[test]
fn trim_in_moves_the_in_point_and_keeps_the_out_point() {
    let mut fixture = Fixture::new(&[("a", 0, 24), ("b", 0, 24)]);
    let clip = fixture.id_of("b");
    round_trip(
        &mut fixture.project,
        TrimClipIn {
            sequence: fixture.sequence,
            track: fixture.track,
            clip,
            delta: frames(6),
        },
    )
    .expect("trim in");

    assert_eq!(fixture.layout(), expect(&[("a", 0, 24), ("b", 30, 18)]));
    assert_eq!(fixture.source_of("b"), (6, 18));
    assert_eq!(
        fixture.items()[1].as_gap().map(|gap| gap.duration),
        Some(frames(6)),
        "trimming in later opens a hole before the clip"
    );
}

#[test]
fn trim_in_earlier_overwrites_the_neighbour_it_reaches_into() {
    let mut fixture = Fixture::new(&[("a", 0, 24), ("b", 12, 24)]);
    let clip = fixture.id_of("b");
    round_trip(
        &mut fixture.project,
        TrimClipIn {
            sequence: fixture.sequence,
            track: fixture.track,
            clip,
            delta: frames(-6),
        },
    )
    .expect("trim in");

    assert_eq!(fixture.layout(), expect(&[("a", 0, 18), ("b", 18, 30)]));
    assert_eq!(fixture.source_of("b"), (6, 30));
    assert_eq!(fixture.source_of("a"), (0, 18));
}

#[test]
fn trim_out_moves_the_out_point_and_keeps_the_in_point() {
    let mut fixture = Fixture::new(&[("a", 0, 24), ("b", 0, 24)]);
    let clip = fixture.id_of("a");
    round_trip(
        &mut fixture.project,
        TrimClipOut {
            sequence: fixture.sequence,
            track: fixture.track,
            clip,
            delta: frames(-6),
        },
    )
    .expect("trim out");

    assert_eq!(fixture.layout(), expect(&[("a", 0, 18), ("b", 24, 24)]));
    assert_eq!(fixture.source_of("a"), (0, 18));

    let clip = fixture.id_of("a");
    round_trip(
        &mut fixture.project,
        TrimClipOut {
            sequence: fixture.sequence,
            track: fixture.track,
            clip,
            delta: frames(12),
        },
    )
    .expect("trim out");
    assert_eq!(
        fixture.layout(),
        expect(&[("a", 0, 30), ("b", 30, 18)]),
        "lengthening overwrites the neighbour"
    );
}

#[test]
fn a_trim_that_would_empty_a_clip_or_leave_its_source_is_refused() {
    let mut fixture = Fixture::new(&[("a", 0, 24)]);
    let clip = fixture.id_of("a");

    for delta in [frames(24), frames(30)] {
        let err = TrimClipIn {
            sequence: fixture.sequence,
            track: fixture.track,
            clip,
            delta,
        }
        .apply(&mut fixture.project)
        .unwrap_err();
        assert_eq!(err.code, codes::INVALID_TRIM);
    }

    let err = TrimClipOut {
        sequence: fixture.sequence,
        track: fixture.track,
        clip,
        delta: frames(-24),
    }
    .apply(&mut fixture.project)
    .unwrap_err();
    assert_eq!(err.code, codes::INVALID_TRIM);

    // The source starts at frame 0, so the in point cannot go earlier.
    let err = TrimClipIn {
        sequence: fixture.sequence,
        track: fixture.track,
        clip,
        delta: frames(-1),
    }
    .apply(&mut fixture.project)
    .unwrap_err();
    assert_eq!(err.code, codes::INVALID_TRIM);

    // The probed media lasts 1000 frames, so the out point cannot pass it.
    let err = TrimClipOut {
        sequence: fixture.sequence,
        track: fixture.track,
        clip,
        delta: frames(1000),
    }
    .apply(&mut fixture.project)
    .unwrap_err();
    assert_eq!(err.code, codes::INVALID_TRIM);

    assert_eq!(fixture.layout(), expect(&[("a", 0, 24)]));
}

#[test]
fn split_cuts_a_clip_in_two_butt_joined_halves() {
    let mut fixture = Fixture::new(&[("a", 10, 24), ("b", 0, 24)]);
    let clip = fixture.id_of("a");
    round_trip(
        &mut fixture.project,
        SplitClip {
            sequence: fixture.sequence,
            track: fixture.track,
            clip,
            at: frames(6),
            tail_id: None,
        },
    )
    .expect("split");

    assert_eq!(
        fixture.layout(),
        expect(&[("a", 0, 6), ("a", 6, 18), ("b", 24, 24)]),
        "nothing after the split moves"
    );
    let clips = fixture.clips();
    assert_eq!(clips[0].1, clip, "the head keeps the identity");
    assert_ne!(clips[1].1, clip, "the tail is a new clip");

    let track = fixture.track_named(fixture.track);
    let tail = track.clip(clips[1].1).expect("tail");
    assert_eq!(tail.source_range.start(), frames(16));
    assert_eq!(tail.source_range.duration(), frames(18));
}

#[test]
fn split_at_a_boundary_or_outside_the_clip_is_a_structured_error() {
    let mut fixture = Fixture::new(&[("a", 0, 24), ("b", 0, 24)]);
    let clip = fixture.id_of("a");
    for at in [frames(0), frames(24), frames(30), frames(-6)] {
        let err = SplitClip {
            sequence: fixture.sequence,
            track: fixture.track,
            clip,
            at,
            tail_id: None,
        }
        .apply(&mut fixture.project)
        .unwrap_err();
        assert_eq!(err.code, codes::INVALID_SPLIT, "split at {at}");
        assert!(err.details.contains_key("clip"));
    }
    assert_eq!(fixture.layout(), expect(&[("a", 0, 24), ("b", 24, 24)]));
}

#[test]
fn a_split_may_name_the_tail_so_a_replay_reproduces_it() {
    let mut fixture = Fixture::new(&[("a", 0, 24)]);
    let clip = fixture.id_of("a");
    let tail = ClipId::new();
    round_trip(
        &mut fixture.project,
        SplitClip {
            sequence: fixture.sequence,
            track: fixture.track,
            clip,
            at: frames(12),
            tail_id: Some(tail),
        },
    )
    .expect("split");
    assert_eq!(fixture.clips()[1].1, tail);

    let err = SplitClip {
        sequence: fixture.sequence,
        track: fixture.track,
        clip,
        at: frames(6),
        tail_id: Some(clip),
    }
    .apply(&mut fixture.project)
    .unwrap_err();
    assert_eq!(err.code, codes::DUPLICATE_CLIP);
}

#[test]
fn ripple_delete_closes_the_hole() {
    let mut fixture = Fixture::new(&[("a", 0, 24), ("b", 0, 24), ("c", 0, 24)]);
    let clip = fixture.id_of("b");
    round_trip(
        &mut fixture.project,
        RippleDelete {
            sequence: fixture.sequence,
            track: fixture.track,
            clip,
        },
    )
    .expect("ripple delete");

    assert_eq!(fixture.layout(), expect(&[("a", 0, 24), ("c", 24, 24)]));
    assert_eq!(fixture.items().len(), 2, "no gap is left behind");
}

#[test]
fn ripple_delete_pulls_a_gap_back_too() {
    let mut fixture = Fixture::new(&[("a", 0, 24), ("b", 0, 24)]);
    // Open a hole between a and b, then ripple a away.
    let b = fixture.id_of("b");
    let mut history = History::new();
    history
        .apply(
            &mut fixture.project,
            MoveClip {
                sequence: fixture.sequence,
                track: fixture.track,
                clip: b,
                start: frames(36),
                to_track: None,
            },
        )
        .expect("move");
    assert_eq!(fixture.layout(), expect(&[("a", 0, 24), ("b", 36, 24)]));

    let a = fixture.id_of("a");
    round_trip(
        &mut fixture.project,
        RippleDelete {
            sequence: fixture.sequence,
            track: fixture.track,
            clip: a,
        },
    )
    .expect("ripple delete");
    assert_eq!(
        fixture.layout(),
        expect(&[("b", 12, 24)]),
        "everything after the removed clip moves back by its duration"
    );
}

#[test]
fn fades_are_clamped_when_a_trim_shortens_a_clip() {
    let mut fixture = Fixture::new(&[]);
    let mut clip = fixture.clip("a", 24);
    clip.fade_in = frames(8);
    clip.fade_out = frames(8);
    let id = clip.id;
    AddClip {
        sequence: fixture.sequence,
        track: fixture.track,
        start: frames(0),
        clip,
    }
    .apply(&mut fixture.project)
    .expect("add");

    round_trip(
        &mut fixture.project,
        TrimClipOut {
            sequence: fixture.sequence,
            track: fixture.track,
            clip: id,
            delta: frames(-18),
        },
    )
    .expect("trim out");

    let track = fixture.track_named(fixture.track);
    let clip = track.clip(id).expect("clip");
    assert_eq!(clip.duration(), frames(6));
    assert_eq!(clip.fade_in, frames(6));
    assert_eq!(clip.fade_out, frames(0));
    clip.validate().expect("a trimmed clip stays valid");
}

#[test]
fn a_split_keeps_the_fades_on_the_outer_edges() {
    let mut fixture = Fixture::new(&[]);
    let mut clip = fixture.clip("a", 48);
    clip.fade_in = frames(6);
    clip.fade_out = frames(6);
    let id = clip.id;
    AddClip {
        sequence: fixture.sequence,
        track: fixture.track,
        start: frames(0),
        clip,
    }
    .apply(&mut fixture.project)
    .expect("add");

    SplitClip {
        sequence: fixture.sequence,
        track: fixture.track,
        clip: id,
        at: frames(24),
        tail_id: None,
    }
    .apply(&mut fixture.project)
    .expect("split");

    let clips = fixture.clips();
    let track = fixture.track_named(fixture.track);
    let head = track.clip(clips[0].1).expect("head");
    let tail = track.clip(clips[1].1).expect("tail");
    assert_eq!((head.fade_in, head.fade_out), (frames(6), frames(0)));
    assert_eq!((tail.fade_in, tail.fade_out), (frames(0), frames(6)));
}

#[test]
fn a_group_of_clip_commands_undoes_in_one_step() {
    let mut fixture = Fixture::new(&[("a", 0, 24), ("b", 0, 24)]);
    let before = json::to_json(&fixture.project).expect("serialisable");
    let mut history = History::new();
    history.begin_group("Assemble").expect("group");
    for start in [48, 72] {
        let clip = fixture.clip("added", 24);
        history
            .apply(
                &mut fixture.project,
                AddClip {
                    sequence: fixture.sequence,
                    track: fixture.track,
                    start: frames(start),
                    clip,
                },
            )
            .expect("add");
    }
    assert!(history.commit_group().expect("commit"));
    assert_eq!(fixture.clips().len(), 4);

    history.undo(&mut fixture.project).expect("undo");
    assert_eq!(
        json::to_json(&fixture.project).expect("serialisable"),
        before
    );
    assert_eq!(history.undo_len(), 0);
}

#[test]
fn every_clip_command_registers_and_decodes_from_its_envelope() {
    let mut registry = CommandRegistry::new();
    clip::register(&mut registry).expect("register");
    assert_eq!(
        registry.kinds().collect::<Vec<_>>(),
        [
            "clip.add",
            "clip.insert",
            "clip.move",
            "clip.remove",
            "clip.ripple_delete",
            "clip.split",
            "clip.trim_in",
            "clip.trim_out",
            "edit.restore_track_items",
        ]
    );
    assert!(clip::register(&mut registry).is_err(), "kinds are unique");

    let mut fixture = Fixture::new(&[("a", 0, 48)]);
    let envelope = SplitClip {
        sequence: fixture.sequence,
        track: fixture.track,
        clip: fixture.id_of("a"),
        at: frames(24),
        tail_id: None,
    }
    .to_envelope()
    .expect("envelope");
    assert_eq!(envelope.kind, "clip.split");

    let decoded = registry.decode(&envelope).expect("decode");
    assert_eq!(decoded.label_erased(), "Split clip");
    decoded
        .apply_erased(&mut fixture.project)
        .expect("apply the decoded command");
    assert_eq!(fixture.clips().len(), 2);

    let bad = CommandEnvelope::new("clip.split", serde_json::json!({ "clip": "not an id" }));
    assert_eq!(
        registry.decode(&bad).unwrap_err().code,
        codes::INVALID_COMMAND
    );
}

#[test]
fn restoring_a_track_that_is_gone_is_refused_before_anything_changes() {
    let mut fixture = Fixture::new(&[("a", 0, 24)]);
    let before = json::to_json(&fixture.project).expect("serialisable");
    let err = RestoreTrackItems {
        sequence: fixture.sequence,
        tracks: vec![sub_edit::TrackItems {
            track: TrackId::new(),
            items: Vec::new(),
        }],
    }
    .apply(&mut fixture.project)
    .unwrap_err();
    assert_eq!(err.code, codes::TRACK_NOT_FOUND);
    assert_eq!(
        json::to_json(&fixture.project).expect("serialisable"),
        before
    );
}
