---
id: TASK-112
title: Install RunsOn in the AWS account and connect it to the GitHub repository
status: To Do
assignee: []
created_date: '2026-09-09 17:29'
labels:
  - infra
  - gpu
  - manual
milestone: m-8
dependencies: []
references:
  - 'https://runs-on.com/docs/'
priority: high
ordinal: 132000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GPU-only acceptance criteria (NVENC, VA-API, AMF, 4K hardware scrub) cannot be verified on GitHub-hosted runners. RunsOn (runs-on.com) boots EC2 instances per job from a runs-on: label and supports NVIDIA g4dn and AMD g4ad on Linux and Windows. Installation is a CloudFormation stack in the user's AWS account plus a GitHub App install, both interactive steps the user performs; an agent can prepare the parameters, verify the stack and record the outcome. Licence: free for personal non-commercial use, otherwise about 300 EUR/yr.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 RunsOn CloudFormation stack is deployed in the chosen region and reports healthy in the RunsOn dashboard
- [ ] #2 The RunsOn GitHub App is installed on thowd22/Subordinate
- [ ] #3 A smoke workflow job with runs-on: runs-on=${{ github.run_id }}/runner=2cpu-linux-x64 starts and passes
- [ ] #4 docs/DEVELOPMENT.md gains a GPU CI section naming the region, stack and how to re-run the smoke job
<!-- AC:END -->
