//! The clip parameter command: opacity, transform, gain and fades.
//!
//! The inspector edits these one field at a time, so [`SetClipParams`] names
//! each of them optionally and leaves the rest alone. What it never does is
//! write a parameter the model would reject: the command builds the clip it
//! wants, runs [`Clip::validate`] on it, and only then puts it on the track.
//! A fade longer than the clip is therefore an error rather than a project
//! that fails to save.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};
use sub_model::{Clip, ClipId, GainDb, Opacity, Project, SequenceId, TrackId, Transform};
use sub_time::RationalTime;

use super::clip_mut;
use crate::{Command, Inverse};

/// Sets any of a clip's parameters, leaving the ones it does not name alone.
///
/// The inverse names exactly the same fields, carrying the values they had, so
/// undoing an opacity edit does not also reset a gain the user changed
/// earlier.
///
/// ```
/// use sub_edit::History;
/// use sub_edit::commands::SetClipParams;
/// use sub_model::{
///     Clip, MediaItem, MediaPath, Opacity, Project, Sequence, SequenceSettings, Track, TrackKind,
/// };
/// use sub_time::{Rational, RationalTime, TimeRange};
///
/// let mut project = Project::new("Doc cut");
/// let media = MediaItem::new(MediaPath::new("a.mp4").unwrap());
/// let media_id = media.id;
/// project.media.push(media);
///
/// let rate = Rational::FPS_24;
/// let source = TimeRange::new(RationalTime::zero(rate), RationalTime::new(48, rate)).unwrap();
/// let clip = Clip::new("shot 1", media_id, source);
/// let clip_id = clip.id;
/// let mut track = Track::new("V1", TrackKind::Video);
/// track.items.push(clip.into());
/// let track_id = track.id;
/// let mut sequence = Sequence::new("Main", SequenceSettings::default());
/// sequence.tracks.push(track);
/// let sequence_id = sequence.id;
/// project.sequences.push(sequence);
///
/// let mut history = History::new();
/// history
///     .apply(
///         &mut project,
///         SetClipParams::new(sequence_id, track_id, clip_id).with_opacity(Opacity::TRANSPARENT),
///     )
///     .unwrap();
/// let clip_of = |project: &Project| project.sequences[0].tracks[0].clip(clip_id).unwrap().opacity;
/// assert!(clip_of(&project).is_transparent());
///
/// history.undo(&mut project).unwrap();
/// assert_eq!(clip_of(&project), Opacity::OPAQUE);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetClipParams {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track holding the clip.
    pub track: TrackId,
    /// The clip to edit.
    pub clip: ClipId,
    /// How opaque the picture is when composited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<Opacity>,
    /// Position, scale and rotation on the sequence canvas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<Transform>,
    /// Clip audio level in decibels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain: Option<GainDb>,
    /// Which of the source's audio streams the clip takes, counted from zero
    /// (TASK-153).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_stream: Option<u16>,
    /// How long the clip ramps up from nothing at its head.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade_in: Option<RationalTime>,
    /// How long the clip ramps down to nothing at its tail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade_out: Option<RationalTime>,
}

impl SetClipParams {
    /// Addresses `clip`, changing nothing yet.
    #[must_use]
    pub fn new(sequence: SequenceId, track: TrackId, clip: ClipId) -> Self {
        Self {
            sequence,
            track,
            clip,
            opacity: None,
            transform: None,
            gain: None,
            audio_stream: None,
            fade_in: None,
            fade_out: None,
        }
    }

    /// The same command, also setting the opacity.
    #[must_use]
    pub fn with_opacity(mut self, opacity: Opacity) -> Self {
        self.opacity = Some(opacity);
        self
    }

    /// The same command, also setting the transform.
    #[must_use]
    pub fn with_transform(mut self, transform: Transform) -> Self {
        self.transform = Some(transform);
        self
    }

    /// The same command, also setting the gain.
    #[must_use]
    pub fn with_gain(mut self, gain: GainDb) -> Self {
        self.gain = Some(gain);
        self
    }

