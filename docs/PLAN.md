# Subordinate — Cross-platform video editor MVP plan

Working name: **Subordinate** (repo name). Date: 2026-09-08.

## 1. Problem and thesis

Linux has no editor that combines a modern GPU-driven timeline, hardware encode on
NVIDIA and AMD, and an extensible architecture. Premiere-class editors are closed and
not on Linux; Kdenlive/Shotcut are bound to MLT's CPU-first design; Olive is dormant.

Thesis: build a **small, fast core** (timeline model, media I/O, GPU compositor,
audio mixer, project files, export) and push **everything else into plugins**. Make
the plugin system the product: a typed, sandboxed plugin ABI plus a local MCP server,
so a coding agent (Claude Code etc.) can write, install, hot-reload and test a plugin
without a human touching the build system.

## 2. MVP scope

In scope:

- Multiple sequences (timelines) per project, each with N video and N audio tracks.
- Timeline editing: import, trim, split, ripple/overwrite insert, move, snap, zoom.
- Basic audio: gain, mute/solo, fades, simple mix-down, level meters.
- Project media bin: import, folders, metadata, thumbnails, offline/relink, proxies.
- Preview: in-app viewer, pop-out to a second display, scrubbing, JKL playback, A/V sync.
- Export: H.264/HEVC via NVENC (Linux, Windows) and AMD (VA-API on Linux, AMF on
  Windows), software x264 fallback, VideoToolbox on macOS.
- Project files: human-diffable JSON, autosave, versioned schema, undo/redo.
- Plugin system (WASM) with the extension points listed in §6, and one MCP server.

Out of scope for MVP (plugins or later): colour grading, titles/text, transitions
beyond crossfade, motion tracking, multicam, OTIO/EDL/FCPXML/AAF interchange,
OpenFX/frei0r hosting, keyframed effect parameters beyond opacity/gain/position.

## 3. Key decisions

| Area | Decision | Why | Alternatives rejected |
|---|---|---|---|
| Language | **Rust** | Memory safety in a heavily threaded media app; first-class wgpu, wasmtime, cpal, gstreamer-rs bindings; single toolchain on all three OSes | C++/Qt (fine, but plugin sandboxing and agent-friendliness are worse; Qt licensing); Zig (media ecosystem too thin) |
| Media decode/encode | **GStreamer 1.28 via gstreamer-rs 0.25** | Only safe-Rust route to NVDEC/NVENC (`nvcodec`), AMD (`va` on Linux, `amf` on Windows), VideoToolbox, Media Foundation, plus Vulkan Video. Actively maintained; Rust is >35% of GStreamer commits | ffmpeg-next (maintenance-only; HW paths require unsafe raw-pointer FFI); writing our own demuxers (no) |
| GPU compositing | **wgpu 30** | Cross-platform Vulkan/D3D12/Metal; NV12 plane helpers and cross-API fences landed in 30.0; DMA-BUF import merged on trunk | Raw Vulkan (portability cost); OpenGL (dying on macOS) |
| GUI | **egui/eframe 0.36** | Immediate mode is ideal for a custom-painted timeline; mature multi-viewport API gives the pop-out preview for free; renders through wgpu so the preview texture is shared with the compositor | iced (good, slower cadence, multi-window edge bugs); Slint (DSL awkward for a canvas-heavy tool); GPUI (API churn, thin docs); Tauri/web UI (timeline perf and GPU frame sharing are painful) |
| Audio | **cpal 0.18 + symphonia 0.6 + rubato 5**, own lock-free mixer graph; GStreamer decodes audio inside video files, symphonia decodes audio-only files | No production DAW-grade graph crate exists; MVP needs only a mixer. Firewheel is the reference if a real graph is needed later | Routing audio through GStreamer (fine for decode, wrong for a low-latency editor mixer) |
| Timeline model | Own model, **OTIO-shaped** (Timeline / Stack / Track / Clip / Gap / Transition / Marker, rational time) | Keeps a future OTIO export trivial; OTIO itself lacks effects/keyframes/proxy metadata and has no usable Rust bindings | Using OTIO JSON as the native format |
| Project format | JSON (serde), one file per project plus a `.subordinate/` sidecar dir for cache, thumbnails, proxies | Diffable, agent-readable, git-friendly | SQLite (better for huge projects; revisit post-MVP), binary |
| Plugin ABI | **WASM Component Model** (wasmtime 48, WIT interfaces), Zed's pattern | Sandboxed, language-agnostic, typed interfaces, hot-reloadable, versioned WIT with shims. Agents can target it from Rust, Go, Python, JS | Native dylibs (no sandbox; keep as a later "trusted" tier for OFX/frei0r bridges); Extism (simpler but untyped byte-buffer ABI); scripting (Lua/Python) only |
| GPU effects from plugins | Plugins ship **WGSL shaders + a parameter schema**; the core compiles and runs them | WASM cannot touch the GPU; running pixel loops in WASM is far too slow for 4K. CPU pixel ops remain available for small buffers (thumbnails, analysis) | Letting plugins own wgpu handles (unsafe, unsandboxable) |
| Automation | One internal **Command API** (JSON-RPC over a local socket / named pipe). GUI, CLI, MCP server and plugins all call it | Single source of truth for every editing operation; makes the MCP surface exactly as capable as the UI | Separate GUI code paths and scripting code paths |
| Licence | Core **GPL-3.0-or-later**; SDK and WIT **MIT OR Apache-2.0** | Copyleft core discourages closed forks; permissive SDK lets plugins be any licence | Fully permissive core; fully GPL SDK |
| MCP | **rmcp 3.x** (spec 2026-07-28). A tiny `subordinate-mcp` stdio binary bridges to the running app's Command API; optional Streamable HTTP on localhost for long-lived multi-client use | stdio is what Claude Code's `.mcp.json` expects; the bridge lets the GUI app stay a normal desktop process | Embedding a full MCP server inside the GUI process only |

