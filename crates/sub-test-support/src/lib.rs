//! Shared test helpers: locating and describing the generated media fixtures.
//!
//! Media binaries are never committed. `scripts/gen-fixtures.sh` (or
//! `scripts/gen-fixtures.ps1` on Windows) synthesises a deterministic set with
//! GStreamer into `fixtures/`, which is gitignored, together with a
//! `manifest.json` describing every fixture. Tests reach that set through this
//! crate rather than hard-coding paths.
//!
//! ```no_run
//! # fn main() -> Result<(), sub_test_support::FixtureError> {
//! let clip = sub_test_support::fixture("bars_1080p_h264.mp4")?;
//! assert!(clip.is_file());
//! # Ok(())
//! # }
//! ```
//!
//! Fixtures that were not generated (the ten-minute long-GOP clip is skipped
//! unless the generator is run with `--long`) are still listed in the manifest
//! with `generated: false`, so a test can skip itself instead of failing:
//!
//! ```no_run
//! if let Some(clip) = sub_test_support::try_fixture("longgop_720p_10min.mp4") {
//!     // exercise long-GOP seeking
//!     let _ = clip;
//! }
//! ```

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Environment variable that overrides where fixtures are looked for.
pub const FIXTURES_DIR_ENV: &str = "SUB_FIXTURES_DIR";

/// File name of the manifest written next to the fixtures.
pub const MANIFEST_FILE_NAME: &str = "manifest.json";

/// Manifest schema version this crate understands.
pub const MANIFEST_VERSION: u32 = 1;

/// Advice printed with every error, so a failing test says how to recover.
const HINT: &str = "run scripts/gen-fixtures.sh (scripts/gen-fixtures.ps1 on Windows) \
                    to generate the test media fixtures";

/// What kind of media a fixture holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FixtureKind {
    /// A video file, possibly with no audio track.
    Video,
    /// An audio-only file.
    Audio,
}

/// One entry of the fixture manifest.
///
/// Durations are exact integer nanosecond counts and frame rates are exact
/// rationals; no timing value in a fixture description is ever a float.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fixture {
    /// File name inside the fixtures directory.
    pub name: String,
    /// Whether the fixture is video or audio only.
    pub kind: FixtureKind,
    /// Pixel width, or zero for audio-only fixtures.
    pub width: u32,
    /// Pixel height, or zero for audio-only fixtures.
    pub height: u32,
    /// Exact duration in nanoseconds.
    pub duration_ns: u64,
    /// Numerator of the nominal frame rate, or zero for audio-only fixtures.
    pub fps_num: u32,
    /// Denominator of the nominal frame rate.
    pub fps_den: u32,
    /// True when frame durations genuinely vary across the file.
    pub vfr: bool,
    /// False when the generator skipped this fixture, for example the
    /// ten-minute clip without `--long`.
    pub generated: bool,
    /// Human-readable description of what the fixture contains.
    pub description: String,
}

impl Fixture {
    /// Nominal frame rate as an exact `(numerator, denominator)` rational.
    ///
    /// For a variable-frame-rate fixture this is the peak rate the file was
    /// authored at, not a rate every frame follows.
    pub fn fps(&self) -> (u32, u32) {
        (self.fps_num, self.fps_den)
    }
}

/// The parsed contents of `fixtures/manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Schema version of the manifest.
    pub version: u32,
    /// Which generator script wrote the manifest.
    pub generator: String,
    /// Every fixture in the catalogue, generated or not.
    pub fixtures: Vec<Fixture>,
}

impl Manifest {
    /// Looks a fixture up by file name.
    pub fn get(&self, name: &str) -> Option<&Fixture> {
        self.fixtures.iter().find(|f| f.name == name)
    }

    /// Every fixture the generator actually wrote.
    pub fn generated(&self) -> impl Iterator<Item = &Fixture> {
        self.fixtures.iter().filter(|f| f.generated)
    }

