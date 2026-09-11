---
id: TASK-134
title: Export honours an explicitly pinned encoder even when GStreamer ranks it NONE
status: Done
assignee:
  - '@opus-task-134'
created_date: '2026-09-11 14:48'
updated_date: '2026-09-11 17:25'
labels:
  - export
milestone: m-4
dependencies:
  - TASK-116
priority: medium
ordinal: 154000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
sub-export refuses any encoder element whose GStreamer rank is NONE. Every VA-API encoder (vah264enc, vah265enc) ships with rank NONE by design, so on AMD Linux an explicit --encoder vah264enc fails unless GST_PLUGIN_FEATURE_RANK is set, which is what the hardware workflow does as a workaround (TASK-116 notes). The automatic selection order may still skip rank-NONE elements, but a user or preset that names an encoder explicitly has already made the choice.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 subordinate-cli render --encoder vah264enc works on box without GST_PLUGIN_FEATURE_RANK, verified in the hardware workflow's AMD job
- [x] #2 Automatic encoder selection behaviour is unchanged and covered by the existing capability-probe tests
- [x] #3 The workaround environment variable is removed from hardware.yml
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Split probe_element into a rank-blind READY probe plus a deranked flag, so a rank-NONE element is still probed for real and its NONE rank is recorded rather than faked as not-ready.
2. Carry the flag through ElementProbe and EncoderStatus (new 'deranked' field); is_usable() stays 'present && ready && !deranked' so automatic selection order is unchanged.
3. EncoderProbe::select: an explicit override is accepted when the element is present and READY even if deranked (log it); automatic fall-through still skips deranked elements. element_is_usable() keeps honouring NONE.
4. Update the diagnostics panel line to distinguish 'ready but ranked NONE (pinned only)' from ready/not usable.
5. Tests: pinned deranked element selects; automatic selection still skips it; real-GStreamer test that probe_element reports a deranked element present+ready+deranked; existing capability-probe tests unchanged.
6. Remove the GST_PLUGIN_FEATURE_RANK workaround from both render steps in .github/workflows/hardware.yml and update docs/DEVELOPMENT.md note.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implementation: a NONE rank is now recorded on the probe result rather than folded into 'ready'. ElementProbe and EncoderStatus carry a 'deranked' flag; probe_element drives every element to READY (probe_ready) and only then reads the rank. EncoderStatus::is_usable() stays 'present && ready && !deranked', so the automatic selection order behaves exactly as before, and a new EncoderStatus::is_pinnable() ('present && ready') is what EncoderProbe::select uses for an explicit override. element_is_usable() (muxers, parsers, audio encoders - all autoplug-adjacent) still honours a NONE rank. The diagnostics panel gained a third line state: 'ready, ranked NONE here: used only when it is pinned'.

Verification run in this worktree (Linux, no GPU, extracted GStreamer): cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (EncoderStatus needed an explicit allow for clippy::struct_excessive_bools, with a reason); cargo test -p sub-export (60 lib + 6 + 4 + 1 + 4 doc, all pass) and cargo test -p sub-ui (all pass).

New tests: encoder::tests::automatic_selection_skips_a_deranked_encoder, ::a_pinned_encoder_is_used_even_when_it_is_ranked_none, ::a_pinned_encoder_that_cannot_run_is_still_refused, the rewritten ::an_element_ranked_none_is_reported_deranked_but_still_probed (real GStreamer), sub-ui diagnostics::tests::a_deranked_encoder_reads_as_pinned_only, and the integration test crates/sub-export/tests/pinned_deranked_encoder.rs, which ranks x264enc NONE for the process and then renders a real Matroska file through ExportElements::resolve with the encoder pinned - the same path subordinate-cli render --encoder takes.

AC #1 is left unchecked: it names the hardware workflow's AMD job on 'box', which needs the GPU runner and cannot be exercised from this environment. The code path it covers is proven locally by the integration test above (a deranked element, pinned, renders), and the GST_PLUGIN_FEATURE_RANK workaround is gone from both render steps in .github/workflows/hardware.yml, so the next hardware run is the verification.

AC #3 evidence: grep GST_PLUGIN_FEATURE_RANK .github/workflows/hardware.yml returns only a comment line explaining why none is needed. docs/DEVELOPMENT.md's 'Hardware encoders are ranked NONE' note was updated to match.

2026-09-11 supervisor verification: hardware run 34626676058's AMD job on box rendered the sample project with --encoder vah264enc and validated it with the discoverer; hardware.yml on main no longer sets GST_PLUGIN_FEATURE_RANK (only a comment remains explaining it is unnecessary).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
An explicitly pinned encoder bypasses the rank-NONE filter while automatic selection is unchanged; verified on the box APU with vah264enc through the hardware workflow without any environment override.
<!-- SECTION:FINAL_SUMMARY:END -->
