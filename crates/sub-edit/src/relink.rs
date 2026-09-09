//! Finding moved media files again, and relinking the items that point at
//! them.
//!
//! A project stores project-relative paths and a [`ContentHash`] for every
//! source file (docs/PLAN.md §5.6), which is what makes relinking mechanical:
//! the bytes identify the file, so a renamed, re-foldered or re-copied take is
//! found without asking the user which take it was. This module is the search
//! and the plan; the mutation is still an ordinary [`RelinkMedia`] command, so
//! nothing here bypasses undo.
//!
//! The order is fixed: **hash first, name second**. A file whose hash equals
//! the item's is the same source, whatever it is now called. Only when the
//! item was never hashed, or nothing in the searched folder matches its hash,
//! does the file name decide — and a name match is a guess, which is why
//! [`RelinkMatch::kind`] says which of the two happened, so the dialog can
//! show it.
//!
//! A file is claimed by at most one item, and items are considered in project
//! order, so a folder holding two copies of the same take relinks two items to
//! two files rather than both to the first.
//!
//! ```no_run
//! use std::path::Path;
//! use sub_edit::History;
//! use sub_edit::relink::{RelinkPlan, SearchOptions, match_offline, scan_folder};
//! use sub_model::Project;
//!
//! # fn main() -> sub_core::SubResult<()> {
//! let mut project = Project::new("Doc cut");
//! let dir = Path::new("/projects/doc");
//! project.refresh_offline(dir);
//!
//! let files = scan_folder(dir, SearchOptions::default());
//! let matches = match_offline(&project, &files);
//! let plan = RelinkPlan::build(dir, &matches);
//!
//! let mut history = History::new();
//! plan.apply(&mut history, &mut project)?;
//! # Ok(())
//! # }
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sub_core::{SubError, SubResult};
use sub_model::{ContentHash, MediaId, MediaItem, MediaPath, Project};

use crate::History;
use crate::commands::RelinkMedia;

/// How deep a folder search descends by default.
///
/// Deep enough for the card-and-day folder layouts footage arrives in, shallow
/// enough that pointing the search at a home directory does not walk a whole
/// machine.
pub const DEFAULT_MAX_DEPTH: usize = 8;

/// How many files a folder search looks at by default.
pub const DEFAULT_MAX_FILES: usize = 20_000;

/// The label a bulk relink appears under in the undo menu.
pub const RELINK_GROUP_LABEL: &str = "Relink media";

/// The bounds a folder search runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions {
    /// How many folder levels below the root are visited. Zero searches the
    /// root folder itself and none of its subfolders.
    pub max_depth: usize,
    /// How many files are collected before the search stops.
    pub max_files: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            max_files: DEFAULT_MAX_FILES,
        }
    }
}

impl SearchOptions {
    /// The default bounds.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The same bounds, descending `max_depth` levels below the root.
    #[must_use]
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// The same bounds, stopping after `max_files` files.
    #[must_use]
    pub fn with_max_files(mut self, max_files: usize) -> Self {
        self.max_files = max_files;
        self
    }
}

/// Every file under `root`, breadth first and in name order within a folder.
///
/// The walk is bounded by [`SearchOptions`] and never descends a symbolic link
/// to a folder, so a loop in the filesystem cannot hang the search. Folders it
/// cannot read are skipped: a search that half works is more useful than one
/// that refuses.
#[must_use]
pub fn scan_folder(root: &Path, options: SearchOptions) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut level = vec![root.to_path_buf()];
    let mut depth = 0;

    while !level.is_empty() && files.len() < options.max_files {
        let mut next = Vec::new();
        for dir in level {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut here: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect();
            here.sort();
            for path in here {
                // `symlink_metadata` rather than `metadata`: a link to a
                // folder is not descended into, and a link to a file is left
                // for the hash to read or to fail on.
                let Ok(meta) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                if meta.is_dir() {
                    if depth < options.max_depth {
                        next.push(path);
                    }
                } else if files.len() < options.max_files {
                    files.push(path);
                }
            }
        }
        level = next;
        depth += 1;
    }
    files
}