    /// The same command, also naming which of the source's audio streams the
    /// clip takes, counted from zero in the order the probe reported them.
    ///
    /// A stream the source does not carry is refused when the command is
    /// applied, so a project never names a take that is not in the file.
    #[must_use]
    pub fn with_audio_stream(mut self, audio_stream: u16) -> Self {
        self.audio_stream = Some(audio_stream);
        self
    }

    /// The same command, also setting the fade in.
    #[must_use]
    pub fn with_fade_in(mut self, fade_in: RationalTime) -> Self {
        self.fade_in = Some(fade_in);
        self
    }

    /// The same command, also setting the fade out.
    #[must_use]
    pub fn with_fade_out(mut self, fade_out: RationalTime) -> Self {
        self.fade_out = Some(fade_out);
        self
    }

    /// True when the command names no parameter at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.opacity.is_none()
            && self.transform.is_none()
            && self.gain.is_none()
            && self.audio_stream.is_none()
            && self.fade_in.is_none()
            && self.fade_out.is_none()
    }

    /// The command that puts back the parameters this one names, reading their
    /// current values off `clip`.
    fn inverse_of(&self, clip: &Clip) -> Self {
        Self {
            sequence: self.sequence,
            track: self.track,
            clip: self.clip,
            opacity: self.opacity.map(|_| clip.opacity),
            transform: self.transform.map(|_| clip.transform),
            gain: self.gain.map(|_| clip.gain),
            audio_stream: self.audio_stream.map(|_| clip.audio_stream),
            fade_in: self.fade_in.map(|_| clip.fade_in),
            fade_out: self.fade_out.map(|_| clip.fade_out),
        }
    }
}

/// Refuses an audio stream index the clip's source does not carry.
///
/// A source whose streams have not been probed yet accepts any index: the
/// project may be offline, and refusing an edit because a file has not been
/// read would be worse than storing a choice the export later warns about.
fn check_audio_stream(project: &Project, clip: ClipId, stream: u16) -> SubResult<()> {
    let Some(media) = project
        .sequences
        .iter()
        .flat_map(|sequence| sequence.tracks.iter())
        .flat_map(sub_model::Track::clips)
        .find(|on_track| on_track.id == clip)
        .map(|on_track| on_track.media)
    else {
        return Ok(());
    };
    let Some(info) = project
        .media
        .iter()
        .find(|item| item.id == media)
        .and_then(|item| item.info.as_ref())
    else {
        return Ok(());
    };
    let streams = info.audio_stream_count();
    if usize::from(stream) < streams {
        return Ok(());
    }
    Err(SubError::new(
        crate::codes::AUDIO_STREAM_NOT_FOUND,
        "the clip's source carries no audio stream at that index",
    )
    .with_detail("clip_id", clip)
    .with_detail("audio_stream", stream.to_string())
    .with_detail("audio_streams", streams.to_string()))
}

impl Command for SetClipParams {
    const KIND: &'static str = "clip.set_params";
    const DESCRIPTION: &'static str =
        "Set a clip's inspector parameters: opacity, gain, transform and fades.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        if let Some(stream) = self.audio_stream {
            check_audio_stream(project, self.clip, stream)?;
        }
        let clip = clip_mut(project, self.sequence, self.track, self.clip)?;

        let mut candidate = clip.clone();
        if let Some(opacity) = self.opacity {
            candidate.opacity = opacity;
        }
        if let Some(transform) = self.transform {
            candidate.transform = transform;
        }
        if let Some(gain) = self.gain {
            candidate.gain = gain;
        }
        if let Some(audio_stream) = self.audio_stream {
            candidate.audio_stream = audio_stream;
        }
        if let Some(fade_in) = self.fade_in {
            candidate.fade_in = fade_in;
        }
        if let Some(fade_out) = self.fade_out {
            candidate.fade_out = fade_out;
        }
        candidate.validate()?;

        let inverse = self.inverse_of(clip);
        *clip = candidate;
        Ok(Inverse::new(inverse))
    }

    fn label(&self) -> String {
        "Change clip parameters".to_owned()
    }
}
