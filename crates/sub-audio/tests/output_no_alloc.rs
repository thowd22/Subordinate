//! Proves the cpal callback body allocates nothing, on the hardest path it
//! has: a 48 kHz stereo sequence played on a 44.1 kHz six-channel device that
//! wants 16-bit samples (docs/PLAN.md §4).
//!
//! A global allocator counts every allocation, reallocation and free while it
//! is armed. The test arms it only around
//! [`sub_audio::OutputRenderer::render_i16`], runs ten seconds of audio
//! through it in callback-sized blocks, and asserts the counters never moved.
//! Filling the clip's ring is engine-side work and runs disarmed.
//!
//! Everything here is a single test in its own binary: the counters are
//! global, so a second test running beside it would see the other's
//! allocations.

// A global allocator cannot be written in safe Rust; this file is a test, and
// the allocator only counts and forwards to the system one.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use sub_audio::mixer::{ClipSpec, MixGraphBuilder, MixerConfig, TrackSpec, mixer};
use sub_audio::output::{OutputMetrics, OutputRenderer, OutputSampleFormat, SupportedFormat};
use sub_audio::resample::{PcmWriter, pcm_ring};
use sub_time::{Rational, RationalTime};

/// Whether the counters are watching.
static ARMED: AtomicBool = AtomicBool::new(false);
/// Allocations and reallocations seen while armed.
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
/// Frees seen while armed.
static FREES: AtomicU64 = AtomicU64::new(0);

/// The system allocator, with a counter in front of it.
struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if ARMED.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ARMED.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ARMED.load(Ordering::Relaxed) {
            FREES.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// The sequence rate: what the mixer renders at.
const SEQUENCE_RATE: u32 = 48_000;
/// The device rate: what the callback has to hand the host.
const DEVICE_RATE: u32 = 44_100;
/// Stereo in.
const SEQUENCE_CHANNELS: u16 = 2;
/// A 5.1 device out, so the channel map runs too.
const DEVICE_CHANNELS: u16 = 6;
/// A typical low-latency callback: 512 device frames, about 11.6 ms.
const BLOCK_FRAMES: usize = 512;
/// Ten seconds of playback.
const SECONDS: u64 = 10;

/// Tops the clip's ring up. Engine-side work, so it runs disarmed.
fn refill(writer: &mut PcmWriter, block: &[f32]) {
    while writer.vacant_frames() >= BLOCK_FRAMES {
        writer.write(block);
    }
}

#[test]
fn the_output_callback_allocates_nothing_over_ten_seconds() {
    let rate = Rational::from_integer(SEQUENCE_RATE).expect("a valid rate");
    let graph = MixGraphBuilder::new(SEQUENCE_RATE, SEQUENCE_CHANNELS)
        .track(TrackSpec::new().with_clip(ClipSpec::new(
            0,
            RationalTime::zero(rate),
            RationalTime::new(i64::from(SEQUENCE_RATE) * 600, rate),
        )))
        .build()
        .expect("a graph");
    let (mut control, mixer) = mixer(
        graph,
        MixerConfig {
            // The mixer is asked for more source frames than the device
            // block, because the sequence rate is the higher of the two.
            max_block_frames: BLOCK_FRAMES * 2,
            slot_capacity: 4,
            queue_capacity: 8,
        },
    )
    .expect("a mixer");

    let signal = vec![0.25f32; BLOCK_FRAMES * usize::from(SEQUENCE_CHANNELS)];
    let (mut writer, reader) = pcm_ring(SEQUENCE_CHANNELS, BLOCK_FRAMES * 16).expect("a ring");
    refill(&mut writer, &signal);
    control.install_slot(0, reader).expect("a free slot");

    // The shape a 5.1, 44.1 kHz, 16-bit-only device would negotiate.
    let supported = [SupportedFormat::new(
        DEVICE_CHANNELS,
        DEVICE_RATE,
        DEVICE_RATE,
        OutputSampleFormat::I16,
    )];
    let format = sub_audio::negotiate(&supported, SEQUENCE_RATE, SEQUENCE_CHANNELS)
        .expect("a negotiated format");
    assert!(format.needs_resampling());
    assert!(format.needs_channel_map());
    assert_eq!(format.sample_format(), OutputSampleFormat::I16);

    let metrics = Arc::new(OutputMetrics::new());
    let mut renderer =
        OutputRenderer::new(mixer, format, Arc::clone(&metrics), BLOCK_FRAMES).expect("a renderer");

    let mut out = vec![0i16; BLOCK_FRAMES * usize::from(DEVICE_CHANNELS)];
    let blocks = u64::from(DEVICE_RATE) * SECONDS / BLOCK_FRAMES as u64;

    for _ in 0..blocks {
        // Engine-side work: allowed to allocate, so do it disarmed.
        refill(&mut writer, &signal);
        control.collect_retired();

        // The audio callback itself.
        ARMED.store(true, Ordering::SeqCst);
        renderer.render_i16(&mut out);
        ARMED.store(false, Ordering::SeqCst);
    }

    let allocations = ALLOCATIONS.load(Ordering::SeqCst);
    let frees = FREES.load(Ordering::SeqCst);
    assert_eq!(
        (allocations, frees),
        (0, 0),
        "the output callback allocated {allocations} times and freed {frees} times"
    );
    assert_eq!(
        metrics.frames_rendered(),
        blocks * BLOCK_FRAMES as u64,
        "every callback was accounted for"
    );
    assert_eq!(
        metrics.underrun_frames(),
        0,
        "the ring was kept full, so nothing underran"
    );
    // The signal reaches the device's first two channels and nothing else.
    let frame = &out[..usize::from(DEVICE_CHANNELS)];
    assert!(
        frame[0] > 8_000 && frame[1] > 8_000,
        "front channels: {frame:?}"
    );
    assert!(
        frame[2..].iter().all(|sample| *sample == 0),
        "the channels the sequence does not fill stay silent: {frame:?}"
    );
}
