---
id: TASK-113
title: Request EC2 G-family vCPU quotas for on-demand and spot GPU instances
status: To Do
assignee: []
created_date: '2026-09-09 17:29'
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
- [ ] #1 Service quota requests filed for 'Running On-Demand G and VT instances' and 'All G and VT Spot Instance Requests' to at least 16 vCPUs each in the RunsOn region
- [ ] #2 Approved values are confirmed with aws service-quotas get-service-quota and recorded in the task notes
<!-- AC:END -->
