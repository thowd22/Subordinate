//! The real-time budget allocates nothing while it accounts for a block.
//!
//! Blocks are handed to a plugin from an engine worker, not from the audio
//! callback, but the accounting itself is written to callback rules
//! (docs/PLAN.md §4): a global allocator counts every allocation, reallocation
//! and free while it is armed, and the counters must not move across a long run
//! of timed blocks, faults, bypasses and resets.
//!
//! Everything here is a single test in its own binary: the counters are global,
//! so a second test running beside it would see the other's allocations.

// A global allocator cannot be written in safe Rust; this file is a test, and
// the allocator only counts and forwards to the system one.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use sub_plugin::audio::{BlockFormat, BlockOutcome, RealTimeBudget};

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
        unsafe { System.dealloc(ptr, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn accounting_for_a_block_allocates_nothing() {
    // Everything that may allocate happens before the counters are armed: the
    // format, the budget and the sample buffer are built up front, exactly as a
    // host would build them once per stream rather than once per block.
    let format = BlockFormat::new(512, 2, 48_000).unwrap();
    let mut budget = RealTimeBudget::for_claim(
        format,
        RealTimeBudget::DEFAULT_PERCENT,
        25,
        RealTimeBudget::DEFAULT_TOLERATED_OVERRUNS,
    )
    .unwrap();
    let block = vec![0.25_f32; format.samples()];
    let deadline = budget.budget();

    ARMED.store(true, Ordering::Relaxed);

    for round in 0..10_000_u64 {
        // A block the plugin returned in time, then one it overran, then one it
        // failed outright: the three ways a block can end.
        black_box(format.validate(&block).is_ok());
        black_box(budget.record(deadline / 2));

        if let Some(timer) = budget.start_block() {
            black_box(timer.elapsed());
            black_box(timer.finish());
        }

        if round % 7 == 0 {
            black_box(budget.record(deadline * 4));
        }
        if round % 11 == 0 {
            black_box(budget.fault());
        }
        // Drive it all the way into bypass now and then, and back out, so the
        // latching and the reset are both on the counted path.
        if round % 101 == 0 {
            while !budget.is_bypassed() {
                black_box(budget.fault());
            }
            black_box(budget.record(Duration::ZERO) == BlockOutcome::Bypassed);
            black_box(budget.start_block().is_none());
            budget.reset();
        }
    }

    ARMED.store(false, Ordering::Relaxed);

    assert_eq!(
        (
            ALLOCATIONS.load(Ordering::Relaxed),
            FREES.load(Ordering::Relaxed)
        ),
        (0, 0),
        "the budget allocated or freed while accounting for blocks"
    );
    // A validation failure builds a SubError and does allocate, which is why it
    // is checked only after the counters are disarmed: a malformed block is a
    // bug on the way in, not a per-block cost.
    assert!(format.validate(&block[1..]).is_err());
}
