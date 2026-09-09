//! Frame cache: LRU keyed by media ID and PTS under a memory budget
//! (docs/PLAN.md §5.2).
//!
//! Scrubbing is the workload this exists for. Dragging the playhead back and
//! forth over a few seconds asks for the same pictures again and again, and a
//! long-GOP source answers a miss by seeking to a keyframe and decoding
//! forward — tens of milliseconds for one frame. Keeping the decoded pictures
//! is what makes the second pass over a region feel immediate, and a byte
//! budget is what keeps that from eating the machine.
//!
//! ```
//! use std::sync::Arc;
//!
//! use sub_media::{FrameCache, FrameKey};
//! use sub_model::MediaId;
//! use sub_time::{Rational, RationalTime};
//!
//! # #[derive(Debug)]
//! # struct Picture(Vec<u8>);
//! # impl sub_media::Cacheable for Picture {
//! #     fn byte_size(&self) -> usize {
//! #         self.0.len()
//! #     }
//! # }
//! let media = MediaId::new();
//! let rate = Rational::new(25, 1).unwrap();
//! let key = FrameKey::new(media, RationalTime::from_frames(7, rate));
//!
//! let mut cache: FrameCache<Picture> = FrameCache::new(4 * 1024 * 1024);
//! assert!(cache.get(key).is_none());
//! let frame: Arc<Picture> = cache.insert(key, Picture(vec![0; 1024]));
//! assert!(Arc::ptr_eq(&cache.get(key).unwrap(), &frame));
//! assert_eq!(cache.stats().hits, 1);
//! assert_eq!(cache.stats().misses, 1);
//! ```
//!
//! Three properties are the point of this type:
//!
//! * **Budgeted.** [`FrameCache::bytes_used`] never exceeds
//!   [`FrameCache::budget`] once an insert returns. Insertion evicts as many
//!   least-recently-used entries as it takes to make room, and a single value
//!   larger than the whole budget is refused rather than stored.
//! * **Least-recently-used.** Every [`FrameCache::get`] and every insert stamps
//!   the entry with a monotonic tick; eviction always takes the lowest stamp.
//!   [`FrameCache::peek`] deliberately does not, so a diagnostic read cannot
//!   reorder what a scrub is about to need.
//! * **Reference-counted.** Values live behind an [`Arc`] and every lookup hands
//!   out a clone of it. A frame the compositor is holding stays alive and its
//!   pixels stay mapped after the cache has evicted it; the memory is released
//!   when the last holder drops it, not when the cache does.
//!
//! The cache is deliberately not internally synchronised: a lookup mutates
//! recency, so a shared cache belongs in a `Mutex` owned by the caller that
//! knows its threading. Nothing here is safe to touch from an audio callback.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use sub_model::MediaId;
use sub_time::RationalTime;

use crate::decode::VideoFrame;

/// Byte budget a [`FrameCache`] uses when a caller does not choose one.
///
/// 512 MiB is about 200 frames of 1080p NV12, or 50 of 4K: enough that a few
/// seconds either side of the playhead survive a scrub on the 8 GB machines the
/// MVP targets, and small enough to leave the decoders and the GPU uploader
/// their own headroom.
pub const DEFAULT_BUDGET_BYTES: usize = 512 * 1024 * 1024;

/// A value a [`FrameCache`] can hold: something that knows what it costs.
///
/// The cost is the resident size charged against the budget, so it must be the
/// bytes the value actually keeps alive rather than `size_of` its handle.
pub trait Cacheable {
    /// Resident size of this value in bytes.
    fn byte_size(&self) -> usize;
}

impl Cacheable for VideoFrame {
    fn byte_size(&self) -> usize {
        VideoFrame::byte_size(self)
    }
}

/// What a cached picture is: one media item at one presentation timestamp.
///
/// [`RationalTime`] compares and hashes by wall-clock length, so a PTS keys the
/// same entry however its rate is expressed — 1/25 s written as one frame at
/// 25 fps and as 40 000 000 ns are the same key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameKey {
    /// The media item the picture was decoded from.
    pub media: MediaId,
    /// The picture's presentation timestamp within that media item.
    pub pts: RationalTime,
}

