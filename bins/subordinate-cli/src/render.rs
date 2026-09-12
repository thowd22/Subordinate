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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use sub_core::{SubError, SubResult, codes};
use sub_export::sequence::{
    FramePng, FrameSpan, clips_with_effects, lift_render_error, open_streams, sequence_frames,
    settings_for_sequence, unused_audio_streams_for_export,
};
use sub_export::{
    AudioFrameSource, EncoderPreferences, ExportElements, ExportEvent, ExportJob, ExportSettings,
    PresetLibrary,
};
use sub_model::{Project, Sequence};
use sub_render::RenderContext;
use sub_time::RationalTime;

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
    run_with(options, &mut progress)
}

/// Renders `options`, reporting every export event to `on_event`.
///
/// [`run`] is this with the progress lines that go to stderr; the Command API's
/// `export.render` passes a sink that keeps the latest status where
/// `export.progress` can read it, so an agent follows a render without a
/// terminal.
///
/// # Errors
///
/// As [`run`].
pub fn run_with(options: &Options, on_event: &mut dyn FnMut(&ExportEvent)) -> SubResult<Value> {
    let (project, _report) = crate::project::load(&options.project)?;
    // Absolute, because the decoders open a URI: a project named by a relative
    // path would otherwise resolve its media to a relative path too, and
    // GStreamer takes no such thing.
    let project_dir = absolute(options.project.parent().unwrap_or(Path::new(".")))?;
    let sequence = sequence_of(&project, options.sequence.as_deref())?.clone();
    let library = PresetLibrary::load()?;
    let preset = library.require(&options.preset)?.clone();

    let (settings, mut warnings) = settings_for_sequence(&preset, &sequence)?;
    let range = frames_of(&sequence, options.range)?;
    let frames = frame_count(range)?;
    let span = FrameSpan::new(range.0, frames);

    // The GPU device first, then the encoder probe. Whether an encoder can
    // open a session is a question about *this process*, not only about this
    // machine: on Windows the NVENC elements answer it differently once a
    // graphics device is open, so the probe has to be asked with the process
    // in the state the export will find it (TASK-146).
    let context = RenderContext::headless().map_err(|error| lift_render_error(&error))?;

    let mut preferences = EncoderPreferences::new();
    if let Some(element) = &options.encoder {
        preferences.set_override(settings.video_codec, element)?;
    }
    let elements = ExportElements::resolve(&settings, &preferences)?;

    for clip in clips_with_effects(&sequence) {
        warnings.push(format!(
            "clip '{clip}' carries plugin effects, which a headless render does not run yet",
        ));
    }
    // A source with several audio streams is cut on one of them; the others
    // are named here rather than dropped in silence (TASK-153).
    warnings.extend(unused_audio_streams_for_export(
        &project,
        &sequence,
        &project_dir,
    ));
    // The same adapters the editor window exports through (TASK-135): one
    // bridge from a sequence to an export, so a GUI export and this render
    // write the same file.
    let sequence = Arc::new(sequence);
    let (mut video, mut audio) = open_streams(
        &context,
        Arc::new(project),
        Arc::clone(&sequence),
        &project_dir,
        &settings,
        span,
    );
    let job = ExportJob::new(&options.output, &settings, &elements).with_total_frames(frames);
    let report = job.run(
        &mut video,
        audio
            .as_mut()
            .map(|source| source as &mut dyn AudioFrameSource),
        on_event,
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

/// Composites `time` of `sequence` headlessly and encodes it as a PNG.
///
/// A serving process passes its own render context to
/// [`sub_export::sequence::frame_png`]; this is the headless caller, which
/// makes a context of its own for the one picture. The editor passes the
/// device the viewer paints with instead, so a frame an agent asks the running
/// window for comes off the compositor the user is looking at.
///
/// # Errors
///
/// Whatever the compositor and the decoders return, `render.no_adapter` when
/// this machine enumerates no GPU, and `core.internal` when the picture cannot
/// be encoded.
pub fn frame_png(
    project: &Project,
    project_dir: &Path,
    sequence: &Sequence,
    time: RationalTime,
    width: Option<u32>,
) -> SubResult<FramePng> {
    let context = RenderContext::headless().map_err(|error| lift_render_error(&error))?;
    sub_export::sequence::frame_png(&context, project, project_dir, sequence, time, width)
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

#[cfg(test)]
mod tests {
    use super::{FrameRange, frames_of};
    use sub_export::sequence::{has_audio, sequence_frames};
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
}
