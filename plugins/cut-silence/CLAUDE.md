# Cut Silence

The first-party worked example, and the reference the `subordinate-cli plugin new`
templates are modelled on. It finds the silent spans of a media item and ripples
them out of the timeline, in two halves:

- **`analyze`** — the `analyzer` world. Reads one media item, measures it, and
  reports silent spans in *media* time. It edits nothing.
- **`run`** — the `command` world. Reads those findings back and removes them
  with `clip.split` and `clip.ripple_delete`, inside one undo group.

That split is the architecture in miniature: looking is re-runnable and
reviewable, editing is an undoable command, and nothing crosses between them
except findings the host stored on the media item.

- Plugin id: `com.subordinate.cut-silence` — the install directory, the log tag
  and the capability grant all key on it.
- Bindings: raw `wit_bindgen::generate!` against a world of this plugin's own
  (see below). A plugin implementing a single stock world should use
  `subordinate-sdk` with that world's feature instead; it is less code.
- Target: `wasm32-wasip2`. The Rust toolchain emits a component directly, so
  there is no `cargo component` or `wasm-tools` step.

## The interface

```rust
fn analyze(media: MediaId, options: String) -> Result<AnalysisResult>
fn run(project: ProjectId, args: String) -> Result<String>
```

`analyze` is handed an identifier, never a path: a sandboxed plugin never learns
where the project's media lives, so it asks the host (`project.get`) and reads
the file through the `$PROJECT` root the user approved. `options` is a JSON
object of this plugin's own schema:

| key | default | meaning |
| --- | --- | --- |
| `threshold_db` | `-50.0` | at or below this level a window is silent |
| `window_seconds` | `0.02` | the RMS window |
| `minimum_seconds` | `0.5` | the shortest silence worth cutting |
| `padding_seconds` | `0.1` | air left at each end of a cut |

`run` takes `sequence`, `clips` (the selection — the host owns it and passes it
in, because a plugin holds no editor state), `analyzer`, `label` and `dry_run`,
all optional, and answers a JSON report: how many cuts were planned, how many
were applied, and how much time came out. An unknown key in either document is a
`core.invalid_argument` error rather than a silently ignored line.

## One component, two worlds

A plugin is one component and a component has one world, but this plugin needs
an export from each of two. `wit/cut-silence.wit` therefore declares a world of
its own that is the union: the same two export signatures, drawn from the host's
own interfaces, with `command-api` imported once. `plugin.toml` then honestly
declares `worlds = ["analyzer", "command"]`, and the host instantiates the one
`plugin.wasm` against either set of bindings.

The one thing it deliberately does not import is `analysis-host`, the analyzer
world's progress and cancellation channel: a component only instantiates when
every import it declares is in the linker, and the host's `command` world links
`command-api` alone. The cost is that one `analyze` call reports no progress and
cannot be cancelled before it returns; that is affordable only because this
analyzer's work is bounded. **An analyzer that thinks for minutes should be its
own component, implement the stock `analyzer` world, and import `analysis-host`.**

## Host imports

`command-api`, and nothing else. Both halves reach the project through it:
`run-command` applies one undoable command, `query` reads without mutating, and
`sequences`, `tracks` and `clips` are typed accessors that save parsing the
project document. `log` writes to the host's tracing subscriber, tagged with the
plugin id.

Times cross as `rational-time` — a tick count and an exact fractional rate —
never as seconds in a float. `src/plan.rs` is where that matters: mapping a
silence in media time onto sequence ticks is done by multiplying by both rates
as fractions in `i128`, rounding the start up and the end down so a cut never
eats a tick of audible material it only partly covers.

Everything returns `Result`, whose error carries a stable `code`, a one-line
message and details naming what it belongs to and a hint. Match on a code, never
on a message.

## Capabilities

```toml
[capabilities]
fs_read = ["$PROJECT"]
```

The analyzer reads the media file, so it asks for the project root read-only and
the user approves that at install time; the host mounts it at `/project` inside
the sandbox. Nothing else is requested, and an unrequested capability is a
denied one — the edit itself needs none, because it goes through the Command
API.

A sandbox that granted no filesystem is not a failure case worth stopping for:
`analyze` then reports an analysis with no ranges and `status = "unread"` in its
metadata, saying why. An analysis is a finding, and "there was nothing here I
could read" is one the host can store, show and hand to an agent.

## The test contract

```
cargo build --release --target wasm32-wasip2
cargo test                       # the decoder, the detector and the planner
subordinate-cli plugin install ./target/wasm32-wasip2/release/subordinate_plugin_cut_silence.wasm --dev
subordinate-cli plugin test com.subordinate.cut-silence
```

The interesting parts of this plugin are three plain Rust modules with no WIT in
them, and that is deliberate: `wav.rs` reads the sound, `silence.rs` decides what
is silent and `plan.rs` does the timeline arithmetic, so all three are unit-
tested on the host triple by `cargo test`, with no component, no host and no
audio hardware. `tests/cut_silence.rs` joins them up: it builds a `.wav` with two
pauses in it and asserts the cuts that come out the far end.

What is left in `lib.rs` is glue — JSON in, Command API calls out — and that is
what `plugin test` exercises. `fixture/fixture.sub` is this plugin's fixture
project: one 25 fps sequence, one audio clip, and a media item carrying a
`silence` analysis already stored, as an analyzer run would have left it. So the
harness run really cuts: two splits and two ripple deletes turn one clip into
three, and the harness's `undoable` check undoes all four with one undo, which is
the whole point of the group. (The media file itself is not committed — fixtures
are generated, not stored — so `analyze` reports `status = "unread"` there, and
the sandbox grants no filesystem anyway.)

Structure a plugin the same way and most of it stays testable in a second.

## House rules

- Never hold the project model: every change is a Command API call, so it lands
  on the host's undo stack. A multi-step edit opens `edit.begin_group` and
  commits it, so the user undoes the whole thing in one step — and aborts the
  group on any failure, so a half-applied cut is never left behind.
- Never compute timeline positions in floating point. `RationalTime` is exact;
  seconds are not. Floats belong in the audio measurement and nowhere else.
- Keep `plugin.toml` truthful: the host refuses anything it declares that the
  component does not export, and an undeclared capability is a denied one.
- Keep the label vocabulary small and stable. `silence` is the contract between
  the two halves of this plugin; renaming it would orphan every analysis already
  stored in a project.
