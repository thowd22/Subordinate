//! A branch that stops draining ends the export, it does not stall it
//! (TASK-146, docs/PLAN.md §5.5).
//!
//! The Windows GPU runner found this the hard way: `nvh264enc` could not open
//! an encode session, rejected the caps and put `not-negotiated` on the bus,
//! and the export went on waiting inside `appsrc`'s blocking push for the rest
//! of the job — one frame written, no error, no file. `appsrc`'s own blocking
//! push cannot read a bus, so the exporter waits for room itself and reads the
//! bus while it waits.
//!
//! The stall is reproduced here without any hardware: an export that has an
//! audio stream and is fed only picture starves its muxer, which stops the
//! video branch exactly as a failed encoder does. Before the fix this test
//! never returned.

use std::time::{Duration, Instant};

use sub_export::{
    AudioCodec, Container, ExportElements, ExportPipeline, ExportSettings, PcmAudioSource,
    SolidFrames, VideoCodec, element_is_usable, export_with,
};
use sub_time::Rational;

/// How long a starved branch is given before the export must give up.
const PUSH_TIMEOUT: Duration = Duration::from_secs(3);

/// Frames the starving test is allowed to push before it fails the test.
///
/// The queue is 32 MiB and these frames are small, so the stall arrives after
/// a few thousand; a cap an order of magnitude above that keeps a broken
/// implementation from running forever.
const PUSH_LIMIT: u64 = 100_000;

/// A fresh temporary directory named after the test using it.
fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sub-export-stall-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    dir
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

/// A Matroska export with both streams, which is what gives the muxer two
/// pads to wait on.
fn settings(width: u32, height: u32) -> ExportSettings {
    ExportSettings::new(width, height, Rational::FPS_24, Container::Mkv)
        .with_video_codec(VideoCodec::H264)
        .with_audio_codec(Some(AudioCodec::Opus))
        .with_audio_format(48_000, 2)
}

#[test]
fn a_branch_that_stops_draining_ends_the_export_instead_of_stalling_it() {
    let settings = settings(64, 48);
    let Some(elements) = can_export(&settings) else {
        eprintln!("skipping: this machine has no usable H.264 or Opus encoder");
        return;
    };
    let dir = temp_dir("starved");
    let path = dir.join("starved.mkv");

    let mut pipeline = ExportPipeline::new(&path, &settings, &elements)
        .expect("the pipeline builds")
        .with_push_timeout(PUSH_TIMEOUT);
    assert!(
        pipeline.has_audio(),
        "the muxer needs a second pad to wait on"
    );

    // Picture and nothing else: the muxer holds the video branch waiting for
    // sound that never comes, which is the same back-pressure a failed
    // encoder applies.
    let pixels = vec![0u8; settings.frame_bytes()];
    let started = Instant::now();
    let mut pushed = 0u64;
    let error = loop {
        match pipeline.push_video_frame(&pixels) {
            Ok(()) => pushed += 1,
            Err(error) => break error,
        }
        assert!(
            pushed < PUSH_LIMIT,
            "the video branch took {pushed} frames without ever filling up"
        );
    };

    assert_eq!(
        error.code,
        sub_export::codes::EXPORT_TIMEOUT,
        "a starved branch is a timeout, not a stall: {error}"
    );
    assert_eq!(error.details["stream"], "video");
    assert!(
        started.elapsed() < PUSH_TIMEOUT * 10,
        "the export gave up after {:?}, which is not promptly",
        started.elapsed()
    );
    assert!(pushed > 0, "the branch took at least one frame first");
    pipeline.abort();
}

#[test]
fn a_uhd_export_with_sound_finishes_rather_than_taking_turns() {
    // 4K RGBA is 33 177 600 bytes, so two of them do not fit the 32 MiB an
    // appsrc queues by default: a UHD export that did not raise the cap would
    // have to empty the queue for every single frame, taking turns with the
    // encoder instead of overlapping with it. The Windows runner's clip is UHD
    // with three audio tracks, which is why this is the shape the regression
    // test takes.
    let settings = settings(3840, 2160);
    assert!(
        settings.frame_bytes() * 2 > 32 * 1024 * 1024,
        "a canvas two of whose frames fit the default cap proves nothing"
    );
    let Some(elements) = can_export(&settings) else {
        eprintln!("skipping: this machine has no usable H.264 or Opus encoder");
        return;
    };
    let dir = temp_dir("uhd");
    let path = dir.join("uhd.mkv");

    let frames = 4;
    let mut video = SolidFrames::new(settings.frame_bytes(), frames);
    let samples = usize::try_from(settings.audio_frames_through(frames)).expect("small")
        * usize::from(settings.channels);
    let mut audio = PcmAudioSource::new(vec![0.0; samples]);
    let report = export_with(
        &path,
        &settings,
        &elements,
        &mut video,
        Some(&mut audio),
        &mut |_| {},
    )
    .expect("a UHD export with sound finishes");

    assert_eq!(report.video_frames, frames);
    assert!(report.audio_frames > 0, "the sound was written too");
    assert!(path.is_file(), "the file is on disk");
}
