//! Bin commands: create, insert, remove, rename and file media.
//!
//! The bin tree is the user's arrangement of the project's media, so every
//! command here addresses a position as well as an identifier and the inverse
//! of a removal puts the whole subtree back where it came from.
//!
//! Two rules hold throughout:
//!
//! - **The root bin is not a bin like the others.** It is the tree itself, so
//!   it cannot be removed or reparented; `edit.root_bin` says so. It can be
//!   renamed, because its name is the project's name in the bin panel.
//! - **A media item is filed in exactly one bin.** [`MoveToBin`] lifts it out
//!   of the bin it is in and files it in another, and its inverse puts it back
//!   at the index it held.

use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};
use sub_model::{Bin, BinId, MediaId, Project};

use super::{bin_mut, check_insert_index, require_media};
use crate::{Command, Inverse, codes};

/// Creates a new, empty bin.
///
/// It joins the root bin unless `parent` names another, and is appended unless
/// `index` names a position.
///
/// ```
/// use sub_edit::History;
/// use sub_edit::commands::CreateBin;
/// use sub_model::Project;
///
/// let mut project = Project::new("Doc cut");
/// let mut history = History::new();
/// history.apply(&mut project, CreateBin::new("Interviews")).unwrap();
/// assert_eq!(project.root_bin.children[0].name, "Interviews");
///
/// history.undo(&mut project).unwrap();
/// assert!(project.root_bin.children.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBin {
    /// Display name, shown in the bin panel.
    pub name: String,
    /// The bin it goes inside. The root bin when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<BinId>,
    /// Where in the parent's children it goes. Appended when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

impl CreateBin {
    /// Appends a bin named `name` to the root bin.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            parent: None,
            index: None,
        }
    }

    /// The same command, creating the bin inside `parent`.
    #[must_use]
    pub fn inside(mut self, parent: BinId) -> Self {
        self.parent = Some(parent);
        self
    }

    /// The same command, inserting at `index` instead of appending.
    #[must_use]
    pub fn at(mut self, index: usize) -> Self {
        self.index = Some(index);
        self
    }
}

impl Command for CreateBin {
    const KIND: &'static str = "bin.create";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let parent_id = self.parent.unwrap_or(project.root_bin.id);
        let parent = bin_mut(project, parent_id)?;
        let index = self.index.unwrap_or(parent.children.len());
        check_insert_index(index, parent.children.len(), "bin")?;

        let bin = Bin::new(self.name.clone());
        let id = bin.id;
        parent.children.insert(index, bin);
        Ok(Inverse::new(RemoveBin::forced(id)))
    }

    fn label(&self) -> String {
        format!("Create bin {}", self.name)
    }
}

/// Puts a whole bin subtree back at a given position.
///
/// This is the inverse of [`RemoveBin`], and therefore what redo runs after a
/// [`CreateBin`] is undone. It carries the entire subtree — identifiers, child
/// bins and the media filed in them — so undo is exact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InsertBin {
    /// The bin it goes inside.
    pub parent: BinId,
    /// Where in the parent's children it goes.
    pub index: usize,
    /// The bin itself, exactly as it was.
    pub bin: Bin,
}

impl InsertBin {
    /// Inserts `bin` into `parent` at `index`.
    #[must_use]
    pub fn new(parent: BinId, index: usize, bin: Bin) -> Self {
        Self { parent, index, bin }
    }
}

impl Command for InsertBin {
    const KIND: &'static str = "bin.insert";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let id = self.bin.id;
        for existing in self.bin.iter() {
            if project.root_bin.find(existing.id).is_some() {
                return Err(SubError::new(
                    codes::DUPLICATE_BIN,
                    "the bin tree already holds a bin with this identifier",
                )
                .with_detail("bin_id", existing.id));
            }
        }

        let parent = bin_mut(project, self.parent)?;
        check_insert_index(self.index, parent.children.len(), "bin")?;
        parent.children.insert(self.index, self.bin.clone());
        Ok(Inverse::new(RemoveBin::forced(id)))
    }

    fn label(&self) -> String {
        format!("Restore bin {}", self.bin.name)
    }
}

