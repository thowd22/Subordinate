---
id: TASK-7
title: >-
  Spike: decode H.264 via gstreamer-rs, upload NV12 to wgpu, show in egui with
  pop-out viewport
status: In Progress
assignee:
  - '@opus-task-7'
created_date: '2026-09-08 20:53'
updated_date: '2026-09-09 03:46'
labels:
  - spike
  - media
  - ui
milestone: m-1
dependencies:
  - TASK-1
references:
  - docs/PLAN.md
priority: high
ordinal: 7000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
This single spike de-risks both the preview phase and the second-display pop-out phase (PLAN.md §11 step 3). Hardware decode should be preferred where available. Throwaway code is acceptable; the findings are the deliverable.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A 4K H.264 file plays in an egui window on Linux using nvdec or va decode when available
- [ ] #2 The same frame texture is shown in a second egui viewport that can be moved to another monitor
- [x] #3 Findings on frame upload cost and decode-to-display latency are written to backlog docs
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Build a self-contained spike crate 'spikes/nv12-viewport' (workspace member, not shipped) holding the whole path: GStreamer uridecodebin3 -> hardware-decoder preference -> appsink delivering NV12 -> bounded channel -> wgpu NV12 upload (Y + UV planes, write_texture) -> WGSL BT.709 limited-range YUV->RGB pass -> egui image, with the same converted texture registered once and drawn in BOTH the main window and an egui deferred pop-out viewport that the window manager can move to a second monitor.
2. Instrument it: per-frame upload cost (Y+UV write_texture + conversion pass), decode-to-display latency (appsink buffer arrival -> egui paint of that frame), plus counters for frames dropped by the bounded channel. Numbers printed as a summary table on exit.
3. Give the binary two modes so the parts are measurable independently: 'play <file>' (full pipeline, needs GStreamer plugins + a display) and 'bench' (headless: synthesised 4K NV12 frames through the identical upload+convert path on RenderContext::headless, no GStreamer plugins and no window needed).
4. Unit-test the pure parts that a headless CI can prove: NV12 plane geometry/stride maths, hardware-decoder rank ordering (nvdec/va/vtdec/d3d12 before software), bounded-channel newest-frame-wins drop policy, and RationalTime PTS conversion from GStreamer nanoseconds (never floats).
5. Run bench here on llvmpipe to obtain real upload numbers; record what this environment cannot prove (no /dev/dri, no VA/NVDEC, GStreamer runtime ships only coreelements so nothing can decode, single headless display).
6. Write the findings up with 'backlog doc create' covering upload cost, latency budget, the pop-out sharing design and the recommendation for TASK-20/TASK-67; leave any acceptance criterion this machine cannot demonstrate unchecked with a note.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Built the spike as a workspace member 'spikes/nv12-viewport' (lib + bin, publish = false) with four subcommands so each stage is measurable on its own: 'bench' (headless upload + BT.709 conversion on synthesised NV12), 'decode-bench' (real GStreamer pipeline into the real upload path, no window), 'play' (windowed, main viewport plus an egui deferred pop-out viewport sharing one texture) and 'make-clip' (synthesises a test clip, because gst-launch-1.0 is not installed here). 38 unit tests cover the pure parts: NV12 plane/stride geometry, decoder-kind classification and rank ordering, the newest-wins frame slot, integer-only duration statistics and CLI parsing.

Two real GStreamer findings that changed the implementation:
1. uridecodebin3/decodebin3 have no 'autoplug-sort' signal - connecting to it panics ('Signal autoplug-sort of type GstURIDecodeBin3 not found'). Hardware preference had to be rewritten as process-local registry rank promotion (the in-process equivalent of GST_PLUGIN_FEATURE_RANK), with 'deep-element-added' used to observe which decoder was actually instantiated.
2. decodebin3 adds its pads before caps are negotiated, so pad.current_caps() is None at pad-added and a caps-based video test never links anything - the pipeline sits in Playing and delivers no buffers. Matching on the pad name (video_0) is what makes it work.

Measured here (Mesa lavapipe CPU adapter, so the conversion figures are upper bounds, not GPU predictions):
- bench 3840x2160, 60 frames: write_texture min 584 / mean 733 / p95 842 us, ~16 GiB/s of NV12 payload; upload+conversion mean 8194 us.
- bench 1920x1080, 60 frames: write_texture min 66 / mean 143 / p95 221 us; upload+conversion mean 3193 us.
- decode-bench on a real 4K VP8/Matroska clip, 60 frames, vp8dec: upload+convert mean 9069 us; appsink hand-off to converted-on-GPU min 7843 / mean 9097 / p95 9016 us, 0 frames dropped by the newest-wins slot. Latency and upload+convert agree to ~30 us, so the thread hand-off itself costs nothing.
- Queue::write_texture imposes no bytes_per_row alignment, so decoder strides upload verbatim with no CPU repack.

Environment limits (AC #1 and AC #2 left unchecked):
- No /dev/dri at all, so no VA-API and no NVDEC; the only wgpu adapter is Mesa lavapipe (software).
- No H.264 GStreamer decoder exists here: gst-libav, openh264 and x264 plugins are all absent (libavcodec.so.60 and libx264.so.164 are installed but no plugin wraps them). VP8 in Matroska was used instead - it exercises the identical uridecodebin3 -> videoconvert -> appsink(NV12) path and the identical upload, differing only in which decoder autoplugs.
- One headless X display and no compositor: 'play' starts, links the pipeline and creates the shared device, but eframe never calls App::ui, so no frame is ever painted and the pop-out viewport cannot be exercised, let alone moved to a second monitor. 'decode-bench' exists precisely to route around that and is what produced the latency numbers.

Verification run: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p spike-nv12-viewport -p sub-render -p sub-media all green (38 spike tests, 12 sub-render, 11 sub-media).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Spike delivered: 'spikes/nv12-viewport' implements GStreamer decode -> NV12 -> wgpu upload -> Rec.709 conversion -> egui, with the converted texture registered once and drawn in both the main window and an egui deferred pop-out viewport. Findings are written up in backlog doc-1 with measured upload cost (4K write_texture mean 733 us, ~16 GiB/s payload; no bytes_per_row alignment needed so decoder strides upload verbatim) and measured decode-to-display latency on a real 4K clip (mean 9097 us, of which the thread hand-off is ~30 us), plus two GStreamer findings that will otherwise cost TASK-14 the same debugging: decodebin3 has no autoplug-sort signal (rank promotion is the only lever) and adds its pads before caps exist. AC #3 is checked. AC #1 and AC #2 are left unchecked and cannot be met in this environment: there is no /dev/dri (no VA-API, no NVDEC, software-only wgpu adapter), no H.264 GStreamer decoder plugin is installed, and the headless X server has no compositor, so eframe never paints a frame and the pop-out cannot be shown on a second monitor. Both need re-running on real hardware with 'spike-nv12-viewport play <file.mp4>', whose status line names the decoder that was instantiated. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, and cargo test across the touched crates.
<!-- SECTION:FINAL_SUMMARY:END -->
