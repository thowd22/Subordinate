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

## CI

The `Scrub and playback benchmark (Linux)` step of `.github/workflows/ci.yml`
runs the harness on every CI Linux job, prints the summary into the job log and
the step summary, and uploads `target/bench/perf.json` as the `perf-report`
artifact (30 days). It runs on the debug artifacts the test step already built,
so those numbers are several times slower than the release baseline above and
are only ever compared against other CI runs.
