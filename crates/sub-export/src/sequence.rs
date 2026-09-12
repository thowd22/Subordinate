//! A sequence as the two streams an export reads (docs/PLAN.md §5.5).
//!
//! An export needs pixels and samples. The pixels are the compositor's
//! full-resolution readback (TASK-58) and the samples are the mixer's offline
//! render (TASK-55) — the same compositor the viewer draws with and the same
//! mix graph playback publishes, run frame by frame instead of in real time.
//!
//! That bridge was written once inside the `render` subcommand of
//! `subordinate-cli` and lives here so the editor window and the CLI render
//! the same file from the same project: one adapter, one set of rounding
//! rules, one answer.
//!
//! Both sources are lazy on purpose. [`SequenceFrames`] opens a decoder the
//! first time a clip is under the playhead and [`SequenceAudio`] decodes,
//! resamples and mixes the whole span on its first read, so a caller can build
//! them on a UI thread and hand them straight to a worker without a single
//! frame of decode happening where the window is painting.
//!
//! Every time here is a [`RationalTime`] at the sequence's own timebase: the
//! range is a frame count and the audio span is the exact number of samples
//! those frames cover. No float decides which frame is written.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sub_audio::decode::Pcm;
use sub_audio::mixer::{ClipSpec, MixGraphBuilder, TrackSpec};
use sub_audio::offline::{OfflineSequence, PcmSource, render_audio};
use sub_core::{ErrorCode, SubError, SubResult, codes};
use sub_media::{Decoder, DecoderOptions, FrameFormat, ProbeOptions, StreamSelection};
use sub_model::{ClipId, MediaUse, Project, Sequence, TrackKind};
use sub_render::{
    Compositor, Nv12Converter, Nv12Geometry, RenderContext, RenderError, ResolvedClip, SourceFrame,
};
use sub_time::{Rational, RationalTime, TimeRange};

use crate::ExportSettings;
use crate::pipeline::{AudioFrameSource, PcmAudioSource, VideoFrameSource};
use crate::presets::Preset;

/// The settings for `preset` over `sequence`, and what they differ on.
///
/// The preset chooses the container, the codecs and the audio format; the
/// sequence chooses the canvas and the exact timebase, because the compositor
/// draws at the sequence's own resolution and nothing rescales a finished
/// frame. A preset that asks for another canvas or rate is honoured in
/// everything else and the difference comes back as a warning rather than
/// being silently written into the file's caps.
///
/// A sequence with no audio clip drops the preset's audio codec: an empty
/// audio stream in the file would be a lie about the timeline.
///
/// # Errors
///
/// [`crate::codes::UNSUPPORTED_COMBINATION`] for a preset with no video
/// stream, and whatever [`ExportSettings::validate`] refuses.
pub fn settings_for_sequence(
    preset: &Preset,
    sequence: &Sequence,
) -> SubResult<(ExportSettings, Vec<String>)> {
    let mut warnings = Vec::new();
    let video = preset.video.as_ref().ok_or_else(|| {
        SubError::new(
            crate::codes::UNSUPPORTED_COMBINATION,
            format!("preset '{}' has no video stream to render", preset.id),
        )
        .with_detail("preset", preset.id.clone())
    })?;
    let resolution = sequence.settings.resolution;
    let frame_rate = sequence.settings.frame_rate;
    if video.width != resolution.width() || video.height != resolution.height() {
        warnings.push(format!(
            "preset '{}' asks for {}x{}; the sequence canvas is {}x{} and that is what is written",
            preset.id,
            video.width,
            video.height,
            resolution.width(),
            resolution.height(),
        ));
    }
    if video.frame_rate != frame_rate {
        warnings.push(format!(
            "preset '{}' asks for {} fps; the sequence runs at {} fps and that is what is written",
            preset.id, video.frame_rate, frame_rate,
        ));
    }

    let mut settings = ExportSettings::new(
        resolution.width(),
        resolution.height(),
        frame_rate,
        preset.container,
    )
    .with_video_codec(video.codec)
    .with_video_quality(Some(video.quality))
    .with_audio_codec(preset.audio.as_ref().map(|audio| audio.codec));
    if let Some(audio) = &preset.audio {
        settings = settings
            .with_audio_format(audio.sample_rate, audio.channels)
            .with_audio_bitrate(audio.bitrate_kbps);
    }
    if !has_audio(sequence) {
        settings = settings.with_audio_codec(None);
    }
    settings.validate()?;
    Ok((settings, warnings))
}

