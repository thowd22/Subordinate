---
id: TASK-19
title: wgpu device and eframe integration with a shared render context
status: In Progress
assignee:
  - '@opus-task-19'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-08 22:54'
labels:
  - render
  - ui
milestone: m-1
dependencies:
  - TASK-1.1
references:
  - docs/PLAN.md
priority: high
ordinal: 40000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The compositor and UI share one wgpu device so preview textures need no copies (§3 GUI decision).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 eframe launches with the wgpu backend and exposes device, queue and adapter info to sub-render
- [x] #2 Adapter selection prefers a discrete GPU and logs the backend (Vulkan, D3D12, Metal)
- [ ] #3 App starts and renders an empty window on all three OSes in CI using a software adapter where needed
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add wgpu 30 to sub-render; add eframe 0.36 (wgpu backend, no glow) to sub-ui; wire bins/subordinate to sub-ui.
2. sub-render: RenderContext holding Arc<Device>, Arc<Queue> and wgpu AdapterInfo, built either from eframe's shared RenderState or headlessly; RenderError with stable string codes (folds into SubError when TASK-11 lands).
3. sub-render: pure, testable adapter ranking that prefers DiscreteGpu > Integrated > Virtual > Cpu/software, plus a backend label used for logging.
4. sub-ui: eframe launch with WgpuConfiguration using that adapter selector; App captures device/queue/adapter info into a RenderContext shared with the compositor; log the chosen backend and adapter on startup.
5. Smoke mode (--smoke-test / SUB_SMOKE_FRAMES) that renders N empty frames then closes, so CI can start the app headlessly with a software adapter (lavapipe on Linux via xvfb, WARP on Windows, Metal on macOS).
6. Tests: adapter-ranking unit tests, error-code stability test, headless RenderContext + one empty render pass integration test that skips when no adapter exists. CI: add mesa-vulkan-drivers/xvfb + GUI smoke step.
7. Verify with cargo fmt --check, clippy -D warnings, cargo test.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implementation

- sub-render gained three modules. `error` defines RenderError with the stable codes render.no_adapter, render.device_request_failed and render.missing_render_state (folds into SubError when TASK-11 lands). `adapter` holds the pure selection rule: AdapterRank orders Discrete > Integrated > Virtual > Other > Software, best_adapter_index picks the first highest-ranked adapter, and backend_label spells backends as PLAN.md does (Vulkan, D3D12, Metal). `context` holds RenderContext, a cheap-to-clone handle on the shared wgpu Device, Queue and AdapterInfo, built either from eframe's RenderState or headlessly (RenderContext::headless, used by tests and later by export/CLI).
- sub-ui now runs eframe 0.36 on its wgpu backend. SubordinateApp::new lifts device/queue/adapter out of cc.wgpu_render_state into a RenderContext and logs the backend and adapter; eframe's native_adapter_selector is wired to sub_render::select_adapter so the UI and headless paths use one rule. AppOptions::smoke_frames (--smoke-test, or SUB_SMOKE_FRAMES) paints N empty frames then sends ViewportCommand::Close, which is how CI starts the real app without a human.
- bins/subordinate initialises env_logger (temporary; TASK-11 replaces it with tracing) and forwards --smoke-test.
- CI: added a Linux step installing mesa-vulkan-drivers/libvulkan1/xvfb/libxkbcommon-x11-0, then a GUI smoke-test step on all three OSes (xvfb-run on Linux, direct elsewhere).

Toolchain bump (please review)

egui/eframe 0.36 - the version PLAN.md §3 mandates - declares rust-version 1.95, and eframe 0.35 would drag wgpu back to 29, losing the wgpu 30 NV12 helpers PLAN.md §3 requires for TASK-20. Staying on both plan-of-record versions therefore forced rust-toolchain.toml, the workspace rust-version and the CI toolchain from 1.93.1 to 1.95.0, with docs/DEVELOPMENT.md updated to say why. This is the one change outside the task's own crates.

Verification

- cargo fmt --all --check: clean.
- cargo clippy --workspace --all-targets -- -D warnings: clean.
- cargo test --workspace: all pass, including 9 sub-render unit tests (ranking, labels, error codes), 3 headless-context integration tests (adapter reported, empty frame cleared with no validation error, a cloned context drives the same device) and 2 sub-ui tests.
- RUST_LOG=info cargo run -p subordinate -- --smoke-test on this Linux box: window opened, logged 'adapter chosen: llvmpipe ... (software, Vulkan, ...)' and 'render device ready on Vulkan', painted 3 frames, closed, exit 0. That is AC #3 on Linux with a software adapter.

AC #3 left unchecked: the Windows and macOS halves can only be proven by a CI run, which cannot be started from this environment. The workflow steps are in place; check it once CI is green on all three runners.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the shared wgpu render context: sub-render now owns RenderContext (shared Device/Queue/AdapterInfo, plus a headless constructor), a pure adapter-ranking rule that prefers a discrete GPU and falls back to software, and RenderError with stable codes; sub-ui runs eframe 0.36 on the wgpu backend, lifts eframe's RenderState into that context, logs the chosen backend and adapter, and gained a --smoke-test mode that paints a few empty frames and exits so CI can start the app headlessly. Verified with cargo fmt --check, clippy -D warnings and cargo test --workspace (14 new tests, including a headless empty-frame render), and by running the app with --smoke-test on a software Vulkan adapter (llvmpipe): it opened a window, logged the Vulkan backend and exited 0. AC #3 stays unchecked because only its Linux half could be proven here; the Windows and macOS smoke steps are wired into CI but need a CI run. Note that keeping eframe 0.36 and wgpu 30 (both plan of record) required bumping the pinned toolchain to 1.95.0.
<!-- SECTION:FINAL_SUMMARY:END -->
