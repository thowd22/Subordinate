---
id: TASK-112
title: Install RunsOn in the AWS account and connect it to the GitHub repository
status: Done
assignee:
  - '@tyler'
created_date: '2026-09-09 17:29'
updated_date: '2026-09-09 18:11'
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
- [x] #1 RunsOn CloudFormation stack is deployed in the chosen region and reports healthy in the RunsOn dashboard
- [x] #2 The RunsOn GitHub App is installed on thowd22/Subordinate
- [x] #3 A smoke workflow job with runs-on: runs-on=${{ github.run_id }}/runner=2cpu-linux-x64 starts and passes
- [x] #4 docs/DEVELOPMENT.md gains a GPU CI section naming the region, stack and how to re-run the smoke job
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
User deploys the RunsOn CloudFormation stack (runs-on, us-east-1) and installs the GitHub App; supervisor watches the stack, then adds the smoke workflow and DEVELOPMENT.md section.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-09-09 18:00 UTC: stack runs-on in us-east-1 reached CREATE_COMPLETE (RunsOn v3.3.0); ECS worker running, licence status valid. GitHub App not yet configured: worker logs show primary_app_config_id=0 and the smoke job (run 34386369216) stayed queued with no webhook delivered; cancelled it. Entry point for the app setup: https://qiog5mfgyg.execute-api.us-east-1.amazonaws.com/prod . Smoke workflow committed at .github/workflows/runs-on-smoke.yml (workflow_dispatch).

2026-09-09 18:10 UTC: GitHub App installed by the user (scheduler now reports primary_app_config_id 4888563). Smoke run 34387366919 was picked up immediately and passed on spot m8i.large i-0ae748f62de736ec8 (2 vCPU, Ubuntu 24.04). DEVELOPMENT.md gained the GPU CI section.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
RunsOn v3.3.0 deployed in us-east-1 and connected to the repository; verified by a workflow_dispatch smoke job that ran on an EC2 spot instance in the account. Documented in docs/DEVELOPMENT.md.
<!-- SECTION:FINAL_SUMMARY:END -->
