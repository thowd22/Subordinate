//! Export presets: data, not code (docs/PLAN.md §5.5).
//!
//! A preset is one named answer to "what file should this sequence become":
//! the container, the video codec with its resolution, frame rate and either
//! an average bitrate or a constant-quality CRF, and the audio codec with its
//! bitrate, sample rate and channel count. The shipped presets live in
//! `presets/builtin.toml` beside this crate and are embedded with
//! `include_str!`, so the built-ins and a user's own file go through exactly
//! the same parser and exactly the same validation — which is what makes
//! presets an extension point rather than a hard-coded list.
//!
//! A user file named [`PRESETS_FILE_NAME`] in the config directory
//! ([`config_dir`]) is loaded on top of the built-ins: a preset with a new id
//! is added, a preset with a built-in's id replaces it. A missing file is the
//! ordinary case and not an error; a file that is present but wrong is an
//! error, and the [`SubError`] names the field that is wrong.
//!
//! Frame rates are written as an exact numerator and denominator, never a
//! decimal, so 23.976 fps is `24000/1001` and stays exact all the way to the
//! [`Rational`] the pipeline times frames with.
//!
//! ```
//! use sub_export::presets::PresetLibrary;
//!
//! let library = PresetLibrary::builtin();
//! let preset = library.require("youtube-1080p").unwrap();
//! assert_eq!(preset.video.as_ref().unwrap().width, 1920);
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};
use sub_time::Rational;

use crate::chroma::ChromaFormat;
use crate::codes;
use crate::encoder::VideoCodec;
use crate::pipeline::{AudioCodec, Container, ExportSettings, MAX_CRF, VideoQuality};

/// The environment variable that overrides where the config directory is.
///
/// It is the same variable the editor's keymap and layout files honour, so a
/// portable install points every per-user file at one directory.
pub const CONFIG_DIR_ENV: &str = "SUBORDINATE_CONFIG_DIR";

/// The name of the user preset file inside the config directory.
pub const PRESETS_FILE_NAME: &str = "presets.toml";

/// The shipped presets, embedded at compile time.
const BUILTIN_TOML: &str = include_str!("../presets/builtin.toml");

/// The most audio channels a preset may ask for.
const MAX_CHANNELS: u16 = 8;

/// The sample rate a preset gets when it does not say.
const DEFAULT_SAMPLE_RATE: u32 = 48_000;

/// The channel count a preset gets when it does not say.
const DEFAULT_CHANNELS: u16 = 2;

/// The video half of a preset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoPreset {
    /// The codec to encode.
    pub codec: VideoCodec,
    /// Canvas width in pixels, even because the chroma planes are subsampled.
    pub width: u32,
    /// Canvas height in pixels, even for the same reason.
    pub height: u32,
    /// The frame rate, exact.
    pub frame_rate: Rational,
    /// The bitrate or CRF the encoder is driven with.
    pub quality: VideoQuality,
    /// The chroma format the encoder is fed.
    ///
    /// 4:2:0 when the preset does not say, because that is what plays
    /// everywhere; a mezzanine or master preset may ask for more, and the
    /// export then fails by name if the chosen encoder cannot take it.
    pub chroma: ChromaFormat,
}

/// The audio half of a preset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioPreset {
    /// The codec to encode.
    pub codec: AudioCodec,
    /// The bitrate in kbit/s for a lossy codec; `None` for a lossless one,
    /// which is the only case where it may be left out.
    pub bitrate_kbps: Option<u32>,
    /// The sample rate handed to the encoder, in hertz.
    pub sample_rate: u32,
    /// Channels per audio frame.
    pub channels: u16,
}

/// One named export target.
///
/// At least one of [`Preset::video`] and [`Preset::audio`] is present: the
/// audio-only preset has no video, and a silent delivery has no audio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    /// The stable identifier, used by the CLI, the MCP tools and the panel.
    pub id: String,
    /// The human-readable name.
    pub name: String,
    /// The container the streams are wrapped in.
    pub container: Container,
    /// The video stream, or `None` for an audio-only preset.
    pub video: Option<VideoPreset>,
    /// The audio stream, or `None` for a silent preset.
    pub audio: Option<AudioPreset>,
}

impl Preset {
    /// The export settings this preset asks for.
    ///
    /// # Errors
    ///
    /// [`codes::UNSUPPORTED_COMBINATION`] for an audio-only preset: the export
    /// pipeline is driven by composited frames and has no video-less form yet.
    /// The settings are validated before they are returned, so the container
    /// and codec errors of [`ExportSettings::validate`] surface here too.
    pub fn to_settings(&self) -> SubResult<ExportSettings> {
        let video = self.video.as_ref().ok_or_else(|| {
            SubError::new(
                codes::UNSUPPORTED_COMBINATION,
                format!("preset '{}' has no video stream to export", self.id),
            )
            .with_detail("preset", self.id.clone())
            .with_detail("field", "video_codec")
        })?;
        let mut settings =
            ExportSettings::new(video.width, video.height, video.frame_rate, self.container)
                .with_video_codec(video.codec)
                .with_chroma(video.chroma)
                .with_video_quality(Some(video.quality))
                .with_audio_codec(self.audio.as_ref().map(|audio| audio.codec));
        if let Some(audio) = &self.audio {
            settings = settings
                .with_audio_format(audio.sample_rate, audio.channels)
                .with_audio_bitrate(audio.bitrate_kbps);
        }
        settings.validate()?;
        Ok(settings)
    }