    /// Parses a manifest from JSON text.
    ///
    /// # Errors
    ///
    /// Returns [`FixtureError::ManifestInvalid`] if the JSON does not parse or
    /// carries a schema version this crate does not understand.
    pub fn from_json(json: &str) -> Result<Self, FixtureError> {
        let manifest: Self =
            serde_json::from_str(json).map_err(|e| FixtureError::ManifestInvalid {
                reason: e.to_string(),
            })?;
        if manifest.version != MANIFEST_VERSION {
            return Err(FixtureError::ManifestInvalid {
                reason: format!(
                    "manifest version {} is not the supported version {MANIFEST_VERSION}",
                    manifest.version
                ),
            });
        }
        Ok(manifest)
    }
}

/// Why a fixture could not be produced.
///
/// Each variant carries a stable string code through [`FixtureError::code`],
/// matching the project-wide convention that errors are identified by code
/// rather than by message text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixtureError {
    /// No `manifest.json` in the fixtures directory.
    ManifestMissing {
        /// Where the manifest was looked for.
        path: PathBuf,
    },
    /// The manifest exists but could not be read.
    ManifestUnreadable {
        /// Where the manifest was looked for.
        path: PathBuf,
        /// The underlying I/O error, rendered.
        reason: String,
    },
    /// The manifest could not be parsed or has an unsupported version.
    ManifestInvalid {
        /// What was wrong with it.
        reason: String,
    },
    /// The manifest has no entry with that name.
    Unknown {
        /// The requested fixture name.
        name: String,
    },
    /// The fixture is in the catalogue but was deliberately not generated.
    NotGenerated {
        /// The requested fixture name.
        name: String,
    },
    /// The manifest claims the fixture exists but the file is not there.
    FileMissing {
        /// The requested fixture name.
        name: String,
        /// Where the file was expected.
        path: PathBuf,
    },
}

impl FixtureError {
    /// Stable machine-readable code for this error.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ManifestMissing { .. } => "fixtures.manifest_missing",
            Self::ManifestUnreadable { .. } => "fixtures.manifest_unreadable",
            Self::ManifestInvalid { .. } => "fixtures.manifest_invalid",
            Self::Unknown { .. } => "fixtures.unknown",
            Self::NotGenerated { .. } => "fixtures.not_generated",
            Self::FileMissing { .. } => "fixtures.file_missing",
        }
    }
}

impl fmt::Display for FixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ManifestMissing { path } => {
                write!(f, "no fixture manifest at {}; {HINT}", path.display())
            }
            Self::ManifestUnreadable { path, reason } => {
                write!(f, "cannot read {}: {reason}; {HINT}", path.display())
            }
            Self::ManifestInvalid { reason } => {
                write!(f, "invalid fixture manifest: {reason}; {HINT}")
            }
            Self::Unknown { name } => write!(f, "no fixture named '{name}' in the manifest"),
            Self::NotGenerated { name } => write!(
                f,
                "fixture '{name}' was not generated; re-run the generator with --long"
            ),
            Self::FileMissing { name, path } => write!(
                f,
                "fixture '{name}' is missing from {}; {HINT}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for FixtureError {}

/// Directory holding the generated fixtures.
///
/// `SUB_FIXTURES_DIR` wins if it is set; otherwise it is `fixtures/` at the
/// workspace root, resolved from this crate's own location so the answer does
/// not depend on the current working directory.
pub fn fixtures_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(FIXTURES_DIR_ENV) {
        return PathBuf::from(dir);
    }
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = crate_dir
        .parent()
        .and_then(Path::parent)
        .unwrap_or(crate_dir);
    workspace_root.join("fixtures")
}

/// Path of the fixture manifest inside [`fixtures_dir`].
pub fn manifest_path() -> PathBuf {
    fixtures_dir().join(MANIFEST_FILE_NAME)
}

/// Loads the manifest from [`fixtures_dir`].
///
/// # Errors
///
/// Returns a [`FixtureError`] if the manifest is absent, unreadable, malformed
/// or of an unsupported version.
pub fn load_manifest() -> Result<Manifest, FixtureError> {
    load_manifest_from(&fixtures_dir())
}

