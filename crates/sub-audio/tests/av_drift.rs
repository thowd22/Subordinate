//! A/V drift over the length of the long fixture.
//!
//! Sync is defined by audio: the callback's position is the master and the
//! video scheduler picks the frame that covers it (docs/PLAN.md §5.4). What
//! can go wrong over a long run is drift — a clock that rounds the same way
//! every block accumulates error until the picture is a frame or more away
//! from the sound.
//!
//! This drives ten minutes of real callbacks: the mixer renders at the
//! sequence rate, the renderer converts to a device running at a *different*
//! rate so the sample-rate conversion is exercised, and the clock it
//! publishes drives a `PlaybackScheduler` at the long fixture's frame rate.
//! After every block the frame on screen is compared with the audio position
//! it is supposed to be covering.
//!
//! The timeline is the long fixture's — ten minutes at its frame rate, read
//! from `fixtures/manifest.json` when it is there — but no decode happens
//! here: what is measured is the clock relationship, which is the same
//! whatever pictures the frames carry. Playing the fixture's own decoded
//! media through the same path is the sync harness of TASK-56.

use std::sync::Arc;

use sub_audio::mixer::{MixGraphBuilder, MixerConfig, mixer};
use sub_audio::output::{
    OutputDeviceInfo, OutputMetrics, OutputRenderer, OutputSampleFormat, SupportedFormat,
};
use sub_edit::playback::PlaybackScheduler;
use sub_time::{Rational, RationalTime, Rounding};

/// The fixture whose shape the run is timed against.
const LONG_FIXTURE: &str = "longgop_720p_10min.mp4";

/// The sequence sample rate the mixer renders at.
const SEQUENCE_RATE: u32 = 48_000;

/// The device rate, deliberately not the sequence rate so that every block
/// goes through the sample-rate conversion.
const DEVICE_RATE: u32 = 44_100;

/// Device frames per callback: a common low-latency buffer.
const BLOCK_FRAMES: usize = 512;

/// The long fixture's frame rate and length in seconds, from the manifest
/// when it is there and from its catalogue entry otherwise. The file itself
/// is never opened: these numbers are the timeline being timed.
fn long_fixture_shape() -> (Rational, u64) {
    let listed = sub_test_support::load_manifest()
        .ok()
        .and_then(|manifest| manifest.get(LONG_FIXTURE).cloned());
    let (num, den, duration_ns) = listed.map_or((25, 1, 600_000_000_000), |fixture| {
        let (num, den) = fixture.fps();
        (num, den, fixture.duration_ns)
    });
    let rate = Rational::new(num, den).expect("the long fixture declares a real frame rate");
    (rate, duration_ns / 1_000_000_000)
}

#[test]
fn video_follows_the_audio_clock_without_drifting_over_ten_minutes() {
    let (video_rate, seconds) = long_fixture_shape();
    assert_eq!(seconds, 600, "the long fixture is a ten-minute clip");
    let audio_rate = Rational::from_integer(SEQUENCE_RATE).expect("a valid audio timebase");
    let samples = i64::try_from(seconds).expect("ten minutes fits") * i64::from(SEQUENCE_RATE);

    // What is in the graph does not matter to the clock, only that the
    // transport runs the full ten minutes.
    let graph = MixGraphBuilder::new(SEQUENCE_RATE, 2)
        .build()
        .expect("a valid graph");
    let (_control, mixer) = mixer(graph, MixerConfig::default()).expect("a mixer");

    // A device that offers 44.1 kHz only, so the callback has to resample.
    let device = OutputDeviceInfo::new(
        "drift",
        "Drift Test Device",
        vec![SupportedFormat::new(
            2,
            DEVICE_RATE,
            DEVICE_RATE,
            OutputSampleFormat::F32,
        )],
    );
    let format = device
        .negotiate(SEQUENCE_RATE, 2)
        .expect("the device can be fed from the sequence");
    assert!(format.needs_resampling(), "the drift path must resample");
    let mut renderer =
        OutputRenderer::new(mixer, format, Arc::new(OutputMetrics::new()), BLOCK_FRAMES)
            .expect("a renderer");
    let clock = Arc::clone(renderer.clock());

    let duration =
        RationalTime::new(samples, audio_rate).rescaled_to_rounding(video_rate, Rounding::Ceil);
    let mut scheduler = PlaybackScheduler::new(video_rate);
    scheduler.set_duration(duration);
    scheduler.play_forward();

    // One frame of video, as a count of audio samples times the video rate's
    // numerator: the units the drift is measured in, so that 23.976 and 25
    // are both exact.
    let frame_units = i128::from(video_rate.denominator()) * i128::from(SEQUENCE_RATE);

    let mut block = vec![0.0_f32; BLOCK_FRAMES * 2];
    let blocks = u64::from(DEVICE_RATE) * seconds / BLOCK_FRAMES as u64;
    let mut worst = 0_i128;
    let mut ran_to_block = 0;
    for index in 0..blocks {
        renderer.render(&mut block);
        let master = clock.position().expect("the callback published a position");
        // The picture is chosen for the audio position, never for the last
        // frame shown.
        scheduler.follow(master);
        if !scheduler.is_playing() {
            // The sequence ended: only the last block or two can reach it,
            // and it ends on the last frame.
            assert!(
                blocks - index <= 2,
                "playback stopped {} blocks early",
                blocks - index
            );
            assert_eq!(scheduler.position_frames(), scheduler.last_frame_number());
            break;
        }
        ran_to_block = index;

        let audible = i128::from(master.value());
        let shown = i128::from(scheduler.position_frames());
        let drift = audible * i128::from(video_rate.numerator()) - shown * frame_units;
        assert!(
            (0..frame_units).contains(&drift),
            "video drifted from the audio clock at block {index}: \
             {audible} samples audible, frame {shown} on screen"
        );
        worst = worst.max(drift);
    }

    // Ten minutes in, the transport has rendered the sequence samples those
    // device frames are worth, give or take the frame the converter is
    // holding: the exact rational phase has accumulated no error.
    let transport = i128::from(clock.rendered_frames());
    let expected = i128::from(blocks * BLOCK_FRAMES as u64) * i128::from(SEQUENCE_RATE)
        / i128::from(DEVICE_RATE);
    assert!(
        (transport - expected).abs() <= 2,
        "the audio transport drifted from the device: {transport} rendered, {expected} expected"
    );
    assert!(worst < frame_units, "worst drift reached a whole frame");
    assert!(
        ran_to_block >= blocks - 3,
        "the run ended early at block {ran_to_block} of {blocks}"
    );
}
