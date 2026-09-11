---
id: TASK-106
title: Release workflow producing all packages from a tag
status: In Progress
assignee:
  - '@opus-task-106'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 19:13'
labels:
  - release
  - infra
milestone: m-7
dependencies:
  - TASK-103
  - TASK-104
references:
  - docs/PLAN.md
priority: medium
ordinal: 127000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Repeatable releases.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Tagging vX.Y.Z builds AppImage, Flatpak bundle, MSI and dmg and attaches them to a GitHub release
- [ ] #2 Checksums are published
- [ ] #3 A dry-run mode builds without publishing
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Base the branch on origin/main, which already carries TASK-103 (packaging.yml) and TASK-104 (windows-packaging.yml).
2. Make both packaging workflows reusable: add a workflow_call trigger to packaging.yml (with a string input 'hardware' so a release can skip the self-hosted AMD box job) and to windows-packaging.yml. Remove their own push:tags:['v*'] triggers so a tag produces exactly one run, the release run, instead of three overlapping ones.
3. Add .github/workflows/release.yml: triggered by push tags v* and by workflow_dispatch with a dry_run boolean (default true). It calls packaging.yml (AppImage, Flatpak, skipping packages-amd) and windows-packaging.yml (MSI), then a publish job downloads the three artifacts, checks the versions embedded in the filenames agree with each other and with the tag, writes SHA256SUMS, uploads it as an artifact, and only on a real v* tag push with dry_run false creates the GitHub release for exactly that tag with the packages and SHA256SUMS attached. Everything else prints what would be published.
4. Leave a clearly commented, disabled macOS dmg job in release.yml showing where TASK-105 slots in (a 'macos' caller plus two lines in the publish job).
5. Guard the existing sample-media-v1 release: the workflow only ever names github.ref_name, refuses a ref that is not a v* tag, and uses 'gh release create' (never edit/upload against an existing release).
6. Verify the dry run end to end from task/task-106 with a temporary push trigger; confirm all three packages and a correct SHA256SUMS appear as artifacts; remove the temporary trigger.
7. Add a 'Releasing' section to docs/DEVELOPMENT.md: how to cut a release, what the dry run does, where the dmg slots in.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-11 supervisor: the macOS dmg (TASK-105) is back-burnered until the user's M1 Mac mini arrives; the release workflow ships AppImage/Flatpak and MSI now and gains the dmg later.
<!-- SECTION:NOTES:END -->