/// Removes a bin and everything inside it.
///
/// A bin still holding media or child bins is refused with
/// `edit.bin_not_empty` unless `force` is set. A forced removal leaves the
/// media items in the project but files them nowhere, which is what
/// [`crate::commands::RemoveMedia`] then reports as an unfiled item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveBin {
    /// The bin to remove.
    pub bin: BinId,
    /// Whether to remove it even though it is not empty.
    #[serde(default)]
    pub force: bool,
}

impl RemoveBin {
    /// Removes `bin`, refusing to if it still holds anything.
    #[must_use]
    pub fn new(bin: BinId) -> Self {
        Self { bin, force: false }
    }

    /// Removes `bin` and everything inside it.
    #[must_use]
    pub fn forced(bin: BinId) -> Self {
        Self { bin, force: true }
    }
}

impl Command for RemoveBin {
    const KIND: &'static str = "bin.remove";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        if self.bin == project.root_bin.id {
            return Err(
                SubError::new(codes::ROOT_BIN, "the root bin cannot be removed")
                    .with_detail("bin_id", self.bin),
            );
        }
        let (parent_id, index) = parent_of(&project.root_bin, self.bin).ok_or_else(|| {
            SubError::new(codes::BIN_NOT_FOUND, "no such bin").with_detail("bin_id", self.bin)
        })?;

        let parent = bin_mut(project, parent_id)?;
        let found = &parent.children[index];
        if !(self.force || found.media.is_empty() && found.children.is_empty()) {
            let (media, children) = (found.media.len(), found.children.len());
            return Err(SubError::new(
                codes::BIN_NOT_EMPTY,
                "the bin still holds media or child bins",
            )
            .with_detail("bin_id", self.bin)
            .with_detail("media", media)
            .with_detail("children", children));
        }

        let removed = parent.children.remove(index);
        Ok(Inverse::new(InsertBin::new(parent_id, index, removed)))
    }

    fn label(&self) -> String {
        "Remove bin".to_owned()
    }
}

/// Renames a bin, the root bin included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenameBin {
    /// The bin to rename.
    pub bin: BinId,
    /// The new name.
    pub name: String,
}

impl RenameBin {
    /// Renames `bin` to `name`.
    #[must_use]
    pub fn new(bin: BinId, name: impl Into<String>) -> Self {
        Self {
            bin,
            name: name.into(),
        }
    }
}

impl Command for RenameBin {
    const KIND: &'static str = "bin.rename";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let found = bin_mut(project, self.bin)?;
        let previous = std::mem::replace(&mut found.name, self.name.clone());
        Ok(Inverse::new(Self::new(self.bin, previous)))
    }

    fn label(&self) -> String {
        format!("Rename bin to {}", self.name)
    }
}

/// Files a media item in another bin.
///
/// The item is lifted out of the bin it is in and inserted into `bin`, at the
/// end unless `index` names a position in the list it lands in *after* the
/// lift. The inverse puts it back in the bin and at the index it held.
///
/// # Errors
///
/// An item filed nowhere — which only a hand-built project has, since
/// [`crate::commands::ImportMedia`] always files what it imports — is refused
/// with `edit.bin_not_found`, because there would be no bin for undo to
/// return it to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveToBin {
    /// The media item to file.
    pub media: MediaId,
    /// The bin it is filed in.
    pub bin: BinId,
    /// Where in that bin's media list it goes. Appended when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

impl MoveToBin {
    /// Files `media` at the end of `bin`.
    #[must_use]
    pub fn new(media: MediaId, bin: BinId) -> Self {
        Self {
            media,
            bin,
            index: None,
        }
    }

    /// The same command, inserting at `index` instead of appending.
    #[must_use]
    pub fn at(mut self, index: usize) -> Self {
        self.index = Some(index);
        self
    }
}

impl Command for MoveToBin {
    const KIND: &'static str = "bin.move_media";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        require_media(project, self.media)?;
        let (previous_bin, previous_index) = filed_at(project, self.media).ok_or_else(|| {
            SubError::new(
                codes::BIN_NOT_FOUND,
                "the media item is not filed in any bin",
            )
            .with_detail("media_id", self.media)
        })?;
        // Resolve the destination and check the index before mutating
        // anything, so a bad target leaves the item where it is.
        let destination = bin_mut(project, self.bin)?;
        let room = if self.bin == previous_bin {
            // The item is lifted out first, so it does not count as a slot.
            destination.media.len() - 1
        } else {
            destination.media.len()
        };
        let index = self.index.unwrap_or(room);
        check_insert_index(index, room, "media")?;

