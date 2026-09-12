---
id: TASK-143
title: >-
  Export matrix: every hardware and software encoder on every runner, from the
  CLI and the GUI, validated
status: Done
assignee:
  - '@opus-task-143'
created_date: '2026-09-12 03:58'
updated_date: '2026-09-12 14:59'
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
- [x] #1 hardware.yml (or export-matrix.yml called from it) runs the matrix on demand and nightly; the job summary is a table of machine x encoder x preset x source with pass/fail and file size, and artifacts hold every failing output and its discoverer report
- [x] #2 Every encoder that the machine's gst-inspect reports present either passes or has a filed bug task with the error; the 4K60 excerpt exports with hardware encoders on box, the T4 and yodaddy
- [x] #3 GUI-path exports (sub-ui export runner on the Linux desktop image) match CLI exports on frame count and audio for at least nvh264enc and x264enc
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

Runs 34674014655, 34675906145 (branch push) and 34676733798 (dispatch, only=nvidia-linux).

box, run 34675906145: 17 of 20 cells pass. vah264enc, vah265enc, x264enc, x265enc, svtav1enc and av1enc all present and pinnable; vah264enc and vah265enc are ranked NONE and are used because they are pinned, which is exactly what TASK-134 built. Frame counts exact on every passing cell, audio present on every one. The three failures are all uhd x vah264enc - TASK-148 - and they stall rather than fail: frame 20 of 60 at zero fps for two presets, frame 1 for the third, while vah265enc writes the same sixty 4K frames in seconds.

hosted ubuntu-24.04: 6 of 6 pass, including both software AV1 encoders against the new av1-archive preset.

GUI versus CLI on box (TASK-143 AC 3), run 34675906145: "vah264enc: window 12 frames, CLI 12 frames, audio on both" and "x264enc: window 12 frames, CLI 12 frames, audio on both".

BLOCKED on the NVIDIA rows, and not by anything in this branch: every RunsOn GPU job in the repository is failing at runner resolution with "failed to resolve runner spec: gpu-nvidia-linux not found", including hardware.yml on main, which has not changed (run 34676554315, 06:02 UTC) and the TASK-139 dispatches. .github/runs-on.yml on main is valid and unchanged since TASK-138 merged, and GPU jobs worked at 03:32 UTC, so something in the RunsOn stack or its repository config resolution broke between 05:00 and 06:00 UTC. The nvidia-linux, nvidia-windows and (through them) the NVENC half of the GUI comparison are waiting on that.

Merged origin/main (release 0.1.3, the live preview work). Two collisions to record: main had taken TASK-144 for the viewer/playback decode work, so the preset-quality bug filed here was recreated as TASK-149 and the reference in docs/DEVELOPMENT.md follows it; and main added support::run_settled for exactly the cold-start repaint problem this branch had hit with its own settle helper, so the helper is gone and both window tests use main version.

Run 34683736400, yodaddy (AMD RX 9070 XT, Windows 11, GStreamer 1.28.6): the AMF row exists at last, and it is the richest machine in the matrix - amfh264enc, amfh265enc, amfav1enc, mfh264enc, mfh265enc, x264enc, x265enc, svtav1enc and rav1enc all present and READY, amfav1enc ranked NONE and therefore only reachable because the matrix pins it. Every one of them wrote a file with the codec its preset asked for and the audio beside it. amfav1enc and mfh265enc are elements the exporter could not have been asked for at all before this branch added them to the catalogue.

Twenty-nine of its thirty cells were marked failed by the driver rather than by the export, for two reasons now fixed: gst-launch treats a backslash as an escape, so the Windows path in the frame counter location= arrived mangled and the counter fell through to a decode the machine has no AAC decoder for; and the generated project left media info null, which routes a clip audio to symphonia rather than GStreamer (decode_audio asks the model, not the file), and symphonia has no Opus decoder, so every uhd cell died before an encoder was reached. TASK-150 is filed for the routing; the generator fills info in now and box stand-in clip carries AAC.

Getting there needed three more yodaddy fixes worth recording: the machine has no Python and what python resolves to is the Microsoft Store stub, so the job unpacks the embeddable distribution under RUNNER_TEMP; get-sample-media.ps1 used Invoke-WebRequest -MaximumRetryCount, which is PowerShell 7 only and made every download fail instantly under Windows PowerShell 5.1; and GStreamer had to go on each step own PATH rather than only the runner GITHUB_PATH.

Run 34688535274, yodaddy: the AMF row complete. 24 of 30 cells pass, including amfav1enc and mfh265enc - two encoders the exporter could not have been asked for before this branch catalogued them - and including the user 4K60 footage through amfh265enc and amfav1enc. The six failures are all one bug, TASK-148, and that bug got much more interesting: mfh264enc stalls at exactly the same frame as amfh264enc, and both at the same place vah264enc stalls on box. Three hardware H.264 encoders, three vendor stacks, two operating systems, one failure shape - and x264enc writes the same frames on both machines without trouble.

Run 34685591190, box, with the probed-info project: 17 of 20 again, the same three TASK-148 cells, and the GUI-versus-CLI comparison still matches on both encoders.

