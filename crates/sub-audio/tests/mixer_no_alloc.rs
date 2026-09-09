//! Proves the mixer callback allocates nothing (docs/PLAN.md §4).
//!
//! A global allocator counts every allocation, reallocation and free while it
//! is armed. The test arms it only around [`sub_audio::Mixer::process`], runs
//! ten seconds of 48 kHz audio through a graph that is edited, seeked and
//! re-slotted along the way, and asserts the counters never moved. The engine
//! side of that work — filling the rings, publishing graphs, freeing what the
//! callback hands back — happens with the allocator disarmed, because that
//! half is allowed to allocate.
//!
//! Everything here is a single test in its own binary: the counters are global,
//! so a second test running beside it would see the other's allocations.

// A global allocator cannot be written in safe Rust; this file is a test, and
// the allocator only counts and forwards to the system one.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use sub_audio::mixer::{ClipSpec, MixGraphBuilder, MixerConfig, TrackSpec, mixer};
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

/// The sequence sample rate and block size the test runs at.
const SAMPLE_RATE: u32 = 48_000;
/// A typical low-latency callback: 512 frames, about 10.7 ms.
const BLOCK_FRAMES: usize = 512;
/// Stereo, like every real output device the MVP opens.
const CHANNELS: usize = 2;
/// Ten seconds of audio, the window the callback must stay silent in.
const SECONDS: u64 = 10;

/// Tops a clip's ring up with a steady tone-ish signal. Engine-side work, so
/// it runs with the allocator disarmed.
fn refill(writer: &mut PcmWriter, block: &[f32]) {
    while writer.vacant_frames() >= BLOCK_FRAMES {
        writer.write(block);
    }
}

#[test]
fn the_callback_allocates_nothing_over_ten_seconds() {
    let rate = Rational::from_integer(SAMPLE_RATE).expect("a valid rate");
    let length = RationalTime::new(i64::from(SAMPLE_RATE) * 60, rate);
    let fade = RationalTime::new(4_800, rate);

    let graph = |gain_db: f64, solo: bool| {
        MixGraphBuilder::new(SAMPLE_RATE, 2)
            .master_gain_db(gain_db)
            .track(
                TrackSpec::new().with_clip(
                    ClipSpec::new(0, RationalTime::zero(rate), length)
                        .with_fades(fade, fade)
                        .with_gain_db(-3.0),
                ),
            )
            .track(TrackSpec::new().with_solo(solo).with_clip(ClipSpec::new(
                1,
                RationalTime::zero(rate),
                length,
            )))
            .track(TrackSpec::new().with_muted(true).with_clip(ClipSpec::new(
                2,
                RationalTime::zero(rate),
                length,
            )))
            .build()
            .expect("a graph")
    };

    let (mut control, mut mixer) = mixer(
        graph(0.0, false),
        MixerConfig {
            max_block_frames: BLOCK_FRAMES,
            slot_capacity: 8,
            queue_capacity: 8,
        },
    )
    .expect("a mixer");

    let signal = vec![0.25f32; BLOCK_FRAMES * CHANNELS];
    let mut writers = Vec::new();
    for slot in 0..3 {
        let (mut writer, reader) = pcm_ring(2, BLOCK_FRAMES * 8).expect("a ring");
        refill(&mut writer, &signal);
        control.install_slot(slot, reader).expect("install");
        writers.push(writer);
    }

    let mut out = vec![0.0f32; BLOCK_FRAMES * CHANNELS];
    let blocks = u64::from(SAMPLE_RATE) * SECONDS / BLOCK_FRAMES as u64;
    let mut rendered = 0u64;

    for block in 0..blocks {
        // Engine-side work: allowed to allocate, so do it disarmed.
        for writer in &mut writers {
            refill(writer, &signal);
        }
        match block % 100 {
            17 => control.publish(graph(-6.0, block % 200 == 17)).unwrap(),
            41 => {
                let (mut writer, reader) = pcm_ring(2, BLOCK_FRAMES * 8).expect("a ring");
                refill(&mut writer, &signal);
                control.install_slot(1, reader).expect("install");
                writers[1] = writer;
            }
            63 => control
                .seek(RationalTime::new(i64::try_from(block).unwrap() * 480, rate))
                .expect("seek"),
            _ => {}
        }
        control.collect_retired();

        // The audio callback itself.
        ARMED.store(true, Ordering::SeqCst);
        let frames = mixer.process(&mut out);
        ARMED.store(false, Ordering::SeqCst);
        rendered += frames as u64;

        assert_eq!(frames, BLOCK_FRAMES);
    }

    let allocations = ALLOCATIONS.load(Ordering::SeqCst);
    let frees = FREES.load(Ordering::SeqCst);
    assert_eq!(
        (allocations, frees),
        (0, 0),
        "the audio callback allocated {allocations} times and freed {frees} times \
         over {rendered} frames"
    );
    assert_eq!(rendered, blocks * BLOCK_FRAMES as u64);
    assert!(
        out.iter().any(|sample| sample.abs() > 0.0),
        "the mixer produced audible output"
    );

    // The rings never ran dry, so the callback was never taking a shortcut.
    assert_eq!(
        mixer.underrun_frames(),
        0,
        "the test kept every clip ring fed"
    );
}
