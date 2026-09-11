---
id: TASK-106
title: Release workflow producing all packages from a tag
status: Done
assignee: []
created_date: '2026-09-08 21:05'
updated_date: '2026-09-11 20:46'
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
- [x] #1 Checksums are published
- [x] #2 A dry-run mode builds without publishing
- [ ] #3 Tagging vX.Y.Z builds the AppImage, Flatpak bundle and MSI and attaches them to a GitHub release (the macOS dmg joins when TASK-105 lands)
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

2026-09-11 dry run verified: run 34638014639 on task/task-106 (Release workflow, push), 20m42s, green.

Shape: release.yml calls packaging.yml and windows-packaging.yml as reusable workflows (workflow_call). Both lost their own 'push: tags: [v*]' trigger in exchange, so a tag starts exactly one run and the packages that were smoke tested are the packages published, instead of three runs racing to build the same thing. packaging.yml gained a 'hardware' input so the release skips packages-amd: box is a mini PC in the user's house and a release must neither wait on it nor fail because it is off. The input is a string, not a boolean, because for non-workflow_call events the inputs context is empty and GitHub compares an empty string to boolean false numerically, so a '!= false' guard would have skipped the job on every push too.

Evidence from the run. The publish job received all three packages: Subordinate-0.1.0-x86_64.AppImage (117,340,664 bytes), Subordinate-0.1.0.flatpak (15,267,672), Subordinate-0.1.0-x86_64.msi (67,022,848), plus the AppImage job's host-libraries.txt, which the collect step correctly excluded from dist/. All three agreed on version 0.1.0. SHA256SUMS was written and re-read with 'sha256sum -c' (three OK lines). The job printed 'DRY RUN -- no release was created (ref task/task-106 is a branch, not a tag)' and uploaded dist/ as the subordinate-release-0.1.0 artifact, 198,780,599 bytes. Inside the called workflows: validate, AppImage, Flatpak and all three AppImage smoke targets passed, the MSI install/render/uninstall round trip passed, and packages-amd was skipped as intended.

Checksums checked from outside the run, not just against themselves: downloading the separately uploaded subordinate-flatpak artifact and hashing it locally gives c700f186a07c27ab62c506e7b722deb9af5308fe66f3e11fa0fa47d6ae449329, exactly the value in SHA256SUMS.

sample-media-v1 untouched: 'gh release list' after the run still shows only 'Sample media v1 (CC0)' dated 2026-09-11T00:15:19Z. Structurally it cannot be touched -- the only release name the workflow mentions is github.ref_name, it refuses a ref that is not a v* tag, and the only verb is 'gh release create', which fails rather than overwriting. There is no release edit, upload or delete anywhere in it.

Temporary 'push: branches: [task/task-106]' trigger added for the verification and removed in 36e891e; pushing that removal started no new run.

Not proven, deliberately: AC 1 and AC 2 both depend on 'gh release create' actually running, and no tag was pushed -- the supervisor's instruction was to verify the dry run and never publish a real release or tag. AC 1 additionally names the dmg, which is TASK-105 and is deferred until the user's Mac arrives; release.yml carries a commented macos job and a three-step note (uncomment the job, add it to publish's needs, add '*.dmg' to the required-package list) so wiring it in is mechanical.

2026-09-11 supervisor: merged to main. Criterion 1 reworded to the three packages that exist now; criteria 1 and 2 will be proven by the first real v* tag, which the user must approve since it publishes a public release. Dry run 34638014639 produced all three packages plus SHA256SUMS. Note the design change: packaging.yml and windows-packaging.yml no longer trigger on tags; release.yml owns v* tags and calls them, so the published artifacts are the smoke-tested ones.

2026-09-11 supervisor verification: tag v0.1.0 (commit 14bba40) triggered release run 34644113031; all package jobs and smoke tests passed, Publish created https://github.com/thowd22/Subordinate/releases/tag/v0.1.0 with Subordinate-0.1.0-x86_64.AppImage (113 MB), Subordinate-0.1.0.flatpak (15 MB), Subordinate-0.1.0-x86_64.msi (64 MB) and SHA256SUMS. Marked as a pre-release by hand (the workflow has no prerelease flag). Known gap in this build: the AppImage predates the TASK-110 discoverer-tool fix.
<!-- SECTION:NOTES:END -->

## Comments

<!-- COMMENTS:BEGIN -->
author: @opus-task-106
created: 2026-09-11 19:40
---
AC 1 and AC 2 are left unchecked on purpose and need a decision. Both turn on 'gh release create' having run, and this task was verified in dry-run mode only (no tag pushed, no release published, per the brief). AC 1 also names the dmg, which is TASK-105 and deferred. Two ways to close them: (a) reword AC 1 to the three packages that exist now and let TASK-105 add the dmg, then prove 1 and 2 with the first real v0.x tag; or (b) leave both open until TASK-105 lands and the first release is cut. Everything the workflow does before the release call is verified in run 34638014639.
---
<!-- COMMENTS:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
release.yml builds all packages from a v* tag through the packaging workflows, checks versions and checksums, and publishes a GitHub release; dry-run mode verified in run 34638014639 and the real path by v0.1.0 (run 34644113031). dmg slot reserved for TASK-105.
<!-- SECTION:FINAL_SUMMARY:END -->
