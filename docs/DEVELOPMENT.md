# Development setup

Subordinate is a Rust workspace. Media I/O is built on GStreamer, so a
GStreamer development install is required to build `sub-media` and everything
above it. CI pins **GStreamer 1.28.x** on every OS (see `GST_VERSION` in
`.github/workflows/ci.yml`); match that minor version locally.

## Rust

`rust-toolchain.toml` pins the toolchain (currently 1.95.0 with rustfmt and
clippy). `rustup` picks it up automatically. The floor comes from the GUI
stack: egui/eframe 0.36 declares `rust-version = 1.95`, so the pin cannot go
below it while PLAN.md §3 mandates that version.

```
cargo build --workspace --all-targets
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

All four must pass before a PR. Clippy runs with `all` and `pedantic` as
warnings and `-D warnings` in CI; product names that should not be backticked
in doc comments are listed in `clippy.toml`.

## GStreamer

### Linux

Ubuntu 26.04 (and Debian 14) ship GStreamer 1.28 in apt, which is what CI uses:

```
sudo apt-get install -y libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  libgstreamer-plugins-bad1.0-dev gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
  gstreamer1.0-plugins-ugly gstreamer1.0-libav gstreamer1.0-tools gstreamer1.0-x
```

Ubuntu 24.04 ships 1.24, which builds the crates but lacks the newer hardware
plugins; prefer 26.04, Fedora 44+, Arch, or Homebrew on Linux (`brew install
gstreamer`, then `export PKG_CONFIG_PATH=$(brew --prefix)/lib/pkgconfig`).

Hardware notes: the `va` plugin (AMD and Intel VA-API encode) is not packaged
by Ubuntu; `nvcodec` is in `gstreamer1.0-plugins-bad` and needs the NVIDIA
driver at runtime. `gstreamer-vaapi` was removed upstream in 1.28.

### Windows

Download the official MSVC x86_64 installer for the pinned version from
`https://gstreamer.freedesktop.org/data/pkg/windows/<version>/msvc/` and run
it, or silently:

```
gstreamer-1.0-msvc-x86_64-1.28.6.exe /VERYSILENT /NORESTART /TYPE=devel /ALLUSERS /DIR=C:\gstreamer\1.0\msvc_x86_64
```

Then set (the 1.28 installer does not reliably set these):

```
GSTREAMER_1_0_ROOT_MSVC_X86_64=C:\gstreamer\1.0\msvc_x86_64
PKG_CONFIG_PATH=C:\gstreamer\1.0\msvc_x86_64\lib\pkgconfig
PATH=C:\gstreamer\1.0\msvc_x86_64\bin;%PATH%
```

GStreamer's own `pkg-config.exe` must come first on `PATH`. The official
Windows build includes `nvcodec`, `amfcodec` and `mediafoundation`.

### macOS

Install both the runtime and devel packages from
`https://gstreamer.freedesktop.org/data/pkg/osx/<version>/`:

```
sudo installer -pkg gstreamer-1.0-1.28.6-universal.pkg -target /
sudo installer -pkg gstreamer-1.0-devel-1.28.6-universal.pkg -target /
export PKG_CONFIG_PATH=/Library/Frameworks/GStreamer.framework/Versions/1.0/lib/pkgconfig
export PATH=/Library/Frameworks/GStreamer.framework/Versions/1.0/bin:$PATH
```

The framework's dylibs use `@rpath` install names; `.cargo/config.toml` in
the repo adds the framework rpath for Apple targets so test binaries load.

Homebrew's `gstreamer` formula also works but cannot be pinned to a patch
release. The macOS build includes `applemedia` (`vtenc_h264`).

### Check the install

```
gst-inspect-1.0 --version
pkg-config --modversion gstreamer-1.0
gst-inspect-1.0 --exists x264enc && echo x264enc present
```

CI prints the presence of every hardware encoder element in its "GStreamer
diagnostics" step. Hosted runners have no GPU, so hardware elements register
but cannot encode there; hardware verification tasks are manual.

The editor answers the same question about the machine it is running on:

```
cargo run -p subordinate-cli -- diag            # JSON report
cargo run -p subordinate-cli -- diag --compact  # the same report on one line
```

`diag` walks the GStreamer registry for every decoder and encoder the editor
can use, grouped by vendor (`nvcodec`, `va`, `amf`, `vtenc`, `mf`, `x264`),
with the plugin and version behind each element. A vendor that should be
present on the current platform but is missing elements carries a `hint`
naming the install step from this file. The GUI shows the same report in its
"Hardware diagnostics" panel.

## Working on a project headlessly

`subordinate-cli` loads, saves and inspects a project without a GUI, and
serves the Command API for anything that wants to drive one (PLAN.md §7):

```
cargo run -p subordinate-cli -- new cut.sub --name "Doc cut"  # one Main sequence, V1 and A1
cargo run -p subordinate-cli -- open cut.sub                  # schema version, migrations, offline media
cargo run -p subordinate-cli -- inspect cut.sub               # sequences, tracks, clips, exact times
cargo run -p subordinate-cli -- save cut.sub --output copy.sub
cargo run -p subordinate-cli -- serve --instance default      # the Command API socket, no window
```

Every subcommand prints JSON, indented by default and on one line with
`--compact`; a failure prints a JSON `SubError` on stderr and exits non-zero.
`new` builds its starter sequence through the same undoable commands the GUI
uses, and the file it writes is deterministic text, so a project diffs in git.
Times are reported as an exact unit count and rate with a timecode alongside,
never as a float.

`serve` prints one readiness line naming the transport, the address and the
lock file as soon as the endpoint is bound, then serves until its stdin
reaches end of file (Ctrl-D in a terminal). That is how `subordinate-mcp`
launches an editor when no GUI is running and knows when to connect.

## Driving the editor from an agent (MCP)

This section is the setup; [docs/mcp-guide.md](mcp-guide.md) is the whole agent
surface — every tool family, the resources, the error codes and the runbook.

`subordinate-mcp` speaks the Model Context Protocol on stdin and stdout and
forwards every tool call to the Command API (PLAN.md §7). It holds no state of
its own: it connects to the editor the socket lock file advertises, and starts
`subordinate-cli serve` when no editor is running — so an agent can work
whether or not a window is open, and the headless server it started stops with
it.

Its tools are generated from `docs/schema/command-api.json`, one per Command
API method, with the same descriptions and parameter schemas. Method names are
dotted and MCP tool names may not be, so each dot becomes an underscore:
`clip.trim_in` is the tool `clip_trim_in`, and the tool's title is the method
name unchanged. Nothing is hand-written, so a new command is a new tool as soon
as the schema is re-exported (`cargo run -p subordinate-cli -- schema`).

Register it with Claude Code by copying `docs/examples/mcp.json` into the
project's `.mcp.json` (or merging the `subordinate` entry into the one that is
there):

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

Build it first with `cargo build --release -p subordinate-mcp`, or point
`command` at an installed binary. The environment it reads:

| Variable | What it does |
| --- | --- |
| `SUBORDINATE_INSTANCE` | Which editor instance to reach; defaults to `default` |
| `SUBORDINATE_ENDPOINT_DIR` | Where the socket and lock file live, overriding the per-user default |
| `SUBORDINATE_CLI` | The `subordinate-cli` to launch, when it is not the one beside `subordinate-mcp` |
| `SUBORDINATE_MCP_NO_LAUNCH` | `1` to fail with `command.not_running` rather than start a headless server |
| `SUBORDINATE_LOG` | The log filter; diagnostics go to stderr, because stdout is the protocol |

A tool call that the engine rejects comes back as a tool error whose content is
the usual JSON `SubError`, code and all, rather than a protocol error, so the
agent can read the reason and try something else.

## Test media fixtures

Media tests need deterministic sample files, and binaries are never committed.
`scripts/gen-fixtures.sh` (`scripts/gen-fixtures.ps1` on Windows) synthesises
them with `gst-launch-1.0` into `fixtures/`, which is gitignored:

```
./scripts/gen-fixtures.sh            # everything except the two long-GOP clips
./scripts/gen-fixtures.sh --long     # add the 10-minute long-GOP clip
./scripts/gen-fixtures.sh --hour     # add the one-hour long-GOP clip
./scripts/gen-fixtures.sh --list     # show the catalogue
./scripts/gen-fixtures.sh --dry-run  # print the pipelines without running them
```

The catalogue is 1080p and 4K H.264 colour bars with a burnt-in timecode, a
29.97 drop-frame clip, a variable-frame-rate clip, a 10-minute long-GOP clip,
a one-hour long-GOP clip, and 48 kHz stereo audio-only files in WAV, FLAC,
MP3, AAC (in MP4) and Ogg Vorbis. Existing files are kept unless `--force` is
given, so re-running the script is cheap.

The two long-GOP clips are behind their own switches because they cost real
time to encode: minutes for the ten-minute clip, a quarter of an hour and a
few hundred megabytes for the one-hour clip, which only
`subordinate-bench --proxy` needs (docs/PERFORMANCE.md).

The lossy audio fixtures need `lamemp3enc` (plugins-ugly), `avenc_aac`
(libav) and `vorbisenc` with `oggmux` (plugins-base). Where one of those is
not installed the script skips that fixture and marks it ungenerated instead
of failing, so a minimal install still produces a usable fixture set.

Alongside them the script writes `fixtures/manifest.json`, recording each
fixture's name, kind, dimensions, exact duration in nanoseconds, frame rate as
an exact rational, whether it is variable-frame-rate, and whether its codec is
lossy. `duration_ns` is the authored length: a lossy encoder adds priming and
padding, so those files match it only to within a few tens of milliseconds, and
tests give them a tolerance while holding the lossless ones exact. Fixtures the
run skipped stay in the manifest with `"generated": false`.

Tests locate fixtures through the `sub-test-support` crate rather than by path:

```rust
let clip = sub_test_support::fixture("bars_1080p_h264.mp4")?;      // Result
let long = sub_test_support::try_fixture("longgop_720p_10min.mp4"); // Option
```

`SUB_FIXTURES_DIR` overrides the directory; otherwise it is `fixtures/` at the
workspace root. CI generates the small fixtures before building, so the media
tests have their inputs on every runner.

## Reference plugins

[docs/plugin-guide.md](plugin-guide.md) is the author's guide: every world, the
manifest, the capability model, the developer loop and the test harness. What
follows is where the worked examples live and how to build them.

`plugins/` holds the first-party plugins. They are worked examples as much as
they are shipped code: one per world that is worth a template, licensed
MIT OR Apache-2.0 so their code may be copied into a plugin under any licence,
and each with its own `CLAUDE.md` for whoever — human or agent — edits one
next.

| Plugin | World | What it shows |
| --- | --- | --- |
| `plugins/gain` | `audio-effect` | Block processing with a real-time budget: `process()` reuses its input buffer and holds a smoothing ramp between calls, and nothing in it locks or allocates. |
| `plugins/color` | `effect` | The **effect template**: a tint, an exposure and a saturation declared as parameters plus one WGSL shader the host compiles, caches and runs (decision-6). |

Each keeps a one-package workspace of its own, because it targets
`wasm32-wasip2` and must stay out of the host workspace's build and lint graph.
So they are built and tested from their own directory:

```
cd plugins/color
cargo build --release --target wasm32-wasip2   # the component
cargo test                                     # the parameter table and the grade, on the host triple
```

The effect template splits deliberately into three files: `src/grade.rs` (the
parameter table and a CPU reference for the grade, with no WIT in it),
`src/effect.wgsl` (the shader) and `src/lib.rs` (the lift from the first into
the `effect-desc` that `describe` returns). That split is what lets the host's
golden test, `crates/sub-render/tests/color_plugin_golden.rs`, pull in the
plugin's own shader with `include_str!` and its own parameter table with
`#[path]` — no dependency edge into the plugin's workspace — render the effect
through the compositor and compare the readback with the plugin's own
reference, in linear light. Rename a parameter or reorder the grade on one side
only and that test fails.

## Benchmarks

`bins/subordinate-bench` measures decode-to-texture latency and sustained
playback and scrub frame rates on the 1080p and 4K fixtures, writes a JSON
report and prints a summary:

```
cargo run --release -p subordinate-bench          # target/bench/perf.json
```

