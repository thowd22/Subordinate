//! The preset's quality actually reaches the encoder (TASK-149,
//! docs/PLAN.md §5.5).
//!
//! The gap this test closes is a silent one: before TASK-149 every preset
//! produced the same file, because nothing ever set a rate-control property on
//! the element the pipeline plugged. So the test is a size comparison — the
//! same noisy source, encoded twice at bitrates an order of magnitude apart,
//! has to come out as two materially different files. A blank source would
//! compress to nothing at any bitrate and prove nothing, so the frames carry
//! deterministic pseudo-random noise that the encoder has to spend bits on.
//!
//! A machine with no usable H.264 encoder skips, exactly as the other export
//! tests do.

use std::path::PathBuf;

use sub_core::SubResult;
use sub_export::{
    Container, ExportElements, ExportSettings, VideoCodec, VideoFrameSource, VideoQuality,
    element_is_usable, export,
};
use sub_time::Rational;

/// Canvas of the test exports: big enough that the bitrate cap bites, small
/// enough to encode in a moment.
const WIDTH: u32 = 320;
/// Canvas height.
const HEIGHT: u32 = 240;
/// Frames encoded per export: two seconds at 24 fps.
const FRAMES: u64 = 48;
/// The starved bitrate, in kbit/s.
const LOW_KBPS: u32 = 100;
/// The generous bitrate, in kbit/s.
const HIGH_KBPS: u32 = 8_000;

/// Settings for a video-only Matroska export at `quality`.
fn settings(quality: VideoQuality) -> ExportSettings {
    ExportSettings::new(WIDTH, HEIGHT, Rational::FPS_24, Container::Mkv)
        .with_video_codec(VideoCodec::H264)
        .with_audio_codec(None)
        .with_video_quality(Some(quality))
}

/// The elements for `settings`, or `None` when this machine cannot encode.
fn can_export(settings: &ExportSettings) -> Option<ExportElements> {
    if sub_export::EncoderProbe::cached().is_err() {
        return None;
    }
    let elements =
        ExportElements::resolve(settings, &sub_export::EncoderPreferences::new()).ok()?;
    element_is_usable(settings.container.muxer()).then_some(elements)
}

/// A fresh temporary directory named after the test using it.
fn temp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("sub-export-quality-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    dir
}

/// Frames of deterministic noise: incompressible enough that the encoder's
/// bitrate ceiling decides how big the file gets.
///
/// The generator is a plain linear congruential sequence seeded once, so every
/// run of the test encodes exactly the same pictures and the comparison is
/// between rate controls rather than between two rolls of a die.
struct NoisyFrames {
    frame: Vec<u8>,
    remaining: u64,
    state: u64,
}

impl NoisyFrames {
    fn new(bytes: usize, count: u64) -> Self {
        Self {
            frame: vec![0; bytes],
            remaining: count,
            state: 0x2545_f491_4f6c_dd1d,
        }
    }

    /// The next pseudo-random byte of the sequence.
    fn next_byte(&mut self) -> u8 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        u8::try_from((self.state >> 33) & 0xff).unwrap_or(0)
    }
}

impl VideoFrameSource for NoisyFrames {
    fn next_frame(&mut self) -> SubResult<Option<&[u8]>> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.remaining -= 1;
        for index in 0..self.frame.len() {
            // Alpha stays opaque; the three colour bytes carry the noise.
            self.frame[index] = if index % 4 == 3 {
                255
            } else {
                self.next_byte()
            };
        }
        Ok(Some(&self.frame))
    }
}

/// Encodes the same noise at `quality` and returns the file's size in bytes.
fn encode(name: &str, quality: VideoQuality) -> Option<u64> {
    let settings = settings(quality);
    can_export(&settings)?;
    let dir = temp_dir(name);
    let path = dir.join(format!("{name}.mkv"));
    let mut frames = NoisyFrames::new(settings.frame_bytes(), FRAMES);
    let report = export(&path, &settings, &mut frames, None).expect("the export runs");
    assert_eq!(report.video_frames, FRAMES);
    let size = std::fs::metadata(&path)
        .expect("the export wrote a file")
        .len();
    let _ = std::fs::remove_dir_all(&dir);
    Some(size)
}

#[test]
fn two_bitrates_over_the_same_source_write_files_of_different_size() {
    let Some(low) = encode("low", VideoQuality::Bitrate { kbps: LOW_KBPS }) else {
        eprintln!("skipping: this machine has no usable H.264 encoder or Matroska muxer");
        return;
    };
    let high = encode("high", VideoQuality::Bitrate { kbps: HIGH_KBPS })
        .expect("the second export has the same elements as the first");
    assert!(low > 0 && high > 0, "both exports must write a file");
    assert!(
        high > low * 2,
        "a {HIGH_KBPS} kbit/s export ({high} bytes) must be materially larger than a \
         {LOW_KBPS} kbit/s one ({low} bytes); if they match, the preset's bitrate is not \
         reaching the encoder"
    );
}

#[test]
fn a_lossless_crf_is_larger_than_a_lossy_one() {
    let Some(lossy) = encode("crf-high", VideoQuality::Crf { value: 40 }) else {
        eprintln!("skipping: this machine has no usable H.264 encoder or Matroska muxer");
        return;
    };
    let lossless = encode("crf-low", VideoQuality::Crf { value: 0 })
        .expect("the second export has the same elements as the first");
    assert!(
        lossless > lossy * 2,
        "CRF 0 ({lossless} bytes) must be materially larger than CRF 40 ({lossy} bytes); if \
         they match, the preset's CRF is not reaching the encoder"
    );
}