impl FrameKey {
    /// Creates a key for one picture.
    pub const fn new(media: MediaId, pts: RationalTime) -> Self {
        Self { media, pts }
    }
}

/// A snapshot of what one [`FrameCache`] has been doing.
///
/// Counters are cumulative since construction or the last
/// [`FrameCache::reset_stats`]; the sizes are instantaneous.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameCacheStats {
    /// Lookups through [`FrameCache::get`] that found a live entry.
    pub hits: u64,
    /// Lookups through [`FrameCache::get`] that found nothing.
    pub misses: u64,
    /// Values stored, replacements included.
    pub inserts: u64,
    /// Inserts that overwrote an entry already under that key.
    pub replacements: u64,
    /// Entries dropped to stay under the budget.
    pub evictions: u64,
    /// Inserts refused because the value alone exceeds the whole budget.
    pub rejections: u64,
    /// Entries dropped by [`FrameCache::invalidate_media`],
    /// [`FrameCache::remove`] and [`FrameCache::clear`].
    pub invalidations: u64,
    /// Bytes currently charged to the cache.
    pub bytes_used: usize,
    /// Budget the cache is holding itself to.
    pub bytes_budget: usize,
    /// Largest `bytes_used` reached since the last reset.
    pub peak_bytes: usize,
    /// Entries currently held.
    pub len: usize,
}

impl FrameCacheStats {
    /// Share of lookups that hit, or `None` before the first lookup.
    ///
    /// A monitoring ratio, never timeline arithmetic: nothing derived from it
    /// is allowed back into frame timing.
    #[allow(
        clippy::cast_precision_loss,
        reason = "lookup tallies, exact in f64 well past any session"
    )]
    pub fn hit_rate(&self) -> Option<f64> {
        let total = self.hits.checked_add(self.misses).filter(|n| *n > 0)?;
        Some(self.hits as f64 / total as f64)
    }
}

/// One stored value with its cost and its place in the recency order.
#[derive(Debug)]
struct Entry<T> {
    value: Arc<T>,
    bytes: usize,
    tick: u64,
}

/// An LRU cache of decoded frames keyed by media ID and PTS, held under a byte
/// budget.
///
/// See the [module documentation](self) for the guarantees. `T` defaults to
/// [`VideoFrame`]; the parameter exists so proxies, thumbnails and tests can
/// reuse the same policy.
#[derive(Debug)]
pub struct FrameCache<T: Cacheable = VideoFrame> {
    entries: HashMap<FrameKey, Entry<T>>,
    /// Recency order: tick -> key, lowest tick least recently used. Kept in
    /// step with `entries`, one node per entry.
    order: BTreeMap<u64, FrameKey>,
    budget: usize,
    used: usize,
    clock: u64,
    stats: FrameCacheStats,
}