/// True when the sequence has an audio track carrying a clip.
#[must_use]
pub fn has_audio(sequence: &Sequence) -> bool {
    sequence
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Audio)
        .any(|track| track.clips().next().is_some())
}

/// The sequence's length in frames of its own timebase.
#[must_use]
pub fn sequence_frames(sequence: &Sequence) -> i64 {
    let rate = sequence.settings.frame_rate;
    sequence
        .tracks
        .iter()
        .map(|track| track.duration(rate).value())
        .max()
        .unwrap_or(0)
}

/// Every clip that carries an enabled plugin effect, by name.
///
/// A headless render does not run plugin effects yet, and a caller reports
/// these as warnings rather than writing a picture that quietly lacks them.
#[must_use]
pub fn clips_with_effects(sequence: &Sequence) -> Vec<String> {
    sequence
        .tracks
        .iter()
        .flat_map(sub_model::Track::clips)
        .filter(|clip| clip.effects.iter().any(|effect| effect.enabled))
        .map(|clip| clip.name.clone())
        .collect()
}

/// The half-open frame range an export covers, in the sequence's own frames.
///
/// `start` is the first frame written and `count` is how many follow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSpan {
    /// The first frame of the sequence that is written.
    pub start: i64,
    /// How many frames are written.
    pub count: u64,
}

impl FrameSpan {
    /// A span of `count` frames beginning at `start`.
    #[must_use]
    pub const fn new(start: i64, count: u64) -> Self {
        Self { start, count }
    }

    /// The whole of `sequence`.
    ///
    /// # Errors
    ///
    /// [`codes::INVALID_ARGUMENT`] when the sequence holds no frames.
    pub fn whole(sequence: &Sequence) -> SubResult<Self> {
        let total = sequence_frames(sequence);
        if total <= 0 {
            return Err(SubError::new(
                codes::INVALID_ARGUMENT,
                format!(
                    "sequence '{}' is empty; there is nothing to render",
                    sequence.name
                ),
            )
            .with_detail("sequence", sequence.name.clone()));
        }
        Ok(Self::new(0, u64::try_from(total).unwrap_or(0)))
    }

    /// The frame after the last one written.
    #[must_use]
    pub fn end(self) -> i64 {
        self.start
            .saturating_add(i64::try_from(self.count).unwrap_or(i64::MAX))
    }
}

/// Everything a sequence export reads, opened together.
///
/// The video source is always there; the audio source is `None` when the
/// settings carry no audio codec, which is what
/// [`settings_for_sequence`] leaves behind for a sequence with no audio clip.
///
/// Nothing is decoded here: both halves open on their first read, on whatever
/// thread the export runs on.
#[must_use]
pub fn open_streams(
    context: &RenderContext,
    project: Arc<Project>,
    sequence: Arc<Sequence>,
    project_dir: &Path,
    settings: &ExportSettings,
    span: FrameSpan,
) -> (SequenceFrames, Option<SequenceAudio>) {
    let video = SequenceFrames::new(
        context,
        Arc::clone(&project),
        Arc::clone(&sequence),
        project_dir,
        span,
    );
    let audio = settings
        .audio_codec
        .is_some()
        .then(|| SequenceAudio::new(project, sequence, project_dir, settings, span));
    (video, audio)
}

/// The composited picture of a sequence, one frame at a time.
///
/// One decoder per clip, kept open across frames: a render walks forward, and
/// [`Decoder::seek_to`] decodes forward inside its own GOP window rather than
/// re-seeking, so a sequential render costs one keyframe seek per clip.
pub struct SequenceFrames {
    context: RenderContext,
    compositor: Compositor,
    project: Arc<Project>,
    sequence: Arc<Sequence>,
    project_dir: PathBuf,
    decoders: HashMap<ClipId, Decoder>,
    span: FrameSpan,
    index: u64,
    pixels: Vec<u8>,
}

