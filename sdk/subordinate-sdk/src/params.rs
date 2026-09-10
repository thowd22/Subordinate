//! Typed parameters for the Command API methods.
//!
//! `command-api.run-command` takes a method name and a JSON `params` document,
//! exactly as the local socket does. That is flexible and completely
//! unchecked: a plugin that writes `"clip.trimin"`, or spells a field
//! `clipId`, finds out at runtime.
//!
//! Every struct here is one method's parameters, and each names its own method
//! through [`CommandParams`], so [`Project::run`](crate::Project::run) takes the struct and the method
//! name comes with it. The field names, the optional fields and the JSON shapes
//! are those of the engine's own command structs in `sub_edit`, and the tests
//! at the bottom of this file deserialise each one *as* the engine's struct to
//! keep it that way.
//!
//! ```
//! use subordinate_sdk::params::TrimClipIn;
//! use subordinate_sdk::time::{frames, rate};
//! use subordinate_sdk::{ClipId, CommandParams, Id, SequenceId, TrackId};
//!
//! let trim = TrimClipIn {
//!     sequence: SequenceId::parse("018f-seq"),
//!     track: TrackId::parse("018f-trk"),
//!     clip: ClipId::parse("018f-clp"),
//!     delta: frames(2, rate::FPS_24),
//! };
//! assert_eq!(TrimClipIn::METHOD, "clip.trim_in");
//! assert_eq!(serde_json::to_value(&trim).unwrap()["delta"]["value"], 2);
//! ```
//!
//! # What is not here
//!
//! The commands whose parameters embed a whole project entity — `clip.add`
//! carries a `Clip`, `track.insert` a `Track`, `sequence.create` a
//! `SequenceSettings`, `marker.add` a `Marker` — are deliberately absent. The
//! SDK would have to mirror the project model to type them, and a mirror is a
//! second definition that drifts. Use [`Project::run_json`](crate::Project::run_json) for those: it takes
//! any [`serde::Serialize`] value, including a `serde_json::json!` literal, so
//! nothing is out of reach.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::bindings::subordinate::plugin::command_api::TrackKind;
use crate::bindings::subordinate::plugin::types::{
    ClipId, MarkerId, RationalTime, SequenceId, TimeRange, TrackId,
};

/// One Command API method's parameters, and the result it produces.
///
/// Implementing this ties a parameter struct to its method name, so a call site
/// can never pair the two up wrongly. `Output` is what the method's `result`
/// deserialises to: [`Applied`] for every undoable command, and a method's own
/// result type for the queries.
pub trait CommandParams: Serialize {
    /// The JSON-RPC method name, such as `clip.trim_in`.
    const METHOD: &'static str;

    /// What the method returns.
    type Output: DeserializeOwned;
}

/// What one applied, undone or redone command did.
///
/// The project itself is not included: a caller that wants it asks for
/// [`ProjectGet`], so an edit loop does not serialise the whole timeline on
/// every call.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct Applied {
    /// The revision the project reached.
    pub revision: u64,
    /// The label of the history step, as the undo menu shows it.
    pub label: String,
    /// What changed, in the order it changed, in the engine's event shape.
    #[serde(default)]
    pub events: Vec<serde_json::Value>,
}

