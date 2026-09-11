//! The measurements themselves: sequential playback and scrubbing, each
//! timed from the decoder's hand-off to the converted texture on the GPU.
//!
//! Both scenarios drive the production paths — `sub_media::Decoder` and
//! `sub_render::Nv12Converter` — so a regression in either shows up here.
//! Nothing in this module is allowed to fail a run because of the machine it
//! is on: a missing fixture or a machine with no wgpu adapter records a
//! skipped or decode-only scenario instead.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use sub_core::{ErrorCode, SubError, SubResult};
use sub_media::probe::NANOSECONDS;
use sub_media::{Decoder, DecoderOptions, FrameFormat, HardwarePreference, PtsIndex, VideoFrame};
use sub_render::{Nv12Converter, Nv12Geometry, RenderContext, RenderError};
use sub_time::RationalTime;

use crate::codes;
use crate::report::{Gpu, Report, Scenario, ScenarioKind, ScenarioStatus, finish};
use crate::stats::Samples;

/// The fixtures the harness measures, in report order: the 1080p and 4K
/// colour-bar clips that phase 1's exit criteria are stated against.
pub const FIXTURES: [&str; 2] = ["bars_1080p_h264.mp4", "bars_2160p_h264.mp4"];

/// What one run should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Where the fixtures live; `None` means the usual lookup, which honours
    /// `SUB_FIXTURES_DIR`.
    pub fixtures_dir: Option<PathBuf>,
    /// Frames timed per playback scenario.
    pub frames: u64,
    /// Seeks timed per scrub scenario.
    pub seeks: u64,
    /// Frames decoded and uploaded before timing starts, which pay for shader
    /// compilation and the decoder's first allocations.
    pub warmup: u64,
    /// Whether a hardware decoder may be preferred.
    pub hardware: HardwarePreference,
    /// Whether the texture stage runs at all.
    pub use_gpu: bool,
    /// How long a playback step may take before it counts as a stall, in
    /// nanoseconds. `None` counts no stalls, which is what the colour-bar
    /// scenarios want: they measure a rate, not real-time playback.
    pub stall_threshold_nanos: Option<u64>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            fixtures_dir: None,
            frames: 60,
            seeks: 30,
            warmup: 3,
            hardware: HardwarePreference::Prefer,
            use_gpu: true,
            stall_threshold_nanos: None,
        }
    }
}

/// Runs every scenario and returns the report.
///
/// # Errors
///
/// Only genuine failures of the paths under test: a decoder that errors, a
/// device that stops responding, a frame whose layout is not NV12. A machine
/// without fixtures or without an adapter still returns a report.
pub fn run(options: &Options) -> SubResult<Report> {
    let context = if options.use_gpu {
        open_adapter()?
    } else {
        tracing::info!("the texture stage was switched off; measuring decode only");
        None
    };
    let gpu = context.as_ref().map(|context| Gpu {
        adapter: context.describe(),
        software: context.is_software(),
    });
    let mut report = Report::new(gpu);

    let dir = options
        .fixtures_dir
        .clone()
        .unwrap_or_else(sub_test_support::fixtures_dir);

    for name in FIXTURES {
        let (path, duration_nanos) = match locate(&dir, name) {
            Ok(found) => found,
            Err(reason) => {
                tracing::warn!(fixture = name, %reason, "skipping a fixture");
                report
                    .scenarios
                    .push(Scenario::skipped(name, ScenarioKind::Playback, &reason));
                report
                    .scenarios
                    .push(Scenario::skipped(name, ScenarioKind::Scrub, &reason));
                continue;
            }
        };
        report
            .scenarios
            .push(measure_playback(&path, name, options, context.as_ref())?);
        report.scenarios.push(measure_scrub(
            &path,
            name,
            duration_nanos,
            options,
            context.as_ref(),
        )?);
    }
    Ok(report)
}

/// The adapter to measure on, or `None` when this machine has none.
///
/// # Errors
///
/// Any adapter failure other than "there is no adapter": a driver that
/// refuses a device is a real problem, not an absent GPU.
pub fn open_adapter() -> SubResult<Option<RenderContext>> {
    match RenderContext::headless() {
        Ok(context) => Ok(Some(context)),
        Err(RenderError::NoAdapter { backends }) => {
            tracing::warn!(%backends, "no wgpu adapter; measuring decode only");
            Ok(None)
        }
        Err(error) => Err(render_error(&error)),
    }
}

