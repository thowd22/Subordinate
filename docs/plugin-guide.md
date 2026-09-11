# Writing a Subordinate plugin

The plugin system is the product (docs/PLAN.md §6). This is the guide for
whoever writes one — a person reading it once, or an agent reading it in a
conversation — and it is written so that both can work from it alone: every
world, the manifest, the capability model, the developer loop, and how to prove
the result works without opening a window.

Everything here is true of the checkout it ships with. The worked examples are
the first-party plugins in `plugins/`, the interface is `wit/subordinate-plugin.wit`,
and the error catalogue is `sub_plugin::errors` (published to plugin authors as
`subordinate_sdk::errors`).

## What a plugin is

A plugin is **one WebAssembly component** implementing one or more
`subordinate:plugin@0.1.0` worlds, plus a `plugin.toml` beside it. It runs
sandboxed under wasmtime with a fuel budget, a wall-clock deadline and a memory
ceiling, and it starts with no filesystem, no network and no shader
compilation.

Three rules shape every world:

1. **A plugin never holds the project model.** It edits by calling back into the
   Command API (`run-command`) and reads by querying it (`query`), so a plugin
   edit is an ordinary undoable command that the GUI, the CLI and the MCP bridge
   all see identically (decision-6, decision-7).
2. **All time is exact.** Times cross the boundary as a rational — a numerator,
   a denominator and a rate — never as floating-point seconds.
3. **Every failure is a code.** A plugin returns, and receives, an `error`
   record carrying a stable dotted code, a message and details; the host adds a
   `wit` detail naming the interface item and a `hint` saying what would fix it.

Build any of them with:

```sh
cargo build --release --target wasm32-wasip2
```

The Rust toolchain emits a component directly from `wasm32-wasip2`, so there is
no `cargo component` and no `wasm-tools component new` step.

## The worlds

`wit/subordinate-plugin.wit` declares them all; `wit/README.md` is the
interface-level summary and this table is the authoring view. Every world
imports the same `command-api` interface, so anything one world can do to a
project, all of them can.

| World | Plugin exports | Used for | Worked example |
| --- | --- | --- | --- |
| `command` | `run(project, args) -> string` | A new editing operation, built from Command API primitives | `plugins/cut-silence`, `plugins/otio/otio-exporter` |
| `commands` | `commands() -> list<command-desc>`, `run(id, context)` | The same, plus entries in the Plugins menu and the shortcut map | scaffold: `plugin new --world command` |
| `effect` | `describe() -> effect-desc` | A GPU effect: a parameter schema plus WGSL the host compiles and runs | `plugins/color` |
| `effect-cpu` | `describe()`, optional `process-cpu(frame, params)` | Small-buffer pixel work: thumbnails, analysis, fixtures | — |
| `audio-effect` | `describe()`, `process(block, channels, rate, params)` | Block processing under a real-time budget | `plugins/gain` |
| `importer` | `supported-extensions()`, `import(path)` | Turning a file into media items and sequences | `plugins/otio/otio-importer` |
| `exporter` | `presets()`, `post-export(path)` | Contributing encoder presets and a post-encode hook | — |
| `analyzer` | `analyze(media, options) -> analysis-result` | Background analysis reporting markers, ranges and metadata | `plugins/cut-silence` |
| `mcp-tools` | `tools() -> list<tool-desc>`, `call(name, args-json)` | Extra MCP tools the bridge forwards to an agent | scaffold: `plugin new --world mcp-tools` |
| `panel` | — | A declarative UI panel. **Post-MVP**: the name parses, nothing hosts it yet | — |

`plugin.worlds` in the manifest takes the names in the first column with two
exceptions worth knowing: `commands` is declared as `command` (it is the same
world with menu registration on top), and `effect-cpu` is declared as `effect`
(it is `effect` plus one optional export). The manifest world names are
`effect`, `audio-effect`, `importer`, `exporter`, `analyzer`, `command`,
`panel` and `mcp-tools`.

### command and commands

The whole world is one import. `run` receives a project identifier and a JSON
argument string, and works by sending Command API methods back at the host:

