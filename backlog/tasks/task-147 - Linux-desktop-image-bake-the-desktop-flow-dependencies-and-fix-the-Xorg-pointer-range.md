---
id: TASK-147
title: >-
  Linux desktop image: bake the desktop-flow dependencies and fix the Xorg
  pointer range
status: To Do
assignee: []
created_date: '2026-09-12 10:03'
labels:
  - infra
  - gpu
milestone: m-8
dependencies:
  - TASK-137
  - TASK-139
priority: medium
ordinal: 167000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-139's clicks job apt-installs at-spi2-core, python3-pyatspi, xdg-desktop-portal(-gtk), zenity, libva-drm2 and libvdpau1 on every run, and works around an Xorg configuration where the pointer cannot move past x=448 on the 1920-wide virtual screen. Bake the packages into infra/images/linux-desktop and fix the virtual screen/pointer configuration so xdotool can reach the whole display; rebuild and re-run the flows.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Linux desktop AMI rebuilt with the packages; the clicks job no longer runs apt
- [ ] #2 xdotool mousemove 1900 1000 lands at that coordinate on the image (asserted in the smoke job)
<!-- AC:END -->
