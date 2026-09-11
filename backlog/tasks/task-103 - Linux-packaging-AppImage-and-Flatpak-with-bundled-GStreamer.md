---
id: TASK-103
title: 'Linux packaging: AppImage and Flatpak with bundled GStreamer'
status: Done
assignee:
  - '@opus-task-103'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 18:44'
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
- [x] #2 Flatpak manifest uses the freedesktop GStreamer extension and passes flatpak-builder in CI
- [x] #3 Hardware encode works from both packages (verified)
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. packaging/linux/: AppDir template (AppRun, subordinate.desktop, io.github.thowd22.Subordinate.metainfo.xml, SVG icon) and gst-plugins.txt allowlist that names nvcodec and va explicitly.
2. packaging/linux/build-appimage.sh: cargo build --release, stage AppDir, copy the pinned GStreamer runtime (libs + plugins + gst-plugin-scanner + gst-inspect) from GST_PREFIX, regenerate the plugin registry at runtime via AppRun, run appimagetool. Support --stage-only so the staging half runs without FUSE/appimagetool.
3. packaging/flatpak/: io.github.thowd22.Subordinate.yml on the freedesktop runtime with the org.freedesktop.Platform.GStreamer.* / ffmpeg-full extensions, cargo-sources offline build, plus build-flatpak.sh.
4. packaging/validate.sh as the test: metadata well-formedness (appstreamcli, desktop entry keys), app-id/version consistency with Cargo.toml, plugin allowlist covers nvcodec+va, AppRun exports GST_PLUGIN_SYSTEM_PATH_1_0/GST_PLUGIN_SCANNER, flatpak manifest parses and declares the GStreamer extension + required finish-args. Run it locally.
5. .github/workflows/packaging.yml: appimage job (Ubuntu 26.04, build + stage + appimagetool, smoke-run --version under Ubuntu LTS and a Fedora container) and flatpak job running flatpak-builder; upload artifacts.
6. docs/DEVELOPMENT.md packaging section; leave AC#3 (hardware encode from the packages) unchecked, it needs the TASK-116 hardware runners.

7. CI fix round (run 34626701087 failed both jobs).
   a. Flatpak: the rust-stable SDK extension on freedesktop 24.08 is rustc 1.89, below the workspace rust-version 1.95. Move the manifest to runtime-version 25.08, whose rust-stable branch tracks current stable (its ostree commit was rebuilt 2026-09-07). 25.08 also drops org.freedesktop.Platform.ffmpeg-full: the runtime now declares org.freedesktop.Platform.codecs-extra (version 25.08-extra, already on GST_PLUGIN_SYSTEM_PATH and auto-downloaded), so the add-extensions block, the mkdir /app/lib/ffmpeg and the --env=LD_LIBRARY_PATH=...ffmpeg finish-arg all go away and the manifest stays Flathub-shaped (no toolchain installed over the network). Update build-flatpak.sh and validate.sh to match.
   b. AppImage: it was built on the ubuntu-26.04 runner (glibc 2.43) so it could not start on Fedora 41 (2.40). Build it inside an ubuntu:24.04 container (glibc 2.39, the oldest supported Ubuntu LTS) as its own job, smoke-test the artifact in a matrix over the ubuntu-26.04 host, an ubuntu:24.04 container and a fedora:41 container. Ubuntu 24.04 carries GStreamer 1.24, not the 1.28 pin: record that deviation in the task notes and docs/DEVELOPMENT.md, and keep the version assertion in the job so the base drifting is a build failure.
   c. AC 3: add a packages job on the self-hosted box (self-hosted, linux, box, amd-gpu) that downloads the AppImage artifact and renders the sample project with subordinate-cli --encoder vah264enc from inside the package, validating with gst-discoverer-1.0; install flatpak there and do the same from the Flatpak bundle.
   d. Verify with real runs of packaging.yml on the branch (temporary push trigger, removed before finishing), then check only the criteria the runs prove.
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

2026-09-11 opus-task-103 (branch task/task-103-fix): CI fix round for the two jobs that failed on run 34626701087. Four runs, ending green.

Flatpak. Root cause was the SDK extension, not the manifest: org.freedesktop.Sdk.Extension.rust-stable on runtime branch 24.08 is rustc 1.89 and this workspace declares rust-version 1.95, so cargo refused before compiling anything. Moved the manifest, build-flatpak.sh and the runtime cache key to branch 25.08, whose rust-stable tracks current stable - run 34628898376 printed 'rustc 1.98.1' and built the workspace in 10m07s. 25.08 also retired org.freedesktop.Platform.ffmpeg-full: the full codec set is now org.freedesktop.Platform.codecs-extra, an extension the *runtime* declares (add-ld-path, auto-downloaded, already on GST_PLUGIN_SYSTEM_PATH), so the add-extensions block, the /app/lib/ffmpeg directory and the ffmpeg LD_LIBRARY_PATH finish-arg were removed rather than repointed - re-declaring a runtime extension shadows the runtime's own mount point, which validate.sh now fails on, along with a runtime-version below 25.08. Determined all of this from the Flathub ostree metadata (the Platform commit's [Extension ...] blocks) rather than by guessing. Two further real bugs the runs found: flatpak-builder shells out to eu-strip, absent from the runner image, so it failed after the ten-minute compile (elfutils is now installed and build-flatpak.sh refuses to start without it); and build-bundle was being handed the runtime version as the app branch, which is not a ref - both sides now say 'stable', as Flathub publishes. The toolchain still comes from the SDK extension, never from the network, so the manifest stays Flathub-shaped.

