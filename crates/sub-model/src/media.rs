//! Media items and the bin tree that organises them.

use std::path::Path;

use sub_time::{Rational, RationalTime};

use crate::content::{ContentHash, MediaPath};
use crate::ids::{BinId, MediaId};
use crate::sequence::ColorTags;

/// One video stream reported by the probe.
///
/// Frame rate and sample aspect are exact rationals; nothing here is a float.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VideoStream {
    /// Coded width in pixels.
    pub width: u32,
    /// Coded height in pixels.
    pub height: u32,
    /// Nominal frame rate. Variable-rate sources report their average here and
    /// carry a PTS index instead (TASK-16).
    pub frame_rate: Rational,
    /// Pixel (sample) aspect ratio; `1/1` for square pixels.
    pub sample_aspect: Rational,
    /// Colour tags read from the container or bitstream (decision-3).
    pub color: ColorTags,
}

/// One audio stream reported by the probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AudioStream {
    /// Channel count.
    pub channels: u16,
    /// Sample rate in hertz.
    pub sample_rate: u32,
}

/// What the probe found in a media file (docs/PLAN.md §5.2).
///
/// Filled in by the GStreamer discoverer in `sub-media` (TASK-13); the model
/// stores it so the bin, the timeline and an offline project can all show
/// duration and format without touching the file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreamInfo {
    /// Total duration of the file, exact at its own rate.
    pub duration: Option<RationalTime>,
    /// Video streams, in file order.
    pub video: Vec<VideoStream>,
    /// Audio streams, in file order.
    pub audio: Vec<AudioStream>,
}

impl StreamInfo {
    /// True when the file carries at least one video stream.
    #[must_use]
    pub fn has_video(&self) -> bool {
        !self.video.is_empty()
    }

    /// True when the file carries at least one audio stream.
    #[must_use]
    pub fn has_audio(&self) -> bool {
        !self.audio.is_empty()
    }
}

/// Whether a reduced-resolution proxy exists for a media item (TASK-69,
/// TASK-70).
///
/// The proxy path is project-relative like every other media path; proxies live
/// in the `project.sub.d/` sidecar folder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProxyState {
    /// No proxy has been asked for.
    #[default]
    None,
    /// A proxy job has been queued or is running.
    Pending,
    /// A proxy exists at this project-relative path.
    Ready(MediaPath),
    /// Proxy generation failed; the message is for the user, the code was
    /// reported when it happened.
    Failed(String),
}

impl ProxyState {
    /// The proxy file, when one is ready.
    #[must_use]
    pub fn path(&self) -> Option<&MediaPath> {
        match self {
            Self::Ready(path) => Some(path),
            _ => None,
        }
    }
}

/// A source file the project references.
///
/// OTIO counterpart: `ExternalReference`, the media reference a `Clip` points
/// at. Subordinate hoists it out of the clip into a project-level list so many
/// clips share one entry, and so relinking a moved file is a single edit.
///
/// The stored path is always project-relative and the [`ContentHash`] identifies
/// the bytes, so a project folder copied to another machine relinks without
/// user intervention (docs/PLAN.md §5.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaItem {
    /// Stable identity, preserved across save, load, undo and relink.
    pub id: MediaId,
    /// Display name, usually the file name.
    pub name: String,
    /// Where the file lives, relative to the project folder.
    pub path: MediaPath,
    /// Fingerprint of the file's bytes, when it has been hashed. `None` until
    /// the file is first read, and on items restored from a project whose
    /// media was offline at save time.
    pub hash: Option<ContentHash>,
    /// What the probe found, when the file has been probed.
    pub info: Option<StreamInfo>,
    /// Proxy availability.
    pub proxy: ProxyState,
    /// True when the file was missing the last time it was looked for. Set by
    /// [`MediaItem::refresh_offline`]; nothing else in the model reads the
    /// filesystem.
    pub offline: bool,
    /// Colour tags read from the file, stored but not applied in the MVP
    /// (decision-3).
    pub color: ColorTags,
}

impl MediaItem {
    /// Creates a media item for `path`, named after its file name.
    #[must_use]
    pub fn new(path: MediaPath) -> Self {
        Self {
            id: MediaId::new(),
            name: path.file_name().to_owned(),
            path,
            hash: None,
            info: None,
            proxy: ProxyState::None,
            offline: false,
            color: ColorTags::REC709,
        }
    }

    /// The absolute path of the source file, given the folder holding the
    /// project file.
    #[must_use]
    pub fn absolute_path(&self, project_dir: &Path) -> std::path::PathBuf {
        self.path.resolve(project_dir)
    }

    /// The absolute path of the proxy, when one is ready.
    #[must_use]
    pub fn absolute_proxy_path(&self, project_dir: &Path) -> Option<std::path::PathBuf> {
        self.proxy.path().map(|path| path.resolve(project_dir))
    }

    /// Looks for the source file and updates [`MediaItem::offline`].
    ///
    /// Returns true when the file is present. This is the one place in the
    /// model that touches the filesystem.
    pub fn refresh_offline(&mut self, project_dir: &Path) -> bool {
        let online = self.absolute_path(project_dir).is_file();
        self.offline = !online;
        online
    }
}

/// A folder in the media bin.
///
/// OTIO has no counterpart; the closest is `SerializableCollection`, which
/// carries no hierarchy. Bins nest, and hold media items by ID rather than by
/// value so an item appears in exactly one place in the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bin {
    /// Stable identity, preserved across save, load and undo.
    pub id: BinId,
    /// Display name. The root bin conventionally carries the project name.
    pub name: String,
    /// Media items filed directly in this bin, in user order.
    pub media: Vec<MediaId>,
    /// Nested bins, in user order.
    pub children: Vec<Bin>,
}

