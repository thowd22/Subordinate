---
id: TASK-140
title: >-
  Store the user's test media in a private S3 bucket readable by RunsOn
  instances
status: To Do
assignee: []
created_date: '2026-09-11 22:18'
labels:
  - infra
  - manual
milestone: m-8
dependencies: []
priority: high
ordinal: 160000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The repository is public, so personal test clips must not become release assets. Create a private bucket in the RunsOn AWS account with a lifecycle rule, grant the RunsOn instance role read access, upload the user's test MP4 (delivered to box or via aws s3 cp once the AWS CLI session is renewed), and document the s3:// path for the image recipes. Needs the user's AWS CLI login.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Bucket exists with block-public-access on, versioning off, and a policy allowing only the RunsOn instance role and the account to read
- [ ] #2 The test MP4 is uploaded and its s3 path and checksum are recorded in the task notes
<!-- AC:END -->