impl std::fmt::Debug for SequenceFrames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SequenceFrames")
            .field("sequence", &self.sequence.name)
            .field("span", &self.span)
            .field("index", &self.index)
            .finish_non_exhaustive()
    }
}

impl SequenceFrames {
    /// A source over `span` of `sequence`.
    ///
    /// Cheap: the compositor's canvas is allocated, and nothing else happens
    /// until the first frame is asked for.
    #[must_use]
    pub fn new(
        context: &RenderContext,
        project: Arc<Project>,
        sequence: Arc<Sequence>,
        project_dir: &Path,
        span: FrameSpan,
    ) -> Self {
        Self {
            context: context.clone(),
            compositor: Compositor::for_sequence(context.clone(), &sequence),
            project,
            sequence,
            project_dir: project_dir.to_path_buf(),
            decoders: HashMap::new(),
            span,
            index: 0,
            pixels: Vec::new(),
        }
    }

    /// Composites one frame at `time` and hands back the canvas as RGBA.
    ///
    /// The export walks the span in order through [`VideoFrameSource`]; this
    /// is the same picture for a caller that wants a single frame — the
    /// CLI's `render_frame_png`, which lets an agent look at the timeline.
    ///
    /// # Errors
    ///
    /// Whatever the decoders and the NV12 converter raise.
    pub fn frame_at(&mut self, time: RationalTime) -> SubResult<Vec<u8>> {
        self.composite(time)
    }

    /// Composites the frame at `time` and reads the canvas back as RGBA.
    fn composite(&mut self, time: RationalTime) -> SubResult<Vec<u8>> {
        // Held by its own handle so the resolved layers borrow the sequence
        // rather than `self`, which the decoders below are taken from.
        let sequence = Arc::clone(&self.sequence);
        let layers: Vec<ResolvedClip<'_>> =
            sub_render::resolve_layers_at(&sequence, time).collect();
        let mut pictures: HashMap<ClipId, SourceFrame> = HashMap::new();
        // The converters own the textures the views point at, so they live as
        // long as the frames do.
        let mut converters = Vec::with_capacity(layers.len());
        for layer in &layers {
            if let Some((frame, converter)) = self.picture(layer)? {
                pictures.insert(layer.clip_id(), frame);
                converters.push(converter);
            }
        }
        let mut source = |layer: &ResolvedClip<'_>| pictures.get(&layer.clip_id()).cloned();
        self.compositor.render(&sequence, time, &mut source);
        Ok(self.compositor.read_rgba())
    }

    /// Decodes and uploads one layer's source frame, or `None` when its media
    /// has nothing at that time.
    fn picture(
        &mut self,
        layer: &ResolvedClip<'_>,
    ) -> SubResult<Option<(SourceFrame, Nv12Converter)>> {
        let decoder = match self.decoders.entry(layer.clip_id()) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let path = media_path(&self.project, &self.project_dir, layer)?;
                let options = DecoderOptions {
                    streams: StreamSelection::Video,
                    format: FrameFormat::Nv12,
                    ..DecoderOptions::default()
                };
                entry.insert(Decoder::open_with(&path, options)?)
            }
        };
        tracing::trace!(
            clip = %layer.clip_id(),
            source_time = %layer.source_time,
            "decoding an export layer"
        );
        let Some(picture) = decoder.seek_to(layer.source_time)? else {
            return Ok(None);
        };
        tracing::trace!(
            clip = %layer.clip_id(),
            width = picture.width(),
            height = picture.height(),
            "decoded an export layer"
        );
        let geometry = Nv12Geometry::new(
            picture.width(),
            picture.height(),
            picture.plane_stride(0).unwrap_or(0),
            picture.plane_stride(1).unwrap_or(0),
        )
        .map_err(|error| lift_render_error(&error))?;
        let converter = Nv12Converter::new(self.context.device(), geometry);
        converter
            .submit_frame(
                self.context.device(),
                self.context.queue(),
                picture.plane_data(0).unwrap_or(&[]),
                picture.plane_data(1).unwrap_or(&[]),
            )
            .map_err(|error| lift_render_error(&error))?;
        let view = converter
            .output()
            .create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Some((
            SourceFrame::new(view, geometry.width(), geometry.height()),
            converter,
        )))
    }
}

