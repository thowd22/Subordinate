# Performance baselines

Phase 1's exit criteria are numeric (PLAN.md §8: *scrub a 4K H.264 clip at
>30 fps on Linux with nvdec/va*), so the numbers behind them are measured by a
harness rather than by hand: `bins/subordinate-bench`. This page records what
it measured, on what, and how to reproduce it.

## What the harness measures

Two scenarios per fixture, both driving the production paths
(`sub_media::Decoder` and `sub_render::Nv12Converter`, the same NV12 upload and
YUV-to-RGB pass the viewer uses):

| Scenario | What one step is |
| --- | --- |
| `playback` | `next_frame`, then upload and convert, waited to GPU completion |
| `scrub` | `seek_to` a fresh position, then the same upload |

Scrub targets alternate between the head and the tail of the clip and work
inward, so a run covers the whole file and every step is a real seek rather
than the decode-forward playback gets for free.

Two numbers come out of each scenario:

* **decode-to-texture latency** — one step end to end, reported as p50, p95,
  mean, min and max. This is what the viewer pays before a scrubbed frame can
  be shown.
* **sustained fps** — timed frames divided by the wall time of the whole run.

Every duration in the JSON report is an exact nanosecond count and every rate
is in milli-frames per second (`30_500` is 30.5 fps): no timing value is ever
a float, in line with the project-wide rule.

## Running it

```
cargo run --release -p subordinate-bench          # writes target/bench/perf.json
cargo run --release -p subordinate-bench -- --help
```

The fixtures must exist first (`scripts/gen-fixtures.sh`, or
`scripts/gen-fixtures.ps1` on Windows). The harness never fails on an
environment problem: a fixture that is not there is a `skipped` scenario, and a
machine with no wgpu adapter measures decode alone and marks the run
`[decode only: no GPU]`.

Useful switches: `--frames`/`--seeks` (how much is timed), `--software` (keep
hardware decoders out of the measurement, for a comparable software number),
`--no-gpu`, `--fixtures DIR`.

## How to read the numbers

* **Compare like with like.** The report records the OS, architecture, cargo
  profile and adapter. A debug build and a release build are not comparable,
  and neither is a software adapter and a real GPU.
* **A software adapter is an upper bound**, not a measurement of the machine:
  llvmpipe does the YUV-to-RGB pass on the CPU, and on a 4K frame that
  dominates everything else.
* **Scrub is slower than playback by design.** Each step is a keyframe seek
  plus a decode-forward, so it costs a fraction of a GOP, not one frame.

## Baseline: Linux software (no GPU, no hardware decode)

Recorded 2026-09-10 (TASK-26). WSL2 on an AMD Ryzen 9 9900X (24 threads), Mesa
lavapipe 25.2.8 (software Vulkan), GStreamer 1.24 with `avdec_h264` software
decode, release build, harness defaults (60 playback frames and 30 seeks per
fixture).

| Fixture | Scenario | Sustained fps | Latency p50 | Latency p95 |
| --- | --- | --- | --- | --- |
| `bars_1080p_h264.mp4` (1920x1080) | playback | 158.2 | 6.111 ms | 7.268 ms |
| `bars_1080p_h264.mp4` (1920x1080) | scrub | 21.7 | 45.282 ms | 63.519 ms |
| `bars_2160p_h264.mp4` (3840x2160) | playback | 47.3 | 20.769 ms | 23.463 ms |
| `bars_2160p_h264.mp4` (3840x2160) | scrub | 6.9 | 137.236 ms | 255.497 ms |

Run-to-run spread on this machine is about 10%, so treat a change smaller than
that as noise.

What the split inside the report shows: on playback the decode stage is tens of
microseconds, because the pipeline has already produced the frame by the time
`next_frame` is called, and the upload plus the YUV-to-RGB pass is essentially
the whole latency. On this machine that pass runs on the CPU, so it dominates —
these are software-rasteriser numbers, not what the compositor costs on a GPU.

