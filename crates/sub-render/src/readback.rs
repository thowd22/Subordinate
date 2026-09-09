//! Full-resolution readback of composited frames, for export.
//!
//! docs/PLAN.md §5.3 has one frame graph serving both consumers. The preview
//! samples [`Compositor::output`] on the GPU and may drop frames; export must
//! drop none and needs the picture on the CPU, where the encoder's appsrc can
//! take it. That is this module.
//!
//! The naive way — copy the target into a fresh buffer, map it, wait — makes
//! the CPU and the GPU take turns: nothing is drawn while a frame is being
//! unpacked, and every frame allocates. [`FrameReadback`] instead keeps a
//! small ring of staging buffers ([`StagingRing`]). Frame *N*'s copy lands in
//! its own buffer, so frame *N+1* can be drawn and copied while *N* is still
//! mapped and being unpacked, and the buffers are reused for the length of
//! the export rather than reallocated per frame.
//!
//! Two shapes of use:
//!
//! - [`FrameReadback::render_frame_to_buffer`] renders one frame and blocks
//!   until its pixels are back. Simple, and what a still export or a test
//!   wants.
//! - [`FrameReadback::submit`] plus [`FrameReadback::receive`] is the export
//!   loop: keep up to [`FrameReadback::depth`] frames in flight and take them
//!   out in submission order. This is where the overlap actually happens.
//!
//! Readback never runs on the UI thread. `FrameReadback` is [`Send`], the
//! export job owns one on its own thread, and the UI's own path
//! ([`Compositor::output`] registered with egui) has no readback in it at
//! all.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use sub_model::{Resolution, Sequence};
use sub_time::RationalTime;

use crate::context::RenderContext;
use crate::error::RenderError;
use crate::graph::{Compositor, FrameSource, FrameSummary};

/// Bytes per pixel of a readback frame: [`crate::OUTPUT_FORMAT`] is 8-bit
/// RGBA.
pub const BYTES_PER_PIXEL: u32 = 4;

/// How many frames a [`FrameReadback`] keeps in flight unless told otherwise.
///
/// Three is the usual triple buffer: one being drawn, one being copied, one
/// being unpacked.
pub const DEFAULT_DEPTH: usize = 3;

/// Map state of one staging buffer, shared with the wgpu map callback.
const UNMAPPED: u8 = 0;
/// A map was asked for and the callback has not run yet.
const MAPPING: u8 = 1;
/// The callback ran and the buffer is readable.
const MAPPED: u8 = 2;
/// The callback ran and the map failed.
const MAP_FAILED: u8 = 3;

/// One composited frame, on the CPU.
///
/// Rows are tightly packed — the row padding a texture-to-buffer copy demands
/// is stripped on the way out — so [`FrameBuffer::pixels`] is exactly
/// `width * height * 4` bytes of RGBA in the compositor's output format.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameBuffer {
    time: RationalTime,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    summary: FrameSummary,
}

impl FrameBuffer {
    /// The sequence time this frame was rendered at.
    pub fn time(&self) -> RationalTime {
        self.time
    }

    /// Canvas width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Canvas height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Bytes in one row of [`FrameBuffer::pixels`]: `width * 4`.
    pub fn bytes_per_row(&self) -> u32 {
        self.width * BYTES_PER_PIXEL
    }

    /// Tightly packed RGBA rows, top row first.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Take the pixels, leaving the frame's metadata behind.
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }

    /// What the graph resolved for this frame: layers, opacities, placements.
    pub fn summary(&self) -> &FrameSummary {
        &self.summary
    }
}

/// One reusable staging buffer plus the map state its callback writes.
#[derive(Debug, Clone)]
struct Staging {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
}

impl Staging {
    /// Allocate a staging buffer of `size` bytes.
    fn new(device: &wgpu::Device, size: u64) -> Self {
        Self {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("compositor readback"),
                size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            state: Arc::new(AtomicU8::new(UNMAPPED)),
        }
    }
}

/// A ring of staging buffers reused across frames.
///
/// Each slot holds a buffer big enough for the largest frame it has been
/// asked for; a slot only ever reallocates when a bigger canvas arrives, and
/// a buffer still in flight is never disturbed because the frame waiting on
/// it holds its own handle.
#[derive(Debug)]
pub struct StagingRing {
    depth: usize,
    slots: Vec<Staging>,
    next: usize,
}

impl StagingRing {
    /// A ring of `depth` empty slots. A depth of zero is raised to one: there
    /// must be somewhere to copy into.
    pub fn new(depth: usize) -> Self {
        let depth = depth.max(1);
        Self {
            depth,
            slots: Vec::with_capacity(depth),
            next: 0,
        }
    }

