---
id: TASK-3.3
title: Media items and bins with relative paths and content hashes
status: Done
assignee:
  - '@opus-task-3.3'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-09 00:31'
labels:
  - core
milestone: m-0
dependencies:
  - TASK-3.1
references:
  - docs/PLAN.md
parent_task_id: TASK-3
priority: high
ordinal: 21000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Projects move between machines. Relative paths plus a content hash let media be relinked reliably (§5.6).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 MediaItem stores project-relative path, content hash (BLAKE3 of first and last 1 MiB plus size), probed stream info, proxy state, offline flag
- [x] #2 Bins form a tree with a root bin; items reference bins by ID
- [x] #3 Helper resolves absolute paths from the project location and reports offline items
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add blake3 dep to sub-model; new module media: ContentHash (BLAKE3 of first 1 MiB + last 1 MiB + size, hex string) with hash_file helper and SubError codes.
2. Add MediaPath newtype for project-relative, forward-slash-normalised paths with validation (rejects absolute/parent-escaping paths); resolve(project_dir) -> PathBuf.
3. Extend MediaItem: path (MediaPath), hash (Option<ContentHash>), info (Option<MediaInfo> stream summary: video/audio stream descriptions, duration RationalTime, no floats), proxy (ProxyState enum), offline flag.
4. Bin tree: keep root bin, add find_mut, insert/remove helpers as needed; ensure items reference bins by ID (Project::bin_of(media)).
5. Project helper: media_path(item) absolute resolution from project location (Project::location/path field), refresh_offline / offline_items() listing offline media.
6. Tests for hashing (small file < 2 MiB, large file > 2 MiB overlap-free), path validation and resolution, bin tree, offline reporting.
7. Verify: cargo fmt --check, clippy -D warnings workspace, cargo test -p sub-model.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Added crates/sub-model/src/content.rs: MediaPath (validated project-relative, slash-separated path; rejects absolute, drive-prefixed, empty, '.'/'..' and NUL components; relative_to/resolve helpers) and ContentHash (BLAKE3 over the file length plus the first and last 1 MiB, hex Display/parse, stable codes model.invalid_path / model.invalid_hash / model.file_unreadable). MediaItem now stores path, hash, probed StreamInfo (video/audio streams, duration as RationalTime, exact Rational frame rate and sample aspect - no floats), ProxyState and an offline flag; Bin gained find_mut, bin_of, remove_media and iter, and Project gained media_item_mut, bin_of, absolute_path, refresh_offline and offline_media. Filesystem access is confined to MediaItem::refresh_offline and ContentHash::of_file. Validation: cargo fmt --all --check clean; cargo clippy --workspace --all-targets -- -D warnings exit 0 (with the scratchpad GStreamer env for sub-media); cargo test -p sub-model 36 unit tests + 3 doctests pass, covering hash stability and head/tail/size sensitivity on files larger than 2 MiB, path validation and resolution, bin tree search/removal, and offline detection against real temp directories.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
MediaItem now carries a validated project-relative MediaPath, a BLAKE3 content hash of the file length plus its first and last 1 MiB, probed StreamInfo, ProxyState and an offline flag, with the bin tree unchanged in shape (root bin, media referenced by ID) but gaining lookup, removal and traversal helpers. Project resolves absolute paths from a supplied project folder and reports offline media via refresh_offline/offline_media, the only places the model touches disk. Verified with cargo fmt --all --check, cargo clippy --workspace --all-targets -D warnings (exit 0) and cargo test -p sub-model (36 unit tests, 3 doctests, all passing).
<!-- SECTION:FINAL_SUMMARY:END -->