impl<T: Cacheable> FrameCache<T> {
    /// Creates an empty cache holding itself to `budget_bytes`.
    ///
    /// A budget of zero is legal and stores nothing.
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: BTreeMap::new(),
            budget: budget_bytes,
            used: 0,
            clock: 0,
            stats: FrameCacheStats {
                bytes_budget: budget_bytes,
                ..FrameCacheStats::default()
            },
        }
    }

    /// The byte budget.
    pub fn budget(&self) -> usize {
        self.budget
    }

    /// Bytes currently charged to the cache. Never above [`Self::budget`].
    pub fn bytes_used(&self) -> usize {
        self.used
    }

    /// Entries currently held.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing is held.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Counters and sizes as of now.
    pub fn stats(&self) -> FrameCacheStats {
        FrameCacheStats {
            bytes_used: self.used,
            bytes_budget: self.budget,
            len: self.entries.len(),
            ..self.stats
        }
    }

    /// Zeroes the cumulative counters, leaving the contents alone.
    pub fn reset_stats(&mut self) {
        self.stats = FrameCacheStats {
            bytes_used: self.used,
            bytes_budget: self.budget,
            peak_bytes: self.used,
            len: self.entries.len(),
            ..FrameCacheStats::default()
        };
    }

    /// Changes the budget, evicting least-recently-used entries at once if the
    /// new budget is smaller than what is held.
    pub fn set_budget(&mut self, budget_bytes: usize) {
        self.budget = budget_bytes;
        self.stats.bytes_budget = budget_bytes;
        self.evict_to_fit(0);
    }

    /// True when `key` is held, without disturbing the recency order.
    pub fn contains(&self, key: FrameKey) -> bool {
        self.entries.contains_key(&key)
    }

    /// Looks `key` up, counts the hit or miss, and marks the entry as the most
    /// recently used.
    ///
    /// The returned handle keeps the frame alive even if a later insert evicts
    /// it from the cache.
    pub fn get(&mut self, key: FrameKey) -> Option<Arc<T>> {
        let tick = self.next_tick();
        let Some(entry) = self.entries.get_mut(&key) else {
            self.stats.misses += 1;
            return None;
        };
        self.order.remove(&entry.tick);
        entry.tick = tick;
        self.order.insert(tick, key);
        self.stats.hits += 1;
        Some(Arc::clone(&entry.value))
    }

    /// Looks `key` up without counting a hit or a miss and without touching the
    /// recency order. For diagnostics and assertions.
    pub fn peek(&self, key: FrameKey) -> Option<Arc<T>> {
        self.entries.get(&key).map(|entry| Arc::clone(&entry.value))
    }

    /// Stores `value` under `key` and returns the shared handle to it.
    ///
    /// Evicts least-recently-used entries as needed to stay inside the budget.
    /// A value bigger than the whole budget is not stored at all — the handle
    /// still comes back, so a caller that decoded an oversized frame can use it
    /// — and the refusal is counted in [`FrameCacheStats::rejections`].
    pub fn insert(&mut self, key: FrameKey, value: T) -> Arc<T> {
        self.insert_shared(key, Arc::new(value))
    }

    /// Stores an already-shared value, for a frame the caller is holding too.
    ///
    /// Behaves exactly like [`Self::insert`] otherwise.
    pub fn insert_shared(&mut self, key: FrameKey, value: Arc<T>) -> Arc<T> {
        let bytes = value.byte_size();
        let handle = Arc::clone(&value);

        // Replacing an entry frees its bytes whether or not the new value fits.
        if let Some(previous) = self.entries.remove(&key) {
            self.order.remove(&previous.tick);
            self.used -= previous.bytes;
            self.stats.replacements += 1;
        }

        if bytes > self.budget {
            self.stats.rejections += 1;
            return handle;
        }

        self.evict_to_fit(bytes);
        let tick = self.next_tick();
        self.entries.insert(key, Entry { value, bytes, tick });
        self.order.insert(tick, key);
        self.used += bytes;
        self.stats.inserts += 1;
        self.stats.peak_bytes = self.stats.peak_bytes.max(self.used);
        handle
    }

    /// Drops `key` and returns what was held, if anything.
    pub fn remove(&mut self, key: FrameKey) -> Option<Arc<T>> {
        let entry = self.entries.remove(&key)?;
        self.order.remove(&entry.tick);
        self.used -= entry.bytes;
        self.stats.invalidations += 1;
        Some(entry.value)
    }

    /// Drops every frame of one media item, returning how many went.
    ///
    /// This is what a relink, a proxy swap or a media item leaving the project
    /// calls: cached pictures for that media are no longer what the file says.
    pub fn invalidate_media(&mut self, media: MediaId) -> usize {
        let doomed: Vec<FrameKey> = self
            .entries
            .keys()
            .filter(|key| key.media == media)
            .copied()
            .collect();
        for key in &doomed {
            if let Some(entry) = self.entries.remove(key) {
                self.order.remove(&entry.tick);
                self.used -= entry.bytes;
            }
        }
        let dropped = doomed.len();
        self.stats.invalidations += dropped as u64;
        dropped
    }

    /// Drops everything, returning how many entries went.
    pub fn clear(&mut self) -> usize {
        let dropped = self.entries.len();
        self.entries.clear();
        self.order.clear();
        self.used = 0;
        self.stats.invalidations += dropped as u64;
        dropped
    }

    /// The next recency stamp. Monotonic for the life of the cache; `u64` at
    /// one tick per lookup outlives any session by a wide margin.
    fn next_tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Evicts least-recently-used entries until `incoming` more bytes fit.
    ///
    /// Callers guarantee `incoming <= budget`, so this always terminates with
    /// room; with `incoming == 0` it simply trims to the budget.
    fn evict_to_fit(&mut self, incoming: usize) {
        while self.used + incoming > self.budget {
            let Some((tick, key)) = self.order.iter().next().map(|(t, k)| (*t, *k)) else {
                // Nothing left to evict: `used` is already zero here.
                break;
            };
            self.order.remove(&tick);
            if let Some(entry) = self.entries.remove(&key) {
                self.used -= entry.bytes;
            }
            self.stats.evictions += 1;
        }
    }
}

