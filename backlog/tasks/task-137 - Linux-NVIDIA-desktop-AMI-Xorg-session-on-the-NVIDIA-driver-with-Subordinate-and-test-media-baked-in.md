---
id: TASK-137
title: >-
  Linux NVIDIA desktop AMI: Xorg session on the NVIDIA driver with Subordinate
  and test media baked in
status: In Progress
assignee:
  - '@opus-task-137'
created_date: '2026-09-11 22:18'
updated_date: '2026-09-11 22:46'
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
- [ ] #1 infra/images/linux-desktop/ holds a committed Image Builder recipe (or Packer template) and a workflow that builds the AMI and records its id in .github/runs-on.yml as runner gpu-nvidia-desktop-linux
- [ ] #2 A smoke job on that runner starts the app on the Xorg display, captures a screenshot showing the editor window rendered on the NVIDIA adapter (title bar reports a non-software Vulkan adapter), and uploads it
- [ ] #3 The test MP4 is present on the image at a documented path and probes correctly with gst-discoverer
- [ ] #4 subordinate-mcp from the same release is installed on the image and a job proves an MCP call (project.new then timeline.get_state) round-trips against the running app while its window is visible
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
