//! The `render` subcommand: a whole sequence written to a file, headlessly.
//!
//! This is where the three halves of the renderer meet for the first time
//! (docs/PLAN.md §5.5). The project model says what is on the timeline, the
//! compositor draws it and the mixer sums it, and `sub-export` encodes and
//! muxes the result:
//!
//! - **picture**: for every frame of the range, the layers under the playhead
//!   are resolved exactly as the viewer resolves them, each layer's source
//!   frame is decoded at its own source time, uploaded as NV12 and converted,
//!   and the stack is composited and read back as RGBA;
//! - **sound**: the sequence's audio tracks are compiled into the same
//!   [`MixGraph`](sub_audio::mixer::MixGraph) playback publishes, each clip
//!   backed by its decoded and resampled PCM, and rendered offline;
//! - **encoding**: both streams are pushed through an
//!   [`ExportJob`](sub_export::ExportJob), which reports progress and an ETA
//!   and deletes a part-written file if anything fails.
//!
//! Every time here is a [`RationalTime`] at the sequence's own timebase: the
//! range is a frame count, the audio span is the exact number of samples those
//! frames cover, and no float ever decides which frame is written.
//!
//! Progress goes to stderr as it happens and the report goes to stdout as JSON
//! when the render is done, so a shell pipeline, CI and an agent can each take
//! the half they want.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sub_audio::decode::Pcm;
use sub_audio::mixer::{ClipSpec, MixGraphBuilder, TrackSpec};
use sub_audio::offline::{OfflineSequence, PcmSource, render_audio};
use sub_core::{ErrorCode, SubError, SubResult, codes};
use sub_export::{
    AudioFrameSource, EncoderPreferences, ExportElements, ExportEvent, ExportJob, ExportSettings,
    PcmAudioSource, Preset, PresetLibrary, VideoFrameSource,
};
use sub_media::{Decoder, DecoderOptions, FrameFormat, StreamSelection};
use sub_model::{ClipId, MediaUse, Project, Sequence, TrackKind};
use sub_render::{
    Compositor, Nv12Converter, Nv12Geometry, RenderContext, RenderError, ResolvedClip, SourceFrame,
};
use sub_time::{Rational, RationalTime, TimeRange};

/// The frames of the sequence a render covers.
///
/// Both ends are frame numbers at the sequence timebase, and `end` is
/// exclusive, so `0:25` writes twenty-five frames. An absent end means "to the
/// end of the sequence".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameRange {
    /// First frame written.
    pub start: i64,
    /// One past the last frame written, or `None` for the whole sequence.
    pub end: Option<i64>,
}

impl FrameRange {
    /// Reads `IN:OUT`, `IN:` or `:OUT`, in frames of the sequence timebase.
    ///
    /// # Errors
    ///
    /// [`codes::INVALID_ARGUMENT`] when the text is not two frame numbers
    /// around a colon, when either is not a whole number, when the start is
    /// negative, or when the end is not after the start.
    pub fn parse(text: &str) -> SubResult<Self> {
        let (start, end) = text.split_once(':').ok_or_else(|| {
            Self::invalid(
                text,
                "a range is written IN:OUT in frames, either end optional",
            )
        })?;
        let start = Self::frame(text, start)?.unwrap_or(0);
        let end = Self::frame(text, end)?;
        if start < 0 {
            return Err(Self::invalid(
                text,
                "a range cannot start before frame zero",
            ));
        }
        if let Some(end) = end
            && end <= start
        {
            return Err(Self::invalid(text, "a range must end after it starts"));
        }
        Ok(Self { start, end })
    }

    /// One end of a range: a frame number, or `None` when it was left out.
    fn frame(text: &str, part: &str) -> SubResult<Option<i64>> {
        let part = part.trim();
        if part.is_empty() {
            return Ok(None);
        }
        part.parse::<i64>()
            .map(Some)
            .map_err(|_| Self::invalid(text, "a range end must be a whole frame number"))
    }

    /// An `INVALID_ARGUMENT` naming the range that was refused.
    fn invalid(text: &str, why: &str) -> SubError {
        SubError::new(codes::INVALID_ARGUMENT, format!("--range {text}: {why}"))
            .with_detail("range", text.to_owned())
    }
}

