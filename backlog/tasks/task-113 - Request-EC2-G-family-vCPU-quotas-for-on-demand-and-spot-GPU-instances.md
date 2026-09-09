---
id: TASK-113
title: Request EC2 G-family vCPU quotas for on-demand and spot GPU instances
status: In Progress
assignee:
  - '@claude'
created_date: '2026-09-09 17:29'
updated_date: '2026-09-09 20:20'
labels:
  - infra
  - gpu
  - manual
milestone: m-8
dependencies: []
priority: high
ordinal: 133000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
New AWS accounts have a zero or tiny quota for G-family instances, so g4dn and g4ad runners cannot start until Service Quotas are raised. Requests can be filed with the AWS CLI; approval is manual on AWS's side and can take a day.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Service quota requests filed for 'Running On-Demand G and VT instances' and 'All G and VT Spot Instance Requests' to at least 16 vCPUs each in the RunsOn region
- [ ] #2 Approved values are confirmed with aws service-quotas get-service-quota and recorded in the task notes
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Read current quotas (both 0). 2. File requests for 16 vCPUs each via aws service-quotas in us-east-1. 3. Poll until approved, record values.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-09: both quotas were 0.0 in us-east-1. Filed increase requests to 16 vCPUs for L-DB2E81BA (Running On-Demand G and VT instances) and L-3819A6DF (All G and VT Spot Instance Requests); both PENDING. g4dn.xlarge and g4ad.xlarge are 4 vCPUs each, so 16 allows four concurrent GPU jobs.

2026-09-09 (later): on-demand G-family quota approved at 8 vCPUs (requested 16, case still open for the rest); spot G-family quota still 0.0 with the case open. 8 on-demand vCPUs allows two concurrent g4dn/g4ad xlarge jobs. RunsOn automatically falls back from spot to on-demand when the spot request is refused (observed on the first gpu-smoke dispatch, run 34400346107).
<!-- SECTION:NOTES:END -->
