---
id: TASK-1.4
title: Add licence files and per-crate licence metadata
status: Done
assignee:
  - '@claude'
created_date: '2026-09-08 21:04'
updated_date: '2026-09-08 21:16'
labels:
  - infra
milestone: m-0
dependencies:
  - TASK-1.1
references:
  - docs/PLAN.md
parent_task_id: TASK-1
priority: medium
ordinal: 13000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Licensing was settled in decision-2: core GPL-3.0-or-later, SDK and WIT MIT OR Apache-2.0. Getting the metadata right before external contributors arrive avoids relicensing pain.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 LICENSE (GPL-3.0-or-later) at the repo root; LICENSE-MIT and LICENSE-APACHE under sdk/ and wit/
- [x] #2 Every core crate Cargo.toml declares license = "GPL-3.0-or-later"; SDK crates declare "MIT OR Apache-2.0"
- [x] #3 README states the licence split in one paragraph
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Fetch canonical GPL-3.0-or-later, MIT and Apache-2.0 texts into LICENSE, sdk/LICENSE-MIT, sdk/LICENSE-APACHE, wit/LICENSE-MIT, wit/LICENSE-APACHE
2. Core crates already inherit license = GPL-3.0-or-later via workspace; add sdk/ placeholder crate with MIT OR Apache-2.0 and wit/ README
3. README paragraph on licence split
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Licence texts fetched from gnu.org and apache.org (canonical). Core crates inherit license via [workspace.package]; added sdk/subordinate-sdk placeholder crate with license = "MIT OR Apache-2.0" and wit/README.md. Verified with cargo metadata: 13 core packages report GPL-3.0-or-later, subordinate-sdk reports MIT OR Apache-2.0.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added GPL-3.0-or-later at root, MIT and Apache-2.0 under sdk/ and wit/, an SDK placeholder crate under the permissive licence, and a README licence paragraph. Verified via file presence and cargo metadata licence fields.
<!-- SECTION:FINAL_SUMMARY:END -->
