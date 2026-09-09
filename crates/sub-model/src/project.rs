//! The project: the root of the model and the thing a `.sub` file holds.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::{BinId, MediaId, ProjectId, SequenceId};
use crate::media::{Bin, MediaItem};
use crate::sequence::Sequence;

/// Everything a `.sub` file holds: media, bins and sequences (docs/PLAN.md
/// §5.6).
///
/// OTIO has no counterpart. An OTIO file is a single `Timeline`, so exporting
/// a project means exporting one [`Sequence`] at a time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// Stable identity, preserved across save and load.
    pub id: ProjectId,
    /// Display name, shown in the window title.
    pub name: String,
    /// Every source the project references, in no particular order; the bin
    /// tree, not this list, carries the user's arrangement.
    pub media: Vec<MediaItem>,
    /// The root of the bin tree.
    pub root_bin: Bin,
    /// The sequences, in tab order.
    pub sequences: Vec<Sequence>,
}

impl Project {
    /// Creates an empty project with a fresh identifier and a root bin named
    /// after it.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            id: ProjectId::new(),
            root_bin: Bin::new(name.clone()),
            name,
            media: Vec::new(),
            sequences: Vec::new(),
        }
    }

    /// The media item with `id`, if the project references it.
    #[must_use]
    pub fn media_item(&self, id: MediaId) -> Option<&MediaItem> {
        self.media.iter().find(|item| item.id == id)
    }

    /// The media item with `id`, mutably.
    pub fn media_item_mut(&mut self, id: MediaId) -> Option<&mut MediaItem> {
        self.media.iter_mut().find(|item| item.id == id)
    }

    /// The bin holding `media`, anywhere in the bin tree.
    #[must_use]
    pub fn bin_of(&self, media: MediaId) -> Option<BinId> {
        self.root_bin.bin_of(media).map(|bin| bin.id)
    }

    /// The sequence with `id`, if the project holds it.
    #[must_use]
    pub fn sequence(&self, id: SequenceId) -> Option<&Sequence> {
        self.sequences.iter().find(|sequence| sequence.id == id)
    }

    /// The sequence with `id`, mutably.
    pub fn sequence_mut(&mut self, id: SequenceId) -> Option<&mut Sequence> {
        self.sequences.iter_mut().find(|sequence| sequence.id == id)
    }

    /// The position of the sequence with `id` in [`Project::sequences`].
    ///
    /// Sequence order is the tab order, so commands that create or restore a
    /// sequence address it by index.
    #[must_use]
    pub fn sequence_index(&self, id: SequenceId) -> Option<usize> {
        self.sequences.iter().position(|sequence| sequence.id == id)
    }

    /// The absolute path of a media item's source file, given the folder
    /// holding the project file.
    ///
    /// Paths are stored project-relative (docs/PLAN.md §5.6), so this is the
    /// only supported way to turn one back into something openable.
    #[must_use]
    pub fn absolute_path(&self, project_dir: &Path, media: MediaId) -> Option<std::path::PathBuf> {
        self.media_item(media)
            .map(|item| item.absolute_path(project_dir))
    }

    /// Looks for every source file and updates each item's offline flag.
    ///
    /// Returns the items that are missing, in project order. Call it after
    /// loading a project or after the user points the app at a moved folder.
    pub fn refresh_offline(&mut self, project_dir: &Path) -> Vec<MediaId> {
        let mut offline = Vec::new();
        for item in &mut self.media {
            if !item.refresh_offline(project_dir) {
                offline.push(item.id);
            }
        }
        offline
    }

    /// The media items currently flagged offline, without touching the disk.
    pub fn offline_media(&self) -> impl Iterator<Item = &MediaItem> {
        self.media.iter().filter(|item| item.offline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::MediaPath;
    use crate::sequence::SequenceSettings;
    use crate::track::{Clip, Gap, Track, TrackKind, Transition};
    use sub_time::{Rational, RationalTime, TimeRange};

    #[test]
    fn a_new_project_is_empty_but_named() {
        let project = Project::new("Doc cut");
        assert_eq!(project.name, "Doc cut");
        assert_eq!(project.root_bin.name, "Doc cut");
        assert!(project.media.is_empty());
        assert!(project.sequences.is_empty());
    }

    #[test]
    fn a_project_holds_a_whole_otio_shaped_tree() {
        let rate = Rational::FPS_24;
        let mut project = Project::new("Doc cut");

        let media = MediaItem::new(MediaPath::new("interview.mp4").unwrap());
        let media_id = media.id;
        project.root_bin.media.push(media_id);
        project.media.push(media);

        let source =
            TimeRange::new(RationalTime::new(0, rate), RationalTime::new(24, rate)).unwrap();
        let half = RationalTime::new(6, rate);

        let mut video = Track::new("V1", TrackKind::Video);
        video.items.push(Clip::new("a", media_id, source).into());
        video.items.push(Transition::crossfade(half, half).into());
        video.items.push(Clip::new("b", media_id, source).into());

        let mut audio = Track::new("A1", TrackKind::Audio);
        audio
            .items
            .push(Gap::new(RationalTime::new(12, rate)).into());
        audio
            .items
            .push(Clip::new("room tone", media_id, source).into());

        let mut sequence = Sequence::new("Main", SequenceSettings::default());
        sequence.tracks.push(video);
        sequence.tracks.push(audio);
        let sequence_id = sequence.id;
        project.sequences.push(sequence);

        let sequence = project.sequence(sequence_id).unwrap();
        assert_eq!(sequence.tracks.len(), 2);
        assert_eq!(sequence.tracks[0].kind, TrackKind::Video);
        assert_eq!(
            sequence.tracks[0].duration(rate),
            RationalTime::new(48, rate)
        );
        assert_eq!(
            sequence.tracks[1].duration(rate),
            RationalTime::new(36, rate)
        );
        for clip in sequence.tracks.iter().flat_map(Track::clips) {
            assert!(project.media_item(clip.media).is_some());
        }
        assert!(project.media_item(MediaId::new()).is_none());
        assert!(project.sequence(crate::ids::SequenceId::new()).is_none());
    }

    #[test]
    fn media_is_filed_in_bins_by_id() {
        let mut project = Project::new("Doc cut");
        let item = MediaItem::new(MediaPath::new("footage/a.mp4").unwrap());
        let media_id = item.id;
        project.media.push(item);

        let mut interviews = Bin::new("Interviews");
        interviews.media.push(media_id);
        let bin_id = interviews.id;
        project.root_bin.children.push(interviews);

        assert_eq!(project.bin_of(media_id), Some(bin_id));
        assert_eq!(project.bin_of(MediaId::new()), None);
        assert_eq!(
            project.media_item_mut(media_id).map(|item| {
                item.name = "Interview A".to_owned();
                item.id
            }),
            Some(media_id)
        );
        assert_eq!(project.media_item(media_id).unwrap().name, "Interview A");
        assert!(project.media_item_mut(MediaId::new()).is_none());
    }

    #[test]
    fn offline_media_is_resolved_against_the_project_folder() {
        let dir = std::env::temp_dir().join(format!(
            "sub-model-project-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("footage")).unwrap();
        std::fs::write(dir.join("footage/present.mp4"), b"bytes").unwrap();

        let mut project = Project::new("Doc cut");
        let present = MediaItem::new(MediaPath::new("footage/present.mp4").unwrap());
        let present_id = present.id;
        let missing = MediaItem::new(MediaPath::new("footage/missing.mp4").unwrap());
        let missing_id = missing.id;
        project.media.push(present);
        project.media.push(missing);

        assert_eq!(
            project.absolute_path(&dir, present_id).unwrap(),
            dir.join("footage/present.mp4")
        );
        assert!(project.absolute_path(&dir, MediaId::new()).is_none());
        assert_eq!(project.offline_media().count(), 0);

        assert_eq!(project.refresh_offline(&dir), vec![missing_id]);
        let offline: Vec<MediaId> = project.offline_media().map(|item| item.id).collect();
        assert_eq!(offline, vec![missing_id]);
        assert!(!project.media_item(present_id).unwrap().offline);

        std::fs::remove_file(dir.join("footage/present.mp4")).unwrap();
        assert_eq!(project.refresh_offline(&dir).len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
