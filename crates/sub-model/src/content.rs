//! Project-relative media paths and the content hash used to relink them.
//!
//! Projects move between machines, so a `.sub` file never stores an absolute
//! path (docs/PLAN.md §5.6). It stores a [`MediaPath`]: a slash-separated path
//! relative to the directory holding the project file, plus a [`ContentHash`]
//! that identifies the bytes so a moved or renamed file can be found again
//! (TASK-72).
//!
//! ```
//! use sub_model::MediaPath;
//! use std::path::Path;
//!
//! let path = MediaPath::new("footage/interview.mp4").unwrap();
//! assert_eq!(path.file_name(), "interview.mp4");
//! assert_eq!(
//!     path.resolve(Path::new("/projects/doc")),
//!     Path::new("/projects/doc/footage/interview.mp4")
//! );
//! // Absolute paths and escapes out of the project folder are rejected.
//! assert!(MediaPath::new("/etc/passwd").is_err());
//! assert!(MediaPath::new("../secrets.mov").is_err());
//! ```

use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};

use crate::codes;

/// A path to a media file, relative to the directory holding the project file.
///
/// Separators are always `/`, on every platform, so a project authored on
/// Windows opens unchanged on Linux and macOS. The path is validated on
/// construction: it is non-empty, is not absolute, carries no drive letter, and
/// has no `.` or `..` component, so resolving it can never escape the project
/// folder.
///
/// The serde form is the path string itself, validated on the way in by
/// [`MediaPath::new`].
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(into = "String", try_from = "String")]
#[schemars(
    with = "String",
    description = "A slash-separated project-relative media path."
)]
pub struct MediaPath(String);

impl From<MediaPath> for String {
    fn from(path: MediaPath) -> Self {
        path.0
    }
}

impl TryFrom<String> for MediaPath {
    type Error = SubError;

    fn try_from(text: String) -> SubResult<Self> {
        Self::new(text)
    }
}

impl MediaPath {
    /// Validates and normalises a relative path.
    ///
    /// Backslashes are folded to `/` so a Windows-style relative path is
    /// accepted as written.
    ///
    /// # Errors
    ///
    /// Returns `model.invalid_path` when `text` is empty, absolute, contains a
    /// drive letter, has an empty, `.` or `..` component, or contains a NUL.
    pub fn new(text: impl AsRef<str>) -> SubResult<Self> {
        let text = text.as_ref().replace('\\', "/");
        let reject = |reason: &str| {
            Err(SubError::new(
                codes::INVALID_PATH,
                format!("media path must be project-relative: {reason}"),
            )
            .with_detail("path", text.clone()))
        };

        if text.is_empty() {
            return reject("it is empty");
        }
        if text.starts_with('/') {
            return reject("it is absolute");
        }
        if text.contains('\0') {
            return reject("it contains a NUL byte");
        }
        for segment in text.split('/') {
            if segment.is_empty() {
                return reject("it has an empty component");
            }
            if segment == "." || segment == ".." {
                return reject("it has a `.` or `..` component");
            }
            if segment.len() >= 2 && segment.ends_with(':') {
                return reject("it has a drive or scheme prefix");
            }
        }
        Ok(Self(text))
    }

    /// Expresses `file` relative to `project_dir`.
    ///
    /// The comparison is textual first, and only asks the filesystem when that
    /// fails, because on a real machine one folder has more than one spelling:
    /// Windows hands out 8.3 short names (`RUNNER~1`) and, from
    /// [`std::fs::canonicalize`], verbatim `\\?\C:\...` paths, and on macOS the
    /// temp folder is a symlink (`/var` to `/private/var`). A host that
    /// canonicalised the project folder while the file dialog did not would
    /// otherwise refuse every import of a file sitting right beside the
    /// project file.
    ///
    /// # Errors
    ///
    /// Returns `model.invalid_path` when `file` is not inside `project_dir`, or
    /// when the resulting relative path is not representable as UTF-8.
    pub fn relative_to(project_dir: &Path, file: &Path) -> SubResult<Self> {
        let outside = || {
            SubError::new(
                codes::INVALID_PATH,
                "media file is not inside the project folder",
            )
            .with_detail("path", file.display().to_string())
            .with_detail("project_dir", project_dir.display().to_string())
        };

        let relative = if let Ok(relative) = file.strip_prefix(project_dir) {
            relative.to_path_buf()
        } else {
            let (Some(dir), Some(real)) = (real_path(project_dir), real_path(file)) else {
                return Err(outside());
            };
            let Ok(stripped) = real.strip_prefix(&dir) else {
                return Err(outside());
            };
            stripped.to_path_buf()
        };

        let mut segments = Vec::new();
        for component in relative.components() {
            match component {
                Component::Normal(part) => {
                    let part = part.to_str().ok_or_else(|| {
                        SubError::new(codes::INVALID_PATH, "media path is not valid UTF-8")
                            .with_detail("path", file.display().to_string())
                    })?;
                    segments.push(part);
                }
                Component::CurDir => {}
                _ => {
                    return Err(SubError::new(
                        codes::INVALID_PATH,
                        "media path must not leave the project folder",
                    )
                    .with_detail("path", file.display().to_string()));
                }
            }
        }
        Self::new(segments.join("/"))
    }