/// Why a file was proposed for an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    /// The file's bytes hash to what the item recorded: the same source.
    Hash,
    /// The file carries the item's file name. A guess, offered only when no
    /// hash matched.
    Name,
}

impl MatchKind {
    /// A word for the dialog: `hash` or `name`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Hash => "hash",
            Self::Name => "name",
        }
    }
}

/// A file a search proposes for one offline item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelinkMatch {
    /// The item that would be relinked.
    pub media: MediaId,
    /// The file it would be pointed at.
    pub file: PathBuf,
    /// Whether the hash or the name found it.
    pub kind: MatchKind,
    /// The file's hash, when it could be read. A name match on a file that
    /// cannot be hashed carries `None` and relinks without one.
    pub hash: Option<ContentHash>,
}

/// What a search knows about one item it is looking for.
///
/// The search runs on a worker thread — hashing a folder of footage is not
/// something the UI thread may do — so it takes these rather than a borrowed
/// [`Project`]: identity, the name to fall back to, and the bytes to look for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelinkTarget {
    /// The item to relink.
    pub media: MediaId,
    /// The file name it had, used when no hash matches.
    pub file_name: String,
    /// The hash recorded when it was last read, if it ever was.
    pub hash: Option<ContentHash>,
}

impl RelinkTarget {
    /// What to look for on behalf of `item`.
    #[must_use]
    pub fn for_item(item: &MediaItem) -> Self {
        Self {
            media: item.id,
            file_name: item.path.file_name().to_owned(),
            hash: item.hash,
        }
    }

    /// Every offline item in `project`, in project order.
    #[must_use]
    pub fn offline(project: &Project) -> Vec<Self> {
        project.offline_media().map(Self::for_item).collect()
    }

    /// The one item `media`, whether or not it is flagged offline.
    ///
    /// Relinking an online item is legitimate: a source can be replaced by a
    /// graded or re-wrapped version of itself.
    #[must_use]
    pub fn one(project: &Project, media: MediaId) -> Option<Self> {
        project.media_item(media).map(Self::for_item)
    }
}

/// The file the user picked for `target`, whatever it is called.
///
/// An explicit choice is never refused: the file is hashed so the item records
/// what it now points at, and [`RelinkMatch::kind`] reports [`MatchKind::Hash`]
/// only when those bytes really are the ones the item remembered.
#[must_use]
pub fn match_chosen(target: &RelinkTarget, file: &Path) -> RelinkMatch {
    let hash = ContentHash::of_file(file).ok();
    let kind = if hash.is_some() && hash == target.hash {
        MatchKind::Hash
    } else {
        MatchKind::Name
    };
    RelinkMatch {
        media: target.media,
        file: file.to_path_buf(),
        kind,
        hash,
    }
}

/// Proposes a file for every offline item in `project`.
///
/// A convenience over [`match_targets`] for a caller that already holds the
/// project.
#[must_use]
pub fn match_offline(project: &Project, files: &[PathBuf]) -> Vec<RelinkMatch> {
    match_targets(&RelinkTarget::offline(project), files)
}

/// Proposes a file for each of `targets`.
///
/// `files` is what [`scan_folder`] returned. Files are hashed at most once
/// each, and only when some target has a hash to compare against; a project
/// whose media was already offline when it was saved therefore costs a name
/// search and no reading at all.
#[must_use]
pub fn match_targets(targets: &[RelinkTarget], files: &[PathBuf]) -> Vec<RelinkMatch> {
    let offline = targets;
    if offline.is_empty() || files.is_empty() {
        return Vec::new();
    }

    let hashes = if offline.iter().any(|item| item.hash.is_some()) {
        hash_files(files)
    } else {
        BTreeMap::new()
    };

    let mut matches: Vec<RelinkMatch> = Vec::new();
    let mut claimed: Vec<PathBuf> = Vec::new();

    // Hash first: an exact match is the same source, whatever it is called.
    for item in offline {
        let Some(wanted) = item.hash else { continue };
        let Some(file) = hashes
            .get(&wanted)
            .and_then(|found| found.iter().find(|file| !claimed.contains(file)))
            .cloned()
        else {
            continue;
        };
        claimed.push(file.clone());
        matches.push(RelinkMatch {
            media: item.media,
            file,
            kind: MatchKind::Hash,
            hash: Some(wanted),
        });
    }

    // Name second, for whatever the hashes did not account for.
    for item in offline {
        if matches.iter().any(|found| found.media == item.media) {
            continue;
        }
        let wanted = item.file_name.as_str();
        let Some(file) = files
            .iter()
            .find(|file| !claimed.contains(file) && file_name_matches(file, wanted))
            .cloned()
        else {
            continue;
        };
        let hash = hash_of(&hashes, &file).or_else(|| ContentHash::of_file(&file).ok());
        claimed.push(file.clone());
        matches.push(RelinkMatch {
            media: item.media,
            file,
            kind: MatchKind::Name,
            hash,
        });
    }

    matches
}