        bin_mut(project, previous_bin)?.media.remove(previous_index);
        bin_mut(project, self.bin)?.media.insert(index, self.media);

        Ok(Inverse::new(
            Self::new(self.media, previous_bin).at(previous_index),
        ))
    }

    fn label(&self) -> String {
        "Move media to bin".to_owned()
    }
}

/// Reparents a bin, carrying everything inside it.
///
/// The bin is lifted out of the parent it is in and inserted into `parent`, at
/// the end unless `index` names a position in the child list it lands in
/// *after* the lift. The inverse puts it back under the parent and at the
/// index it held, so a mis-drag in the bin panel is one undo.
///
/// # Errors
///
/// The root bin is the tree itself and has no parent, so moving it is refused
/// with `edit.root_bin`. Moving a bin into itself or into one of its own
/// descendants would detach the subtree from the tree, and is refused with
/// `edit.invalid_index`: the destination is not a position this bin can hold.
/// An unknown bin or parent is `edit.bin_not_found`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveBin {
    /// The bin to reparent.
    pub bin: BinId,
    /// The bin it goes inside.
    pub parent: BinId,
    /// Where in that parent's children it goes. Appended when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

impl MoveBin {
    /// Files `bin` at the end of `parent`.
    #[must_use]
    pub fn new(bin: BinId, parent: BinId) -> Self {
        Self {
            bin,
            parent,
            index: None,
        }
    }

    /// The same command, inserting at `index` instead of appending.
    #[must_use]
    pub fn at(mut self, index: usize) -> Self {
        self.index = Some(index);
        self
    }
}

impl Command for MoveBin {
    const KIND: &'static str = "bin.move";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        if self.bin == project.root_bin.id {
            return Err(
                SubError::new(codes::ROOT_BIN, "the root bin cannot be reparented")
                    .with_detail("bin_id", self.bin),
            );
        }
        let (previous_parent, previous_index) =
            parent_of(&project.root_bin, self.bin).ok_or_else(|| {
                SubError::new(codes::BIN_NOT_FOUND, "no such bin").with_detail("bin_id", self.bin)
            })?;
        // The destination is resolved, and checked for sitting inside the
        // subtree being moved, before anything is lifted out: a refused move
        // leaves the tree exactly as it was.
        let moving = project.root_bin.find(self.bin).ok_or_else(|| {
            SubError::new(codes::BIN_NOT_FOUND, "no such bin").with_detail("bin_id", self.bin)
        })?;
        if moving.find(self.parent).is_some() {
            return Err(SubError::new(
                codes::INVALID_INDEX,
                "a bin cannot be moved into itself or into its own subtree",
            )
            .with_detail("bin_id", self.bin)
            .with_detail("parent_id", self.parent));
        }
        let destination = bin_mut(project, self.parent)?;
        let room = if self.parent == previous_parent {
            // The bin is lifted out first, so it does not count as a slot.
            destination.children.len() - 1
        } else {
            destination.children.len()
        };
        let index = self.index.unwrap_or(room);
        check_insert_index(index, room, "bin")?;

        let lifted = bin_mut(project, previous_parent)?
            .children
            .remove(previous_index);
        bin_mut(project, self.parent)?
            .children
            .insert(index, lifted);

        Ok(Inverse::new(
            Self::new(self.bin, previous_parent).at(previous_index),
        ))
    }

    fn label(&self) -> String {
        "Move bin".to_owned()
    }
}

/// The bin holding `media` and the position it holds in that bin's list.
pub(super) fn filed_at(project: &Project, media: MediaId) -> Option<(BinId, usize)> {
    let bin = project.root_bin.bin_of(media)?;
    let index = bin.media.iter().position(|id| *id == media)?;
    Some((bin.id, index))
}

/// The bin holding `bin` as a child, and the position it holds.
fn parent_of(root: &Bin, bin: BinId) -> Option<(BinId, usize)> {
    if let Some(index) = root.children.iter().position(|child| child.id == bin) {
        return Some((root.id, index));
    }
    root.children.iter().find_map(|child| parent_of(child, bin))
}