## 4. Architecture

Cargo workspace, layered so that nothing above `core` is needed to run a headless render.

```
subordinate/
  crates/
    sub-core        shared conventions: SubError with stable codes, tracing setup
    sub-time        rational time, timecode, frame rates (drop-frame aware)
    sub-model       project/sequence/track/clip data model, serde schema, migrations
    sub-edit        editing operations, undo/redo (command pattern), validation
    sub-media       GStreamer wrappers: probe, decode (frame cache, seek), thumbnails, proxies
    sub-audio       cpal output, mixer graph, resampling, meters
    sub-render      wgpu compositor: frame graph, NV12/RGB conversion, opacity/transform, shader effects
    sub-export      render sequence -> encoder pipeline (NVENC/AMF/VA/VT/MF/x264), progress, cancel
    sub-command     Command API: JSON-RPC schema, dispatcher, local socket server
    sub-plugin      wasmtime host, WIT bindings, manifest, registry, hot reload
    sub-ui          egui app: timeline, bins, viewer, pop-out viewport, inspector
  bins/
    subordinate     GUI binary
    subordinate-cli headless: render, probe, run commands, plugin scaffold/test
    subordinate-mcp MCP stdio bridge (rmcp)
  wit/              plugin interface definitions, versioned (subordinate:plugin@0.1.0)
  plugins/          first-party reference plugins (also serve as agent templates)
  sdk/              plugin SDK crates for Rust (+ later Python/JS via componentize)
  docs/
```

Threading model:

- **UI thread**: egui only. Never blocks on media.
- **Engine thread**: owns the project state and the Command API. All mutations are
  commands. Emits change events to UI and MCP subscribers.
- **Decode pool**: one GStreamer pipeline per active clip, decode-ahead ring buffer,
  keyframe-aware seek (seek to previous keyframe, decode forward, drop until PTS).
- **Compositor thread**: pulls frames for time T, runs the wgpu frame graph, writes
  to a swapchain texture (preview) or a readback/encoder texture (export).
- **Audio callback**: real-time thread, lock-free ring buffers only; no allocation.
- **Plugin executor**: wasmtime instances with fuel/epoch limits so a bad plugin
  cannot stall the engine.