```rust
use subordinate_sdk::{Guest, Project, ProjectId, Result, export};

struct Plugin;

impl Guest for Plugin {
    fn run(project: ProjectId, _args: String) -> Result<String> {
        let project = Project::new(project);
        let revision = project.query(&subordinate_sdk::params::ProjectRevision {})?;
        Ok(format!(r#"{{"revision":{}}}"#, revision.revision))
    }
}

export!(Plugin);
```

`Project::run` applies one undoable command, `Project::query` reads without
mutating, and both take a typed parameter struct from `subordinate_sdk::params`
that carries its own method name. The host opens **one undo group** around a
run, so a command that applies twenty primitives is still a single Ctrl+Z, and a
run that fails part way leaves the project as it found it.

A `commands` plugin additionally answers `commands()` when it loads, with an id,
a title and optionally the chord it would like per entry. The host validates
those, qualifies each id with the plugin id and keeps them in a registry the
Plugins menu and the keyboard map read without instantiating anything.

`plugins/cut-silence` is the reference: it splits looking from editing, which is
the architecture in miniature.

### effect and effect-cpu

A WASM guest cannot touch the GPU, and per-pixel loops in WASM are far too slow
at picture size, so a GPU effect is **declared, not executed** (decision-6).
`describe()` returns a parameter schema — float, int, bool, colour or a closed
set of choices, each with its range and default — plus WGSL source and the name
of its fragment entry point. The core compiles the shader, caches it by hash,
builds the uniform struct from the declared parameters and runs it in the
compositor. `describe` is called at load time and after a hot reload, never per
frame; per-clip parameter values live in the project model, where ordinary
undoable commands edit them.

`plugins/color` is the effect template, and it splits into three files on
purpose: `src/grade.rs` (the parameter table and a CPU reference for the grade,
with no WIT in it), `src/effect.wgsl` (the shader) and `src/lib.rs` (the lift
from the first into the `effect-desc`). That split is what lets the host's
golden test, `crates/sub-render/tests/color_plugin_golden.rs`, render the
plugin's own shader with the plugin's own parameter table and compare the
readback against the plugin's own reference, in linear light.

`effect-cpu` adds `process-cpu(frame, params)`. **It is slow**: the frame is
copied into the sandbox, looped over in WASM and copied back. Use it for
thumbnails, analysis passes and test fixtures, never for playback or export at
picture size. A plugin that only ships a shader targets `effect` and exports
nothing else.

An effect must ask for the `shaders` capability; the host compiles a plugin's
WGSL only where it was granted.

### audio-effect

`describe()` reports identity, parameters with their units and ranges, and the
share of real time the plugin claims. `process(block, channels, rate, params)`
maps one buffer of interleaved `f32` onto another of exactly the same shape.

The host takes the smaller of the claim and its own policy, times every block,
and bypasses the plugin after repeated overruns — so a plugin that asks for more
than it needs only gets itself bypassed sooner. Inside `process`, hold state
between calls (a smoothed parameter is only smooth if it remembers where it
was), reuse the incoming buffer rather than allocating a second one, and never
lock: the host side of this contract lives on the audio path, where nothing
allocates or locks. `plugins/gain` is the reference.

### importer and exporter

Neither world touches the project. An importer parses a file and *describes*
what it found — media specs, sequence specs, clips, transitions, markers — and
the host turns that description into `media.import` and `sequence.insert` calls
it makes itself, so an import is undoable and every identifier is minted
host-side. An exporter contributes named encoder presets and may act on the file
once the host has written it.

Both directions treat the plugin as untrusted input: a zero denominator, a path
escaping the project folder, a media reference pointing past the end of the
import, a preset id that is not in the stable form, or a setting whose value is
not JSON all come back as a `plugin.*` error rather than reaching the project.

`plugins/otio` is the worked example, and its README explains why the OTIO
*exporter* is a `command` plugin rather than an `exporter` one: the `exporter`
world contributes encoder presets, and writing an OTIO document encodes nothing.

### analyzer