Read the table as the floor, not the target: phase 1's ">30 fps 4K scrub"
criterion is stated against a GPU with a hardware decoder (PLAN.md §8).
Confirming it there is TASK-74 and the `verify` tasks, on hardware neither this
machine nor hosted CI has; when those run, add their rows here rather than
replacing these.

## Baseline: NVIDIA T4, hardware decode (nvdec)

Recorded 2026-09-11 (TASK-116), the first numbers from real GPU hardware. AWS
`g4dn.xlarge` in us-east-1 through RunsOn (4 vCPU, Tesla T4, driver 580.173.02,
Vulkan), Ubuntu 24.04 with apt GStreamer 1.24, `nvh264dec` plugged by the
production ranking, release build, harness defaults. Run 34610600133 of the
`Hardware verification` workflow; `perf.json` is on that run as the
`hardware-nvidia-linux` artifact.

| Fixture | Scenario | Sustained fps | Latency p50 | Latency p95 |
| --- | --- | --- | --- | --- |
| `bars_1080p_h264.mp4` (1920x1080) | playback | 535.8 | 1.652 ms | 3.678 ms |
| `bars_1080p_h264.mp4` (1920x1080) | scrub | 26.9 | 34.055 ms | 69.862 ms |
| `bars_2160p_h264.mp4` (3840x2160) | playback | 171.9 | 5.348 ms | 12.072 ms |
| `bars_2160p_h264.mp4` (3840x2160) | scrub | 8.3 | 111.690 ms | 219.388 ms |

Playback is what the hardware decoder buys: 4K playback is 171.9 fps against
47.3 on the software baseline above, and a 1080p frame reaches a texture in
1.65 ms instead of 6.1.

**Scrub is not there yet.** 8.3 fps on a 4K clip is a long way below phase 1's
">30 fps" criterion, and it is only marginally better than the 6.9 fps of the
software baseline - the decoder is plainly not what limits it. Each scrub step
is a keyframe seek plus a decode-forward, so what dominates is the flush,
re-prime and forward decode of a fraction of a GOP, not the decode of the one
frame asked for; the p95 of 219 ms against a p50 of 112 ms is the shape of a
seek that lands further from a keyframe. Closing that gap is work for TASK-64
and the phase 1 exit criterion, not something the harness can report away: the
hardware verification workflow prints this number and warns, and deliberately
does not fail on it.

Compositor readback on the same machine is comfortably past its own criterion:
588.7 fps on a 1080p canvas (611.7 fps on the run before it), against the
60 fps TASK-58 asked for.

## CI

The `Scrub and playback benchmark (Linux)` step of `.github/workflows/ci.yml`
runs the harness on every CI Linux job, prints the summary into the job log and
the step summary, and uploads `target/bench/perf.json` as the `perf-report`
artifact (30 days). It runs on the debug artifacts the test step already built,
so those numbers are several times slower than the release baseline above and
are only ever compared against other CI runs.

## A/V sync and drift

Phase 3's exit criterion is that the picture stays with the sound: over the
whole ten-minute fixture, video must never be a frame or more away from the
audio clock (PLAN.md §5.4, §8). The same binary measures it, in a mode of its
own: `subordinate-bench --sync` (TASK-56).

### What the harness measures

One run plays `longgop_720p_10min.mp4` headlessly, as fast as the machine will
go, through the production parts:

* `sub_audio::OutputRenderer` renders real callback-sized blocks (512 device
  frames) from the mixer at the 48 kHz sequence rate into a device that only
  offers 44.1 kHz, so every block goes through the sample-rate conversion, and
  publishes the `AudioClock` the callback would publish in the app;
* `sub_edit::playback::PlaybackScheduler` follows that clock and picks the
  frame to show — the audio position is the master, never a clock of the
  video's own;
* `sub_media::Decoder` decodes the fixture forward to that frame, so what is
  compared against the clock is a real decoded picture's PTS.

Once per second of timeline the audible position is sampled against the PTS of
the frame on screen, and two numbers come out of it:

