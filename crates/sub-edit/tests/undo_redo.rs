//! Undo then redo must land on a byte-identical project file.
//!
//! The commands here stand in for the real clip and track commands that arrive
//! in TASK-4.2 onwards: they mutate the same model in the same way (inserting
//! and removing track items, renaming clips), so the round-trip they prove is
//! the one the real ones will need.

use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult, codes as core_codes};
use sub_edit::{AnyCommand, Command, CommandEnvelope, CommandRegistry, History, Inverse};
use sub_model::{
    Clip, ClipId, MediaId, MediaItem, MediaPath, Project, Sequence, SequenceId, SequenceSettings,
    Track, TrackId, TrackItem, TrackKind, json,
};
use sub_time::{Rational, RationalTime, TimeRange};

/// Inserts an item into a track at `index`.
#[derive(Debug, schemars::JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InsertItem {
    sequence: SequenceId,
    track: TrackId,
    index: usize,
    item: TrackItem,
}

impl Command for InsertItem {
    const KIND: &'static str = "test.insert_item";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let track = track_mut(project, self.sequence, self.track)?;
        if self.index > track.items.len() {
            return Err(SubError::new(
                core_codes::INVALID_ARGUMENT,
                "insert index is past the end of the track",
            ));
        }
        track.items.insert(self.index, self.item.clone());
        Ok(Inverse::new(RemoveItem {
            sequence: self.sequence,
            track: self.track,
            index: self.index,
        }))
    }

    fn label(&self) -> String {
        "Insert item".to_owned()
    }
}

/// Removes the item at `index`; its inverse carries the item back.
#[derive(Debug, schemars::JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveItem {
    sequence: SequenceId,
    track: TrackId,
    index: usize,
}

impl Command for RemoveItem {
    const KIND: &'static str = "test.remove_item";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let track = track_mut(project, self.sequence, self.track)?;
        if self.index >= track.items.len() {
            return Err(SubError::new(
                core_codes::NOT_FOUND,
                "no item at that index",
            ));
        }
        let item = track.items.remove(self.index);
        Ok(Inverse::new(InsertItem {
            sequence: self.sequence,
            track: self.track,
            index: self.index,
            item,
        }))
    }
}

/// Renames a clip.
#[derive(Debug, schemars::JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameClip {
    sequence: SequenceId,
    track: TrackId,
    clip: ClipId,
    name: String,
}

impl Command for RenameClip {
    const KIND: &'static str = "test.rename_clip";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let track = track_mut(project, self.sequence, self.track)?;
        let clip = track
            .items
            .iter_mut()
            .filter_map(|item| match item {
                TrackItem::Clip(clip) => Some(clip),
                _ => None,
            })
            .find(|clip| clip.id == self.clip)
            .ok_or_else(|| SubError::new(core_codes::NOT_FOUND, "no such clip"))?;
        let previous = std::mem::replace(&mut clip.name, self.name.clone());
        Ok(Inverse::new(RenameClip {
            sequence: self.sequence,
            track: self.track,
            clip: self.clip,
            name: previous,
        }))
    }
}

fn track_mut(project: &mut Project, sequence: SequenceId, track: TrackId) -> SubResult<&mut Track> {
    project
        .sequences
        .iter_mut()
        .find(|candidate| candidate.id == sequence)
        .ok_or_else(|| SubError::new(core_codes::NOT_FOUND, "no such sequence"))?
        .tracks
        .iter_mut()
        .find(|candidate| candidate.id == track)
        .ok_or_else(|| SubError::new(core_codes::NOT_FOUND, "no such track"))
}