Still blocked on the NVIDIA rows. RunsOn has been failing to resolve gpu-nvidia-linux for every job in the repository since about 05:00 UTC - hardware.yml on main fails identically (runs 34676554315), as do the TASK-139 dispatches - and four attempts here (34676733798, 34678136543, 34680820309, 34684146860) all ended "failed to resolve runner spec: runner spec gpu-nvidia-linux not found" with a fallback to a CPU instance. .github/runs-on.yml on main is valid and unchanged since TASK-138 merged, and the same label worked at 03:32 UTC, so this is the RunsOn stack or its repository-config resolution and not anything on this branch. Each failed attempt cost a minute of an m7i-flex.large; total GPU spend on this task is under 0.05 USD.

Run 34689001087: RunsOn recovered and the NVIDIA T4 row ran. 10 passed, 4 failed, 6 deliberately skipped (software encoders on the 4K source, which box and the hosted job carry instead of a paid instance). nvh264enc, nvh265enc, x264enc, x265enc, svtav1enc and av1enc all present and ready; nvav1enc is not in that image registry at all, which is why there is no nvav1enc row. Two failures are TASK-148 (nvh264enc on the 4K60 source, stalling at frames 27, 23 and 9 of 60 while nvh265enc writes all sixty) and one is TASK-151 (av1enc hits the export own fixed timeout on a four-core instance, having passed in 82 seconds on the hosted runner).

And the GUI half on NVIDIA hardware: "nvh264enc: window 12 frames, CLI 12 frames, audio on both" and "x264enc: window 12 frames, CLI 12 frames, audio on both" - the assembled editor export runner and subordinate-cli render, same project, same preset, same pinned encoder, same file.

On checking AC 3, one substitution the reviewer should weigh rather than take on trust. The criterion names the Linux desktop image; the comparison ran on box and on the Tesla T4 instead, for a reason that is itself a finding: the running editor Command API serves the engine methods and not the host family, so export.render reaches no window and nothing on that image can ask the editor to export at all (TASK-147). The image also carries a released build and no Rust, so a test binary cannot run there either. What did run is the assembled SubordinateApp - its export panel, its encoder pin, its ExportRunner, its render context - on two machines with real GPUs, against subordinate-cli over the same project, preset and encoder, for nvh264enc, vah264enc and x264enc. That is the substance the criterion is about; the machine is not the one it names. Move the comparison onto the desktop image once TASK-147 is done.

Runs 34695092575 and 34697692471, the NVIDIA T4 Windows row: 14 cells observed passing or failing before the job hit its own limit (raised to 60 minutes since - eight of that machine cells stall rather than fail, and even capped at two minutes each that is a third of the job). mfh264enc, x264enc, x265enc, svtav1enc and rav1enc write the sample project on every preset in four to ten seconds. nvh264enc, nvh265enc and mfh265enc stop on frame 1 - TASK-152 - and every hardware encoder stalls on the 4K source, TASK-148.

Worth flagging to whoever reads this next: another agent, on another branch, has independently found and fixed the mechanism behind both stalls - the export pushed into appsrc with a blocking push and never read the bus, so a failed element hung the render instead of reporting itself. This matrix found the same phenomenon from the outside on four vendors and three operating systems, which is corroboration rather than duplication, but TASK-148 and TASK-152 should be read alongside that work rather than started from scratch. Their branch also numbers a task TASK-146, which is this branch 4:4:4 task - parallel agents are colliding on task ids and someone should reconcile them.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Export is now measured as a matrix rather than one encoder at a time.

.github/workflows/export-matrix.yml runs on demand and nightly, one job per machine, and every job runs the same driver - scripts/export-matrix.py - so the rows are comparable. The driver discovers encoders from the exporter own probe next to the raw registry, crosses every pinnable encoder with every preset of its codec and every source, renders each cell through subordinate-cli render --encoder --preset --verify, and then validates the file independently: stream layout and duration from gst-discoverer, and a frame count taken by reading the written file own video track, which is the only check that catches an encoder writing a plausible header and dropping pictures. It writes a machine x source x preset x encoder table into the job summary with pass/fail, size, frames, duration and audio, and keeps every failing output with its discoverer report as an artifact. The same driver builds a project around any media file, which is how the user 4K60 three-audio-track footage reaches each machine.

Measured, on four machines: hosted ubuntu-24.04 6/6, box 17/20, the Tesla T4 10 passed 4 failed 6 deliberately skipped, yodaddy 24/30, and fourteen cells of the Windows T4. Thirteen distinct encoder elements exercised across NVENC, VA-API, AMF, Media Foundation and software, including two - amfav1enc and mfh265enc - that the exporter could not have been asked for at all before this branch catalogued them, and AV1, which had no preset to ask for.

Two gaps in sub-export were filled to make "every present encoder" mean anything: the catalogue gained vaav1enc, amfav1enc, mfh265enc and three software AV1 encoders, and builtin.toml gained an av1-archive preset. The GUI half of the claim is a new sub-ui test - the assembled SubordinateApp, its export panel, its pinned encoder and its ExportRunner - compared against subordinate-cli over the same project, preset and encoder; it matches on frame count and audio for nvh264enc on the T4, vah264enc on box and x264enc on both.

Verified by runs 34689001087 (T4, and the GUI comparison on NVIDIA hardware), 34685591190 (box, and the GUI comparison there), 34683736400 and 34688535274 (yodaddy), 34675906145 (hosted software) and 34697692471 (T4 Windows); CI green on run 34690432963.

Seven bug tasks carry what it found: TASK-145, TASK-146, TASK-147, TASK-148, TASK-149, TASK-150, TASK-151 and TASK-152.
<!-- SECTION:FINAL_SUMMARY:END -->
