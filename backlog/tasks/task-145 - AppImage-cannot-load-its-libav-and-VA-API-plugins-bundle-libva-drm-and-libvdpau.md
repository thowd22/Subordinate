---
id: TASK-145
title: >-
  AppImage cannot load its libav and VA-API plugins: bundle libva-drm and
  libvdpau
status: To Do
assignee: []
created_date: '2026-09-12 10:02'
labels:
  - release
  - bug
milestone: m-7
dependencies:
  - TASK-103
priority: high
ordinal: 165000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On the Linux desktop image (Ubuntu 24.04 base) the v0.1.2/v0.1.3 AppImage's libav, va, qsv and msdk GStreamer plugins fail to load because libva-drm.so.2 and libvdpau.so.1 are not bundled, so media.probe of the user's MKV answers media.unsupported/MissingPlugins; TASK-139's clicks job apt-installs those libraries as a workaround. Add the missing libraries (and any others the AppImage's own plugin-load check reports) to packaging/linux/build-appimage.sh's bundle list, make the build fail when a bundled GStreamer plugin does not load inside the AppImage on a clean container, and verify with the fresh-install workflow (fedora and ubuntu containers must probe an MKV with H.264 and AAC through libav).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 packaging/validate.sh (or the AppImage build) runs gst-inspect-1.0 on every bundled plugin inside the AppImage on a clean ubuntu:24.04 and fedora container and fails on any load error
- [ ] #2 Fresh-install workflow probes an H.264/AAC MKV through the AppImage on both containers without installing extra libraries
- [ ] #3 The desktop flows' apt workaround is removed and the Linux clicks flow still passes
<!-- AC:END -->