impl VideoFrameSource for SequenceFrames {
    fn next_frame(&mut self) -> SubResult<Option<&[u8]>> {
        if self.index >= self.span.count {
            return Ok(None);
        }
        let offset = i64::try_from(self.index).map_err(|_| {
            SubError::new(
                codes::INTERNAL,
                "a render longer than time itself was asked for",
            )
        })?;
        let time = RationalTime::new(self.span.start + offset, self.sequence.settings.frame_rate);
        // Debug rather than trace: one line a frame either side of the
        // composite is what tells a stalled decode apart from a stalled
        // readback in a log taken off a runner.
        tracing::debug!(frame = self.index, time = %time, "compositing an export frame");
        self.pixels = self.composite(time)?;
        tracing::debug!(
            frame = self.index,
            bytes = self.pixels.len(),
            "composited an export frame"
        );
        self.index += 1;
        Ok(Some(&self.pixels))
    }
}

/// The offline mix of a sequence's audio over an export's range.
///
/// The whole span is rendered on the first read and handed out from there:
/// decoding, resampling and mixing a sequence is minutes of work for a long
/// export, and doing it when the source is *built* would stall whichever
/// thread asked for the export. Building one costs nothing.
///
/// The graph is compiled at the *export* sample rate and channel count: the
/// clip sources are resampled on the way in, so the mixer runs once at the
/// rate the encoder wants and nothing is resampled twice.
pub struct SequenceAudio {
    project: Arc<Project>,
    sequence: Arc<Sequence>,
    project_dir: PathBuf,
    settings: ExportSettings,
    span: FrameSpan,
    rendered: Option<PcmAudioSource>,
}

impl std::fmt::Debug for SequenceAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SequenceAudio")
            .field("sequence", &self.sequence.name)
            .field("span", &self.span)
            .field("rendered", &self.rendered.is_some())
            .finish_non_exhaustive()
    }
}

impl SequenceAudio {
    /// A source over `span` of `sequence`, mixed to `settings`' audio format.
    #[must_use]
    pub fn new(
        project: Arc<Project>,
        sequence: Arc<Sequence>,
        project_dir: &Path,
        settings: &ExportSettings,
        span: FrameSpan,
    ) -> Self {
        Self {
            project,
            sequence,
            project_dir: project_dir.to_path_buf(),
            settings: settings.clone(),
            span,
            rendered: None,
        }
    }

    /// Renders the whole span, once.
    ///
    /// # Errors
    ///
    /// [`codes::NOT_FOUND`] for a clip whose media the project does not hold
    /// or whose file is offline, and whatever the decoders, the resampler and
    /// the mixer raise.
    pub fn render(&mut self) -> SubResult<&mut PcmAudioSource> {
        if self.rendered.is_none() {
            // The whole span is decoded, resampled and mixed here, on the
            // first read: minutes of work for a long export, and the one place
            // an export looks stalled while it is only busy.
            tracing::debug!(span = ?self.span, "mixing the export's audio");
            let mix = self.mix()?;
            tracing::debug!(samples = mix.remaining(), "mixed the export's audio");
            self.rendered = Some(mix);
        }
        Ok(self
            .rendered
            .as_mut()
            .unwrap_or_else(|| unreachable!("just rendered")))
    }