/// A project with one media item and one empty video track.
fn fixture() -> (Project, MediaId, SequenceId, TrackId) {
    let mut project = Project::new("Doc cut");
    let media = MediaItem::new(MediaPath::new("footage/interview.mp4").unwrap());
    let media_id = media.id;
    project.root_bin.media.push(media_id);
    project.media.push(media);

    let track = Track::new("V1", TrackKind::Video);
    let track_id = track.id;
    let mut sequence = Sequence::new("Main", SequenceSettings::default());
    let sequence_id = sequence.id;
    sequence.tracks.push(track);
    project.sequences.push(sequence);

    (project, media_id, sequence_id, track_id)
}

fn clip(media: MediaId, name: &str, start: i64, frames: i64) -> Clip {
    let rate = Rational::FPS_24;
    let range = TimeRange::new(
        RationalTime::new(start, rate),
        RationalTime::new(frames, rate),
    )
    .unwrap();
    Clip::new(name, media, range)
}

fn text(project: &Project) -> String {
    json::to_json(project).unwrap()
}

#[test]
fn undo_then_redo_restores_json_identical_state() {
    let (mut project, media, sequence, track) = fixture();
    let empty = text(&project);

    let first = clip(media, "shot 1", 0, 48);
    let clip_id = first.id;
    let second = clip(media, "shot 2", 48, 24);

    let mut history = History::new();
    history
        .apply(
            &mut project,
            InsertItem {
                sequence,
                track,
                index: 0,
                item: first.into(),
            },
        )
        .unwrap();
    history
        .apply(
            &mut project,
            InsertItem {
                sequence,
                track,
                index: 1,
                item: second.into(),
            },
        )
        .unwrap();
    history
        .apply(
            &mut project,
            RenameClip {
                sequence,
                track,
                clip: clip_id,
                name: "opening".to_owned(),
            },
        )
        .unwrap();
    history
        .apply(
            &mut project,
            RemoveItem {
                sequence,
                track,
                index: 1,
            },
        )
        .unwrap();

    let edited = text(&project);
    assert_ne!(edited, empty);
    assert_eq!(history.undo_len(), 4);

    // Two full cycles: the project text after undoing everything and after
    // redoing everything is identical each time round.
    for _ in 0..2 {
        while history.can_undo() {
            history.undo(&mut project).unwrap();
        }
        assert_eq!(text(&project), empty);

        while history.can_redo() {
            history.redo(&mut project).unwrap();
        }
        assert_eq!(text(&project), edited);
    }
}

#[test]
fn a_partial_undo_matches_the_state_that_step_produced() {
    let (mut project, media, sequence, track) = fixture();
    let mut history = History::new();
    let mut snapshots = vec![text(&project)];

    for (index, name) in ["a", "b", "c"].into_iter().enumerate() {
        history
            .apply(
                &mut project,
                InsertItem {
                    sequence,
                    track,
                    index,
                    item: clip(media, name, 0, 24).into(),
                },
            )
            .unwrap();
        snapshots.push(text(&project));
    }

    for expected in snapshots.iter().rev().skip(1) {
        history.undo(&mut project).unwrap();
        assert_eq!(&text(&project), expected);
    }
    for expected in snapshots.iter().skip(1) {
        history.redo(&mut project).unwrap();
        assert_eq!(&text(&project), expected);
    }
}

#[test]
fn a_group_of_commands_undoes_and_redoes_as_one_step() {
    let (mut project, media, sequence, track) = fixture();
    let before = text(&project);
    let mut history = History::new();

    // A drag: three clips laid down and one renamed, all in one undo step.
    history.begin_group("Assemble").unwrap();
    let first = clip(media, "a", 0, 24);
    let clip_id = first.id;
    history
        .apply(
            &mut project,
            InsertItem {
                sequence,
                track,
                index: 0,
                item: first.into(),
            },
        )
        .unwrap();
    for (index, name) in [(1_usize, "b"), (2, "c")] {
        history
            .apply(
                &mut project,
                InsertItem {
                    sequence,
                    track,
                    index,
                    item: clip(media, name, 0, 24).into(),
                },
            )
            .unwrap();
    }
    history
        .apply(
            &mut project,
            RenameClip {
                sequence,
                track,
                clip: clip_id,
                name: "opening".to_owned(),
            },
        )
        .unwrap();
    assert!(history.commit_group().unwrap());

    let after = text(&project);
    assert_eq!(history.undo_len(), 1);
    assert_eq!(history.undo_label(), Some("Assemble"));

    assert_eq!(
        history.undo(&mut project).unwrap().as_deref(),
        Some("Assemble")
    );
    assert_eq!(text(&project), before);
    assert!(!history.can_undo());

    history.redo(&mut project).unwrap();
    assert_eq!(text(&project), after);
}