    /// The path as stored: slash-separated and relative.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The final component, usually the file name.
    #[must_use]
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// The absolute path of this file, given the directory holding the project.
    #[must_use]
    pub fn resolve(&self, project_dir: &Path) -> PathBuf {
        let mut path = project_dir.to_path_buf();
        for segment in self.0.split('/') {
            path.push(segment);
        }
        path
    }
}

impl fmt::Display for MediaPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `path` as the filesystem spells it, or `None` when it cannot say.
///
/// [`std::fs::canonicalize`] resolves symlinks, `.` and `..`, Windows 8.3 short
/// names and the letter case the volume actually stores, which is what makes
/// two spellings of one folder comparable. It needs the path to exist, so a
/// file that has not been written yet is resolved through its folder and the
/// name is put back on: a relink target that has moved away still has a real
/// parent to resolve against.
#[must_use]
pub fn real_path(path: &Path) -> Option<PathBuf> {
    if let Ok(real) = std::fs::canonicalize(path) {
        return Some(plain_path(real));
    }
    let parent = path.parent()?;
    let name = path.file_name()?;
    let real = std::fs::canonicalize(parent).ok()?;
    Some(plain_path(real).join(name))
}

/// The same path without Windows' verbatim `\\?\` prefix.
///
/// `canonicalize` returns one on Windows, and it is contagious: it leaks into
/// error details the user reads, into the `file://` URI the decoder is handed,
/// and into every comparison against a path that came from a file dialog. A
/// UNC path (`\\?\UNC\server\share`) is left alone, because dropping its prefix
/// would change which file it names.
#[must_use]
pub fn plain_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        use std::path::Prefix;
        let mut components = path.components();
        if let Some(Component::Prefix(prefix)) = components.next()
            && matches!(prefix.kind(), Prefix::VerbatimDisk(_))
        {
            // `C:` plus the rest; the root is still in `components`.
            let text = prefix.as_os_str().to_string_lossy();
            let disk = text.trim_start_matches(r"\\?\");
            return Path::new(disk).join(components.as_path());
        }
    }
    path
}

/// The number of bytes hashed at each end of a file.
const CHUNK_BYTES: u64 = 1 << 20;

/// A fingerprint of a media file's bytes, used to relink moved sources.
///
/// It is the BLAKE3 hash of the file length followed by the first mebibyte and
/// the last mebibyte of the file (docs/PLAN.md §5.6). Hashing the ends rather
/// than the whole file keeps import of a hundred-gigabyte card fast while still
/// discriminating between takes: media containers differ in their header and in
/// their trailing index, and the length is mixed in so two files sharing both
/// ends still differ.
///
/// Files of 2 MiB or less are hashed in full, with no overlap and no gap.
///
/// This is a relink hint, not a security primitive: it does not prove two files
/// are identical, only that they are very likely the same source.
///
/// The serde form is the 64-character lowercase hex string.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(into = "String", try_from = "String")]
#[schemars(
    with = "String",
    description = "A BLAKE3 content hash as 64 lowercase hex digits."
)]
pub struct ContentHash([u8; 32]);

impl From<ContentHash> for String {
    fn from(hash: ContentHash) -> Self {
        hash.to_string()
    }
}

impl TryFrom<String> for ContentHash {
    type Error = SubError;

    fn try_from(text: String) -> SubResult<Self> {
        Self::parse(&text)
    }
}

