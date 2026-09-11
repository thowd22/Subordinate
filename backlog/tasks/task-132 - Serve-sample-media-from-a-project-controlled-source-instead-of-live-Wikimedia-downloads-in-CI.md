---
id: TASK-132
title: >-
  Serve sample media from a project-controlled source instead of live Wikimedia
  downloads in CI
status: To Do
assignee: []
created_date: '2026-09-11 00:06'
updated_date: '2026-09-11 00:15'
labels:
  - infra
  - ci
  - docs
milestone: m-7
dependencies: []
priority: medium
ordinal: 152000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-109 fetches CC0 sample media from Wikimedia Commons during CI. Concurrent runs from shared runner egress hit HTTP 429 rate limits (four straight failures on Windows in run 34538636034); the CI guard added exponential retries, but a third-party site remains a single point of failure for every CI run and slows Windows further. The media should come from a source the project controls and be cached on runners.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Sample media files are attached to a GitHub release (or stored under a project-owned bucket) with their CC0 attribution recorded in docs, and the fetch script downloads from there with checksum verification
- [ ] #2 CI caches the downloaded media keyed on the fetch script so a warm run downloads nothing
- [ ] #3 Wikimedia is no longer contacted during CI; the original source URLs remain documented for provenance
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-10 supervisor: the three CC0 files are published as GitHub release assets on tag sample-media-v1 (https://github.com/thowd22/Subordinate/releases/tag/sample-media-v1) together with SHA256SUMS, manifest.json and provenance notes. Asset URLs follow https://github.com/thowd22/Subordinate/releases/download/sample-media-v1/<file> for crowned-pigeon.webm, porters-paris-1921.webm and soneros-en-xalapa.webm; SHA-256 values are unchanged from the scripts. Remaining work is in scripts/get-sample-media.sh and .ps1 (switch url to the release asset, keep source for provenance, verify against SHA256SUMS) plus an actions/cache step keyed on the script hash. Dependency on TASK-109 cleared so this can proceed.
<!-- SECTION:NOTES:END -->
