---
id: TASK-140
title: >-
  Store the user's test media in a private S3 bucket readable by RunsOn
  instances
status: In Progress
assignee:
  - '@claude'
created_date: '2026-09-11 22:18'
updated_date: '2026-09-11 22:22'
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
- [x] #1 Bucket exists with block-public-access on, versioning off, and a policy allowing only the RunsOn instance role and the account to read
- [ ] #2 The test MP4 is uploaded and its s3 path and checksum are recorded in the task notes
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-11 supervisor: created s3://subordinate-test-media-731537225673 in us-east-1: all four public-access blocks on, AES256 default encryption, lifecycle aborts incomplete multipart uploads after 2 days, policy allows only the RunsOn instance role (runs-on-EC2InstanceRole-KGQrU1NH7McV) GetObject/ListBucket plus a deny on non-TLS; tagged stack=runs-on, project=subordinate. Awaiting the user's MP4 (expected at box:~/test-media/) for criterion 2.
<!-- SECTION:NOTES:END -->
