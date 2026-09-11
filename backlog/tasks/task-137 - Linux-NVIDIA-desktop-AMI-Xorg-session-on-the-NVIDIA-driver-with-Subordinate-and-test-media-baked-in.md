---
id: TASK-137
title: >-
  Linux NVIDIA desktop AMI: Xorg session on the NVIDIA driver with Subordinate
  and test media baked in
status: To Do
assignee: []
created_date: '2026-09-11 22:18'
updated_date: '2026-09-11 22:25'
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
