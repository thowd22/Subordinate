# Agent runbook: a working plugin from one prompt

This is the phase 6 exit test (TASK-102, docs/PLAN.md §8). It asks one
question and answers it objectively: **given the editor and nothing else, does
a fresh Claude Code session turn a single prompt into a plugin that installs
and passes `plugin test`?**

Everything the agent needs is reachable from the conversation. Scaffolding,
installing, reloading and testing are MCP tools (`plugin_new`,
`plugin_install`, `plugin_reload`, `plugin_test`), and so is every edit the
plugin itself makes, because a plugin edits the project through the same
Command API the bridge exposes (docs/PLAN.md §6.4, §7). The one step that is
not an MCP tool is `cargo build`: the editor deliberately does not run
compilers on an agent's behalf (`sub_plugin::authoring`), so the agent runs
that in its own terminal.

## Run it

```
scripts/agent-runbook.sh
```

From a clean checkout that is the whole run: it builds `subordinate-cli` and
`subordinate-mcp`, serves the Command API on a scratch instance with a plugin
directory of its own (nothing touches the user's installed plugins), writes
the MCP config, hands the prompt below to a fresh non-interactive Claude Code
session with the editor's tools allowed, then re-checks the result itself with
`subordinate-cli plugin test`. It prints one JSON verdict and exits non-zero
when the run failed.

Useful flags: `--work DIR` to keep the run in a directory of your choosing,
`--prompt` to print the rendered prompt and stop, `--verify-only` to re-check
a previous run's work, `--agent CMD` to drive some other agent, `--release` to
build the binaries in release, and `--no-build` to use the ones already in
`target/`.

Preconditions, all of them checked by the script before it starts: a Rust
toolchain with the `wasm32-wasip2` target, a GStreamer development install
(docs/DEVELOPMENT.md — `subordinate-cli` links `sub-media`), and the `claude`
CLI on `PATH`, authenticated.

## The prompt

One message, no follow-ups. The script renders `{{WORKDIR}}`, `{{SDK}}` and
`{{FIXTURE}}` and sends what is between the markers below verbatim, so this
file is the prompt of record and the two cannot drift.

<!-- prompt:start -->
```
You have the Subordinate video editor connected over MCP. Build, install and
test a plugin that removes the gaps from a timeline. Work on your own: make
reasonable choices rather than asking me questions.

1. Scaffold it with the `plugin_new` MCP tool: world `command`, name
   `remove-gaps`, id `com.example.remove-gaps`, parent `{{WORKDIR}}`,
   sdk_path `{{SDK}}`. Read the CLAUDE.md and the src/lib.rs it writes before
   you change anything.
2. Implement `run` in `{{WORKDIR}}/remove-gaps/src/lib.rs`. For every sequence
   and every track it must close the gaps: afterwards the first clip starts at
   zero and each later clip starts exactly where the one before it ends, in
   the order they were already in, none of them trimmed. Every edit goes
   through the Command API on `Project`, so the whole thing is undoable in one
   step. All timeline arithmetic is exact `RationalTime`; never convert a time
   to a float. Answer with a JSON object saying how many gaps were closed and
   how much time was removed.
3. Build it in your terminal: `cargo build --release --target wasm32-wasip2`
   in `{{WORKDIR}}/remove-gaps`. That is the one step the editor does not do
   for you.
4. Install it with the `plugin_install` MCP tool: path
   `{{WORKDIR}}/remove-gaps/target/wasm32-wasip2/release/remove_gaps.wasm`,
   dev true.
5. Test it with the `plugin_test` MCP tool: id `com.example.remove-gaps`,
   fixture `{{FIXTURE}}`. That fixture is one 25 fps video track holding three
   clips of 25 frames with a 10 frame gap and a 5 frame gap between them, so a
   correct run removes 15 frames. Keep fixing, rebuilding, reloading
   (`plugin_reload`) and re-testing until the report's `ok` is true, its
   `project_state` check reports `changed: true`, and its `undoable` check
   passes.

Finish by printing the final `plugin_test` report as JSON.
```
<!-- prompt:end -->

## The tool surface

The session gets the editor's MCP server (`docs/examples/mcp.json` with the
scratch instance in its environment) plus the file and shell tools it needs to
write Rust and run `cargo`. It gets no other MCP server, no browser and no
network beyond the crate registry.

## Pass criteria

A run passes when all of these hold, none of them judged by the agent:

1. `com.example.remove-gaps` is installed in the run's plugin directory, with
   a `plugin.wasm` the host loaded.
2. `subordinate-cli plugin test com.example.remove-gaps --fixture
   docs/examples/gaps.sub`, run by the script after the session ends, reports
   `ok: true` with no failed checks.
3. Its `project_state` check reports `changed: true` and its `undoable` check
   passes — the plugin actually edited the fixture, and one undo put it back,
   which is the proof that every edit went through the Command API.
4. Nobody typed anything after the prompt, and the session recorded no
   permission denial it had to be let past.

The plugin's own answer — how many gaps it closed and how much time came out —
is printed beside the verdict but is not part of it. The harness can prove that
the project changed and that one undo reversed it; it cannot yet assert the
track's resulting layout, so a plugin that moved the clips to the wrong places
would still pass (TASK-129).

The script writes the session transcript, the rendered prompt, the plugin
source and the verification report into the work directory, and names them in
its verdict.

## Recording a run

Findings go in a backlog doc — `backlog doc create` — with the verdict, the
transcript path, and one friction note per thing that made the run harder than
it should have been. Each friction note becomes a follow-up task; the runbook
is only worth re-running if what it finds turns into work.

`backlog/docs/doc-3` is the first recorded run: 2026-09-10, 35 turns and 225
seconds from the prompt to a plugin that passes all six checks, with three
friction notes that became TASK-128, TASK-129 and TASK-130.