    /// How many frames the ring can hold in flight.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// The next slot, grown to `size` bytes if it is not already that big.
    ///
    /// Slots are allocated lazily so a ring that is never used costs nothing.
    fn acquire(&mut self, device: &wgpu::Device, size: u64) -> Staging {
        let depth = self.depth;
        if self.slots.len() < depth {
            self.slots.push(Staging::new(device, size));
        } else if self.slots[self.next].buffer.size() < size {
            self.slots[self.next] = Staging::new(device, size);
        }
        let index = self.next.min(self.slots.len() - 1);
        self.next = (index + 1) % depth;
        self.slots[index].clone()
    }
}

/// A frame whose copy has been submitted and whose pixels are not back yet.
#[derive(Debug)]
struct InFlight {
    staging: Staging,
    submission: wgpu::SubmissionIndex,
    time: RationalTime,
    width: u32,
    height: u32,
    padded_row: u32,
    summary: FrameSummary,
}

impl InFlight {
    /// Bytes of the buffer the copy actually wrote.
    fn mapped_len(&self) -> u64 {
        u64::from(self.padded_row) * u64::from(self.height)
    }
}

/// The export-side compositor: renders a sequence frame and hands it back on
/// the CPU.
///
/// Owns the [`Compositor`] it reads from, so nothing else can resize the
/// target underneath a frame that is still in flight.
#[derive(Debug)]
pub struct FrameReadback {
    compositor: Compositor,
    ring: StagingRing,
    pending: VecDeque<InFlight>,
}

impl FrameReadback {
    /// Wrap `compositor` with a [`DEFAULT_DEPTH`] staging ring.
    pub fn new(compositor: Compositor) -> Self {
        Self::with_depth(compositor, DEFAULT_DEPTH)
    }

    /// Wrap `compositor` with a ring `depth` frames deep.
    ///
    /// A depth of one is a plain synchronous readback: no overlap, one
    /// buffer. Anything more lets the GPU draw ahead of the CPU.
    pub fn with_depth(compositor: Compositor, depth: usize) -> Self {
        let ring = StagingRing::new(depth);
        let pending = VecDeque::with_capacity(ring.depth());
        Self {
            compositor,
            ring,
            pending,
        }
    }

    /// The compositor being read from.
    pub fn compositor(&self) -> &Compositor {
        &self.compositor
    }

    /// Give the compositor back, dropping any frames still in flight.
    pub fn into_compositor(self) -> Compositor {
        self.compositor
    }

    /// The device and queue shared with the UI.
    pub fn context(&self) -> &RenderContext {
        self.compositor.context()
    }

    /// The canvas the last render used.
    pub fn resolution(&self) -> Resolution {
        self.compositor.resolution()
    }

    /// How many frames may be in flight at once.
    pub fn depth(&self) -> usize {
        self.ring.depth()
    }

    /// How many submitted frames have not been received yet.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// `true` when [`FrameReadback::submit`] would refuse: the ring is full
    /// and a frame must be received before another is drawn.
    pub fn is_full(&self) -> bool {
        self.pending.len() >= self.depth()
    }

    /// Render `sequence` at `time` and start a copy of the target into the
    /// next staging buffer.
    ///
    /// Returns as soon as the work is queued; the pixels come out of
    /// [`FrameReadback::receive`], in submission order.
    ///
    /// # Errors
    ///
    /// [`RenderError::ReadbackRingFull`] when [`FrameReadback::depth`] frames
    /// are already in flight. Receive one first: dropping a frame instead is
    /// exactly what export may not do.
    pub fn submit(
        &mut self,
        sequence: &Sequence,
        time: RationalTime,
        source: &mut dyn FrameSource,
    ) -> Result<(), RenderError> {
        if self.is_full() {
            return Err(RenderError::ReadbackRingFull {
                depth: self.depth(),
            });
        }
        let summary = self.compositor.render(sequence, time, source);
        let resolution = self.compositor.resolution();
        let width = resolution.width();
        let height = resolution.height();
        let padded_row = padded_row_bytes(width);
        let size = u64::from(padded_row) * u64::from(height);

        let device = self.compositor.context().device().clone();
        let staging = self.ring.acquire(&device, size);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("compositor readback"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: self.compositor.output(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let submission = self.compositor.context().queue().submit([encoder.finish()]);

        // The map is asked for now, so the driver can satisfy it as soon as
        // the copy retires rather than when the CPU next asks.
        staging.state.store(MAPPING, Ordering::Release);
        let state = Arc::clone(&staging.state);
        staging
            .buffer
            .slice(..size)
            .map_async(wgpu::MapMode::Read, move |result| {
                let outcome = if result.is_ok() { MAPPED } else { MAP_FAILED };
                state.store(outcome, Ordering::Release);
            });

        self.pending.push_back(InFlight {
            staging,
            submission,
            time,
            width,
            height,
            padded_row,
            summary,
        });
        Ok(())
    }

    /// Take the oldest frame in flight, blocking until it is readable.
    ///
    /// `Ok(None)` means nothing was submitted, not that a frame was lost.
    ///
    /// # Errors
    ///
    /// [`RenderError::ReadbackFailed`] when the device cannot be polled or
    /// the staging buffer will not map.
    pub fn receive(&mut self) -> Result<Option<FrameBuffer>, RenderError> {
        let Some(frame) = self.pending.pop_front() else {
            return Ok(None);
        };
        let device = self.compositor.context().device();
        // Waiting on this frame's own submission, not the newest one, is what
        // keeps later frames overlapping instead of being waited on too.
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(frame.submission.clone()),
                timeout: None,
            })
            .map_err(|error| RenderError::ReadbackFailed {
                reason: error.to_string(),
            })?;
        if frame.staging.state.load(Ordering::Acquire) == MAPPING {
            // The copy has retired but the map callback has not been
            // delivered; one more poll flushes the queue's callbacks.
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .map_err(|error| RenderError::ReadbackFailed {
                    reason: error.to_string(),
                })?;
        }
        match frame.staging.state.load(Ordering::Acquire) {
            MAPPED => {}
            MAP_FAILED => {
                frame.staging.state.store(UNMAPPED, Ordering::Release);
                return Err(RenderError::ReadbackFailed {
                    reason: "the staging buffer refused to map".to_owned(),
                });
            }
            _ => {
                return Err(RenderError::ReadbackFailed {
                    reason: "the staging buffer never became readable".to_owned(),
                });
            }
        }

        let unpadded = frame.width * BYTES_PER_PIXEL;
        let rows = frame.height as usize;
        let mut pixels = Vec::with_capacity(unpadded as usize * rows);
        let slice = frame.staging.buffer.slice(..frame.mapped_len());
        let view = slice
            .get_mapped_range()
            .map_err(|error| RenderError::ReadbackFailed {
                reason: error.to_string(),
            })?;
        for row in 0..rows {
            let start = row * frame.padded_row as usize;
            pixels.extend_from_slice(&view[start..start + unpadded as usize]);
        }
        drop(view);
        frame.staging.buffer.unmap();
        frame.staging.state.store(UNMAPPED, Ordering::Release);

        Ok(Some(FrameBuffer {
            time: frame.time,
            width: frame.width,
            height: frame.height,
            pixels,
            summary: frame.summary,
        }))
    }