Data flow for preview: `Playhead -> Scheduler -> {decode requests} -> frame cache ->
compositor -> wgpu texture -> egui Image (main viewport or pop-out viewport)`.

## 5. Core subsystems

### 5.1 Time and model
- `RationalTime { value: i64, rate: Rational }` everywhere. No floats for time.
- Sequence settings: resolution, frame rate, audio sample rate, colour space tag (assume Rec.709 for MVP, but store it).
- Clip = reference to media item + source range + track placement + per-clip params (opacity, transform, gain, fades).
- Every edit is a `Command` with `apply`/`revert`; undo stack lives in the engine. This same command set is what the Command API and MCP expose.

### 5.2 Media I/O
- Probe with `discoverer`; store stream info, duration, VFR flag, rotation, colour metadata.
- Decode: `uridecodebin` -> `appsink` with HW decoders preferred (nvdec/va/vtdec/d3d12). MVP uploads NV12 planes from CPU to wgpu; zero-copy DMA-BUF/D3D12 import is a post-MVP optimisation once wgpu ships the DMA-BUF API in a release.
- Frame cache: LRU keyed by (media id, pts) with a memory budget; scrub responsiveness depends on it.
- Proxies: auto-generate intra-only proxies (DNxHR LB or MJPEG at 1/2 or 1/4 res) for long-GOP sources above a threshold. Proxy state tracked per media item; export always uses originals. Handle VFR by building a PTS index instead of assuming constant frame duration.
- Thumbnails and waveforms generated in a background job queue (also a plugin extension point).

### 5.3 Compositor
- Frame graph per frame: for each video track top-down, sample clip frame -> colour convert -> transform/opacity -> blend. Crossfade is the only transition.
- Shader effects: `Effect { wgsl: String, params: Schema, uniforms }` compiled and cached by hash. This is the mechanism plugins use.
- Same graph serves preview (any resolution, drop frames under load) and export (full resolution, never drop).

### 5.4 Audio
- Mixer: per-clip source (symphonia decode -> rubato resample to sequence rate) -> clip gain/fade -> track gain/mute/solo -> master -> cpal. Meters via atomics.
- Playback clock is the audio clock; video follows it. Scrubbing uses short audio grains.
- Export renders audio offline through the same graph.

### 5.5 Export
- Pipeline: compositor readback -> `appsrc` -> encoder element chosen by capability probe -> muxer (mp4/mkv/mov).
- Encoder selection order, overridable: NVIDIA `nvh264enc`/`nvh265enc`/`nvav1enc`; AMD Linux `vah264enc`/`vah265enc`; AMD Windows `amfh264enc`/`amfh265enc`; macOS `vtenc_h264`/`vtenc_h265`; Windows fallback `mfh264enc`; software `x264enc`/`x265enc`.
- Presets are data (TOML) and an extension point.
- Headless export via `subordinate-cli render project.sub --sequence Main --preset youtube-1080p`. This is also the CI smoke test.

### 5.6 Project files
- `project.sub` (JSON): schema version, media items (relative paths + content hash for relinking), sequences, bins, settings.
- Sidecar `project.sub.d/`: thumbnails, waveforms, proxies, autosave snapshots. Gitignored by default.
- Migrations keyed by schema version; never break old files.

### 5.7 UI
- Panels: Media bin, Timeline, Viewer, Inspector, Export. Dockable layout persisted.
- Viewer pop-out: an egui deferred viewport that renders the same wgpu texture; fullscreen on a chosen monitor.
- Timeline painted directly with `Painter`: virtualised rendering, only visible clips drawn; waveform/thumbnail strips cached as textures.

## 6. Plugin system (the product)

### 6.1 Principles
1. **Agent-first**: a plugin should be creatable by Claude Code from a single prompt. That means: a scaffold command, a small typed interface, fast build/reload loop, and machine-readable errors.
2. **Sandboxed by default**: WASM components with explicit capabilities (filesystem paths, network, GPU shaders) declared in the manifest and approved on install.
3. **Everything the UI can do, a plugin can do** through the Command API host import.
4. **Versioned interfaces**: `subordinate:plugin@0.x` WIT worlds; host keeps shims for older versions (Zed model).