* **offset** — the audible position minus the displayed frame's PTS. A frame is
  on screen for a whole frame interval, so while sync holds this sits between
  0 and 1 frame;
* **drift** — how far that offset reaches *outside* the displayed frame's own
  interval. Zero means the frame on screen is the one the samples being heard
  belong to; positive means the picture lags the sound, negative means it has
  run ahead.

Both are reported in milli-frames (`1_000` is one whole frame) and computed by
exact integer arithmetic over both timebases, so 23.976 and 25 are equally
exact and no drift number is ever a float.

The long fixture has no audio track, so the master is the sequence transport
running silence over the fixture's timeline; what the samples contain does not
enter into the clock relationship being measured. The complementary test that
drives ten minutes of callbacks with no decode at all is
`crates/sub-audio/tests/av_drift.rs`.

### Running it

```
cargo run --release -p subordinate-bench -- --sync   # target/bench/av-sync.json
cargo run -p subordinate-bench -- --sync --seconds 30
```

The long fixture must exist first: `scripts/gen-fixtures.sh --long`
(`scripts/gen-fixtures.ps1 -Long` on Windows), which the plain fixture run
deliberately skips. Without it the run is a `skipped` section and exits 0.
`--seconds N` shortens a run, `--fixtures DIR` points at another fixture set
and `--software` keeps hardware decoders out of it.

Unlike the scrub and playback harness, this one fails on a number: drift of a
whole frame or more exits non-zero with `bench.drift_exceeded`.

### Baseline: Linux software decode

Recorded 2026-09-10 (TASK-56). WSL2 on an AMD Ryzen 9 9900X (24 threads),
GStreamer 1.24 with `avdec_h264` software decode, debug build, whole fixture.

| Seconds played | Samples | Frames shown | Dropped | Max drift | Max offset |
| --- | --- | --- | --- | --- | --- |
| 599 | 599 | 14998 | 0 | 0.000 frames | 0.288 frames |

Zero is the expected result and the only passing one: the scheduler picks the
frame containing the master time, so as long as decode keeps up the picture is
never outside the frame the samples belong to. What the run proves is that
nothing accumulates over ten minutes of resampled blocks — no rounding creeps
into the clock, and no presentation is dropped. A non-zero drift, or a dropped
frame count that climbs, is a real regression in the clock relationship rather
than noise.

### CI

The `A/V sync and drift harness (Linux)` step of `.github/workflows/ci.yml`
generates the long fixture (the only step that does) and runs the harness on
every CI Linux job, printing the summary into the job log and the step summary
and uploading `target/bench/av-sync.json` as the `av-sync-report` artifact (30
days). This step is allowed to fail the job: that failure is how "under one
frame on Linux" is asserted.

## Editing an hour of long-GOP footage with proxies

Phase 5's exit criterion is about editing rather than decoding: a one-hour
long-GOP source, proxies switched on, a scrub that stays above 30 fps, no stall
over five seconds of playback, and memory inside a documented budget
(PLAN.md §5.2, §8). The same binary measures it in a third mode:
`subordinate-bench --proxy` (TASK-74).

### What the harness measures

One run takes `longgop_1080p_1h.mp4` — an hour of 1920x1080 H.264 with a
250-frame GOP and B-frames, the same structure as the ten-minute clip — and:

* makes an intra-only proxy of it through the production `sub_media::Proxy`
  path, at half resolution in whichever of `DNxHR` LB and MJPEG this
  installation can write at that size, or reuses one already in the cache
  exactly as the editor would;
* scrubs and plays **both** files through the same `sub_media::Decoder` and
  `sub_render::Nv12Converter` path the other scenarios use, so the four
  scenarios sit in one report and the proxy's advantage is visible rather than
  asserted;
* reads the process's own peak resident size (`VmHWM` on Linux) at the end.

Three integer comparisons come out of it, and they are what the criterion is:

| Criterion | How it is judged |
| --- | --- |
| Scrub above 30 fps | the proxy's sustained scrub rate, in milli-fps, against `30_000` |
| No stalls over 5 s of playback | timed playback steps that took longer than one frame interval (40 ms at 25 fps); zero is the passing value |
| Memory under a documented budget | peak RSS against 2048 MiB, `--memory-budget` to change it |

