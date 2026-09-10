---
id: doc-3
title: 'TASK-102 run log: Claude Code builds a plugin from one prompt'
type: other
created_date: '2026-09-10 21:48'
updated_date: '2026-09-10 22:00'
---
The phase 6 exit test (TASK-102), run on 2026-09-10. One prompt, one plugin,
no manual steps: `scripts/agent-runbook.sh` served the Command API on a scratch
instance, handed `docs/agent-runbook.md`'s prompt to a fresh non-interactive
Claude Code session with only the editor's MCP tools plus a shell, and then
re-checked the result itself with `subordinate-cli plugin test`.

## Verdict

```json
{
  "agent_exit": 0,
  "changed": true,
  "failed": 0,
  "ok": true,
  "passed": 6,
  "plugin_test_exit": 0,
  "skipped": 0,
  "summary": "com.example.remove-gaps: 6 passed, 0 failed, 0 skipped",
  "undoable": "pass",
  "work": "/tmp/agent-runbook-KlvCRQ"
}
```

The six checks: `component_loads`, `instantiates`, `run`, `answers_json`,
`project_state` (changed) and `undoable` (one undo put the fixture back exactly
as it was, which is the proof that every edit went through the Command API).
The plugin's own answer reported 2 gaps closed, 2 clips moved and 15 frames
removed at 25 fps — the whole of what `docs/examples/gaps.sub` has in it — and
the host log carries its two `info` lines.

## The run

- Model session: `fe4e16ad-d220-409a-9ee1-91ce953256c5`, 35 turns, 225 s wall,
  $3.06, `stop_reason: end_turn`, `permission_denials: []`, no subagents.
- Nothing was typed after the prompt, and nothing was fixed by hand: the
  session scaffolded with `plugin_new`, wrote 326 lines of Rust, built for
  `wasm32-wasip2`, installed with `plugin_install --dev`, and iterated with
  `plugin_test` until the report came back clean.
- One self-caught bug on the way: its first build reported 25 frames removed
  instead of 15, because each gap was measured against the already-shifted
  cursor. It found that in its own answer, not in the harness's report.
- Artifacts kept off the temporary directory:
  `~/.cache/subordinate/agent-runbook-2026-09-10/` — the rendered prompt, the
  session result JSON, the verification report, the MCP config, the editor log
  and the plugin's `src/`, `Cargo.toml` and `plugin.toml`.

The plugin the session wrote reads every sequence and track through the typed
`Project` accessors, plans the whole pack before touching anything, then
applies it as `clip.move` commands; all arithmetic goes through
`subordinate_sdk::time`, and it opens an undo group only when `history.get`
says none is open, because the host already opens one around a `command` run.

## Friction, and what each became

1. **The scaffolded crate does not build on a default toolchain.** It declares
   `rust-version = 1.95` and writes no `rust-toolchain.toml`, so with a stable
   older than that (1.93.1 here) `cargo build --release --target wasm32-wasip2`
   fails and the agent has to work out `rustup run 1.95.0` for itself. The
   scaffold's promise is that what it writes builds with no edits. — TASK-128
2. **`plugin test` cannot see what a command plugin actually did.** Its
   `project_state` check carries counts only, so the 25-frames-instead-of-15
   version of the plugin passed the harness; only the plugin's own answer said
   otherwise. A fixture needs a way to declare the timeline it expects
   afterwards. — TASK-129
3. **The command template answers JSON with no JSON dependency.** The
   generated `src/lib.rs` assembles its answer with `format!` over hand-written
   braces because the generated `Cargo.toml` depends on `subordinate-sdk`
   alone; the first edit the agent made was to add `serde_json`. — TASK-130
4. **`cargo build` is not an MCP tool, by design.** The editor does not run
   compilers on an agent's behalf (`sub_plugin::authoring`), so the loop is
   MCP, MCP, *shell*, MCP, MCP. That cost this run nothing — the session had a
   terminal — but it is the reason the runbook says "the editor's MCP tools"
   rather than "only MCP tools", and it is what an agent in a conversation with
   no shell would hit. Left as it is; no task.

## Re-running it

`scripts/agent-runbook.sh` is the whole procedure and `docs/agent-runbook.md`
the document of record; the prompt lives in the document and the script reads
it from there, so the two cannot drift, and
`cargo test -p subordinate-mcp --test runbook` fails if the prompt names a tool
the bridge does not serve or the fixture stops having 15 frames of gap in it.
Worth re-running when the plugin developer loop changes: a new world template,
a change to `plugin.new`'s parameters, or a new step in install or test.