It needs the fixtures above; without them (or without a wgpu adapter) it
records skipped or decode-only scenarios instead of failing. CI runs it on
Linux and uploads the report as the `perf-report` artifact. Baselines and how
to read them: `docs/PERFORMANCE.md`.

## Errors and logging

Both conventions live in `sub-core`, the lowest crate in the workspace. Every
other crate may depend on it; it depends on nothing of ours.

### Errors are `SubError`

Anything that can reach the Command API, the MCP bridge or a plugin fails with a
`SubError`, because agents have to match on failures rather than read prose
(PLAN §6.4). Its JSON shape is stable:

```json
{
  "code": "media.decode_failed",
  "message": "could not decode frame 42",
  "details": { "path": "/tmp/a.mp4", "pts_ns": 1400000000 },
  "cause": "gstreamer: pipeline failed to start: no element \"nvh264dec\""
}
```

- `code` — a stable, machine-readable `domain.reason` string.
- `message` — one line for a human, lowercase, no trailing period.
- `details` — optional map of machine-readable specifics: field paths, ids,
  file names. Omitted when empty.
- `cause` — the lower-level error's whole `source()` chain, flattened to a
  string at construction time so `SubError` stays `Clone`, `Send` and
  serializable. Omitted when absent.

Use `SubResult<T>` (`Result<T, SubError>`) for fallible public functions.

### Error codes

Codes are two or more dot-separated segments of `[a-z0-9_]`: a domain (usually
the crate — `model`, `media`, `render`, `export`, `plugin`, `command`) and a
reason. `ErrorCode::from_static` builds them in `const` context;
`ErrorCode::parse` validates one built at runtime (a plugin manifest, a
deserialized message) and deserialization rejects a malformed one.

Each crate declares its own codes as constants in one `codes` module, next to
the shared ones in `sub_core::codes` (`core.invalid_argument`, `core.not_found`,
`core.invalid_state`, `core.unimplemented`, `core.cancelled`, `core.timeout`,
`core.io`, `core.internal`, `core.logging_init`):

```rust
pub mod codes {
    use sub_core::ErrorCode;

    /// The file exists but no decoder could handle it.
    pub const UNSUPPORTED_CODEC: ErrorCode = ErrorCode::from_static("media.unsupported_codec");
}
```

**When to add a code.** Add one when a caller could reasonably react
differently to this failure than to its neighbours — retry, relink, prompt,
fall back, give up. If the only sensible reaction is the same as an existing
code's, reuse that code and put the specifics in `details`. A code is a public
contract: once it ships it is never renamed and never given a new meaning; a
changed meaning is a new code. Document every constant with the situation it
names, and keep the domain equal to the crate that owns it.

### Wrapping lower-level errors

Never surface a foreign error type (`std::io::Error`, `serde_json::Error`, a
GStreamer or wasmtime error) across a crate boundary, and never discard it
either. Wrap it at the boundary where you know what the caller was trying to
do, with `ResultExt::sub_context` (or `sub_context_with` when the message
allocates):

```rust
use sub_core::{ResultExt, SubError, SubResult, codes};

let text = std::fs::read_to_string(path)
    .sub_context_with(codes::IO, || format!("could not read project {}", path.display()))?;

let project: Project = serde_json::from_str(&text)
    .map_err(|err| {
        SubError::wrap(codes::PARSE_FAILED, "project file is not valid JSON", &err)
            .with_detail("path", path.display().to_string())
            .with_detail("line", err.line())
    })?;
```

Rules of thumb: wrap once, at the crate boundary, not at every call site; put
the caller's intent in `message` and the machine-usable facts in `details`;
`code` describes the situation, not the library that produced it. Inside a
crate, a private `thiserror` enum is fine — convert it to `SubError` on the way
out. `core.internal` is for broken invariants (i.e. bugs), never for a user's
bad input.

### Logging

Logging is `tracing`. Every binary calls `sub_core::logging::init("info")` as
its first statement and exits non-zero if that fails. Logs always go to
**stderr**: `subordinate-mcp` speaks its protocol on stdout and the CLI prints
results there.

| Variable | Effect |
|---|---|
| `SUBORDINATE_LOG` | env-filter directive, e.g. `info,sub_media=debug` |
| `RUST_LOG` | same, used only when `SUBORDINATE_LOG` is unset |
| `SUBORDINATE_LOG_FORMAT` | `text` (default) or `json` — one JSON object per event |
| `NO_COLOR` | disables colour in text output (colour is also off when stderr is not a terminal) |

Use structured fields, not formatted strings: `tracing::warn!(clip_id = %id,
"clip is offline")`, not `warn!("clip {id} is offline")`. Spans go around
units of work that cross threads (a decode request, a command, an export job).
Log a `SubError` by its `Display` (`[code] message: cause`) or its fields; do
not log and return the same error — the caller decides.

Nothing in the audio callback logs, allocates or locks.

## Project files

A project is `name.sub` (JSON). Its sidecar directory `name.sub.d/` sits beside
the project file and holds thumbnails, waveforms, proxies, PTS indexes
(`<content-hash>.ptsindex.json`, written by `sub_media::PtsIndex`) and autosave
snapshots. The naming is mechanical: append `.d` to the project file name, so
`doc-cut.sub` owns `doc-cut.sub.d/`. Everything in it is derived data and can
be deleted at the cost of regenerating it, so it is never committed: the
repository `.gitignore` carries

```gitignore
*.sub.d/
```

Add the same line to any repository that keeps `.sub` projects under version
control. See docs/PLAN.md §5.6.

### Autosave and snapshot history

`sub_edit::autosave` keeps `name.sub.d/autosave/`, one complete `.sub` file per
snapshot named `autosave-<unix millis>-r<revision>.sub`, so a plain name sort is
a time sort and every entry can be opened by the ordinary loader. Snapshots are
written to a `.tmp` and renamed into place, and the newest K (`AutosaveConfig`,
ten by default) are kept.

`Autosave::spawn(engine.handle(), store, config)` starts the worker: it
subscribes to the engine's change events and, at most once per interval and only
when the revision moved, writes an `EngineHandle::snapshot` `Arc` on its own
thread — never on the UI or engine thread. Stopping it flushes whatever the last
interval did not reach. A write that fails (a read-only or full sidecar) is
reported in `AutosaveStatus::last_error` and retried, never raised, because it
must not stop an edit.

Opening a project asks `autosave::check_for_recovery(path)` first. A snapshot
more than `RECOVERY_TOLERANCE` newer than the project file means the last
session ended without saving; the returned `Recovery` carries the prompt text
and the two answers, `recover()` (load the snapshot instead of the file) and
`discard()` (drop the history and open the file). `SnapshotStore::list()` is the
restore menu: newest first, each entry a `Snapshot::label()` and a `load()` that
returns the project it holds. Restoring is not a `Command` — a snapshot is a
whole project, so the caller opens it with a fresh engine and history, exactly
as opening a file does.

`sub_ui::recovery` is the pair of widgets over that. `RecoveryPrompt` is the
dialog `SubordinateApp::open_project` raises when the check finds something: it
states which autosave is ahead and how much work the file on disk is missing,
and its two buttons are the two answers. `SnapshotMenu` is the restore list
behind File > Snapshot history — the kept snapshots, newest first, each labelled
with its revision and age and each restorable with a click. Both hand a
`RecoveryOutcome` back rather than editing anything; the app adopts the returned
project (`adopt_project`), which replaces the project, sequence, compositor,
viewer, scheduler and timeline panel, because a snapshot is not a mutation of
the open project. `crates/sub-ui/tests/autosave_recovery.rs` drives both over a
real sidecar directory and holds the dialog to a committed snapshot.

`crates/sub-model/tests/fixtures/sample-project.sub` is a committed sample
project (two sequences, three tracks each, clips, a crossfade, markers and two
bins). `crates/sub-model/tests/golden.rs` checks that it loads and saves
byte-identically and that the builder in that test still produces exactly those
bytes, so any change to the on-disk format fails with a line diff. When such a
change is intended, regenerate the fixture and commit its diff:

```bash
SUB_UPDATE_GOLDEN=1 cargo test -p sub-model --test golden
```

### The sample project

`examples/sample-project/demo.sub` is the ready-made project: two sequences,
clips over three tracks, a crossfade, sequence and clip markers, an applied
effect and three real CC0 clips. It is what a new user (or an agent) opens to
see the editor doing something, and what the CI render test renders.

The effect is the first-party colour grade `com.subordinate.color`
(`plugins/color`) on the wide shot. A clip stores an effect as a reference —
the plugin id plus the values that differ from the plugin's declared defaults
(`sub_model::effect::ClipEffect`) — never the parameter table or the shader,
which belong to the plugin and are read from it at load time.

The media is never committed. Fetch it first — three files, about 4.7 MB, each
pinned by byte size and SHA-256:

```bash
./scripts/get-sample-media.sh          # scripts\get-sample-media.ps1 on Windows
cargo run -p subordinate -- examples/sample-project/demo.sub
```

