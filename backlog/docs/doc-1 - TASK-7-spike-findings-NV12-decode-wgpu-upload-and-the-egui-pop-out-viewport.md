---
id: doc-1
title: 'TASK-7 spike findings: NV12 decode, wgpu upload and the egui pop-out viewport'
type: specification
created_date: '2026-09-09 03:42'
updated_date: '2026-09-09 03:45'
tags:
  - spike
  - media
  - ui
---
Spike for TASK-7 (docs/PLAN.md §11 step 3). Code lives in `spikes/nv12-viewport`
and is throwaway; these findings are the deliverable. Every number below was
measured on the machine described under *Environment*, and every number that
could **not** be measured there is called out as such rather than estimated.

## What the spike does

One binary, four subcommands, so each part of the path can be measured on its
own:

| subcommand | needs | measures |
| --- | --- | --- |
| `bench` | a wgpu adapter | upload + BT.709 conversion on synthesised NV12 |
| `decode-bench <file>` | GStreamer decoders + a wgpu adapter | appsink hand-off through to converted-on-GPU |
| `play <file>` | the above plus a display | the same, painted in a window and a pop-out viewport |
| `make-clip <file>` | GStreamer encoders | synthesises a test clip so the others have input |

The pipeline is `uridecodebin3 -> videoconvert -> appsink(NV12)`. Frames cross
to the UI through a single newest-wins slot: a preview may drop frames, and
holding a stale one only adds latency. PTS crosses as an exact `RationalTime`
at 1/1 000 000 000 s, never a float.

## Finding 1: `decodebin3` has no `autoplug-sort`, so rank is the only lever

The obvious way to prefer hardware decoders — connect to `autoplug-sort` and
reorder the candidate factories — **does not work with `decodebin3` or
`uridecodebin3`**. The signal only exists on the old `decodebin`. Connecting to
it aborts the process:

```
thread 'main' panicked at glib-0.22.9/src/object.rs:2675:36:
Signal 'autoplug-sort' of type 'GstURIDecodeBin3' not found
```

What decodebin3 honours is the **registry rank**. `prefer_hardware_decoders()`
therefore walks `ElementFactory::factories_with_type(DECODER | MEDIA_VIDEO)` and
raises every hardware decoder above `GST_RANK_PRIMARY` (256), which is exactly
where the software `avdec_h264` sits. This is the in-process equivalent of
`GST_PLUGIN_FEATURE_RANK`, it is process-local (nothing on disk is touched), and
it demotes nothing, so a machine with no hardware decoder behaves as before.

Preference order, highest first: `nvdec` > `va` > `vtdec` > `d3d12` > `d3d11` >
`qsv/msdk` > unknown > software. Classification is by factory-name prefix
(`vah264dec`, `vah265dec`, `vavp9dec` are all VA-API), which is a pure function
and is unit-tested on machines that have none of these decoders.

**For TASK-14:** budget for rank manipulation plus a `deep-element-added`
observer to report which decoder was really chosen; `decodebin3` will not tell
you any other way.

## Finding 2: `decodebin3` adds its pads before their caps exist

`connect_pad_added` fires with `pad.current_caps() == None`, so the usual
"is this a video pad?" test silently answers no and nothing is ever linked —
the pipeline sits in `Playing` and delivers not one buffer. The pad *name*
(`video_0`, `audio_0`) is what identifies the stream at that moment. This cost
the spike two debugging cycles and will cost TASK-14 the same if it is not
written down.

## Finding 3: frame upload cost

`Queue::write_texture` places **no alignment requirement on `bytes_per_row`**,
unlike `copy_buffer_to_texture`'s 256-byte rule. Decoder strides (VA-API pads 4K
luma to 4096, NVDEC to 3968 or 4096) are therefore uploaded verbatim, with no
CPU repack. This matters: repacking a 4K luma plane on the CPU would cost more
than the upload does.

