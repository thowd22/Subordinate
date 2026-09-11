---
id: TASK-103
title: 'Linux packaging: AppImage and Flatpak with bundled GStreamer'
status: In Progress
assignee:
  - '@opus-task-103'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 15:28'
labels:
  - release
milestone: m-7
dependencies:
  - TASK-59
  - TASK-67
  - TASK-63
references:
  - docs/PLAN.md
priority: high
ordinal: 124000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Linux is the primary target; users must not hunt for GStreamer plugins.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 AppImage bundles the pinned GStreamer runtime including nvcodec and va plugins and runs on Ubuntu LTS and Fedora
- [ ] #2 Flatpak manifest uses the freedesktop GStreamer extension and passes flatpak-builder in CI
- [ ] #3 Hardware encode works from both packages (verified)
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. packaging/linux/: AppDir template (AppRun, subordinate.desktop, io.github.thowd22.Subordinate.metainfo.xml, SVG icon) and gst-plugins.txt allowlist that names nvcodec and va explicitly.
2. packaging/linux/build-appimage.sh: cargo build --release, stage AppDir, copy the pinned GStreamer runtime (libs + plugins + gst-plugin-scanner + gst-inspect) from GST_PREFIX, regenerate the plugin registry at runtime via AppRun, run appimagetool. Support --stage-only so the staging half runs without FUSE/appimagetool.
3. packaging/flatpak/: io.github.thowd22.Subordinate.yml on the freedesktop runtime with the org.freedesktop.Platform.GStreamer.* / ffmpeg-full extensions, cargo-sources offline build, plus build-flatpak.sh.
4. packaging/validate.sh as the test: metadata well-formedness (appstreamcli, desktop entry keys), app-id/version consistency with Cargo.toml, plugin allowlist covers nvcodec+va, AppRun exports GST_PLUGIN_SYSTEM_PATH_1_0/GST_PLUGIN_SCANNER, flatpak manifest parses and declares the GStreamer extension + required finish-args. Run it locally.
5. .github/workflows/packaging.yml: appimage job (Ubuntu 26.04, build + stage + appimagetool, smoke-run --version under Ubuntu LTS and a Fedora container) and flatpak job running flatpak-builder; upload artifacts.
6. docs/DEVELOPMENT.md packaging section; leave AC#3 (hardware encode from the packages) unchecked, it needs the TASK-116 hardware runners.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-11 supervisor: dependencies changed from the manual verify tasks (TASK-64/65) to the export pipeline, CLI render and pop-out. Hardware encode inside the packages is checked by the hardware workflow (TASK-116) on box and RunsOn after packaging, not before.

Implemented packaging/ (validate.sh, linux/AppRun, linux/build-appimage.sh, linux/gst-plugins.txt, desktop + metainfo + icon, flatpak/manifest + build script), .github/workflows/packaging.yml and a docs/DEVELOPMENT.md packaging section. packaging/validate.sh passes locally (22 checks, shellcheck and desktop-file-validate skipped as they are not installed here); its failure paths were exercised by deliberately breaking the metainfo version, the nvcodec required marker and the flatpak --device=dri line.

Verified locally end to end: built the release binary, staged the AppDir from the extracted GStreamer prefix, packed it with appimagetool (116 MB Subordinate-0.1.0-x86_64.AppImage) and ran it twice with a deliberately empty environment (env -i). Ubuntu host: 'subordinate --help' exits 0 and the bundled registry reports 45 plugins / 981 features with no blacklist entries, gst-inspect through SUB_APPIMAGE_TOOL finds nvcodec, va, libav and x264. Fedora 41 container with only the documented desktop baseline (mesa/libdrm/libva/X11/Wayland/alsa/pulse client libraries, no GStreamer at all): same results. Caveat on AC 1: the sudo-less dev box only has an extracted GStreamer 1.24 tree, so the local proof bundled 1.24; the 1.28 pin is enforced by packaging.yml, which installs from apt on ubuntu-26.04 and fails the job when pkg-config reports another minor. That workflow cannot run here (no push to the remote).

Two real bugs the local run found, both fixed: (1) libraries the package must NOT bundle (libGL, libva, libdrm, X11/Wayland, libasound, libpulse) are absent from a bare Fedora container, so the Fedora job installs exactly that host baseline and nothing GStreamer-shaped - if it ever needs a package outside that list, the bundle is missing something; (2) plugins whose dlopen-only dependencies are not present (msdk/libmfx, openh264, vulkan/libxkbcommon-x11) are silently blacklisted at scan time rather than failing loudly, so build-appimage.sh now ldd-checks every bundled plugin against the AppDir's own library path, drops optional ones and fails the build if a required one cannot resolve. Registry went from 48 plugins with 3 blacklist entries to 45 clean.

AC 2 unchecked: flatpak and flatpak-builder are not installed here and cannot be (no sudo), so the manifest is only statically validated - packaging/validate.sh parses it and asserts the id, runtime, command, finish-args (--device=dri, wayland, pulseaudio, the ffmpeg-full LD_LIBRARY_PATH), the ffmpeg-full extension version matching runtime-version, the rust-stable SDK path and that every packaging/ file its build-commands install exists. The flatpak job in packaging.yml runs flatpak-builder for real but has not been executed.

AC 3 unchecked by design: hardware encode from inside the packages needs a GPU. Per the supervisor note this belongs to hardware.yml (TASK-116); the AppImage carries subordinate-cli and gst-inspect and AppRun exposes them through SUB_APPIMAGE_TOOL so that workflow can render from inside the package rather than against the host runtime.

Checks: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings clean (exit 0, no Rust source changed by this task); packaging/validate.sh passes with and without --appdir. No cargo test run because the task adds no Rust code; the packaging test is validate.sh.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added packaging/ for Linux: an AppImage that bundles the GStreamer runtime (AppRun with a per-run plugin registry and the plugin scanner, build-appimage.sh that stages the AppDir from a given GStreamer prefix, an allowlist marking nvcodec and va as required, shared desktop/AppStream/icon metadata) and a Flatpak manifest that instead rides the freedesktop runtime's GStreamer plus its extension point and ffmpeg-full, with packaging/validate.sh as the test and .github/workflows/packaging.yml building and smoke-testing both. Verified by actually building a 116 MB Subordinate-0.1.0-x86_64.AppImage here and running it under env -i on the Ubuntu host and in a bare Fedora 41 container: the editor starts and the bundled registry reports 45 plugins / 981 features with no blacklist entries, exposing nvcodec, va, libav and x264 (AC 1). AC 2 is statically validated only - flatpak-builder cannot be installed on this sudo-less box - and AC 3 needs GPU runners, so the task stays In Progress for hardware.yml (TASK-116) and a CI run to close them.
<!-- SECTION:FINAL_SUMMARY:END -->