impl ContentHash {
    /// Hashes the file at `path`.
    ///
    /// # Errors
    ///
    /// Returns `model.file_unreadable` if the file cannot be opened, measured
    /// or read.
    pub fn of_file(path: &Path) -> SubResult<Self> {
        let unreadable = |err: &std::io::Error| {
            SubError::wrap(codes::FILE_UNREADABLE, "could not hash media file", err)
                .with_detail("path", path.display().to_string())
        };

        let mut file = File::open(path).map_err(|err| unreadable(&err))?;
        let len = file.metadata().map_err(|err| unreadable(&err))?.len();

        let mut hasher = blake3::Hasher::new();
        hasher.update(&len.to_le_bytes());

        let head_len = len.min(CHUNK_BYTES);
        let mut buffer = vec![0_u8; usize::try_from(head_len).unwrap_or(usize::MAX)];
        file.read_exact(&mut buffer)
            .map_err(|err| unreadable(&err))?;
        hasher.update(&buffer);

        let tail_start = head_len.max(len.saturating_sub(CHUNK_BYTES));
        if tail_start < len {
            let tail_len = len - tail_start;
            buffer.resize(usize::try_from(tail_len).unwrap_or(usize::MAX), 0);
            file.seek(SeekFrom::Start(tail_start))
                .map_err(|err| unreadable(&err))?;
            file.read_exact(&mut buffer)
                .map_err(|err| unreadable(&err))?;
            hasher.update(&buffer);
        }

        Ok(Self(*hasher.finalize().as_bytes()))
    }

    /// Wraps raw hash bytes, for tests, migrations and importers.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw hash bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Parses the 64-character lowercase hex form.
    ///
    /// # Errors
    ///
    /// Returns `model.invalid_hash` when `text` is not 64 hex digits.
    pub fn parse(text: &str) -> SubResult<Self> {
        blake3::Hash::from_hex(text)
            .map(|hash| Self(*hash.as_bytes()))
            .map_err(|err| {
                SubError::wrap(
                    codes::INVALID_HASH,
                    "content hash must be 64 hex digits",
                    &err,
                )
                .with_detail("hash", text.to_owned())
            })
    }
}

impl fmt::Display for ContentHash {
    /// Writes the 64-character lowercase hex form.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(blake3::Hash::from_bytes(self.0).to_hex().as_str())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    /// Writes `bytes` to a uniquely named file under the temp directory.
    fn write_temp(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sub-model-{}-{name}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut file = File::create(&path).unwrap();
        file.write_all(bytes).unwrap();
        path
    }

    #[test]
    fn relative_paths_are_normalised_and_resolved() {
        let path = MediaPath::new("footage\\day one/a.mp4").unwrap();
        assert_eq!(path.as_str(), "footage/day one/a.mp4");
        assert_eq!(path.file_name(), "a.mp4");
        assert_eq!(path.to_string(), "footage/day one/a.mp4");
        assert_eq!(
            path.resolve(Path::new("/p/doc")),
            PathBuf::from("/p/doc/footage/day one/a.mp4")
        );
    }

    #[test]
    fn absolute_and_escaping_paths_are_rejected() {
        for bad in [
            "",
            "/abs/a.mp4",
            "C:/media/a.mp4",
            "c:\\media\\a.mp4",
            "../a.mp4",
            "footage/../../a.mp4",
            "footage/./a.mp4",
            "footage//a.mp4",
            "a\0.mp4",
        ] {
            let err = MediaPath::new(bad).unwrap_err();
            assert_eq!(err.code, codes::INVALID_PATH, "accepted {bad:?}");
        }
    }

    #[test]
    fn paths_are_derived_relative_to_the_project_folder() {
        let dir = Path::new("/p/doc");
        let path = MediaPath::relative_to(dir, Path::new("/p/doc/footage/a.mp4")).unwrap();
        assert_eq!(path.as_str(), "footage/a.mp4");

        let err = MediaPath::relative_to(dir, Path::new("/elsewhere/a.mp4")).unwrap_err();
        assert_eq!(err.code, codes::INVALID_PATH);
    }

    /// The bug the Windows CI run found: the host canonicalises the project
    /// folder and the file dialog does not, so the two spellings of one folder
    /// have to compare equal or every import is refused.
    ///
    /// `canonicalize` is what the host uses, so it is what this asks for: on
    /// Windows it answers with a verbatim `\\?\C:\...` path and resolves 8.3
    /// short names, and on macOS it resolves `/var` to `/private/var`. On
    /// Linux the two spellings usually already agree, and the test still
    /// proves the fallback does not break that.
    #[test]
    fn a_canonicalised_project_folder_still_matches_the_path_a_dialog_hands_back() {
        let dir = std::env::temp_dir().join("sub-model-canonical-import");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("footage")).expect("a project folder");
        let file = dir.join("footage").join("take.mov");
        std::fs::write(&file, b"take").expect("the file writes");

        let canonical = std::fs::canonicalize(&dir).expect("the folder resolves");
        let path = MediaPath::relative_to(&canonical, &file).expect("the file is inside");
        assert_eq!(path.as_str(), "footage/take.mov");

