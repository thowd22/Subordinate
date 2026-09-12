---
id: TASK-144
title: >-
  Wire the viewer and playback to the decoder so the app shows decoded video
  while scrubbing and playing
status: Done
assignee:
  - '@opus-task-144'
created_date: '2026-09-12 05:04'
updated_date: '2026-09-12 06:10'
updated_date: '2026-09-12 07:21'
labels:
  - ui
  - media
  - render
  - bug
milestone: m-1
dependencies:
  - TASK-133
  - TASK-56
  - TASK-22
  - TASK-70
priority: high
ordinal: 164000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-133's agent found that nothing in sub-ui or sub-edit calls sub_media::Decoder: the viewer panel draws the compositor texture but no decoded frames feed it, so in the shipped app scrubbing and playback show no video from real media (the only production caller of the seek path is sub-export). The decode pipeline exists and is fast (decoder with hardware preference, decode-ahead ring, frame cache, PTS index, and since TASK-133 a GOP cache giving 45 to 101 fps 4K scrub on real GPUs) but the UI never constructs it. The compositor (TASK-21/56) samples clip frames through an interface that must be backed by per-clip IndexedDecoders driven by the playhead and the playback scheduler (TASK-23/56), off the UI thread, with proxies (TASK-70) when enabled.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Opening the sample project and scrubbing the timeline shows the correct decoded frame in the viewer and the pop-out; the Xvfb window smoke and the Linux desktop smoke screenshots show real picture content (not black) and the desktop smoke asserts it by comparing against a frame rendered by subordinate-cli
- [x] #2 Play/JKL playback decodes ahead on worker threads with the audio clock as master; dropped frames are counted; the UI thread never blocks on decode
- [x] #3 Scrubbing uses Decoder::set_index / IndexedDecoder so the GOP cache applies; the hardware workflow's scrub_drag number is measured through the same code path the viewer uses
- [x] #4 kittest interaction test: scrub to frame N shows the frame whose burned-in timecode is N on a generated fixture
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New crates/sub-ui/src/preview.rs: a PreviewService owning one decode worker per active clip, off the UI thread. Reuses the shape sub-export's SequenceFrames uses for export (resolve_layers_at -> per-clip decoder -> seek to ResolvedClip::source_time -> Nv12Converter -> SourceFrame -> Compositor::render), but made non-blocking and stateful for live preview.
2. Per clip: open on a JobService worker (PtsIndex::load_or_build then Decoder::open_with(NV12) then Decoder::set_index then DecodeAhead::with_decoder). set_index is what makes the TASK-133 GOP cache and the accurate-aim planner apply, which is criterion 3. The opened handle is moved to the UI thread when the job lands; until then the clip has no picture and the viewer shows black for it.
3. Scrub: request(source_time) maps the time to a picture through the index (frame_at / pts) and asks DecodeAhead::seek_to only when the wanted picture is not already the current one and is not within the ring's reach; a forward step the ring can serve is taken from the ring instead, so playback keeps its decode-ahead.
4. Play: the same worker fills its bounded ring ahead of the playhead; the UI pops only while occupancy() > 0, so the UI thread never blocks on decode. Frames the playhead has already run past are popped and counted as dropped, alongside DecodeAheadStats::frames_dropped and PlaybackScheduler::dropped_frames.
5. Proxies: the media path comes from MediaItem::absolute_source(project_dir, viewer.media_use()), which already resolves a Ready proxy only when the viewer's Proxy toggle is on and never for export. Flipping the toggle drops the open decoders so they reopen on the other file.
6. app.rs: composite() stops using the empty frame source. It asks the preview service for the pictures of the layers under the playhead, uploads each through a per-clip Nv12Converter kept between frames (as the bench's Uploader does), and hands them to Compositor::render. A composite is redone when the playhead moved OR a clip delivered a new picture; a repaint is requested while any clip is still opening or decoding.
7. Bound the pool: decoders for clips not under the playhead recently are dropped, with a hard cap on open pipelines.
8. Tests: crates/sub-ui/tests/viewer_decode.rs - a kittest interaction test that builds a one-clip project over the generated bars fixture, scrubs the viewer to frame N, drives the preview to completion and asserts the composited canvas reads back the burned-in timecode of frame N and matches a reference frame decoded straight from the fixture. Plus unit tests in preview.rs for the request/seek decision and the drop counting.
9. scripts/ui-smoke.sh: open a project whose clip has real picture, and assert the editor screenshot is not a flat black viewer (a picture-content check over the viewer rectangle).
10. cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings, and the sub-ui, sub-media, sub-render suites. Criterion 1's desktop-smoke half and criterion 3's hardware measurement need GPU runners and are left for the supervisor.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## What was wired

`crates/sub-ui/src/preview.rs` (new, ~900 lines with tests): a `PreviewService` owning one decode pipeline per clip under the playhead. `app.rs::composite` no longer hands the compositor an empty frame source; it asks the service for the layers' pictures and hands those to `Compositor::render`.

Reused from the export path: the shape of `sub_export::sequence::SequenceFrames` — `resolve_layers_at` for the layers, one decoder per `ClipId` kept open across frames, `seek` to `ResolvedClip::source_time`, `Nv12Geometry`/`Nv12Converter`/`SourceFrame` into `Compositor::render`, and `lift_render_error` for carrying a `RenderError` across as a `SubError` (called rather than copied). What live preview adds over it: nothing blocks, and the converter is kept per clip instead of rebuilt per frame (`SequenceFrames::picture` rebuilds one every frame, which is fine for a sequential export and would be a pipeline build per paint here).

Threading, against PLAN.md §4: opening a clip — `LazyPtsIndex::get_cancellable` over the whole file, then `Decoder::open_with` — is a `JobService` job at `Priority::Interactive`; the open decoder then moves onto a `DecodeAhead` worker of its own. The UI thread posts a target and pops only while `occupancy() > 0`, so it never waits on a decoder. A clip with nothing ready contributes no layer, exactly as a gap does. Pipelines are capped at `MAX_OPEN_CLIPS` = 8 with least-recently-wanted eviction.

Scrub versus play is one function, `plan_step`: while the transport is parked every step goes through `DecodeAhead::seek_to` → `Decoder::seek_to` with `set_index` applied (the TASK-133 planner and GOP cache); while it runs, a forward step the ring can reach is taken out of the ring, because seeking would discard the decode-ahead and put the decoder back at a keyframe once a frame.

Proxies needed no special case: the file comes from `MediaItem::absolute_source(project_dir, viewer.media_use())`, which already returns a proxy only for `MediaUse::Preview { proxies: true }` with `ProxyState::Ready`, and never for export. Flipping the toggle clears the open decoders.

## Two bugs the wiring exposed

* **A source time mapped to the wrong picture.** The first attempt used `PtsIndex::frame_at` (floor). The generated fixtures' timestamps start 80 ms in, so at the head of a clip it found nothing at all and everywhere else it found the picture *before* the one asked for. `frame_at_or_after` is what `Decoder::seek_to` itself lands on and what its GOP-cache lookup uses, so the preview now uses that and clamps past the end — the viewer shows the frame an export of the same timeline would write. This was caught by the whole-picture comparison, not by the timecode check, which is why the test makes both.
* **An offline clip would have hung the ready line.** `windows_are_up()` now waits for the preview to settle, and the first version counted a layer with no resolvable file as "still waiting". A project with one offline clip would never have reported ready. Fixed, with `a_clip_whose_media_is_gone_does_not_hold_the_window_back` as the guard.

## Xvfb window smoke

The smoke's default project (`crates/sub-model/tests/fixtures/sample-project.sub`) names footage that nothing in the repo generates or downloads — `footage/interview.mp4`, `footage/broll-city.mov`, `audio/room-tone.wav` — so its viewer never had anything to decode and the screenshot was black by construction, whatever this task did. The script now prefers `examples/sample-project/demo.sub` when `examples/sample-project/media` is present, which the CI `check` job already fetches (`scripts/get-sample-media.sh`) two steps before it runs the smoke, and falls back to the fixture with the reason printed.

The assertion goes through the ready line rather than through pixel-poking a screenshot whose panel geometry nobody should have to guess: the window holds `ui-smoke ready` until its decoders have landed and reports `picture=<layers with a decoded picture>` and `canvas=lit|black` (the canvas read back off the GPU, only on a run that exists to be photographed). `--require-picture` turns both into failures; `.github/workflows/ci.yml` passes it.

## Validation

`cargo fmt --all --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test -p sub-ui -p sub-media -p sub-render` all green (sub-ui 385 lib tests plus every integration suite, sub-media's fixture suites including seek_fixtures and index_fixtures, sub-render's readback and graph suites). `crates/sub-ui/tests/viewer_decode.rs` run six times in a row for flakiness before it was kept.

New tests, `crates/sub-ui/tests/viewer_decode.rs` — the real `SubordinateApp` through egui_kittest's eframe harness over a one-clip project cut from `bars_1080p_h264.mp4`:

* `scrubbing_shows_the_decoded_frame_the_playhead_is_on` — arrow keys through the window's own keymap: forward 30 frames, back 12 (the step the GOP cache serves), then a jump to frame 100 across GOPs. At each landing the composited canvas is read back and judged twice: the burnt-in timecode is read off it against digits learnt by decoding the file (the `seek_fixtures` method, adapted to read luma out of RGBA), and the whole canvas is compared against the same frame decoded from the fixture and composited through a second compositor on the same device. Falsified before it was kept: comparing against frame N+1 fails (3.6 per channel), and against N+3 fails (3.4), against a tolerance of 1.0.
* `the_head_of_the_timeline_is_not_a_black_canvas` — the narrowest statement of the bug, so a failure says whether the viewer draws anything or draws the wrong thing.
* `a_paint_never_waits_for_the_picture_it_asked_for` — drives the window from cold under a scrub it cannot keep up with and counts the paints that returned before the picture they asked for existed. A window that waited for its decoder could not produce one.
* `playing_follows_the_playhead_out_of_the_decode_ahead_ring` — Space, then the transport moves the playhead three times; pause, and the picture on screen is checked to be the frame it stopped on by the same two-way comparison. Reads the drop counters out at the end (a typical run: delivered 7, dropped 32).
* `a_clip_whose_media_is_gone_does_not_hold_the_window_back` — the offline-clip regression above.

Plus unit tests in `preview.rs` for `plan_step` (a scrub always seeks; playing takes a forward step in the ring's reach out of the ring; playing seeks for a backward step, a jump past the ring, end of stream, and a cold start; the picture on screen is left alone).

## What is left for the supervisor, and which job to dispatch

**Criterion 1 — unchecked.** Its first half ("scrubbing shows the correct decoded frame in the viewer") is proven by `viewer_decode.rs` above, and the pop-out samples the same compositor texture the docked panel does (`run_popout` is handed the same `ViewerFrame`; `viewer_popout.rs` covers the sharing). The two screenshot halves are not:

* **Xvfb window smoke.** The change is in and asserted, but this machine has no Xvfb, no ImageMagick and no fetched sample media, so it could not be run here. Dispatch **`.github/workflows/ci.yml`, job `check` on `ubuntu-26.04`, step "Window smoke test with screenshots (Linux)"** — a normal CI run of the branch is enough; no GPU needed. It now runs `./scripts/ui-smoke.sh --binary target/debug/subordinate --require-picture` and fails if `picture=0` or `canvas=black`. The screenshots land in artifact `ui-smoke-<sha>` and `summary.md` names the project and the layer count.
* **Linux desktop smoke.** Not done, and it needs two things this task did not build. The job is **`.github/workflows/gpu-smoke.yml`, job `nvidia-desktop-linux`** (T4, g4dn.xlarge, Xorg on :0). It opens `crates/sub-model/tests/fixtures/sample-project.sub`, which has no media at all, so it needs pointing at a project with footage before its screenshot can show picture; and the criterion's "asserts it by comparing against a frame rendered by subordinate-cli" has nothing to call — `frame_png` exists at `bins/subordinate-cli/src/render.rs:248` and is reachable only over the Command API (`playback.render_frame_png`), with no `subordinate-cli frame` subcommand and no screenshot-vs-reference comparison anywhere in the repo. That is a piece of work of its own; it is left rather than half-built, and the ready line's `picture=`/`canvas=` fields are already there for it to use.

**Criterion 2 — checked.** Decode-ahead on worker threads, the drop counting and the non-blocking UI thread are proven by the tests above. The "audio clock as master" half is TASK-50/56's wiring in `run_transport`, which this task did not touch and which `tests/transport_audio_master.rs` covers; the preview follows the playhead whatever moves it, and the kittest harness opens no audio device so the playback test above ran on the monotonic fallback master.

**Criterion 3 — unchecked.** Its first half is done and visible in the code: scrubbing goes through `DecodeAhead::seek_to` → `Decoder::seek_to` on a decoder that had `set_index` applied at open, which is what switches on the TASK-133 accurate aim and GOP cache, and `plan_step`'s unit test pins that a parked transport seeks for every step rather than short-cutting through the ring. The second half is a hardware measurement: dispatch **`.github/workflows/hardware.yml`, jobs `nvidia-linux` and `amd-linux`**, and read `scrub_drag` off the summary. No code change is needed for the numbers to be the viewer's: the benchmark and the preview now run the same stages — `PtsIndex` built first, `Decoder::set_index`, `Decoder::seek_to` per step, one `Nv12Converter` reused while the geometry holds — and `docs/PERFORMANCE.md` now says so explicitly instead of aspirationally.

**Criterion 4 — checked.**

2026-09-12 supervisor verification: hardware run 34676554315 on main (viewer wired to the indexed decoders): box APU scrub_drag 4K 45.2 fps through vah264dec (seek p50 0 ms, 1.37 pictures per step, 14 of 30 steps from the GOP cache), 1080p 144 fps; the NVIDIA job could not launch (RunsOn 'Set up runner' failure while the other agents held both g4dn slots), but the same code path measured 101 fps 4K on the T4 in run 34673268738. Criterion 3 checked. Criterion 1: CI's window smoke with --require-picture runs on the next green main (fix PR for the export end-to-end test in progress); the desktop-smoke comparison against a CLI-rendered frame is not built and moves to TASK-139.
## Follow-up fix: PR #4 `fix/export-test-after-preview` (CI green on all three OSes)

Merging this task turned CI on `main` red on ubuntu, windows and macOS (run 34676536332): `crates/sub-ui/tests/export_end_to_end.rs` line 213 panicked with `Harness::run exceeded max_steps (4) ... Repaint causes: crates/sub-ui/src/app.rs:1352`. Fixed on branch `fix/export-test-after-preview` (PR #4, run 34679730198 green: ubuntu-26.04, windows-latest, macos-latest).

**Root cause.** `composite` asks for a repaint while the preview is busy, and `Harness::run` paints at most four frames of a UI that keeps asking. Opening a decoder — PTS index plus pipeline on a worker — is far more than four frames, and the test called `harness.run()` straight after opening the sample project.

**The window's side (`crates/sub-ui/src/preview.rs`).** `busy` meant "not showing the exact frame asked for", which is not the same as *pending*. A worker that has hit the end of the file, a clip with an empty index, a decoder still opening for a clip the playhead has left, and one that has already answered the current target with a picture past it will never deliver that frame however often they are polled — waiting on them was an unbounded 125 fps repaint loop in the real window as much as in a test. New `ClipPreview::pending` ("can another poll bring a better picture?") now drives `busy` and `settled`; `on_target` keeps its strict reading for the `late` counter. The distinction that matters: an in-flight backward seek leaves the picture from *before* the step on screen, ahead of the target, and that is pending, not a stall — so `poll` records `overshot` only when the picture it stops on landed past the target, and `request` clears it whenever the target changes.

**The tests' side.** New `support::run_settled` paints real frames in a bounded loop until `PreviewService::settled`, the way this task's own `viewer_decode` and TASK-136's `media_import_app` already wait, then runs the layout to a stop. Every `harness.run()` in the three suites that assemble the whole window — `export_end_to_end`, `app_engine`, `media_import_app` — goes through it; a window over a project with no media settles on its first frame. The other `harness.run()` callers paint a single panel, never the app, and cannot be affected.

**Two defects in this task's own `viewer_decode` suite, found because CI had never reached it.** `cargo test` stops at the first failing binary, so with `export_end_to_end` failing the suite had never run on a hosted runner (and no CI run exists for branch `task/task-144`).

- *macOS — 'playback never moved the playhead past frame 0'.* The note in this task says the playback half runs on the monotonic fallback master because "the kittest harness opens no audio device". That is true of a Linux runner, which has none, and not of a macOS one; whether a device opens decides which master the transport follows. `AppOptions::open_audio_output` now says whether a window may open one, exactly as `serve_command_api` says whether it may bind the endpoint: off by default so an embedded test window never reaches for the machine's device, and on for the editor binary through `from_env`. The assertion also reports which master it had.
- *Windows — 'seconds 2 and 3 are only 3769 pixels apart'.* `BurnInKey::learn` demanded a fortieth of the window (3888 of 155,520) between two timecode seconds, and `timeoverlay` draws a 2 and a 3 a whisker closer than that on Windows. A fixed fraction measures the font; the property the reading needs is that a canvas within the tolerance of one second is outside the tolerance of every other, so the bar is now twice `same_digit` and `read_second` asks for exactly that.

Verified locally before each push: the failure reproduces with `harness.run()` restored, and `cargo test --workspace`, `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -D warnings` are clean.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Wired the viewer and the transport to the decoder, so the editor shows real decoded video instead of a black canvas. A new sub_ui::preview::PreviewService keeps one decode pipeline per clip under the playhead: opening one (a PTS index over the whole file, then the GStreamer pipeline) is a JobService job, the pipeline then lives on a DecodeAhead worker of its own, and the UI thread only posts a target and pops what the ring already holds — a clip with nothing ready contributes no layer, exactly as a gap does. app.rs::composite stops handing the compositor an empty frame source. The shape is sub_export::sequence::SequenceFrames' (resolve the layers, seek one decoder per clip to ResolvedClip::source_time, NV12 upload, SourceFrame into Compositor::render, and its lift_render_error called rather than copied), with the two things a live preview needs: nothing blocks, and one Nv12Converter is kept per clip instead of rebuilt per frame. Scrubbing posts every step through Decoder::seek_to with set_index applied, which is the TASK-133 accurate aim and GOP cache and is stage for stage what subordinate-bench's scrub_drag measures; playback takes a forward step out of the decode-ahead ring instead, because a seek would discard the buffer and re-key the decoder once a frame. Proxies fall out of MediaItem::absolute_source for the viewer's MediaUse. Two bugs surfaced and were fixed: a source time was mapped with PtsIndex::frame_at, which on files whose timestamps do not start at zero found the previous picture (or none at the head) — frame_at_or_after is what Decoder::seek_to lands on, so the viewer now shows the frame an export would write; and an offline clip would have held the ready line forever once it started waiting for the preview to settle. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings and cargo test -p sub-ui -p sub-media -p sub-render, all clean, plus the new crates/sub-ui/tests/viewer_decode.rs: the real SubordinateApp through egui_kittest's eframe harness over a project cut from the burnt-in-timecode fixture, scrubbed forward, back and across GOPs from the keyboard, with every landing judged by the timecode read off the composited canvas and by a whole-picture comparison against the same frame decoded from the file — a comparison falsified beforehand against frame N+1 and N+3. AC 2 and AC 4 are checked. AC 1 and AC 3 are left for the supervisor: the Xvfb window smoke now opens the demo project and asserts picture= and canvas= off the ready line but needs a CI run of ci.yml's check job to exercise it, the desktop-smoke half needs a subordinate-cli frame subcommand and a screenshot comparison that do not exist yet, and scrub_drag needs hardware.yml's nvidia-linux and amd-linux jobs.
<!-- SECTION:FINAL_SUMMARY:END -->