    /// This preset written back out in the file's own shape.
    ///
    /// # Errors
    ///
    /// [`codes::PRESET_INVALID`] if the preset cannot be written as TOML,
    /// which a preset built by this module never is.
    pub fn to_toml_string(&self) -> SubResult<String> {
        let file = RawFile {
            preset: vec![RawPreset::from(self)],
        };
        toml::to_string(&file).map_err(|error| {
            SubError::new(
                codes::PRESET_INVALID,
                format!("preset '{}' could not be written as TOML: {error}", self.id),
            )
            .with_detail("preset", self.id.clone())
        })
    }
}

/// The presets available to this session: the built-ins, plus the user's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetLibrary {
    presets: Vec<Preset>,
    source: Option<PathBuf>,
}

impl PresetLibrary {
    /// The presets shipped with the editor.
    ///
    /// # Panics
    ///
    /// If the embedded `presets/builtin.toml` is not valid, which a test in
    /// this module makes a build failure rather than a run-time one.
    #[must_use]
    pub fn builtin() -> Self {
        Self::from_toml(BUILTIN_TOML).expect("the shipped presets must be valid")
    }

    /// The presets a TOML document describes.
    ///
    /// # Errors
    ///
    /// - [`codes::PRESET_INVALID`] when the document is not TOML of the preset
    ///   shape, or when a preset's field is missing, empty, out of range or
    ///   contradictory. The error's `field` detail names the field.
    pub fn from_toml(text: &str) -> SubResult<Self> {
        let file: RawFile = toml::from_str(text).map_err(|error| {
            SubError::new(
                codes::PRESET_INVALID,
                format!("the preset file could not be read: {error}"),
            )
        })?;
        let mut presets: Vec<Preset> = Vec::with_capacity(file.preset.len());
        for raw in &file.preset {
            let preset = raw.validate()?;
            if presets.iter().any(|other| other.id == preset.id) {
                return Err(invalid(&preset.id, "id", "two presets share one id"));
            }
            presets.push(preset);
        }
        Ok(Self {
            presets,
            source: None,
        })
    }

    /// The built-ins with `path`'s presets applied, when the file exists.
    ///
    /// A missing file leaves the built-ins alone: most users never write one.
    ///
    /// # Errors
    ///
    /// - [`codes::PRESET_UNREADABLE`] when the file exists but cannot be read.
    /// - The parse and validation errors of [`PresetLibrary::from_toml`], with
    ///   the file's path attached.
    pub fn from_path(path: &Path) -> SubResult<Self> {
        let mut library = Self::builtin();
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(library),
            Err(error) => {
                return Err(SubError::new(
                    codes::PRESET_UNREADABLE,
                    format!("{} could not be read: {error}", path.display()),
                )
                .with_detail("path", path.display().to_string()));
            }
        };
        let user = Self::from_toml(&text)
            .map_err(|error| error.with_detail("path", path.display().to_string()))?;
        library.merge(user);
        library.source = Some(path.to_path_buf());
        Ok(library)
    }

    /// The built-ins plus the user presets in `dir`, if that file exists.
    ///
    /// # Errors
    ///
    /// The errors of [`PresetLibrary::from_path`].
    pub fn from_dir(dir: &Path) -> SubResult<Self> {
        Self::from_path(&dir.join(PRESETS_FILE_NAME))
    }

    /// The built-ins plus the user presets from the config directory.
    ///
    /// # Errors
    ///
    /// The errors of [`PresetLibrary::from_path`]. A machine with no config
    /// directory at all simply gets the built-ins.
    pub fn load() -> SubResult<Self> {
        match presets_path() {
            Some(path) => Self::from_path(&path),
            None => Ok(Self::builtin()),
        }
    }

    /// Adds `other`'s presets, replacing any built-in with the same id in
    /// place so the shipped order is kept.
    fn merge(&mut self, other: Self) {
        for preset in other.presets {
            match self
                .presets
                .iter_mut()
                .find(|existing| existing.id == preset.id)
            {
                Some(existing) => *existing = preset,
                None => self.presets.push(preset),
            }
        }
    }

    /// The file the user presets came from, when there was one.
    #[must_use]
    pub fn source(&self) -> Option<&Path> {
        self.source.as_deref()
    }

    /// The preset with this id, if the library has one.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Preset> {
        self.presets.iter().find(|preset| preset.id == id)
    }

    /// The preset with this id.
    ///
    /// # Errors
    ///
    /// [`codes::PRESET_UNKNOWN`], listing the ids there are, which is what an
    /// agent or a CLI user needs to fix the call.
    pub fn require(&self, id: &str) -> SubResult<&Preset> {
        self.get(id).ok_or_else(|| {
            SubError::new(codes::PRESET_UNKNOWN, format!("no export preset '{id}'"))
                .with_detail("preset", id)
                .with_detail("known", self.ids())
        })
    }

    /// Every preset, in file order: built-ins first, then the user's own.
    pub fn iter(&self) -> impl Iterator<Item = &Preset> {
        self.presets.iter()
    }

    /// Every preset id, in the same order.
    #[must_use]
    pub fn ids(&self) -> Vec<&str> {
        self.presets
            .iter()
            .map(|preset| preset.id.as_str())
            .collect()
    }

    /// How many presets there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.presets.len()
    }

    /// Whether the library is empty, which only an empty file produces.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.presets.is_empty()
    }

    /// The whole library written back out in the file's own shape.
    ///
    /// # Errors
    ///
    /// [`codes::PRESET_INVALID`] if the presets cannot be written as TOML.
    pub fn to_toml_string(&self) -> SubResult<String> {
        let file = RawFile {
            preset: self.presets.iter().map(RawPreset::from).collect(),
        };
        toml::to_string(&file).map_err(|error| {
            SubError::new(
                codes::PRESET_INVALID,
                format!("the presets could not be written as TOML: {error}"),
            )
        })
    }
}

