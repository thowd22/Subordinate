//! A pinned encoder is plugged even when this machine ranks it `NONE`
//! (TASK-134).
//!
//! Every VA-API encoder ships ranked `NONE` -- GStreamer never autoplugs an
//! encoder, so the rank means nothing to autoplugging and the plugin leaves it
//! at the floor -- and the export probe used to read that as "unusable", which
//! made `--encoder vah264enc` fail on a stock AMD box. The rank now only keeps
//! an element out of the *automatic* order: naming it is the decision the rank
//! defers.
//!
//! The test deranks `x264enc`, the one encoder every machine here has, because
//! a deranked element is a deranked element whoever deranked it: what the
//! exporter sees is what it would see for a stock `vah264enc`. It lives in its
//! own test binary so the rank is set before anything in the process probes.

use gstreamer::prelude::PluginFeatureExtManual;
use sub_export::{
    Container, EncoderPreferences, EncoderProbe, ExportElements, ExportSettings, SolidFrames,
    VideoCodec, element_is_usable, export_with,
};
use sub_time::Rational;

/// Canvas of the test export: small enough to encode in a moment.
const WIDTH: u32 = 64;
/// Canvas height.
const HEIGHT: u32 = 48;
/// Frames exported.
const FRAMES: u64 = 6;

/// Ranks `x264enc` `NONE` for the life of this process, or reports that this
/// machine has no `x264enc` to derank.
fn derank_x264enc() -> bool {
    if gstreamer::init().is_err() {
        return false;
    }
    let Some(factory) = gstreamer::ElementFactory::find("x264enc") else {
        return false;
    };
    factory.set_rank(gstreamer::Rank::NONE);
    true
}

#[test]
fn a_pinned_encoder_ranked_none_still_renders_a_file() {
    if !derank_x264enc() {
        eprintln!("skipping: this machine has no x264enc");
        return;
    }
    let probe = EncoderProbe::cached().expect("GStreamer must initialise");
    let status = probe.status("x264enc").expect("x264enc is catalogued");
    if !status.present || !status.ready {
        eprintln!("skipping: x264enc does not run here");
        return;
    }
    assert!(status.deranked, "the probe read the NONE rank");
    assert!(
        !status.is_usable(),
        "so the automatic order must not pick it"
    );
    assert!(
        !probe
            .usable(VideoCodec::H264)
            .iter()
            .any(|usable| usable.element == "x264enc"),
        "and it is absent from the usable list"
    );

    let settings = ExportSettings::new(WIDTH, HEIGHT, Rational::FPS_24, Container::Mkv)
        .with_video_codec(VideoCodec::H264)
        .with_audio_codec(None);
    let mut preferences = EncoderPreferences::new();
    preferences
        .set_override(VideoCodec::H264, "x264enc")
        .expect("x264enc encodes H.264");
    let elements =
        ExportElements::resolve(&settings, &preferences).expect("a pinned encoder is honoured");
    assert_eq!(elements.video_encoder, "x264enc");

    if !element_is_usable(settings.container.muxer()) {
        eprintln!("skipping the render: this machine has no matroskamux");
        return;
    }
    let dir =
        std::env::temp_dir().join(format!("sub-export-pinned-deranked-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    let path = dir.join("pinned.mkv");
    let mut frames = SolidFrames::new(settings.frame_bytes(), FRAMES);
    let report = export_with(&path, &settings, &elements, &mut frames, None, &mut |_| {})
        .expect("a pinned encoder encodes even when it is ranked NONE");
    assert_eq!(report.video_encoder, "x264enc");
    assert_eq!(report.video_frames, FRAMES);
    assert!(
        std::fs::metadata(&path).is_ok_and(|meta| meta.len() > 0),
        "the file was written"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