/// The path and exact duration of a fixture, or why it cannot be measured.
fn locate(dir: &Path, name: &str) -> Result<(PathBuf, u64), String> {
    let path = sub_test_support::fixture_from(dir, name).map_err(|error| error.to_string())?;
    let manifest = sub_test_support::load_manifest_from(dir).map_err(|error| error.to_string())?;
    let duration_nanos = manifest
        .get(name)
        .ok_or_else(|| format!("no fixture named '{name}' in the manifest"))?
        .duration_ns;
    Ok((path, duration_nanos))
}

/// Decode `options.frames` consecutive frames and put every one on the GPU.
pub fn measure_playback(
    path: &Path,
    name: &str,
    options: &Options,
    context: Option<&RenderContext>,
) -> SubResult<Scenario> {
    let mut decoder = Decoder::open_with(path, decoder_options(options))?;
    let mut uploader = Uploader::new(context);
    let mut scenario = new_scenario(name, ScenarioKind::Playback);

    for _ in 0..options.warmup {
        let Some(frame) = decoder.next_frame()? else {
            break;
        };
        uploader.upload(&frame)?;
    }

    let mut decode = Samples::new();
    let mut upload = Samples::new();
    let mut total = Samples::new();
    let started = Instant::now();
    for _ in 0..options.frames {
        let frame_started = Instant::now();
        let Some(frame) = decoder.next_frame()? else {
            break;
        };
        let handed_off = Instant::now();
        let upload_nanos = uploader.upload(&frame)?;
        decode.push(nanos_between(frame_started, handed_off));
        upload.push(upload_nanos);
        total.push(nanos_between(frame_started, Instant::now()));
        describe(&mut scenario, &frame);
    }
    let wall = nanos_between(started, Instant::now());

    if total.is_empty() {
        return Ok(Scenario::skipped(
            name,
            ScenarioKind::Playback,
            "the decoder delivered no frames",
        ));
    }
    let frames = u64::try_from(total.len()).unwrap_or(u64::MAX);
    scenario.decoder = decoder.decoder_element();
    scenario.decode = decode.summary();
    scenario.upload = context.and(upload.summary());
    scenario.decode_to_texture = context.and(total.summary());
    if let Some(threshold) = options.stall_threshold_nanos {
        scenario.stall_threshold_nanos = Some(threshold);
        scenario.stalls = Some(total.count_over(threshold));
    }
    finish(&mut scenario, frames, wall);
    Ok(scenario)
}

/// Seek across the whole file and put every landed frame on the GPU.
///
/// This is what dragging the scrub bar costs: each step is a frame-accurate
/// seek followed by the same upload the viewer does.
pub fn measure_scrub(
    path: &Path,
    name: &str,
    duration_nanos: u64,
    options: &Options,
    context: Option<&RenderContext>,
) -> SubResult<Scenario> {
    let mut decoder = Decoder::open_with(path, decoder_options(options))?;
    // The viewer scrubs against a PTS index -- it is built in the background as
    // soon as a clip is imported -- so the harness measures the seek path the
    // way the product uses it: the index says which keyframe a target needs, so
    // a step never flushes the pipeline when the target is in the GOP the
    // decoder is already in, and a step that must seek aims at the keyframe
    // itself rather than at wherever the demuxer's snap falls (TASK-133).
    // Building it is a parse pass, not a decode, and it happens before anything
    // is timed. A file this machine cannot index is still measured, without
    // one.
    match PtsIndex::build(path) {
        Ok(index) => decoder.set_index(Arc::new(index)),
        Err(error) => {
            tracing::warn!(fixture = name, %error, "scrubbing without a PTS index");
        }
    }
    let mut uploader = Uploader::new(context);
    let mut scenario = new_scenario(name, ScenarioKind::Scrub);

    for target in scrub_targets(duration_nanos, options.warmup) {
        if let Some(frame) = decoder.seek_to(target)? {
            uploader.upload(&frame)?;
        }
    }

    let targets = scrub_targets(duration_nanos, options.seeks);
    let mut decode = Samples::new();
    let mut upload = Samples::new();
    let mut total = Samples::new();
    // The two halves of a scrub step, kept apart because they answer to
    // different fixes: the flushing keyframe seek, and the decode-forward from
    // that keyframe to the frame that was asked for (TASK-133).
    let mut seek = Samples::new();
    let mut forward = Samples::new();
    let mut frames_decoded = 0_u64;
    let mut seeks_issued = 0_u64;
    let started = Instant::now();
    for target in targets {
        let step_started = Instant::now();
        let Some(frame) = decoder.seek_to(target)? else {
            continue;
        };
        let seeked = Instant::now();
        let upload_nanos = uploader.upload(&frame)?;
        decode.push(nanos_between(step_started, seeked));
        if let Some(timing) = decoder.last_seek_timing() {
            seek.push(timing.seek_nanos);
            forward.push(timing.decode_forward_nanos);
            frames_decoded = frames_decoded.saturating_add(timing.frames_decoded);
            seeks_issued = seeks_issued.saturating_add(timing.seeks_issued);
        }
        upload.push(upload_nanos);
        total.push(nanos_between(step_started, Instant::now()));
        describe(&mut scenario, &frame);
    }
    let wall = nanos_between(started, Instant::now());

    if total.is_empty() {
        return Ok(Scenario::skipped(
            name,
            ScenarioKind::Scrub,
            "no seek landed on a frame",
        ));
    }
    let frames = u64::try_from(total.len()).unwrap_or(u64::MAX);
    scenario.decoder = decoder.decoder_element();
    scenario.decode = decode.summary();
    scenario.upload = context.and(upload.summary());
    scenario.decode_to_texture = context.and(total.summary());
    scenario.seek = seek.summary();
    scenario.decode_forward = forward.summary();
    scenario.frames_decoded = Some(frames_decoded);
    scenario.seeks_issued = Some(seeks_issued);
    finish(&mut scenario, frames, wall);
    Ok(scenario)
}

