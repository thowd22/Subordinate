---
id: TASK-57
title: Encoder capability probe and selection order
status: Done
assignee:
  - '@opus-task-57'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-09 05:25'
labels:
  - export
milestone: m-4
dependencies:
  - TASK-24
references:
  - docs/PLAN.md
priority: high
ordinal: 78000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Encoder availability differs per machine (§5.5). Export must pick the best available element and let users override.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Probe enumerates nvh264enc/nvh265enc/nvav1enc, vah264enc/vah265enc, amfh264enc/amfh265enc, vtenc_h264/vtenc_h265, mfh264enc, x264enc/x265enc and tests each can reach READY state
- [x] #2 Selection follows the plan's order per codec with a user override in settings
- [x] #3 Result is cached per session and shown in the diagnostics panel
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add sub-export encoder module: VideoCodec (h264/h265/av1), EncoderVendor, catalogue of candidate elements in docs/PLAN.md 5.5 order.
2. Probe: build each factory and drive it to READY, recording present/ready/failure; injectable probe fn for tests.
3. EncoderPreferences with per-codec user override plus validation; selection order per codec honouring override, stable export.* error codes.
4. Cache the probe per session (OnceLock) and expose it; show encoder probe results in the sub-ui diagnostics panel and subordinate-cli diag JSON.
5. Unit tests over injected probe results plus a real-registry smoke test; fmt, clippy, cargo test.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in the previously empty sub-export crate.

crates/sub-export/src/encoder.rs: a catalogue of exactly the 13 element names in AC#1 (nvh264enc/nvh265enc/nvav1enc, vah264enc/vah265enc, amfh264enc/amfh265enc, vtenc_h264/vtenc_h265, mfh264enc, x264enc/x265enc) held in docs/PLAN.md 5.5 order. probe_element() builds each factory and drives it to READY (waiting out an async state change with a 2s timeout), then returns it to NULL; an element that registers but cannot start (plugin present, no device) is recorded present-but-not-ready with the GStreamer reason, which is what separates installed from usable. EncoderProbe::from_probe takes an injectable probe fn so the tests describe machines this environment does not have.

Selection: EncoderProbe::select(codec, prefs) walks the catalogue order filtered by codec and returns the first usable element; EncoderPreferences (serde, stored in settings) pins one element per codec. set_override validates against the catalogue (export.unknown_encoder) so settings can never name an unknown element, and an override that this machine cannot use is reported as export.encoder_unavailable rather than silently downgraded to software. Nothing available gives export.no_encoder. New stable codes live in sub_export::codes (export.init_failed, export.probe_failed, export.no_encoder, export.unknown_encoder, export.encoder_unavailable).

Caching: EncoderProbe::cached() runs the probe once per process behind a OnceLock and remembers the failure too, so the UI never re-instantiates elements while painting. The sub-ui diagnostics panel gained an 'Export encoders' section under the vendor list: a heading per codec naming the encoder an export would use and how it was chosen (automatic hardware/software, or set in settings), plus a line per candidate saying ready / registered-but-not-usable-with-reason / not registered. subordinate-cli diag gained the same data as an additive encoder_probe key so the CLI and the panel show the same thing.

Verification: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-export -p sub-ui -p subordinate-cli all green (15 sub-export unit tests, 11 sub-ui including headless painted-panel assertions, 4 CLI integration tests in tests/diag.rs). The real probe was also run end to end on this machine: subordinate-cli diag reports x264enc and x265enc present and reaching READY and the ten hardware elements absent, which is correct for this GStreamer install with no GPU. AC#1's READY check is therefore proven for the elements that exist here; the hardware paths cannot be exercised without NVIDIA/AMD/Apple hardware, which is what the separate verify tasks TASK-64/65/66 are for.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the encoder capability probe and selection order to sub-export: every encoder docs/PLAN.md 5.5 names is instantiated and driven to READY, so an installed-but-unusable element is distinguished from a working one, and selection takes the first usable element in the plan's per-codec order unless EncoderPreferences pins one in settings (validated, and an unavailable pin is an export.encoder_unavailable error rather than a silent downgrade). The probe is cached per session behind a OnceLock and rendered in the sub-ui diagnostics panel as an 'Export encoders' section plus an encoder_probe key in subordinate-cli diag. Verified with cargo fmt --check, clippy -D warnings, and cargo test across sub-export, sub-ui and subordinate-cli, plus a real diag run showing x264enc/x265enc reaching READY on this machine.
<!-- SECTION:FINAL_SUMMARY:END -->
