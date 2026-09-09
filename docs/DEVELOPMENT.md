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

## Test media fixtures

Media tests need deterministic sample files, and binaries are never committed.
`scripts/gen-fixtures.sh` (`scripts/gen-fixtures.ps1` on Windows) synthesises
them with `gst-launch-1.0` into `fixtures/`, which is gitignored:

```
./scripts/gen-fixtures.sh            # everything except the 10-minute clip
./scripts/gen-fixtures.sh --long     # add the 10-minute long-GOP clip
./scripts/gen-fixtures.sh --list     # show the catalogue
./scripts/gen-fixtures.sh --dry-run  # print the pipelines without running them
```

The catalogue is 1080p and 4K H.264 colour bars with a burnt-in timecode, a
29.97 drop-frame clip, a variable-frame-rate clip, a 10-minute long-GOP clip,
and 48 kHz stereo audio-only files in WAV, FLAC, MP3, AAC (in MP4) and Ogg
Vorbis. Existing files are kept unless `--force` is given, so re-running the
script is cheap.

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

`crates/sub-model/tests/fixtures/sample-project.sub` is a committed sample
project (two sequences, three tracks each, clips, a crossfade, markers and two
bins). `crates/sub-model/tests/golden.rs` checks that it loads and saves
byte-identically and that the builder in that test still produces exactly those
bytes, so any change to the on-disk format fails with a line diff. When such a
change is intended, regenerate the fixture and commit its diff:

```bash
SUB_UPDATE_GOLDEN=1 cargo test -p sub-model --test golden
```

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
  | `gpu-amd-linux` | `g4ad.xlarge` (Radeon Pro V520) | `ubuntu26-full-x64` | 0.379 | ready |
  | `gpu-nvidia-windows` | `g4dn.xlarge` | custom AMI placeholder | 0.526 | not usable until TASK-115 |
  | `gpu-amd-windows` | `g4ad.xlarge` | custom AMI placeholder | 0.379 | not usable until TASK-115 |

  All four need the EC2 G-family vCPU quotas (`L-DB2E81BA` on-demand,
  `L-3819A6DF` spot) above zero in us-east-1.
- RunsOn's `*-gpu-*` images carry the NVIDIA driver and CUDA only, so the AMD
  runner uses the plain Ubuntu 26.04 image and jobs install the Mesa VA-API
  stack (`mesa-va-drivers`, `vainfo`) themselves. The GStreamer `va` plugin
  (`vah264enc`) is not a separate Ubuntu package - it ships in
  `gstreamer1.0-plugins-bad`. Note the NVIDIA GPU image is Ubuntu 24.04, whose
  apt GStreamer is 1.24, not the 1.28 pinned everywhere else.
- GPU smoke test: run the **GPU smoke** workflow (`workflow_dispatch`). It
  checks `nvidia-smi` plus `gst-inspect-1.0 --exists nvh264enc` on the NVIDIA
  runner, and `/dev/dri` + `vainfo` plus `vah264enc` on the AMD one.
- Cost: the RunsOn config schema has no per-runner price cap, so hourly prices
  are recorded in comments there and every GPU job must set `timeout-minutes`.
  Runners request spot (`price-capacity-optimized`) with
  `retry: when-interrupted`; if spot capacity is unavailable, re-dispatch with
  `/spot=false` appended to the label to force on-demand. Instances are billed
  by AWS with no markup; every job gets a fresh instance that is terminated
  when the job ends.

Useful commands:

```
aws cloudformation describe-stacks --stack-name runs-on --region us-east-1
aws logs tail /aws/ecs/runs-on/runs-on-worker --region us-east-1 --since 10m
aws ec2 describe-instances --region us-east-1 \
  --query "Reservations[].Instances[].[InstanceId,InstanceType,State.Name]" --output text
```