    /// Receive every frame still in flight, oldest first.
    ///
    /// The tail of an export loop: submit until the source runs out, then
    /// drain what the ring is still holding.
    ///
    /// # Errors
    ///
    /// As [`FrameReadback::receive`].
    pub fn drain(&mut self) -> Result<Vec<FrameBuffer>, RenderError> {
        let mut frames = Vec::with_capacity(self.pending.len());
        while let Some(frame) = self.receive()? {
            frames.push(frame);
        }
        Ok(frames)
    }

    /// Render `sequence` at `time` and block until its pixels are back.
    ///
    /// This is the one-shot form: it needs the ring empty, so it cannot
    /// silently return some earlier frame's pixels. Interleave it with
    /// [`FrameReadback::submit`] only after draining.
    ///
    /// # Errors
    ///
    /// [`RenderError::ReadbackPending`] when frames submitted earlier have
    /// not been received, and otherwise as [`FrameReadback::submit`] and
    /// [`FrameReadback::receive`].
    pub fn render_frame_to_buffer(
        &mut self,
        sequence: &Sequence,
        time: RationalTime,
        source: &mut dyn FrameSource,
    ) -> Result<FrameBuffer, RenderError> {
        if !self.pending.is_empty() {
            return Err(RenderError::ReadbackPending {
                pending: self.pending.len(),
            });
        }
        self.submit(sequence, time, source)?;
        self.receive()?.ok_or_else(|| RenderError::ReadbackFailed {
            reason: "the frame just submitted was not in flight".to_owned(),
        })
    }
}

/// Row pitch a texture-to-buffer copy demands for a `width`-pixel RGBA row:
/// the packed width rounded up to [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`].
pub fn padded_row_bytes(width: u32) -> u32 {
    (width * BYTES_PER_PIXEL).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
}

#[cfg(test)]
mod tests {
    use super::{BYTES_PER_PIXEL, DEFAULT_DEPTH, StagingRing, padded_row_bytes};

    #[test]
    fn a_padded_row_is_the_packed_row_rounded_up() {
        assert_eq!(padded_row_bytes(64), 256);
        assert_eq!(padded_row_bytes(65), 512);
        assert_eq!(padded_row_bytes(1920), 1920 * BYTES_PER_PIXEL);
        assert_eq!(padded_row_bytes(1), 256);
    }

    #[test]
    fn a_padded_row_never_loses_pixels() {
        for width in 1..600_u32 {
            assert!(padded_row_bytes(width) >= width * BYTES_PER_PIXEL);
            assert_eq!(
                padded_row_bytes(width) % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
                0
            );
        }
    }

    #[test]
    fn a_ring_always_has_at_least_one_slot() {
        assert_eq!(StagingRing::new(0).depth(), 1);
        assert_eq!(StagingRing::new(1).depth(), 1);
        assert_eq!(StagingRing::new(DEFAULT_DEPTH).depth(), DEFAULT_DEPTH);
    }
}
