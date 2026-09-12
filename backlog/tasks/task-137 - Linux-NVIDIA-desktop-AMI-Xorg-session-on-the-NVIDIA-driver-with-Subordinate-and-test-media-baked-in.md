---
id: TASK-137
title: >-
  Linux NVIDIA desktop AMI: Xorg session on the NVIDIA driver with Subordinate
  and test media baked in
status: Done
assignee:
  - '@opus-task-137'
created_date: '2026-09-11 22:18'
updated_date: '2026-09-12 01:20'
labels:
  - infra
  - gpu
  - ui
  - test
milestone: m-8
dependencies:
  - TASK-115
  - TASK-118
  - TASK-140
priority: high
ordinal: 157000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Human-style desktop testing needs a real window session on a real GPU. Build an AMI with EC2 Image Builder from RunsOn's ubuntu24-gpu-x64 image: Xorg on the NVIDIA driver with a virtual 1920x1080 screen (nvidia-xconfig --virtual, or a headless EDID), a lightweight window manager, xdotool, scrot or ImageMagick, GStreamer 1.28, the latest Subordinate AppImage from the GitHub release installed, and the user's test MP4 copied from a private S3 bucket (path recorded in the task, never committed). The RunsOn agent must remain in the image so the runner starts in a session that can see the display. Rebuilding for a new release is a workflow_dispatch run of the Image Builder pipeline.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 infra/images/linux-desktop/ holds a committed Image Builder recipe (or Packer template) and a workflow that builds the AMI and records its id in .github/runs-on.yml as runner gpu-nvidia-desktop-linux
- [x] #2 A smoke job on that runner starts the app on the Xorg display, captures a screenshot showing the editor window rendered on the NVIDIA adapter (title bar reports a non-software Vulkan adapter), and uploads it
- [x] #3 The test MP4 is present on the image at a documented path and probes correctly with gst-discoverer
- [x] #4 subordinate-mcp from the same release is installed on the image and a job proves an MCP call (project.new then timeline.get_state) round-trips against the running app while its window is visible
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. infra/images/linux-desktop/: EC2 Image Builder component (component.yaml), CloudFormation stack (stack.yaml) with component + recipe + infrastructure config (g4dn.xlarge, so the NVIDIA driver and Xorg are validated on a real GPU) + distribution config (us-east-1) + pipeline, and deploy.sh that renders the component into the template and runs `aws cloudformation deploy`.
2. Component contents, on top of RunsOn's runs-on-v2.2-ubuntu24-gpu-x64-* base (owner 135269210855), leaving the runner user, the Actions runner and /usr/local/bin/runs-on-bootstrap-* untouched:
   - Xorg (xserver-xorg-core, xinit, xauth), openbox, xdotool, x11-utils (xdpyinfo/xwininfo), x11-xserver-utils, scrot, imagemagick, x11-apps;
   - /usr/local/bin/subordinate-xorg-setup writes /etc/X11/xorg.conf at boot from the live PCI bus id (nvidia-smi --query-gpu=pci.bus_id) with UseDisplayDevice none, ConnectedMonitor DFP-0 and Virtual 1920 1080, then a shared MIT cookie at /run/subordinate/Xauthority;
   - systemd units subordinate-xorg.service (Xorg :0, WantedBy multi-user.target) and subordinate-wm.service (openbox as runner); /etc/profile.d exports DISPLAY=:0 and XAUTHORITY;
   - latest v* release AppImage extracted to /opt/subordinate/app (no FUSE needed) with /usr/local/bin/subordinate, subordinate-cli and subordinate-gst-discoverer wrappers over AppRun/SUB_APPIMAGE_TOOL - the bundled runtime is the pinned GStreamer 1.28, which Ubuntu 24.04 apt cannot supply;
   - subordinate-mcp: taken from the AppImage when the release carries it, else built from the release tag with a throwaway rustup toolchain (its dep tree is pure Rust) into /opt/subordinate/bin;
   - s3://subordinate-test-media-731537225673/meld-4k60-excerpt-2min.mkv to /opt/subordinate/test-media/, sha256-verified; Image Builder instance profile gets a read policy for that bucket;
   - test phase on an instance booted from the new AMI: nvidia-smi, xdpyinfo -display :0, subordinate --smoke-test, gst-discoverer on the test media.
