//! Media items and the bin tree that organises them.

use crate::ids::{BinId, MediaId};
use crate::sequence::ColorTags;

/// A source file the project references.
///
/// OTIO counterpart: `ExternalReference`, the media reference a `Clip` points
/// at. Subordinate hoists it out of the clip into a project-level list so many
/// clips share one entry, and so relinking a moved file is a single edit.
///
/// The relative path, content hash, probed stream info and proxy state are
/// added by TASK-3.3 and TASK-13; identity and colour tags live here because
/// every other type already needs them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaItem {
    /// Stable identity, preserved across save, load, undo and relink.
    pub id: MediaId,
    /// Display name, usually the file name.
    pub name: String,
    /// Colour tags read from the file, stored but not applied in the MVP
    /// (decision-3).
    pub color: ColorTags,
}

impl MediaItem {
    /// Creates a media item with a fresh identifier and Rec.709 tags.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: MediaId::new(),
            name: name.into(),
            color: ColorTags::REC709,
        }
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

    /// True when `media` is filed in this bin or any descendant.
    #[must_use]
    pub fn contains_media(&self, media: MediaId) -> bool {
        self.media.contains(&media)
            || self
                .children
                .iter()
                .any(|child| child.contains_media(media))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_items_default_to_rec709_tags() {
        let item = MediaItem::new("a.mp4");
        assert_eq!(item.color, ColorTags::REC709);
        assert_ne!(item.id, MediaItem::new("a.mp4").id);
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
        assert!(!root.contains_media(MediaId::new()));
    }
}