`analyze(media, options)` is handed an identifier, never a path — a sandboxed
plugin never learns where the project's media lives, so it asks the host and
reads the file through the `$PROJECT` root the user approved. The world adds one
import, `analysis-host`, which is the progress and cancellation channel of the
job the host runs the call as.

An analyzer edits nothing. It reports markers, ranges and metadata in **media**
time, and the host stores them by applying `media.set_analysis` through the
engine, so findings arrive as an ordinary undoable command. Turning findings
into timeline markers is `marker.from_analysis`, another ordinary command.

### mcp-tools

A plugin declares its tools **twice**: in `[mcp.tools.*]` in the manifest, which
is what the user approved on install, and in the component's `tools()` export,
which is what the code offers. The host refuses a component whose exports do not
match the manifest, and validates a call's arguments against the declared JSON
Schema before dispatching, so a plugin never sees arguments its own schema
rejects and never offers a tool the manifest did not declare.

Names are namespaced by plugin id so an agent can tell two plugins' tools apart:
the plugin id with its dots turned into underscores, then an underscore, then the
plugin-local name. `com.example.silence-cutter` plus `cut_silence` is
`com_example_silence-cutter_cut_silence`. See [docs/mcp-guide.md](mcp-guide.md)
for the agent's side of this.

## plugin.toml

The manifest is the only thing the host reads from an uninstalled plugin, so it
carries everything an install decision needs. Parsing is **strict**: an unknown
key is an error rather than a silently ignored line, because a misspelt
capability must not read as "not requested". Every failure is a
`plugin.invalid_manifest` error whose `field` detail is the dotted path of the
offending key, so the exact line can be fixed.

```toml
[plugin]
id = "com.example.silence-cutter"      # reverse-DNS, at least one dot
name = "Silence Cutter"                # shown in the plugins panel
version = "0.1.0"                      # the plugin's own version, semver
api = "0.1"                            # the subordinate:plugin version built against
worlds = ["command", "mcp-tools"]      # non-empty set
description = "Remove silent regions." # optional, one sentence
authors = ["Ada <ada@example.com>"]    # optional, free-form

[capabilities]                         # optional; absent means nothing is asked for
fs_read = ["$PROJECT"]
fs_write = []
network = false
shaders = false

[mcp.tools.cut_silence]                # optional, one block per contributed tool
description = "Remove silent regions from the selected clips"
schema = "schemas/cut_silence.json"    # relative to the plugin directory
```

`plugin.api` is the interface version, not the plugin's. This host implements
`0.1` and refuses a manifest declaring one it does not support; the JSON Schema
of the whole file is committed at `docs/schema/plugin-manifest.json`.

The plugin id is load-bearing beyond identity: it is the install directory name,
the log tag, the key the capability grant is recorded under and the prefix on
every MCP tool the plugin contributes.

## Capabilities

Plugins are sandboxed by default (docs/PLAN.md §6.1). A component starts with
nothing and gets back exactly what `[capabilities]` asked for **and** the user
approved on install.

| Key | Type | What it grants |
| --- | --- | --- |
| `fs_read` | list of roots | Read access to those roots, as WASI preopens |
| `fs_write` | list of roots | Write access to those roots |
| `network` | bool | Opening network connections |
| `shaders` | bool | Having the plugin's WGSL compiled and run |

A root is written against a **variable**, never a host path, because a plugin
never learns where anything lives:

- `$PROJECT` — the directory holding the open project file;
- `$PLUGIN_DATA` — the plugin's own private directory.

`$PROJECT/footage` is a root; `/home/ada/footage` is not. A root asked for in
both `fs_read` and `fs_write` becomes one read-write preopen, not two, and the
guest sees it at a stable guest path (`/project`).

Three steps enforce it: **expansion** turns a declared root into a host path plus
the guest path, **resolution** turns the approved block into the WASI preopen
list plus the two boolean gates, and **gating** covers what the host does *on
behalf of* a plugin, where WASI's own preopen check does not apply — every host
function that reaches outside the sandbox authorizes first. Every refusal is
`plugin.capability_denied`.

