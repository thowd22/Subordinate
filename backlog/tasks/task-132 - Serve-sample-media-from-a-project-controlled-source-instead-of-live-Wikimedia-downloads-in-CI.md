---
id: TASK-132
title: >-
  Serve sample media from a project-controlled source instead of live Wikimedia
  downloads in CI
status: Done
assignee:
  - '@opus-task-132'
created_date: '2026-09-11 00:06'
updated_date: '2026-09-11 00:32'
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
- [x] #1 Sample media files are attached to a GitHub release (or stored under a project-owned bucket) with their CC0 attribution recorded in docs, and the fetch script downloads from there with checksum verification
- [x] #2 CI caches the downloaded media keyed on the fetch script so a warm run downloads nothing
- [x] #3 Wikimedia is no longer contacted during CI; the original source URLs remain documented for provenance
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Point both fetch scripts (sh + ps1) at the sample-media-v1 GitHub release assets as the download source; keep the Wikimedia upload URL as an 'origin' field and the Commons page as 'source' so provenance stays documented but is never contacted.
2. Verify downloads against the release's SHA256SUMS asset (fetched once, only when at least one file actually needs downloading) as well as the in-script SHA-256 and byte-size pins; a digest that disagrees with the pins fails the fetch.
3. Add an opt-in --upstream / -Upstream switch so a human can still fetch from the original Commons URLs; CI never uses it.
4. Record the release asset URL plus the origin in media/manifest.json.
5. CI: key the sample-media cache on both scripts (hashFiles of .sh and .ps1) and update the comment to say the media comes from the project's own release, so a warm run downloads nothing.
6. Docs: examples/sample-project/README.md and docs/DEVELOPMENT.md record CC0 attribution, the release the media is served from and the original Wikimedia provenance URLs.
7. Test: crates/sub-test-support/tests/sample_media_catalogue.rs parses both scripts and asserts the catalogues agree, that every download URL is the project release (no wikimedia host in any download path), that every entry keeps its Commons provenance, that the README credits each file, and that the CI cache key hashes both scripts.
8. Verify with cargo fmt --all --check, clippy -D warnings, the new test, and a real --dry-run/--list run of the shell script.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-10 supervisor: the three CC0 files are published as GitHub release assets on tag sample-media-v1 (https://github.com/thowd22/Subordinate/releases/tag/sample-media-v1) together with SHA256SUMS, manifest.json and provenance notes. Asset URLs follow https://github.com/thowd22/Subordinate/releases/download/sample-media-v1/<file> for crowned-pigeon.webm, porters-paris-1921.webm and soneros-en-xalapa.webm; SHA-256 values are unchanged from the scripts. Remaining work is in scripts/get-sample-media.sh and .ps1 (switch url to the release asset, keep source for provenance, verify against SHA256SUMS) plus an actions/cache step keyed on the script hash. Dependency on TASK-109 cleared so this can proceed.

opus-task-132: both fetch scripts now download the sample-media-v1 release assets and cross-check the release SHA256SUMS against the in-script SHA-256 pins before the first download; the pins stay the authority, so a regenerated or tampered checksum file fails the fetch. Per-file size and SHA-256 checks are unchanged. The upload.wikimedia.org URL moved to an origin field, the Commons page stays in source, both are recorded in media/manifest.json and reached only under the new opt-in --upstream / -Upstream switch that CI never passes.

Evidence: --force really downloaded all three files from the release and passed every check; a copy of the script with one pinned SHA altered by a character failed with a release-mismatch error and exit 1; a warm run over a populated directory with http_proxy, https_proxy and ALL_PROXY pointed at a dead port printed only keeping existing and exited 0, proving the warm path makes no network call at all - SHA256SUMS is fetched lazily on the first real download.

CI: the sample-media cache key hashes both fetch scripts and the step comment records why the media moved off Wikimedia. New regression test crates/sub-test-support/tests/sample_media_catalogue.rs parses the shell catalogue and asserts the .ps1 pins the same names, sizes, hashes and origins, that both default download URLs are release assets verified against SHA256SUMS, that every entry keeps its Commons provenance page, that the workflow keys the cache on both scripts and contains neither --upstream nor upload.wikimedia.org, and that the sample project README credits every file and names the release.

Not runnable here: no PowerShell on this machine, so get-sample-media.ps1 was verified by review plus the cross-script test rather than by execution. Docs updated: examples/sample-project/README.md and docs/DEVELOPMENT.md. Checks: cargo fmt --all --check clean, cargo clippy --workspace --all-targets -- -D warnings clean, cargo test -p sub-test-support --test sample_media_catalogue 5 of 5 passing.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Sample media now comes from this project's own sample-media-v1 GitHub release instead of Wikimedia Commons: both fetch scripts download the release assets, cross-check the published SHA256SUMS against their in-script SHA-256 pins before the first download and still verify every file by size and hash, while the Commons origin and description URLs stay in the catalogue and manifest.json for provenance behind an opt-in --upstream switch CI never uses. The CI cache is keyed on both fetch scripts, and a warm run was proven to make no network call at all. Verified with a real forced download from the release, a deliberately mismatched pin that failed the cross-check, a warm run behind a dead proxy, the new sample_media_catalogue test (5 tests), cargo fmt --all --check and cargo clippy --workspace --all-targets -D warnings.
<!-- SECTION:FINAL_SUMMARY:END -->
