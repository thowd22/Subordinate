---
id: TASK-143
title: >-
  Export matrix: every hardware and software encoder on every runner, from the
  CLI and the GUI, validated
status: In Progress
assignee:
  - '@opus-task-143'
created_date: '2026-09-12 03:58'
updated_date: '2026-09-12 04:50'
labels:
  - export
  - gpu
  - test
milestone: m-4
dependencies:
  - TASK-135
  - TASK-116
  - TASK-138
priority: high
ordinal: 163000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Export has been verified one encoder at a time (hardware workflow: nvh264enc on the T4, vah264enc on box; AMF smoke on the user's desktop; x264 in CI) but never as a matrix, never through the GUI export path on hardware (TASK-135's note), and never for HEVC or AV1. Build a matrix job set that renders the sample project and the user's 4K60 three-audio-track excerpt with every encoder each machine exposes, from subordinate-cli and through the assembled app's export runner, with each preset, validating stream layout, frame count, duration and audio presence with gst-discoverer, and comparing GUI versus CLI outputs. Machines: box (vah264enc, vah265enc, x264enc, x265enc), NVIDIA Linux runner (nvh264enc, nvh265enc, nvav1enc), NVIDIA Windows runner (nvh264enc, nvh265enc, mfh264enc, mfh265enc), the user's Windows desktop yodaddy (amfh264enc, amfh265enc, amfav1enc), hosted runners (software). Failures must name encoder, preset, machine and the GStreamer error. Keep GPU jobs on spot and short; never run more than two g4dn jobs at once.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 hardware.yml (or export-matrix.yml called from it) runs the matrix on demand and nightly; the job summary is a table of machine x encoder x preset x source with pass/fail and file size, and artifacts hold every failing output and its discoverer report
- [ ] #2 Every encoder that the machine's gst-inspect reports present either passes or has a filed bug task with the error; the 4K60 excerpt exports with hardware encoders on box, the T4 and yodaddy
- [ ] #3 GUI-path exports (sub-ui export runner on the Linux desktop image) match CLI exports on frame count and audio for at least nvh264enc and x264enc
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Read the existing workflows (hardware.yml, gpu-smoke.yml), the CLI render path, sub-export's encoder catalogue/presets and the GUI export runner; confirm how binaries reach each machine.
2. sub-export gaps found: the catalogue has no mfh265enc/amfav1enc/vaav1enc/software-AV1 entry and there is no AV1 preset, so AV1 and MF HEVC cannot be pinned at all. Add those catalogue entries and an 'av1-archive' preset (mkv/av1/opus) so 'every present encoder' is reachable; keep the documented selection order.
3. New shared driver scripts/export-matrix.py (Python 3, Linux+Windows): discovers encoders from 'subordinate-cli diag' (probe: present/ready/deranked) plus gst-inspect, builds the machine x encoder x preset x source case list, runs 'subordinate-cli render --encoder --preset --verify', then validates each output independently: gst-discoverer stream layout and duration, an exact decoded frame count through a gst-launch identity counter, audio presence and codec, and file size. Writes results.json, a markdown table for the job summary, and keeps only failing outputs plus their discoverer reports.
4. Second script half: build a project around the 4K60 excerpt by driving the Command API of 'subordinate-cli serve' over its socket/named pipe (project.new, media.import, timeline.add_track, timeline.add_clip, project.save), so the 4K source needs no hand-written .sub.
5. New workflow .github/workflows/export-matrix.yml (workflow_dispatch + nightly), free hosted build jobs for the Linux and Windows binaries, then one job per machine: hosted-software (ubuntu), amd-linux (box), nvidia-linux (T4), nvidia-windows (T4 Windows), amd-windows (yodaddy, short, RUNNER_TEMP only, cleaned up), and nvidia-desktop-linux (inline ami=... desktop image) for the GUI-vs-CLI comparison. The g4dn jobs are chained with 'needs' so at most one of mine runs at a time.
6. GUI comparison on the desktop image: the baked release editor exports through the Command API socket (export.render); nvh264enc versus x264enc is forced with GST_PLUGIN_FEATURE_RANK on the editor process, and the GUI output is compared with a CLI render of the same project for frame count and audio.
7. Iterate on the free machines first (box, yodaddy, hosted) then the paid ones, staggering T4 dispatches after 'gh run list'; file a backlog bug task per failure naming encoder, preset, machine and the GStreamer error; record run ids in the task notes.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Run 34672568992 (branch push, free jobs only): build-linux passed; box failed at the matrix step because the --source list was built as a shell string and the sample sequence is called "Main cut" - word splitting handed argparse "cut::50". Fixed with bash arrays. box also has no ffmpeg, so no 4K60 excerpt reached the Windows jobs; a synthetic 4K60 three-track clip is now the stated fallback. The project builder itself worked: it read the user 32 GB meld-4k60-full.mkv as 3840x2160 at 60/1 with three 48 kHz stereo AAC streams and wrote a two-track project around it. CI run 34672571538 failed only on the user guide troubleshooting section, which is asserted against sub_export::encoder_names - fixed by naming the new catalogue entries there.

Run 34673584123: the hosted software job is green - 6 of 6 cells pass with exact frame counts (x264enc over three presets, x265enc, svtav1enc and av1enc), 2-4 seconds a cell except libaom av1enc at 82 seconds for eight frames, which is why software AV1 now renders a short range. Two driver faults it found and fixed: the frame counter decoded the whole file, so a runner with no AAC, FLAC or Opus decoder failed to preroll on the audio track and counted nothing - it reads the muxed video track off qtdemux or matroskademux now - and a multi-line GStreamer failure dropped into a Markdown cell ended the table where it stood.

Findings filed as bug tasks: TASK-144 (presets carry a bitrate and a CRF that reach no encoder - ExportSettings has no quality field at all), TASK-145 (a source with three audio streams exports only its first), TASK-146 (every software export comes out in High 4:4:4 profile because nothing pins 4:2:0 between the compositor RGBA and the encoder), TASK-147 (the running editor Command API serves no export, probe or frame methods, so export.render reaches no window).

TASK-147 changed the plan for AC 3: the GUI export cannot be driven over the socket on the desktop image at all. The comparison is now a sub-ui test - the_window_and_the_cli_write_the_same_file_for_each_encoder - that opens the assembled SubordinateApp, pins an encoder in the export panel, exports the way a click on Export does, then renders the same frames through subordinate-cli with the same encoder and compares frame count and audio. It runs on box (vah264enc, x264enc) and on the T4 (nvh264enc, x264enc). The paid desktop-image job is gone with it.
<!-- SECTION:NOTES:END -->