### 6.2 Extension points (WIT worlds)
| World | Purpose | MVP? |
|---|---|---|
| `effect` | Declares params + WGSL shader(s); optional CPU `process` for small buffers | yes |
| `audio-effect` | Block-based f32 processing (gain, EQ, etc.) with a real-time budget | yes (simple) |
| `importer` | Turn a file/URL into media items or a sequence (e.g. OTIO, EDL, YouTube) | yes |
| `exporter` | Encoder presets, or full custom export targets (upload, sidecar generation) | yes |
| `analyzer` | Background jobs producing metadata (scene detect, transcripts, loudness) | yes |
| `command` | Adds new editing commands (auto-cut silence, montage from markers) built from primitives | yes |
| `panel` | UI panel via a declarative widget tree (JSON) rendered by egui; later MCP Apps-style HTML | post-MVP |
| `mcp-tools` | Plugin contributes extra MCP tools; core forwards calls | yes |

Native (dylib) plugin tier for OpenFX 1.5 and frei0r bridges is a post-MVP "trusted" tier.

### 6.3 Manifest
```toml
[plugin]
id = "com.example.silence-cutter"
name = "Silence Cutter"
version = "0.1.0"
api = "0.1"
worlds = ["command", "mcp-tools"]

[capabilities]
fs_read = ["$PROJECT"]
network = false

[mcp.tools.cut_silence]
description = "Remove silent regions from the selected clips"
schema = "schemas/cut_silence.json"
```

### 6.4 Developer loop (what an agent runs)
```
subordinate-cli plugin new --world command my-plugin   # scaffold with CLAUDE.md + tests
cargo component build --release                        # or componentize-py / jco
subordinate-cli plugin install ./target/wasm32-wasip1/release/my_plugin.wasm --dev
subordinate-cli plugin test my-plugin                  # runs against a fixture project headlessly
```
Every one of these is also an MCP tool (`plugin.new`, `plugin.install`, `plugin.reload`, `plugin.test`), so the agent never leaves the conversation. Install with `--dev` watches the file and hot-reloads. Errors are structured JSON with WIT type names, not stack traces.

The scaffold ships a `CLAUDE.md` describing the world's interface, host imports, and testing contract, plus a fixture project. First-party plugins in `plugins/` are the worked examples.

## 7. MCP server and automation

- `subordinate-mcp` (rmcp 3.x, stdio) connects to the running app's local socket, or launches `subordinate-cli` headless if none is running. Registered via `.mcp.json` in the project scope so Claude Code picks it up automatically.
- Tool families, mirroring the Command API:
  - `project.*` open/save/new, list sequences, settings
  - `media.*` import, list, probe, relink, generate proxies
  - `timeline.*` add/move/trim/split/delete clips, add tracks, markers, get state as OTIO-shaped JSON
  - `playback.*` seek, play/pause, render a single frame to PNG (so an agent can "look")
  - `export.*` list presets, render, progress
  - `plugin.*` new/install/reload/test/list
- Resources: `project://current`, `sequence://{id}`, `media://{id}`; list results carry `ttlMs` for caching.
- Multi round-trip input (`input_required`) for destructive operations that need confirmation.
- Later: an MCP Apps `ui://` resource that renders a mini timeline/preview inside Claude.

## 8. Roadmap

Each phase ends with a demo and a headless CI check. Estimates assume one focused developer plus agents; treat them as ordering, not commitments.

