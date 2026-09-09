//! Decode-ahead worker with a bounded ring buffer, one per active clip
//! (docs/PLAN.md §4).
//!
//! Playback must never decode on the compositor thread: a stalled demuxer or a
//! slow long-GOP seek would show up as a dropped preview frame. [`DecodeAhead`]
//! moves one [`Decoder`] onto a worker thread of its own and keeps a bounded
//! number of decoded pictures ready ahead of the playhead. The compositor pops
//! them; the worker refills.
//!
//! ```no_run
//! # fn main() -> sub_core::SubResult<()> {
//! use sub_media::DecodeAhead;
//!
//! let mut ahead = DecodeAhead::open(std::path::Path::new("/media/a.mp4"))?;
//! while let Some(frame) = ahead.next_frame()? {
//!     println!("{} ns, {} buffered", frame.pts().value(), ahead.occupancy());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Three properties are the point of this type:
//!
//! * **Bounded.** The ring is a `VecDeque` allocated once to
//!   [`DecodeAheadOptions::capacity`] and never grown. Decoding a frame pushes
//!   into that reservation and popping hands the slot back, so steady-state
//!   decode allocates nothing per frame beyond the picture GStreamer itself
//!   hands over.
//! * **Backpressured.** A full ring blocks the worker on a condition variable
//!   rather than queueing without limit, so a paused playhead cannot make a
//!   decoder read a whole file into memory.
//! * **Seek-clean.** [`DecodeAhead::seek_to`] drops every buffered frame and
//!   bumps a generation counter. Pictures the worker was already decoding for
//!   the old generation are discarded rather than delivered, so the first frame
//!   after a seek is the frame at or after the seek target — the same guarantee
//!   [`Decoder::seek_to`] gives.
//!
//! Occupancy, per-frame decode time, drops and backpressure stalls are counted
//! in [`DecodeAheadStats`] and emitted on the `sub_media::decode_ahead` tracing
//! target: `trace` per frame, `debug` for seeks and end of stream.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use sub_core::{SubError, SubResult};
use sub_time::RationalTime;

use crate::codes;
use crate::decode::{Decoder, DecoderOptions, VideoFrame};

/// Frames kept ahead of the playhead when a caller does not choose.
///
/// Eight pictures is about a third of a second at 25 fps: long enough to ride
/// out a slow keyframe seek, short enough that eight 1080p NV12 frames (some
/// 25 MB) stay a rounding error against the frame cache budget.
pub const DEFAULT_CAPACITY: usize = 8;

/// How long a blocked consumer or worker waits before re-checking state.
///
/// Every state change notifies, so this only bounds the wait when a
/// notification is lost with a worker that died; it is not the mechanism.
const WAIT_SLICE: Duration = Duration::from_millis(200);

/// How a [`DecodeAhead`] should be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeAheadOptions {
    /// How many decoded frames the ring holds. A value below one is treated as
    /// one; the ring is allocated to this size once and never grows.
    pub capacity: usize,
    /// Options for the [`Decoder`] the worker owns.
    pub decoder: DecoderOptions,
}

impl Default for DecodeAheadOptions {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_CAPACITY,
            decoder: DecoderOptions::default(),
        }
    }
}

/// A snapshot of what one decode-ahead worker has been doing.
///
/// Counters are cumulative over the life of the handle and are never reset by a
/// seek; `occupancy` is the instantaneous depth of the ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DecodeAheadStats {
    /// Frames sitting in the ring right now.
    pub occupancy: usize,
    /// Frames the ring can hold.
    pub capacity: usize,
    /// Frames the worker decoded and kept.
    pub frames_decoded: u64,
    /// Frames handed to the consumer.
    pub frames_delivered: u64,
    /// Frames thrown away: buffered when a seek arrived, or decoded for a
    /// generation a seek had already replaced.
    pub frames_dropped: u64,
    /// Seeks the worker performed.
    pub seeks: u64,
    /// Times the worker found the ring full and had to wait for space. That is
    /// backpressure doing its job, not an error.
    pub backpressure_waits: u64,
    /// Total time spent inside the decoder producing the kept frames.
    pub decode_time_total: Duration,
    /// Longest single frame decode, seeks included.
    pub decode_time_max: Duration,
}

