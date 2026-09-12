---
id: TASK-145
title: >-
  AppImage cannot load its libav and VA-API plugins: bundle libva-drm and
  libvdpau
status: In Progress
assignee:
  - '@opus-task-145'
created_date: '2026-09-12 10:02'
updated_date: '2026-09-12 15:32'
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
- [x] #1 packaging/validate.sh (or the AppImage build) runs gst-inspect-1.0 on every bundled plugin inside the AppImage on a clean ubuntu:24.04 and fedora container and fails on any load error
- [x] #2 Fresh-install workflow probes an H.264/AAC MKV through the AppImage on both containers without installing extra libraries
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Reproduced locally with podman before changing anything: staged the AppDir in an ubuntu:24.04 container (build-appimage.sh --skip-build --stage-only with stub binaries) and ran gst-inspect-1.0 on every bundled plugin in a container carrying only the documented host contract. libav, va, qsv and msdk fail; the unresolved sonames across the whole bundle are exactly libva.so.2, libva-drm.so.2, libva-x11.so.2 and libvdpau.so.1, named by libavcodec, libavformat, libavutil, libavfilter, libswscale, libswresample, libpostproc and the three hardware plugin modules.

Why they are not simply bundled: libva and libvdpau are dispatchers that dlopen the host driver and bind it by a version-stamped init symbol. The build base is ubuntu:24.04 (libva 2.20) and fedora:41 ships libva 2.22, so a bundled libva in front of the host's would be handed a 2.22 driver and refuse it -- working hardware encode turning into silent software encode, which is exactly what build-appimage.sh's excludelist exists to prevent. So they are staged in usr/lib/fallback (off the library path) and AppRun links only the sonames the host has no answer for into a per-run shim directory. Proved in podman on fedora:41 with libva installed: libva, libva-drm and libva-x11 resolve to /lib64 and only libvdpau.so.1 comes from the bundle.

The audit also found a baseline gap the old smoke list hid: fedora needs libXfixes, which used to arrive as a dependency of the libva package the job installed. Added to the fedora baselines.

Verification (real runs, hosted runners only; packages-amd and the self-hosted fresh-install legs were skipped).

release.yml dry run 34688479672 (branch task/task-145), which calls packaging.yml:
  - validate: pass, including the new fallback-library checks and shellcheck 0.11 on check-plugins.sh.
  - appimage: built in ubuntu:24.04; 'fallback libraries: libva-drm.so.2 libva-x11.so.2 libva.so.2 libvdpau.so.1'; highest glibc symbol version still GLIBC_2.39, unchanged; host-libraries.txt no longer lists any libva or libvdpau and now lists libXfixes.so.3.
  - appimage-smoke on all three targets (ubuntu-26.04 host, ubuntu:24.04, fedora:41, none of them carrying libva2/libva-drm2/libva-x11-2/libvdpau1 any more): check-plugins.sh reports 'checked 49 bundled plugins', 'Total count: 50 plugins, 999 features' with no blacklist entry, 'every bundled plugin loads on this machine'. Before this change the same containers blacklisted libav, va, qsv and msdk.
  - flatpak and the Windows MSI: unaffected, both pass.

fresh-install.yml run 34689459175 against those packages: AppImage on a clean ubuntu:24.04 and a clean fedora:41 both pass. Each wrote an H.264/AAC MKV with the package's own gst-launch-1.0, x264enc and voaacenc (~435 kB), read it back with the bundled gst-discoverer-1.0 ('H.264 (High 4:4:4 Profile)' and 'MPEG-4 AAC'), and probed it through media.probe on the package's own MCP bridge, which answered a MediaInfo carrying audio/mpeg / MPEG-4 AAC rather than media.unsupported -- the exact call TASK-139 watched fail. Flatpak and MSI legs also pass.

Earlier run 34688259287 failed at validate on shellcheck SC3067 ('test -O' is not POSIX sh); fixed in ec402c6, which also makes validate.sh print shellcheck's findings.

Locally (podman, before pushing): staged the AppDir in ubuntu:24.04, ran check-plugins.sh in clean ubuntu:24.04 and fedora:41 containers against both the AppDir and a packed AppImage, and confirmed on fedora:41 with libva installed that the host's libva/libva-drm/libva-x11 are used and only libvdpau.so.1 comes from the bundle.

2026-09-12 supervisor handoff: merged to main; criterion 3 needs release 0.1.4, a Linux desktop AMI rebuild (infra/images/linux-desktop/deploy.sh --run --wait), then desktop-flows.yml with only=linux-clicks.
<!-- SECTION:NOTES:END -->

## Comments

<!-- COMMENTS:BEGIN -->
author: @opus-task-145
created: 2026-09-12 10:51
---
Left In Progress rather than Done: AC3's second half (the Linux clicks flow still passes) cannot be proven from here. The apt workaround is removed, but linux-clicks runs the AppImage baked into the desktop AMI from the newest v* release, so it needs a release carrying this fix and a rebuilt infra/images/linux-desktop before it can go green. Handing that verification to the supervisor on the GPU runner.
---
<!-- COMMENTS:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The AppImage now carries the four VA-API/VDPAU dispatchers gst-libav and the hardware plugins link -- libva.so.2, libva-drm.so.2, libva-x11.so.2 and libvdpau.so.1 -- in usr/lib/fallback, off the library path, with AppRun linking in per run only the sonames the host has no answer for. They cannot go in usr/lib: libva and libvdpau dlopen the host driver and bind it by a version-stamped init symbol, so a bundled libva 2.20 from the ubuntu:24.04 build base in front of Fedora 41's 2.22 driver would refuse it and turn working hardware encode into silent software encode.

packaging/linux/check-plugins.sh is the check the build cannot make -- the build machine's development packages resolve dependencies the package does not carry -- and packaging.yml's appimage-smoke now runs it on the ubuntu-26.04 host, a ubuntu:24.04 container and a fedora:41 container, failing on any plugin that will not load and on any blacklist entry. libva2/libva-drm2/libva-x11-2/libvdpau1 are gone from every container baseline in packaging.yml and fresh-install.yml, which is what lets those jobs catch this again; the audit found fedora also needed libXfixes, previously arriving as a dependency of libva.

The fresh-machine check now writes an H.264/AAC MKV with the package's own gst-launch-1.0, x264enc and voaacenc (the sample media is all WebM, which never loads gst-libav) and reads it back twice: bundled gst-discoverer-1.0 and media.probe on the MCP bridge. The apt workaround is gone from desktop-flows.yml's linux-clicks job.

Verified by release.yml dry run 34688479672 (validate, AppImage, all three smoke targets, Flatpak, MSI; glibc floor still GLIBC_2.39) and fresh-install.yml run 34689459175 (clean ubuntu:24.04 and fedora:41 both pass, media.probe answering a MediaInfo for the MKV). AC3's second half is not proven here: linux-clicks runs the AppImage baked into the desktop AMI from the newest v* release, so it stays red until a release carrying this fix is cut and the image rebuilt -- the supervisor verifies that on the GPU runner.
<!-- SECTION:FINAL_SUMMARY:END -->