/// Declares one command's parameter struct: its fields, its method name and its
/// result type.
macro_rules! command {
    (
        $(#[$meta:meta])*
        $name:ident => $method:literal, $output:ty {
            $($(#[$field_meta:meta])* $field:ident : $ty:ty),* $(,)?
        }
        $(optional { $($(#[$opt_meta:meta])* $opt:ident : $opt_ty:ty),* $(,)? })?
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Serialize, serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $($(#[$field_meta])* pub $field: $ty,)*
            $($(
                $(#[$opt_meta])*
                #[serde(default, skip_serializing_if = "Option::is_none")]
                pub $opt: Option<$opt_ty>,
            )*)?
        }

        impl CommandParams for $name {
            const METHOD: &'static str = $method;
            type Output = $output;
        }
    };
}

command! {
    /// Lifts a clip off its track, leaving the hole behind.
    ///
    /// This is the non-rippling delete: the clips after it stay where they are.
    /// Use [`RippleDelete`] to close the hole.
    RemoveClip => "clip.remove", Applied {
        /// The sequence holding the track.
        sequence: SequenceId,
        /// The track holding the clip.
        track: TrackId,
        /// The clip to remove.
        clip: ClipId,
    }
}

command! {
    /// Moves a clip to another position, and optionally to another track.
    ///
    /// The clip leaves a hole behind and overwrites what it lands on.
    MoveClip => "clip.move", Applied {
        /// The sequence holding the tracks.
        sequence: SequenceId,
        /// The track the clip is on now.
        track: TrackId,
        /// The clip to move.
        clip: ClipId,
        /// Where the clip starts afterwards, in sequence time.
        start: RationalTime,
    }
    optional {
        /// The track the clip lands on; the same track when absent.
        to_track: TrackId,
    }
}

command! {
    /// Moves a clip's in point, keeping its out point where it is.
    ///
    /// A positive `delta` shortens the clip from the head and leaves a hole; a
    /// negative one lengthens it, overwriting whatever lies before it.
    TrimClipIn => "clip.trim_in", Applied {
        /// The sequence holding the track.
        sequence: SequenceId,
        /// The track holding the clip.
        track: TrackId,
        /// The clip to trim.
        clip: ClipId,
        /// How far the in point moves later; negative moves it earlier.
        delta: RationalTime,
    }
}

command! {
    /// Moves a clip's out point, keeping its in point where it is.
    ///
    /// A positive `delta` lengthens the clip, overwriting whatever lies after
    /// it; a negative one shortens it and leaves a hole.
    TrimClipOut => "clip.trim_out", Applied {
        /// The sequence holding the track.
        sequence: SequenceId,
        /// The track holding the clip.
        track: TrackId,
        /// The clip to trim.
        clip: ClipId,
        /// How far the out point moves later; negative moves it earlier.
        delta: RationalTime,
    }
}

command! {
    /// Cuts a clip in two at a timeline instant.
    ///
    /// The head keeps the clip's identity and the tail becomes a new clip butt
    /// joined to it, so nothing on the track moves. `at` must fall strictly
    /// inside the clip.
    SplitClip => "clip.split", Applied {
        /// The sequence holding the track.
        sequence: SequenceId,
        /// The track holding the clip.
        track: TrackId,
        /// The clip to split.
        clip: ClipId,
        /// Where to cut, in sequence time.
        at: RationalTime,
    }
    optional {
        /// The identity to give the tail; a fresh one when absent.
        tail_id: ClipId,
    }
}

command! {
    /// Removes a clip and closes the hole, pulling the rest of the track back.
    ///
    /// Only the clip's own track ripples, so an edit that must ripple several
    /// tracks issues one command per track inside a history group.
    RippleDelete => "clip.ripple_delete", Applied {
        /// The sequence holding the track.
        sequence: SequenceId,
        /// The track holding the clip.
        track: TrackId,
        /// The clip to remove.
        clip: ClipId,
    }
}

command! {
    /// Adds an empty track to a sequence.
    AddTrack => "track.add", Applied {
        /// The sequence the track joins.
        sequence: SequenceId,
        /// Display name for the track header, such as `V1`.
        name: String,
        /// Whether the lane carries picture or sound.
        kind: TrackKind,
    }
    optional {
        /// Where in the sequence's track list it goes; appended when absent.
        index: usize,
    }
}

command! {
    /// Removes a track and everything on it.
    RemoveTrack => "track.remove", Applied {
        /// The sequence holding the track.
        sequence: SequenceId,
        /// The track to remove.
        track: TrackId,
        /// Whether to remove a track that still holds clips.
        force: bool,
    }
}

command! {
    /// Moves a track to another position in the sequence's track list.
    ReorderTrack => "track.reorder", Applied {
        /// The sequence holding the track.
        sequence: SequenceId,
        /// The track to move.
        track: TrackId,
        /// Its index afterwards.
        to_index: usize,
    }
}

command! {
    /// Renames a track.
    RenameTrack => "track.rename", Applied {
        /// The sequence holding the track.
        sequence: SequenceId,
        /// The track to rename.
        track: TrackId,
        /// The new display name.
        name: String,
    }
}

command! {
    /// Mutes or unmutes a track.
    SetTrackMuted => "track.set_muted", Applied {
        /// The sequence holding the track.
        sequence: SequenceId,
        /// The track to change.
        track: TrackId,
        /// Whether the track is muted afterwards.
        muted: bool,
    }
}

command! {
    /// Locks or unlocks a track.
    SetTrackLocked => "track.set_locked", Applied {
        /// The sequence holding the track.
        sequence: SequenceId,
        /// The track to change.
        track: TrackId,
        /// Whether the track is locked afterwards.
        locked: bool,
    }
}

command! {
    /// Deletes a sequence and everything in it.
    DeleteSequence => "sequence.delete", Applied {
        /// The sequence to delete.
        sequence: SequenceId,
    }
}

command! {
    /// Renames a sequence.
    RenameSequence => "sequence.rename", Applied {
        /// The sequence to rename.
        sequence: SequenceId,
        /// The new display name.
        name: String,
    }
}

command! {
    /// Removes a marker from a sequence or a clip.
    RemoveMarker => "marker.remove", Applied {
        /// The list the marker is on.
        target: MarkerTarget,
        /// The marker to remove.
        marker: MarkerId,
    }
}

command! {
    /// Moves a marker to another range on the same list.
    MoveMarker => "marker.move", Applied {
        /// The list the marker is on.
        target: MarkerTarget,
        /// The marker to move.
        marker: MarkerId,
        /// Its range afterwards, in the list's own time base.
        marked_range: TimeRange,
    }
}

/// Which marker list a marker command acts on.
///
/// The JSON is a tagged union: `{ "on": "sequence", "sequence": … }` or
/// `{ "on": "clip", "sequence": …, "track": …, "clip": … }`. A sequence's
/// markers are in sequence time; a clip's are in that clip's source time.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(tag = "on", rename_all = "snake_case", deny_unknown_fields)]
pub enum MarkerTarget {
    /// The sequence's own marker list, in sequence time.
    Sequence {
        /// The sequence carrying the markers.
        sequence: SequenceId,
    },
    /// A clip's marker list, in that clip's source time.
    Clip {
        /// The sequence holding the track.
        sequence: SequenceId,
        /// The track holding the clip.
        track: TrackId,
        /// The clip carrying the markers.
        clip: ClipId,
    },
}

/// Declares one query method: parameters, name and result type.
macro_rules! query {
    (
        $(#[$meta:meta])*
        $name:ident => $method:literal, $output:ty {
            $($(#[$field_meta:meta])* $field:ident : $ty:ty),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $($(#[$field_meta])* pub $field: $ty,)*
        }

        impl CommandParams for $name {
            const METHOD: &'static str = $method;
            type Output = $output;
        }
    };
}

query! {
    /// The whole project as JSON, with the revision it was read at.
    ///
    /// The project arrives as a `serde_json::Value` rather than a typed model:
    /// mirroring the project schema in the SDK would be a second definition to
    /// keep in step, and a plugin that needs one field should not pay for all
    /// of them.
    ProjectGet => "project.get", ProjectResult {}
}

query! {
    /// The current revision alone: the cheap way to notice a change.
    ProjectRevision => "project.revision", RevisionResult {}
}

query! {
    /// The state of the undo and redo stacks.
    HistoryGet => "history.get", HistoryResult {}
}

query! {
    /// Undoes the most recent history step.
    Undo => "edit.undo", UndoResult {}
}

query! {
    /// Redoes the most recently undone step.
    Redo => "edit.redo", UndoResult {}
}

query! {
    /// Opens a command group, so everything that follows undoes in one step.
    ///
    /// The host already opens one group around a `commands`-world plugin's
    /// `run`, so a menu command is one undo step without asking. Open a group
    /// explicitly when a single call has to apply several primitives and the
    /// host has not.
    BeginGroup => "edit.begin_group", BeginGroupResult {
        /// The label the grouped step gets in the undo menu.
        label: String,
    }
}

query! {
    /// Closes the open command group.
    CommitGroup => "edit.commit_group", CommitGroupResult {}
}

query! {
    /// Closes the open command group and reverses everything in it.
    AbortGroup => "edit.abort_group", AbortGroupResult {}
}

/// The result of [`ProjectGet`].
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ProjectResult {
    /// The revision the project was read at.
    pub revision: u64,
    /// The whole project, in the shape the project file stores it.
    pub project: serde_json::Value,
}

/// The result of [`ProjectRevision`].
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct RevisionResult {
    /// The current revision.
    pub revision: u64,
}

/// The result of [`HistoryGet`].
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct HistoryResult {
    /// Whether there is a step to undo.
    pub can_undo: bool,
    /// Whether there is a step to redo.
    pub can_redo: bool,
    /// The label of the step an undo would reverse.
    pub undo_label: Option<String>,
    /// The label of the step a redo would replay.
    pub redo_label: Option<String>,
    /// The number of steps that can be undone.
    pub undo_len: usize,
    /// The number of steps that can be redone.
    pub redo_len: usize,
    /// Whether a command group is open.
    pub in_group: bool,
}

/// The result of [`Undo`] and [`Redo`].
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct UndoResult {
    /// What the undo or redo did, or `None` when the stack was empty.
    pub applied: Option<Applied>,
}

/// The result of [`BeginGroup`].
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct BeginGroupResult {
    /// Always `true`: a group is now open.
    pub in_group: bool,
}

/// The result of [`CommitGroup`].
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct CommitGroupResult {
    /// Whether the group held anything, and so became a history step.
    pub committed: bool,
}

/// The result of [`AbortGroup`].
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct AbortGroupResult {
    /// Always `true`: the group was closed and reversed.
    pub aborted: bool,
}

#[cfg(test)]
mod tests {
    use super::{
        AddTrack, CommandParams, DeleteSequence, MarkerTarget, MoveClip, MoveMarker, RemoveClip,
        RemoveMarker, RemoveTrack, RenameSequence, RenameTrack, ReorderTrack, RippleDelete,
        SetTrackLocked, SetTrackMuted, SplitClip, TrimClipIn, TrimClipOut,
    };
    use crate::bindings::subordinate::plugin::command_api::TrackKind;
    use crate::ids::Id;
    use crate::time::{frames, range, rate};
    use serde::de::DeserializeOwned;
    use serde_json::Value;

    /// A `UUIDv7` in the canonical form the engine's identifiers parse.
    fn uuid(last: u8) -> String {
        format!("018f3c2e-0000-7000-8000-0000000000{last:02x}")
    }

    fn sequence() -> crate::SequenceId {
        crate::SequenceId::parse(uuid(1))
    }

    fn track() -> crate::TrackId {
        crate::TrackId::parse(uuid(2))
    }

    fn clip() -> crate::ClipId {
        crate::ClipId::parse(uuid(3))
    }

    fn marker() -> crate::MarkerId {
        crate::MarkerId::parse(uuid(4))
    }

    /// Serialises `params` and reads the JSON back as the engine's own command
    /// struct. Anything the SDK spells differently — a renamed field, a missing
    /// one, a time that is not two integers — fails here, because every engine
    /// command struct is `deny_unknown_fields`.
    fn matches_engine<P, E>(params: &P) -> Value
    where
        P: CommandParams,
        E: DeserializeOwned,
    {
        let json = serde_json::to_value(params).unwrap();
        serde_json::from_value::<E>(json.clone()).unwrap_or_else(|error| {
            panic!("{} params do not match the engine: {error}", P::METHOD)
        });
        json
    }

    #[test]
    fn clip_command_params_match_the_engine() {
        matches_engine::<_, sub_edit::RemoveClip>(&RemoveClip {
            sequence: sequence(),
            track: track(),
            clip: clip(),
        });
        matches_engine::<_, sub_edit::RippleDelete>(&RippleDelete {
            sequence: sequence(),
            track: track(),
            clip: clip(),
        });
        matches_engine::<_, sub_edit::TrimClipIn>(&TrimClipIn {
            sequence: sequence(),
            track: track(),
            clip: clip(),
            delta: frames(2, rate::FPS_23_976),
        });
        matches_engine::<_, sub_edit::TrimClipOut>(&TrimClipOut {
            sequence: sequence(),
            track: track(),
            clip: clip(),
            delta: frames(-2, rate::FPS_24),
        });
    }

    #[test]
    fn an_absent_option_is_omitted_rather_than_sent_as_null() {
        let json = matches_engine::<_, sub_edit::MoveClip>(&MoveClip {
            sequence: sequence(),
            track: track(),
            clip: clip(),
            start: frames(10, rate::FPS_24),
            to_track: None,
        });
        assert!(json.get("to_track").is_none());

        let json = matches_engine::<_, sub_edit::MoveClip>(&MoveClip {
            sequence: sequence(),
            track: track(),
            clip: clip(),
            start: frames(10, rate::FPS_24),
            to_track: Some(crate::TrackId::parse(uuid(5))),
        });
        assert_eq!(json["to_track"], Value::String(uuid(5)));

        let json = matches_engine::<_, sub_edit::SplitClip>(&SplitClip {
            sequence: sequence(),
            track: track(),
            clip: clip(),
            at: frames(4, rate::FPS_24),
            tail_id: None,
        });
        assert!(json.get("tail_id").is_none());
    }

    #[test]
    fn track_command_params_match_the_engine() {
        let json = matches_engine::<_, sub_edit::commands::AddTrack>(&AddTrack {
            sequence: sequence(),
            name: "V1".to_owned(),
            kind: TrackKind::Video,
            index: None,
        });
        assert_eq!(json["kind"], Value::String("video".to_owned()));

        matches_engine::<_, sub_edit::commands::RemoveTrack>(&RemoveTrack {
            sequence: sequence(),
            track: track(),
            force: true,
        });
        matches_engine::<_, sub_edit::commands::ReorderTrack>(&ReorderTrack {
            sequence: sequence(),
            track: track(),
            to_index: 2,
        });
        matches_engine::<_, sub_edit::commands::RenameTrack>(&RenameTrack {
            sequence: sequence(),
            track: track(),
            name: "A1".to_owned(),
        });
        matches_engine::<_, sub_edit::commands::SetTrackMuted>(&SetTrackMuted {
            sequence: sequence(),
            track: track(),
            muted: true,
        });
        matches_engine::<_, sub_edit::commands::SetTrackLocked>(&SetTrackLocked {
            sequence: sequence(),
            track: track(),
            locked: false,
        });
    }

    #[test]
    fn sequence_and_marker_command_params_match_the_engine() {
        matches_engine::<_, sub_edit::commands::DeleteSequence>(&DeleteSequence {
            sequence: sequence(),
        });
        matches_engine::<_, sub_edit::commands::RenameSequence>(&RenameSequence {
            sequence: sequence(),
            name: "Reel 1".to_owned(),
        });
        let json = matches_engine::<_, sub_edit::commands::RemoveMarker>(&RemoveMarker {
            target: MarkerTarget::Sequence {
                sequence: sequence(),
            },
            marker: marker(),
        });
        assert_eq!(json["target"]["on"], Value::String("sequence".to_owned()));

        let json = matches_engine::<_, sub_edit::commands::MoveMarker>(&MoveMarker {
            target: MarkerTarget::Clip {
                sequence: sequence(),
                track: track(),
                clip: clip(),
            },
            marker: marker(),
            marked_range: range(frames(0, rate::FPS_24), frames(1, rate::FPS_24)),
        });
        assert_eq!(json["target"]["on"], Value::String("clip".to_owned()));
    }

    #[test]
    fn every_method_name_is_one_the_engine_registers() {
        let registry = sub_edit::builtin_registry().expect("the built-in registry builds");
        for method in [
            RemoveClip::METHOD,
            MoveClip::METHOD,
            TrimClipIn::METHOD,
            TrimClipOut::METHOD,
            SplitClip::METHOD,
            RippleDelete::METHOD,
            AddTrack::METHOD,
            RemoveTrack::METHOD,
            ReorderTrack::METHOD,
            RenameTrack::METHOD,
            SetTrackMuted::METHOD,
            SetTrackLocked::METHOD,
            DeleteSequence::METHOD,
            RenameSequence::METHOD,
            RemoveMarker::METHOD,
            MoveMarker::METHOD,
        ] {
            assert!(
                registry.contains(method),
                "the engine registers no command named {method}"
            );
        }
    }
}