/// What `render` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The project file to render.
    pub project: PathBuf,
    /// Which sequence, by name; `None` takes the only one.
    pub sequence: Option<String>,
    /// The export preset, by id.
    pub preset: String,
    /// Where the file goes.
    pub output: PathBuf,
    /// An encoder element pinned for the preset's video codec.
    pub encoder: Option<String>,
    /// The frames to write; `None` renders the whole sequence.
    pub range: Option<FrameRange>,
    /// Whether the written file is probed with the discoverer afterwards.
    pub verify: bool,
}

/// Renders `options` and reports what was written.
///
/// # Errors
///
/// [`codes::NOT_FOUND`] when the project names no such sequence and
/// [`codes::INVALID_ARGUMENT`] for a sequence with no frames or a range
/// outside it; whatever the preset library, the encoder probe, the decoders,
/// the mixer and the export pipeline return; and `render.no_adapter` when this
/// machine enumerates no GPU to composite on.
pub fn run(options: &Options) -> SubResult<Value> {
    let (project, _report) = crate::project::load(&options.project)?;
    // Absolute, because the decoders open a URI: a project named by a relative
    // path would otherwise resolve its media to a relative path too, and
    // GStreamer takes no such thing.
    let project_dir = absolute(options.project.parent().unwrap_or(Path::new(".")))?;
    let sequence = sequence_of(&project, options.sequence.as_deref())?;
    let library = PresetLibrary::load()?;
    let preset = library.require(&options.preset)?.clone();

    let mut warnings = Vec::new();
    let settings = settings_for(&preset, sequence, &mut warnings)?;
    let range = frames_of(sequence, options.range)?;
    let frames = frame_count(range)?;

    let mut preferences = EncoderPreferences::new();
    if let Some(element) = &options.encoder {
        preferences.set_override(settings.video_codec, element)?;
    }
    let elements = ExportElements::resolve(&settings, &preferences)?;

    let context = RenderContext::headless().map_err(|error| lift_render_error(&error))?;
    let mut video = SequenceFrames::new(&context, &project, sequence, &project_dir, range, frames);
    for clip in clips_with_effects(sequence) {
        warnings.push(format!(
            "clip '{clip}' carries plugin effects, which a headless render does not run yet",
        ));
    }

    let mut audio = audio_source(&project, sequence, &project_dir, &settings, range, frames)?;
    let job = ExportJob::new(&options.output, &settings, &elements).with_total_frames(frames);
    let report = job.run(
        &mut video,
        audio
            .as_mut()
            .map(|source| source as &mut dyn AudioFrameSource),
        &mut progress,
    )?;

    let mut answer = serde_json::to_value(&report).map_err(|error| {
        SubError::new(codes::INTERNAL, "the export report could not be reported")
            .with_detail("reason", error.to_string())
    })?;
    let object = answer
        .as_object_mut()
        .ok_or_else(|| SubError::new(codes::INTERNAL, "the export report is not a JSON object"))?;
    object.insert("sequence".to_owned(), json!(sequence.name));
    object.insert("preset".to_owned(), json!(preset.id));
    object.insert("settings".to_owned(), settings_json(&settings));
    object.insert(
        "range".to_owned(),
        json!({
            "start_frame": range.0,
            "end_frame": range.1,
            "frames": frames,
        }),
    );
    object.insert("adapter".to_owned(), json!(context.describe()));
    object.insert("warnings".to_owned(), json!(warnings));
    if options.verify {
        object.insert("probe".to_owned(), verify(&options.output, frames)?);
    }
    Ok(answer)
}

/// `path` made absolute against the working directory, without touching the
/// filesystem beyond asking where that is.
fn absolute(path: &Path) -> SubResult<PathBuf> {
    std::path::absolute(path).map_err(|error| {
        SubError::new(codes::IO, "the project directory could not be resolved")
            .with_detail("path", path.display().to_string())
            .with_detail("reason", error.to_string())
    })
}