/// Loads the manifest from an explicit fixtures directory.
///
/// # Errors
///
/// Returns a [`FixtureError`] if the manifest is absent, unreadable, malformed
/// or of an unsupported version.
pub fn load_manifest_from(dir: &Path) -> Result<Manifest, FixtureError> {
    let path = dir.join(MANIFEST_FILE_NAME);
    let json = match std::fs::read_to_string(&path) {
        Ok(json) => json,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(FixtureError::ManifestMissing { path });
        }
        Err(e) => {
            return Err(FixtureError::ManifestUnreadable {
                path,
                reason: e.to_string(),
            });
        }
    };
    Manifest::from_json(&json)
}

/// Absolute path of a generated fixture, checked against the manifest.
///
/// # Errors
///
/// Returns a [`FixtureError`] if the manifest cannot be loaded, the name is not
/// in the catalogue, the fixture was skipped by the generator, or the file is
/// not on disk.
pub fn fixture(name: &str) -> Result<PathBuf, FixtureError> {
    fixture_from(&fixtures_dir(), name)
}

/// Absolute path of a generated fixture inside an explicit fixtures directory.
///
/// # Errors
///
/// Returns a [`FixtureError`] if the manifest cannot be loaded, the name is not
/// in the catalogue, the fixture was skipped by the generator, or the file is
/// not on disk.
pub fn fixture_from(dir: &Path, name: &str) -> Result<PathBuf, FixtureError> {
    let manifest = load_manifest_from(dir)?;
    let entry = manifest.get(name).ok_or_else(|| FixtureError::Unknown {
        name: name.to_owned(),
    })?;
    if !entry.generated {
        return Err(FixtureError::NotGenerated {
            name: name.to_owned(),
        });
    }
    let path = dir.join(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(FixtureError::FileMissing {
            name: name.to_owned(),
            path,
        })
    }
}

/// Path of a fixture, or `None` when it is unavailable for any reason.
///
/// Lets a test skip itself when an optional fixture, such as the ten-minute
/// long-GOP clip, was not generated.
pub fn try_fixture(name: &str) -> Option<PathBuf> {
    fixture(name).ok()
}

#[cfg(test)]
mod tests {
    use super::{FixtureError, FixtureKind, Manifest, fixture_from, load_manifest_from};
    use std::path::{Path, PathBuf};

