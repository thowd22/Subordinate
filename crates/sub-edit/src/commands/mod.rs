//! The concrete command set.
//!
//! Every mutation the Command API, the MCP bridge and plugins can perform is
//! one of these types. They are grouped by what they touch: [`track`] for the
//! lanes of a sequence, [`sequence`] for the sequences of a project,
//! [`params`] for a clip's inspector parameters, [`marker`] for annotations on
//! sequences and clips, [`media`] for the sources a project references and
//! [`bin`] for the folders they are filed in.
//!
//! Two conventions run through the whole set.
//!
//! - **Restoring commands carry the whole entity.** Removing a track returns
//!   [`InsertTrack`] holding the removed [`Track`] and the index it sat at, so
//!   undo restores identifiers, contents and order exactly rather than
//!   rebuilding something that merely looks the same.
//! - **Lookups fail with a code, not a panic.** [`sequence_mut`],
//!   [`track_mut`] and [`track_for_clip_edit`] return
//!   `edit.sequence_not_found`, `edit.track_not_found` and `edit.track_locked`;
//!   commands validate through them before mutating anything, which is what
//!   keeps them atomic.

pub mod bin;
pub mod marker;
pub mod media;
pub mod params;
pub mod sequence;
pub mod track;

use sub_core::{SubError, SubResult};
use sub_model::{
    Bin, BinId, Clip, ClipId, MediaId, MediaItem, Project, Sequence, SequenceId, Track, TrackId,
    TrackItem,
};

pub use bin::{CreateBin, InsertBin, MoveBin, MoveToBin, RemoveBin, RenameBin};
pub use marker::{AddMarker, MarkerTarget, MoveMarker, RemoveMarker};
pub use media::{Filing, ImportMedia, InsertMedia, RelinkMedia, RemoveMedia};
pub use params::SetClipParams;
pub use sequence::{
    CreateSequence, DeleteSequence, InsertSequence, RenameSequence, SetSequenceSettings,
};
pub use track::{
    AddTrack, InsertTrack, RemoveTrack, RenameTrack, ReorderTrack, SetTrackLocked, SetTrackMuted,
};

use crate::CommandRegistry;

/// Registers every built-in command kind on `registry`.
///
/// The Command API builds its dispatcher from a registry, so this is the list
/// of kinds a `list methods` call enumerates.
///
/// ```
/// use sub_edit::{CommandRegistry, commands};
///
/// let mut registry = CommandRegistry::new();
/// commands::register_builtin(&mut registry).unwrap();
/// assert!(registry.contains("track.add"));
/// assert!(registry.contains("sequence.create"));
/// ```
///
/// # Errors
///
/// Returns `edit.duplicate_command` if `registry` already holds one of the
/// built-in kinds.
pub fn register_builtin(registry: &mut CommandRegistry) -> SubResult<()> {
    registry.register::<AddTrack>()?;
    registry.register::<InsertTrack>()?;
    registry.register::<RemoveTrack>()?;
    registry.register::<ReorderTrack>()?;
    registry.register::<RenameTrack>()?;
    registry.register::<SetTrackMuted>()?;
    registry.register::<SetTrackLocked>()?;
    registry.register::<CreateSequence>()?;
    registry.register::<InsertSequence>()?;
    registry.register::<DeleteSequence>()?;
    registry.register::<RenameSequence>()?;
    registry.register::<SetSequenceSettings>()?;
    registry.register::<SetClipParams>()?;
    registry.register::<AddMarker>()?;
    registry.register::<MoveMarker>()?;
    registry.register::<RemoveMarker>()?;
    registry.register::<ImportMedia>()?;
    registry.register::<InsertMedia>()?;
    registry.register::<RemoveMedia>()?;
    registry.register::<RelinkMedia>()?;
    registry.register::<CreateBin>()?;
    registry.register::<InsertBin>()?;
    registry.register::<RemoveBin>()?;
    registry.register::<RenameBin>()?;
    registry.register::<MoveToBin>()?;
    registry.register::<MoveBin>()?;
    Ok(())
}

/// A registry holding every built-in command kind.
///
/// # Errors
///
/// Never in practice: the built-in kinds are distinct, and a duplicate among
/// them would be a bug in this crate.
pub fn builtin_registry() -> SubResult<CommandRegistry> {
    let mut registry = CommandRegistry::new();
    register_builtin(&mut registry)?;
    Ok(registry)
}

