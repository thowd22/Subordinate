//! The project: the root of the model and the thing a `.sub` file holds.

use crate::ids::{MediaId, ProjectId, SequenceId};
use crate::media::{Bin, MediaItem};
use crate::sequence::Sequence;

/// Everything a `.sub` file holds: media, bins and sequences (docs/PLAN.md
/// §5.6).
///
/// OTIO has no counterpart. An OTIO file is a single `Timeline`, so exporting
/// a project means exporting one [`Sequence`] at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// The sequence with `id`, if the project holds it.
    #[must_use]
    pub fn sequence(&self, id: SequenceId) -> Option<&Sequence> {
        self.sequences.iter().find(|sequence| sequence.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

        let media = MediaItem::new("interview.mp4");
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
}
