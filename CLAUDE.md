# Subordinate

Cross-platform video editor: small Rust core, agent-first WASM plugin system, local MCP server.

- **Plan of record:** `docs/PLAN.md`. Architecture, stack choices and roadmap live there; tasks reference its sections.
- **Decisions:** `backlog/decisions/` (all accepted). Do not reopen them without the user.
- **Tasks:** 8 milestones (m-0 .. m-7) mirror the roadmap phases. Coarse tasks TASK-1..5 are parents; their dotted subtasks (TASK-1.1 etc.) are the units of work.
- **Picking work:** choose a To Do task whose dependencies are all Done (`backlog task view <id> --plain` shows the graph). Start at m-0. Tasks labelled `spike` produce findings in a backlog doc, not production code. Tasks labelled `verify` need real hardware and are manual.
- **Conventions:** all timeline math in `RationalTime`, never floats. Every mutation is an undoable Command exposed through the Command API. Errors are `SubError` with stable codes. Nothing in the audio callback may lock or allocate.


<!-- BACKLOG.MD GUIDELINES START -->
<!-- backlog.md-instructions-version: 1.51.0 -->
<CRITICAL_INSTRUCTION>

## Backlog.md Workflow

This project uses Backlog.md for task and project management.

**At the beginning of each conversation in this project, run `backlog instructions overview` before answering or taking action. Re-read it only if you have not read it yet in the current conversation.**

Use the overview to decide whether to search, read, create, or update Backlog tasks.

Before task lifecycle actions, read the matching detailed guide:
- `backlog instructions task-creation` before creating or splitting tasks
- `backlog instructions task-execution` before planning, changing status or assignee, adding a plan or implementation notes, or implementing task work
- `backlog instructions task-finalization` before checking acceptance criteria, writing final summaries, or moving tasks to terminal statuses

Use `backlog <command> --help` before running unfamiliar commands. Help shows options, fields, and examples.

Do not edit Backlog task, draft, document, decision, or milestone markdown files directly. Use the `backlog` CLI so metadata, relationships, and history stay consistent.

</CRITICAL_INSTRUCTION>
<!-- BACKLOG.MD GUIDELINES END -->
