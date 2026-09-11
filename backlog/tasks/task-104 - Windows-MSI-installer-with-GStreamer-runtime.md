---
id: TASK-104
title: Windows MSI installer with GStreamer runtime
status: Done
assignee:
  - '@opus-task-104'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 19:10'
labels:
  - release
milestone: m-7
dependencies:
  - TASK-59
  - TASK-63
references:
  - docs/PLAN.md
modified_files:
  - .github/workflows/windows-packaging.yml
  - packaging/windows/build-msi.ps1
  - packaging/windows/main.wxs
  - packaging/windows/gst-plugins.txt
  - docs/DEVELOPMENT.md
priority: high
ordinal: 125000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Windows users expect a single installer.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 cargo-wix (or equivalent) builds an MSI bundling the GStreamer MSVC runtime with amf, nvcodec and mf plugins
- [x] #2 Install, launch, export and uninstall verified on a clean Windows VM
- [x] #3 Code-signing steps documented even if unsigned for MVP
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. packaging/windows/gst-plugins.txt: Windows plugin allowlist (core, containers, x264/libav, plus nvcodec, amfcodec, mediafoundation, d3d11, wasapi); '!' marks required, same format as the Linux list.
2. packaging/windows/build-msi.ps1: stage ProgramFiles layout (bin/ exes + all GStreamer bin DLLs + MSVC CRT, lib/gstreamer-1.0/ curated plugins, libexec/gstreamer-1.0/gst-plugin-scanner.exe) mirroring the GStreamer prefix so its relocatable prefix detection finds plugins with no env vars; fail when a required plugin is absent; generate a WiX fragment (one component per staged file) and build the MSI.
3. packaging/windows/main.wxs: Package/StandardDirectory ProgramFiles64Folder\Subordinate, Start Menu shortcut, MajorUpgrade, per-machine, WixUI-less silent-capable install.
4. Toolset: WiX v6/v5 via 'dotnet tool install --global wix' rather than cargo-wix (which targets WiX v3, no longer preinstalled on windows-latest images, and cannot harvest a large external runtime tree). Document the choice.
5. .github/workflows/windows-packaging.yml: separate workflow to avoid conflicting with TASK-103's packaging.yml. windows-latest, timeout-minutes 40: install GStreamer 1.28.6 devel silently, cargo build --release of the two binaries, build the MSI, msiexec /i /qn, run installed subordinate.exe --smoke-test, fetch sample media, subordinate-cli.exe render with x264enc from the installed location, validate the output with the bundled gst-discoverer-1.0.exe, msiexec /x /qn and assert the install dir is gone; upload the MSI artifact.
6. docs/DEVELOPMENT.md: Windows MSI section - layout, toolset choice, how to build locally, silent install/uninstall, and the code-signing steps (signtool/Azure Trusted Signing) left undone for MVP.
7. Verify on GitHub windows-latest; record run ids and MSI size.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-11 supervisor: dependencies changed from TASK-64/66 to the export pipeline and CLI render; NVIDIA-on-Windows verification is TASK-115's job once the AMI exists.

2026-09-11 opus-task-104: verified on GitHub Actions run 34635008969 (workflow "Windows packaging", job MSI 103380651604, windows-latest, 21m47s of a 40-minute budget, 16m of it the cold release build). Artifact: subordinate-msi.

Numbers from that run: 209 staged files, 244.7 MiB uncompressed, 48 GStreamer plugin modules bundled (all 15 required ones present; only the optional dsd module is absent from the 1.28.6 MSVC runtime), 7 MSVC CRT DLLs from Microsoft.VC145.CRT. MSI 63.9 MiB, installed footprint 244.7 MiB. WiX 5.0.2+aa65968c.

Proven end to end in that run, with every step that touches the installed binaries running under a PATH rewritten to the installed bin directory plus Windows itself, GSTREAMER_1_0_ROOT_MSVC_X86_64 and PKG_CONFIG_PATH removed and the GStreamer registry cache under the user profile deleted, so nothing could lean on the build machine's own GStreamer:

