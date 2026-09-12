//! What chroma format an export actually writes (TASK-154).
//!
//! The bug this guards is invisible from inside the app: the file exists, the
//! frame count is right and the picture is correct, and only a probe of the
//! finished file says that `x264enc` was handed `Y444` and wrote High 4:4:4
//! Predictive — a profile browsers, phones and hardware decoders refuse. So
//! the test writes a file with the software encoders and reads the profile
//! back off it with the same discoverer a player would use.

use std::path::Path;

use gstreamer_pbutils::prelude::*;
use gstreamer_pbutils::{Discoverer, DiscovererVideoInfo};
use sub_export::{
    ChromaFormat, Container, EncoderPreferences, EncoderProbe, ExportElements, ExportSettings,
    SolidFrames, VideoCodec, element_is_usable, export_with,
};
use sub_time::Rational;

/// Canvas of the test exports: `x265enc` refuses anything below 16x16.
const WIDTH: u32 = 64;
/// Canvas height.
const HEIGHT: u32 = 48;
/// Frames exported: enough for a keyframe and a little more.
const FRAMES: u64 = 6;
/// How long the discoverer waits for one small file.
const DISCOVER_SECONDS: u64 = 30;

/// The H.264 profiles that are 4:2:0, which is every one a delivery file uses.
const H264_420_PROFILES: [&str; 5] = [
    "constrained-baseline",
    "baseline",
    "main",
    "high",
    "progressive-high",
];

/// The H.265 profiles that are 4:2:0.
const H265_420_PROFILES: [&str; 3] = ["main", "main-still-picture", "main-10"];

/// The elements this export needs, or `None` when this machine has not got
/// them.
fn elements_for(settings: &ExportSettings, encoder: &str) -> Option<ExportElements> {
    if EncoderProbe::cached().is_err() || !element_is_usable(settings.container.muxer()) {
        return None;
    }
    let mut preferences = EncoderPreferences::new();
    preferences
        .set_override(settings.video_codec, encoder)
        .ok()?;
    ExportElements::resolve(settings, &preferences).ok()
}

/// A directory of this process's own, emptied first.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sub-export-chroma-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    dir
}

/// The video stream caps of a written file, as a player would read them.
fn video_caps(path: &Path) -> gstreamer::Caps {
    gstreamer::init().expect("GStreamer must initialise");
    let discoverer = Discoverer::new(gstreamer::ClockTime::from_seconds(DISCOVER_SECONDS))
        .expect("a discoverer");
    let uri = gstreamer::glib::filename_to_uri(path, None).expect("a file URI");
    let info = discoverer.discover_uri(&uri).expect("the file is readable");
    let video = info
        .video_streams()
        .into_iter()
        .next()
        .expect("the file carries a video stream");
    video
        .downcast::<DiscovererVideoInfo>()
        .expect("a video stream info")
        .caps()
        .expect("the video stream has caps")
}

/// The `profile` the file declares, which is what a decoder reads first.
fn profile(path: &Path) -> String {
    let caps = video_caps(path);
    let structure = caps.structure(0).expect("the caps have a structure");
    structure
        .get::<String>("profile")
        .unwrap_or_else(|_| panic!("no profile in {caps}"))
}

/// Writes `FRAMES` frames with `encoder` and returns the file's profile.
fn export_and_read_profile(
    name: &str,
    codec: VideoCodec,
    encoder: &str,
    chroma: ChromaFormat,
) -> Option<String> {
    let settings = ExportSettings::new(WIDTH, HEIGHT, Rational::FPS_24, Container::Mkv)
        .with_video_codec(codec)
        .with_chroma(chroma)
        .with_audio_codec(None);
    let Some(elements) = elements_for(&settings, encoder) else {
        eprintln!("skipping: this machine cannot run {encoder} into matroskamux");
        return None;
    };
    assert_eq!(elements.video_encoder, encoder);
    let dir = scratch(name);
    let path = dir.join(format!("{name}.mkv"));
    let mut frames = SolidFrames::new(settings.frame_bytes(), FRAMES);
    let report = export_with(&path, &settings, &elements, &mut frames, None, &mut |_| {})
        .expect("the export runs");
    assert_eq!(report.video_frames, FRAMES);
    let profile = profile(&path);
    let _ = std::fs::remove_dir_all(&dir);
    Some(profile)
}

#[test]
fn the_software_h264_encoder_writes_a_four_two_zero_profile() {
    let Some(profile) =
        export_and_read_profile("h264", VideoCodec::H264, "x264enc", ChromaFormat::Yuv420)
    else {
        return;
    };
    assert!(
        H264_420_PROFILES.contains(&profile.as_str()),
        "a default H.264 export is a 4:2:0 profile, not '{profile}'",
    );
}

#[test]
fn the_software_h265_encoder_writes_a_four_two_zero_profile() {
    let Some(profile) =
        export_and_read_profile("h265", VideoCodec::H265, "x265enc", ChromaFormat::Yuv420)
    else {
        return;
    };
    assert!(
        H265_420_PROFILES.contains(&profile.as_str()),
        "a default H.265 export is a 4:2:0 profile, not '{profile}'",
    );
}

#[test]
fn a_preset_that_asks_for_four_four_four_still_gets_it() {
    let Some(profile) = export_and_read_profile(
        "h264-444",
        VideoCodec::H264,
        "x264enc",
        ChromaFormat::Yuv444,
    ) else {
        return;
    };
    assert!(
        profile.contains("4:4:4"),
        "asking for 4:4:4 writes a 4:4:4 profile, not '{profile}'",
    );
}