Approval is the other half. What was granted at install is recorded by plugin id
and stamped with a digest of the whole manifest, so a plugin that later rewrites
its `plugin.toml` — to ask for the network, to add a world, to add an MCP tool —
comes back as *changed* and must be approved again before it loads. Editing the
manifest of an installed plugin is therefore a deliberate act, not a silent one.

Ask for nothing you do not need. Editing the project needs **no capability at
all**: the Command API is the plugin's own import, not a resource it borrows.
`plugins/cut-silence` asks only for `fs_read = ["$PROJECT"]` — to measure the
media — and `plugins/color` only for `shaders = true`.

## The developer loop

Scaffold, build, install, test — and then keep the editor running while you
iterate (docs/PLAN.md §6.4).

```sh
# 1. Scaffold: a manifest, a CLAUDE.md, a source template and a fixture project
subordinate-cli plugin new --world command cut-silence --output ~/src

# 2. Build the component
cd ~/src/cut-silence
cargo build --release --target wasm32-wasip2

# 3. Install it as a dev install, so edits reload instead of reinstalling
subordinate-cli plugin install ./target/wasm32-wasip2/release/cut_silence.wasm --dev

# 4. Prove it works, headlessly
subordinate-cli plugin test com.example.cut-silence
```

`plugin new` takes the crate name as its one positional argument, writes it under
`--output` (the working directory by default), and defaults the plugin id to
`com.example.<name>` unless `--id` is given. It has templates for the `command`, `effect`, `analyzer` and
`mcp-tools` worlds; asking for one that has no template is refused with
`core.invalid_argument` listing the ones that do. `plugin install` takes either
a built `.wasm` — finding the `plugin.toml` beside it or above it — or a plugin
directory holding a manifest and one component.

The other subcommands are `plugin list`, `plugin reload`, `plugin enable`,
`plugin disable` and `plugin remove`. Every one of them is also a Command API
method and therefore an MCP tool, which is how an agent runs this loop from a
conversation without a shell; the one step that is not a tool is `cargo build`,
because the editor deliberately does not run compilers on an agent's behalf.

### Where plugins live

Two directories, and a plugin is a directory holding a `plugin.toml`:

- the **user** directory, where `plugin install` puts everything by default —
  `$XDG_DATA_HOME/subordinate/plugins` on Linux,
  `~/Library/Application Support/Subordinate/plugins` on macOS,
  `%APPDATA%\Subordinate\plugins` on Windows;
- the **project-local** directory, `.subordinate/plugins` beside the project
  file, which travels with the edit so a project can pin the plugins it needs.

Project-local wins an id conflict: the project's copy loads and the user's is
reported as shadowed and logged, rather than silently disappearing. A broken
`plugin.toml` never stops the rest — it becomes one load-failure row in the
scan and the other plugins load as usual. Whether a plugin is enabled is the
user's choice, not the plugin's, so it is kept outside the plugin directory in a
small `plugins.json` in the user directory listing the ids that are switched off.

### Hot reload

A `--dev` install is an ordinary plugin directory whose contents point back at
the source tree instead of being a snapshot of it, with a marker file recording
where those sources are. The watcher polls their fingerprint — length and
modification time — rather than depending on a platform notification API, so it
behaves identically on all three OSes, and a change is acted on only once the
fingerprint has held still for one poll, which is what keeps a half-written
`.wasm` from being loaded. A rebuild is picked up within about 400 ms.

A reload runs the host's loader again and, **only if it succeeds**, replaces
that plugin's commands, effect declarations and tool catalogue. Nothing else is
touched: the engine, the open project and the undo stack are not part of a
reload, and a reload that fails leaves the previous version registered and
running, records the failure for the plugins panel, and returns the error to
whoever asked.

Symlinks are not available to every process on Windows, so a dev install falls
back to copying and re-copies changed sources before each reload. The reload
path itself is identical.

## Testing

`subordinate-cli plugin test <id>` is the answer to "does it work?" as **data**,
not as a screenshot. It loads the component, serves it the real Command API over
a fixture project, exercises every world the manifest declares, and reports
individually named checks. The host it sees is the real one: Command API calls
dispatch into a live engine, so a plugin that edits the fixture edits it through
the same undoable commands the GUI uses. Nothing is granted that the manifest did
not ask for — no preopened directory, no socket.

