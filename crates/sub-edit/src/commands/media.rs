//! Media commands: import, insert, remove and relink.
//!
//! A [`MediaItem`] is referenced by every clip cut from it, so removing one is
//! a destructive edit: [`RemoveMedia`] refuses an item clips still use unless
//! `force` is set, and the inverse carries the whole item back — identifier,
//! hash, probe result and proxy state — along with the bin and the position it
//! was filed at.
//!
//! Relinking never invents a new identity. [`RelinkMedia`] points the existing
//! item at another path, which is what keeps the clips cut from it intact when
//! a project folder moves (docs/PLAN.md §5.6).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};
use sub_model::{BinId, ContentHash, MediaId, MediaItem, MediaPath, Project, ProxyState};

use super::bin::filed_at;
use super::{bin_mut, check_insert_index, media_item_mut};
use crate::{Command, Inverse, codes};

/// Adds a media item to the project and files it in a bin.
///
/// The item arrives whole, with its own identifier, so replaying the command
/// from a log or a plugin reproduces the same project byte for byte. It is
/// filed in the root bin unless `bin` names another. Probing and hashing
/// happen before the command is built; nothing here touches the filesystem.
///
/// ```
/// use sub_edit::History;
/// use sub_edit::commands::ImportMedia;
/// use sub_model::{MediaItem, MediaPath, Project};
///
/// let mut project = Project::new("Doc cut");
/// let item = MediaItem::new(MediaPath::new("footage/interview.mp4").unwrap());
/// let media_id = item.id;
///
/// let mut history = History::new();
/// history.apply(&mut project, ImportMedia::new(item)).unwrap();
/// assert_eq!(project.bin_of(media_id), Some(project.root_bin.id));
///
/// history.undo(&mut project).unwrap();
/// assert!(project.media.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportMedia {
    /// The item itself.
    pub item: MediaItem,
    /// The bin it is filed in. The root bin when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<BinId>,
}

impl ImportMedia {
    /// Imports `item` into the root bin.
    #[must_use]
    pub fn new(item: MediaItem) -> Self {
        Self { item, bin: None }
    }

    /// The same command, filing the item in `bin`.
    #[must_use]
    pub fn into_bin(mut self, bin: BinId) -> Self {
        self.bin = Some(bin);
        self
    }
}

impl Command for ImportMedia {
    const KIND: &'static str = "media.import";
    const DESCRIPTION: &'static str = "Import a media file into the project and file it in a bin.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let id = self.item.id;
        require_absent(project, id)?;
        let bin_id = self.bin.unwrap_or(project.root_bin.id);
        let bin = bin_mut(project, bin_id)?;

        bin.media.push(id);
        project.media.push(self.item.clone());
        Ok(Inverse::new(RemoveMedia::forced(id)))
    }

    fn label(&self) -> String {
        format!("Import {}", self.item.name)
    }
}

/// Puts a whole media item back where it was.
///
/// This is the inverse of [`RemoveMedia`], and therefore what redo runs after
/// an [`ImportMedia`] is undone. Unlike [`ImportMedia`] it restores the item's
/// position in [`Project::media`] and in its bin, so the project file comes
/// back byte for byte.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InsertMedia {
    /// Where in [`Project::media`] the item goes.
    pub index: usize,
    /// The item itself, exactly as it was.
    pub item: MediaItem,
    /// The bin it was filed in, and where in that bin's list it sat. Absent
    /// when the item was filed nowhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filed: Option<Filing>,
}

/// Where a media item sits in the bin tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filing {
    /// The bin holding the item.
    pub bin: BinId,
    /// Where in that bin's media list it sits.
    pub index: usize,
}

impl Filing {
    /// The item sits at `index` in `bin`.
    #[must_use]
    pub fn new(bin: BinId, index: usize) -> Self {
        Self { bin, index }
    }
}

impl InsertMedia {
    /// Restores `item` at `index` in [`Project::media`], filed at `filed`.
    #[must_use]
    pub fn new(index: usize, item: MediaItem, filed: Option<Filing>) -> Self {
        Self { index, item, filed }
    }
}