    const SAMPLE: &str = r#"{
      "version": 1,
      "generator": "scripts/gen-fixtures.sh",
      "fixtures": [
        {
          "name": "bars_1080p_h264.mp4", "kind": "video", "width": 1920, "height": 1080,
          "duration_ns": 5000000000, "fps_num": 25, "fps_den": 1,
          "vfr": false, "generated": true, "description": "bars"
        },
        {
          "name": "vfr_60_30.mkv", "kind": "video", "width": 1280, "height": 720,
          "duration_ns": 6000000000, "fps_num": 60, "fps_den": 1,
          "vfr": true, "generated": true, "description": "vfr"
        },
        {
          "name": "longgop_720p_10min.mp4", "kind": "video", "width": 1280, "height": 720,
          "duration_ns": 600000000000, "fps_num": 25, "fps_den": 1,
          "vfr": false, "generated": false, "description": "long"
        },
        {
          "name": "tone_48k_stereo.wav", "kind": "audio", "width": 0, "height": 0,
          "duration_ns": 5000000000, "fps_num": 0, "fps_den": 1,
          "vfr": false, "generated": true, "description": "tone"
        }
      ]
    }"#;

    /// Writes SAMPLE plus the named empty fixture files into a fresh directory.
    fn scratch(files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sub-fixtures-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        std::fs::write(dir.join(super::MANIFEST_FILE_NAME), SAMPLE).expect("manifest");
        for f in files {
            std::fs::write(dir.join(f), b"x").expect("fixture file");
        }
        dir
    }

    #[test]
    fn manifest_parses_with_exact_integer_timing() {
        let manifest = Manifest::from_json(SAMPLE).expect("sample manifest parses");
        assert_eq!(manifest.version, super::MANIFEST_VERSION);
        assert_eq!(manifest.fixtures.len(), 4);
        assert_eq!(manifest.generated().count(), 3);

        let bars = manifest.get("bars_1080p_h264.mp4").expect("bars entry");
        assert_eq!(bars.kind, FixtureKind::Video);
        assert_eq!(bars.duration_ns, 5_000_000_000);
        assert_eq!(bars.fps(), (25, 1));
        assert!(!bars.vfr);

        let tone = manifest.get("tone_48k_stereo.wav").expect("tone entry");
        assert_eq!(tone.kind, FixtureKind::Audio);
        assert_eq!(tone.width, 0);

        assert!(manifest.get("vfr_60_30.mkv").expect("vfr entry").vfr);
        assert!(manifest.get("nope.mp4").is_none());
    }

    #[test]
    fn manifest_of_a_future_version_is_rejected() {
        let bumped = SAMPLE.replace("\"version\": 1", "\"version\": 99");
        let err = Manifest::from_json(&bumped).expect_err("version 99 must be rejected");
        assert_eq!(err.code(), "fixtures.manifest_invalid");
    }

    #[test]
    fn malformed_manifest_is_rejected() {
        let err = Manifest::from_json("{ not json").expect_err("garbage must be rejected");
        assert_eq!(err.code(), "fixtures.manifest_invalid");
    }

    #[test]
    fn missing_manifest_reports_where_it_looked() {
        let dir = std::env::temp_dir().join("sub-fixtures-definitely-absent");
        let _ = std::fs::remove_dir_all(&dir);
        let err = load_manifest_from(&dir).expect_err("absent manifest must error");
        assert_eq!(err.code(), "fixtures.manifest_missing");
        assert!(err.to_string().contains("gen-fixtures"));
    }

    #[test]
    fn lookup_distinguishes_unknown_skipped_and_absent() {
        let dir = scratch(&["bars_1080p_h264.mp4"]);

        let found = fixture_from(&dir, "bars_1080p_h264.mp4").expect("generated fixture resolves");
        assert_eq!(found, dir.join("bars_1080p_h264.mp4"));

        let cases: [(&str, &str); 3] = [
            ("nope.mp4", "fixtures.unknown"),
            ("longgop_720p_10min.mp4", "fixtures.not_generated"),
            ("vfr_60_30.mkv", "fixtures.file_missing"),
        ];
        for (name, code) in cases {
            let err = fixture_from(&dir, name).expect_err("must not resolve");
            assert_eq!(err.code(), code, "wrong code for {name}");
        }

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn default_fixtures_dir_sits_at_the_workspace_root() {
        // The override is read from the environment, which tests must not
        // mutate, so only the default is asserted here.
        if std::env::var_os(super::FIXTURES_DIR_ENV).is_none() {
            let dir = super::fixtures_dir();
            assert_eq!(
                dir.file_name().and_then(std::ffi::OsStr::to_str),
                Some("fixtures")
            );
            let root = dir.parent().expect("workspace root");
            assert!(
                root.join("Cargo.toml").is_file(),
                "{root:?} is not the workspace root"
            );
            assert!(root.join("scripts/gen-fixtures.sh").is_file());
        }
    }

    #[test]
    fn error_codes_are_distinct() {
        let path = Path::new("/tmp/x").to_path_buf();
        let errors = [
            FixtureError::ManifestMissing { path: path.clone() },
            FixtureError::ManifestUnreadable {
                path: path.clone(),
                reason: "e".into(),
            },
            FixtureError::ManifestInvalid { reason: "e".into() },
            FixtureError::Unknown { name: "n".into() },
            FixtureError::NotGenerated { name: "n".into() },
            FixtureError::FileMissing {
                name: "n".into(),
                path,
            },
        ];
        let mut codes: Vec<&str> = errors.iter().map(FixtureError::code).collect();
        codes.sort_unstable();
        let unique = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), unique, "error codes must be unique");
        assert!(codes.iter().all(|c| c.starts_with("fixtures.")));
    }
}
