# Performance baselines

Phase 1's exit criteria are numeric (PLAN.md §8: *scrub a 4K H.264 clip at
>30 fps on Linux with nvdec/va*), so the numbers behind them are measured by a
harness rather than by hand: `bins/subordinate-bench`. This page records what
it measured, on what, and how to reproduce it.

## What the harness measures

Three scenarios per fixture, all driving the production paths
(`sub_media::Decoder` and `sub_render::Nv12Converter`, the same NV12 upload and
YUV-to-RGB pass the viewer uses):

| Scenario | What one step is |
| --- | --- |
| `playback` | `next_frame`, then upload and convert, waited to GPU completion |
| `scrub_drag` | `seek_to` the next picture of a dragged playhead, then the same upload |
| `scrub` | `seek_to` a fresh position across the file, then the same upload |

`scrub_drag` is the scrub the exit criterion is about: a hand moving a
playhead, which walks forward a few pictures at a time and steps back every
few moves. `scrub` is the worst case the seek path has, and it is deliberately
not what a hand does: its targets alternate between the head and the tail of
the clip and work inward, so every step lands in a GOP the decoder is not in
and a flushing seek is unavoidable. Both are reported; the criterion is read
off the drag, and the jump scrub is what says whether the seek itself is
getting cheaper.

Both scrub decoders are driven by a `PtsIndex` of the fixture, built before
anything is timed, because that is how the viewer scrubs: the index is what
can say where the pictures are without decoding them, and every cheap path
below depends on it.

Two numbers come out of each scenario:

* **decode-to-texture latency** — one step end to end, reported as p50, p95,
  mean, min and max. This is what the viewer pays before a scrubbed frame can
  be shown.
* **sustained fps** — timed frames divided by the wall time of the whole run.

A scrub scenario also reports where that latency went, because the two halves of
a scrub step answer to different fixes (TASK-133):

* **`seek`** — the flushing seek: the flush itself, the demuxer's re-prime, the
  reference chain the decoder works through before it can produce anything, and
  the first picture it delivers. Zero for a step that was reached by decoding
  forward.
* **`decode_forward`** — the pictures delivered between there and the frame that
  was actually asked for.
* **`frames_decoded`**, **`seeks_issued`** and **`cache_hits`** — pictures
  delivered, flushes issued and steps answered out of the decoder's picture
  cache across the timed steps. Divided by `frames` the first two are what a
  step costs, and they are the numbers to watch: a drag should be near one
  picture and near zero seeks a step, and every picture over one is a picture
  decoded, converted, downloaded and copied only to be thrown away.

The summary line prints the split, so a workflow log shows it without opening
the JSON:

```
scrub_drag bars_2160p_h264.mp4  3840x2160  20 frames  130.863 fps  p50 7.156 ms \
  p95 13.549 ms  seek p50 0.000 ms  fwd p50 5.810 ms  1.55 pics/step \
  0.05 seeks/step  8 cached
```

`--legacy-scrub` measures the seek path as it was before TASK-133 — no cache of
the pictures a step decoded, and no allowance for what a flush costs when a
step chooses between decoding on and seeking — so a before-and-after can be
taken on one machine, and on a runner, without rebuilding anything.

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
* **Scrub is slower than playback by design.** Each step flushes the pipeline
  and decodes the reference chain of the frame it wants, so it costs a fraction
  of a GOP of decoding, not one frame.

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

## Where a scrub step's time goes

Recorded 2026-09-11 (TASK-133) on the same WSL2 machine as the software
baseline above, release build, software decode, `--no-gpu --seeks 20`, so these
are decode-only numbers: no upload, no YUV-to-RGB pass.

| Fixture | seek p50 | decode-forward p50 | pictures delivered per timed step |
| --- | --- | --- | --- |
| `bars_1080p_h264.mp4` | 25.0 ms | 0.8 ms | 3.0 |
| `bars_2160p_h264.mp4` | 47.2 ms | 3.6 ms | 3.0 |

The fixtures have a one-second GOP at 25 fps, so a random target sits about
twelve pictures past the keyframe its decode has to start from. Those pictures
still have to be *decoded* — they are the frame's reference chain — but they no
longer have to be *delivered*. A seek is issued as a flushing accurate seek
whose segment opens just before the target (TASK-133), so the decoder clips them
instead of pushing them: nothing between the keyframe and the target is colour
converted, downloaded out of the decoder's memory or copied into a `VideoFrame`,
which at 4K is 12 MB a picture. A step delivers three pictures instead of
thirteen, and `decode_forward` collapses to the decode of the one that was
asked for. The reference chain is now inside the `seek` half, because it is
decoded before the first delivered picture exists.

Measured on the same machine, before and after that change, `--no-gpu
--seeks 20`:

