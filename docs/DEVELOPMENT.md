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
and 48 kHz stereo WAV and FLAC audio-only files. Existing files are kept unless
`--force` is given, so re-running the script is cheap.

Alongside them the script writes `fixtures/manifest.json`, recording each
fixture's name, kind, dimensions, exact duration in nanoseconds, frame rate as
an exact rational, and whether it is variable-frame-rate. Fixtures the run
skipped stay in the manifest with `"generated": false`.

Tests locate fixtures through the `sub-test-support` crate rather than by path:

```rust
let clip = sub_test_support::fixture("bars_1080p_h264.mp4")?;      // Result
let long = sub_test_support::try_fixture("longgop_720p_10min.mp4"); // Option
```

`SUB_FIXTURES_DIR` overrides the directory; otherwise it is `fixtures/` at the
workspace root. CI generates the small fixtures before building, so the media
tests have their inputs on every runner.

## Errors and logging

See the conventions task (TASK-11) once implemented: errors are `SubError`
values with stable string codes; logging uses `tracing`.

## Project files

A project is `name.sub` (JSON). Its sidecar directory `name.sub.d/` holds
thumbnails, waveforms, proxies and autosave snapshots and is gitignored.