3. Also add subordinate-mcp to packaging/linux/build-appimage.sh so future releases ship it.
4. Register the AMI in .github/runs-on.yml as images.subordinate-desktop-linux + runner gpu-nvidia-desktop-linux, and add a nvidia-desktop-linux job to gpu-smoke.yml: nvidia-smi, xdpyinfo, launch `subordinate --ui-smoke --hold-seconds N sample-project.sub` on :0, wait for the `ui-smoke ready` line, xdotool search --name, scrot of :0 uploaded as an artifact, assert the `render device ready on <backend>` log line names a non-software adapter, gst-discoverer the baked MKV, and an MCP round-trip (project.new then timeline.get_state over stdio JSON-RPC to subordinate-mcp) while the window is up.
5. Deploy the stack, run the pipeline in the foreground, record the AMI id, then verify by dispatching gpu-smoke.yml from the branch with the inline label form runs-on=<run_id>/image=<ami-id>/family=g4dn.xlarge/spot=false (named runners resolve only from main).
6. Document the runner, the rebuild runbook and the cost in docs/DEVELOPMENT.md; remove the temporary push trigger before finishing.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Image build

Image Builder pipeline `subordinate-linux-desktop` (CloudFormation stack of the same name, us-east-1), built on RunsOn's `ami-005e36ab413bd2f30` (`runs-on-v2.2-ubuntu24-gpu-x64-20260821141648`, owner 135269210855) - confirmed as what `ubuntu24-gpu-x64` resolves to from `RUNS_ON_AMI_ID` in gpu-smoke run 34643890462, not guessed.

Four things the driver and the tooling taught us, all now recorded in `infra/images/linux-desktop/component.yaml`:

1. Ubuntu keeps `nvidia_drv.so` outside Xorg's default module path (`/usr/lib/x86_64-linux-gnu/nvidia/xorg`), reached through a `ModulePath` the driver package's own `xorg.conf.d` snippet adds. The component finds it at build time and `subordinate-xorg-setup` writes a matching `Files` section.
2. The 580 driver on a T4 refuses `UseDisplayDevice "None"`: *"not supported with virtual display"*, then *"no screens found"*. The headless screen is `AllowEmptyInitialConfiguration` plus `Virtual 1920 1080` and nothing else.
3. `After=multi-user.target` on the Xorg unit plus `WantedBy=multi-user.target` on both units is an ordering cycle; systemd breaks it by dropping a job, and the window manager came up `inactive` on a fresh boot while Xorg itself was fine. The Xorg unit is now ordered after `systemd-user-sessions.service`.
4. `xdpyinfo | grep -q 1920x1080` is a check that passes and then reports failure - grep leaves on the match, xdpyinfo takes SIGPIPE, `pipefail` turns that into exit 141. It failed the image's test phase on an instance whose display was demonstrably correct.

The build starts the session on the build instance itself (`XorgStartsOnThisInstance`) rather than waiting for the test phase, because a bad `xorg.conf` is the likeliest failure and finding it there costs seconds instead of the half hour a snapshot plus test boot takes.

`subordinate-mcp` is not in the v0.1.x AppImage, so the component builds it from the release tag with a throwaway rustup toolchain (its tree is pure Rust - sub-command, sub-edit, sub-model, sub-time, sub-core, rmcp, tokio - about two minutes). `packaging/linux/build-appimage.sh` now bundles it, so a future image will take it from the package; the component prefers the bundled one when it is there.

## Verification