impl<'a> IntoIterator for &'a PresetLibrary {
    type Item = &'a Preset;
    type IntoIter = std::slice::Iter<'a, Preset>;

    fn into_iter(self) -> Self::IntoIter {
        self.presets.iter()
    }
}

impl Default for PresetLibrary {
    fn default() -> Self {
        Self::builtin()
    }
}

/// The editor's config directory, or `None` when the platform gives no home.
///
/// [`CONFIG_DIR_ENV`] wins when it is set to a non-empty value. Otherwise it
/// is `$XDG_CONFIG_HOME/subordinate` (falling back to `~/.config`) on Unix,
/// `%APPDATA%\subordinate` on Windows, and
/// `~/Library/Application Support/subordinate` on macOS — the same directory
/// the keymap and the dock layout use.
#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    config_dir_from(&non_empty_env)
}

/// The path `presets.toml` is read from, if there is a config directory.
#[must_use]
pub fn presets_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(PRESETS_FILE_NAME))
}

/// [`config_dir`] against an arbitrary environment, so the tests can describe
/// a Windows or a home-less machine without mutating this process's
/// environment, which is unsafe and would race the other tests.
fn config_dir_from(lookup: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(dir) = lookup(CONFIG_DIR_ENV) {
        return Some(PathBuf::from(dir));
    }
    platform_config_dir(lookup).map(|base| base.join("subordinate"))
}

/// The value of `name` in the environment, if it is set and not blank.
fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// The platform's per-user configuration root, before the app name is joined.
fn platform_config_dir(lookup: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        lookup("APPDATA").map(PathBuf::from)
    }
    #[cfg(target_os = "macos")]
    {
        lookup("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        lookup("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| lookup("HOME").map(|home| PathBuf::from(home).join(".config")))
    }
}

/// A validation failure that names the preset and the offending field.
fn invalid(id: &str, field: &str, message: &str) -> SubError {
    SubError::new(
        codes::PRESET_INVALID,
        format!("preset '{id}': {field}: {message}"),
    )
    .with_detail("preset", id)
    .with_detail("field", field)
}

/// The preset file exactly as it is written: one array of tables.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    /// The `[[preset]]` entries, in file order.
    #[serde(default)]
    preset: Vec<RawPreset>,
}

/// One `[[preset]]` table, with every field still as the file wrote it.
///
/// Parsing into strings and plain integers, rather than straight into the
/// typed [`Preset`], is what lets every failure name its own field instead of
/// handing the user a serde message about a variant it could not pick.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPreset {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    container: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    video_codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_rate_numerator: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_rate_denominator: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    video_bitrate_kbps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    video_crf: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chroma: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_bitrate_kbps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    channels: Option<u16>,
}

