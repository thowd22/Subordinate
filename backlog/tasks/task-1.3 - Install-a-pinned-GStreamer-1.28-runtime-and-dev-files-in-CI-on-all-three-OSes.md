---
id: TASK-1.3
title: Install a pinned GStreamer 1.28 runtime and dev files in CI on all three OSes
status: Done
assignee:
  - '@claude'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-08 21:54'
labels:
  - infra
  - media
milestone: m-0
dependencies:
  - TASK-1.2
references:
  - docs/PLAN.md
parent_task_id: TASK-1
priority: high
ordinal: 12000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Bundling and building against GStreamer is the top packaging risk in the plan (§9). Pinning the version in CI from day one surfaces install problems immediately and gives every later media task a known environment.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 CI installs the same pinned GStreamer minor version on Linux (apt or official tarball), Windows (official MSVC installer) and macOS (official pkg or Homebrew pinned)
- [x] #2 gst-inspect-1.0 runs in CI and the log lists the available encoder elements per OS
- [x] #3 PKG_CONFIG_PATH and PATH are set so a crate depending on gstreamer builds on all three runners
- [x] #4 docs/DEVELOPMENT.md documents the local install steps per OS matching the CI versions
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Research prebuilt GStreamer 1.28 availability per runner: Ubuntu 26.04 apt 1.28.x; official Windows Inno installer 1.28.6 (MSIs dropped in 1.28); official macOS pkgs 1.28.6
2. ci.yml: GST_VERSION/GST_MINOR env pins; Linux apt on ubuntu-26.04 with minor-version assert; Windows silent installer to C:\gstreamer with explicit env vars; macOS runtime+devel pkg; PKG_CONFIG_PATH and PATH exported via GITHUB_ENV/GITHUB_PATH
3. Diagnostics step: gst-inspect version plus presence of every hardware encoder element
4. Real dependency: sub-media depends on gstreamer 0.25 with an init() smoke test so CI links and runs GStreamer
5. docs/DEVELOPMENT.md with matching local install steps per OS
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Linux link verified locally without sudo by extracting Ubuntu 24.04 dev debs (gstreamer, glib, libdw chain) into a scratch prefix with PKG_CONFIG_SYSROOT_DIR: cargo build/test -p sub-media passes and gstreamer::init() reports 1.24.2. actionlint 1.7.12 clean (ubuntu-26.04 label declared in .github/actionlint.yaml since the bundled list predates it). Windows and macOS install steps and the CI diagnostics step are authored from the official download docs but have not been executed: the repo has no GitHub remote yet, so no Actions run has been observed. blinemedical/setup-gstreamer was rejected: archived Feb 2026 and only knows the old MSI layout.

docs/DEVELOPMENT.md written with per-OS install steps matching CI (1.28 minor, 1.28.6 official binaries), env vars, hardware plugin availability and the check commands.

Verified by GitHub Actions run https://github.com/thowd22/Subordinate/actions/runs/34281067519 (all three jobs green): Linux apt installed 1.28.2, Windows official installer 1.28.6, macOS official pkgs 1.28.6; the diagnostics step printed gst-inspect version and per-encoder presence (Linux/mac: x264enc,x265enc; Windows additionally mfh264enc; mac vtenc_h264; hardware vendor elements absent on GPU-less runners as expected); sub-media compiled against pkg-config and its gstreamer::init() test passed on all three OSes. First run exposed two platform issues, now fixed: Windows CRLF checkout broke rustfmt (.gitattributes eol=lf) and macOS @rpath dylibs (.cargo/config.toml rpath).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
CI installs GStreamer 1.28 on all three OSes (Ubuntu 26.04 apt 1.28.2, official Windows installer and macOS pkgs 1.28.6), exports PKG_CONFIG_PATH and PATH, prints gst-inspect diagnostics, and sub-media links gstreamer 0.25 with an init test. Verified by green Actions runs on all three OSes; docs/DEVELOPMENT.md documents matching local setup. Fixed along the way: CRLF checkout on Windows (.gitattributes) and macOS @rpath dylibs (.cargo/config.toml).
<!-- SECTION:FINAL_SUMMARY:END -->