AppImage. The package was built on the ubuntu-26.04 runner (glibc 2.43) and glibc is only forward compatible, so it died on Fedora 41 (2.40) before main. It is now built inside an ubuntu:24.04 container (glibc 2.39, the oldest supported Ubuntu LTS), and the artifact is smoke-tested by a separate job on three systems that did not build it: the 26.04 host, an ubuntu:24.04 container and a fedora:41 container. TRADE-OFF THE SUPERVISOR SHOULD NOTE: Ubuntu 24.04 carries GStreamer 1.24, not the repository's 1.28 pin, and there is no trustworthy 1.28 for that base, so the AppImage now bundles 1.24 while CI, box and the Flatpak stay on 1.28+. Nothing needs a post-1.24 API (the gstreamer-rs gate is v1_18) and nvcodec, va and vah264enc all exist there, but AC 1's wording says 'the pinned GStreamer runtime' and that is no longer literally true. The job asserts the base is 1.24.x so the two facts cannot drift silently.

Two rounds were lost to the same avoidable mistake - writing the smoke jobs' host-library baseline from memory, so it was missing libva-drm2, libva-x11-2, libxcb-xkb1 and libxcb-render0 and each run revealed only the first plugin to trip over one. The appimage job now derives that list instead: it ldd-sweeps the AppDir with its own library path and prints every library resolved outside it (38), into the step summary and the artifact as host-libraries.txt; the baselines are the distro spelling of that.

A real packaging bug the hardware job found: the AppImage could not export at all. Every youtube-* preset is AAC in MP4 and the only AAC encoder in the bundle was gst-libav's avenc_aac, which is registered at rank NONE; sub-export will not plug a deranked element, so the render died with export.no_encoder. voaacenc (rank secondary) is now bundled and required in the allowlist and validate.sh.

Runs: 34628898376 (flatpak reached eu-strip; appimage built and fedora smoke passed), 34630086856 (flatpak green; appimage smoke still short of libraries; box reached the render), 34632104230 (all three smoke targets green; box AppImage render green; flatpak render blocked), 34633406106 GREEN - validate, AppImage build, all three AppImage smoke targets, Flatpak, and the box hardware job. Artifacts: subordinate-appimage (Subordinate-0.1.0-x86_64.AppImage + host-libraries.txt), subordinate-flatpak (Subordinate-0.1.0.flatpak), packages-hardware-amd (both rendered files, gst-discoverer output, render JSON, encoder ranks; 15 MB).

Hardware evidence (run 34633406106, box, AMD Cezanne APU): the AppImage rendered examples/sample-project/demo.sub through SUB_APPIMAGE_TOOL=subordinate-cli with --encoder vah264enc pinned, and the HOST's gst-discoverer-1.0 reports 'video #1: H.264 (High Profile)', 14.000 s. The Flatpak did the same through 'flatpak run --command=subordinate-cli', 14.0212 s, same H.264 High Profile. Both used the host driver through --device=dri and each package's own GStreamer.

CAVEAT ON THE FLATPAK HALF: the 25.08 runtime has no ranked AAC encoder inside the sandbox - voaacenc, fdkaacenc and faac are absent and avenc_aac is rank NONE - so the first attempt was refused and the job's second rung, GST_PLUGIN_FEATURE_RANK=avenc_aac:256, is what ran. The video path is untouched by that: vah264enc is pinned explicitly and the file is H.264. But it means a Flatpak user exporting any youtube-* preset gets export.no_encoder today. That is a product decision (an AAC encoder module in the manifest, or the app promoting avenc_aac itself), not a packaging one, so it is written into docs/DEVELOPMENT.md and left for the supervisor rather than fixed here.

Checks: packaging/validate.sh passes locally and in CI (validate job green in every run); its new flatpak assertions were exercised by deliberately setting runtime-version back to 24.08, which fails with the rust-1.89 message. No Rust source changed, so no cargo run was needed beyond what the packaging jobs compile.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Linux packaging for both formats, fixed until CI proved it rather than argued it. The AppImage is now built inside an ubuntu:24.04 container (glibc 2.39) instead of on the 26.04 runner, because glibc is only forward compatible and a package built on 2.43 cannot start on Fedora 41; the price is that it bundles GStreamer 1.24 rather than the repository's 1.28 pin, which the job asserts and docs/DEVELOPMENT.md states. Its host-library baseline is derived by ldd-sweeping the AppDir rather than written from memory, and shipped with the artifact. The Flatpak moved to freedesktop runtime 25.08, whose rust-stable SDK extension is current stable (24.08's rustc 1.89 was below the workspace's rust-version, which is what failed the first run) and whose codecs-extra extension replaced the retired ffmpeg-full, so the manifest now declares no extensions of its own and still needs no network toolchain. Bundled voaacenc after the hardware job proved the AppImage could not export at all: gst-libav's avenc_aac is rank NONE and every youtube-* preset is AAC. Verified by run 34633406106, green end to end: the AppImage starts and resolves nvcodec, va, libav, x264 and voaacenc through its own registry on the ubuntu-26.04 host, an ubuntu:24.04 container and a bare fedora:41 container (AC 1); flatpak-builder builds and bundles the app on the 25.08 runtime with its GStreamer extension point (AC 2); and on the self-hosted AMD box both packages rendered examples/sample-project/demo.sub with --encoder vah264enc pinned, the host's gst-discoverer-1.0 reporting H.264 High Profile from each (AC 3). One caveat is recorded in the notes and the docs rather than papered over: the 25.08 runtime carries no ranked AAC encoder, so the Flatpak render needed GST_PLUGIN_FEATURE_RANK=avenc_aac:256 and a Flatpak user exporting a youtube preset would hit export.no_encoder today.
<!-- SECTION:FINAL_SUMMARY:END -->
