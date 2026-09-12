---
id: doc-5
title: Supervisor handoff 2026-09-12
type: guide
created_date: '2026-09-12 15:32'
updated_date: '2026-09-12 15:32'
---
# Supervisor handoff, 2026-09-12

State of the project when the supervising session paused (usage limits).

## Releases
- v0.1.3 is the latest pre-release (live preview, GOP-cached scrub, MCP bridge in every package, import/relink fixed on all OSes). MSI sha256 18589907aed62469657c358bb542ee874e657b5a8a34b2ebd23ddb8475153eb7.
- Next release (0.1.4) should carry: TASK-145 (AppImage plugin libraries, merged), TASK-146 (export stall fix, agent in progress), and whatever the bug wave lands (TASK-149..155). Bump Cargo.toml version and the metainfo release entry, push, wait for CI, tag vX.Y.Z; release.yml publishes; mark the release as pre-release by hand (gh release edit --prerelease).

## Machines and images
- RunsOn (AWS us-east-1, public mode, no NAT). Runners: gpu-nvidia-linux, gpu-nvidia-windows, gpu-nvidia-desktop-linux (ami-018b86365d9ae09c1, v0.1.3), gpu-nvidia-desktop-windows (see .github/runs-on.yml; 1.1.6 rebuilt 2026-09-12). Quota: 8 G-family vCPUs = two g4dn at once; the increase case to 16 is open and unanswered. Named runners resolve only from main and RunsOn caches runs-on.yml about an hour; dispatch with inline ami=... labels to bypass.
- box: self-hosted AMD Linux (labels self-hosted,linux,box,amd-gpu). yodaddy: the user's Windows desktop (self-hosted,windows,yodaddy,amd-gpu,two-monitors; NEVER drive it via WSL PowerShell interop, only through runner jobs). M1 Mac mini purchased, not yet arrived (TASK-117).
- Test media: s3://subordinate-test-media-731537225673 (excerpt, private) and box:~/test-media/meld-4k60-full.mkv; baked into both desktop images.
- AWS CLI at ~/.local/bin/aws; sessions from aws login expire in hours.

## Running when paused
- Wave workflow run 6 (backlog-waves-v2) dispatching the export-matrix bug tasks (TASK-149..155).
- Dedicated agent on TASK-146 (branch task/task-146).
- Windows desktop AMI build 1.1.6 followed by its smoke job.

## Open work (see each task's notes)
- Export bugs from the matrix: 146, 148, 149, 150, 151, 152, 153, 154, 155.
- Desktop flows: 139 (Windows export half; screenshot comparison). Image bake list: 147.
- Mac: 117, 105, 66 (VideoToolbox), 64 (NVENC visual check).
- Manual checks: 68 (monitor picker on yodaddy), 97 (Claude Code elicitation dialog).

## Conventions that matter
- Supervisor commits go through /home/admin2/.cache/subordinate/supervisor, never the agents' checkout at /home/admin2/Subordinate.
- Workers must not create Backlog tasks (id collisions); the supervisor files follow-ups after merges.
- Merge agent branches with --no-ff; if a task file conflicts, keep both sides and dedupe frontmatter keys; run backlog doctor before pushing.
- cargo target directories are never garbage-collected: clean the agents' checkout target when it passes 40 GB.


## Stopped on 2026-09-12 at the user's usage limit
- Wave workflow run wf_423097c5-8cf was STOPPED mid-wave. TASK-150 finished and is merged. Workers on TASK-149, 151, 153, 154 and 155 were cut off; their worktrees under /home/admin2/Subordinate/.claude/worktrees/ still hold whatever they had:
task/task-155: 15 uncommitted files, 0 commits ahead of main (/home/admin2/Subordinate/.claude/worktrees/wf_423097c5-8cf-2)
task/task-150: 0 uncommitted files, 1 commits ahead of main (/home/admin2/Subordinate/.claude/worktrees/wf_423097c5-8cf-3)
task/task-154: 7 uncommitted files, 0 commits ahead of main (/home/admin2/Subordinate/.claude/worktrees/wf_423097c5-8cf-4)
task/task-149: 8 uncommitted files, 0 commits ahead of main (/home/admin2/Subordinate/.claude/worktrees/wf_423097c5-8cf-5)
task/task-151: 4 uncommitted files, 0 commits ahead of main (/home/admin2/Subordinate/.claude/worktrees/wf_423097c5-8cf-6)
task/task-153: 17 uncommitted files, 0 commits ahead of main (/home/admin2/Subordinate/.claude/worktrees/wf_423097c5-8cf-7)

  On resume: for each, `git -C <worktree> status`; if the work looks usable, commit it on its task branch and let a fresh worker continue from it (relaunching the wave with the same task starts from main, so merge or discard first); otherwise `git worktree remove --force` and delete the branch. Those tasks remain To Do on main.
- The TASK-146 agent (export stall) was asked to commit a WIP state to task/task-146 and stop; read its notes before relaunching.
- The Windows desktop AMI 1.1.6 build and its smoke job were left running unattended (a shell task, no agent tokens); check the newest subordinate-windows-desktop AMI and update .github/runs-on.yml if it succeeded.