impl Command for InsertMedia {
    const KIND: &'static str = "media.insert";
    const DESCRIPTION: &'static str =
        "Insert an existing media item into the project at a given index.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let id = self.item.id;
        require_absent(project, id)?;
        check_insert_index(self.index, project.media.len(), "media")?;
        if let Some(filed) = self.filed {
            let bin = bin_mut(project, filed.bin)?;
            check_insert_index(filed.index, bin.media.len(), "media")?;
            bin.media.insert(filed.index, id);
        }

        project.media.insert(self.index, self.item.clone());
        Ok(Inverse::new(RemoveMedia::forced(id)))
    }

    fn label(&self) -> String {
        format!("Restore {}", self.item.name)
    }
}

/// Removes a media item from the project and from its bin.
///
/// An item clips still reference is refused with `edit.media_in_use` unless
/// `force` is set: dropping the source out from under an edit must be
/// something the caller asked for, not something a mis-click does. A forced
/// removal is undoable like any other command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveMedia {
    /// The item to remove.
    pub media: MediaId,
    /// Whether to remove it even though clips still use it.
    #[serde(default)]
    pub force: bool,
}

impl RemoveMedia {
    /// Removes `media`, refusing to if clips still use it.
    #[must_use]
    pub fn new(media: MediaId) -> Self {
        Self {
            media,
            force: false,
        }
    }

    /// Removes `media` even though clips use it.
    #[must_use]
    pub fn forced(media: MediaId) -> Self {
        Self { media, force: true }
    }
}

impl Command for RemoveMedia {
    const KIND: &'static str = "media.remove";
    const DESCRIPTION: &'static str =
        "Remove a media item from the project, provided no clip uses it.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let index = project
            .media
            .iter()
            .position(|item| item.id == self.media)
            .ok_or_else(|| {
                SubError::new(codes::MEDIA_NOT_FOUND, "no such media item in the project")
                    .with_detail("media", self.media)
            })?;

        if !self.force {
            let users = clip_users(project, self.media);
            if users > 0 {
                return Err(SubError::new(
                    codes::MEDIA_IN_USE,
                    "clips in the project still use this media item",
                )
                .with_detail("media", self.media)
                .with_detail("clips", users));
            }
        }

        let filed = filed_at(project, self.media).map(|(bin, at)| Filing::new(bin, at));
        if let Some(filing) = filed {
            bin_mut(project, filing.bin)?.media.remove(filing.index);
        }
        let item = project.media.remove(index);
        Ok(Inverse::new(InsertMedia::new(index, item, filed)))
    }

    fn label(&self) -> String {
        "Remove media".to_owned()
    }
}

/// Sets a media item's proxy state.
///
/// Proxy generation is a background job, and what it reports — queued,
/// finished at a path, failed — is project state that is saved, undone and
/// redone like any other, so it arrives as a command rather than as a direct
/// mutation. The inverse carries the state the item had, which is what puts a
/// ready proxy back after an undo.
///
/// ```
/// use sub_edit::History;
/// use sub_edit::commands::{ImportMedia, SetProxyState};
/// use sub_model::{MediaItem, MediaPath, ProxyState, Project};
///
/// let mut project = Project::new("Doc cut");
/// let item = MediaItem::new(MediaPath::new("footage/interview.mp4").unwrap());
/// let media = item.id;
/// let mut history = History::new();
/// history.apply(&mut project, ImportMedia::new(item)).unwrap();
///
/// let proxy = MediaPath::new("cut.sub.d/interview.proxy.mov").unwrap();
/// let command = SetProxyState::new(media, ProxyState::Ready(proxy));
/// history.apply(&mut project, command).unwrap();
/// assert!(project.media_item(media).unwrap().proxy.is_ready());
///
/// history.undo(&mut project).unwrap();
/// assert_eq!(project.media_item(media).unwrap().proxy, ProxyState::None);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetProxyState {
    /// The item whose proxy state changes.
    pub media: MediaId,
    /// The state it takes.
    pub proxy: ProxyState,
}

impl SetProxyState {
    /// Sets `media`'s proxy state to `proxy`.
    #[must_use]
    pub fn new(media: MediaId, proxy: ProxyState) -> Self {
        Self { media, proxy }
    }

    /// Marks `media`'s proxy as generating.
    #[must_use]
    pub fn generating(media: MediaId) -> Self {
        Self::new(media, ProxyState::Pending)
    }

