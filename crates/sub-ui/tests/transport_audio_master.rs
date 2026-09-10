//! The viewer's playhead follows the audio clock, not a clock of its own.
//!
//! The app wires the transport to whichever master is running: the audio
//! output's clock while a stream is playing, and the monotonic fallback
//! otherwise (docs/PLAN.md §5.4). This drives the same two paths the app
//! does — a real [`OutputRenderer`] callback publishing an
//! [`AudioClock`], and [`PlaybackScheduler::advance`] with no stream — over a
//! viewer panel painted through the shared harness, and asserts on where the
//! playhead ends up.

mod support;

use std::sync::Arc;

use sub_audio::clock::AudioClock;
use sub_audio::mixer::{MixGraphBuilder, MixerConfig, mixer};
use sub_audio::output::{
    OutputDeviceInfo, OutputMetrics, OutputRenderer, OutputSampleFormat, SupportedFormat,
};
use sub_edit::playback::PlaybackScheduler;
use sub_time::{RationalTime, Rounding};
use sub_ui::viewer::{TransportAction, ViewerPanel};

use std::time::Duration;

/// The sequence sample rate, and the device rate the callback converts to.
const SEQUENCE_RATE: u32 = 48_000;

/// Device frames per callback.
const BLOCK_FRAMES: usize = 1_024;

/// A renderer with the clock its callback publishes into, at a device rate
/// that matches the sequence so a block of device frames is a block of
/// sequence frames.
fn renderer_with_clock() -> (OutputRenderer, Arc<AudioClock>) {
    let graph = MixGraphBuilder::new(SEQUENCE_RATE, 2)
        .build()
        .expect("a valid graph");
    let (_control, mixer) = mixer(graph, MixerConfig::default()).expect("a mixer");
    let device = OutputDeviceInfo::new(
        "harness",
        "Harness Output",
        vec![SupportedFormat::new(
            2,
            SEQUENCE_RATE,
            SEQUENCE_RATE,
            OutputSampleFormat::F32,
        )],
    );
    let format = device.negotiate(SEQUENCE_RATE, 2).expect("a format");
    let renderer = OutputRenderer::new(mixer, format, Arc::new(OutputMetrics::new()), BLOCK_FRAMES)
        .expect("a renderer");
    let clock = Arc::clone(renderer.clock());
    (renderer, clock)
}

#[test]
fn the_viewer_playhead_lands_on_the_frame_the_audio_clock_is_in() {
    let project = support::fixture_project();
    let sequence = support::fixture_sequence(&project);
    let rate = sequence.settings.frame_rate;
    let mut scheduler = PlaybackScheduler::for_sequence(sequence);
    let panel = ViewerPanel::for_sequence(sequence);

    // Space bar: play forward at 1x, exactly as the app's shortcut map does.
    TransportAction::Toggle.apply(&mut scheduler);
    assert!(scheduler.is_playing());

    let (mut renderer, clock) = renderer_with_clock();
    let mut block = vec![0.0_f32; BLOCK_FRAMES * 2];

    let mut harness = support::panel_harness_state(panel, move |ui, panel| {
        panel.ui(ui, None);
    });

    // Twenty callbacks of audio, with the picture chosen for the position
    // each one publishes.
    for _ in 0..20 {
        renderer.render(&mut block);
        let master = clock.position().expect("the callback published a position");
        if let Some(tick) = scheduler.follow(master) {
            harness.state_mut().state.seek_to(tick.position);
        }
        harness.run();
    }

    let audible = clock.position().expect("a position");
    let expected = audible.rescaled_to_rounding(rate, Rounding::Floor);
    assert_eq!(
        harness.state().state.playhead(),
        expected,
        "the viewer is showing the frame the audio clock is inside"
    );
    // Nineteen blocks of 1024 frames are audible: well past frame zero, so
    // this is a playhead that actually moved.
    assert!(harness.state().state.playhead().value() > 0);
}

#[test]
fn with_no_audio_stream_the_viewer_follows_the_fallback_master() {
    let project = support::fixture_project();
    let sequence = support::fixture_sequence(&project);
    let rate = sequence.settings.frame_rate;
    let mut scheduler = PlaybackScheduler::for_sequence(sequence);
    let panel = ViewerPanel::for_sequence(sequence);
    TransportAction::PlayForward.apply(&mut scheduler);

    let mut harness = support::panel_harness_state(panel, move |ui, panel| {
        panel.ui(ui, None);
    });

    // A second of wall clock with no stream open: the fallback master carries
    // the picture the whole frames one second is worth — 23 at 23.976, not a
    // rounded-up 24.
    for _ in 0..10 {
        if let Some(tick) = scheduler.advance(Duration::from_millis(100)) {
            harness.state_mut().state.seek_to(tick.position);
        }
        harness.run();
    }

    let one_second = RationalTime::from_seconds(1).rescaled_to_rounding(rate, Rounding::Floor);
    assert_eq!(harness.state().state.playhead(), one_second);
}