/// The sequence with `id`, mutably.
///
/// # Errors
///
/// Returns `edit.sequence_not_found` when the project holds no such sequence.
pub fn sequence_mut(project: &mut Project, id: SequenceId) -> SubResult<&mut Sequence> {
    project.sequence_mut(id).ok_or_else(|| {
        SubError::new(crate::codes::SEQUENCE_NOT_FOUND, "no such sequence")
            .with_detail("sequence_id", id)
    })
}

/// The track with `track` in the sequence with `sequence`, mutably.
///
/// This is the lookup for commands that edit the track *header* — its name,
/// its mute and its lock — and it deliberately succeeds on a locked track, so
/// that the command unlocking it can run. Commands that edit what is *on* the
/// track use [`track_for_clip_edit`] instead.
///
/// # Errors
///
/// - `edit.sequence_not_found` when the project holds no such sequence.
/// - `edit.track_not_found` when the sequence holds no such track.
pub fn track_mut(
    project: &mut Project,
    sequence: SequenceId,
    track: TrackId,
) -> SubResult<&mut Track> {
    sequence_mut(project, sequence)?
        .track_mut(track)
        .ok_or_else(|| {
            SubError::new(crate::codes::TRACK_NOT_FOUND, "no such track")
                .with_detail("sequence_id", sequence)
                .with_detail("track_id", track)
        })
}

/// The track to edit clips on, refusing a locked one.
///
/// Every command that adds, removes, moves or trims an item on a track goes
/// through this lookup, which is what makes the track lock one rule rather
/// than a check each command has to remember (docs/PLAN.md §5.1).
///
/// # Errors
///
/// - `edit.sequence_not_found` when the project holds no such sequence.
/// - `edit.track_not_found` when the sequence holds no such track.
/// - `edit.track_locked` when the track is locked.
pub fn track_for_clip_edit(
    project: &mut Project,
    sequence: SequenceId,
    track: TrackId,
) -> SubResult<&mut Track> {
    let found = track_mut(project, sequence, track)?;
    if found.locked {
        return Err(SubError::new(
            crate::codes::TRACK_LOCKED,
            "the track is locked against edits",
        )
        .with_detail("sequence_id", sequence)
        .with_detail("track_id", track)
        .with_detail("track_name", found.name.clone()));
    }
    Ok(found)
}

/// The clip with `clip`, mutably, on a track that is open to clip edits.
///
/// # Errors
///
/// - `edit.sequence_not_found`, `edit.track_not_found` or `edit.track_locked`
///   as [`track_for_clip_edit`].
/// - `edit.clip_not_found` when the track holds no such clip.
pub fn clip_mut(
    project: &mut Project,
    sequence: SequenceId,
    track: TrackId,
    clip: ClipId,
) -> SubResult<&mut Clip> {
    let found = track_for_clip_edit(project, sequence, track)?;
    found
        .items
        .iter_mut()
        .filter_map(|item| match item {
            TrackItem::Clip(on_track) => Some(on_track),
            _ => None,
        })
        .find(|on_track| on_track.id == clip)
        .ok_or_else(|| {
            SubError::new(crate::codes::CLIP_NOT_FOUND, "no such clip on the track")
                .with_detail("sequence_id", sequence)
                .with_detail("track_id", track)
                .with_detail("clip_id", clip)
        })
}

/// The bin with `id`, mutably, searching the whole bin tree from the root.
///
/// # Errors
///
/// Returns `edit.bin_not_found` when the project holds no such bin.
pub fn bin_mut(project: &mut Project, id: BinId) -> SubResult<&mut Bin> {
    project.root_bin.find_mut(id).ok_or_else(|| {
        SubError::new(crate::codes::BIN_NOT_FOUND, "no such bin").with_detail("bin_id", id)
    })
}

/// The media item with `id`, mutably.
///
/// # Errors
///
/// Returns `edit.media_not_found` when the project references no such item.
pub fn media_item_mut(project: &mut Project, id: MediaId) -> SubResult<&mut MediaItem> {
    project.media_item_mut(id).ok_or_else(|| {
        SubError::new(
            crate::codes::MEDIA_NOT_FOUND,
            "no such media item in the project",
        )
        .with_detail("media", id)
    })
}