/// The hash already computed for `file`, if the hash pass read it.
fn hash_of(hashes: &BTreeMap<ContentHash, Vec<PathBuf>>, file: &Path) -> Option<ContentHash> {
    hashes
        .iter()
        .find(|(_, found)| found.iter().any(|candidate| candidate == file))
        .map(|(hash, _)| *hash)
}

/// True when `file` is called `wanted`, ignoring ASCII case.
///
/// Case is ignored because a card copied through a Windows machine comes back
/// as `TAKE1.MOV`, and a case-sensitive search would miss it.
fn file_name_matches(file: &Path, wanted: &str) -> bool {
    file.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|name| name.eq_ignore_ascii_case(wanted))
}

/// Hashes each file, in the order given, skipping those that cannot be read.
///
/// Several files can share a hash — a take copied twice — so each hash keeps
/// every file that produced it and the match claims them one at a time.
fn hash_files(files: &[PathBuf]) -> BTreeMap<ContentHash, Vec<PathBuf>> {
    let mut hashes: BTreeMap<ContentHash, Vec<PathBuf>> = BTreeMap::new();
    for file in files {
        if let Ok(hash) = ContentHash::of_file(file) {
            hashes.entry(hash).or_default().push(file.clone());
        }
    }
    hashes
}

/// What a search decided, ready to apply as one undo step.
///
/// [`RelinkPlan::build`] turns matches into commands, which is where the
/// project-relative path rule bites: a file outside the project folder cannot
/// be stored, so it is refused with `model.invalid_path` and reported in
/// [`RelinkPlan::rejected`] rather than silently dropped.
#[derive(Debug, Default)]
pub struct RelinkPlan {
    /// The commands to apply, in project order.
    pub relinks: Vec<RelinkMedia>,
    /// The items a file was found for but which could not be relinked, and
    /// why.
    pub rejected: Vec<(MediaId, SubError)>,
}

impl RelinkPlan {
    /// Turns `matches` into commands, expressed relative to `project_dir`.
    #[must_use]
    pub fn build(project_dir: &Path, matches: &[RelinkMatch]) -> Self {
        let mut plan = Self::default();
        for found in matches {
            match MediaPath::relative_to(project_dir, &found.file) {
                Ok(path) => {
                    let mut command = RelinkMedia::new(found.media, path);
                    command.hash = found.hash;
                    plan.relinks.push(command);
                }
                Err(error) => plan.rejected.push((found.media, error)),
            }
        }
        plan
    }

    /// How many items the plan would relink.
    #[must_use]
    pub fn len(&self) -> usize {
        self.relinks.len()
    }

    /// True when nothing would be relinked.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.relinks.is_empty()
    }

    /// Applies every relink as a single undo step, and returns how many ran.
    ///
    /// An empty plan applies nothing and pushes no history entry. A failure
    /// part way through rolls the whole group back, so the project is never
    /// left half relinked.
    ///
    /// # Errors
    ///
    /// Returns `edit.group_open` when a group is already open, and whatever
    /// [`RelinkMedia`] reports — `edit.media_not_found` for an item the
    /// project no longer holds — after rolling the group back.
    pub fn apply(&self, history: &mut History, project: &mut Project) -> SubResult<usize> {
        if self.relinks.is_empty() {
            return Ok(0);
        }
        history.begin_group(RELINK_GROUP_LABEL)?;
        for command in &self.relinks {
            history.apply(project, command.clone())?;
        }
        history.commit_group()?;
        Ok(self.relinks.len())
    }
}