impl RawPreset {
    /// Checks every field and builds the typed preset.
    fn validate(&self) -> SubResult<Preset> {
        let id = self.id.trim();
        if id.is_empty() {
            return Err(invalid("", "id", "a preset needs an id"));
        }
        if !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(invalid(
                id,
                "id",
                "an id is lower-case ASCII letters, digits and hyphens",
            ));
        }
        let name = match &self.name {
            Some(name) if name.trim().is_empty() => {
                return Err(invalid(id, "name", "a name cannot be blank"));
            }
            Some(name) => name.trim().to_owned(),
            None => id.to_owned(),
        };
        let container = Container::parse(self.container.trim()).ok_or_else(|| {
            invalid(
                id,
                "container",
                &format!(
                    "'{}' is not a container this exporter writes",
                    self.container
                ),
            )
        })?;
        let video = self.validate_video(id)?;
        let audio = self.validate_audio(id)?;
        if video.is_none() && audio.is_none() {
            return Err(invalid(
                id,
                "video_codec",
                "a preset needs a video codec, an audio codec, or both",
            ));
        }
        if let Some(video) = &video
            && !container.accepts_video(video.codec)
        {
            return Err(invalid(
                id,
                "video_codec",
                &format!("{container} cannot carry {}", video.codec.label()),
            ));
        }
        if let Some(audio) = &audio
            && !container.accepts_audio(audio.codec)
        {
            return Err(invalid(
                id,
                "audio_codec",
                &format!("{container} cannot carry {}", audio.codec.label()),
            ));
        }
        Ok(Preset {
            id: id.to_owned(),
            name,
            container,
            video,
            audio,
        })
    }

    /// The video half, or `None` when the preset names no video field at all.
    fn validate_video(&self, id: &str) -> SubResult<Option<VideoPreset>> {
        let mentions_video = self.video_codec.is_some()
            || self.width.is_some()
            || self.height.is_some()
            || self.frame_rate_numerator.is_some()
            || self.frame_rate_denominator.is_some()
            || self.video_bitrate_kbps.is_some()
            || self.video_crf.is_some()
            || self.chroma.is_some();
        if !mentions_video {
            return Ok(None);
        }
        let name = self
            .video_codec
            .as_deref()
            .ok_or_else(|| invalid(id, "video_codec", "the video fields need a codec"))?;
        let codec = VideoCodec::parse(name.trim()).ok_or_else(|| {
            invalid(
                id,
                "video_codec",
                &format!("'{name}' is not a codec this exporter encodes"),
            )
        })?;
        let width = self
            .width
            .ok_or_else(|| invalid(id, "width", "a video preset needs a width"))?;
        let height = self
            .height
            .ok_or_else(|| invalid(id, "height", "a video preset needs a height"))?;
        check_dimension(id, "width", width)?;
        check_dimension(id, "height", height)?;
        let numerator = self.frame_rate_numerator.ok_or_else(|| {
            invalid(
                id,
                "frame_rate_numerator",
                "a video preset needs a frame rate numerator",
            )
        })?;
        let denominator = self.frame_rate_denominator.unwrap_or(1);
        let frame_rate = Rational::new(numerator, denominator).ok_or_else(|| {
            invalid(
                id,
                if numerator == 0 {
                    "frame_rate_numerator"
                } else {
                    "frame_rate_denominator"
                },
                "a frame rate is strictly positive",
            )
        })?;
        let quality = match (self.video_bitrate_kbps, self.video_crf) {
            (Some(kbps), None) => {
                if kbps == 0 {
                    return Err(invalid(
                        id,
                        "video_bitrate_kbps",
                        "a bitrate is strictly positive",
                    ));
                }
                VideoQuality::Bitrate { kbps }
            }
            (None, Some(value)) => {
                if value > MAX_CRF {
                    return Err(invalid(
                        id,
                        "video_crf",
                        &format!("a CRF is 0 through {MAX_CRF}"),
                    ));
                }
                VideoQuality::Crf { value }
            }
            (Some(_), Some(_)) => {
                return Err(invalid(
                    id,
                    "video_bitrate_kbps",
                    "a preset sets a bitrate or a CRF, never both",
                ));
            }
            (None, None) => {
                return Err(invalid(
                    id,
                    "video_bitrate_kbps",
                    "a video preset needs a bitrate or a CRF",
                ));
            }
        };
        let chroma = match self.chroma.as_deref() {
            None => ChromaFormat::default(),
            Some(name) => ChromaFormat::parse(name).ok_or_else(|| {
                invalid(
                    id,
                    "chroma",
                    &format!("'{name}' is not a chroma format this exporter encodes"),
                )
            })?,
        };
        Ok(Some(VideoPreset {
            codec,
            width,
            height,
            frame_rate,
            quality,
            chroma,
        }))
    }

    /// The audio half, or `None` when the preset names no audio field at all.
    fn validate_audio(&self, id: &str) -> SubResult<Option<AudioPreset>> {
        let mentions_audio = self.audio_codec.is_some()
            || self.audio_bitrate_kbps.is_some()
            || self.sample_rate.is_some()
            || self.channels.is_some();
        if !mentions_audio {
            return Ok(None);
        }
        let name = self
            .audio_codec
            .as_deref()
            .ok_or_else(|| invalid(id, "audio_codec", "the audio fields need a codec"))?;
        let codec = AudioCodec::parse(name.trim()).ok_or_else(|| {
            invalid(
                id,
                "audio_codec",
                &format!("'{name}' is not a codec this exporter encodes"),
            )
        })?;
        let lossless = codec == AudioCodec::Flac;
        let bitrate_kbps = match (self.audio_bitrate_kbps, lossless) {
            (Some(_), true) => {
                return Err(invalid(
                    id,
                    "audio_bitrate_kbps",
                    "a lossless codec takes no bitrate",
                ));
            }
            (Some(0), false) => {
                return Err(invalid(
                    id,
                    "audio_bitrate_kbps",
                    "a bitrate is strictly positive",
                ));
            }
            (Some(kbps), false) => Some(kbps),
            (None, true) => None,
            (None, false) => {
                return Err(invalid(
                    id,
                    "audio_bitrate_kbps",
                    "a lossy codec needs a bitrate",
                ));
            }
        };
        let sample_rate = self.sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE);
        if sample_rate == 0 {
            return Err(invalid(
                id,
                "sample_rate",
                "a sample rate is strictly positive",
            ));
        }
        let channels = self.channels.unwrap_or(DEFAULT_CHANNELS);
        if channels == 0 || channels > MAX_CHANNELS {
            return Err(invalid(
                id,
                "channels",
                &format!("a channel count is 1 through {MAX_CHANNELS}"),
            ));
        }
        Ok(Some(AudioPreset {
            codec,
            bitrate_kbps,
            sample_rate,
            channels,
        }))
    }
}