#[test]
fn a_group_that_fails_halfway_leaves_the_project_untouched() {
    let (mut project, media, sequence, track) = fixture();
    let before = text(&project);
    let mut history = History::new();

    history.begin_group("Assemble").unwrap();
    history
        .apply(
            &mut project,
            InsertItem {
                sequence,
                track,
                index: 0,
                item: clip(media, "a", 0, 24).into(),
            },
        )
        .unwrap();
    let err = history
        .apply(
            &mut project,
            RemoveItem {
                sequence,
                track,
                index: 9,
            },
        )
        .unwrap_err();

    assert_eq!(err.code, core_codes::NOT_FOUND);
    assert!(!history.in_group());
    assert!(!history.can_undo());
    assert_eq!(text(&project), before);
}

#[test]
fn commands_decoded_from_json_apply_and_undo_the_same_way() {
    let (mut project, media, sequence, track) = fixture();
    let before = text(&project);

    let mut registry = CommandRegistry::new();
    registry.register::<InsertItem>().unwrap();
    registry.register::<RemoveItem>().unwrap();
    registry.register::<RenameClip>().unwrap();
    assert_eq!(
        registry.kinds().collect::<Vec<_>>(),
        ["test.insert_item", "test.remove_item", "test.rename_clip"]
    );

    // The shape an agent or plugin would send over the Command API.
    let envelope = InsertItem {
        sequence,
        track,
        index: 0,
        item: clip(media, "shot 1", 0, 48).into(),
    }
    .to_envelope()
    .unwrap();
    let logged = serde_json::to_string(&envelope).unwrap();
    assert!(logged.starts_with(r#"{"kind":"test.insert_item","params":{"#));

    let round_tripped: CommandEnvelope = serde_json::from_str(&logged).unwrap();
    let command = registry.decode(&round_tripped).unwrap();

    let mut history = History::new();
    history.apply_boxed(&mut project, command).unwrap();
    let after = text(&project);
    assert_ne!(after, before);

    // The history logs exactly what was applied.
    let entry = history.undo_entries().next().unwrap();
    assert_eq!(entry.to_envelopes().unwrap(), vec![envelope]);
    assert_eq!(entry.label(), "Insert item");

    history.undo(&mut project).unwrap();
    assert_eq!(text(&project), before);
    history.redo(&mut project).unwrap();
    assert_eq!(text(&project), after);
}

#[test]
fn a_bounded_history_still_round_trips_the_steps_it_kept() {
    let (mut project, media, sequence, track) = fixture();
    let mut history = History::with_depth(2).unwrap();

    for index in 0..4_usize {
        history
            .apply(
                &mut project,
                InsertItem {
                    sequence,
                    track,
                    index,
                    item: clip(media, "clip", 0, 24).into(),
                },
            )
            .unwrap();
    }
    let four = text(&project);
    assert_eq!(history.undo_len(), 2);

    history.undo(&mut project).unwrap();
    history.undo(&mut project).unwrap();
    let two = text(&project);
    assert!(!history.can_undo());

    history.redo(&mut project).unwrap();
    history.redo(&mut project).unwrap();
    assert_eq!(text(&project), four);

    history.undo(&mut project).unwrap();
    history.undo(&mut project).unwrap();
    assert_eq!(text(&project), two);
}