Final image: **ami-05c99b3a15c9d2fef** (`subordinate-linux-desktop-2026-09-12T00-33-26.135Z`), component `1.0.56639`, image build version `arn:aws:imagebuilder:us-east-1:731537225673:image/subordinate-linux-desktop-desktop/1.0.56639/2`, state AVAILABLE - so the image's own test phase passed on a fresh instance booted from it (Xorg and openbox active at boot, 1920x1080 depth 24 on :0, `subordinate --smoke-test` paints, the baked clip probes). Carries release **v0.1.1**.

GPU smoke run **34663975398**, job `nvidia-desktop-linux` on `ami=ami-05c99b3a15c9d2fef/family=g4dn.xlarge/spot=false` - green, every step. Artifact `desktop-smoke-9b1a75e35876bbecad1d4425c79719432af04d41`:

- `app.log`: `render device ready on Vulkan: Tesla T4 (discrete GPU, Vulkan, driver NVIDIA 580.173.02)` and `ui-smoke ready: frames=3 popout=true popout_frames=1 project=loaded sequences=2 tracks=3`.
- `windows.txt`: two real windows on :0 - `Subordinate` 1024x768 at (1,14) and `Subordinate viewer` 960x540 at (929,534), both found through `xdotool search --name`.
- `display0.png`: the editor with its bar reading *"Subordinate  Vulkan - Tesla T4 (discrete GPU, Vulkan, driver NVIDIA 580.173.02)"*, the media bin, viewer, inspector and the timeline with the sample project's clips, and the pop-out viewer bottom-right.
- `probe.txt`: `/opt/subordinate/test-media/meld-4k60-excerpt-2min.mkv` through the AppImage's bundled GStreamer 1.28 - Matroska, H.264 High Profile, 3840x2160, 60/1, 0:02:00.459. (It also reports the MPEG-4 AAC decoder as a missing plugin - the same AAC gap TASK-103 recorded for the packages; the video stream, which is what this proves, is fine.)
- `mcp.txt`: `project.new` -> revision 1, `sequence.create` -> revision 2, `timeline.get_state` -> the sequence that was just made, 1920x1080 at 24000/1001, 48 kHz, Rec.709 - all over the socket while the editor window was up on :0.

Earlier runs are part of the record: 34661255850 (`image=` refuses a raw AMI id - the key is `ami=`), 34661369585 (first green desktop job; `timeline.get_state` on an empty project correctly refuses), 34663392571 (on the sample project it correctly refuses again, two sequences).

## Cost

About **0.80 USD** all in, against a 3 USD cap: roughly 75 minutes of `g4dn.xlarge` on-demand across five pipeline executions (0.526 USD/h, ~0.66 USD) plus four smoke runs (~0.11 USD) and a few cents of snapshot. The first, broken image (ami-0812b474041b378f3, window manager inactive at boot) has been deregistered and its snapshot deleted; ami-02168a4d57f6fa24c is kept as the previous good image.

Three pipeline executions failed with `VcpuLimitExceeded` and cost nothing - the account's on-demand G quota is 8 vCPU, exactly two `g4dn.xlarge`, and TASK-138's Windows image was building at the same time. `deploy.sh --run --wait` treats that as a retry, not a failure.

## One thing the supervisor has to decide

`.github/workflows/desktop-ami.yml` is committed and complete, but it cannot run yet: the repository has no AWS credentials and no OIDC provider, so it fails fast on a missing `AWS_IMAGEBUILDER_ROLE_ARN` repository variable. `infra/ci-oidc/stack.yaml` is the template that would create the identity - a GitHub OIDC provider scoped to `repo:thowd22/Subordinate:*` and one role allowed only to start `subordinate-*` image pipelines and read image state, nothing else. Deploying it gives a GitHub workflow an identity inside the AWS account, which is a call for the user rather than a side effect of this task, so it is committed and unapplied. To turn it on:

