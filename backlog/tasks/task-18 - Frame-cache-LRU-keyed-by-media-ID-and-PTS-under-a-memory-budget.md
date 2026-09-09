---
id: TASK-18
title: 'Frame cache: LRU keyed by media ID and PTS under a memory budget'
status: Done
assignee:
  - '@opus-task-18'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 13:46'
labels:
  - media
milestone: m-1
dependencies:
  - TASK-17
references:
  - docs/PLAN.md
priority: high
ordinal: 39000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Scrub responsiveness depends on hitting cached frames (§5.2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Cache stores decoded frames with a configurable byte budget and LRU eviction
- [x] #2 Hit and miss counters are exposed; a test proves eviction respects the budget
- [x] #3 Frames are reference-counted so the compositor can hold one across eviction
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add sub_media::frame_cache with a FrameKey of (MediaId, RationalTime) — RationalTime hashes rate-normalised so a PTS keys the same however it is expressed.
2. FrameCache<T: Cacheable = VideoFrame>: HashMap of entries plus a BTreeMap recency index keyed by a monotonic tick, byte budget enforced on every insert and on set_budget by evicting least-recently-used entries.
3. Store values as Arc<T> and hand Arc clones out of insert/get/peek, so an evicted frame the compositor still holds stays alive and valid.
4. Expose FrameCacheStats: hits, misses, inserts, replacements, evictions, rejections, bytes_used, peak_bytes, len, budget.
5. Add VideoFrame::byte_size (GStreamer VideoInfo size) and impl Cacheable for it.
6. Tests: hit/miss counting, LRU order, budget never exceeded across a churn of inserts, oversize rejection, Arc outliving eviction, per-media invalidation, budget shrink, Send+Sync assertion.
7. Verify with cargo fmt --check, clippy --workspace --all-targets -D warnings, cargo test -p sub-media under the GStreamer env script.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-media/src/frame_cache.rs: FrameCache<T: Cacheable = VideoFrame>, keyed by FrameKey { MediaId, RationalTime }. RationalTime already hashes and compares by wall-clock length, so one PTS keys one entry however its rate is written (test a_pts_keys_the_same_entry_however_its_rate_is_written).

Design decisions:
- Recency is a monotonic u64 tick per entry, held in a BTreeMap<tick, key> beside the HashMap of entries, so eviction is O(log n) and no external LRU crate is added. get() and insert() restamp; peek() and contains() deliberately do not, so a diagnostic read cannot reorder what a scrub is about to need. A test-only is_consistent() invariant is asserted through 500 steps of insert/get/remove churn.
- Values live behind Arc and every lookup returns a clone, which is how AC 3 is met: a frame the compositor holds stays alive and mapped after eviction. Proven by an_evicted_frame_stays_alive_while_the_compositor_holds_it (strong_count 2 -> 1, contents still readable, cache stops charging its bytes).
- A value larger than the whole budget is refused rather than stored, and the Arc still comes back so the caller can use the frame it just decoded; counted in stats.rejections.
- The cache is not internally synchronised: a lookup mutates recency, so the Mutex belongs to the caller that knows its threading. Documented as unsafe to touch from an audio callback.
- VideoFrame::byte_size() (GStreamer VideoInfo size, every plane and its padding) is the cost charged to the budget; Cacheable is implemented for it, and the generic parameter lets the tests exercise the policy without a decoder or a GPU.
- FrameCacheStats carries hits, misses, inserts, replacements, evictions, rejections, invalidations, bytes_used, bytes_budget, peak_bytes and len. hit_rate() is a monitoring f64 ratio only; no float touches timeline arithmetic.

Not done, and deliberately out of scope: nothing calls the cache yet. Wiring it behind the decode-ahead worker and the compositor belongs to TASK-22/TASK-23.

Environment note: the GStreamer sysroot this worktree was told to source had been removed from the scratchpad, so it was rebuilt from the Ubuntu -dev packages (libgstreamer1.0-dev, libgstreamer-plugins-base1.0-dev, libglib2.0-dev, liborc-0.4-dev, libdw-dev, libunwind-dev) unpacked under the scratchpad with the .so symlinks repointed at the system runtime libraries. No system packages were installed and nothing in the repo depends on it.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added sub_media::frame_cache: an LRU frame cache keyed by (MediaId, PTS) that holds itself to a byte budget, evicting least-recently-used entries on insert and on a budget shrink, and exposing hit/miss/eviction/rejection counters plus bytes_used, peak_bytes and len. Frames are stored behind Arc and every lookup hands out a clone, so the compositor can keep a frame alive and mapped after the cache has evicted it. Verified with 14 new unit tests plus a doctest (budget never exceeded across 64 inserts into a 4-slot budget, LRU order including that peek does not reorder, Arc strong_count 2 -> 1 across eviction with the contents still readable, oversize rejection, zero budget, replacement recharging, budget shrink, per-media invalidation, and an index-consistency invariant asserted through 500 steps of churn); cargo test -p sub-media is green at 69 lib + 41 integration + 8 doc tests, and cargo fmt --all --check and cargo clippy --workspace --all-targets -D warnings both pass.
<!-- SECTION:FINAL_SUMMARY:END -->