What each world is checked for:

| World | Check |
| --- | --- |
| `command` | The plugin runs against the fixture, must answer JSON, and whatever it changed must be undoable |
| `effect` | The declaration is lifted into the compositor's types and a frame is rendered at the defaults, so a shader that does not compile fails here rather than at the next composite |
| `analyzer` | `analyze` runs over the fixture's media and its findings are lifted into the model, so a marker at an impossible time is caught |
| `mcp-tools` | The exports are checked against the manifest and each tool is called with arguments its own schema accepts |

A failure is never a panic and rarely an error return: a plugin that traps, runs
out of fuel or returns an error is **reported**, with its stable code and hint,
beside the checks that passed — which is what makes the report worth printing
whichever way the run went.

Test the parts that are not the WIT on the host triple, too. Each first-party
plugin keeps a one-package workspace of its own, because it targets
`wasm32-wasip2` and must stay out of the host workspace's build and lint graph,
so both commands run from the plugin's own directory:

```sh
cargo build --release --target wasm32-wasip2   # the component
cargo test                                     # the parameter table, the maths, on the host
```

## Limits, and what happens when one fires

Every plugin call runs inside limits the host sets before the guest gets
control:

- **Fuel** counts executed instructions. It is deterministic — the same plugin on
  the same input stops in the same place — which is what a test or a
  reproducible export wants. It is the per-call CPU budget.
- **An epoch deadline** is wall clock, and catches what fuel cannot: a guest
  parked in a long host call, or one whose instruction budget is generous but
  whose work is slow.
- **A memory ceiling** caps linear memory, tables and how many core instances one
  component may create. A guest that asks for more sees `memory.grow` fail; the
  host process is never the one that runs out.

When a limit fires the call comes back as `plugin.fuel_exhausted`,
`plugin.deadline_exceeded` or `plugin.trapped`. The store that call ran in is
finished; nothing else is. The engine, the compiled components and every other
plugin's store are untouched, so one runaway plugin is reported and the rest keep
working.

The host keeps a bounded number of warm instances per plugin for the hot paths —
an effect's `describe` after a parameter edit, a plugin command from a menu, a
tool call from an agent — and re-arms fuel and the deadline on every checkout. A
call that ended in a termination is never reused.

## Errors

A plugin failure is never just a log line. It is a `SubError` with a stable
`plugin.*` code, a one-line message, a `wit` detail naming the interface item it
belongs to (`subordinate:plugin/audio-effect.process`) and a `hint` saying what
would fix it. The same catalogue is published to plugin authors as
`subordinate_sdk::errors`, and a parity test keeps the two in step, so the codes
an author reads are exactly the codes the host raises.

Errors a plugin *returns* follow the same shape: a code of two or more
dot-separated lowercase segments, in the plugin's own namespace
(`com.example.no_such_take`). A code that is not in that form is replaced with
`plugin.invalid_error_code` and the original is kept in a `plugin_code` detail.

## Licensing

The SDK (`sdk/`) and the interface definitions (`wit/`) are MIT OR Apache-2.0,
so a plugin built against them may use any licence, including closed source
(decision-2). The first-party plugins in `plugins/` are under the same permissive
pair on purpose: their code is meant to be copied.

## Where to look next

- [docs/mcp-guide.md](mcp-guide.md) — driving the editor, and this whole loop,
  from an agent.
- [docs/agent-runbook.md](agent-runbook.md) — one prompt to a working plugin,
  end to end.
- `wit/subordinate-plugin.wit` and `wit/README.md` — the interface itself.
- `plugins/color`, `plugins/gain`, `plugins/cut-silence`, `plugins/otio` — the
  worked examples, each with its own `CLAUDE.md` or `README.md`.
- `docs/schema/plugin-manifest.json` — the manifest schema.
- [docs/DEVELOPMENT.md](DEVELOPMENT.md) — toolchain, GStreamer and fixtures.
