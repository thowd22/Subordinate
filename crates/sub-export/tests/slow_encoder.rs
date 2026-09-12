//! A slow encoder is not a stopped one (TASK-151, docs/PLAN.md §5.5).
//!
//! The export matrix lost an eight-frame libaom AV1 render to the old fixed
//! 120-second end-of-stream wait: the encoder was producing frames the whole
//! time, just not fast enough for the pipeline's patience. These tests run the
//! slowest encoder this machine has over frames of noise it cannot cheat on,
//! against a patience window several times shorter than the encode takes, and
//! expect the file anyway; the second one asks for the hard limit the request
//! may set and expects the failure to name the element it was waiting on.

use std::time::{Duration, Instant};

use sub_core::SubResult;
use sub_export::{
    Container, EncoderPreferences, EncoderProbe, ExportElements, ExportSettings, VideoCodec,
    VideoFrameSource, element_is_usable, export_with,
};
use sub_time::Rational;

/// Canvas of the slow export: small, so one frame is quick even though the
/// whole encode is not.
const WIDTH: u32 = 640;
/// Canvas height.
const HEIGHT: u32 = 480;
/// Frames exported: enough noise that AV1 takes many times the patience
/// window over the whole file.
const FRAMES: u64 = 240;
/// The patience the export is given. Far less than the whole encode needs, and
/// far more than the gap between two frames coming out of the encoder, so an
/// export the old fixed budget would have abandoned runs to the end here.
const STALL_MS: u64 = 250;

/// Frames of deterministic noise, which no encoder can dispose of cheaply.
///
/// A solid colour encodes in microseconds; noise is what makes this export
/// take long enough for a fixed budget to give up on it.
struct NoiseFrames {
    pixels: Vec<u8>,
    seed: u64,
    left: u64,
}

impl NoiseFrames {
    fn new(bytes: usize, count: u64) -> Self {
        Self {
            pixels: vec![0; bytes],
            seed: 0x2545_f491_4f6c_dd1d,
            left: count,
        }
    }
}

impl VideoFrameSource for NoiseFrames {
    fn next_frame(&mut self) -> SubResult<Option<&[u8]>> {
        if self.left == 0 {
            return Ok(None);
        }
        self.left -= 1;
        // xorshift, so the picture is the same on every machine and every run.
        for pixel in self.pixels.chunks_exact_mut(4) {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 7;
            self.seed ^= self.seed << 17;
            let bytes = self.seed.to_le_bytes();
            pixel[0] = bytes[0];
            pixel[1] = bytes[1];
            pixel[2] = bytes[2];
            pixel[3] = 0xff;
        }
        Ok(Some(&self.pixels))
    }
}

/// AV1 in Matroska with no audio, at the patience the test allows.
fn av1_settings() -> ExportSettings {
    ExportSettings::new(WIDTH, HEIGHT, Rational::FPS_24, Container::Mkv)
        .with_video_codec(VideoCodec::Av1)
        .with_audio_codec(None)
        .with_stall_timeout_ms(STALL_MS)
}

/// The resolved elements, or `None` when this machine has no AV1 encoder.
fn av1_elements(settings: &ExportSettings) -> Option<ExportElements> {
    // The probe is what initialises GStreamer, so it comes before any question
    // about a single element.
    if EncoderProbe::cached().is_err() {
        return None;
    }
    let elements = ExportElements::resolve(settings, &EncoderPreferences::new()).ok()?;
    element_is_usable(Container::Mkv.muxer()).then_some(elements)
}

/// A directory of this process's own, for a file the test writes.
fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sub-export-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    dir
}

#[test]
fn a_slow_encoder_finishes_an_export_that_a_fixed_budget_would_abandon() {
    let settings = av1_settings();
    let Some(elements) = av1_elements(&settings) else {
        eprintln!("skipping: this machine has no usable AV1 encoder or matroskamux");
        return;
    };

    let dir = temp_dir("slow");
    let path = dir.join("slow.mkv");
    let mut frames = NoiseFrames::new(settings.frame_bytes(), FRAMES);

    let started = Instant::now();
    let report = export_with(&path, &settings, &elements, &mut frames, None, &mut |_| {})
        .expect("a slow encoder that is still working must not be abandoned");
    let elapsed = started.elapsed();

    assert_eq!(report.video_frames, FRAMES);
    assert!(
        std::fs::metadata(&path).expect("the file exists").len() > 0,
        "the muxer wrote nothing"
    );
    assert!(
        elapsed > Duration::from_millis(STALL_MS),
        "the whole encode fitted inside its own patience window ({elapsed:?}), \
         so this run says nothing about an export slower than the budget"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn a_hard_limit_in_the_request_fails_with_the_element_it_was_waiting_on() {
    let settings = av1_settings().with_timeout_ms(Some(50));
    let Some(elements) = av1_elements(&settings) else {
        eprintln!("skipping: this machine has no usable AV1 encoder or matroskamux");
        return;
    };

    let dir = temp_dir("limit");
    let path = dir.join("limited.mkv");
    let mut frames = NoiseFrames::new(settings.frame_bytes(), FRAMES);

    let error = export_with(&path, &settings, &elements, &mut frames, None, &mut |_| {})
        .expect_err("a fifty-millisecond limit cannot cover this encode");

    assert_eq!(error.code.as_str(), "export.timeout");
    assert_eq!(
        error.details.get("reason").and_then(|value| value.as_str()),
        Some("time_limit"),
        "the request's own limit is what ran out, not the pipeline's patience"
    );
    let element = error
        .details
        .get("element")
        .and_then(|value| value.as_str())
        .expect("the error names the element it was waiting on")
        .to_owned();
    assert!(
        element == elements.video_encoder || element == Container::Mkv.muxer(),
        "unexpected element {element}"
    );
    assert!(
        error.message.contains(&element),
        "the message names the element too: {}",
        error.message
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}