impl DecodeAheadStats {
    /// Mean decode time per kept frame, or `None` before the first frame.
    pub fn decode_time_mean(&self) -> Option<Duration> {
        let frames = u32::try_from(self.frames_decoded).ok().filter(|n| *n > 0)?;
        Some(self.decode_time_total / frames)
    }
}

/// What the consumer waits for and the worker fills.
struct RingState {
    /// The bounded ring itself, allocated once to `capacity`.
    frames: VecDeque<VideoFrame>,
    /// How many frames `frames` may hold.
    capacity: usize,
    /// Bumped by every seek. A frame decoded for an older generation is stale.
    generation: u64,
    /// A seek the worker has not picked up yet.
    seek_request: Option<RationalTime>,
    /// True once the worker reached end of stream for `generation`.
    eos: bool,
    /// The error the worker stopped on, until a consumer takes it.
    error: Option<SubError>,
    /// Set when the handle is dropped, to wind the worker up.
    stopping: bool,
    /// Metrics, read out by [`DecodeAhead::stats`].
    stats: DecodeAheadStats,
}

/// The handle and its worker share exactly this.
struct Shared {
    state: Mutex<RingState>,
    /// Signalled when the worker may make progress: space freed, a seek
    /// requested, or a stop asked for.
    space: Condvar,
    /// Signalled when the consumer may make progress: a frame pushed, end of
    /// stream reached, or an error recorded.
    filled: Condvar,
}

impl Shared {
    fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(RingState {
                frames: VecDeque::with_capacity(capacity),
                capacity,
                generation: 0,
                seek_request: None,
                eos: false,
                error: None,
                stopping: false,
                stats: DecodeAheadStats {
                    capacity,
                    ..DecodeAheadStats::default()
                },
            }),
            space: Condvar::new(),
            filled: Condvar::new(),
        }
    }

    /// Locks the state, turning a worker panic into a `media.decode_failed`
    /// rather than a panic of its own.
    fn lock(&self) -> SubResult<MutexGuard<'_, RingState>> {
        self.state
            .lock()
            .map_err(|_| SubError::new(codes::DECODE_FAILED, "the decode-ahead worker panicked"))
    }
}

/// What the worker decided to do next.
enum Step {
    /// Decode one frame for this generation.
    Decode(u64),
    /// Seek to this target for this generation.
    Seek(RationalTime, u64),
    /// Wind up.
    Stop,
}

/// A decoder running ahead of the playhead on a worker thread of its own.
///
/// See the [module documentation](self) for the ring, the backpressure and the
/// seek behaviour.
pub struct DecodeAhead {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    capacity: usize,
}

impl DecodeAhead {
    /// Opens `path` with the default options and starts decoding ahead.
    ///
    /// # Errors
    ///
    /// Returns whatever [`Decoder::open`] returns: the file is opened on the
    /// calling thread, so an unreadable or undecodable file fails here rather
    /// than on the first frame.
    pub fn open(path: &Path) -> SubResult<Self> {
        Self::open_with(path, DecodeAheadOptions::default())
    }

    /// Opens `path` with explicit options and starts decoding ahead.
    ///
    /// # Errors
    ///
    /// Returns whatever [`Decoder::open_with`] returns.
    pub fn open_with(path: &Path, options: DecodeAheadOptions) -> SubResult<Self> {
        let decoder = Decoder::open_with(path, options.decoder)?;
        Ok(Self::with_decoder(decoder, options.capacity))
    }

    /// Wraps an already-open decoder, taking ownership of it.
    ///
    /// The decoder moves onto the worker thread and cannot be reached again;
    /// everything a caller needs from it is on this handle.
    pub fn with_decoder(decoder: Decoder, capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let shared = Arc::new(Shared::new(capacity));
        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("sub-decode-ahead".to_owned())
            .spawn(move || run_worker(&worker_shared, decoder))
            .ok();
        Self {
            shared,
            worker,
            capacity,
        }
    }