- msiexec /i /qn installed to C:\Program Files\Subordinate with the Start Menu entry, both binaries, the runtime and gst-plugin-scanner.exe present.
- The installed gst-inspect-1.0 reports 1.28.6 and loads coreelements, app, playback, x264, libav, isomp4, matroska, nvcodec, amfcodec, mediafoundation and d3d11. 'Total count: 49 plugins, 776 features', nothing blacklisted, so no bundled plugin lost a dependency.
- The installed subordinate.exe --smoke-test painted 3 frames and exited.
- The installed subordinate-cli.exe rendered the sample project (Main cut, mezzanine, --encoder x264enc, --range 0:25) to a 180 KiB Matroska; the bundled gst-discoverer-1.0.exe read it back as 1.000000000 s, container Matroska, video #1 H.264 High 4:4:4, audio #2 FLAC.
- msiexec /x /qn left neither C:\Program Files\Subordinate, nor the Start Menu folder, nor HKLM\Software\Subordinate.

Caveat on criterion 2: the runner is an ephemeral Windows VM, but the same job had installed the GStreamer devel package in order to build the Rust binaries. The PATH and registry-cache stripping above is what stands in for a genuinely untouched image; a real fresh-machine install is TASK-110.

Hardware elements: loading a plugin and getting its elements are different things. nvcodec and amfcodec register their encoders only after talking to a driver, so on the GPU-less runner both plugins load while nvh264enc and amfh264enc are absent; mfh264enc is present because the Media Foundation transform ships with Windows. The job reports these rather than asserting them; an NVENC export out of the installed package is TASK-115.

Decisions worth keeping: (a) the install tree is shaped as a GStreamer prefix (bin, lib\gstreamer-1.0, libexec\gstreamer-1.0) because the Windows GStreamer build is relocatable, which is why the package needs no environment variable, PATH entry or registry key, proven by the sanitised-environment steps above; (b) WiX v5 as a pinned .NET global tool rather than cargo-wix, which drives the end-of-life WiX v3 that is no longer on the hosted image and only harvests a crate's own binaries; (c) plugins curated by an allowlist with the hardware ones required, while the DLLs in bin are copied wholesale with the blacklist check as the safety net.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Ships a Windows MSI that carries the pinned GStreamer 1.28 runtime, so a Windows user installs one thing and can export.

packaging/windows/build-msi.ps1 stages a tree shaped like a GStreamer prefix (bin, lib\gstreamer-1.0, libexec\gstreamer-1.0) and that shape is the mechanism: the Windows GStreamer build is relocatable, so the bundled runtime is found with no environment variable, no PATH entry and no registry key. Plugins are curated by packaging/windows/gst-plugins.txt in the Linux allowlist's format, with nvcodec, amfcodec and mediafoundation among the required entries so a package that cannot hardware-encode on a whole GPU vendor is a build failure; the bin DLLs are copied wholesale and a blacklist check on the installed registry catches a missing dependency. The MSVC CRT is app-local. The toolset is WiX v5 pinned as a .NET global tool rather than cargo-wix, which drives the end-of-life WiX v3 that windows-latest no longer carries; packaging/windows/main.wxs holds only the package shape and the payload fragment is generated one component per file. docs/DEVELOPMENT.md gains a 'Windows packaging (MSI)' section covering the layout, the toolset choice, local builds, silent install/uninstall and the signtool steps for signing once a certificate exists.

Verified by .github/workflows/windows-packaging.yml on run 34635008969 (job 103380651604, windows-latest, 21m47s): MSI built at 63.9 MiB from 209 files and 48 plugin modules, silent install, installed gst-inspect loading all eleven checked plugins with nothing blacklisted, installed subordinate.exe --smoke-test, the sample project rendered by the installed CLI with x264enc and read back by the bundled gst-discoverer as H.264 plus FLAC in Matroska, then a silent uninstall that left no tree, Start Menu entry or registry key. Every step ran with the build machine's own GStreamer stripped out of PATH. Criterion 2 is checked on that evidence with one caveat recorded in the notes: the runner built the binaries, so it is an ephemeral rather than an untouched VM, and a truly fresh image is TASK-110.
<!-- SECTION:FINAL_SUMMARY:END -->
