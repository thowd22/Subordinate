# Development setup

Subordinate is a Rust workspace. Media I/O is built on GStreamer, so a
GStreamer development install is required to build `sub-media` and everything
above it. CI pins **GStreamer 1.28.x** on every OS (see `GST_VERSION` in
`.github/workflows/ci.yml`); match that minor version locally.

## Rust

`rust-toolchain.toml` pins the toolchain (currently 1.93.1 with rustfmt and
clippy). `rustup` picks it up automatically.

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
  gstreamer1.0-plugins-ugly gstreamer1.0-libav gstreamer1.0-tools
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

A project is `name.sub` (JSON). Its sidecar directory `name.sub.d/` holds
thumbnails, waveforms, proxies and autosave snapshots and is gitignored.