    /// Pops the next frame, blocking until one is ready, or `None` at end of
    /// stream.
    ///
    /// Frames come out in presentation order, and after [`Self::seek_to`] the
    /// first one is the frame at or after the seek target.
    ///
    /// # Errors
    ///
    /// Returns the error the worker's decode or seek failed with — every code
    /// [`Decoder::next_frame`] and [`Decoder::seek_to`] document — and
    /// `media.decode_failed` when the worker thread is gone.
    pub fn next_frame(&mut self) -> SubResult<Option<VideoFrame>> {
        let mut state = self.shared.lock()?;
        loop {
            if let Some(frame) = state.frames.pop_front() {
                state.stats.frames_delivered += 1;
                state.stats.occupancy = state.frames.len();
                drop(state);
                self.shared.space.notify_all();
                return Ok(Some(frame));
            }
            if let Some(error) = state.error.take() {
                return Err(error);
            }
            if state.eos {
                return Ok(None);
            }
            if self.worker.as_ref().is_none_or(JoinHandle::is_finished) {
                return Err(SubError::new(
                    codes::DECODE_FAILED,
                    "the decode-ahead worker stopped without delivering a frame",
                ));
            }
            let (guard, _) = self
                .shared
                .filled
                .wait_timeout(state, WAIT_SLICE)
                .map_err(|_| {
                    SubError::new(codes::DECODE_FAILED, "the decode-ahead worker panicked")
                })?;
            state = guard;
        }
    }

    /// Asks the worker to seek to `target`, dropping every buffered frame.
    ///
    /// The seek runs on the worker thread, so this returns as soon as the
    /// request is recorded; the next [`Self::next_frame`] delivers the frame at
    /// or after `target`, or the error the seek failed with. A second seek
    /// issued before the first has finished simply replaces it — scrubbing
    /// never queues seeks.
    ///
    /// # Errors
    ///
    /// Returns `media.decode_failed` when the worker thread is gone.
    pub fn seek_to(&mut self, target: RationalTime) -> SubResult<()> {
        let mut state = self.shared.lock()?;
        let dropped = state.frames.len();
        // `clear` keeps the reservation: the ring is never reallocated.
        state.frames.clear();
        state.stats.frames_dropped += u64::try_from(dropped).unwrap_or(u64::MAX);
        state.stats.occupancy = 0;
        state.generation += 1;
        state.seek_request = Some(target);
        state.eos = false;
        state.error = None;
        tracing::debug!(
            target: "sub_media::decode_ahead",
            target_ns = target.value(),
            generation = state.generation,
            dropped,
            "decode-ahead seek requested"
        );
        drop(state);
        self.shared.space.notify_all();
        Ok(())
    }

    /// Frames waiting in the ring right now.
    pub fn occupancy(&self) -> usize {
        self.shared.lock().map_or(0, |state| state.frames.len())
    }

    /// Frames the ring can hold.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// A snapshot of the worker's metrics.
    pub fn stats(&self) -> DecodeAheadStats {
        self.shared.lock().map_or(
            DecodeAheadStats {
                capacity: self.capacity,
                ..DecodeAheadStats::default()
            },
            |state| DecodeAheadStats {
                occupancy: state.frames.len(),
                ..state.stats
            },
        )
    }
}

impl Drop for DecodeAhead {
    fn drop(&mut self) {
        if let Ok(mut state) = self.shared.lock() {
            state.stopping = true;
            // Freeing the ring lets a worker parked on backpressure notice the
            // stop instead of sitting there until its wait slice runs out.
            state.frames.clear();
        }
        self.shared.space.notify_all();
        if let Some(worker) = self.worker.take() {
            // The worker can be inside a decode with its own frame budget, so
            // this waits at most that budget.
            let _ = worker.join();
        }
    }
}

impl std::fmt::Debug for DecodeAhead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stats = self.stats();
        f.debug_struct("DecodeAhead")
            .field("occupancy", &stats.occupancy)
            .field("capacity", &stats.capacity)
            .field("frames_decoded", &stats.frames_decoded)
            .finish_non_exhaustive()
    }
}

/// Decides what the worker does next, blocking while there is nothing to do.
///
/// A full ring, an error waiting to be taken and end of stream all park the
/// worker on the `space` condition variable; a seek or a stop wakes it.
fn next_step(shared: &Shared) -> Step {
    let Ok(mut state) = shared.lock() else {
        return Step::Stop;
    };
    loop {
        if state.stopping {
            return Step::Stop;
        }
        if let Some(target) = state.seek_request.take() {
            return Step::Seek(target, state.generation);
        }
        let idle = state.eos || state.error.is_some();
        if !idle && state.frames.len() < state.capacity {
            return Step::Decode(state.generation);
        }
        if !idle {
            state.stats.backpressure_waits += 1;
            tracing::trace!(
                target: "sub_media::decode_ahead",
                occupancy = state.frames.len(),
                capacity = state.capacity,
                "decode-ahead ring full, worker waiting"
            );
        }
        let Ok((guard, _)) = shared.space.wait_timeout(state, WAIT_SLICE) else {
            return Step::Stop;
        };
        state = guard;
    }
}