| Phase | Weeks | Deliverable | Exit criteria |
|---|---|---|---|
| 0 Foundations | 1–2 | Workspace, CI (Linux/Win/mac), `sub-time`, `sub-model` with serde + migrations, Command API skeleton, CLI that loads and saves a project | Round-trip a project file; 100% of edits go through commands with undo |
| 1 Media + preview | 3–5 | GStreamer probe/decode, frame cache, wgpu NV12 upload + compositor, egui viewer with scrub | Scrub a 4K H.264 clip at >30 fps on Linux with nvdec/va; frame-accurate seek test suite |
| 2 Timeline | 6–8 | Multi-track, multi-sequence timeline UI and editing ops, media bin, thumbnails | Assemble a 20-clip edit with cuts and crossfades; undo/redo across everything |
| 3 Audio | 9–10 | Mixer, cpal output, A/V sync on the audio clock, waveforms, fades, meters | Sync drift < 1 frame over 10 minutes; no audio thread allocations (verified) |
| 4 Export | 11–12 | Encoder capability probe, NVENC/VA/AMF/VT/MF/x264 paths, presets, headless render, progress/cancel | Same project renders on all three OSes; GPU encode verified on one NVIDIA and one AMD machine |
| 5 Pop-out + proxies + polish | 13–14 | Second-display viewer, proxy pipeline, autosave, relink, keyboard map | Edit a 1-hour long-GOP source smoothly with proxies |
| 6 Plugins + MCP | 15–18 | wasmtime host, WIT 0.1 worlds (effect, audio-effect, importer, exporter, analyzer, command, mcp-tools), SDK, scaffold, hot reload, `subordinate-mcp` | Claude Code, from a prompt, creates, installs and tests a working "cut silence" plugin with no manual steps; three first-party plugins shipped |
| 7 MVP release | 19–20 | Packaging (AppImage/Flatpak, MSI, .dmg with bundled GStreamer), docs, sample project | Fresh-machine install works on all three OSes |

Plugin system work (phase 6) can start in parallel at phase 3, since it depends only on the Command API.

## 9. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Shipping GStreamer on Windows/macOS is heavy and fiddly | Bundle a pinned GStreamer runtime per platform from day one in CI; test hardware element availability at startup and show a diagnostics panel |
| CPU upload of decoded frames caps 4K performance | Acceptable for MVP; zero-copy (DMA-BUF, D3D12 NV12 planes) is scheduled once wgpu releases the API. Proxies cover the gap |
| egui timeline feels less "native" than retained UIs | Prototype the timeline in week 6 and benchmark with 500 clips; iced is the fallback and the model/engine are UI-agnostic |
| WASM effect plugins cannot do heavy pixel work | Shader-based effects are the primary path; document this clearly in the SDK |
| wasmtime monthly majors and MCP spec churn | Pin versions; WIT is versioned; MCP bridge is a separate small binary that can be rebuilt without touching the app |
| A/V sync and VFR sources | Audio-clock master, PTS index per media item, VFR test fixtures in CI from week 3 |
| Scope creep into "Premiere parity" | The MVP feature list in §2 is frozen; anything else is a plugin issue, and the plugin system is where extra effort goes |

## 10. Settled decisions (2026-09-08)

These were open questions; all four were resolved by taking the recommendation.

1. **Licence**: core crates and binaries under **GPL-3.0-or-later**. The plugin SDK crates and the `wit/` interface definitions under **MIT OR Apache-2.0**, so plugins may use any licence, including closed source.
2. **Colour management**: the model stores colour space, transfer and primaries **tags only** for the MVP. Rendering assumes Rec.709. A transform pipeline is a post-MVP plugin-extensible concern; the tags make that migration lossless.
3. **Audio decode path**: **GStreamer** decodes audio tracks of video files (single demux, shared pipeline). **symphonia** decodes audio-only files. Both feed the same owned mixer graph.
4. **Panel plugins**: **deferred until after the MVP**. The `panel` world is not part of WIT 0.1. When revisited, evaluate declarative JSON widgets versus MCP-Apps-style HTML in a webview.

## 11. First concrete steps

1. `cargo new` workspace with the crate layout in §4, CI matrix on GitHub Actions with a pinned GStreamer install on all three OSes.
2. Implement `sub-time` and `sub-model` with the OTIO-shaped schema and serde round-trip tests.
3. Spike: decode one H.264 file with gstreamer-rs to `appsink`, upload NV12 to wgpu, display it in an egui window and a pop-out viewport. This single spike de-risks phases 1 and 5.
4. Spike: `nvh264enc` and `vah264enc` export of a colour-bar sequence via `appsrc`, verify on real hardware.
5. Write the `subordinate:plugin@0.1.0` WIT for the `command` world and get a hello-world component running through wasmtime with the Command API host import.
