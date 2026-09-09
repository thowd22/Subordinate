---
id: TASK-36
title: Thumbnail strips on timeline clips with texture caching
status: Done
assignee:
  - '@opus-task-36'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 16:59'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-28
  - TASK-25
references:
  - docs/PLAN.md
priority: medium
ordinal: 57000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Visual identification of clips on the timeline (§5.7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Video clips show thumbnail frames along their length, sampled from the thumbnail job output
- [x] #2 Strips are cached as egui textures per zoom bucket and evicted under a budget
- [x] #3 No visible stall when scrolling a 200-clip sequence
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-media: decode side of a strip — ThumbnailImage (packed RGBA), ThumbnailStrip::load_image(index)/ThumbnailFrame::load_rgba via zune-jpeg, plus a public ThumbnailStrip::from_frames constructor so a caller (and a test) can assemble a strip without re-generating it.
2. sub-ui: new module thumbnails.rs with ZoomBucket (quantised tile-size ladder), StripKey (media id + bucket), ThumbnailCache: strips inserted from thumbnail job output, egui textures decoded and downscaled per bucket, LRU eviction against a byte budget, bounded texture uploads per painted frame, and a wanted-set of media the timeline drew but has no strip for.
3. sub-ui/timeline_panel.rs: TimelinePanel owns a ThumbnailCache; paint_clip draws a tile strip inside video clip rectangles, tile source times computed in RationalTime (never floats), falling back to the flat clip colour for tiles whose texture is not resident yet.
4. Tests: tile layout and sampling maths, bucket ladder, eviction under budget, per-frame upload bound, JPEG round-trip through a synthetic strip, and a headless 200-clip paint loop asserting bounded per-frame work.
5. Verify: cargo fmt --check, clippy pedantic -D warnings, cargo test -p sub-media -p sub-ui under the GStreamer env script.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implementation

sub-media: added the decode side of a strip — ThumbnailImage (packed RGBA) with ThumbnailFrame::load_rgba / ThumbnailStrip::load_image over zune-jpeg (already in the lock tree via tiff), and ThumbnailStrip::from_frames so a caller can assemble a strip from pictures already on disk without regenerating it. Both failure paths return media.thumbnail_failed; a bad index is core.invalid_argument.

sub-ui/thumbnails.rs (new): ThumbnailCache keyed by (MediaId, ZoomBucket). ZoomBucket is a four-rung ladder (32/64/128/256 px on the longest side) that the panel's wanted tile size rounds up into, so zooming or resizing re-uses textures instead of rebuilding them. Pictures are box-downscaled into the bucket and uploaded with Context::load_texture; the cache tracks its bytes and evicts whole (media, bucket) entries least-recently-drawn first under ThumbnailCacheConfig::budget_bytes (16 MiB by default), never evicting an entry drawn in the current frame. At most uploads_per_frame (4) textures are decoded and uploaded per painted frame — a decode failure costs an upload too, so a deleted sidecar cannot spin. The panel never spawns a job: media it drew without a strip goes into a wanted set that the application drains through take_missing() and feeds back through insert_strip().

sub-ui/timeline_panel.rs: TimelinePanel owns the cache (thumbnails()/thumbnails_mut()); ui() begins a cache frame and paints clip body -> strip tiles -> outline/trim/name. strip_tiles() divides a clip rectangle into tiles that keep the thumbnail's aspect, capped at MAX_TILES_PER_CLIP (64) after which tiles widen rather than multiply; tile_time() picks the source time each tile shows with exact integer RationalTime arithmetic (centred in its slice, matching sub_media::strip_times), never floats. A tile whose texture is not resident is left as body colour. A name over a picture gets a translucent plate.

Nothing here mutates the project; no command was needed since the strip is a view concern.

Verification (this machine, GStreamer from the local prefix):
- cargo fmt --all --check: clean.
- cargo clippy --workspace --all-targets -- -D warnings: clean.
- cargo test -p sub-media --lib: 82 passed (includes the new JPEG round-trip, corrupt/missing picture and from_frames tests).
- cargo test -p sub-ui: 106 lib tests + integration and doc tests passed, including the nine new cache tests and the three new panel tests.
- AC3 evidence: timeline_panel::tests::scrolling_a_two_hundred_clip_sequence_is_bounded_work_per_frame paints 120 frames of a 200-clip sequence headlessly, asserting no frame uploads more than DEFAULT_UPLOADS_PER_FRAME textures, that the byte budget always holds and that strips are re-used across frames; the whole 120-frame loop runs in about 0.01 s (~0.08 ms/frame). Smoothness on a real display still needs hardware; what is proven here is that per-frame work is bounded and independent of the clip count.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Timeline clips now paint thumbnail strips from thumbnail-job output, drawn through a new egui texture cache in sub-ui (crates/sub-ui/src/thumbnails.rs) keyed by media and zoom bucket, with an LRU byte budget and a per-frame upload ceiling; sub-media gained the decode half of a strip (ThumbnailImage, load_rgba/load_image, from_frames) over zune-jpeg. Tile counts and tile source times are exact integer RationalTime arithmetic and the panel still mutates nothing. Verified with cargo fmt --check, clippy pedantic -D warnings across the workspace and cargo test for sub-media (82) and sub-ui (106 plus integration and doc tests), including a headless 120-frame paint of a 200-clip sequence that proves per-frame work stays bounded.
<!-- SECTION:FINAL_SUMMARY:END -->