/// Checks one canvas dimension: non-zero, and even for chroma subsampling.
fn check_dimension(id: &str, field: &str, value: u32) -> SubResult<()> {
    if value == 0 {
        return Err(invalid(
            id,
            field,
            "a canvas dimension is strictly positive",
        ));
    }
    if !value.is_multiple_of(2) {
        return Err(invalid(
            id,
            field,
            "a canvas dimension is even, because the chroma planes are subsampled",
        ));
    }
    Ok(())
}

impl From<&Preset> for RawPreset {
    fn from(preset: &Preset) -> Self {
        let video = preset.video.as_ref();
        let audio = preset.audio.as_ref();
        Self {
            id: preset.id.clone(),
            name: Some(preset.name.clone()),
            container: preset.container.as_str().to_owned(),
            video_codec: video.map(|video| video.codec.as_str().to_owned()),
            width: video.map(|video| video.width),
            height: video.map(|video| video.height),
            frame_rate_numerator: video.map(|video| video.frame_rate.numerator()),
            frame_rate_denominator: video.map(|video| video.frame_rate.denominator()),
            video_bitrate_kbps: video.and_then(|video| match video.quality {
                VideoQuality::Bitrate { kbps } => Some(kbps),
                VideoQuality::Crf { .. } => None,
            }),
            video_crf: video.and_then(|video| match video.quality {
                VideoQuality::Crf { value } => Some(value),
                VideoQuality::Bitrate { .. } => None,
            }),
            chroma: video.map(|video| video.chroma.as_str().to_owned()),
            audio_codec: audio.map(|audio| audio.codec.as_str().to_owned()),
            audio_bitrate_kbps: audio.and_then(|audio| audio.bitrate_kbps),
            sample_rate: audio.map(|audio| audio.sample_rate),
            channels: audio.map(|audio| audio.channels),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AudioPreset, CONFIG_DIR_ENV, PRESETS_FILE_NAME, Preset, PresetLibrary, VideoQuality,
        config_dir_from,
    };
    use crate::chroma::ChromaFormat;
    use crate::codes;
    use crate::encoder::VideoCodec;
    use crate::pipeline::{AudioCodec, Container};
    use std::path::PathBuf;
    use sub_time::Rational;

    /// An environment built from a list of pairs, for `config_dir_from`.
    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .filter(|value| !value.trim().is_empty())
        }
    }

    /// A one-preset document with `body` as the fields after the id.
    fn document(body: &str) -> String {
        format!("[[preset]]\nid = \"custom\"\n{body}\n")
    }

    /// The `field` detail of the error `body` produces.
    fn failing_field(body: &str) -> String {
        let error = PresetLibrary::from_toml(&document(body)).expect_err("should be rejected");
        assert_eq!(error.code, codes::PRESET_INVALID, "{}", error.message);
        error
            .details
            .get("field")
            .expect("every preset error names a field")
            .as_str()
            .expect("the field detail is a string")
            .to_owned()
    }

    #[test]
    fn the_shipped_presets_load_and_cover_the_plan() {
        let library = PresetLibrary::builtin();
        assert_eq!(
            library.ids(),
            vec![
                "youtube-1080p",
                "youtube-4k",
                "mezzanine",
                "h265-archive",
                "av1-archive",
                "audio-only"
            ]
        );
        assert!(library.source().is_none());
        assert!(!library.is_empty());
        assert_eq!(library.len(), library.iter().count());
    }

    #[test]
    fn youtube_1080p_is_bitrate_h264_in_mp4() {
        let library = PresetLibrary::builtin();
        let preset = library.require("youtube-1080p").expect("shipped");
        assert_eq!(preset.container, Container::Mp4);
        let video = preset.video.as_ref().expect("has video");
        assert_eq!(video.codec, VideoCodec::H264);
        assert_eq!((video.width, video.height), (1920, 1080));
        assert_eq!(video.frame_rate, Rational::FPS_30);
        assert_eq!(video.quality, VideoQuality::Bitrate { kbps: 12_000 });
        let audio = preset.audio.as_ref().expect("has audio");
        assert_eq!(audio.codec, AudioCodec::Aac);
        assert_eq!(audio.bitrate_kbps, Some(192));
        assert_eq!((audio.sample_rate, audio.channels), (48_000, 2));
    }

    #[test]
    fn youtube_4k_is_the_same_shape_at_uhd() {
        let library = PresetLibrary::builtin();
        let video = library
            .require("youtube-4k")
            .expect("shipped")
            .video
            .clone()
            .expect("has video");
        assert_eq!((video.width, video.height), (3840, 2160));
        assert!(matches!(video.quality, VideoQuality::Bitrate { .. }));
    }

    #[test]
    fn the_mezzanine_is_lossless_video_and_lossless_audio() {
        let library = PresetLibrary::builtin();
        let preset = library.require("mezzanine").expect("shipped");
        assert_eq!(
            preset.video.as_ref().expect("has video").quality,
            VideoQuality::Crf { value: 0 }
        );
        let audio = preset.audio.as_ref().expect("has audio");
        assert_eq!(audio.codec, AudioCodec::Flac);
        assert_eq!(audio.bitrate_kbps, None, "FLAC takes no bitrate");
    }

    #[test]
    fn a_preset_hands_its_quality_to_the_export_settings() {
        let library = PresetLibrary::builtin();
        let youtube = library
            .require("youtube-1080p")
            .expect("the built-ins carry youtube-1080p");
        let settings = youtube.to_settings().expect("a video preset has settings");
        assert_eq!(
            settings.video_quality,
            Some(VideoQuality::Bitrate { kbps: 12_000 }),
            "the preset's bitrate must reach the encoder"
        );
        assert_eq!(settings.audio_bitrate_kbps, Some(192));

        for preset in library.iter() {
            let Some(video) = preset.video.as_ref() else {
                continue;
            };
            let settings = preset.to_settings().expect("a video preset has settings");
            assert_eq!(
                settings.video_quality,
                Some(video.quality),
                "preset '{}' loses its quality on the way to the pipeline",
                preset.id
            );
            assert_eq!(
                settings.audio_bitrate_kbps,
                preset.audio.as_ref().and_then(|audio| audio.bitrate_kbps),
                "preset '{}' loses its audio bitrate",
                preset.id
            );
        }
    }

    #[test]
    fn two_presets_of_different_quality_make_different_settings() {
        let library = PresetLibrary::builtin();
        let youtube = library
            .require("youtube-1080p")
            .expect("the built-ins carry youtube-1080p")
            .to_settings()
            .expect("settings");
        let mezzanine = library
            .require("mezzanine")
            .expect("the built-ins carry mezzanine")
            .to_settings()
            .expect("settings");
        assert_ne!(
            youtube.video_quality, mezzanine.video_quality,
            "the delivery and the master preset must not ask the encoder for the same thing"
        );
    }

    #[test]
    fn the_archive_preset_is_crf_h265() {
        let library = PresetLibrary::builtin();
        let preset = library.require("h265-archive").expect("shipped");
        assert_eq!(preset.container, Container::Mkv);
        let video = preset.video.as_ref().expect("has video");
        assert_eq!(video.codec, VideoCodec::H265);
        assert!(matches!(video.quality, VideoQuality::Crf { .. }));
    }

    #[test]
    fn the_av1_preset_is_av1_in_matroska() {
        let library = PresetLibrary::builtin();
        let preset = library.require("av1-archive").expect("shipped");
        assert_eq!(preset.container, Container::Mkv);
        let video = preset.video.as_ref().expect("has video");
        assert_eq!(video.codec, VideoCodec::Av1);
        assert_eq!(
            preset.audio.as_ref().expect("has audio").codec,
            AudioCodec::Opus
        );
    }

    #[test]
    fn the_audio_only_preset_has_no_video_at_all() {
        let library = PresetLibrary::builtin();
        let preset = library.require("audio-only").expect("shipped");
        assert!(preset.video.is_none());
        assert_eq!(
            preset.audio.as_ref().expect("has audio").codec,
            AudioCodec::Aac
        );
        let error = preset.to_settings().expect_err("nothing to encode");
        assert_eq!(error.code, codes::UNSUPPORTED_COMBINATION);
    }

    #[test]
    fn every_video_preset_converts_to_settings_that_validate() {
        let library = PresetLibrary::builtin();
        for preset in &library {
            let Some(video) = preset.video.as_ref() else {
                continue;
            };
            let settings = preset.to_settings().expect("shipped presets are writable");
            assert_eq!(settings.width, video.width);
            assert_eq!(settings.height, video.height);
            assert_eq!(settings.frame_rate, video.frame_rate);
            assert_eq!(settings.container, preset.container);
            assert_eq!(settings.video_codec, video.codec);
            assert_eq!(
                settings.audio_codec,
                preset.audio.as_ref().map(|audio| audio.codec)
            );
            settings.validate().expect("valid");
        }
    }

    #[test]
    fn an_unknown_id_names_the_ids_there_are() {
        let library = PresetLibrary::builtin();
        assert!(library.get("nope").is_none());
        let error = library.require("nope").expect_err("no such preset");
        assert_eq!(error.code, codes::PRESET_UNKNOWN);
        assert!(error.details.contains_key("known"));
    }

    #[test]
    fn a_fractional_frame_rate_stays_exact() {
        let library = PresetLibrary::from_toml(&document(
            "container = \"mkv\"\nvideo_codec = \"h264\"\nwidth = 1920\nheight = 1080\n\
             frame_rate_numerator = 24000\nframe_rate_denominator = 1001\nvideo_crf = 18",
        ))
        .expect("valid");
        let rate = library.require("custom").expect("present").video.clone();
        assert_eq!(rate.expect("has video").frame_rate, Rational::FPS_23_976);
    }

    #[test]
    fn a_missing_frame_rate_denominator_means_whole_frames_per_second() {
        let library = PresetLibrary::from_toml(&document(
            "container = \"mkv\"\nvideo_codec = \"h264\"\nwidth = 640\nheight = 480\n\
             frame_rate_numerator = 25\nvideo_crf = 18",
        ))
        .expect("valid");
        assert_eq!(
            library
                .require("custom")
                .expect("present")
                .video
                .as_ref()
                .expect("has video")
                .frame_rate,
            Rational::FPS_25
        );
    }

    #[test]
    fn every_broken_field_is_named_in_the_error() {
        let video = "container = \"mkv\"\nvideo_codec = \"h264\"\nwidth = 1920\nheight = 1080\n\
                     frame_rate_numerator = 30\n";
        let cases: [(&str, &str); 12] = [
            ("container = \"avi\"", "container"),
            (
                "container = \"mkv\"\nname = \"  \"\nvideo_codec = \"h264\"\nwidth = 16\nheight = 16\nframe_rate_numerator = 30\nvideo_crf = 18",
                "name",
            ),
            (
                "container = \"mkv\"\nvideo_codec = \"vp9\"\nwidth = 16\nheight = 16\nframe_rate_numerator = 30\nvideo_crf = 18",
                "video_codec",
            ),
            (
                "container = \"mkv\"\nvideo_codec = \"h264\"\nwidth = 0\nheight = 16\nframe_rate_numerator = 30\nvideo_crf = 18",
                "width",
            ),
            (
                "container = \"mkv\"\nvideo_codec = \"h264\"\nwidth = 16\nheight = 1081\nframe_rate_numerator = 30\nvideo_crf = 18",
                "height",
            ),
            (
                "container = \"mkv\"\nvideo_codec = \"h264\"\nwidth = 16\nheight = 16\nvideo_crf = 18",
                "frame_rate_numerator",
            ),
            (
                "container = \"mkv\"\nvideo_codec = \"h264\"\nwidth = 16\nheight = 16\nframe_rate_numerator = 30\nframe_rate_denominator = 0\nvideo_crf = 18",
                "frame_rate_denominator",
            ),
            (&format!("{video}video_crf = 52"), "video_crf"),
            (
                &format!("{video}video_crf = 18\nvideo_bitrate_kbps = 5000"),
                "video_bitrate_kbps",
            ),
            (video, "video_bitrate_kbps"),
            (
                &format!("{video}video_crf = 18\naudio_codec = \"flac\"\naudio_bitrate_kbps = 128"),
                "audio_bitrate_kbps",
            ),
            (
                &format!(
                    "{video}video_crf = 18\naudio_codec = \"aac\"\naudio_bitrate_kbps = 128\nchannels = 0"
                ),
                "channels",
            ),
        ];
        for (body, field) in cases {
            assert_eq!(failing_field(body), field, "for body: {body}");
        }
    }

    #[test]
    fn a_container_that_cannot_carry_the_codec_is_rejected() {
        assert_eq!(
            failing_field(
                "container = \"mov\"\nvideo_codec = \"av1\"\nwidth = 1920\nheight = 1080\n\
                 frame_rate_numerator = 30\nvideo_crf = 18"
            ),
            "video_codec"
        );
        assert_eq!(
            failing_field(
                "container = \"mp4\"\nvideo_codec = \"h264\"\nwidth = 1920\nheight = 1080\n\
                 frame_rate_numerator = 30\nvideo_crf = 18\naudio_codec = \"opus\"\n\
                 audio_bitrate_kbps = 128"
            ),
            "audio_codec"
        );
    }

    #[test]
    fn a_preset_with_neither_stream_is_rejected() {
        assert_eq!(failing_field("container = \"mkv\""), "video_codec");
    }

    #[test]
    fn a_blank_or_shouty_id_is_rejected() {
        let blank = PresetLibrary::from_toml("[[preset]]\nid = \"\"\ncontainer = \"mkv\"\n")
            .expect_err("no id");
        assert_eq!(blank.code, codes::PRESET_INVALID);
        let shouty =
            PresetLibrary::from_toml("[[preset]]\nid = \"Loud Preset\"\ncontainer = \"mkv\"\n")
                .expect_err("bad id");
        assert_eq!(
            shouty.details.get("field").and_then(|v| v.as_str()),
            Some("id")
        );
    }

    #[test]
    fn two_presets_may_not_share_an_id() {
        let text = "[[preset]]\nid = \"twin\"\ncontainer = \"mkv\"\naudio_codec = \"flac\"\n\
                    [[preset]]\nid = \"twin\"\ncontainer = \"mkv\"\naudio_codec = \"flac\"\n";
        let error = PresetLibrary::from_toml(text).expect_err("duplicate");
        assert_eq!(
            error.details.get("field").and_then(|v| v.as_str()),
            Some("id")
        );
    }

    #[test]
    fn an_unknown_key_is_rejected_rather_than_silently_ignored() {
        let error = PresetLibrary::from_toml(
            "[[preset]]\nid = \"custom\"\ncontainer = \"mkv\"\naudio_codec = \"flac\"\nspeed = 3\n",
        )
        .expect_err("unknown key");
        assert_eq!(error.code, codes::PRESET_INVALID);
    }

    #[test]
    fn user_presets_add_to_and_replace_the_built_ins() {
        let dir = std::env::temp_dir().join(format!(
            "sub-export-presets-{}-{}",
            std::process::id(),
            "merge"
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(PRESETS_FILE_NAME);
        std::fs::write(
            &path,
            "[[preset]]\nid = \"youtube-1080p\"\nname = \"Mine\"\ncontainer = \"mkv\"\n\
             video_codec = \"h265\"\nwidth = 1920\nheight = 1080\nframe_rate_numerator = 25\n\
             video_crf = 22\n\n[[preset]]\nid = \"podcast\"\ncontainer = \"mkv\"\n\
             audio_codec = \"opus\"\naudio_bitrate_kbps = 96\nchannels = 1\n",
        )
        .expect("write");
        let library = PresetLibrary::from_dir(&dir).expect("loads");
        assert_eq!(library.len(), PresetLibrary::builtin().len() + 1);
        let replaced = library.require("youtube-1080p").expect("still there");
        assert_eq!(replaced.name, "Mine");
        assert_eq!(replaced.container, Container::Mkv);
        assert_eq!(library.ids().first().copied(), Some("youtube-1080p"));
        let added = library.require("podcast").expect("added");
        assert_eq!(added.audio.as_ref().expect("has audio").channels, 1);
        assert_eq!(library.source(), Some(path.as_path()));
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn a_missing_user_file_leaves_the_built_ins_alone() {
        let dir = std::env::temp_dir().join("sub-export-presets-does-not-exist");
        let library = PresetLibrary::from_dir(&dir).expect("missing is fine");
        assert_eq!(library, PresetLibrary::builtin());
    }

    #[test]
    fn a_broken_user_file_fails_with_the_path_and_the_field() {
        let dir = std::env::temp_dir().join(format!(
            "sub-export-presets-{}-{}",
            std::process::id(),
            "broken"
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(
            dir.join(PRESETS_FILE_NAME),
            "[[preset]]\nid = \"bad\"\ncontainer = \"ogg\"\naudio_codec = \"flac\"\n",
        )
        .expect("write");
        let error = PresetLibrary::from_dir(&dir).expect_err("invalid");
        assert_eq!(error.code, codes::PRESET_INVALID);
        assert_eq!(
            error.details.get("field").and_then(|v| v.as_str()),
            Some("container")
        );
        assert!(error.details.contains_key("path"));
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn the_config_dir_env_wins_over_the_platform_location() {
        let forced = config_dir_from(&env_of(&[(CONFIG_DIR_ENV, "/tmp/forced")]));
        assert_eq!(forced, Some(PathBuf::from("/tmp/forced")));
        let blank = config_dir_from(&env_of(&[(CONFIG_DIR_ENV, "   ")]));
        assert_ne!(blank, Some(PathBuf::from("   ")));
        assert!(config_dir_from(&env_of(&[])).is_none(), "no home, no dir");
    }

    #[test]
    fn presets_round_trip_through_toml() {
        let library = PresetLibrary::builtin();
        let text = library.to_toml_string().expect("writable");
        let reloaded = PresetLibrary::from_toml(&text).expect("re-readable");
        assert_eq!(reloaded, library);
        let one: Preset = library.require("mezzanine").expect("shipped").clone();
        let single = PresetLibrary::from_toml(&one.to_toml_string().expect("writable"))
            .expect("re-readable");
        assert_eq!(single.iter().next(), Some(&one));
    }

    #[test]
    fn an_empty_document_has_no_presets() {
        let library = PresetLibrary::from_toml("").expect("valid, if useless");
        assert!(library.is_empty());
        assert_eq!(library.ids(), Vec::<&str>::new());
        assert_eq!(
            AudioPreset {
                codec: AudioCodec::Flac,
                bitrate_kbps: None,
                sample_rate: 48_000,
                channels: 2,
            }
            .channels,
            2
        );
    }
    #[test]
    fn every_shipped_preset_is_four_two_zero() {
        for preset in &PresetLibrary::builtin() {
            let Some(video) = &preset.video else {
                continue;
            };
            assert_eq!(
                video.chroma,
                ChromaFormat::Yuv420,
                "preset '{}' ships a profile every player takes",
                preset.id,
            );
            let settings = preset.to_settings().expect("a video preset resolves");
            assert_eq!(settings.chroma, ChromaFormat::Yuv420);
        }
    }

    #[test]
    fn a_preset_may_ask_for_a_higher_chroma_format() {
        let library = PresetLibrary::from_toml(&document(
            "container = \"mkv\"\nvideo_codec = \"h264\"\nwidth = 1920\nheight = 1080\nframe_rate_numerator = 30\nvideo_crf = 0\nchroma = \"4:4:4\"",
        ))
        .expect("a master preset is valid");
        let preset = library.require("custom").expect("the preset is there");
        let video = preset.video.as_ref().expect("has video");
        assert_eq!(video.chroma, ChromaFormat::Yuv444);
        assert_eq!(
            preset.to_settings().expect("it resolves").chroma,
            ChromaFormat::Yuv444,
        );
    }

    #[test]
    fn a_chroma_format_this_exporter_does_not_encode_names_its_field() {
        assert_eq!(
            failing_field(
                "container = \"mkv\"\nvideo_codec = \"h264\"\nwidth = 640\nheight = 480\nframe_rate_numerator = 30\nvideo_crf = 20\nchroma = \"4:1:1\"",
            ),
            "chroma",
        );
    }

    #[test]
    fn the_chroma_format_survives_a_round_trip_through_toml() {
        let library = PresetLibrary::from_toml(&document(
            "container = \"mkv\"\nvideo_codec = \"h265\"\nwidth = 640\nheight = 480\nframe_rate_numerator = 30\nvideo_crf = 20\nchroma = \"4:2:2\"",
        ))
        .expect("valid");
        let written = library.to_toml_string().expect("it writes");
        assert!(written.contains("4:2:2"), "{written}");
        let again = PresetLibrary::from_toml(&written).expect("it reads back");
        assert_eq!(
            again
                .require("custom")
                .expect("still there")
                .video
                .as_ref()
                .expect("has video")
                .chroma,
            ChromaFormat::Yuv422,
        );
    }
}