/// Where a scrub of `count` steps lands inside a clip of `duration_nanos`.
///
/// The targets alternate between the head and the tail and work inward, so a
/// run covers the whole file and every step is a real jump rather than the
/// decode-forward the viewer gets for free while playing. Positions are exact
/// nanosecond counts; nothing here is a float.
pub fn scrub_targets(duration_nanos: u64, count: u64) -> Vec<RationalTime> {
    if duration_nanos == 0 || count == 0 {
        return Vec::new();
    }
    let step = duration_nanos / (count + 1);
    if step == 0 {
        return Vec::new();
    }
    (0..count)
        .map(|index| {
            let offset = (index / 2 + 1) * step;
            let nanos = if index % 2 == 0 {
                offset
            } else {
                duration_nanos - offset
            };
            RationalTime::new(i64::try_from(nanos).unwrap_or(i64::MAX), NANOSECONDS)
        })
        .collect()
}

/// A measured scenario with nothing filled in yet.
fn new_scenario(name: &str, kind: ScenarioKind) -> Scenario {
    let mut scenario = Scenario::skipped(name, kind, "not measured");
    scenario.status = ScenarioStatus::Measured;
    scenario.skipped_reason = None;
    scenario
}

/// Records the picture size the fixture actually decoded to.
fn describe(scenario: &mut Scenario, frame: &VideoFrame) {
    scenario.width = frame.width();
    scenario.height = frame.height();
}

/// How the harness opens every decoder: NV12, video only, so the frames go
/// straight into the compositor's upload path.
fn decoder_options(options: &Options) -> DecoderOptions {
    DecoderOptions {
        hardware: options.hardware,
        format: FrameFormat::Nv12,
        ..DecoderOptions::default()
    }
}

/// Puts decoded frames through the real NV12 upload and conversion.
///
/// The converter is rebuilt only when the picture geometry changes, which is
/// what the viewer does too: a per-frame rebuild would time pipeline creation
/// rather than the upload.
struct Uploader<'a> {
    context: Option<&'a RenderContext>,
    converter: Option<Nv12Converter>,
}

impl<'a> Uploader<'a> {
    /// An uploader for `context`, or a no-op one when there is no adapter.
    fn new(context: Option<&'a RenderContext>) -> Self {
        Self {
            context,
            converter: None,
        }
    }

