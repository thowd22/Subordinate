---
id: TASK-69
title: Proxy generation pipeline to intra-only codec at reduced resolution
status: Done
assignee:
  - '@opus-task-69'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 16:54'
labels:
  - media
milestone: m-5
dependencies:
  - TASK-16
  - TASK-25
  - TASK-57
references:
  - docs/PLAN.md
priority: high
ordinal: 90000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Long-GOP sources scrub badly; proxies fix it (§5.2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Job transcodes a media item to DNxHR LB or MJPEG at half or quarter resolution into the sidecar dir, using hardware decode when available
- [x] #2 VFR sources use the PTS index so proxy frames map one-to-one to originals
- [x] #3 Auto-generation triggers for sources above a configurable resolution threshold
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New sub-media module proxy.rs: ProxyCodec (DNxHR LB via avenc_dnxhd, MJPEG via jpegenc), ProxyScale (Half/Quarter), ProxyOptions with validate/fingerprint, Proxy result type with a JSON manifest in the sidecar dir, keyed by ContentHash + options fingerprint (same shape as thumbnail.rs).
2. Transcode pipeline: uridecodebin (hardware decoders promoted, reusing decode.rs prefer_hardware_decoders) -> videoconvert -> videoscale -> capsfilter at the proxy size -> intra-only encoder -> qtmux -> filesink, written to a temporary file and renamed atomically. Bus watched for errors and EOS; cancellable, progress reported in frames.
3. VFR: the source PTS index (PtsIndex::load_or_build) gives the frame count and drives progress; after the transcode the proxy's own index is built and compared frame-for-frame with the source's, so a proxy is only accepted when its frames map one-to-one onto the originals.
4. ProxyPolicy: configurable resolution threshold plus long-GOP codec test; should_generate/options_for pick whether and how to auto-generate, choosing DNxHR LB when the encoder accepts the proxy size and MJPEG otherwise.
5. spawn_proxy_job on JobService (kind 'proxy'), mirroring spawn_thumbnail_job. New stable error code media.proxy_failed.
6. Unit tests for sizes, policy, options and manifest round-trip; fixture-gated integration tests in tests/proxy_fixtures.rs covering a real transcode, resume, cancellation and the VFR one-to-one mapping.
7. Verify with cargo fmt, clippy pedantic and cargo test under the local GStreamer env.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
New crates/sub-media/src/proxy.rs. Proxy::generate_with transcodes a source into an intra-only movie in the sidecar directory: uridecodebin (hardware decoders promoted through the same decode.rs prefer_hardware_decoders() the preview decoder uses) -> videoconvert -> videoscale -> capsfilter at the proxy size -> encoder -> qtmux -> filesink, written to a .part file and renamed once verified, with a JSON manifest keyed by ContentHash + options fingerprint so a finished proxy is reused rather than made again. spawn_proxy_job puts it on the JobService as kind 'proxy' with progress in frames and prompt cancellation. New stable error code media.proxy_failed.

Codec selection: ProxyCodec::is_available_at asks the registry whether this installation can both encode and mux the codec at the target size, and ProxyPolicy prefers DNxHR LB, falling back to MJPEG. On this machine (and on any gst-libav build with the DNxHD-only caps and no muxer accepting video/x-dnxhd) DNxHR is unavailable, so the fixture tests exercise the MJPEG path; the DNxHR path is chosen automatically wherever the registry supports it.

VFR (AC 2): the source PtsIndex is built or read from the sidecar cache before the transcode and gives the exact frame total for progress; the pipeline has no videorate, so buffer timestamps pass through untouched; afterwards the proxy's own index is built and compared with the source's frame for frame, and a proxy whose frames do not line up is deleted rather than written. Timestamps are compared as offsets from each file's own first frame, because a container may start its timeline anywhere (the bars fixture's first PTS is 80 ms) - the spacing, which is what VFR puts at risk, is what is checked, within MAX_PROXY_PTS_DRIFT_NS (1 ms).

Threshold (AC 3): ProxyPolicy carries enabled, min_width, min_height, include_intra_sources, scale and quality; should_generate/options_for decide whether a probed source is proxied and with which codec. Defaults trigger above 1920x1080 and skip codecs known to be intra-only. Like the thumbnail job of TASK-25, the policy is not yet called from an import path - no engine code spawns either job today; wiring it to import belongs with TASK-70's proxy state tracking.

One fix outside the new module: index.rs's pad_carries_video only accepted caps named video/*, so an MJPEG track (image/jpeg) indexed as empty and every proxy failed verification. It now accepts image/* too, which is also what an MJPEG *source* needs.

Verification (Linux, local GStreamer 1.24 prefix, no GPU): cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-media all green - 90 unit tests (10 new proxy ones) plus 7 fixture tests in tests/proxy_fixtures.rs covering a real 1080p->480x270 transcode, the frame-for-frame count and all-keyframe check, the VFR fixture's one-to-one mapping, reuse without rewriting, a cancelled run leaving nothing behind, job progress counted in source frames, and the policy triggering on the 4K fixture but not the HD one. Hardware decode could not be exercised here (no GPU in this environment); the code path is the shared, tested one from decode.rs.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the proxy generation pipeline in sub-media: an intra-only transcode (DNxHR LB where the installation can write it, MJPEG otherwise) at half or quarter resolution into the project's sidecar directory, run as a cancellable JobService job with hardware decoders preferred, reusable through a hash-keyed manifest. VFR sources are handled by building the source's PTS index up front and rejecting any proxy whose frames do not line up with it one for one, and ProxyPolicy decides auto-generation from a configurable resolution threshold. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p sub-media, including seven fixture tests that transcode the real 1080p and VFR fixtures and check the frame mapping, reuse, cancellation, progress and threshold behaviour.
<!-- SECTION:FINAL_SUMMARY:END -->
