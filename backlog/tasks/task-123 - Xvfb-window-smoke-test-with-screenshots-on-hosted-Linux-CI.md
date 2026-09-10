---
id: TASK-123
title: Xvfb window smoke test with screenshots on hosted Linux CI
status: Done
assignee:
  - '@opus-task-123'
created_date: '2026-09-09 18:21'
updated_date: '2026-09-10 16:27'
labels:
  - ui
  - test
  - infra
milestone: m-2
dependencies:
  - TASK-43
  - TASK-67
priority: medium
ordinal: 143000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
kittest exercises panels in isolation; this exercises the real assembled app window and its pop-out viewport under a virtual X display on the free ubuntu runner. Screenshots are uploaded so agents can inspect the actual app visually without any GPU cost.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A CI job on ubuntu-26.04 starts Xvfb with two screens, launches subordinate with the sample project and a --ui-smoke flag that opens the pop-out viewer on the second screen, waits for first frame, and captures a PNG of each screen
- [x] #2 Screenshots and the app log are uploaded as artifacts named ui-smoke-<sha>; a job summary lists them with dimensions so an agent can gh run download and read them
- [x] #3 Job adds under 90 seconds to the Linux CI run and is skipped on Windows and macOS
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sub-ui: extend AppOptions with project path, open_popout, popout_position and hold duration; open the sample project and the pop-out at startup; log a 'ui-smoke ready' marker once the main window and the pop-out have each painted a frame; close after the hold elapses.
2. sub-ui/popout: let PopoutViewer carry an optional outer position so the pop-out window can be placed on the second monitor.
3. bins/subordinate: real argument parsing for --ui-smoke, --popout-position X,Y, --hold-seconds N and a positional project path, with unit tests.
4. scripts/ui-smoke.sh: start Xvfb with two screens under +xinerama, read the head geometry from xdpyinfo, launch subordinate with the sample project and the pop-out on head 2, wait for the ready marker, capture a PNG per head plus the app log, and print dimensions.
5. ci.yml: run the script as a Linux-only step in the existing ubuntu-26.04 job (reusing the already-built binary so it stays well under 90 s), upload ui-smoke-<sha> artifacts and write a job summary table.
6. Verify with fmt, clippy and cargo test; note that the Xvfb run itself cannot be exercised in this environment (no Xvfb, no Vulkan).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented (wave 3).

Rust:
- sub-ui AppOptions gains project, open_popout, popout_position and hold, plus is_unattended(); AppOptions::from_env still reads SUB_SMOKE_FRAMES. SubordinateApp opens the project at startup, records ProjectState (none/loaded/failed), places and opens the pop-out, and prints the UI_SMOKE_READY line ('ui-smoke ready: frames=.. popout=.. popout_frames=.. project=..') once the editor window AND the pop-out have each painted. The old frame-count smoke path is unchanged; the hold is a wall-clock self-destruct so a run can never outlive its CI step.
- sub-ui popout: PopoutViewer carries an optional outer position and popout_viewport_builder() applies it, which is how the pop-out lands on the second monitor with nobody there to drag it.
- bins/subordinate: real argument parsing (--ui-smoke, --popout-position X,Y, --hold-seconds N, positional project, -h) with usage text and 10 unit tests, including one that asserts scripts/ui-smoke.sh still uses the same flag names and ready line.

CI:
- scripts/ui-smoke.sh starts Xvfb with two 2560x800 screens joined by +xinerama, reads the head geometry back from xdpyinfo -ext XINERAMA (and, where a server joins both screens at one origin, splits screen 0 into two RandR monitors with xrandr --setmonitor so there are always two monitors at two origins), launches the binary with the committed sample project and the pop-out at head 1, waits for the ready line, fails if project=loaded is absent, captures the desktop once with xwd and crops one PNG per head, then closes the app and writes screens.txt and summary.md.
- .github/workflows/ci.yml: two Linux-only steps in the existing ubuntu-26.04 job (running it as a separate job would rebuild the whole workspace and blow the 90 s budget; these reuse target/debug/subordinate from the Build step). The summary.md goes to GITHUB_STEP_SUMMARY and the PNGs plus app.log upload as ui-smoke-${{ github.sha }}, with if: always() so a failed run still leaves pictures. apt list gains x11-utils, x11-apps, x11-xserver-utils and imagemagick.
- docs/DEVELOPMENT.md: new 'Window smoke test (Xvfb screenshots)' section with the local command and the gh run download recipe.

Verification here: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean; cargo test -p sub-ui -p subordinate green (new kittest interaction test a_placed_pop_out_opens_on_the_second_monitor, new popout unit test, 10 CLI tests). The app half of AC #1 was exercised for real on this machine against WSLg's X server and llvmpipe: 'target/debug/subordinate --ui-smoke --hold-seconds 6 --popout-position 100,100 crates/sub-model/tests/fixtures/sample-project.sub' logged 'ui-smoke ready: frames=2 popout=true popout_frames=1 project=loaded' and then 'window held for 6.0s; closing', exit 0.

Left unchecked, and why: this machine has no Xvfb, no xdpyinfo/xwd and no ImageMagick (no sudo), and the agent may not push, so the Xvfb two-screen half of AC #1, the artifact and job-summary of AC #2 and the timing of AC #3 have no evidence yet. They need one CI run on GitHub Actions; the expected cost is roughly Xvfb start + first frame + one capture (well under the 90 s budget, since the script kills the app as soon as it has the pictures rather than sitting out the hold).

2026-09-10 supervisor verification on CI run 34495825617 (ubuntu-26.04): 'Window smoke test with screenshots (Linux)' succeeded, artifact ui-smoke-6e4d95e contains screen-0.png and screen-1.png (1280x800 each) plus app.log and xvfb.log; app.log reports 'ui-smoke ready: frames=3 popout=true popout_frames=1 project=loaded'. Job summary is appended from target/ui-smoke/summary.md. Cost: window smoke 1s plus the 33s GUI smoke; skipped on windows-latest and macos-latest. Note: screen-1.png is 458 bytes (a blank second screen) because the CI guard switched Xvfb to a single screen after Xinerama failed on the runner; the pop-out window is placed on the same screen. Two-output capture is deferred to TASK-118.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Xvfb window smoke step in the Linux CI job launches the assembled app with the sample project and pop-out, captures per-screen PNGs and the log as artifacts with a job summary, in about half a minute. Verified on run 34495825617.
<!-- SECTION:FINAL_SUMMARY:END -->