Layout: luma into `R8Unorm`, chroma into `Rg8Unorm` at half resolution, then one
fullscreen-triangle pass sampling both and writing studio-swing Rec.709 into an
`Rgba8UnormSrgb` texture. Two `write_texture` calls, one draw, no readback.

`bench` on the CPU adapter, 60 frames each, warm-up frame excluded, GPU waited
to completion per frame:

| picture | payload/frame | `write_texture` (min/mean/p95) | + conversion pass (mean) |
| --- | --- | --- | --- |
| 1920x1080 | 3 110 400 B | 66 / 143 / 221 us | 3 193 us |
| 3840x2160 | 12 441 600 B | 584 / 733 / 842 us | 8 194 us |
| 3840x2160 (120 frames) | 12 441 600 B | 639 / 830 / 965 us | 9 037 us |

The upload half is the honest number: **~0.7 ms to move a 4K NV12 frame**, about
16 GiB/s of payload, which is a memcpy into mapped memory and will not be slower
on real hardware. The conversion half (~8 ms at 4K) is llvmpipe shading 8.3 M
pixels on the CPU and says nothing about a GPU; on any real GPU a single
fullscreen textured pass at 4K is well under a millisecond. **Treat the
conversion column as an upper bound, not a prediction.**

## Finding 4: decode-to-display latency

`decode-bench` runs the real pipeline into the real upload path with no window,
so it measures appsink hand-off -> plane copy -> both `write_texture` calls ->
conversion pass -> GPU idle. Only the swapchain present is missing. 60 frames,
VP8 in Matroska (see *Environment* for why not H.264), software `vp8dec`:

| clip | frames delivered / dropped | upload+convert (mean) | hand-off to on-GPU (min/mean/p95/max) |
| --- | --- | --- | --- |
| 1280x720 | 60 / 0 | 2 667 us | 2 083 / 2 688 / 2 927 / 10 897 us |
| 3840x2160 | 60 / 0 | 9 069 us | 7 843 / 9 097 / 9 016 / 33 127 us |

The two columns agree to within ~30 us, which is the useful result: **the
hand-off itself is free**. Everything in the latency figure is the upload and
conversion work, and on this machine the conversion is CPU shading. Nothing
queued, nothing waited on, no frame dropped by the newest-wins slot even with
llvmpipe doing the conversion — the decoder was never ahead of the consumer.

Extrapolating with the same upload number and a plausible sub-millisecond GPU
conversion, the decode-to-display budget on real hardware is roughly:

```
decoder output               ~0             (hardware decode pipelines, off the critical path)
plane copy out of GStreamer  ~0.4 ms at 4K  (one memcpy, could be removed - see Finding 5)
write_texture x2             ~0.7 ms at 4K  (measured)
conversion pass              <1 ms at 4K    (extrapolated, NOT measured on a GPU)
present                      up to one refresh interval
```

so about **1-2 ms of work plus one refresh interval**, comfortably inside a
frame at 60 Hz. The preview will be limited by decode throughput, not by the
upload path.

## Finding 5: the copy out of GStreamer is the one worth removing later

The spike copies each mapped buffer into owned `Vec<u8>` planes, deliberately:
it keeps the GStreamer buffer's lifetime out of the UI thread and it is the cost
a zero-copy path would save. At 4K that is ~12.4 MB per frame of pure memcpy on
top of the upload. Two ways out, both out of scope for the MVP:

1. Keep the `gst::Buffer` alive and pass the mapped slice to `write_texture`
   directly — removes the copy with no platform-specific code. This is the
   cheap win and TASK-14/TASK-20 should do it.
2. Import the decoder's DMA-BUF (Linux) or shared D3D texture (Windows) into
   wgpu and skip system memory entirely. This needs `wgpu_hal` and per-platform
   code, and it is what `videoconvert` in the current pipeline forecloses.

Note that pinning the appsink caps to system-memory NV12 forces a download from
the hardware decoder's own memory. That is the right default for the MVP — it
is uniform across platforms — but it is a real cost, and route 2 is what removes
it.

