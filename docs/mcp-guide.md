# Driving Subordinate from an agent (MCP)

`subordinate-mcp` speaks the Model Context Protocol on stdin and stdout and
forwards every tool call to the editor's Command API (docs/PLAN.md §7). It holds
no state of its own. This guide is the agent's side of the editor: how to
register the bridge, what every tool family does, what can be read without
spending a tool call, how failures come back, and the runbook that takes one
prompt to a working plugin.

The author's side — writing the plugin an agent installs — is
[docs/plugin-guide.md](plugin-guide.md).

## What the bridge is

One MCP tool is one Command API method: the same operation the user interface
performs, on the project the editor has open. That is deliberate — there is a
single command set behind the GUI, the CLI, plugins and this bridge — and it
means an agent can do exactly what a user can do, and no more.

Nothing in the tool list is written by hand. It is generated from the two
committed schema documents, `docs/schema/command-api.json` (the engine's
methods, exported from the Rust command set) and `docs/schema/plugin-api.json`
(the plugin host's), with the same names, descriptions and parameter schemas.
A method added to the Command API is a tool as soon as the schema is re-exported
with `cargo run -p subordinate-cli -- schema`.

The one translation is the name. Command API methods are dotted (`clip.trim_in`)
and MCP tool names may only contain `[A-Za-z0-9_-]`, so **each dot becomes an
underscore**: the tool is `clip_trim_in`, and its title carries the method name
unchanged.

The bridge connects to whatever editor the socket lock file advertises, and
starts `subordinate-cli serve` itself when no editor is running — so an agent can
work whether or not a window is open, and a headless server it started stops when
it does.

## Setting up .mcp.json

Build the bridge, then copy `docs/examples/mcp.json` into the project's
`.mcp.json` (or merge the `subordinate` entry into the one already there):

```sh
cargo build --release -p subordinate-mcp
```

```json
{
  "mcpServers": {
    "subordinate": {
      "type": "stdio",
      "command": "target/release/subordinate-mcp",
      "args": [],
      "env": {
        "SUBORDINATE_INSTANCE": "default",
        "SUBORDINATE_LOG": "info"
      }
    }
  }
}
```

Point `command` at an installed binary if the editor is not being built from
this checkout. The environment the bridge reads:

| Variable | What it does |
| --- | --- |
| `SUBORDINATE_INSTANCE` | Which editor instance to reach; defaults to `default` |
| `SUBORDINATE_ENDPOINT_DIR` | Where the socket and lock file live, overriding the per-user default |
| `SUBORDINATE_CLI` | The `subordinate-cli` to launch, when it is not the one beside `subordinate-mcp` |
| `SUBORDINATE_MCP_NO_LAUNCH` | `1` to fail with `command.not_running` rather than start a headless server |
| `SUBORDINATE_LOG` | The log filter; diagnostics go to stderr, because stdout is the protocol |

stdout carries the protocol and nothing else. Every diagnostic goes to stderr,
which is where an MCP client shows a server's log — so that is the first place to
look when a session will not start.

To drive a project that is not the one a running editor has open, or to keep a
run off a user's own instance, give the bridge an instance of its own: set
`SUBORDINATE_INSTANCE` to a scratch name and `SUBORDINATE_ENDPOINT_DIR` to a
scratch directory, and let it launch its own headless server.

## Conventions an agent must follow

- **Time is exact.** Every time in every parameter and result is a rational — a
  numerator, a denominator and a rate — never floating-point seconds. Round-trip
  what the project gives you rather than converting through a float.
- **Mutations are undoable.** Every mutating tool is one command on the editor's
  undo stack, reversible with `edit_undo` and re-applicable with `edit_redo`. A
  sequence of edits that should undo as one goes between `edit_begin_group` and
  `edit_commit_group`.
- **Read before you edit.** The project is available as a resource; reading it
  costs no tool call and is always cheaper than asking the user what is in it.
- **Errors are data.** A rejected call comes back as a tool error whose content
  is the usual JSON `SubError`, stable code and all, rather than as a protocol
  error — so read the code and try something else.

## The tool families

Names below are the MCP tool names. Their full parameter and result schemas are
in `docs/schema/command-api.json` and `docs/schema/plugin-api.json`, and each
tool's own description in `tools/list` is the doc comment on the command itself.

### Reading the project

| Tool | What it does |
| --- | --- |
| `project_get` | The whole open project as JSON, with the revision it was read at |
| `project_revision` | Just the revision, to tell two reads apart cheaply |
| `history_get` | The undo stack: what would be undone and what redone |
| `system_list_methods` | Every method this build serves, which is the tool list from the engine's side |

### Editing the timeline

| Family | Tools |
| --- | --- |
| Clips | `clip_add`, `clip_insert`, `clip_move`, `clip_remove`, `clip_ripple_delete`, `clip_split`, `clip_trim_in`, `clip_trim_out`, `clip_set_params` |
| Tracks | `track_add`, `track_insert`, `track_remove`, `track_rename`, `track_reorder`, `track_set_gain`, `track_set_locked`, `track_set_muted`, `track_set_solo` |
| Sequences | `sequence_create`, `sequence_delete`, `sequence_insert`, `sequence_rename`, `sequence_set_settings` |
| Transitions | `transition_add`, `transition_remove` |
| Effects | `clip_add_effect`, `clip_insert_effect`, `clip_move_effect`, `clip_remove_effect`, `clip_set_effect_param` |
| Markers | `marker_add`, `marker_move`, `marker_remove`, `marker_rename`, `marker_replace_on_clip`, `marker_from_analysis` |

`clip_set_params` is where per-clip opacity, transform, gain and fades live.
The effect tools edit a clip's effect stack: `clip_add_effect` names the plugin
declaring the effect, the stack runs in list order, and `clip_set_effect_param`
binds one declared parameter — omit its `value` to put the parameter back to
the plugin's declared default.
`marker_from_analysis` turns an analyzer plugin's findings into timeline markers
as an ordinary undoable command.

### Media and bins

| Family | Tools |
| --- | --- |
| Media | `media_import`, `media_insert`, `media_relink`, `media_remove`, `media_set_analysis`, `media_remove_analysis`, `media_replace_analyses` |
| Bins | `bin_create`, `bin_insert`, `bin_move`, `bin_move_media`, `bin_remove`, `bin_rename` |

### Undo, redo and grouping

| Tool | What it does |
| --- | --- |
| `edit_undo`, `edit_redo` | One step each way on the shared undo stack |
| `edit_begin_group`, `edit_commit_group`, `edit_abort_group` | Make several commands one undo step; aborting restores what the group had changed |
| `edit_restore_track_items` | The inverse half of a structural edit, used when re-applying one |

### Events

`events_subscribe` and `events_unsubscribe` put an agent on the editor's event
bus, so an edit made in the GUI — or by another agent — is something to be told
about rather than polled for. Resource subscriptions (below) are the higher-level
version of the same thing.

### Plugins

Every plugin-management method is a tool, which is what lets an agent run the
whole plugin developer loop from a conversation.

| Tool | What it does |
| --- | --- |
| `plugin_new` | Scaffold a plugin crate for one WIT world: manifest, `CLAUDE.md`, source template and a fixture project |
| `plugin_install` | Install a built component or a plugin directory, optionally as a watched dev install |
| `plugin_reload` | Load an installed plugin again, re-registering its commands, effects and tools |
| `plugin_test` | Run the headless harness over an installed plugin: every check its declared worlds call for, against a fixture project |
| `plugin_list` | Everything installed, including what failed to load and what is hidden by an id conflict |
| `plugin_status` | What each plugin's last load did, with the error of one that failed |
| `plugin_enable`, `plugin_disable` | Switch one on or off without removing it |
| `plugin_remove` | Delete an installed plugin's directory from disk |
| `plugin_tools` | Every MCP tool the enabled plugins contribute, each under its plugin's id prefix |
| `plugin_call_tool` | Run one of those tools, with arguments the host validates against its declared schema |

The one step of the loop that is **not** a tool is `cargo build --release
--target wasm32-wasip2`: the editor deliberately does not run compilers on an
agent's behalf, so an agent runs that in its own terminal. Everything either side
of it — scaffolding, installing, reloading, testing — is a tool call.

### Tools plugins contribute

A plugin in the `mcp-tools` world contributes tools of its own, and the bridge
publishes them beside the built-in ones. Each is namespaced by its plugin id so
two plugins cannot collide: the plugin id with its dots turned into underscores,
then an underscore, then the plugin-local name. `com.example.silence-cutter`
contributing `cut_silence` appears as
`com_example_silence-cutter_cut_silence`.

Arguments are validated against the schema the plugin's manifest declared before
the call reaches the plugin, and a plugin may only offer tools its manifest
declared and the user approved. `plugin_tools` lists them and `plugin_call_tool`
is the explicit form of the same call.

## Resources: reading without spending a call

Three resources are the project's own JSON — the model the project file
serialises — taken out of one `project.get` round trip:

| URI | What it is |
| --- | --- |
| `project://current` | The whole open project, with the revision it was read at |
| `sequence://{id}` | One timeline |
| `media://{id}` | One media item |

There is no second representation to keep in step: a resource is a projection of
the project, and the revision travels with it so two reads can be told apart.
Every list and every read carries the `ttlMs` and `cacheScope` caching hints of
the 2026-07-28 specification; the times are deliberately short because the
project is edited underneath them, and a client that subscribes is told the
moment an edit makes a resource stale, straight off the editor's event bus. Both
subscription mechanisms are served: the 2026-07-28 `subscriptions/listen` stream
and `resources/subscribe` for clients that negotiated an earlier version.

Attach `project://current` to the conversation instead of asking what is in the
project.

## When a call fails

A call the engine rejects is answered as a **tool error**, not a protocol error:
the request was well formed and reached the editor, and the `SubError` it
produced is what the agent needs to act on. The content is JSON with a stable
`code`, a message, and details — for a plugin failure, the WIT item it belongs to
and a one-line hint saying what would fix it.

Codes worth recognising:

| Code | Means |
| --- | --- |
| `command.not_running` | No editor to talk to, and launching one was refused (`SUBORDINATE_MCP_NO_LAUNCH`) |
| `core.invalid_argument` | The arguments do not satisfy the method's schema |
| `plugin.invalid_manifest` | A `plugin.toml` will not parse; the `field` detail is the offending key |
| `plugin.capability_denied` | The plugin asked for something its manifest or its approval does not cover |
| `plugin.fuel_exhausted`, `plugin.deadline_exceeded`, `plugin.trapped` | A plugin call hit a limit or trapped; nothing else was affected |
| `plugin.not_installed` | No plugin with that id in either plugin directory |

A plugin failure never takes the editor with it: the engine, the project and the
undo stack are untouched, and the other plugins keep running.

## The agent runbook

[docs/agent-runbook.md](agent-runbook.md) is the end-to-end exercise of
everything above, and the phase 6 exit test: **given the editor and nothing
else, does a fresh Claude Code session turn a single prompt into a plugin that
installs and passes `plugin test`?**

```sh
scripts/agent-runbook.sh
```

From a clean checkout that is the whole run. It builds `subordinate-cli` and
`subordinate-mcp`, serves the Command API on a scratch instance with a plugin
directory of its own — nothing touches the user's installed plugins — writes the
MCP config, hands the prompt to a fresh non-interactive session with the
editor's tools allowed, then re-checks the result itself with `subordinate-cli
plugin test`. It prints one JSON verdict and exits non-zero when the run failed.

The prompt is in the runbook between markers, and the script reads it from
there, so the document is the prompt of record and the two cannot drift. The
shape it asks for is the loop this guide describes: `plugin_new`, then a
`cargo build` in the agent's own terminal, then `plugin_install`, then
`plugin_test`.

Preconditions the script checks before it starts: a Rust toolchain with the
`wasm32-wasip2` target, a GStreamer development install (see
[docs/DEVELOPMENT.md](DEVELOPMENT.md) — `subordinate-cli` links `sub-media`), and
the `claude` CLI on `PATH`, authenticated.

## Where to look next

- [docs/plugin-guide.md](plugin-guide.md) — writing the plugins these tools
  install, and the worked examples in `plugins/`: `plugins/color`,
  `plugins/gain`, `plugins/cut-silence` and `plugins/otio`.
- [docs/agent-runbook.md](agent-runbook.md) — the prompt, the script and the
  verdict.
- `docs/schema/command-api.json`, `docs/schema/plugin-api.json` — every method's
  parameters and results.
- `docs/examples/mcp.json` — the configuration above, committed and tested.
- [docs/DEVELOPMENT.md](DEVELOPMENT.md) — toolchain, GStreamer and fixtures.