impl<T: Cacheable> Default for FrameCache<T> {
    /// A cache with the [`DEFAULT_BUDGET_BYTES`] budget.
    fn default() -> Self {
        Self::new(DEFAULT_BUDGET_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sub_model::MediaId;
    use sub_time::{Rational, RationalTime};

    use super::{Cacheable, DEFAULT_BUDGET_BYTES, FrameCache, FrameKey};

    /// A stand-in picture that costs whatever it is told to cost, so the budget
    /// policy is tested without a decoder or a GPU.
    #[derive(Debug, PartialEq, Eq)]
    struct Picture {
        bytes: usize,
        tag: u32,
    }

    impl Picture {
        fn new(bytes: usize, tag: u32) -> Self {
            Self { bytes, tag }
        }
    }

    impl Cacheable for Picture {
        fn byte_size(&self) -> usize {
            self.bytes
        }
    }

    impl<T: Cacheable> FrameCache<T> {
        /// Test-only invariant check: the recency index mirrors the entries and
        /// `used` is the sum of the entry costs.
        fn is_consistent(&self) -> bool {
            self.order.len() == self.entries.len()
                && self.used
                    == self
                        .entries
                        .values()
                        .map(|entry| entry.bytes)
                        .sum::<usize>()
                && self
                    .order
                    .iter()
                    .all(|(tick, key)| self.entries.get(key).is_some_and(|e| e.tick == *tick))
        }
    }

    fn rate() -> Rational {
        Rational::new(25, 1).expect("25 fps is a valid rate")
    }

    fn key(media: MediaId, frame: i64) -> FrameKey {
        FrameKey::new(media, RationalTime::from_frames(frame, rate()))
    }

    #[test]
    fn hits_and_misses_are_counted() {
        let media = MediaId::new();
        let mut cache: FrameCache<Picture> = FrameCache::new(1024);

        assert!(cache.get(key(media, 0)).is_none());
        cache.insert(key(media, 0), Picture::new(16, 0));
        assert_eq!(cache.get(key(media, 0)).unwrap().tag, 0);
        assert_eq!(cache.get(key(media, 0)).unwrap().tag, 0);
        assert!(cache.get(key(media, 1)).is_none());

        let stats = cache.stats();
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 2);
        assert_eq!(stats.inserts, 1);
        assert_eq!(stats.len, 1);
        assert_eq!(stats.bytes_used, 16);
        assert!((stats.hit_rate().unwrap() - 0.5).abs() < f64::EPSILON);

        cache.reset_stats();
        assert_eq!(cache.stats().hits, 0);
        assert_eq!(cache.stats().len, 1, "reset must not drop entries");
    }

    #[test]
    fn a_pts_keys_the_same_entry_however_its_rate_is_written() {
        let media = MediaId::new();
        let mut cache: FrameCache<Picture> = FrameCache::new(1024);
        let nanos = Rational::new(1_000_000_000, 1).expect("nanosecond rate is valid");

        cache.insert(key(media, 1), Picture::new(8, 7));
        let same_instant = FrameKey::new(media, RationalTime::from_frames(40_000_000, nanos));
        assert_eq!(cache.get(same_instant).unwrap().tag, 7);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn eviction_keeps_the_cache_inside_its_budget() {
        let media = MediaId::new();
        let mut cache: FrameCache<Picture> = FrameCache::new(400);

        for frame in 0..64_i64 {
            let tag = u32::try_from(frame).expect("small loop counter");
            cache.insert(key(media, frame), Picture::new(100, tag));
            assert!(
                cache.bytes_used() <= cache.budget(),
                "budget exceeded after frame {frame}: {} > {}",
                cache.bytes_used(),
                cache.budget()
            );
        }

        assert_eq!(cache.len(), 4);
        assert_eq!(cache.bytes_used(), 400);
        let stats = cache.stats();
        assert_eq!(stats.inserts, 64);
        assert_eq!(stats.evictions, 60);
        assert_eq!(stats.peak_bytes, 400);
        for frame in 0..60 {
            assert!(
                !cache.contains(key(media, frame)),
                "frame {frame} should be gone"
            );
        }
        for frame in 60..64 {
            assert!(
                cache.contains(key(media, frame)),
                "frame {frame} should be held"
            );
        }
    }

    #[test]
    fn eviction_takes_the_least_recently_used_entry() {
        let media = MediaId::new();
        let mut cache: FrameCache<Picture> = FrameCache::new(300);
        for frame in 0..3_i64 {
            let tag = u32::try_from(frame).expect("small loop counter");
            cache.insert(key(media, frame), Picture::new(100, tag));
        }

        // Touch the oldest so the middle one becomes least recently used.
        assert!(cache.get(key(media, 0)).is_some());
        // A peek must not reorder anything.
        assert!(cache.peek(key(media, 1)).is_some());

        cache.insert(key(media, 3), Picture::new(100, 3));

        assert!(cache.contains(key(media, 0)), "touched entry must survive");
        assert!(
            !cache.contains(key(media, 1)),
            "least recently used must go"
        );
        assert!(cache.contains(key(media, 2)));
        assert!(cache.contains(key(media, 3)));
    }

    #[test]
    fn an_evicted_frame_stays_alive_while_the_compositor_holds_it() {
        let media = MediaId::new();
        let mut cache: FrameCache<Picture> = FrameCache::new(200);

        let held = cache.insert(key(media, 0), Picture::new(100, 42));
        assert_eq!(Arc::strong_count(&held), 2, "cache and caller both hold it");

        cache.insert(key(media, 1), Picture::new(100, 1));
        cache.insert(key(media, 2), Picture::new(100, 2));

        assert!(
            !cache.contains(key(media, 0)),
            "frame 0 must have been evicted"
        );
        assert_eq!(held.tag, 42, "the evicted frame is still readable");
        assert_eq!(Arc::strong_count(&held), 1, "only the caller holds it now");
        assert_eq!(cache.bytes_used(), 200, "the cache stopped charging for it");
    }

    #[test]
    fn a_value_larger_than_the_budget_is_refused_but_still_returned() {
        let media = MediaId::new();
        let mut cache: FrameCache<Picture> = FrameCache::new(100);

        cache.insert(key(media, 0), Picture::new(50, 0));
        let huge = cache.insert(key(media, 1), Picture::new(101, 1));

        assert_eq!(huge.tag, 1);
        assert!(!cache.contains(key(media, 1)));
        assert!(
            cache.contains(key(media, 0)),
            "an oversized insert evicts nothing"
        );
        assert_eq!(cache.stats().rejections, 1);
        assert_eq!(cache.bytes_used(), 50);
    }

    #[test]
    fn a_zero_budget_stores_nothing() {
        let media = MediaId::new();
        let mut cache: FrameCache<Picture> = FrameCache::new(0);
        cache.insert(key(media, 0), Picture::new(1, 0));
        assert!(cache.is_empty());
        assert_eq!(cache.bytes_used(), 0);
        assert_eq!(cache.stats().rejections, 1);
    }

    #[test]
    fn replacing_an_entry_recharges_its_bytes() {
        let media = MediaId::new();
        let mut cache: FrameCache<Picture> = FrameCache::new(1000);

        cache.insert(key(media, 0), Picture::new(100, 0));
        cache.insert(key(media, 0), Picture::new(300, 1));

        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes_used(), 300);
        assert_eq!(cache.peek(key(media, 0)).unwrap().tag, 1);
        assert_eq!(cache.stats().replacements, 1);
    }

    #[test]
    fn shrinking_the_budget_evicts_at_once() {
        let media = MediaId::new();
        let mut cache: FrameCache<Picture> = FrameCache::new(500);
        for frame in 0..5_i64 {
            let tag = u32::try_from(frame).expect("small loop counter");
            cache.insert(key(media, frame), Picture::new(100, tag));
        }

        cache.set_budget(250);

        assert_eq!(cache.budget(), 250);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.bytes_used(), 200);
        assert!(cache.contains(key(media, 3)));
        assert!(cache.contains(key(media, 4)));
    }