/// Checks that the project references the media item `media`.
///
/// # Errors
///
/// Returns `edit.media_not_found` when it does not.
pub fn require_media(project: &Project, media: MediaId) -> SubResult<()> {
    if project.media_item(media).is_none() {
        return Err(SubError::new(
            crate::codes::MEDIA_NOT_FOUND,
            "no such media item in the project",
        )
        .with_detail("media", media));
    }
    Ok(())
}

/// Checks that `index` is a valid insertion point into a list of `len` items.
///
/// # Errors
///
/// Returns `edit.invalid_index` when `index` is past the end.
fn check_insert_index(index: usize, len: usize, what: &'static str) -> SubResult<()> {
    if index > len {
        return Err(SubError::new(
            crate::codes::INVALID_INDEX,
            "index is past the end of the list",
        )
        .with_detail("what", what)
        .with_detail("index", index)
        .with_detail("len", len));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use sub_model::{Project, Sequence, SequenceSettings, Track, TrackKind};

    use super::*;
    use crate::codes;

    /// A project with one sequence holding one video track.
    fn project_with_track() -> (Project, SequenceId, TrackId) {
        let mut project = Project::new("Doc cut");
        let mut sequence = Sequence::new("Main", SequenceSettings::default());
        let track = Track::new("V1", TrackKind::Video);
        let track_id = track.id;
        sequence.tracks.push(track);
        let sequence_id = sequence.id;
        project.sequences.push(sequence);
        (project, sequence_id, track_id)
    }

    #[test]
    fn lookups_name_what_is_missing() {
        let (mut project, sequence_id, track_id) = project_with_track();

        let err = sequence_mut(&mut project, SequenceId::new()).unwrap_err();
        assert_eq!(err.code, codes::SEQUENCE_NOT_FOUND);

        let err = track_mut(&mut project, sequence_id, TrackId::new()).unwrap_err();
        assert_eq!(err.code, codes::TRACK_NOT_FOUND);
        assert_eq!(
            err.details.get("sequence_id"),
            Some(&serde_json::json!(sequence_id.to_string()))
        );

        let err = track_mut(&mut project, SequenceId::new(), track_id).unwrap_err();
        assert_eq!(err.code, codes::SEQUENCE_NOT_FOUND);
    }

    #[test]
    fn a_locked_track_is_open_to_header_edits_but_closed_to_clip_edits() {
        let (mut project, sequence_id, track_id) = project_with_track();
        track_mut(&mut project, sequence_id, track_id)
            .unwrap()
            .locked = true;

        // The header lookup still works: something has to unlock it again.
        assert!(track_mut(&mut project, sequence_id, track_id).is_ok());

        let err = track_for_clip_edit(&mut project, sequence_id, track_id).unwrap_err();
        assert_eq!(err.code, codes::TRACK_LOCKED);
        assert_eq!(
            err.details.get("track_name"),
            Some(&serde_json::json!("V1"))
        );

        track_mut(&mut project, sequence_id, track_id)
            .unwrap()
            .locked = false;
        assert!(track_for_clip_edit(&mut project, sequence_id, track_id).is_ok());
    }

    #[test]
    fn an_insert_index_may_sit_at_the_end_but_not_past_it() {
        assert!(check_insert_index(0, 0, "track").is_ok());
        assert!(check_insert_index(2, 2, "track").is_ok());
        let err = check_insert_index(3, 2, "track").unwrap_err();
        assert_eq!(err.code, codes::INVALID_INDEX);
        assert_eq!(err.details.get("len"), Some(&serde_json::json!(2)));
    }

    #[test]
    fn the_builtin_registry_holds_every_kind() {
        let registry = builtin_registry().unwrap();
        assert_eq!(
            registry.kinds().collect::<Vec<_>>(),
            [
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
                "sequence.create",
                "sequence.delete",
                "sequence.insert",
                "sequence.rename",
                "sequence.set_settings",
                "track.add",
                "track.insert",
                "track.remove",
                "track.rename",
                "track.reorder",
                "track.set_locked",
                "track.set_muted",
            ]
        );

        let mut registry = registry;
        let err = register_builtin(&mut registry).unwrap_err();
        assert_eq!(err.code, codes::DUPLICATE_COMMAND);
    }
}