/// Records the outcome of one decode or seek, unless a newer generation has
/// made it stale.
///
/// Returns `false` when the worker should wind up.
fn record(
    shared: &Shared,
    generation: u64,
    outcome: SubResult<Option<VideoFrame>>,
    elapsed: Duration,
) -> bool {
    let Ok(mut state) = shared.lock() else {
        return false;
    };
    if state.generation != generation {
        // A seek landed while this frame was being decoded: it belongs to a
        // position the consumer has already left.
        if matches!(outcome, Ok(Some(_))) {
            state.stats.frames_dropped += 1;
        }
        return !state.stopping;
    }
    state.stats.decode_time_max = state.stats.decode_time_max.max(elapsed);
    match outcome {
        Ok(Some(frame)) => {
            let pts_ns = frame.pts().value();
            state.stats.frames_decoded += 1;
            state.stats.decode_time_total += elapsed;
            // The ring is only ever pushed into with room to spare, so this
            // never grows the reservation made at construction.
            debug_assert!(state.frames.len() < state.capacity, "ring overrun");
            state.frames.push_back(frame);
            state.stats.occupancy = state.frames.len();
            tracing::trace!(
                target: "sub_media::decode_ahead",
                pts_ns,
                decode_us = elapsed.as_micros(),
                occupancy = state.frames.len(),
                capacity = state.capacity,
                "decode-ahead frame ready"
            );
        }
        Ok(None) => {
            state.eos = true;
            tracing::debug!(
                target: "sub_media::decode_ahead",
                frames_decoded = state.stats.frames_decoded,
                occupancy = state.frames.len(),
                "decode-ahead reached end of stream"
            );
        }
        Err(error) => {
            tracing::debug!(
                target: "sub_media::decode_ahead",
                code = error.code.as_str(),
                "decode-ahead stopped on an error"
            );
            state.error = Some(error);
        }
    }
    let stopping = state.stopping;
    drop(state);
    shared.filled.notify_all();
    !stopping
}

/// The worker body: decode ahead, honour seeks, stop when the handle goes.
fn run_worker(shared: &Shared, mut decoder: Decoder) {
    loop {
        match next_step(shared) {
            Step::Stop => return,
            Step::Decode(generation) => {
                let started = Instant::now();
                let outcome = decoder.next_frame();
                if !record(shared, generation, outcome, started.elapsed()) {
                    return;
                }
            }
            Step::Seek(target, generation) => {
                let started = Instant::now();
                let outcome = decoder.seek_to(target);
                if let Ok(mut state) = shared.lock() {
                    state.stats.seeks += 1;
                }
                if !record(shared, generation, outcome, started.elapsed()) {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_CAPACITY, DecodeAheadOptions, DecodeAheadStats, Shared};
    use std::time::Duration;

    #[test]
    fn default_options_buffer_the_documented_depth() {
        let options = DecodeAheadOptions::default();
        assert_eq!(options.capacity, DEFAULT_CAPACITY);
        assert!(
            options.capacity >= 1,
            "the ring must be able to hold a frame"
        );
    }

    #[test]
    fn the_ring_is_allocated_once_to_its_capacity() {
        let shared = Shared::new(5);
        let state = shared.lock().expect("a fresh lock is never poisoned");
        assert_eq!(state.capacity, 5);
        assert!(
            state.frames.capacity() >= 5,
            "the reservation is made at construction, not per frame"
        );
        assert_eq!(state.stats.capacity, 5, "capacity is reported in the stats");
    }

    #[test]
    fn mean_decode_time_needs_a_frame() {
        let mut stats = DecodeAheadStats {
            capacity: 4,
            ..DecodeAheadStats::default()
        };
        assert_eq!(stats.decode_time_mean(), None);
        stats.frames_decoded = 4;
        stats.decode_time_total = Duration::from_millis(40);
        assert_eq!(stats.decode_time_mean(), Some(Duration::from_millis(10)));
    }
}