The budget is a constant, not a fraction of the source: an editor holding an
hour-long file open must not grow with the length of that file. 2 GiB is the
frame cache's own default budget (512 MiB) with room around it for the
decoder's buffers, the pictures in flight, the upload staging and the driver's
allocations. Peak memory is only measured on Linux; elsewhere the report
carries no `peak_rss_bytes` and that criterion is recorded as unmeasured rather
than guessed at.

### Running it

```
./scripts/gen-fixtures.sh --hour                      # ~15 min, ~380 MB
cargo run --release -p subordinate-bench -- --proxy   # target/bench/proxy.json
cargo run --release -p subordinate-bench -- --proxy --seconds 2 --seeks 10
```

The first run also transcodes the proxy, which takes minutes; every run after
it reuses the proxy from `target/bench/proxy-cache` (`--proxy-cache` puts it
somewhere else). Without the fixture the run is a `skipped` section and exits
0. Like `--sync`, a *measured* run fails on its numbers: a missed criterion
exits non-zero with `bench.proxy_below_target` and the summary names which one.

### CI

This mode is not in CI and is not meant to be: the fixture costs a quarter of
an hour of encoding and the proxy another few minutes, against a CI Linux job
that currently runs in about eleven minutes in total. It is a run made by hand
on a machine worth measuring, and its numbers are recorded below.

### Baseline: Linux software decode, software rasteriser

Recorded 2026-09-11 (TASK-74). WSL2 on an AMD Ryzen 9 9900X (24 threads), no
GPU and no hardware decoder: GStreamer 1.24 with `avdec_h264`, Mesa llvmpipe
25.2.8 for the NV12 upload and YUV-to-RGB pass, release build, harness
defaults (60 seeks, 5 s of playback).

Source: `longgop_1080p_1h.mp4`, 1920x1080 at 25 fps, one hour, 365 MiB.
Proxy: MJPEG 960x540, 90 000 frames, 1832 MiB, transcoded in 72 s.

| File | Scenario | Sustained fps | Latency p50 | Latency p95 | Stalls |
| --- | --- | --- | --- | --- | --- |
| source | playback | 265.7 | 3.764 ms | 4.028 ms | 0 of 125 |
| source | scrub | 8.8 | 111.689 ms | 203.457 ms | — |
| proxy | playback | 472.5 | 2.108 ms | 2.298 ms | 0 of 125 |
| proxy | scrub | 299.3 | 3.328 ms | 3.557 ms | — |

| Criterion | Result | Verdict |
| --- | --- | --- |
| Proxy scrub above 30 fps | 299.274 fps | PASS |
| No stall over 5 s of playback | 0 steps over 40 ms, slowest 2.437 ms | PASS |
| Peak memory under 2048 MiB | 514 MiB | PASS |

The scrub row is the whole argument for proxies, and it is the only row that
changes by an order of magnitude: on the original, every seek lands inside a
250-frame GOP and pays for the pictures in front of it, so a drag across the
hour runs at 8.8 fps — a quarter of the criterion — while the same drag on the
intra-only proxy costs one picture and runs at 299 fps. Playback is fast on
both, because playback decodes forward and a long GOP is *cheaper* to decode
sequentially than an intra-only file is; what the proxy buys there is headroom,
not a rescue.

Memory is the number to watch over time. 514 MiB peak is the frame cache's
budget plus the decoder and the transcode that ran in the same process, against
an hour-long, 365 MiB source — and it must stay a constant as sources get
longer. A peak that tracks the length of the file is the regression this row
exists to catch.

Read the whole table as a floor: software decode on a software rasteriser is
the slowest way to run any of this, and it still clears every number. A machine
with a hardware decoder and a real GPU has no way to be worse — and the row for
one should be added here (rather than replacing this one) when the `verify`
tasks run on real hardware (TASK-116's GPU runners).