    /// The mix of every audio clip of the sequence over the export's range.
    fn mix(&self) -> SubResult<PcmAudioSource> {
        let settings = &self.settings;
        let rate = self.sequence.settings.frame_rate;
        let mut builder = MixGraphBuilder::new(settings.sample_rate, settings.channels);
        let mut sources: Vec<(usize, PcmSource)> = Vec::new();
        let mut slot = 0;
        // One answer per file for the whole mix: a project that cuts the same
        // clip twenty times probes it once, not twenty times.
        let mut routing: HashMap<PathBuf, bool> = HashMap::new();
        for track in self
            .sequence
            .tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Audio)
        {
            let mut spec = TrackSpec::new()
                .with_gain_db(track.gain.decibels().as_f64())
                .with_muted(track.muted)
                .with_solo(track.solo);
            for (clip, placement) in track.clip_placements(rate) {
                let pcm = clip_pcm(
                    &self.project,
                    &self.project_dir,
                    clip,
                    settings,
                    &mut routing,
                )?;
                spec = spec.with_clip(
                    ClipSpec::new(slot, placement.start(), placement.duration())
                        .with_gain_db(clip.gain.decibels().as_f64())
                        .with_fades(clip.fade_in, clip.fade_out),
                );
                sources.push((slot, PcmSource::new(pcm)));
                slot += 1;
            }
            builder = builder.track(spec);
        }
        if sources.is_empty() {
            return Ok(PcmAudioSource::new(Vec::new()));
        }

        let graph = builder.slots(slot).build()?;
        let mut offline = OfflineSequence::new(graph);
        for (slot, source) in sources {
            offline.set_source(slot, Box::new(source))?;
        }

        // The exact samples those video frames cover: the same arithmetic the
        // pipeline uses to decide how much audio belongs to each frame.
        let start_frame = u64::try_from(self.span.start).unwrap_or(0);
        let first = settings.audio_frames_through(start_frame);
        let last = settings.audio_frames_through(start_frame + self.span.count);
        let sample_rate = Rational::new(settings.sample_rate, 1).ok_or_else(|| {
            SubError::new(codes::INVALID_ARGUMENT, "the export sample rate is zero")
        })?;
        let span = TimeRange::from_start_end(
            RationalTime::new(i64::try_from(first).unwrap_or(i64::MAX), sample_rate),
            RationalTime::new(i64::try_from(last).unwrap_or(i64::MAX), sample_rate),
        )
        .ok_or_else(|| {
            SubError::new(
                codes::INTERNAL,
                "the audio span of the range is not representable",
            )
        })?;
        let mixed = render_audio(&mut offline, span)?;
        Ok(PcmAudioSource::new(mixed.samples))
    }
}

impl AudioFrameSource for SequenceAudio {
    fn read(&mut self, out: &mut [f32], channels: u16) -> SubResult<usize> {
        self.render()?.read(out, channels)
    }
}

/// The absolute path of the media a layer reads.
fn media_path(
    project: &Project,
    project_dir: &Path,
    layer: &ResolvedClip<'_>,
) -> SubResult<PathBuf> {
    let item = project
        .media
        .iter()
        .find(|item| item.id == layer.clip.media)
        .ok_or_else(|| {
            SubError::new(
                codes::NOT_FOUND,
                format!(
                    "clip '{}' names media the project does not hold",
                    layer.clip.name
                ),
            )
            .with_detail("clip", layer.clip.name.clone())
        })?;
    let path = item.absolute_source(project_dir, MediaUse::Export);
    if !path.is_file() {
        return Err(SubError::new(
            codes::NOT_FOUND,
            format!("the media of clip '{}' is offline", layer.clip.name),
        )
        .with_detail("clip", layer.clip.name.clone())
        .with_detail("path", path.display().to_string()));
    }
    Ok(path)
}