        // A file that is not there yet resolves through its folder, which is
        // what a relink target that has moved away needs.
        let gone = dir.join("footage").join("gone.mov");
        let path = MediaPath::relative_to(&canonical, &gone).expect("the folder is inside");
        assert_eq!(path.as_str(), "footage/gone.mov");

        // Resolving both sides never turns an outsider into an insider.
        let outside = std::env::temp_dir().join("sub-model-canonical-elsewhere.mov");
        let err = MediaPath::relative_to(&canonical, &outside).unwrap_err();
        assert_eq!(err.code, codes::INVALID_PATH);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same mismatch the Windows runner hits, reproduced where it can be:
    /// one folder reached by two spellings, the project folder resolved and
    /// the chosen file not. A symlink is how a Unix machine spells that, and
    /// it is literally what macOS' temp folder is.
    #[cfg(unix)]
    #[test]
    fn a_file_reached_through_a_symlinked_folder_is_still_inside_the_project() {
        let base = std::env::temp_dir().join("sub-model-symlinked-import");
        let _ = std::fs::remove_dir_all(&base);
        let real = base.join("real");
        std::fs::create_dir_all(real.join("footage")).expect("a project folder");
        std::fs::write(real.join("footage").join("take.mov"), b"take").expect("the file writes");
        let link = base.join("link");
        std::os::unix::fs::symlink(&real, &link).expect("the link is made");

        // What the host holds: the folder as the filesystem spells it.
        let project_dir = std::fs::canonicalize(&real).expect("the folder resolves");
        // What a file dialog hands back: the folder as the user reached it.
        let chosen = link.join("footage").join("take.mov");
        assert!(
            chosen.strip_prefix(&project_dir).is_err(),
            "the two spellings must differ for this to be testing anything"
        );

        let path = MediaPath::relative_to(&project_dir, &chosen).expect("the file is inside");
        assert_eq!(path.as_str(), "footage/take.mov");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_plain_path_survives_unchanged() {
        let dir = std::env::temp_dir();
        let plain = plain_path(std::fs::canonicalize(&dir).expect("temp resolves"));
        assert!(
            !plain.to_string_lossy().starts_with(r"\\?\"),
            "a verbatim prefix reaches GLib and every path comparison: {plain:?}"
        );
        assert_eq!(
            plain_path(PathBuf::from("footage")),
            PathBuf::from("footage")
        );
    }

    #[test]
    fn small_files_are_hashed_whole_and_distinguished() {
        let a = write_temp("a.bin", b"hello subordinate");
        let b = write_temp("b.bin", b"hello subordinate!");
        let copy = write_temp("copy.bin", b"hello subordinate");

        let hash = ContentHash::of_file(&a).unwrap();
        assert_eq!(hash, ContentHash::of_file(&copy).unwrap());
        assert_ne!(hash, ContentHash::of_file(&b).unwrap());
        assert_eq!(ContentHash::parse(&hash.to_string()).unwrap(), hash);
        assert_eq!(ContentHash::from_bytes(*hash.as_bytes()), hash);
    }

    #[test]
    fn large_files_are_distinguished_by_both_ends_and_by_size() {
        let chunk = usize::try_from(CHUNK_BYTES).unwrap();
        let mut base = vec![7_u8; chunk * 3];
        base[0] = 1;
        base[chunk * 3 - 1] = 2;

        let mut head_changed = base.clone();
        head_changed[0] = 9;
        let mut tail_changed = base.clone();
        tail_changed[chunk * 3 - 1] = 9;
        let mut longer = base.clone();
        longer.insert(chunk, 3);

        let base_hash = ContentHash::of_file(&write_temp("base.bin", &base)).unwrap();
        assert_eq!(
            base_hash,
            ContentHash::of_file(&write_temp("same.bin", &base)).unwrap()
        );
        assert_ne!(
            base_hash,
            ContentHash::of_file(&write_temp("head.bin", &head_changed)).unwrap()
        );
        assert_ne!(
            base_hash,
            ContentHash::of_file(&write_temp("tail.bin", &tail_changed)).unwrap()
        );
        assert_ne!(
            base_hash,
            ContentHash::of_file(&write_temp("longer.bin", &longer)).unwrap()
        );
    }

    #[test]
    fn hashing_a_missing_file_reports_a_stable_code() {
        let err = ContentHash::of_file(Path::new("/nonexistent/sub-model/a.mp4")).unwrap_err();
        assert_eq!(err.code, codes::FILE_UNREADABLE);
        assert!(err.cause.is_some());
    }

    #[test]
    fn malformed_hex_is_rejected() {
        assert_eq!(
            ContentHash::parse("beef").unwrap_err().code,
            codes::INVALID_HASH
        );
    }
}