    #[test]
    fn media_is_invalidated_independently() {
        let one = MediaId::new();
        let two = MediaId::new();
        let mut cache: FrameCache<Picture> = FrameCache::new(1000);

        for frame in 0..3 {
            cache.insert(key(one, frame), Picture::new(50, 0));
            cache.insert(key(two, frame), Picture::new(50, 1));
        }
        assert_eq!(cache.bytes_used(), 300);

        assert_eq!(cache.invalidate_media(one), 3);
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.bytes_used(), 150);
        assert!(cache.is_consistent());
        for frame in 0..3 {
            assert!(!cache.contains(key(one, frame)));
            assert!(cache.contains(key(two, frame)));
        }

        assert_eq!(cache.clear(), 3);
        assert!(cache.is_empty());
        assert_eq!(cache.bytes_used(), 0);
        assert_eq!(cache.stats().invalidations, 6);
    }

    #[test]
    fn removing_one_entry_frees_its_bytes() {
        let media = MediaId::new();
        let mut cache: FrameCache<Picture> = FrameCache::new(1000);
        cache.insert(key(media, 0), Picture::new(100, 9));

        assert_eq!(cache.remove(key(media, 0)).unwrap().tag, 9);
        assert!(cache.remove(key(media, 0)).is_none());
        assert_eq!(cache.bytes_used(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn recency_index_stays_in_step_through_churn() {
        let media = MediaId::new();
        let mut cache: FrameCache<Picture> = FrameCache::new(512);

        for step in 0..500_i64 {
            let tag = u32::try_from(step).expect("small loop counter");
            cache.insert(key(media, step % 13), Picture::new(64, tag));
            let _ = cache.get(key(media, (step * 7) % 13));
            if step % 37 == 0 {
                let _ = cache.remove(key(media, (step * 3) % 13));
            }
            assert!(cache.is_consistent(), "desync at step {step}");
            assert!(cache.bytes_used() <= cache.budget());
        }
    }

    #[test]
    fn default_budget_is_the_documented_one() {
        let cache: FrameCache<Picture> = FrameCache::default();
        assert_eq!(cache.budget(), DEFAULT_BUDGET_BYTES);
        assert!(cache.is_empty());
    }

    #[test]
    fn a_cache_of_frames_can_be_shared_across_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FrameCache>();
        assert_send_sync::<Arc<crate::VideoFrame>>();
    }
}