/// One audio clip's frames, decoded, folded and resampled to the export
/// format, starting at the clip's own in point.
///
/// `routing` remembers, per file, which decoder that file's audio goes
/// through, so a mix over many cuts of one source asks the question once. See
/// [`source_has_video`].
fn clip_pcm(
    project: &Project,
    project_dir: &Path,
    clip: &sub_model::Clip,
    settings: &ExportSettings,
    routing: &mut HashMap<PathBuf, bool>,
) -> SubResult<Pcm> {
    let item = project
        .media
        .iter()
        .find(|item| item.id == clip.media)
        .ok_or_else(|| {
            SubError::new(
                codes::NOT_FOUND,
                format!(
                    "audio clip '{}' names media the project does not hold",
                    clip.name
                ),
            )
            .with_detail("clip", clip.name.clone())
        })?;
    let path = item.absolute_source(project_dir, MediaUse::Export);
    if !path.is_file() {
        return Err(SubError::new(
            codes::NOT_FOUND,
            format!("the media of audio clip '{}' is offline", clip.name),
        )
        .with_detail("clip", clip.name.clone())
        .with_detail("path", path.display().to_string()));
    }

    let has_video = source_has_video(item, &path, routing);
    let decoded = decode_audio(&path, has_video)?;
    let decoded = to_channels(decoded, settings.channels);
    let decoded = resample(decoded, settings.sample_rate)?;
    Ok(trim_to_source_start(decoded, clip.source_range))
}

/// Whether the file a clip reads carries video, which is what decides the
/// decoder its audio goes through (decision-4).
///
/// A probed media item answers from the model, which costs nothing. An item
/// whose `info` is `None` has never been probed — or was offline when the
/// project was saved — and the model has nothing to say about it. Reading that
/// silence as "audio only" sent video files to symphonia, which carries far
/// fewer codecs than GStreamer does, so an unprobed Matroska file with Opus
/// audio failed an export that the same file, probed, sails through
/// (TASK-150). The file itself is asked instead, before the export reads it.
///
/// The frame-timing scan is off: this only needs the list of streams, not how
/// they are spaced, and the scan parses the whole file.
///
/// A probe that fails answers nothing either, and the only decoder left to try
/// is symphonia — which is also the decoder this file would have got before,
/// so a machine whose GStreamer cannot see a format symphonia reads keeps
/// working. The failure is logged rather than raised, because the decode that
/// follows reports a far better error than "the probe failed" would.
fn source_has_video(
    item: &sub_model::MediaItem,
    path: &Path,
    routing: &mut HashMap<PathBuf, bool>,
) -> bool {
    if let Some(info) = item.info.as_ref() {
        return info.has_video();
    }
    if let Some(known) = routing.get(path) {
        return *known;
    }
    let options = ProbeOptions {
        scan_frame_timing: false,
        ..ProbeOptions::default()
    };
    let has_video = match sub_media::probe_with(path, options) {
        Ok(info) => info.has_video(),
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "probing an unprobed source before an export failed; its audio is decoded as audio-only"
            );
            false
        }
    };
    routing.insert(path.to_path_buf(), has_video);
    has_video
}

/// Decodes a whole file's audio: through GStreamer when the file carries
/// video, through symphonia when it does not.
fn decode_audio(path: &Path, has_video: bool) -> SubResult<Pcm> {
    if !has_video {
        return sub_audio::decode::decode_file(path);
    }
    let options = DecoderOptions {
        streams: StreamSelection::AudioOnly,
        ..DecoderOptions::default()
    };
    let mut decoder = Decoder::open_with(path, options)?;
    let format = decoder.audio_format().cloned().ok_or_else(|| {
        SubError::new(
            sub_media::codes::NO_AUDIO_STREAM,
            "the file carries no audio stream to render",
        )
        .with_detail("path", path.display().to_string())
    })?;
    let mut samples = Vec::new();
    while let Some(block) = decoder.next_audio_block()? {
        samples.extend_from_slice(block.samples);
    }
    Ok(Pcm {
        sample_rate: format.sample_rate,
        channels: format.channels,
        samples,
    })
}

/// The same PCM at `channels` channels: a mono source is copied to every
/// channel, a wider one keeps the first `channels` of each frame.
fn to_channels(pcm: Pcm, channels: u16) -> Pcm {
    if pcm.channels == channels || pcm.channels == 0 {
        return pcm;
    }
    let from = usize::from(pcm.channels);
    let to = usize::from(channels);
    let mut samples = Vec::with_capacity(pcm.frames() * to);
    for frame in pcm.samples.chunks_exact(from) {
        for channel in 0..to {
            samples.push(if from == 1 {
                frame[0]
            } else {
                frame.get(channel).copied().unwrap_or(0.0)
            });
        }
    }
    Pcm {
        sample_rate: pcm.sample_rate,
        channels,
        samples,
    }
}