| Fixture | pictures delivered over 20 steps | step p50 | sustained |
| --- | --- | --- | --- |
| `bars_1080p_h264.mp4` before | 254 | 27.1 ms | 36.5 fps |
| `bars_1080p_h264.mp4` after | 60 | 25.9 ms | 40.9 fps |
| `bars_2160p_h264.mp4` before | 240 | 63.7 ms | 14.5 fps |
| `bars_2160p_h264.mp4` after | 60 | 52.2 ms | 18.4 fps |

The fast path needs a `PtsIndex`, which the viewer builds in the background as
soon as a clip is imported and the harness builds before it times anything. A
decoder without one keeps the keyframe seek it always issued: the aim has to
know where the picture before the target is, and where the file's timestamps sit
relative to the timeline the container is seeked in, and only an index can say
either without decoding. The aim is the picture *before* the target rather than
the target itself because a decoder trims a buffer that straddles the start of
the segment rather than dropping it, which on a file whose frame durations are
not its real ones (a variable-rate Matroska carries one default duration for
every block) would otherwise hand back the previous picture wearing the target's
timestamp.

What the index removed in the pass before this one: a seek used to aim at the
target in stream time, which does not carry the offset a reordered stream's
timestamps start with — 80 ms on these fixtures — so the demuxer could snap to
the keyframe *after* the one the target needed and a second, blindly aimed seek
had to recover the frame. That took the pictures decoded over the 20-step 4K
scrub from 302 to 240 and removed the retry seeks.

**4K scrub was still short of the criterion after that pass.** 18.4 fps with
software decode on this machine, against ">30 fps" on hardware decode, and what
was left was the flush: 47 ms of the 52 ms step at 4K was the seek and the
reference chain it had to decode before the first picture of the new position
existed. The hardware runs said the same thing from two directions — on the T4
with `nvh264dec` the seek alone was 86.3 ms of a step (run 34640251740), while
on the box APU with `vah264dec` a step still decoded 14.2 pictures (run
34635562025) — and both are the same mistake: a step flushing the pipeline back
to a keyframe for a picture the decoder had already passed, or had just handed
out.

## Not seeking at all: what a drag costs

The pass that followed (TASK-133 again) stopped the step from flushing rather
than making the flush cheaper. Two changes, both of which need the `PtsIndex`:

* **The decoder keeps what it decodes.** Every picture a `seek_to` step pulls
  goes into a byte-budgeted `FrameCache` belonging to that decoder
  (`DecoderOptions::gop_cache_bytes`, 64 MiB by default — five 4K pictures or
  twenty-two at 1080p). A step whose target the index resolves to a cached
  timestamp is handed that picture: no seek, no decode, and the pipeline left
  where it was, so the next forward step still decodes on instead of rewinding
  to it. `VideoFrame::try_clone` is what makes it free — a second read-only
  mapping of the same GStreamer buffer, never a copy of the pixels.
* **The no-seek rule is "cheaper than seeking", not "inside this GOP".** With
  an index both ways of reaching a target can be priced in pictures: decoding
  on costs the pictures between here and the target, and seeking costs the
  pictures from the target's keyframe to it — the decoder cannot start anywhere
  else — plus the flush, which `DecoderOptions::forward_decode_slack` prices at
  twelve pictures. The old rule is that comparison with the slack set to zero.

Playback is untouched: only `seek_to` fills or reads the cache, a decoder with
no index keeps nothing, and `next_frame` and `DecodeAhead` are as they were.

Measured on the same WSL2 machine, release, software decode, `--no-gpu
--seeks 20`, before and after (`--legacy-scrub` against the default):

| Fixture | Scenario | sustained before | after | pictures/step | seeks/step |
| --- | --- | --- | --- | --- | --- |
| `bars_1080p_h264.mp4` | `scrub_drag` | 83.4 fps | **809.5 fps** | 2.65 → 1.15 | 0.35 → 0.00 |
| `bars_2160p_h264.mp4` | `scrub_drag` | 31.5 fps | **130.7 fps** | 2.60 → 1.55 | 0.40 → 0.05 |
| `bars_2160p_h264.mp4` | `scrub` | 18.1 fps | 18.0 fps | 3.00 | 0.90 |

Repeats agree: the 4K drag measured 29.9 fps legacy and 130.9 fps after on a
second pass, so the change is far outside this machine's run-to-run spread. Of
the 20 timed 4K drag steps, 8 are answered from the cache and one seeks.

The jump scrub does not move, and should not: every one of its steps lands in
a GOP the decoder is not in, which is a flush whatever the planner knows. That
is the measurement to watch if the flush itself is ever made cheaper; the drag
is the one the criterion is stated against.

Frame accuracy is the constraint the whole thing is shaped by, and it is
unchanged: `seek_fixtures` judges every seek by its burnt-in timecode and by
the whole picture, and `index_fixtures` now also walks a drag over a GOP and
compares every picture the cache hands back against the same file decoded from
the start.

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