The files are served as assets of this project's own
[`sample-media-v1`](https://github.com/thowd22/Subordinate/releases/tag/sample-media-v1)
release and are verified against the `SHA256SUMS` published beside them as well
as the pins in the script, so a release that does not match what the project was
built against fails the fetch. They were first published on Wikimedia Commons as
CC0 1.0; those URLs stay in the script's catalogue and in `media/manifest.json`
(`origin` and `source`) for provenance, and `--upstream` (`-Upstream` in
PowerShell) fetches from them, but neither the default path nor CI contacts
Wikimedia — concurrent CI runs off shared runner egress were being answered with
HTTP 429, and no run should hang on a third-party site.

Every media path in the project is relative and forward-slashed, so it opens
without a relink on any of the three OSes, and every media item records the
content hash of the pinned download.

Two test binaries cover it. `cargo test -p sub-model --test demo_project` holds
the committed file to the builder that produced it (regenerate with
`SUB_UPDATE_GOLDEN=1 cargo test -p sub-model --test demo_project`), round-trips
it and checks the path rules; no media needed.
`cargo test -p sub-ui --test sample_project_render` is the render test: it
decodes the real media with `sub-media`, composites it through `sub-render`,
and checks that the overlay stacks, that the dissolve blends and that the grade
the project stores runs over its clip, bound against `plugins/color`'s own
declaration. It skips itself when the media has not been fetched or the machine
has no wgpu adapter. CI fetches the media (cached per OS and keyed on both fetch
scripts, as the fixtures are) before the test job, so there it really runs; a
warm cache downloads nothing at all. CI also sets `SUB_REQUIRE_SAMPLE_MEDIA=1`,
which turns the missing-media skip into a failure: a green test job on Linux,
Windows and macOS is what proves the project opens on each of them with every
media path resolved and nothing to relink. `examples/sample-project/README.md` carries the media credits.

## UI tests (egui_kittest)

**The rule: any task that touches `sub-ui` adds or updates a test here.** A
panel that gains a widget, a colour, a layout or a click gains a snapshot or an
interaction test in the same commit, or updates the one it already has. This is
also in the conventions list in `CLAUDE.md`, because it is the convention new
panels break most often.

`sub-ui` panels are tested headlessly with
[`egui_kittest`](https://docs.rs/egui_kittest) 0.36 (features `eframe`,
`snapshot`, `wgpu`). It runs a real egui pass, renders it through wgpu on a
software adapter, and exposes the frame over `AccessKit`, which buys two kinds
of test:

- **Snapshot tests** render a panel and diff the PNG against a committed
  reference in `crates/sub-ui/tests/snapshots/`.
- **Interaction tests** click and type by accessibility label
  (`harness.get_by_label("Add track").click()`) and then assert on the model.
  These need no adapter and run everywhere.

Prefer an interaction test for behaviour (what a click changes) and a snapshot
for appearance (that the panel is laid out and painted at all). Most panel
tasks want one of each, and one snapshot per panel is usually enough — a
snapshot per state multiplies the PNGs that have to be re-recorded whenever the
theme moves.

### The harness

The shared harness is **`crates/sub-ui/tests/support/mod.rs`**; pull it into a
test file with `mod support;`. It fixes the frame every panel is painted in
(800x600 logical points, one pixel per point, dark theme, wgpu renderer) and
loads the committed sample project, so every snapshot shows the same realistic
two-sequence, three-track edit:

```rust
mod support;

#[test]
fn the_inspector_matches_its_snapshot() {
    if !support::can_render() {
        return;                       // no wgpu adapter here: report and pass
    }
    let project = support::fixture_project();
    let sequence = support::fixture_sequence(&project).clone();
    let mut panel = InspectorPanel::new();
    let mut harness = support::panel_harness(|ui| panel.ui(ui, &project, &sequence));
    harness.run();
    support::snapshot(&mut harness, "inspector_panel");
}
```

- `support::builder()` / `panel_harness()` / `panel_harness_state()` — the
  frame settings; `panel_harness_state` carries the state an interaction test
  asserts on afterwards.
- `support::fixture_project()` / `fixture_sequence()` — the committed sample
  project, never a hand-built one, so a fixture change shows up as a visible
  diff.
- `support::can_render()` — guards the rendering half only. A test that merely
  drives input and asserts on the model must not ask, or it will silently stop
  running on machines with no ICD.
- `support::snapshot(&mut harness, name)` — the comparison. Name snapshots
  after the panel, in `snake_case`.

Use only these entry points, so a change to the frame size or theme moves every
snapshot together.

### Updating snapshots

```bash
cargo test -p sub-ui                          # compare against the references
UPDATE_SNAPSHOTS=1 cargo test -p sub-ui       # re-record them, then commit the diff
```

Re-record only when the change to the UI is intended, and look at the new PNGs
before committing them. `UPDATE_SNAPSHOTS=1` rewrites every reference the run
touches, so run the whole crate's tests first and re-record with a clean tree —
otherwise an unrelated regression is baked into a reference and lost.

Tolerances live in `kittest.toml` at the workspace root. `threshold = 0.6` is
the per-pixel colour distance (egui's own default, enough to cover different
wgpu backends), and `max_failed_pixels` is how many pixels may exceed it: 10 on
Linux, where the references are recorded on lavapipe, and 300 on Windows (WARP)
and macOS (Metal), whose blending rounds differently. Those numbers are far
below what a genuine regression costs, so raise them only with a reason in the
commit message.

Snapshot PNGs are committed, so keep them small: render at 800x600 or less, no
LFS. A test in `crates/sub-ui/tests/ui_harness.rs` enforces the budget — 150 KB
per snapshot and 5 MB for the directory.

### The user guide's generated blocks

`docs/user-guide.md` shows the committed snapshot PNGs as its screenshots and
carries a keyboard reference generated from the action registry in
`crates/sub-ui/src/shortcuts.rs`. `crates/sub-ui/tests/user_guide.rs` fails
when either goes stale — a screenshot that no longer exists, an action the
table is missing, an encoder the troubleshooting section does not name — so
re-record the block after adding or rebinding an action:

```bash
UPDATE_DOCS=1 cargo test -p sub-ui --test user_guide   # rewrite, then commit
```

### Looking at a failure (including from an agent)

A failing comparison writes two PNGs next to the reference:
`<name>.new.png` (what this run rendered) and `<name>.diff.png` (the pixels
that differ). Both are gitignored. Locally they are simply in
`crates/sub-ui/tests/snapshots/` — open them.

From CI they come back as an artifact. The test job uploads
`ui-snapshot-diffs-<os>` whenever it fails, and an agent with no window can
fetch and read it without leaving the terminal:

```bash
gh run list --workflow ci.yml --limit 5                       # find the failing run
gh run download <run-id> -n ui-snapshot-diffs-ubuntu-latest -D /tmp/snap
ls /tmp/snap                                                  # *.new.png, *.diff.png
```

Read the PNGs directly — an agent can open an image file, and the `.diff.png`
usually says what moved at a glance. A diff that is a handful of scattered
pixels along an edge is a backend rounding difference and belongs in
`kittest.toml`; a diff that is a solid block is a real change, and the fix is
either the code or a deliberate re-record.

### Where these run

CI installs Mesa's lavapipe on Linux; Windows uses WARP through D3D12 and macOS
has Metal, so the snapshot tests run inside the ordinary
`cargo test --workspace` on all three hosted runners at no extra cost.

**Hosted runners are where UI tests belong. GPU runners are only for checks
that need real hardware** — hardware encode and decode, driver-specific
behaviour, multi-monitor and fullscreen presentation (see "GPU CI (RunsOn)"
below). They are billed by the hour, and routing a panel snapshot through one
buys nothing: the software adapters render the same egui output. If a panel
test seems to need a GPU, the test is wrong.

## Window smoke test (Xvfb screenshots)

`egui_kittest` renders panels; it never creates a window. The window smoke test
is the other half: the real `subordinate` binary, the window eframe asks the
platform for, the wgpu adapter it picks, the dock as assembled, and the pop-out
viewer in a second window on a second monitor. It runs as two steps of the
Linux CI job — no GPU, no seat, no hourly runner — and leaves a PNG of each
monitor behind.

`scripts/ui-smoke.sh` does the work:

1. starts `Xvfb :99` with two screens joined by `+xinerama`, so the two
   screens are one desktop a window can be placed across;
2. reads the head geometry back with `xdpyinfo -ext XINERAMA` rather than
   assuming a side-by-side layout — and where a server joins both screens at
   the same origin, splits the first screen into two RandR monitors with
   `xrandr --setmonitor` instead, so there are always two monitors at two
   different origins;
3. launches `subordinate --ui-smoke --popout-position <head 1 origin>
   <sample project>`, which opens the committed
   `crates/sub-model/tests/fixtures/sample-project.sub`, pops the viewer out
   onto the second head and holds both windows up for 25 seconds (the hold is
   insurance against a capture that never happens; the script closes the app
   as soon as it has the pictures);
4. waits for the app's `ui-smoke ready` line — printed once the editor window
   *and* the pop-out have each painted a frame, so nothing is photographed
   empty — and fails if the project did not load;
5. captures the desktop once with `xwd` and crops one PNG per head.

Run it locally the same way CI does:

```bash
cargo build -p subordinate
./scripts/ui-smoke.sh --binary target/debug/subordinate
ls target/ui-smoke     # screen-0.png, screen-1.png, app.log, screens.txt
```

Needs `Xvfb`, `xdpyinfo` (x11-utils), `xwd` (x11-apps), ImageMagick and, for
the monitor-split fallback, `xrandr` (x11-xserver-utils).

From CI, the screenshots and the app log come back as `ui-smoke-<sha>`, and
the job summary lists each file with its dimensions:

```bash
gh run download <run-id> -n ui-smoke-$(git rev-parse HEAD) -D /tmp/ui-smoke
ls /tmp/ui-smoke       # screen-0.png is the editor, screen-1.png the pop-out
```

Read the PNGs directly; that is the point of the job. The artifact is uploaded
even when the step failed, because a screenshot of a broken window is the
fastest way to see what went wrong. Windows and macOS skip it: there is no
Xvfb there, and `--smoke-test` already proves the window comes up on each OS.

Two flags exist for the GPU runner (TASK-118) and are off by default here:

| Flag | What it changes |
| --- | --- |
| `--gpu` | does not set `LIBGL_ALWAYS_SOFTWARE`, so the picture is the machine's real adapter — the `adapter chosen:` line from `app.log` goes into the summary as the proof |
| `--require-popout-on-head N` | reads the pop-out window's absolute geometry back with `xwininfo -root -tree` and fails unless it lies inside head `N`, with the editor window on a different head |

Either way the run now also writes `windows.txt` (the top-level windows with
their geometry) and `monitors.txt` (`xrandr --listmonitors`), which is what
turns "the pop-out is on the second display" from a picture someone has to
squint at into an assertion.

## CI build speed

`windows-latest` used to dominate every wave of merges: run 34407960264 on
`main` spent 23 minutes in Build and 7 in Clippy, against 5 minutes for the
whole Linux job. Three things caused it, and all three are handled inside
`.github/workflows/ci.yml` only — **nothing here changes a local build**.
`cargo build`, `cargo test` and `cargo clippy` on a developer machine still use
the profiles in `Cargo.toml` exactly as written (`opt-level = 1` for workspace
crates, `3` for dependencies, full debuginfo).

- **Debuginfo.** The workflow sets `CARGO_PROFILE_DEV_DEBUG` and
  `CARGO_PROFILE_TEST_DEBUG` to `line-tables-only` as job-level environment
  variables. Full debuginfo is what makes the MSVC linker and the `.pdb`
  writes the bulk of the Windows job; line tables still give file and line
  numbers in the `RUST_BACKTRACE=1` output the tests print on failure. Because
  these are environment variables and not a profile in `Cargo.toml`, they apply
  to CI and to nothing else.
- **One compilation cache, and it is `Swatinem/rust-cache` (TASK-126).**
  sccache was introduced on Windows and macOS by TASK-125 and has since been
  removed from every OS. The two caches store the same artifacts in the same
  backend — the 10 GB per-repository Actions cache — but they store them very
  differently: rust-cache writes one large archive per OS, while sccache's
  GitHub Actions backend writes one entry per compiled object. With both
  running the repository held 1080 cache entries totalling 10.4 GB, of which
  all but two were `sccache/...` objects a few MB each. Actions evicts
  least-recently-used entries once a repository is over budget, so the large
  rust-cache archives were the first things evicted and nearly every job
  started with a cold `target/`: Linux went from about 5 minutes to 13 on a
  warm no-change rerun of 34419018398 (331 rustc invocations, 164 sccache hits
  against 167 misses), and after Linux alone was moved back to rust-cache,
  Windows hit the 40-minute timeout on rerun 34451650066 for the same reason.
  rust-cache alone is the configuration that measured 5 minutes on Linux, and
  it keeps the whole repository inside a handful of entries. The `sccache/*`
  entries left behind must be deleted once, by hand, or they keep occupying
  the budget until Actions ages them out: list them with
  `gh cache list -R thowd22/Subordinate --limit 100 --json id,key` and remove
  each id with the matching `gh cache` delete subcommand, repeating until none
  are left.
  `save-if: github.ref == 'refs/heads/main'` restricts writes to `main`: pull
  requests restore main's archive but never add an entry of their own, so the
  count stays at one archive per OS instead of one per branch per OS. Before
  adding a second compilation cache to any job, budget the Actions cache
  first — `gh api repos/thowd22/Subordinate/actions/cache/usage` reports the
  live total, and `gh cache list` shows what is filling it.
- **The workspace's own crates are cached too (TASK-126).**
  `Swatinem/rust-cache` by default strips this repository's crates out of
  `target/` before saving and keeps only third-party dependencies, so a warm
  no-change run still recompiled every workspace member and every test binary:
  184 s of Build and 48 s of Clippy inside a 10m47s Linux job on rerun
  34495825617. `cache-workspace-crates: true` keeps them, and `workspaces:`
  lists every cargo workspace in the repository. Each plugin under `plugins/`
  is a self-contained workspace with its own target directory, which the root
  entry never covered, so their CI steps rebuilt from scratch every run
  (cut-silence 30 s, color 31 s, otio 43 s on rerun 34560385094); only
  `plugins/gain` used to be listed. Keep that list equal to the set of
  `working-directory:` values the plugin steps use. Cargo's fingerprints
  still decide what is stale, so this only ever saves work; the cost is a
  larger archive per OS, which removing sccache made room for. Watch it: the
  three archives were 1.5 GB (Linux) and 1.25 GB (Windows) without the
  workspace crates, and the budget is 10 GB for the whole repository. A
  dependency bump changes the key and leaves the previous archive behind, but
  a stale key is never restored from again, so it is the least recently used
  entry and Actions evicts it first. Check
  `gh api repos/thowd22/Subordinate/actions/cache/usage` if jobs start coming
  up cold.
- **Bump `prefix-key` whenever you change *what* the archive should contain
  (TASK-126).** This is the trap that made the previous bullet a no-op for
  several days. Actions cache entries are immutable, and `Swatinem/rust-cache`
  refuses to re-save a key it restored as a full match — its post step prints
  `Cache up-to-date.` — while its key is derived from the toolchain, the
  lockfile and the environment and **not** from the action's own inputs. So
  after `cache-workspace-crates: true` was added, every run went on restoring
  the archive written before the option existed, with the workspace crates
  already stripped out, and went on recompiling all twenty-odd members: the
  warm no-change rerun of run 34560385094 (attempt 2) still showed Build 184 s
  and Test 203 s on ubuntu-26.04, 12m15s for the job. The key is now prefixed
  `v1-rust`. Any future change to the shape of the cached contents — another
  `workspaces` entry, a different `cache-*` toggle — needs the same bump, and
  the run to measure is the *second* one after it lands: the first writes the
  new archive.
- **Generated fixtures are cached (TASK-126).** `scripts/gen-fixtures.sh` is
  pure x264 encode time — 16 s for the short set, another 129 s for the
  10-minute long-GOP clip the A/V sync harness needs — and its output depends
  on nothing but the script and the GStreamer version. One `actions/cache`
  entry per OS, keyed on `runner.os`, `GST_VERSION` and
  `hashFiles('scripts/gen-fixtures.sh')`, holds both sets; the save half is
  gated on `main` and skipped on an exact hit, so pull requests restore and
  never write. A partial restore is safe: the script keeps whatever files are
  already on disk, regenerates only what is missing and always rewrites
  `manifest.json`. Change the catalogue or a pipeline and the hash changes,
  which regenerates everything once.
- **Every cargo step selects the same packages (TASK-131).** This is the one
  that cost the most and is the easiest to reintroduce. Cargo unifies features
  across the packages a command *selects*, so two commands with different
  selections compile the shared dependencies differently and each one
  invalidates the other's artifacts — no cache and no wrapper can help, the
  fingerprints genuinely differ. The job used to run four selections in a row:
  `cargo build --workspace --all-targets`, then `cargo test --workspace
  --exclude sub-ui`, then `cargo test -p sub-ui`, then `cargo run -p
  subordinate -- --smoke-test`. On windows-latest in run 34574614657 that was
  156 s of recompiling before the first test ran, 197 s more before the second
  and 55 s more before the smoke test, against 89 s of tests actually
  executing: 353 s of a 462 s Test step and 55 s of a 57 s smoke step, with the
  same shape on Linux and macOS. The job now runs one
  `cargo test --workspace -- --test-threads=1` — the same selection as the
  Build step, so it compiles nothing — and the smoke tests invoke
  `target/debug/subordinate` directly instead of `cargo run`. `--test-threads=1`
  is what allows sub-ui to stop being a separate invocation: it needed serial
  execution anyway, because several `egui_kittest` snapshots rendering at once
  segfault Mesa's lavapipe on the Linux runner. It costs about a minute of lost
  parallelism in the other crates — the five slowest Windows binaries in that
  run were sub-media's `seek_fixtures` 9.3 s, sub-test-support's
  `sample_media_catalogue` 5.4 s and sub-ui's `timeline_selection` 3.8 s,
  `inspector` 3.6 s and `timeline_transition` 3.1 s, everything else under 3 s
  — and buys back six minutes. Before adding a cargo invocation to the
  job, make it select `--workspace`, or budget a full workspace recompile for
  it and for the step after it.
- **Windows Defender.** A Windows-only step adds the workspace, `~/.cargo` and
  `~/.rustup` to the Defender exclusion list, plus `rustc.exe` and `link.exe`
  as processes. Hosted runners run as administrator, so this succeeds; it is
  wrapped in `try`/`catch` and never fails the job if Microsoft changes that.

With no compiler wrapper anywhere, the signal for a cold run is the wall time
of the Build step together with the "cache hit"/"cache miss" line that
`Swatinem/rust-cache` prints in its own step. The job timeout is 40 minutes on
every OS; Windows had been raised to 60 and then to 90 as stopgaps while the
repeated recompiles above went undiagnosed. If a run ever approaches the limit
again, check the cache usage total first — a repository over the 10 GB budget
means the cache backend, not the code, is the problem — and then check whether
a new step introduced a package selection of its own.

Where the Linux job's time went on rerun 34495825617, before the two caches
above (10m47s in total): Build 184 s, long-fixture generation 129 s, Clippy
48 s, Test 45 s, benchmark 36 s, GUI smoke 33 s, apt install 33 s, rust-cache
restore 36 s, reference plugin 30 s, short fixtures 16 s, A/V sync 12 s, the
rest under 10 s each. Read those numbers off any run with
`gh api repos/thowd22/Subordinate/actions/runs/<id>/jobs` and the per-step
`started_at`/`completed_at` pairs before optimising anything — the two steps
worth attacking were not the ones the job's shape suggested.

Where it went on the warm no-change rerun of run 34560385094 (attempt 2), the
last measurement taken with the stale archive still in place (12m15s): Test
203 s, Build 184 s, rust-cache restore 61 s, apt install 43 s, the OTIO plugins
43 s, the scrub benchmark 34 s, the color plugin 31 s, cut-silence 30 s, GUI
smoke 26 s, Clippy 23 s, A/V sync 10 s, the rest under 10 s each. Both fixture
steps had disappeared entirely, which is the fixture cache working. Measure it
again once a run has written the `v1-rust` archives; a no-change rerun after
that should leave Build and the three plugin steps as near-no-ops, and the
floor for the job is then roughly the runner and toolchain setup, the apt
restore, the test run itself, and the GUI smoke plus benchmark, which is why
the original 7-minute target predates the A/V sync harness, the benchmark and
the three plugin workspaces the job has since grown.

## GPU CI (RunsOn)

Hosted GitHub runners have no GPU, so hardware encode and decode criteria run
on EC2 instances in the project's AWS account through
[RunsOn](https://runs-on.com/) (self-hosted scheduler, installed 2026-09-09).

- Stack: `runs-on` in **us-east-1**, RunsOn v3.3.0. Scheduler logs:
  CloudWatch log group `/aws/ecs/runs-on/runs-on-worker`.
- The RunsOn GitHub App is installed on `thowd22/Subordinate`; job webhooks go
  to the stack's API Gateway entry point (see the stack outputs).
- Smoke test: run the **RunsOn smoke** workflow (`workflow_dispatch`). It
  starts a 2-CPU spot Linux instance, prints instance facts and exits. Use it
  first whenever the stack or the App changes.
- GPU runners are defined by name in `.github/runs-on.yml`. Reference them as
  `runs-on: runs-on=${{ github.run_id }}/runner=<name>`:

  | Runner | Instance | Image | On-demand $/h | Status |
  | --- | --- | --- | --- | --- |
  | `gpu-nvidia-linux` | `g4dn.xlarge` (T4) | `ubuntu24-gpu-x64` | 0.526 | ready |
  | `gpu-nvidia-desktop-linux` | `g4dn.xlarge` (T4) | custom (Xorg desktop) | 0.526 | ready |
  | `gpu-amd-linux` | `g4ad.xlarge` (Radeon Pro V520) | `ubuntu26-full-x64` | 0.379 | ready |
  | `gpu-nvidia-windows` | `g4dn.xlarge` (T4) | `windows22-full-x64` | 0.752 | ready |

  There is no `gpu-amd-windows` runner and there cannot be one: AWS retired
  g4ad and offers no other AMD GPU instance type, so AMF (`amfh264enc`) has no
  cloud host at all.

  All three need the EC2 G-family vCPU quotas (`L-DB2E81BA` on-demand,
  `L-3819A6DF` spot) above zero in us-east-1.
- **The config is read from `main`, not from your branch.** For public repos
  RunsOn only reads `.github/runs-on.yml` from the default branch, so a runner
  added or changed on a feature branch is invisible and the job fails to
  launch (`InvalidAMIID.Malformed` if the old definition had a placeholder
  AMI). To test a runner definition before merging, spell its parameters out
  inline instead - `runs-on=${{ github.run_id }}/image=windows22-full-x64/family=g4dn.xlarge/spot=false`
  - and switch back to `runner=<name>` in the merge commit.
- **Windows GPU needs no custom AMI** (TASK-115). The stock
  `windows22-full-x64` image boots fine on a `g4dn`; the only thing missing is
  the NVIDIA driver, and the job installs it itself in about 100 seconds:

  - AWS publishes the driver in `s3://ec2-windows-nvidia-drivers/latest/`
    (one ~713 MB `*_grid_*_aws_swl.exe`). Fetch it over **plain anonymous
    HTTPS** (`https://ec2-windows-nvidia-drivers.s3.amazonaws.com/?list-type=2&prefix=latest/`
    to find the key, then `curl.exe` the object). Do *not* use `aws s3 cp` as
    the AWS docs suggest: the RunsOn instance role is scoped to the stack's
    own buckets and returns `AccessDenied` on `ListObjectsV2`.
  - Install with `-s -n` (silent, no reboot). It exits 0 and the driver loads
    straight away - `nvidia-smi` reports the Tesla T4 and `nvh264enc`
    registers in the same job. This is essential: a RunsOn runner is ephemeral
    and cannot survive a restart.
  - Then install GStreamer 1.28 with the same official MSVC installer recipe
    `ci.yml` uses. Both `nvh264enc` (NVENC) and `mfh264enc` (Media Foundation)
    are present afterwards.
  - Copy the `nvidia-windows` job in `.github/workflows/gpu-smoke.yml`
    verbatim for any new Windows GPU job.
- RunsOn's `*-gpu-*` images carry the NVIDIA driver and CUDA only, so the AMD
  runner uses the plain Ubuntu 26.04 image and jobs install the Mesa VA-API
  stack (`mesa-va-drivers`, `vainfo`) themselves. The GStreamer `va` plugin
  (`vah264enc`) is not a separate Ubuntu package - it ships in
  `gstreamer1.0-plugins-bad`. Note the NVIDIA GPU image is Ubuntu 24.04, whose
  apt GStreamer is 1.24, not the 1.28 pinned everywhere else.
- GPU smoke test: run the **GPU smoke** workflow (`workflow_dispatch`). It
  checks `nvidia-smi` plus `gst-inspect-1.0 --exists nvh264enc` on the NVIDIA
  Linux runner, `/dev/dri` + `vainfo` plus `vah264enc` on the AMD box, and
  `nvidia-smi` plus `nvh264enc`/`mfh264enc` on the NVIDIA Windows runner.
- Cost: the RunsOn config schema has no per-runner price cap, so hourly prices
  are recorded in comments there and every GPU job must set `timeout-minutes`.
  Runners request spot (`price-capacity-optimized`) with
  `retry: when-interrupted`; if spot capacity is unavailable, re-dispatch with
  `/spot=false` appended to the label to force on-demand. Instances are billed
  by AWS with no markup; every job gets a fresh instance that is terminated
  when the job ends.
- Measured cost of one GPU smoke run (run 34618546433, 2026-09-11): the
  Windows NVIDIA job billed about 7.5 minutes of `g4dn.xlarge` Windows
  on-demand (instance launched 15:50:54Z, job finished 15:58:22Z) - roughly
  **0.10 USD**. Of the 3m22s inside the job, 1m40s was the NVIDIA driver
  (14s download, 86s install) and 1m19s was the GStreamer installer; the
  remaining 4m06s was Windows boot and runner registration before the job
  started. Windows spot is rarely discounted, so budget the on-demand rate.
- Idle cost: the stack runs in public mode (`Private: false`, changed
  2026-09-09) so there is no NAT gateway; the only idle cost is the small
  Fargate scheduler (about 9 USD/month). Do not enable private mode: the
  NAT gateway alone costs about 33 USD/month idle. Every job's instance and
  its root EBS volume are deleted when the job ends.

Useful commands:

```
aws cloudformation describe-stacks --stack-name runs-on --region us-east-1
aws logs tail /aws/ecs/runs-on/runs-on-worker --region us-east-1 --since 10m
aws ec2 describe-instances --region us-east-1 \
  --query "Reservations[].Instances[].[InstanceId,InstanceType,State.Name]" --output text
```

## Desktop GPU runner (Linux)

`gpu-nvidia-linux` boots into a console: there is no X server, no window
manager and no seat, so nothing there can open a window, click in one or
photograph it. `gpu-nvidia-desktop-linux` is the same `g4dn.xlarge` T4
instance booted into a real Xorg session on the NVIDIA driver, from a custom
AMI built by **`infra/images/linux-desktop/`** (TASK-137). It is the runner for
anything that has to drive the real application the way a person does;
`gpu-nvidia-linux` stays the cheaper choice for offscreen work.

What is on the image, on top of RunsOn's `ubuntu24-gpu-x64` base:

| | |
| --- | --- |
| Display | Xorg on the NVIDIA driver, `:0`, one virtual 1920x1080 screen, started at boot by `subordinate-xorg.service`; openbox as the window manager (`subordinate-wm.service`) as the `runner` user |
| X cookie | `/run/subordinate/Xauthority`, minted per boot, world-readable (single-tenant ephemeral instance) |
| Automation | `xdotool`, `xdpyinfo`, `xwininfo`, `xrandr`, `scrot`, ImageMagick, `vulkaninfo` |
| Editor | the newest `v*` release's AppImage, extracted to `/opt/subordinate/app`, exposed as `subordinate` on `PATH` |
| CLI and bridge | `subordinate-cli` and `subordinate-mcp` on `PATH` (the CLI through `AppRun`'s `SUB_APPIMAGE_TOOL`) |
| GStreamer | the 1.28 runtime **bundled inside the AppImage**, reachable as `subordinate-gst-discoverer` / `subordinate-gst-inspect`. Ubuntu 24.04's apt GStreamer is 1.24, so no system GStreamer is installed at all - probing through the bundle is also the more honest test, since that is the runtime the editor decodes with |
| Test media | `/opt/subordinate/test-media/meld-4k60-excerpt-2min.mkv`, copied from the private bucket (TASK-140) and sha256-verified at build time |
| Release marker | `/opt/subordinate/RELEASE_TAG` names the release the image was built from |

RunsOn's agent, the `runner` user (uid 1001) and `/usr/local/bin/runs-on-bootstrap-*`
are left exactly as the base image has them - the `validate` phase fails the
build if any of them has gone, because RunsOn will not schedule on an image
that lost them.

**Jobs must set `DISPLAY` and `XAUTHORITY` themselves.** The Actions runner is
started by the RunsOn bootstrap, which reads no login profile, so the image's
`/etc/profile.d/subordinate-display.sh` never reaches a step. Copy the job
header from `nvidia-desktop-linux` in `.github/workflows/gpu-smoke.yml`:

```yaml
    runs-on: runs-on=${{ github.run_id }}/runner=gpu-nvidia-desktop-linux
    env:
      DISPLAY: ':0'
      XAUTHORITY: /run/subordinate/Xauthority
      SUBORDINATE_CLI: /usr/local/bin/subordinate-cli
```

That job is also the image's acceptance test. It waits for `:0`, starts
`subordinate --ui-smoke` on the committed sample project, waits for the
`ui-smoke ready` line, finds the window with `xdotool search --name`,
screenshots the display with `scrot`, asserts the `render device ready on
Vulkan` line names something that is not a software adapter, probes the baked
test clip with the bundled `gst-discoverer`, and runs an MCP round-trip
(`project.new` then `timeline.get_state`) through `scripts/mcp-roundtrip.py`
while the window is still up. The screenshot, the app log, the window
geometry and the MCP transcript come back as `desktop-smoke-<sha>`.

One caveat about that round-trip: the editor process does **not** bind the
Command API endpoint today (`sub-ui` builds a `Dispatcher` but nothing calls
`Server::bind`), so the bridge starts `subordinate-cli serve` and talks to
that - the same engine and the same dispatcher with nothing drawn. The call
really does go over the socket and really does come back; it just is not the
GUI process's own project. When the editor learns to serve its endpoint, this
job's assertion gets stronger for free.

### Rebuilding the image

Rebuild after every release, and at least monthly: the image bakes in a
release's AppImage, and GitHub stops routing jobs to a runner agent more than
30 days old.

```bash
# From a machine with admin credentials in the account:
infra/images/linux-desktop/deploy.sh --run --wait
```

`deploy.sh` resolves the newest RunsOn `runs-on-v2.2-ubuntu24-gpu-x64-*` base
AMI (owner `135269210855` - the same image `ubuntu24-gpu-x64` resolves to,
confirmed against `RUNS_ON_AMI_ID` in a GPU smoke run), uploads
`component.yaml` to `s3://subordinate-imagebuilder-<account>/` (an inline
component body is not possible: CloudFormation caps
`AWS::ImageBuilder::Component`'s `Data` at 16000 characters), deploys
`stack.yaml`, then starts the pipeline and waits for the AMI id. The component
version is `VERSION` plus a hash of `component.yaml`, so editing the component
always produces a new immutable version and redeploying an unchanged one is a
no-op.

Then, before adopting the new AMI:

1. Smoke it from a branch. RunsOn reads `.github/runs-on.yml` from the default
   branch only, so a new AMI has to be named inline - dispatch **GPU smoke**
   with `only=nvidia-desktop-linux` and
   `desktop_runner=image=<ami-id>/family=g4dn.xlarge/spot=false`.
2. Only once that passes, set `images.subordinate-desktop-linux.ami` in
   `.github/runs-on.yml` to the new id and merge. The id is pinned rather than
   matched by `name:` on purpose: a name filter takes the lexicographically
   highest match, which would adopt a broken rebuild the moment it exists.

`.github/workflows/desktop-ami.yml` does the pipeline half of this from CI,
but it is **not wired up yet**: it needs the repository variable
`AWS_IMAGEBUILDER_ROLE_ARN` naming a GitHub-OIDC role, and
`infra/ci-oidc/stack.yaml` is the (unapplied) template that creates one -
scoped to starting these pipelines and reading image state, nothing more.
Deploying it grants a GitHub workflow an identity in the AWS account, so it is
a decision to take deliberately, not a side effect of this task. Until then the
local `deploy.sh` above is the supported path.

Where to look when a build fails:

```bash
aws imagebuilder get-image --region us-east-1 \
  --image-build-version-arn <arn from deploy.sh> --output json
aws s3 sync s3://subordinate-imagebuilder-<account>/linux-desktop/logs/ /tmp/ib-logs
# console.log is the one to read; the failing step's stderr is quoted in it.
```

Image Builder also streams the same log to CloudWatch under
`/aws/imagebuilder/subordinate-linux-desktop-desktop`.

### Cost

| | |
| --- | --- |
| One image build | two `g4dn.xlarge` on-demand instances in series (build, then test), us-east-1 $0.526/h, about 35-40 minutes in total - roughly **0.40 USD** |
| Storage | one 100 GB gp3 snapshot per AMI, about **0.25 USD/month** each. Deregister old AMIs and delete their snapshots; keep the one `.github/runs-on.yml` points at and the one before it |
| One smoke job | about 8 minutes of `g4dn.xlarge`, spot where capacity allows - **0.03-0.07 USD** |

A failed build terminates its instance (`TerminateInstanceOnFailure: true`), so
a broken component costs minutes, not hours. Watch for `VcpuLimitExceeded`: the
account's on-demand G-family quota is 8 vCPU, which is exactly two
`g4dn.xlarge`, so an image build and a GPU job can collide - re-run the
pipeline, it is not a real failure.

## Self-hosted AMD runner ("box")

AWS no longer offers AMD GPU instances, so AMD Linux verification runs on
`box`, a user-owned mini PC (Ubuntu 26.04, AMD Cezanne APU with VCN encode)
registered as a GitHub self-hosted runner. Labels:
`self-hosted, linux, x64, box, amd-gpu, vaapi`. Zero cost.

- Runner lives in `~/actions-runner` on box as a systemd service
  (`sudo ./svc.sh status|start|stop`). Re-register with a fresh token from
  `gh api -X POST /repos/thowd22/Subordinate/actions/runners/registration-token`.
- The runner user must be in the `render` and `video` groups or the GStreamer
  `va` plugin registers no elements.
- Installed: GStreamer 1.28 dev and plugins from apt, `mesa-va-drivers`,
  `vainfo`, Rust 1.93.1, `xvfb`, `xdotool`, ImageMagick.
- Security: the repository requires approval before workflows from any external
  contributor's fork run, so pull requests from strangers cannot execute code on
  box. Keep hardware workflows on `workflow_dispatch`, `schedule` and pushes
  to `main`.

## Hardware verification workflow

`.github/workflows/hardware.yml` is where the hardware encode and decode
criteria are proved (TASK-116). It runs on `workflow_dispatch` and once a night
at 04:30 UTC, and never on a push or a pull request: the NVIDIA job costs
money and `box` is a machine in someone's home.

Four jobs:

| Job | Runner | Cost | What it proves |
| --- | --- | --- | --- |
| `build-linux` | hosted `ubuntu-24.04` | free | builds `subordinate-bench` (release), `subordinate-cli` (debug) and the `sub-render` readback test binary, and uploads them as `hardware-linux-binaries` |
| `nvidia-linux` | RunsOn `gpu-nvidia-linux` | about 0.04 USD | NVENC render, 4K hardware-decode scrub, 1080p compositor readback |
| `amd-linux` | self-hosted `box` | free | the same through VA-API (`vah264enc`, `vah264dec`) |
| `popout-two-output-linux` | self-hosted `box` | free | the pop-out viewer as a real OS window on a second output, drawn by RADV (TASK-118) |

Nothing is compiled on the GPU instance. A cold `cargo build -p
subordinate-cli` there took 10m52s on 4 vCPU and burned the whole job budget
before anything was measured, so the binaries are built on a free hosted runner
of the same distro release and handed over as an artifact; the GPU job installs
runtime packages only and measures. That cut the job from 20 minutes (timed
out) to 2m29s.

Things that bite, all of them learned from a real run:

- **apt on a GPU instance.** `archive.ubuntu.com` served 73 MB at 93 kB/s to
  us-east-1: 13 minutes. The job rewrites the sources to
  `us-east-1.ec2.archive.ubuntu.com` and caches the `.deb` archives; the same
  install is now 25 seconds.
- **`gst-discoverer-1.0` is in `gstreamer1.0-plugins-base-apps`**, not in
  `gstreamer1.0-tools`.
- **Hardware encoders are ranked `NONE`.** GStreamer never autoplugs an
  encoder, so `vah264enc` (and friends) carry rank `NONE`. `sub-export` keeps a
  deranked element out of its *automatic* selection order but plugs one that is
  named explicitly (`--encoder vah264enc`, or an override in settings), so the
  render steps need no `GST_PLUGIN_FEATURE_RANK`.
- **`subordinate-bench` resolves its fixtures directory from the path it was
  compiled in.** A binary built elsewhere must be given `--fixtures`.
- **box has no Vulkan driver.** The VA-API stack is there, but the compositor
  runs on wgpu: without `mesa-vulkan-drivers` `wgpu` reports "Found no drivers!"
  and every render fails with `render.no_adapter`. The job checks for an ICD up
  front and says so.

Artifacts per GPU job (30 days): the rendered file, `perf.json`, the discoverer
report, the readback log, the `gst-inspect` diagnostics and the rendered job
summary. The numbers land in docs/PERFORMANCE.md.

There are no Windows jobs: there is no AMD Windows host anywhere this project
can reach, and the NVIDIA Windows AMI is TASK-115. The workflow carries a
commented placeholder rather than a job that would quietly pass on software.

## Pop-out and second display

Two displays are an MVP requirement, and the two features that serve them --
the pop-out viewer (TASK-67) and fullscreen on a chosen monitor (TASK-68) --
were both written on a machine with one head and a software adapter. Their
`egui_kittest` tests cover the menu item, the toggle, the picker and the
keyboard; what those tests cannot cover is the only thing the features are
for: a second OS window, on a second output, drawn by a real GPU.

### Linux: automated, free, on box

`hardware.yml`'s `popout-two-output-linux` job does it on every dispatch:

1. one 2560x800 Xvfb screen carved into two 1280x800 heads (two X screens
   joined by Xinerama break `XTranslateCoordinates` under winit -- see the
   window smoke test above);
2. `scripts/ui-smoke.sh --gpu`, which asks for the machine's real adapter and
   says in the summary which one actually drew;
3. `--require-popout-on-head 1`, which reads the pop-out window's absolute
   geometry back off the server with `xwininfo -root -tree` and fails the job
   unless it is inside head 1's rectangle with the editor on another head;
4. one PNG per output -- each with the window that landed on it outlined and
   named -- plus `windows.txt`, `monitors.txt` and `app.log`, uploaded as
   `popout-two-output-<sha>` and listed in the job summary.

```bash
gh workflow run hardware.yml
gh run download <run-id> -n popout-two-output-$(git rev-parse HEAD) -D /tmp/popout
# screen-0.png is the editor window, screen-1.png the pop-out viewer
```

It runs on `box` rather than on the NVIDIA spot instance on purpose: nothing
about it is NVDEC-specific -- it is RandR, window placement and a Vulkan
adapter -- so the paid runner would buy nothing. That is also why the job is
allowed to compile: box is free.

Two limits of a virtual X display, both learned the hard way and both recorded
in the artifact rather than papered over:

- **Xvfb has no DRI3, so a real GPU cannot present into it.** Mesa's Vulkan WSI
  needs DRI3 for a presentable image; RADV refuses the surface with "There was
  no valid format for the surface at all" and the app exits before its first
  frame (run 34614668762). `--gpu` therefore means "use the real adapter if
  this display can present it": the script notices that failure, says so, and
  redraws on Mesa's lavapipe, which the summary records. Note that
  `LIBGL_ALWAYS_SOFTWARE` does nothing here -- it is a GL variable, and the
  compositor is Vulkan -- so the fallback points `VK_DRIVER_FILES` at
  `lvp_icd.json` instead. Real-GPU rendering of the compositor is proved
  headlessly by the `amd-linux` and `nvidia-linux` jobs beside this one; this
  job proves the windows.
- **Ubuntu 26.04's Xvfb ignores `xrandr --setmonitor`.** The server reports
  RandR 1.6 and accepts `RRSetMonitor` without an X error, and the monitor
  never appears: three spellings, no complaint, one automatic whole-screen
  monitor afterwards (runs 34615366090 and 34616222671). So the two heads are
  two rectangles of one virtual screen rather than two RandR monitors. Window
  placement is unaffected -- X windows are positioned in root coordinates, and
  the pop-out lands and is photographed where it is put -- but the app's own
  display enumeration sees one monitor on this runner, so TASK-68's picker
  cannot be exercised here. That half needs a real second head, which is what
  the RDP procedure below is for.

### Windows: interactive, by hand, over RDP with two monitors

There is no Windows GPU runner yet (the AMI is TASK-115), and a second display
on Windows cannot be faked the way Xvfb fakes one: RDP hands the session
exactly as many displays as the *client* has, so the procedure below is what
someone runs by hand once such a host exists. It closes the Windows half of
TASK-68 AC 3.

**1. Get a Windows GPU host with a session you can log into.** Launch the
TASK-115 AMI by hand (EC2 console, `g4dn.xlarge`) rather than through a
workflow -- a job ends and takes the instance with it. Open TCP 3389 in the
security group to your own address only, never `0.0.0.0/0`, and fetch the
password:

```bash
aws ec2 get-password-data --instance-id i-... --priv-launch-key ~/.ssh/runs-on.pem
```

**2. Give the remote session two displays.** RDP mirrors the client's monitor
layout, so the client is where the two heads have to exist:

- Windows client: `mstsc /multimon`, or in `mstsc.exe` -> Display -> "Use all
  my monitors for the remote session".
- Linux client (which is what this project is developed on):
  `xfreerdp3 /v:<public-ip> /u:Administrator /p:'<password>' /multimon
  /gdi:hw /cert:ignore`.

Do **not** use `/span`. It makes one wide desktop out of both monitors, so
Windows sees a single display, the monitor picker lists one entry, and
fullscreen-on-monitor is untested. If the client physically has only one
monitor, add a virtual one *on the client* (an Indirect Display Driver such as
the Virtual Display Driver, or a second head on the dev machine's own X
server); adding one on the remote host does not help, because RDP replaces the
host's display topology with the client's for the duration of the session.

**3. Confirm two displays inside the session,** before touching the app:

```powershell
Add-Type -AssemblyName System.Windows.Forms
[System.Windows.Forms.Screen]::AllScreens | Format-Table DeviceName, Bounds, Primary
```

Two rows at two different `Bounds` origins, or nothing below is meaningful.

**4. Confirm the app is on the GPU, not on WARP.** An RDP session normally
draws through the Microsoft Remote Display Adapter, and a Vulkan application
can end up on a software adapter without saying so anywhere but its log. Turn
on *Computer Configuration -> Administrative Templates -> Windows Components ->
Remote Desktop Services -> Remote Desktop Session Host -> Remote Session
Environment -> Use hardware graphics adapters for all Remote Desktop Services
sessions*, reconnect, then run the app with `RUST_LOG=info` and read the
`adapter chosen:` line it prints. If it names a software adapter, the run
proves nothing about the GPU: use NICE DCV or a VNC server on the host instead
of RDP, which do not swap the display driver out.

**5. Drive the criteria by hand** with the sample project open
(`target\debug\subordinate.exe examples\sample-project\demo.sub`):

| Check | Criterion |
| --- | --- |
| View -> Viewer pops the preview into its own window; drag it to the second display; the picture keeps painting | TASK-67 AC 1 |
| J/K/L, space and the frame steps move the playhead while the pop-out window has focus | TASK-67 AC 3 |
| Closing the pop-out window returns the picture to the docked viewer | TASK-67 AC 2 |
| The View menu's monitor picker lists **two** displays | TASK-68 AC 1 |
| Fullscreen on display 2 shows the frame on black with no chrome | TASK-68 AC 1 |
| Escape leaves fullscreen and keeps the window | TASK-68 AC 2 |
| Restart the app: the chosen display is still chosen (`fullscreen.json`) | TASK-68 AC 2 |

**6. Record the evidence.** One screenshot per monitor (`Win+Shift+S`, or
`[System.Windows.Forms.Screen]::AllScreens` plus a `Graphics.CopyFromScreen`
script), the `adapter chosen:` line, the instance id and the driver version,
attached to the task. Then terminate the instance -- an interactive GPU host
left running is the most expensive mistake available in this repository.

## Linux packaging (AppImage and Flatpak)

`packaging/` holds everything the Linux packages are built from (TASK-103).
Both packages are built and smoke-tested by `.github/workflows/packaging.yml`,
which runs on demand, on any pull request touching `packaging/`, and as a
reusable workflow called by `release.yml` on a `v*` tag (see "Releasing").

```
packaging/
  validate.sh                        metadata tests; run this first
  linux/AppRun                       AppImage entry point (POSIX sh)
  linux/build-appimage.sh            stages the AppDir and packs it
  linux/gst-plugins.txt              which GStreamer plugins get bundled
  linux/subordinate.desktop          desktop entry, shared by both packages
  linux/subordinate.svg              icon, shared by both packages
  linux/io.github.thowd22.Subordinate.metainfo.xml   AppStream, shared
  flatpak/io.github.thowd22.Subordinate.yml          manifest
  flatpak/build-flatpak.sh           flatpak-builder wrapper
```

The application ID is `io.github.thowd22.Subordinate` everywhere: desktop
entry, icon file, metainfo and Flatpak. `packaging/validate.sh` fails if any of
them drifts, or if the metainfo's newest `<release>` stops matching the
workspace version in `Cargo.toml` -- bump both together.

### AppImage

The AppImage carries the GStreamer runtime, because the promise is that a user
downloads one file and hardware export works. Only the driver stack
(Mesa/NVIDIA), the display server and the audio daemon come from the host:
bundling libc, libstdc++, libGL, libva or libcuda is what makes an AppImage
refuse to start on a newer distro or fall back to software rendering, so
`build-appimage.sh` excludes them by name and `validate.sh --appdir` fails if
one appears anyway.

**Which machine builds it decides which machines can run it.** glibc is only
forward compatible: a binary linked against 2.43 asks for `GLIBC_2.43` symbols
that a 2.40 system does not have, and the AppImage dies before `main` with
`/lib64/libc.so.6: version GLIBC_2.43 not found`. That is exactly what the
first CI run did (run 34626701087): built on the `ubuntu-26.04` runner, dead on
Fedora 41. So the `appimage` job in `packaging.yml` runs inside an
**ubuntu:24.04 container** -- glibc 2.39, the oldest supported Ubuntu LTS and
older than every distro this package targets -- and the runner is only the
machine the container runs on. The job prints the highest `GLIBC_2.x` symbol
version the bundle asks for into the run summary; that number is the package's
real minimum.

The cost of that choice is the GStreamer version: **Ubuntu 24.04 carries
GStreamer 1.24, not the repository's 1.28 pin**, and there is no trustworthy
1.28 for that base (no backport, no upstream Linux binary release, and a
from-source build of the monorepo in CI is a bigger liability than a minor
version). So the AppImage bundles 1.24 while `ci.yml`, `hardware.yml` and the
Flatpak stay on 1.28+. Nothing in `sub-media` or `sub-export` needs a 1.26+
API -- the gstreamer-rs feature gate is `v1_18` -- and both `nvcodec` and `va`
(with `vah264enc`) exist in 1.24, which the packaging jobs assert. The job
fails if the base image's GStreamer is not 1.24.x, so the two facts cannot
drift apart silently. Revisit when the next Ubuntu LTS (28.04) is the oldest
LTS in support, or if a plugin the presets need lands after 1.24.

```bash
packaging/linux/build-appimage.sh                  # CI-style, needs pkg-config
packaging/linux/build-appimage.sh --stage-only     # AppDir only, no FUSE needed
packaging/linux/build-appimage.sh --gst-prefix "$GSTROOT/usr" --skip-build
```

On the sudo-less dev boxes there is no system GStreamer, so pass
`--gst-prefix "$GSTROOT/usr"` (the prefix `env-gst.sh` sets up) and
`--stage-only`; that exercises everything except `appimagetool` itself.

One allowlist entry is there for a reason worth stating: **`voaacenc`**. Every
`youtube-*` preset is AAC in MP4, and the obvious AAC encoder -- gst-libav's
`avenc_aac` -- is registered at rank `NONE`. Rank `NONE` is how a machine says
"never plug this unasked", and `sub-export` will not plug a deranked element,
so a bundle whose only AAC encoder is `avenc_aac` refuses every MP4 export with
`export.no_encoder` even though `gst-inspect` lists the element (run
34630086856, from inside the AppImage on box). `voaacenc` is ranked and is
marked required for that reason.

`linux/gst-plugins.txt` is an allowlist, not the whole plugin directory -- the
full Ubuntu set is about 100 MB of things Subordinate never loads. A leading
`!` marks a plugin the package cannot ship without, and `nvcodec` (NVENC) and
`va` (VA-API, which replaced `gstreamer-vaapi` in 1.28) are both marked: a
runtime image that quietly dropped one fails the build instead of shipping a
package that silently software-encodes. Add a module here whenever an export
preset or decode path starts using a new element.

After staging, the script runs `ldd` over every bundled plugin with the
AppDir's own library path and drops the ones whose dependencies are not in the
bundle -- a plugin with an unsatisfiable `dlopen`-only dependency (`libmfx` for
`msdk`, `libopenh264`) is not a loud failure, it is silently blacklisted at
scan time and the element is simply missing at runtime. Optional plugins are
dropped with a note; a **required** one that cannot resolve fails the build.

Two AppRun details are load-bearing:

- **A private plugin registry per run.** GStreamer's registry caches absolute
  paths, and an AppImage mounts at a different `/tmp/.mount_XXXX` every time,
  so a stale registry points at directories that no longer exist. AppRun points
  `GST_REGISTRY` at a per-process file and deletes it on exit.
- **`GST_PLUGIN_SCANNER`.** Without the scanner binary every plugin is loaded
  in-process, and one bad module takes the editor down with it.

`SUB_APPIMAGE_TOOL=gst-inspect-1.0 ./Subordinate-*.AppImage va` runs a bundled
GStreamer tool instead of the editor, which is how CI checks the package's own
registry rather than the host's.

What the host must still provide is the flip side of the excludelist: libc and
libstdc++, the GL/EGL/gbm/libdrm stack, libva *and* libva-drm (the `va`,
`libav`, `qsv` and `msdk` plugins all link the DRM backend, not just libva
itself), libvdpau and the Vulkan loader, the X11 and Wayland client libraries
including `libxcb-xkb` and `libxcb-render`, `libasound` and `libpulse`. That list is not maintained by hand: the
`appimage` job prints every library the bundle resolves outside its own AppDir
into the run summary and ships it with the artifact as `host-libraries.txt`,
and the smoke baselines are the distro spelling of it. A new name appearing
there is the warning that the package has started depending on something a
stock desktop may not have. Every desktop install has all of it; a *bare*
Fedora container does not, which is why the smoke jobs in `packaging.yml`
install exactly that list and nothing GStreamer-shaped before running the
package. If the smoke test there ever needs
a package outside that list, the bundle is missing something.

The `appimage-smoke` job runs the *artifact* -- never the build tree -- on
three systems that did not build it: the `ubuntu-26.04` runner (newer glibc,
the direction that has to work), an `ubuntu:24.04` container (the build base,
i.e. the oldest system claimed) and a `fedora:41` container. All three run the
same script: start the editor with `--help`, resolve `nvcodec`, `va`, `libav`
and `x264` through the bundled registry, and fail if any plugin was
blacklisted.

### Flatpak

The Flatpak does the opposite: it bundles no GStreamer at all. The freedesktop
runtime ships one and declares the `org.freedesktop.Platform.GStreamer`
extension point (`lib/extensions/gstreamer-1.0`, already on the runtime's
`GST_PLUGIN_SYSTEM_PATH`), so plugin sets drop in without rebuilding the app.
VA-API works through `--device=dri` plus the host driver; NVENC works through
the `org.freedesktop.Platform.GL.nvidia-*` extension flatpak mounts to match
the host's kernel module.

The manifest is on **runtime branch 25.08**, and both reasons are worth
knowing:

- `org.freedesktop.Sdk.Extension.rust-stable` on 24.08 is frozen at **rustc
  1.89**, below this workspace's `rust-version = 1.95`. flatpak-builder gets
  several minutes in and then cargo stops with `error: rustc 1.89.0 is not
  supported by the following packages` (run 34626701087). The 25.08 branch of
  the extension tracks current stable, which keeps the toolchain coming from
  the SDK -- the alternative, installing a pinned toolchain over the network
  inside the build, would work in CI but is exactly what a Flathub submission
  may not do.
- 25.08 **retired `org.freedesktop.Platform.ffmpeg-full`**. The full codec set
  is now `org.freedesktop.Platform.codecs-extra`, an extension the *runtime*
  declares (`add-ld-path`, auto-downloaded) whose GStreamer directory the
  runtime already has on `GST_PLUGIN_SYSTEM_PATH`. So the app declares nothing:
  no `add-extensions` block and no `LD_LIBRARY_PATH` finish-arg, and the
  `libav` elements several export presets use are simply there. Re-declaring a
  runtime extension in `add-extensions` would shadow the runtime's own mount
  point and lose the codecs, so `validate.sh` fails the manifest if it does.

Unlike the AppImage, the Flatpak's GStreamer is whatever the runtime ships
(1.26+ on 25.08), not the 1.28 pin -- that is the trade the package makes for
not carrying a runtime of its own. It also inherits the runtime's *encoders*,
and the 25.08 runtime has **no ranked AAC encoder**: `voaacenc`, `fdkaacenc`
and `faac` are absent and `avenc_aac` is rank NONE (measured inside the sandbox
on box, run 34633406106). Every `youtube-*` preset is AAC in MP4, so a Flatpak
user exporting one gets `export.no_encoder` today. The packaging job works
around it for verification with `GST_PLUGIN_FEATURE_RANK=avenc_aac:256` and
says so in its summary; the real fix is either an AAC encoder module in the
manifest or letting the app promote `avenc_aac` itself, which is a product
decision, not a packaging one.

```bash
packaging/flatpak/build-flatpak.sh --install-deps   # first time
packaging/flatpak/build-flatpak.sh --no-bundle      # rebuild the app only
```

The cargo build runs with `--share=network` because crate sources are fetched
at build time. A Flathub submission would have to replace that with a generated
`cargo-sources.json`; that file is deliberately not committed, as it is a
six-figure-line artefact that churns with every `Cargo.lock` change.

### Hardware encode from the packages

`packages-amd` in `packaging.yml` is the job that proves the point of the
packages: it runs on the self-hosted AMD box, downloads both artifacts and
renders `examples/sample-project/demo.sub` with `--encoder vah264enc` twice --
once through `SUB_APPIMAGE_TOOL=subordinate-cli` inside the AppImage (bundled
GStreamer, host driver) and once through `flatpak run --command=subordinate-cli`
(runtime GStreamer, host driver). Both outputs are checked with the *host's*
`gst-discoverer-1.0`, which is an independent look at the file rather than at
the pipeline that wrote it. Nothing is compiled on box; the packages arrive as
artifacts and are run the way a user would run them.

NVENC out of the packages is not covered here: the NVIDIA runner is the paid
one and `hardware.yml` (TASK-116) already owns that budget. `packaging.yml`
proves the bundled registry exposes `nvcodec` on every smoke target.
Hardware encode *from inside the packages* needs a GPU, so `packaging.yml`
proves only that the bundled runtime exposes `nvcodec` and `va` and that the
binary starts on Ubuntu LTS and on a bare Fedora container. Running an actual
NVENC or VA-API export out of the AppImage belongs to `hardware.yml`
(TASK-116), which has the runners.

## Windows packaging (MSI)

`packaging/windows/` builds a single Windows installer that carries the pinned
GStreamer 1.28 runtime, so a user installs one thing and can export (TASK-104).
`.github/workflows/windows-packaging.yml` builds it, installs it, renders with
it and uninstalls it on a hosted `windows-latest` runner, on demand, on any
pull request touching `packaging/windows/`, and as a reusable workflow called
by `release.yml` on a `v*` tag (see "Releasing"). It is a separate
workflow from `packaging.yml` because that one is Linux end to end, down to the
`validate` job that gates it.

```
packaging/windows/
  build-msi.ps1        stages the tree, generates the payload, runs WiX
  main.wxs             package shape: directories, shortcut, upgrade rules
  gst-plugins.txt      which GStreamer plugins get bundled
```

### What is installed

```
%ProgramFiles%\Subordinate\
  bin\subordinate.exe, subordinate-cli.exe
  bin\*.dll                                    the GStreamer 1.28 MSVC runtime
  bin\gst-inspect-1.0.exe, gst-discoverer-1.0.exe, gst-launch-1.0.exe
  bin\vcruntime140.dll, msvcp140.dll, ...      the MSVC CRT, app-local
  lib\gstreamer-1.0\gst*.dll                   the plugins in gst-plugins.txt
  libexec\gstreamer-1.0\gst-plugin-scanner.exe
  LICENSE.txt, README.md
```

That layout is a GStreamer prefix, and that is the whole trick. The Windows
build of GStreamer is relocatable: at `gst_init` it finds its own
`gstreamer-1.0-0.dll`, walks up out of `bin\` and treats what is left as its
prefix, then looks for plugins in `<prefix>\lib\gstreamer-1.0` and for the
helper in `<prefix>\libexec\gstreamer-1.0`. Installing into that shape means
the bundled runtime is found with **no** environment variable, no `PATH` entry
and no registry key -- the counterpart of `packaging/linux/AppRun`, which has
to export `GST_PLUGIN_SYSTEM_PATH_1_0` and friends only because an AppImage
mounts somewhere different every run. Putting the editor's own executables in
`bin\` beside the DLLs is what makes the loader find those too.

The packaging job proves this rather than assuming it: every step that touches
the installed binaries first rewrites `PATH` to the installed `bin\` plus
Windows itself, drops `GSTREAMER_1_0_ROOT_MSVC_X86_64`, and deletes the
GStreamer registry cache under the user profile. A package that works only
because the build machine's own GStreamer is on `PATH` fails there.

### Why WiX directly and not cargo-wix

`cargo-wix` drives the WiX **v3** toolset (`candle.exe`/`light.exe`), which is
end-of-life and is not on the `windows-latest` hosted image any more, and what
it automates is harvesting one crate's own binaries -- it has nothing to say
about the few hundred files of third-party runtime that are the actual work
here. So the toolset is **WiX v5, pinned, installed as a .NET global tool**
(`dotnet tool install --global wix --version 5.0.2`), which takes seconds on
any runner and needs no image support:

* `main.wxs` is hand-written and describes only the package *shape*: the
  install directories, the Start Menu shortcut, the `HKLM\Software\Subordinate`
  install-location key, `MajorUpgrade` (0.1.1 replaces 0.1.0 in place, a
  downgrade is refused with a message) and the `UpgradeCode` GUID, which is the
  product's identity across versions and must never change.
* `build-msi.ps1` generates `target\wix\payload.wxs` from the staged tree, one
  `<Component>` per file with the file as its keypath. That is what Windows
  Installer wants -- WiX can derive each component's GUID, and uninstall
  reference-counting is per file.

### Why the plugins are curated and the DLLs are not

`gst-plugins.txt` is an allowlist in the same format as the Linux one: one
module per line, `!` marks a module the package cannot ship without, and
`build-msi.ps1` *fails* when a required one is absent from the GStreamer
prefix. The required set includes `nvcodec`, `amfcodec` and `mediafoundation`,
so an installer that could not hardware-encode on a whole GPU vendor is a build
failure rather than something a user discovers when their export runs at
software speed.

The DLLs in `bin\` are copied wholesale instead of curated. Curating them means
a transitive dependency walk over PE import tables, which is fragile -- plugins
load some of their dependencies at runtime, where an import table does not see
them -- and buys little, since the cabinet is compressed and the unused DLLs
are a fraction of the installed size next to the plugins and the CRT. The
installed-runtime check in the workflow fails if *any* bundled plugin ends up
blacklisted, which is how a genuinely missing dependency shows up.

The MSVC CRT is deployed app-local (`vcruntime140.dll`, `msvcp140*.dll`,
`concrt140.dll` from the Visual Studio redistributable directory) rather than
by chaining the VC++ redistributable installer. The UCRT is part of Windows and
is never bundled.

### Building it locally

Needs Windows, the MSVC toolchain, the official GStreamer MSVC package
(`/TYPE=devel`, see "GStreamer / Windows" above) and .NET 6+ for the toolset.

```powershell
packaging\windows\build-msi.ps1                    # release build, then the MSI
packaging\windows\build-msi.ps1 -SkipBuild         # package target\release as-is
packaging\windows\build-msi.ps1 -StageOnly         # stage and generate, no WiX
packaging\windows\build-msi.ps1 -GstRoot C:\gstreamer\1.0\msvc_x86_64
```

The MSI lands in `target\wix\Subordinate-<version>-x86_64.msi` and the staged
tree it was built from stays in `target\wix\stage` for inspection. The version
comes from `[workspace.package]` in `Cargo.toml`, so it can never disagree with
the AppImage or the Flatpak.

Install, use and remove it the way CI does:

```powershell
msiexec /i target\wix\Subordinate-0.1.0-x86_64.msi /qn /norestart /l*v install.log
& "$env:ProgramFiles\Subordinate\bin\subordinate.exe" --smoke-test
msiexec /x target\wix\Subordinate-0.1.0-x86_64.msi /qn /norestart
```

`/qn` is fully silent; use `/qb` for a progress bar. Both need an elevated
shell, because the package is per-machine.

### Code signing

The MVP ships **unsigned**. SmartScreen will warn on first run and Windows will
show "Unknown publisher" in the UAC prompt; that is expected, and clearing it
is the main thing a signed release buys. When a certificate exists, signing is
two `signtool` calls and no change to any of the authoring above:

1. Obtain an OV or EV code-signing certificate (EV, or an Azure Trusted
   Signing subscription, is what clears SmartScreen reputation immediately; an
   OV certificate builds reputation over time). Store it as a PFX in a repo
   secret, or use Azure Trusted Signing and skip the key handling entirely.
2. Sign the two executables **before** `build-msi.ps1` stages them, because a
   file changed after it is packaged invalidates the MSI's own signature:

   ```powershell
   signtool sign /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 `
     /f cert.pfx /p $env:CERT_PASSWORD `
     target\release\subordinate.exe target\release\subordinate-cli.exe
   ```

   The bundled GStreamer DLLs are already signed upstream and are left alone.
3. Sign the MSI afterwards, with the same options:

   ```powershell
   signtool sign /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 `
     /f cert.pfx /p $env:CERT_PASSWORD target\wix\Subordinate-0.1.0-x86_64.msi
   signtool verify /pa /v target\wix\Subordinate-0.1.0-x86_64.msi
   ```

   Always timestamp (`/tr`): without it every signature expires with the
   certificate and old installers start warning.

In CI this becomes two steps in `windows-packaging.yml` guarded on the secret
being present, so forks and pull requests keep building unsigned MSIs.

### What is not verified here

The runner has no GPU, so the packaging job pins `x264enc` for its render and
proves only that the installed runtime loads the `nvcodec`, `amfcodec` and
`mediafoundation` plugin modules. Note that loading a plugin and getting its
elements are different things: `nvcodec` and `amfcodec` register their encoders
only after talking to a driver, so on the hosted runner both plugins load and
neither `nvh264enc` nor `amfh264enc` appears (`mfh264enc` does, because the
Media Foundation transform is part of Windows). The job reports that rather
than asserting it. An actual NVENC export out of the installed package is
TASK-115's job, on the hardware workflow's runners. Installing on a clean
Windows image that has never had a build toolchain on it is
`fresh-install.yml` -- see "Fresh-machine install verification".

## Releasing

`.github/workflows/release.yml` turns a tag into a GitHub release with every
package attached (TASK-106). It builds nothing itself: it calls the two
packaging workflows above as **reusable workflows** and then collects what
they produced.

```
release.yml
  linux    -> packaging.yml         AppImage + Flatpak bundle   (TASK-103)
  windows  -> windows-packaging.yml MSI                         (TASK-104)
  macos    -> (commented placeholder)  dmg                      (TASK-105)
  publish  -> collect, SHA256SUMS, gh release create
```

Because `release.yml` owns the `v*` tags, neither `packaging.yml` nor
`windows-packaging.yml` has a tag trigger of its own any more. That is
deliberate: a tag produces exactly one run, so the packages that were smoke
tested are the packages that get published, rather than a second set built by
a parallel run that nobody looked at.

The self-hosted AMD job (`packages-amd`) is skipped when the release workflow
calls `packaging.yml` -- it passes `hardware: 'false'`. box is a mini PC in the
user's house, and a release must neither wait on it nor fail because it is
switched off. Everything else in `packaging.yml`, including the three-target
AppImage smoke matrix, still runs against the artifacts being published.

### Cutting a release

1. Bump `version` in the workspace `Cargo.toml` **and** add a matching
   `<release>` to `packaging/linux/io.github.thowd22.Subordinate.metainfo.xml`.
   `packaging/validate.sh` fails if they drift, and the release workflow fails
   if the tag and the packaged version disagree.
2. Merge that to `main`.
3. Tag and push:

   ```bash
   git tag -a v0.2.0 -m "Subordinate 0.2.0"
   git push origin v0.2.0
   ```

4. Watch the run (`gh run watch`). About an hour cold: the AppImage and the
   MSI are both full release builds, and they run in parallel.

The `publish` job then:

* downloads the `subordinate-*` artifacts and copies the packages -- and only
  the packages -- into `dist/`, failing if any of the three is missing;
* checks that all packages agree on a version, and that the version matches
  the tag with its leading `v` removed;
* writes `dist/SHA256SUMS` with bare filenames and immediately verifies it
  with `sha256sum -c`, so a corrupt artifact is caught before publication
  rather than by the first person to download it;
* runs `gh release create <tag> --verify-tag --generate-notes dist/*`.

A user checks a download with:

```bash
sha256sum -c --ignore-missing SHA256SUMS
```

### The dry run

Every step above except `gh release create` runs in dry-run mode, so the
release path is exercised without publishing anything:

```bash
gh workflow run release.yml --ref main          # dry_run defaults to true
```

A run publishes **only** when all three of these hold: the ref is a tag, the
tag starts with `v`, and the `dry_run` input is not `true`. Anything else --
a dispatch, a branch, a tag that is not a `v*` -- builds the packages, checks
the versions, writes and verifies `SHA256SUMS`, uploads the whole of `dist/`
as the `subordinate-release-<version>` artifact, and prints the asset list it
*would* have published. The decision is made once, in the "Decide whether this
run publishes" step, and the job summary states which mode ran and why.

The `sample-media-v1` release -- where `scripts/get-sample-media.sh` fetches
the test fixtures from -- is untouchable from this workflow. The only release
name it ever mentions is `github.ref_name`, it refuses a ref that is not a
`v*` tag, and `gh release create` fails rather than modifying an existing
release; there is no `gh release edit`, `upload` or `delete` anywhere in it.

### Where the dmg slots in

The macOS package (TASK-105) is deferred until the user's Apple silicon Mac
arrives: a dmg has to be built, and notarised, on a Mac, and the hosted macOS
runners are not in this project's CI budget. `release.yml` carries a commented
`macos` job marking the spot. When TASK-105 adds
`.github/workflows/macos-packaging.yml` with a `workflow_call` trigger and an
artifact named `subordinate-dmg`, wiring it up is three edits:

1. uncomment the `macos` job,
2. add `macos` to the `publish` job's `needs`,
3. add `'*.dmg'` to the required-package list in the "Collect the packages"
   step.

Nothing else changes: the download step already globs `subordinate-*`, the
collect step already copies `*.dmg`, and the checksum and release steps take
whatever ended up in `dist/`.

## Fresh-machine install verification

`.github/workflows/fresh-install.yml` installs the packages on machines that
have never built this project (TASK-110, the phase 7 exit criterion).

This is the check `packaging.yml` and `windows-packaging.yml` cannot make.
Their install smoke steps run on the machine that *just built* the package,
which still has the Rust toolchain, the GStreamer development files and, on
Windows, `C:\gstreamer` on `PATH`. A machine like that cannot tell a package
that carries its dependencies from one that found them lying around. So every
job here starts somewhere that has never seen the project, downloads nothing
but the package, and installs it the way a user would.

```
fresh-install.yml
  resolve   pick a release.yml run and check it still has all three packages
  appimage  clean ubuntu:24.04 and fedora:41 containers   AppImage
  flatpak   a hosted ubuntu-26.04 runner with no flatpak  Flatpak bundle
  msi       a hosted windows-latest runner with no GStreamer anywhere  MSI
  report    one table, and a failure if any machine failed
```

Nothing is ever checked out: a fresh machine has no source tree. The sample
project and its media are fetched by URL at the commit the packages were built
from, so what is opened is what they were built against. No Rust toolchain and
no GStreamer development files are installed on any of these machines; the
Linux containers assert their own absence (`--strict`), and the Windows job
asserts there is no `C:\gstreamer`, no `gst-inspect-1.0` on `PATH` and no
`GSTREAMER_1_0_ROOT_MSVC_X86_64` before it installs anything.

### Which packages get tested

The packages come from a `release.yml` run -- the same artifacts a release
publishes -- rather than being rebuilt, because a package rebuilt for the test
is not the package the user gets. The `release_run_id` input names that run and
defaults to the latest successful one, which for an untagged repository is
`release.yml`'s dry run:

```bash
gh workflow run release.yml            # dry_run defaults to true; note its run id
gh workflow run fresh-install.yml      # takes the latest successful release run
gh workflow run fresh-install.yml -f release_run_id=34642125623
```

`fresh-install.yml` also has a `workflow_call` trigger, so `release.yml` can
gate a tag on it later by passing `release_run_id: ${{ github.run_id }}`;
artifacts uploaded by earlier jobs of a run are downloadable from within it.

### What each machine does

The sequence is the same everywhere and lives in
`scripts/fresh-install-check.sh` (and `scripts/fresh-install-check.ps1` for
Windows), not in the workflow, so that a person checking a download by hand
runs exactly what CI runs:

1. record the machine: OS version, kernel, libc, architecture, GPU (none on a
   hosted runner -- no `/dev/dri`, and `Microsoft Hyper-V Video` on Windows);
2. start the editor, print the bundled GStreamer version, and fail if the
   plugin scanner blacklisted anything -- a blacklisted plugin is a library
   whose dependencies did not come along, which is exactly what only shows up
   away from the build machine;
3. open the sample project through the installed CLI and fail unless its
   `offline` list is empty: a relink prompt on a fresh machine is the classic
   packaging failure, a path that only resolved because of where the build
   tree happened to be;
4. probe the media with the bundled discoverer;
5. export with the **best available encoder**: the candidates are
   `sub_export`'s own order (`nvh264enc`, `vah264enc`, `amfh264enc`,
   `mfh264enc`, `x264enc`), each pinned with `--encoder` and tried in turn, so
   a hosted runner ends up on software x264 and a machine with a GPU does not.
   Registered is not the same as usable -- `nvcodec` and `va` register
   elements that need a driver behind them -- so a candidate that cannot
   render falls through to the next one;
6. validate what was written: `--verify` makes the package read its own output
   back in process, and where the package ships `gst-discoverer-1.0` as a
   command it is read again from outside;
7. open the project in the real editor window under Xvfb
   (`subordinate --ui-smoke`), which paints, pops the viewer out and prints a
   ready line. On these machines wgpu lands on Mesa's lavapipe.

The preset is `mezzanine` -- H.264 in MKV with FLAC audio -- so no machine is
failed for want of an AAC encoder, and one second of the sequence is rendered:
this workflow is about the install, not the throughput, which `hardware.yml`
measures. "Plays with audio" is verified as decode, mix and encode, with an
audio stream in the output: a hosted runner has no sound card to play to.

Each job writes its facts into the job summary and uploads them, along with
every log, as a `fresh-install-*` artifact.

### Optional: the machines with real GPUs

`-f hardware=true` adds two jobs that are skipped by default, because the
standard run has to stay hosted-only and short -- that is what a release can
afford to wait for, and what can be relied on to be switched on:

* **box**, the self-hosted AMD mini PC, runs the AppImage check with the
  `youtube-1080p` preset, where "best available encoder" resolves to
  `vah264enc` rather than x264;
* **yodaddy**, the user's daily-use Windows desktop, installs the MSI, runs the
  same check and uninstalls it again.

Both work entirely inside `$RUNNER_TEMP`, and yodaddy's uninstall step runs
`if: always()`: leaving a package installed on somebody's own machine is not
this workflow's business. Neither machine is fresh in the strict sense, so
`--strict` is off there and the check records what they already had rather
than failing on it.

### By hand, on a physical fresh machine

This is the version to run when a real laptop is available -- a friend's
Ubuntu install, a newly imaged Windows box, or the user's Mac once TASK-105
lands a dmg. Download the package and the sample project, write a two-line
adapter saying how the package is reached, and run the same script:

```bash
# the package, the project and its media
curl -fLO https://github.com/thowd22/Subordinate/releases/download/v0.1.0/Subordinate-0.1.0-x86_64.AppImage
curl -fLO https://raw.githubusercontent.com/thowd22/Subordinate/main/examples/sample-project/demo.sub
curl -fLO https://raw.githubusercontent.com/thowd22/Subordinate/main/scripts/get-sample-media.sh
curl -fLO https://raw.githubusercontent.com/thowd22/Subordinate/main/scripts/fresh-install-check.sh
sh get-sample-media.sh --out "$PWD/media"

# how this package is reached (a desktop with FUSE needs no extraction)
chmod +x Subordinate-*-x86_64.AppImage
cat > adapter.sh <<'EOF'
SUB_PACKAGE_LABEL="AppImage"
appimage=$(ls "$PWD"/Subordinate-*-x86_64.AppImage)
sub_gui() { "$appimage" "$@"; }
sub_cli() { SUB_APPIMAGE_TOOL=subordinate-cli "$appimage" "$@"; }
sub_tool() { tool=$1; shift; SUB_APPIMAGE_TOOL=$tool "$appimage" "$@"; }
EOF

sh fresh-install-check.sh --adapter "$PWD/adapter.sh" --project "$PWD/demo.sub"
```

On a machine with a desktop session already running, `DISPLAY` is set and the
editor opens a real window on the real GPU instead of an Xvfb one; the facts
record which adapter it chose. On Windows, install the MSI by double-clicking
it and then:

```powershell
.\fresh-install-check.ps1 -Project .\demo.sub
```

Record the result -- OS version, GPU, encoder used, pass or fail -- in
`backlog/docs/doc-4 - Fresh-machine-install-verification-log.md` beside the CI
runs, and open a blocking task for any failure before the release goes out.

### What is not covered

* **macOS.** Deferred with TASK-105/117 until the user's Apple silicon Mac
  arrives: there is no dmg to install yet. The shell check script already
  handles Darwin, so the third leg is a job and an adapter when the package
  exists.
* **Hardware encode from a fresh install**, unless `hardware=true` is passed.
  The hosted runners have no GPU; `hardware.yml` (TASK-116) owns the GPUs.
* **The FUSE path.** A desktop double-clicks an AppImage and libfuse mounts
  it; a container has no `/dev/fuse`, so the CI jobs use the same runtime's
  `--appimage-extract`, which unpacks the identical AppDir. The by-hand
  runbook above is the one that exercises the mount.