/// The sequence `name` picks out, or the project's only one.
fn sequence_of<'a>(project: &'a Project, name: Option<&str>) -> SubResult<&'a Sequence> {
    let known = || {
        project
            .sequences
            .iter()
            .map(|sequence| sequence.name.clone())
            .collect::<Vec<_>>()
    };
    match name {
        Some(name) => project
            .sequences
            .iter()
            .find(|sequence| sequence.name == name)
            .ok_or_else(|| {
                SubError::new(codes::NOT_FOUND, format!("no sequence is named '{name}'"))
                    .with_detail("sequence", name.to_owned())
                    .with_detail("known", known())
            }),
        None => match project.sequences.as_slice() {
            [only] => Ok(only),
            [] => Err(SubError::new(
                codes::INVALID_ARGUMENT,
                "this project has no sequence to render",
            )),
            _ => Err(SubError::new(
                codes::INVALID_ARGUMENT,
                "this project has more than one sequence; name one with --sequence",
            )
            .with_detail("known", known())),
        },
    }
}

/// The export settings for `preset` over `sequence`.
///
/// The preset chooses the container, the codecs and the audio format; the
/// sequence chooses the canvas and the exact timebase, because the compositor
/// draws at the sequence's own resolution and nothing rescales a finished
/// frame. A preset that asks for another canvas or rate is honoured in
/// everything else and the difference is reported as a warning rather than
/// silently written into the file's caps.
fn settings_for(
    preset: &Preset,
    sequence: &Sequence,
    warnings: &mut Vec<String>,
) -> SubResult<ExportSettings> {
    let video = preset.video.as_ref().ok_or_else(|| {
        SubError::new(
            sub_export::codes::UNSUPPORTED_COMBINATION,
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
    .with_audio_codec(preset.audio.as_ref().map(|audio| audio.codec));
    if let Some(audio) = &preset.audio {
        settings = settings.with_audio_format(audio.sample_rate, audio.channels);
    }
    if !has_audio(sequence) {
        settings = settings.with_audio_codec(None);
    }
    settings.validate()?;
    Ok(settings)
}

/// The settings as JSON, with the frame rate written as its exact fraction.
fn settings_json(settings: &ExportSettings) -> Value {
    json!({
        "width": settings.width,
        "height": settings.height,
        "frame_rate": {
            "numerator": settings.frame_rate.numerator(),
            "denominator": settings.frame_rate.denominator(),
        },
        "container": settings.container.as_str(),
        "video_codec": settings.video_codec.as_str(),
        "audio_codec": settings.audio_codec.map(sub_export::AudioCodec::as_str),
        "sample_rate": settings.sample_rate,
        "channels": settings.channels,
    })
}

/// True when the sequence has an audio track carrying a clip.
fn has_audio(sequence: &Sequence) -> bool {
    sequence
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Audio)
        .any(|track| track.clips().next().is_some())
}

/// Every clip that carries an enabled plugin effect, by name.
fn clips_with_effects(sequence: &Sequence) -> Vec<String> {
    sequence
        .tracks
        .iter()
        .flat_map(sub_model::Track::clips)
        .filter(|clip| clip.effects.iter().any(|effect| effect.enabled))
        .map(|clip| clip.name.clone())
        .collect()
}

/// The sequence's length in frames of its own timebase.
fn sequence_frames(sequence: &Sequence) -> i64 {
    let rate = sequence.settings.frame_rate;
    sequence
        .tracks
        .iter()
        .map(|track| track.duration(rate).value())
        .max()
        .unwrap_or(0)
}

/// The half-open frame range a render covers, clamped to the sequence.
fn frames_of(sequence: &Sequence, range: Option<FrameRange>) -> SubResult<(i64, i64)> {
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
    let range = range.unwrap_or_default();
    let start = range.start;
    let end = range.end.unwrap_or(total).min(total);
    if start >= end {
        return Err(SubError::new(
            codes::INVALID_ARGUMENT,
            "the requested range lies outside the sequence",
        )
        .with_detail("start_frame", start)
        .with_detail("end_frame", end)
        .with_detail("sequence_frames", total));
    }
    Ok((start, end))
}

/// How many frames a range writes.
fn frame_count(range: (i64, i64)) -> SubResult<u64> {
    u64::try_from(range.1 - range.0).map_err(|_| {
        SubError::new(codes::INVALID_ARGUMENT, "the range holds no frames")
            .with_detail("start_frame", range.0)
            .with_detail("end_frame", range.1)
    })
}

/// Writes one progress line per event to stderr.
///
/// Numbers are integers throughout: the job carries rates and percentages in
/// thousandths so that what is printed is exactly what was measured.
fn progress(event: &ExportEvent) {
    match event {
        ExportEvent::Started {
            path, frames_total, ..
        } => {
            eprintln!("render: {} frames to {}", frames_total, path.display());
        }
        ExportEvent::Progress(progress) => {
            let percent = progress
                .percent()
                .map_or_else(|| "--".to_owned(), |percent| format!("{percent}%"));
            let fps = progress.frames_per_second_milli / 1_000;
            let eta = progress
                .eta
                .map_or_else(|| "--".to_owned(), |eta| format!("{}s", eta.as_secs()));
            eprintln!(
                "render: {}/{} frames {percent} {fps} fps eta {eta}",
                progress.frames_done, progress.frames_total,
            );
        }
        ExportEvent::Finished(report) => {
            eprintln!(
                "render: wrote {} ({} video frames)",
                report.path.display(),
                report.video_frames
            );
        }
        ExportEvent::Cancelled { frames_done, .. } => {
            eprintln!("render: cancelled after {frames_done} frames");
        }
        ExportEvent::Failed {
            error, frames_done, ..
        } => {
            eprintln!(
                "render: failed after {frames_done} frames: [{}] {error}",
                error.code
            );
        }
    }
}

/// Probes the written file with the GStreamer discoverer.
///
/// A render that wrote a file nothing can open is a failed render however
/// cleanly the pipeline shut down, so this is an error rather than a report
/// when the file carries no video.
fn verify(path: &Path, frames: u64) -> SubResult<Value> {
    // The discoverer takes a URI, so the file is named absolutely whatever the
    // caller typed.
    let info = sub_media::probe::probe(&absolute(path)?)?;
    if info.video.is_empty() {
        return Err(SubError::new(
            sub_export::codes::PIPELINE_FAILED,
            "the rendered file carries no video stream",
        )
        .with_detail("path", path.display().to_string()));
    }
    Ok(json!({
        "duration": info.duration.map(|duration| duration.to_string()),
        "frames_expected": frames,
        "video": info.video.iter().map(|stream| json!({
            "codec": stream.codec.clone(),
            "width": stream.width,
            "height": stream.height,
        })).collect::<Vec<_>>(),
        "audio": info.audio.iter().map(|stream| json!({
            "codec": stream.codec.clone(),
            "channels": stream.channels,
            "sample_rate": stream.sample_rate,
        })).collect::<Vec<_>>(),
    }))
}

/// Carries a [`RenderError`] across as a [`SubError`], keeping its code.
fn lift_render_error(error: &RenderError) -> SubError {
    let code = ErrorCode::parse(error.code()).unwrap_or(codes::INTERNAL);
    SubError::new(code, error.to_string())
}

/// The composited picture of a sequence, one frame at a time.
///
/// One decoder per clip, kept open across frames: a render walks forward, and
/// [`Decoder::seek_to`] decodes forward inside its own GOP window rather than
/// re-seeking, so a sequential render costs one keyframe seek per clip.
struct SequenceFrames<'a> {
    context: RenderContext,
    compositor: Compositor,
    project: &'a Project,
    sequence: &'a Sequence,
    project_dir: PathBuf,
    decoders: HashMap<ClipId, Decoder>,
    start: i64,
    count: u64,
    index: u64,
    pixels: Vec<u8>,
}

impl<'a> SequenceFrames<'a> {
    /// A source over `range` of `sequence`.
    fn new(
        context: &RenderContext,
        project: &'a Project,
        sequence: &'a Sequence,
        project_dir: &Path,
        range: (i64, i64),
        count: u64,
    ) -> Self {
        Self {
            context: context.clone(),
            compositor: Compositor::for_sequence(context.clone(), sequence),
            project,
            sequence,
            project_dir: project_dir.to_path_buf(),
            decoders: HashMap::new(),
            start: range.0,
            count,
            index: 0,
            pixels: Vec::new(),
        }
    }

    /// Composites the frame at `time` and reads the canvas back as RGBA.
    fn composite(&mut self, time: RationalTime) -> SubResult<Vec<u8>> {
        let sequence = self.sequence;
        let layers: Vec<ResolvedClip<'a>> = sub_render::resolve_layers_at(sequence, time).collect();
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
        self.compositor.render(sequence, time, &mut source);
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
                let path = media_path(self.project, &self.project_dir, layer)?;
                let options = DecoderOptions {
                    streams: StreamSelection::Video,
                    format: FrameFormat::Nv12,
                    ..DecoderOptions::default()
                };
                entry.insert(Decoder::open_with(&path, options)?)
            }
        };
        let Some(picture) = decoder.seek_to(layer.source_time)? else {
            return Ok(None);
        };
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

impl VideoFrameSource for SequenceFrames<'_> {
    fn next_frame(&mut self) -> SubResult<Option<&[u8]>> {
        if self.index >= self.count {
            return Ok(None);
        }
        let offset = i64::try_from(self.index).map_err(|_| {
            SubError::new(
                codes::INTERNAL,
                "a render longer than time itself was asked for",
            )
        })?;
        let time = RationalTime::new(self.start + offset, self.sequence.settings.frame_rate);
        self.pixels = self.composite(time)?;
        self.index += 1;
        Ok(Some(&self.pixels))
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

/// The offline mix of the sequence's audio over the rendered range, or `None`
/// when the export has no audio stream.
///
/// The graph is compiled at the *export* sample rate and channel count: the
/// clip sources are resampled on the way in, so the mixer runs once at the
/// rate the encoder wants and nothing is resampled twice.
fn audio_source(
    project: &Project,
    sequence: &Sequence,
    project_dir: &Path,
    settings: &ExportSettings,
    range: (i64, i64),
    frames: u64,
) -> SubResult<Option<PcmAudioSource>> {
    if settings.audio_codec.is_none() {
        return Ok(None);
    }
    let rate = sequence.settings.frame_rate;
    let mut builder = MixGraphBuilder::new(settings.sample_rate, settings.channels);
    let mut sources: Vec<(usize, PcmSource)> = Vec::new();
    let mut slot = 0;
    for track in sequence
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Audio)
    {
        let mut spec = TrackSpec::new()
            .with_gain_db(track.gain.decibels().as_f64())
            .with_muted(track.muted)
            .with_solo(track.solo);
        for (clip, placement) in track.clip_placements(rate) {
            let pcm = clip_pcm(project, project_dir, clip, settings)?;
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
        return Ok(None);
    }

    let graph = builder.slots(slot).build()?;
    let mut offline = OfflineSequence::new(graph);
    for (slot, source) in sources {
        offline.set_source(slot, Box::new(source))?;
    }

    // The exact samples those video frames cover: the same arithmetic the
    // pipeline uses to decide how much audio belongs to each frame.
    let start_frame = u64::try_from(range.0).unwrap_or(0);
    let first = settings.audio_frames_through(start_frame);
    let last = settings.audio_frames_through(start_frame + frames);
    let sample_rate = Rational::new(settings.sample_rate, 1)
        .ok_or_else(|| SubError::new(codes::INVALID_ARGUMENT, "the export sample rate is zero"))?;
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
    Ok(Some(PcmAudioSource::new(mixed.samples)))
}

/// One audio clip's frames, decoded, folded and resampled to the export
/// format, starting at the clip's own in point.
fn clip_pcm(
    project: &Project,
    project_dir: &Path,
    clip: &sub_model::Clip,
    settings: &ExportSettings,
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

    let decoded = decode_audio(
        &path,
        item.info
            .as_ref()
            .is_some_and(sub_model::StreamInfo::has_video),
    )?;
    let decoded = to_channels(decoded, settings.channels);
    let decoded = resample(decoded, settings.sample_rate)?;
    Ok(trim_to_source_start(decoded, clip.source_range))
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

#[cfg(test)]
mod tests {
    use super::{FrameRange, frames_of, has_audio, sequence_frames, to_channels};
    use sub_audio::decode::Pcm;
    use sub_model::{
        Clip, ColorTags, MediaId, Resolution, Sequence, SequenceSettings, Track, TrackKind,
    };
    use sub_time::{Rational, RationalTime, TimeRange};

    fn sequence(video_frames: i64, audio_frames: i64) -> Sequence {
        let settings = SequenceSettings::new(
            Resolution::new(64, 48).expect("a valid resolution"),
            Rational::FPS_25,
            48_000,
            ColorTags::default(),
        )
        .expect("valid settings");
        let mut sequence = Sequence::new("Main", settings);
        for (kind, frames) in [
            (TrackKind::Video, video_frames),
            (TrackKind::Audio, audio_frames),
        ] {
            if frames == 0 {
                continue;
            }
            let mut track = Track::new("T", kind);
            let range = TimeRange::new(
                RationalTime::zero(Rational::FPS_25),
                RationalTime::new(frames, Rational::FPS_25),
            )
            .expect("a valid range");
            track
                .items
                .push(Clip::new("clip", MediaId::new(), range).into());
            sequence.tracks.push(track);
        }
        sequence
    }

    #[test]
    fn a_range_is_two_frame_numbers_either_of_which_may_be_left_out() {
        assert_eq!(
            FrameRange::parse("0:25").expect("a valid range"),
            FrameRange {
                start: 0,
                end: Some(25)
            }
        );
        assert_eq!(
            FrameRange::parse("12:").expect("a valid range"),
            FrameRange {
                start: 12,
                end: None
            }
        );
        assert_eq!(
            FrameRange::parse(":50").expect("a valid range"),
            FrameRange {
                start: 0,
                end: Some(50)
            }
        );
    }

    #[test]
    fn a_range_that_cannot_be_rendered_is_refused_rather_than_guessed_at() {
        for text in ["25", "a:b", "-1:5", "10:10", "10:5", "1:x"] {
            assert!(
                FrameRange::parse(text).is_err(),
                "--range {text} was accepted",
            );
        }
    }

    #[test]
    fn the_rendered_range_is_clamped_to_the_sequence() {
        let sequence = sequence(100, 0);
        assert_eq!(sequence_frames(&sequence), 100);
        assert_eq!(
            frames_of(&sequence, None).expect("the whole sequence"),
            (0, 100)
        );
        assert_eq!(
            frames_of(
                &sequence,
                Some(FrameRange {
                    start: 10,
                    end: Some(400)
                })
            )
            .expect("a clamped range"),
            (10, 100)
        );
        assert!(
            frames_of(
                &sequence,
                Some(FrameRange {
                    start: 200,
                    end: None
                })
            )
            .is_err(),
            "a range past the end of the sequence was accepted",
        );
    }

    #[test]
    fn an_empty_sequence_is_refused() {
        let empty = sequence(0, 0);
        assert!(frames_of(&empty, None).is_err());
    }

    #[test]
    fn a_sequence_with_no_audio_clips_renders_silent() {
        assert!(!has_audio(&sequence(100, 0)));
        assert!(has_audio(&sequence(100, 50)));
    }

    #[test]
    fn channel_counts_are_adapted_without_dropping_frames() {
        let mono = Pcm {
            sample_rate: 48_000,
            channels: 1,
            samples: vec![0.25, -0.5],
        };
        let stereo = to_channels(mono, 2);
        assert_eq!(stereo.channels, 2);
        assert_eq!(stereo.samples, vec![0.25, 0.25, -0.5, -0.5]);

        let wide = Pcm {
            sample_rate: 48_000,
            channels: 3,
            samples: vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        };
        let folded = to_channels(wide, 2);
        assert_eq!(folded.channels, 2);
        assert_eq!(folded.samples, vec![1.0, 2.0, 4.0, 5.0]);
    }
}