/// The same PCM at `rate` hertz.
fn resample(pcm: Pcm, rate: u32) -> SubResult<Pcm> {
    if pcm.sample_rate == rate {
        return Ok(pcm);
    }
    let mut resampler = sub_audio::resample::Resampler::new(pcm.sample_rate, rate, pcm.channels)?;
    let mut samples = Vec::new();
    resampler.process(&pcm.samples, &mut samples)?;
    resampler.flush(&mut samples)?;
    Ok(Pcm {
        sample_rate: rate,
        channels: pcm.channels,
        samples,
    })
}

/// Drops everything before the clip's in point, so frame zero of the source is
/// the clip's own first frame, which is what a clip slot promises the mixer.
fn trim_to_source_start(pcm: Pcm, source_range: TimeRange) -> Pcm {
    let Some(rate) = Rational::new(pcm.sample_rate, 1) else {
        return pcm;
    };
    let start = source_range.start().rescaled_to(rate).value();
    let Ok(start) = usize::try_from(start) else {
        return pcm;
    };
    let channels = usize::from(pcm.channels.max(1));
    let offset = (start * channels).min(pcm.samples.len());
    Pcm {
        sample_rate: pcm.sample_rate,
        channels: pcm.channels,
        samples: pcm.samples[offset..].to_vec(),
    }
}

