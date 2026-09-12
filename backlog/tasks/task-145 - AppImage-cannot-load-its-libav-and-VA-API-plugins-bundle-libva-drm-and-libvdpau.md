---
id: TASK-145
title: >-
  AppImage cannot load its libav and VA-API plugins: bundle libva-drm and
  libvdpau
status: In Progress
assignee:
  - '@opus-task-145'
created_date: '2026-09-12 10:02'
updated_date: '2026-09-12 10:10'
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Reproduce: stage the AppDir in an ubuntu:24.04 container and gst-inspect every bundled plugin in a container carrying only the documented host contract. Done locally with podman: libav, va, qsv and msdk are the four failures, and the unresolved sonames across the bundle are exactly libva.so.2, libva-drm.so.2, libva-x11.so.2 and libvdpau.so.1 (libavcodec/format/util/filter, libswscale, libswresample and libpostproc all link them).
2. Bundle those four as *fallbacks*, not as ordinary bundled libraries. libva is a dispatcher that dlopens the host's VA driver and matches it by a version-stamped init symbol, so an AppImage-owned libva ahead of the host's would break VA-API on any host whose Mesa was built against a newer libva -- which is why build-appimage.sh excludes it today. So: packaging/linux/fallback-libs.txt lists the sonames; build-appimage.sh stages them in usr/lib/fallback/ (never usr/lib); AppRun links, per soname and per run, only the ones the host does not have into a shim directory it puts on LD_LIBRARY_PATH. Host wins when present, bundle answers when absent.
3. Bundle gst-launch-1.0 alongside gst-inspect-1.0/gst-discoverer-1.0 so a package can synthesise media with its own runtime.
4. New packaging/linux/check-plugins.sh: run gst-inspect-1.0 through the package on every bundled plugin, fail on the first load error (with GST_DEBUG=GST_PLUGIN_LOADING:4 for the report) and fail on any blacklist entry. packaging.yml's appimage-smoke matrix runs it on the ubuntu-26.04 host, a ubuntu:24.04 container and a fedora:41 container; validate.sh --appdir checks the fallback staging and lints the new script.
5. Remove libva2/libva-drm2/libva-x11-2/libvdpau1 (and Fedora's libva/libvdpau) from every container baseline in packaging.yml and fresh-install.yml, so those jobs prove the package no longer needs them.
6. fresh-install-check.sh: synthesise an H.264/AAC MKV with the bundled gst-launch-1.0 and probe it twice through the package -- the bundled gst-discoverer-1.0 and media.probe over the MCP bridge, which is the exact call TASK-139 saw answer media.unsupported.
7. Remove the apt workaround from desktop-flows.yml's linux-clicks job and rewrite the comment; update docs/DEVELOPMENT.md.
8. Keep the glibc floor unchanged (the fallback libraries are copied from the same ubuntu:24.04 build base as everything else) and verify with real packaging.yml and fresh-install.yml runs from the branch.
<!-- SECTION:PLAN:END -->