impl Bin {
    /// Creates an empty bin with a fresh identifier.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: BinId::new(),
            name: name.into(),
            media: Vec::new(),
            children: Vec::new(),
        }
    }

    /// The bin with `id`, searching this bin and its descendants.
    #[must_use]
    pub fn find(&self, id: BinId) -> Option<&Bin> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(id))
    }

    /// The bin with `id`, mutably, searching this bin and its descendants.
    pub fn find_mut(&mut self, id: BinId) -> Option<&mut Bin> {
        if self.id == id {
            return Some(self);
        }
        self.children
            .iter_mut()
            .find_map(|child| child.find_mut(id))
    }

    /// The bin holding `media` directly, searching this bin and its
    /// descendants.
    #[must_use]
    pub fn bin_of(&self, media: MediaId) -> Option<&Bin> {
        if self.media.contains(&media) {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.bin_of(media))
    }

    /// True when `media` is filed in this bin or any descendant.
    #[must_use]
    pub fn contains_media(&self, media: MediaId) -> bool {
        self.bin_of(media).is_some()
    }

    /// Removes `media` from this bin or any descendant.
    ///
    /// Returns true when it was filed somewhere in this subtree.
    pub fn remove_media(&mut self, media: MediaId) -> bool {
        if let Some(index) = self.media.iter().position(|id| *id == media) {
            self.media.remove(index);
            return true;
        }
        self.children
            .iter_mut()
            .any(|child| child.remove_media(media))
    }

    /// Every bin in this subtree, parents before children.
    pub fn iter(&self) -> impl Iterator<Item = &Bin> {
        let mut stack = vec![self];
        std::iter::from_fn(move || {
            let bin = stack.pop()?;
            stack.extend(bin.children.iter().rev());
            Some(bin)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `MediaPath` for a valid literal.
    fn path(text: &str) -> MediaPath {
        MediaPath::new(text).unwrap()
    }

    #[test]
    fn media_items_are_named_after_their_file_and_start_unprobed() {
        let item = MediaItem::new(path("footage/interview.mp4"));
        assert_eq!(item.name, "interview.mp4");
        assert_eq!(item.color, ColorTags::REC709);
        assert!(item.hash.is_none());
        assert!(item.info.is_none());
        assert_eq!(item.proxy, ProxyState::None);
        assert!(!item.offline);
        assert_ne!(item.id, MediaItem::new(path("a.mp4")).id);
    }

    #[test]
    fn media_items_resolve_source_and_proxy_paths() {
        let dir = Path::new("/p/doc");
        let mut item = MediaItem::new(path("footage/a.mp4"));
        assert_eq!(item.absolute_path(dir), Path::new("/p/doc/footage/a.mp4"));
        assert!(item.absolute_proxy_path(dir).is_none());

        item.proxy = ProxyState::Ready(path("project.sub.d/proxies/a.mov"));
        assert_eq!(
            item.absolute_proxy_path(dir).unwrap(),
            Path::new("/p/doc/project.sub.d/proxies/a.mov")
        );
        assert_eq!(ProxyState::Pending.path(), None);
        assert_eq!(ProxyState::Failed("no encoder".into()).path(), None);
    }

    #[test]
    fn stream_info_reports_what_the_probe_found() {
        let mut info = StreamInfo::default();
        assert!(!info.has_video());
        assert!(!info.has_audio());
        info.video.push(VideoStream {
            width: 1920,
            height: 1080,
            frame_rate: Rational::FPS_24,
            sample_aspect: Rational::ONE,
            color: ColorTags::REC709,
        });
        info.audio.push(AudioStream {
            channels: 2,
            sample_rate: 48_000,
        });
        info.duration = Some(RationalTime::new(48, Rational::FPS_24));
        assert!(info.has_video());
        assert!(info.has_audio());
        assert_eq!(
            info.duration.unwrap(),
            RationalTime::new(48, Rational::FPS_24)
        );
    }

    #[test]
    fn bins_search_their_descendants() {
        let media = MediaId::new();
        let mut nested = Bin::new("Interviews");
        nested.media.push(media);
        let nested_id = nested.id;

        let mut root = Bin::new("Project");
        root.children.push(nested);

        assert_eq!(
            root.find(nested_id).map(|b| b.name.as_str()),
            Some("Interviews")
        );
        assert_eq!(root.find(root.id).map(|b| b.id), Some(root.id));
        assert!(root.find(BinId::new()).is_none());
        assert!(root.contains_media(media));
        assert_eq!(root.bin_of(media).map(|b| b.id), Some(nested_id));
        assert!(!root.contains_media(MediaId::new()));

        root.find_mut(nested_id).unwrap().name = "B-roll".to_owned();
        assert_eq!(root.find(nested_id).unwrap().name, "B-roll");
        assert!(root.find_mut(BinId::new()).is_none());
        assert_eq!(root.find_mut(root.id).map(|b| b.id), Some(root.id));
    }

    #[test]
    fn bins_remove_media_anywhere_in_the_subtree() {
        let media = MediaId::new();
        let mut nested = Bin::new("Interviews");
        nested.media.push(media);
        let mut root = Bin::new("Project");
        root.children.push(nested);

        assert!(root.remove_media(media));
        assert!(!root.contains_media(media));
        assert!(!root.remove_media(media));

        root.media.push(media);
        assert!(root.remove_media(media));
    }

    #[test]
    fn bins_iterate_parents_before_children_in_order() {
        let mut root = Bin::new("Project");
        root.children.push(Bin::new("A"));
        root.children[0].children.push(Bin::new("A1"));
        root.children.push(Bin::new("B"));

        let names: Vec<&str> = root.iter().map(|bin| bin.name.as_str()).collect();
        assert_eq!(names, ["Project", "A", "A1", "B"]);
    }
}