## Finding 6: the pop-out viewport costs nothing extra

`SpikeApp` lifts eframe's `RenderState` into the existing `sub_render::RenderContext`
(the TASK-19 type), so egui paints with the very device the converter writes
with. The converted RGB texture is registered with egui **once** via
`Renderer::register_native_texture`, and both the main window and the deferred
viewport draw that one `egui::TextureId`. Consequences:

- No second upload, no readback, no copy for the pop-out.
- The converter is rebuilt only when the source resolution changes, never per
  frame; per-frame texture allocation is what makes naive preview paths stutter.
- `show_viewport_deferred` gives the pop-out its own OS window that the window
  manager can move to another monitor. The closure eframe keeps is `Send + Sync`,
  so what it captures must be shareable — here an `Arc` holding the texture id
  and size, updated by the main pass.

**Not verified:** that the pop-out really moves to a second monitor, and that a
4K H.264 file plays in the window at all. See *Environment*.

## Environment, and what could not be tested here

| wanted | actually available |
| --- | --- |
| NVIDIA or Intel/AMD GPU | none: no `/dev/dri` at all, so no VA-API and no NVDEC |
| a GPU wgpu adapter | Mesa lavapipe (llvmpipe), a CPU renderer |
| H.264 decoders | none: `gst-libav`, `openh264` and `x264` GStreamer plugins are all absent (the `libavcodec.so.60` and `libx264.so.164` shared libraries are installed, but no plugin wraps them) |
| a second monitor | one headless X display, no compositor |

The system GStreamer 1.24 plugin set is otherwise complete (109 plugins), which
is why VP8 in Matroska could stand in for H.264: it exercises exactly the same
`uridecodebin3 -> videoconvert -> appsink(NV12)` path and the same NV12 upload,
differing only in which decoder element autoplugs. `make-clip` exists because
`gst-launch-1.0` is not installed either, so the fixtures had to be synthesised
in-process.

Two acceptance criteria therefore stand unproven and must be re-run on real
hardware:

- **4K H.264 on nvdec/va.** Needs a GPU and the codec plugins. The command is
  `spike-nv12-viewport play <file.mp4>`; the window's status line names the
  decoder that was instantiated and its kind, so "did hardware decode happen"
  is one glance.
- **The pop-out on a second monitor.** Needs a compositor and two displays.
  Under the headless X server here, the eframe window never paints a single
  frame — `play` runs, links the pipeline and creates the device, but `App::ui`
  is never called, so the windowed path is unexercised. `decode-bench` was
  written to route around exactly that, and it is what produced Finding 4.

## Recommendations

- **TASK-14** (decoder handle): raise registry ranks, do not reach for
  `autoplug-sort`; match video pads by name at `pad-added`; report the chosen
  decoder through `deep-element-added`.
- **TASK-20** (NV12 upload and shader): keep the two-plane `R8Unorm`/`Rg8Unorm`
  layout and `write_texture` with the decoder's own stride; pass the mapped
  GStreamer slice straight in rather than copying into a `Vec`.
- **TASK-17/18** (decode-ahead, frame cache): the hand-off is free, so the ring
  buffer exists to absorb decode jitter, not upload cost. Size it against
  decoder latency.
- **TASK-67** (pop-out viewport): the shared-`RenderContext` + one registered
  texture design holds. Register once, on resolution change only.
- Re-run `bench` and `decode-bench` on a GPU machine before either number is
  quoted as a budget; the conversion figures here are llvmpipe's, not a GPU's.

## Reproducing

```
cargo run --release -p spike-nv12-viewport -- bench --frames 60
cargo run --release -p spike-nv12-viewport -- make-clip /tmp/clip.webm --frames 75
cargo run --release -p spike-nv12-viewport -- decode-bench /tmp/clip.webm --frames 60
cargo run --release -p spike-nv12-viewport -- play /tmp/clip.webm
```