```bash
aws cloudformation deploy --region us-east-1 --stack-name subordinate-ci-oidc \
  --template-file infra/ci-oidc/stack.yaml --capabilities CAPABILITY_NAMED_IAM \
  --parameter-overrides Repository=thowd22/Subordinate
gh variable set AWS_IMAGEBUILDER_ROLE_ARN --body "<RoleArn output>"
```

Until then the supported and verified build path is the local runbook, `infra/images/linux-desktop/deploy.sh --run --wait`, which is what produced ami-05c99b3a15c9d2fef.

Also worth knowing:

- **The editor does not serve the Command API endpoint.** `sub-ui` builds a `Dispatcher` but nothing calls `Server::bind`, so the MCP bridge launches `subordinate-cli serve` and speaks to that - the same engine and dispatcher with nothing drawn. The round-trip in the smoke job is real (mutations over the socket, the timeline they produced read back, editor window up on :0 beside it) but it is not the GUI process's own project. Making the editor bind its endpoint would strengthen this job for free and is worth a follow-up task.
- **The G-family vCPU quota is the bottleneck**, not cost. 8 vCPU on-demand is exactly two `g4dn.xlarge`, so an image build and a GPU job collide, and the Linux and Windows desktop images cannot build at the same time. Raising `L-DB2E81BA` to 16 would remove the retries.
- `.github/runs-on.yml` is read from the default branch only, so `runner=gpu-nvidia-desktop-linux` does not resolve until this branch is merged. Until then the runner is reached with `ami=ami-05c99b3a15c9d2fef/family=g4dn.xlarge/spot=false` through the `desktop_runner` input on GPU smoke.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A second Linux GPU runner, `gpu-nvidia-desktop-linux`: the same g4dn.xlarge T4 as `gpu-nvidia-linux`, but booted into a real Xorg session on the NVIDIA driver instead of a bare console, so tests can open the actual window, click in it and photograph it.

`infra/images/linux-desktop/` is the image: `component.yaml` runs on the build instance, `stack.yaml` wires it into an EC2 Image Builder component, recipe, infrastructure configuration, distribution configuration and pipeline, and `deploy.sh` is the only piece that needs AWS credentials. It builds on RunsOn's own `ubuntu24-gpu-x64` base, leaving the runner user, the Actions agent and the RunsOn bootstrap untouched (the validate phase fails the build if any has gone), and adds: headless Xorg at :0 with one virtual 1920x1080 screen started at boot by systemd, openbox as the window manager, xdotool/xdpyinfo/xwininfo/scrot/ImageMagick, the newest release's AppImage with `subordinate`, `subordinate-cli` and its bundled GStreamer 1.28 on PATH, `subordinate-mcp` from the same release, and the 4K60 test clip at `/opt/subordinate/test-media/`, sha256-verified from the private bucket. `packaging/linux/build-appimage.sh` now ships `subordinate-mcp` too, so a future image takes it from the package instead of compiling it.

`gpu-smoke.yml` gains `nvidia-desktop-linux`, which is the image's acceptance test, and an `only` input so one runner can be exercised without paying for all five.

Verified on ami-05c99b3a15c9d2fef (component 1.0.56639, release v0.1.1), whose own Image Builder test phase passed on a fresh instance booted from it, and by GPU smoke run 34663975398: the editor came up on :0 on `Tesla T4 (discrete GPU, Vulkan, driver NVIDIA 580.173.02)` with the sample project loaded, `xdotool` found both the editor and the pop-out window, the screenshot shows the editor's bar naming that adapter, the baked MKV probed as H.264 3840x2160 60/1 through the bundled GStreamer, and an MCP round-trip (`project.new`, `sequence.create`, `timeline.get_state`) came back with the sequence it had just made - all while the window was up. About 0.80 USD against a 3 USD cap.
<!-- SECTION:FINAL_SUMMARY:END -->