    /// Records a finished proxy at `path`.
    #[must_use]
    pub fn ready(media: MediaId, path: MediaPath) -> Self {
        Self::new(media, ProxyState::Ready(path))
    }
}

impl Command for SetProxyState {
    const KIND: &'static str = "media.set_proxy";
    const DESCRIPTION: &'static str =
        "Set a media item's proxy state: none, generating, ready at a path, stale or failed.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let item = media_item_mut(project, self.media)?;
        let previous = std::mem::replace(&mut item.proxy, self.proxy.clone());
        Ok(Inverse::new(Self::new(self.media, previous)))
    }

    fn label(&self) -> String {
        format!("Proxy {}", self.proxy.label())
    }
}

/// Points a media item at another file.
///
/// The item keeps its identifier, so every clip cut from it survives the
/// relink. The command carries the new hash and offline flag as well as the
/// path, because relinking is what a successful search for a moved file
/// concludes with, and the inverse carries the values the item had.
///
/// A proxy stands in for particular bytes, so pointing the item at a file that
/// hashes differently invalidates a ready proxy: it goes
/// [`Stale`](ProxyState::Stale) and preview falls back to the original until
/// the proxy is generated again (TASK-70). Re-hashing a file that changed in
/// place is the same command with the item's own path, and invalidates the
/// same way. The inverse carries the proxy state the item had, so undo puts a
/// ready proxy back.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelinkMedia {
    /// The item to relink.
    pub media: MediaId,
    /// The file it now refers to, relative to the project folder.
    pub path: MediaPath,
    /// The fingerprint of that file's bytes, when it has been hashed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<ContentHash>,
    /// Whether the file is still missing. False for a successful relink.
    #[serde(default)]
    pub offline: bool,
    /// The proxy state to force, which only an inverse sets. Left out, the
    /// command invalidates a ready proxy when the hash it records differs from
    /// the one the item had.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxyState>,
}

impl RelinkMedia {
    /// Points `media` at `path`, online and unhashed.
    #[must_use]
    pub fn new(media: MediaId, path: MediaPath) -> Self {
        Self {
            media,
            path,
            hash: None,
            offline: false,
            proxy: None,
        }
    }

    /// The same command, also recording the file's content hash.
    #[must_use]
    pub fn with_hash(mut self, hash: ContentHash) -> Self {
        self.hash = Some(hash);
        self
    }

    /// The same command, marking the item offline.
    #[must_use]
    pub fn offline(mut self) -> Self {
        self.offline = true;
        self
    }
}

impl Command for RelinkMedia {
    const KIND: &'static str = "media.relink";
    const DESCRIPTION: &'static str = "Point a media item at a different file on disk.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let item = media_item_mut(project, self.media)?;
        let previous_hash = std::mem::replace(&mut item.hash, self.hash);
        let previous = Self {
            media: self.media,
            path: std::mem::replace(&mut item.path, self.path.clone()),
            hash: previous_hash,
            offline: std::mem::replace(&mut item.offline, self.offline),
            proxy: Some(item.proxy.clone()),
        };
        if let Some(proxy) = self.proxy.clone() {
            item.proxy = proxy;
        } else if self.hash != previous_hash {
            // The item now refers to other bytes than the proxy was made
            // from, so nothing may be previewed from it until it is made
            // again. The file stays where it is; only the state changes.
            item.invalidate_proxy();
        }
        Ok(Inverse::new(previous))
    }

    fn label(&self) -> String {
        format!("Relink to {}", self.path.as_str())
    }
}

/// Refuses an identifier the project already holds.
fn require_absent(project: &Project, media: MediaId) -> SubResult<()> {
    if project.media_item(media).is_some() {
        return Err(SubError::new(
            codes::DUPLICATE_MEDIA,
            "the project already holds a media item with this identifier",
        )
        .with_detail("media", media));
    }
    Ok(())
}

/// How many clips in the project reference `media`.
fn clip_users(project: &Project, media: MediaId) -> usize {
    project
        .sequences
        .iter()
        .flat_map(|sequence| sequence.tracks.iter())
        .flat_map(sub_model::Track::clips)
        .filter(|clip| clip.media == media)
        .count()
}