/// Carries a [`RenderError`] across as a [`SubError`], keeping its code.
#[must_use]
pub fn lift_render_error(error: &RenderError) -> SubError {
    let code = ErrorCode::parse(error.code()).unwrap_or(codes::INTERNAL);
    SubError::new(code, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        FrameSpan, SequenceAudio, SequenceFrames, has_audio, settings_for_sequence,
        source_has_video,
    };
    use crate::presets::PresetLibrary;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use sub_audio::decode::Pcm;
    use sub_model::{
        MediaItem, MediaPath, Project, Resolution, Sequence, SequenceSettings, StreamInfo,
        TrackKind, VideoStream,
    };
    use sub_time::Rational;

    const fn _assert_send() {
        const fn is_send<T: Send>() {}
        is_send::<SequenceFrames>();
        is_send::<SequenceAudio>();
    }

    fn sequence_at(width: u32, height: u32, rate: Rational) -> Sequence {
        let settings = SequenceSettings::new(
            Resolution::new(width, height).expect("a valid canvas"),
            rate,
            48_000,
            sub_model::ColorTags::default(),
        )
        .expect("valid sequence settings");
        Sequence::new("Main", settings)
    }

    #[test]
    fn the_canvas_and_timebase_come_from_the_sequence_not_the_preset() {
        let library = PresetLibrary::builtin();
        let preset = library.iter().next().expect("a built-in preset");
        let sequence = sequence_at(320, 240, Rational::FPS_25);
        let (settings, warnings) =
            settings_for_sequence(preset, &sequence).expect("the preset resolves");
        assert_eq!(settings.width, 320);
        assert_eq!(settings.height, 240);
        assert_eq!(settings.frame_rate, Rational::FPS_25);
        assert_eq!(settings.container, preset.container);
        assert!(
            warnings.iter().any(|line| line.contains("fps")),
            "the rate difference is reported: {warnings:?}"
        );
    }

    #[test]
    fn a_sequence_with_no_audio_clip_writes_no_audio_stream() {
        let library = PresetLibrary::builtin();
        let preset = library
            .iter()
            .find(|preset| preset.audio.is_some())
            .expect("a built-in preset with audio");
        let sequence = sequence_at(320, 240, Rational::FPS_25);
        assert!(!has_audio(&sequence));
        let (settings, _) = settings_for_sequence(preset, &sequence).expect("the preset resolves");
        assert!(settings.audio_codec.is_none());
    }

    #[test]
    fn channel_counts_are_adapted_without_dropping_frames() {
        let mono = Pcm {
            sample_rate: 48_000,
            channels: 1,
            samples: vec![0.25, -0.5],
        };
        let stereo = super::to_channels(mono, 2);
        assert_eq!(stereo.channels, 2);
        assert_eq!(stereo.samples, vec![0.25, 0.25, -0.5, -0.5]);

        let wide = Pcm {
            sample_rate: 48_000,
            channels: 3,
            samples: vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        };
        let folded = super::to_channels(wide, 2);
        assert_eq!(folded.channels, 2);
        assert_eq!(folded.samples, vec![1.0, 2.0, 4.0, 5.0]);
    }

    #[test]
    fn a_span_of_an_empty_sequence_is_refused() {
        let sequence = sequence_at(320, 240, Rational::FPS_25);
        let error = FrameSpan::whole(&sequence).expect_err("an empty sequence");
        assert_eq!(error.code, sub_core::codes::INVALID_ARGUMENT);
    }

    #[test]
    fn a_span_counts_forward_from_its_start() {
        let span = FrameSpan::new(10, 25);
        assert_eq!(span.end(), 35);
    }

    #[test]
    fn an_audio_source_with_no_clips_mixes_to_silence_without_touching_a_file() {
        let library = PresetLibrary::builtin();
        let preset = library
            .iter()
            .find(|preset| preset.audio.is_some())
            .expect("a built-in preset with audio");
        let mut sequence = sequence_at(320, 240, Rational::FPS_25);
        sequence
            .tracks
            .push(sub_model::Track::new("A1", TrackKind::Audio));
        let (mut settings, _) = settings_for_sequence(preset, &sequence).expect("settings");
        // The settings drop the codec for a sequence with no clip; the source
        // is still buildable, and renders to nothing.
        settings = settings.with_audio_codec(preset.audio.as_ref().map(|audio| audio.codec));
        let project = Arc::new(Project::new("Untitled"));
        let mut audio = SequenceAudio::new(
            project,
            Arc::new(sequence),
            std::path::Path::new("."),
            &settings,
            FrameSpan::new(0, 5),
        );
        assert!(
            format!("{audio:?}").contains("rendered: false"),
            "nothing is rendered until it is read"
        );
        let rendered = audio.render().expect("an empty mix");
        assert_eq!(rendered.remaining(), 0);
    }

    fn item_with(info: Option<StreamInfo>) -> MediaItem {
        let mut item = MediaItem::new(MediaPath::new("footage/a.mkv").expect("a media path"));
        item.info = info;
        item
    }

    #[test]
    fn a_probed_item_routes_from_the_model_without_touching_the_file() {
        let info = StreamInfo {
            video: vec![VideoStream {
                width: 1920,
                height: 1080,
                frame_rate: Rational::FPS_25,
                sample_aspect: Rational::ONE,
                color: sub_model::ColorTags::default(),
            }],
            ..StreamInfo::default()
        };
        let item = item_with(Some(info));
        let mut routing = HashMap::new();
        // The path does not exist: a probed item never reaches the probe.
        assert!(source_has_video(
            &item,
            Path::new("/nonexistent/a.mkv"),
            &mut routing
        ));
        assert!(routing.is_empty(), "nothing was probed, nothing was cached");

        let item = item_with(Some(StreamInfo::default()));
        assert!(!source_has_video(
            &item,
            Path::new("/nonexistent/a.wav"),
            &mut routing
        ));
    }

    #[test]
    fn an_unprobed_item_reuses_the_answer_already_found_for_its_file() {
        let item = item_with(None);
        let path = PathBuf::from("/nonexistent/a.mkv");
        let mut routing = HashMap::from([(path.clone(), true)]);
        // Cached, so this answers "video" for a file that is not there at all:
        // one probe per file per mix, not one per clip.
        assert!(source_has_video(&item, &path, &mut routing));
    }
}