    /// Uploads and converts one frame, waited to GPU completion, and returns
    /// what that cost in nanoseconds. Zero when there is no adapter.
    ///
    /// # Errors
    ///
    /// [`codes::FRAME_LAYOUT`] when the frame is not a usable NV12 picture,
    /// plus whatever the upload path or the device reports.
    fn upload(&mut self, frame: &VideoFrame) -> SubResult<u64> {
        let Some(context) = self.context else {
            return Ok(0);
        };
        let geometry = geometry_of(frame)?;
        let converter = match &self.converter {
            Some(existing) if existing.geometry() == geometry => existing,
            _ => self
                .converter
                .insert(Nv12Converter::new(context.device(), geometry)),
        };
        let (y, uv) = planes_of(frame)?;

        let started = Instant::now();
        converter
            .submit_frame(context.device(), context.queue(), y, uv)
            .map_err(|error| render_error(&error))?;
        context
            .device()
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| {
                SubError::wrap(codes::GPU_LOST, "the GPU did not finish the frame", &error)
            })?;
        Ok(nanos_between(started, Instant::now()))
    }
}

/// The NV12 layout of a decoded frame.
fn geometry_of(frame: &VideoFrame) -> SubResult<Nv12Geometry> {
    if frame.format() != FrameFormat::Nv12 {
        return Err(SubError::new(
            codes::FRAME_LAYOUT,
            format!(
                "the decoder delivered {} frames, not NV12",
                frame.format().as_str()
            ),
        ));
    }
    let y_stride = plane_stride(frame, 0)?;
    let uv_stride = plane_stride(frame, 1)?;
    Nv12Geometry::new(frame.width(), frame.height(), y_stride, uv_stride)
        .map_err(|error| render_error(&error))
}

/// One plane's row stride, or an error naming the plane that is missing.
fn plane_stride(frame: &VideoFrame, plane: u32) -> SubResult<u32> {
    frame.plane_stride(plane).ok_or_else(|| {
        SubError::new(
            codes::FRAME_LAYOUT,
            format!("the frame has no plane {plane}"),
        )
    })
}

/// Both plane slices of an NV12 frame.
fn planes_of(frame: &VideoFrame) -> SubResult<(&[u8], &[u8])> {
    let y = frame.plane_data(0).ok_or_else(|| {
        SubError::new(
            codes::FRAME_LAYOUT,
            "the frame has no luma plane".to_owned(),
        )
    })?;
    let uv = frame.plane_data(1).ok_or_else(|| {
        SubError::new(
            codes::FRAME_LAYOUT,
            "the frame has no chroma plane".to_owned(),
        )
    })?;
    Ok((y, uv))
}

/// A [`RenderError`] as a [`SubError`], keeping its stable `render.*` code.
fn render_error(error: &RenderError) -> SubError {
    let code = ErrorCode::parse(error.code()).unwrap_or(codes::GPU_LOST);
    SubError::new(code, error.to_string())
}

/// Nanoseconds between two instants, saturating rather than panicking.
fn nanos_between(start: Instant, end: Instant) -> u64 {
    u64::try_from(end.saturating_duration_since(start).as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{FIXTURES, Options, scrub_targets};
    use sub_media::probe::NANOSECONDS;

    #[test]
    fn the_measured_fixtures_are_the_1080p_and_4k_clips() {
        assert_eq!(FIXTURES, ["bars_1080p_h264.mp4", "bars_2160p_h264.mp4"]);
    }

    #[test]
    fn scrub_targets_stay_inside_the_clip() {
        let duration: i64 = 5_000_000_000;
        let targets = scrub_targets(u64::try_from(duration).expect("a positive duration"), 8);
        assert_eq!(targets.len(), 8);
        for target in &targets {
            assert_eq!(target.rate(), NANOSECONDS);
            assert!(target.value() > 0, "{target:?} is not inside the clip");
            assert!(target.value() < duration, "{target:?} is past the end");
        }
    }

    #[test]
    fn scrub_targets_alternate_between_the_head_and_the_tail() {
        // step = 5 s / 5 = 1 s, so the walk is 1 s, 4 s, 2 s, 3 s.
        let targets = scrub_targets(5_000_000_000, 4);
        let values: Vec<i64> = targets.iter().map(|target| target.value()).collect();
        assert_eq!(
            values,
            vec![1_000_000_000, 4_000_000_000, 2_000_000_000, 3_000_000_000]
        );
    }

    #[test]
    fn a_clip_too_short_to_divide_yields_no_targets() {
        assert!(scrub_targets(0, 10).is_empty());
        assert!(scrub_targets(1_000, 0).is_empty());
        // Ten steps do not fit in nine nanoseconds.
        assert!(scrub_targets(9, 10).is_empty());
    }

    #[test]
    fn the_defaults_measure_more_than_one_frame() {
        let options = Options::default();
        assert!(options.frames > 1);
        assert!(options.seeks > 1);
        assert!(options.use_gpu);
    }
}
