//! The compositor frame graph, version 0: one video track, opacity and a
//! position/scale/rotation transform.
//!
//! docs/PLAN.md §5.3 describes the graph as, per frame and per video track
//! top-down: sample the clip frame, colour convert, apply transform and
//! opacity, blend. This module is the skeleton of that walk with a single
//! track resolved and a single quad drawn; multi-track blending (TASK-39),
//! crossfades (TASK-38) and shader effects (TASK-87) extend it rather than
//! replace it.
//!
//! Three pieces are deliberately separated so most of the behaviour is
//! testable without a GPU:
//!
//! - [`resolve_clip_at`] is pure model arithmetic. It answers "which clip is
//!   under the playhead, and what source time does it want" entirely in
//!   `RationalTime`; no float and no wgpu call is involved.
//! - [`LetterboxFit`] and [`QuadTransform`] are the geometry. A source
//!   picture is fitted into the sequence canvas (letterbox or pillarbox, as
//!   the aspect ratios demand) and the clip's transform is applied about the
//!   clip centre.
//! - [`Compositor`] is the only part that needs a device. It owns the
//!   offscreen target, the pipeline and the uniform buffer.
//!
//! Where the pictures come from is deliberately *not* decided here. The
//! compositor asks a [`FrameSource`] for the already-converted RGB texture of
//! a resolved clip, so decoding and NV12 conversion stay in `sub-media` and
//! [`crate::Nv12Converter`], and this crate keeps no GStreamer dependency.

use sub_model::{Clip, ClipId, Resolution, Sequence, TrackId, TrackKind, Transform};
use sub_time::{RationalTime, TimeRange};

use crate::context::RenderContext;
use crate::nv12::OUTPUT_FORMAT;

/// Bytes of the uniform block the compositor shader reads.
///
/// Three `vec4<f32>`: the 2x2 basis, the offset plus the canvas size, and the
/// opacity. Named so the buffer size and the packing cannot drift apart.
const UNIFORM_BYTES: usize = 48;

/// The clip the playhead sits on, and the source time it wants.
///
/// Produced by [`resolve_clip_at`]. The borrow ties the resolution to the
/// sequence it was read from, so a clip can never outlive an edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedClip<'a> {
    /// The track the clip sits on.
    pub track: TrackId,
    /// The clip itself, with its opacity and transform.
    pub clip: &'a Clip,
    /// The span the clip occupies in sequence time.
    pub timeline_range: TimeRange,
    /// The time in *source* time the playhead maps to.
    ///
    /// Exactly `clip.source_range.start() + (time - timeline_range.start())`,
    /// computed in `RationalTime`, so a 30000/1001 timebase never drifts.
    pub source_time: RationalTime,
}

impl ResolvedClip<'_> {
    /// The clip's identity, for logs and for [`FrameSummary`].
    pub fn clip_id(&self) -> ClipId {
        self.clip.id
    }
}

/// The clip under the playhead, or `None` when the playhead sits over a gap,
/// past the end of every track, or on a sequence with no video at all.
///
/// Video tracks composite top-down, so the search runs from the last track
/// backwards and takes the first clip it finds; muted tracks are skipped, as
/// a muted video track contributes nothing to the composite. That is already
/// the multi-track walk of §5.3, stopped after the first hit: v0 draws one
/// layer, and TASK-39 keeps going and blends.
///
/// `time` is in sequence time. It is rescaled to the sequence timebase for
/// the comparison, so a caller may hand in a playhead counted in nanoseconds.
pub fn resolve_clip_at(sequence: &Sequence, time: RationalTime) -> Option<ResolvedClip<'_>> {
    let rate = sequence.settings.frame_rate;
    let playhead = time.checked_rescaled_to(rate)?;
    sequence
        .tracks
        .iter()
        .rev()
        .filter(|track| track.kind == TrackKind::Video && !track.muted)
        .find_map(|track| {
            let (clip, range) = track
                .clip_placements(rate)
                .find(|(_, range)| range.contains(playhead))?;
            let offset = playhead.checked_sub(range.start())?;
            let source_time = clip.source_range.start().checked_add(offset)?;
            Some(ResolvedClip {
                track: track.id,
                clip,
                timeline_range: range,
                source_time,
            })
        })
}

/// A source picture fitted into the sequence canvas, centred, in canvas
/// pixels.
///
/// The fit *contains*: the whole picture is visible and the canvas keeps
/// black bars on the two sides where the aspect ratios disagree — pillarbox
/// for a narrower source, letterbox for a wider one. Nothing is ever cropped,
/// and a source whose aspect already matches fills the canvas exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LetterboxFit {
    width: f32,
    height: f32,
}

impl LetterboxFit {
    /// Fit a `source_width` x `source_height` picture into `canvas`.
    ///
    /// A zero source dimension collapses the fit to nothing, which draws
    /// nothing; [`Resolution`] already forbids a zero canvas.
    pub fn new(canvas: Resolution, source_width: u32, source_height: u32) -> Self {
        if source_width == 0 || source_height == 0 {
            return Self {
                width: 0.0,
                height: 0.0,
            };
        }
        let canvas_width = pixels(canvas.width());
        let canvas_height = pixels(canvas.height());
        let source_width = pixels(source_width);
        let source_height = pixels(source_height);
        let scale = (canvas_width / source_width).min(canvas_height / source_height);
        Self {
            width: source_width * scale,
            height: source_height * scale,
        }
    }

    /// Width of the fitted picture in canvas pixels.
    pub fn width(self) -> f32 {
        self.width
    }

    /// Height of the fitted picture in canvas pixels.
    pub fn height(self) -> f32 {
        self.height
    }

    /// Left edge of the fitted picture, in canvas pixels from the left of the
    /// canvas. That is also the width of each pillarbox bar.
    pub fn left(self, canvas: Resolution) -> f32 {
        (pixels(canvas.width()) - self.width) / 2.0
    }

    /// Top edge of the fitted picture, in canvas pixels from the top of the
    /// canvas. That is also the height of each letterbox bar.
    pub fn top(self, canvas: Resolution) -> f32 {
        (pixels(canvas.height()) - self.height) / 2.0
    }
}

/// Where the fitted picture lands on the canvas once the clip's transform is
/// applied.
///
/// The quad is the unit square `[-0.5, 0.5]^2`. `basis` maps it to canvas
/// pixel offsets from the canvas centre — it folds in the letterbox size, the
/// clip scale and the rotation — and `offset` translates the result. The
/// order is scale, then rotation, then translation, all about the clip
/// centre, exactly as [`Transform`] documents.
///
/// The canvas y axis points down (`Point2` is documented that way), so a
/// positive rotation turns clockwise on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuadTransform {
    /// Column-major 2x2: `[m00, m10, m01, m11]`.
    basis: [f32; 4],
    /// Clip centre offset from the canvas centre, in canvas pixels.
    offset: [f32; 2],
    /// Canvas size in pixels, which turns the above into clip space.
    canvas: [f32; 2],
}

impl QuadTransform {
    /// Place a `source_width` x `source_height` picture on `canvas` under
    /// `transform`.
    pub fn new(
        canvas: Resolution,
        source_width: u32,
        source_height: u32,
        transform: &Transform,
    ) -> Self {
        let fit = LetterboxFit::new(canvas, source_width, source_height);
        let scale_x = fit.width() * transform.scale.x().as_f32();
        let scale_y = fit.height() * transform.scale.y().as_f32();
        let (sin, cos) = transform.rotation_degrees.as_f32().to_radians().sin_cos();
        Self {
            basis: [cos * scale_x, sin * scale_x, -sin * scale_y, cos * scale_y],
            offset: [transform.position.x.as_f32(), transform.position.y.as_f32()],
            canvas: [pixels(canvas.width()), pixels(canvas.height())],
        }
    }

    /// Where one corner of the unit quad lands, in canvas pixels measured
    /// from the top-left of the canvas.
    ///
    /// `corner` is in `[-0.5, 0.5]^2` with y pointing down, so `[-0.5, -0.5]`
    /// is the top-left of the picture.
    pub fn corner(self, corner: [f32; 2]) -> [f32; 2] {
        let x = self.basis[0].mul_add(corner[0], self.basis[2] * corner[1]) + self.offset[0];
        let y = self.basis[1].mul_add(corner[0], self.basis[3] * corner[1]) + self.offset[1];
        [x + self.canvas[0] / 2.0, y + self.canvas[1] / 2.0]
    }

    /// The four corners of the placed picture in canvas pixels, in the order
    /// top-left, top-right, bottom-left, bottom-right *of the source*.
    pub fn corners(self) -> [[f32; 2]; 4] {
        [
            self.corner([-0.5, -0.5]),
            self.corner([0.5, -0.5]),
            self.corner([-0.5, 0.5]),
            self.corner([0.5, 0.5]),
        ]
    }

    /// Pack the transform and `opacity` into the shader's uniform block.
    fn uniform_bytes(self, opacity: f32) -> [u8; UNIFORM_BYTES] {
        let values = [
            self.basis[0],
            self.basis[1],
            self.basis[2],
            self.basis[3],
            self.offset[0],
            self.offset[1],
            self.canvas[0],
            self.canvas[1],
            opacity,
            0.0,
            0.0,
            0.0,
        ];
        let mut bytes = [0u8; UNIFORM_BYTES];
        for (slot, value) in bytes.chunks_exact_mut(4).zip(values) {
            slot.copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }
}

/// A picture a [`FrameSource`] hands the compositor.
///
/// The texture is already RGB — normally the output of
/// [`crate::Nv12Converter`] — and is sampled, not copied, so handing the same
/// view back every frame costs nothing.
#[derive(Debug, Clone)]
pub struct SourceFrame {
    /// The view the compositor samples.
    pub view: wgpu::TextureView,
    /// Picture width in pixels, used for the letterbox fit.
    pub width: u32,
    /// Picture height in pixels, used for the letterbox fit.
    pub height: u32,
}

impl SourceFrame {
    /// Describe `view` as a `width` x `height` picture.
    pub fn new(view: wgpu::TextureView, width: u32, height: u32) -> Self {
        Self {
            view,
            width,
            height,
        }
    }
}

/// Where the compositor gets a clip's picture.
///
/// The graph does not decode: it asks for the frame of an already-resolved
/// clip at an exact source time and draws whatever comes back. The playback
/// scheduler (TASK-23) implements this over the decoder and its cache.
/// Returning `None` means "no picture ready", and the composite is then the
/// bare black canvas rather than a stale frame.
///
/// Any `FnMut(&ResolvedClip) -> Option<SourceFrame>` is a source, which is
/// what tests and the viewer's still-frame path use.
pub trait FrameSource {
    /// The picture for `clip` at [`ResolvedClip::source_time`].
    fn frame(&mut self, clip: &ResolvedClip<'_>) -> Option<SourceFrame>;
}

impl<F> FrameSource for F
where
    F: FnMut(&ResolvedClip<'_>) -> Option<SourceFrame>,
{
    fn frame(&mut self, clip: &ResolvedClip<'_>) -> Option<SourceFrame> {
        self(clip)
    }
}

/// What one [`Compositor::render`] call did.
///
/// Enough for the viewer to show "no picture here" and for a test to assert
/// on the graph's decisions without reading pixels back.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameSummary {
    /// The clip the playhead resolved to, if any. Set even when the source
    /// had no picture ready for it.
    pub clip: Option<ClipId>,
    /// The source time asked for, if a clip was resolved.
    pub source_time: Option<RationalTime>,
    /// Where the picture landed, if one was drawn.
    pub placement: Option<QuadTransform>,
}

impl FrameSummary {
    /// True when the composite is the bare canvas: no clip, or no picture.
    pub fn is_blank(&self) -> bool {
        self.placement.is_none()
    }
}

/// The WGSL for the transform-and-opacity pass.
///
/// One quad, one texture, one uniform block. The vertex shader builds the
/// unit square from `vertex_index` so no vertex buffer is bound, and maps it
/// through the basis and offset into clip space.
const COMPOSITE_WGSL: &str = r"
struct Params {
    // Column-major 2x2 mapping the unit quad to canvas pixels.
    basis: vec4<f32>,
    // xy: clip centre offset in canvas pixels. zw: canvas size in pixels.
    offset_canvas: vec4<f32>,
    // x: opacity. The rest is padding.
    opacity: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var source_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) index: u32) -> VsOut {
    // 0,1,2,3 as a triangle strip: the unit quad's TL, TR, BL, BR.
    let corner = vec2<f32>(
        f32(index & 1u) - 0.5,
        f32(index >> 1u) - 0.5,
    );

    let canvas = params.offset_canvas.zw;
    let placed = vec2<f32>(
        params.basis.x * corner.x + params.basis.z * corner.y,
        params.basis.y * corner.x + params.basis.w * corner.y,
    ) + params.offset_canvas.xy;

    // Canvas pixels measured from the centre with y down, into clip space,
    // whose y points up.
    var out: VsOut;
    out.position = vec4<f32>(
        2.0 * placed.x / canvas.x,
        -2.0 * placed.y / canvas.y,
        0.0,
        1.0,
    );
    // The quad's y already runs downwards, which is the texture's own axis.
    out.uv = corner + vec2<f32>(0.5, 0.5);
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(source, source_sampler, in.uv);
    // Straight (non-premultiplied) alpha: the blend state multiplies by it.
    return vec4<f32>(texel.rgb, texel.a * params.opacity.x);
}
";

/// The frame graph: renders one sequence frame into an offscreen texture.
///
/// The target is [`OUTPUT_FORMAT`] with `RENDER_ATTACHMENT | TEXTURE_BINDING
/// | COPY_SRC`, which is what makes it serve both consumers §5.3 names: egui
/// registers it and samples it with no copy, and export copies it into a
/// buffer with [`Compositor::read_rgba`]. Nothing about the graph differs
/// between preview and export but the resolution.
#[derive(Debug)]
pub struct Compositor {
    context: RenderContext,
    resolution: Resolution,
    output: wgpu::Texture,
    output_view: wgpu::TextureView,
    uniforms: wgpu::Buffer,
    sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
}

impl Compositor {
    /// Build a compositor targeting a `resolution` canvas on the shared
    /// device.
    ///
    /// Use [`Compositor::for_sequence`] to take the resolution from a
    /// sequence's settings instead.
    pub fn new(context: RenderContext, resolution: Resolution) -> Self {
        let device = context.device();
        let output = target_texture(device, resolution);
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("compositor params"),
            size: UNIFORM_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("compositor source"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("compositor layer"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline = build_pipeline(device, &bind_group_layout);
        Self {
            context,
            resolution,
            output,
            output_view,
            uniforms,
            sampler,
            bind_group_layout,
            pipeline,
        }
    }

    /// Build a compositor whose canvas is `sequence`'s own resolution.
    pub fn for_sequence(context: RenderContext, sequence: &Sequence) -> Self {
        Self::new(context, sequence.settings.resolution)
    }

    /// The device and queue the compositor shares with the UI.
    pub fn context(&self) -> &RenderContext {
        &self.context
    }

    /// The canvas size currently rendered into.
    pub fn resolution(&self) -> Resolution {
        self.resolution
    }

    /// The offscreen target.
    ///
    /// Register it with egui once. [`Compositor::render`] only replaces it
    /// when the sequence resolution changes, so a consumer that caches the
    /// handle re-reads it when [`Compositor::resolution`] moves.
    pub fn output(&self) -> &wgpu::Texture {
        &self.output
    }

    /// Point the compositor at a new canvas size, rebuilding the target.
    ///
    /// A no-op when the resolution already matches, so calling it every frame
    /// is free.
    pub fn resize(&mut self, resolution: Resolution) {
        if resolution == self.resolution {
            return;
        }
        self.output = target_texture(self.context.device(), resolution);
        self.output_view = self
            .output
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.resolution = resolution;
    }

    /// Render `sequence` at `time` into the offscreen target.
    ///
    /// The canvas first follows the sequence settings, so changing a
    /// sequence's resolution needs no separate call. The target is cleared to
    /// opaque black — that black *is* the letterbox — and the clip under the
    /// playhead, if any, is drawn over it with its opacity and transform.
    pub fn render(
        &mut self,
        sequence: &Sequence,
        time: RationalTime,
        source: &mut dyn FrameSource,
    ) -> FrameSummary {
        self.resize(sequence.settings.resolution);

        let resolved = resolve_clip_at(sequence, time);
        let layer = resolved.and_then(|resolved| {
            let frame = source
                .frame(&resolved)
                .filter(|frame| frame.width > 0 && frame.height > 0)?;
            let placement = QuadTransform::new(
                self.resolution,
                frame.width,
                frame.height,
                &resolved.clip.transform,
            );
            Some((frame, placement, resolved.clip.opacity.as_f32()))
        });

        // The uniform write and the bind group are prepared before the pass:
        // a pass borrows every resource it binds for its whole lifetime.
        let bound = layer.as_ref().map(|(frame, placement, opacity)| {
            self.context.queue().write_buffer(
                &self.uniforms,
                0,
                &placement.uniform_bytes(*opacity),
            );
            self.layer_bind_group(&frame.view)
        });

        let mut encoder =
            self.context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("compositor frame"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("compositor frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.output_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some(bind_group) = bound.as_ref() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, bind_group, &[]);
                pass.draw(0..4, 0..1);
            }
        }
        self.context.queue().submit([encoder.finish()]);

        FrameSummary {
            clip: resolved.map(|resolved| resolved.clip_id()),
            source_time: resolved.map(|resolved| resolved.source_time),
            placement: layer.map(|(_, placement, _)| placement),
        }
    }

    /// Copy the target back to the CPU as tightly packed RGBA rows.
    ///
    /// This is the readback half of §5.3's "same graph serves preview and
    /// export": the row padding a texture copy demands is stripped here, so
    /// the caller gets `width * height * 4` bytes. It blocks until the copy
    /// completes, which is right for export and wrong for the preview, where
    /// the UI samples [`Compositor::output`] directly instead.
    pub fn read_rgba(&self) -> Vec<u8> {
        let width = self.resolution.width();
        let height = self.resolution.height();
        let unpadded = width * 4;
        let padded = unpadded.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let device = self.context.device();
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("compositor readback"),
            size: u64::from(padded) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("compositor readback"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.output,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.context.queue().submit([encoder.finish()]);

        buffer.slice(..).map_async(wgpu::MapMode::Read, |result| {
            debug_assert!(result.is_ok(), "the readback buffer should map");
        });
        drop(device.poll(wgpu::PollType::wait_indefinitely()));

        let mut pixels = Vec::with_capacity(unpadded as usize * height as usize);
        if let Ok(view) = buffer.slice(..).get_mapped_range() {
            for row in 0..height as usize {
                let start = row * padded as usize;
                pixels.extend_from_slice(&view[start..start + unpadded as usize]);
            }
        }
        buffer.unmap();
        pixels
    }

    /// The bind group for one layer. Source views change frame to frame, so
    /// this is built per draw; it is a handful of refcount bumps, not a GPU
    /// allocation.
    fn layer_bind_group(&self, view: &wgpu::TextureView) -> wgpu::BindGroup {
        self.context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("compositor layer"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            })
    }
}

/// A dimension as a float, for the geometry. Canvas sizes in `u32` are far
/// inside `f32`'s exactly representable integers.
#[allow(clippy::cast_precision_loss)]
fn pixels(value: u32) -> f32 {
    value as f32
}

/// Create the offscreen target for `resolution`.
fn target_texture(device: &wgpu::Device, resolution: Resolution) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("compositor target"),
        size: wgpu::Extent3d {
            width: resolution.width(),
            height: resolution.height(),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OUTPUT_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// Compile the composite shader into a pipeline for `layout`.
fn build_pipeline(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("compositor"),
        source: wgpu::ShaderSource::Wgsl(COMPOSITE_WGSL.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("compositor"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("compositor"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: OUTPUT_FORMAT,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            // A mirroring scale flips the winding, so neither face is culled.
            cull_mode: None,
            ..wgpu::PrimitiveState::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{LetterboxFit, QuadTransform, resolve_clip_at};
    use sub_model::params::{Fixed6, Point2, Scale2};
    use sub_model::{
        Clip, Gap, MediaId, Opacity, Resolution, Sequence, SequenceSettings, Track, TrackKind,
        Transform,
    };
    use sub_time::{Rational, RationalTime, TimeRange};

    /// How far two canvas-pixel coordinates may differ and still agree.
    const EPSILON: f32 = 0.001;

    fn range(start: i64, duration: i64) -> TimeRange {
        TimeRange::new(
            RationalTime::new(start, Rational::FPS_24),
            RationalTime::new(duration, Rational::FPS_24),
        )
        .expect("a positive duration is a valid range")
    }

    fn frames(count: i64) -> RationalTime {
        RationalTime::new(count, Rational::FPS_24)
    }

    /// A sequence with one video track: a 10-frame clip, a 5-frame gap and a
    /// second 10-frame clip whose source starts at frame 100.
    fn sequence_with_a_gap() -> Sequence {
        let media = MediaId::new();
        let mut track = Track::new("V1", TrackKind::Video);
        track
            .items
            .push(Clip::new("first", media, range(0, 10)).into());
        track.items.push(
            Gap {
                duration: frames(5),
            }
            .into(),
        );
        track
            .items
            .push(Clip::new("second", media, range(100, 10)).into());
        let mut sequence = Sequence::new("Main", SequenceSettings::default());
        sequence.tracks.push(track);
        sequence
    }

    #[test]
    fn the_playhead_resolves_to_the_clip_it_sits_on() {
        let sequence = sequence_with_a_gap();
        let first = sequence.tracks[0].items[0]
            .as_clip()
            .expect("the first item is a clip");
        let second = sequence.tracks[0].items[2]
            .as_clip()
            .expect("the third item is a clip");

        let head = resolve_clip_at(&sequence, frames(0)).expect("frame 0 is on the first clip");
        assert_eq!(head.clip_id(), first.id);
        assert_eq!(head.source_time, frames(0));
        assert_eq!(head.timeline_range, range(0, 10));

        let inside = resolve_clip_at(&sequence, frames(7)).expect("frame 7 is on the first clip");
        assert_eq!(inside.clip_id(), first.id);
        assert_eq!(inside.source_time, frames(7));

        // Frame 15 is the first frame of the second clip, whose source range
        // starts at 100: the offset is added in source time, not sequence
        // time.
        let after_gap =
            resolve_clip_at(&sequence, frames(16)).expect("frame 16 is on the second clip");
        assert_eq!(after_gap.clip_id(), second.id);
        assert_eq!(after_gap.source_time, frames(101));
        assert_eq!(after_gap.track, sequence.tracks[0].id);
    }

    #[test]
    fn a_clips_last_frame_belongs_to_it_and_the_next_one_does_not() {
        let sequence = sequence_with_a_gap();
        let first = sequence.tracks[0].items[0]
            .as_clip()
            .expect("the first item is a clip");

        let last = resolve_clip_at(&sequence, frames(9)).expect("frame 9 is the clip's last");
        assert_eq!(last.clip_id(), first.id);
        assert_eq!(last.source_time, frames(9));
        // Frame 10 is the first frame of the gap: ranges are end-exclusive.
        assert!(resolve_clip_at(&sequence, frames(10)).is_none());
    }

    #[test]
    fn gaps_and_the_time_past_the_end_resolve_to_nothing() {
        let sequence = sequence_with_a_gap();
        for frame in [10, 11, 14] {
            assert!(
                resolve_clip_at(&sequence, frames(frame)).is_none(),
                "frame {frame} is inside the gap"
            );
        }
        assert!(resolve_clip_at(&sequence, frames(25)).is_none());
        assert!(resolve_clip_at(&sequence, frames(-1)).is_none());
        assert!(
            resolve_clip_at(
                &Sequence::new("Empty", SequenceSettings::default()),
                frames(0)
            )
            .is_none()
        );
    }

    #[test]
    fn a_playhead_at_another_rate_is_rescaled_before_the_lookup() {
        let sequence = sequence_with_a_gap();
        // Frame 3 at 24 fps is 0.125 s, which is 12000 ticks at 96 kHz.
        let audio_rate = Rational::new(96_000, 1).expect("96 kHz is a valid rate");
        let playhead = RationalTime::new(12_000, audio_rate);
        let resolved = resolve_clip_at(&sequence, playhead).expect("0.125 s is on the first clip");
        assert_eq!(resolved.source_time, frames(3));
    }

    #[test]
    fn the_top_unmuted_video_track_wins() {
        let media = MediaId::new();
        let mut sequence = Sequence::new("Main", SequenceSettings::default());

        let mut bottom = Track::new("V1", TrackKind::Video);
        bottom
            .items
            .push(Clip::new("bottom", media, range(0, 10)).into());
        let bottom_clip = bottom.items[0].as_clip().expect("a clip").id;

        let mut top = Track::new("V2", TrackKind::Video);
        top.items.push(Clip::new("top", media, range(0, 10)).into());
        let top_clip = top.items[0].as_clip().expect("a clip").id;

        let mut audio = Track::new("A1", TrackKind::Audio);
        audio
            .items
            .push(Clip::new("sound", media, range(0, 10)).into());

        sequence.tracks.push(bottom);
        sequence.tracks.push(top);
        sequence.tracks.push(audio);

        // Tracks composite bottom-first, so the last video track is on top.
        let resolved = resolve_clip_at(&sequence, frames(2)).expect("frame 2 is covered");
        assert_eq!(resolved.clip_id(), top_clip);

        // Muting the top track falls through to the one below it; an audio
        // track is never a picture source.
        sequence.tracks[1].muted = true;
        let resolved = resolve_clip_at(&sequence, frames(2)).expect("the lower track still covers");
        assert_eq!(resolved.clip_id(), bottom_clip);

        sequence.tracks[0].muted = true;
        assert!(resolve_clip_at(&sequence, frames(2)).is_none());
    }

    #[test]
    fn a_matching_aspect_fills_the_canvas_exactly() {
        let canvas = Resolution::HD_1080;
        let fit = LetterboxFit::new(canvas, 1280, 720);
        assert!((fit.width() - 1920.0).abs() < EPSILON);
        assert!((fit.height() - 1080.0).abs() < EPSILON);
        assert!(fit.left(canvas).abs() < EPSILON);
        assert!(fit.top(canvas).abs() < EPSILON);
    }

    #[test]
    fn a_narrower_source_is_pillarboxed_and_a_wider_one_letterboxed() {
        let canvas = Resolution::HD_1080;

        // 4:3 into 16:9: full height, bars left and right.
        let pillar = LetterboxFit::new(canvas, 1440, 1080);
        assert!((pillar.height() - 1080.0).abs() < EPSILON);
        assert!((pillar.width() - 1440.0).abs() < EPSILON);
        assert!((pillar.left(canvas) - 240.0).abs() < EPSILON);
        assert!(pillar.top(canvas).abs() < EPSILON);

        // 2.39:1 into 16:9: full width, bars top and bottom.
        let letter = LetterboxFit::new(canvas, 2048, 858);
        assert!((letter.width() - 1920.0).abs() < EPSILON);
        assert!((letter.height() - 804.375).abs() < EPSILON);
        assert!(letter.left(canvas).abs() < EPSILON);
        assert!((letter.top(canvas) - 137.8125).abs() < EPSILON);
    }

    #[test]
    fn a_zero_sized_source_fits_to_nothing() {
        let fit = LetterboxFit::new(Resolution::HD_1080, 0, 720);
        assert!(fit.width().abs() < EPSILON);
        assert!(fit.height().abs() < EPSILON);
    }

    /// Assert two canvas-pixel points agree.
    fn assert_point(actual: [f32; 2], expected: [f32; 2], what: &str) {
        assert!(
            (actual[0] - expected[0]).abs() < EPSILON && (actual[1] - expected[1]).abs() < EPSILON,
            "{what}: got {actual:?}, expected {expected:?}"
        );
    }

    #[test]
    fn an_identity_transform_centres_the_letterboxed_picture() {
        let canvas = Resolution::HD_1080;
        let quad = QuadTransform::new(canvas, 1440, 1080, &Transform::IDENTITY);
        let [top_left, top_right, bottom_left, bottom_right] = quad.corners();
        assert_point(top_left, [240.0, 0.0], "top left");
        assert_point(top_right, [1680.0, 0.0], "top right");
        assert_point(bottom_left, [240.0, 1080.0], "bottom left");
        assert_point(bottom_right, [1680.0, 1080.0], "bottom right");
    }

    #[test]
    fn position_moves_the_picture_in_canvas_pixels_with_y_down() {
        let canvas = Resolution::HD_1080;
        let transform = Transform::new(
            Point2::new(Fixed6::from_units(120), Fixed6::from_units(-40)),
            Scale2::UNIFORM,
            Fixed6::ZERO,
        );
        let quad = QuadTransform::new(canvas, 1920, 1080, &transform);
        assert_point(quad.corner([-0.5, -0.5]), [120.0, -40.0], "top left");
        assert_point(quad.corner([0.5, 0.5]), [2040.0, 1040.0], "bottom right");
    }

    #[test]
    fn scale_grows_the_picture_about_its_centre() {
        let canvas = Resolution::HD_1080;
        let transform = Transform::new(
            Point2::ORIGIN,
            Scale2::uniform(Fixed6::from_units(2)).expect("2x is a valid scale"),
            Fixed6::ZERO,
        );
        let quad = QuadTransform::new(canvas, 1920, 1080, &transform);
        // Twice the size, still centred: it overhangs the canvas equally.
        assert_point(quad.corner([-0.5, -0.5]), [-960.0, -540.0], "top left");
        assert_point(quad.corner([0.5, 0.5]), [2880.0, 1620.0], "bottom right");

        // A negative axis mirrors: the source's left edge lands on the right.
        let mirrored = Transform::new(
            Point2::ORIGIN,
            Scale2::new(Fixed6::from_units(-1), Fixed6::ONE).expect("a mirror is a valid scale"),
            Fixed6::ZERO,
        );
        let quad = QuadTransform::new(canvas, 1920, 1080, &mirrored);
        assert_point(
            quad.corner([-0.5, -0.5]),
            [1920.0, 0.0],
            "mirrored top left",
        );
    }

    #[test]
    fn rotation_turns_clockwise_on_screen() {
        // A square source on a square canvas keeps the arithmetic obvious.
        let canvas = Resolution::new(1000, 1000).expect("a square canvas is valid");
        let transform = Transform::new(Point2::ORIGIN, Scale2::UNIFORM, Fixed6::from_units(90));
        let quad = QuadTransform::new(canvas, 500, 500, &transform);
        // 90 degrees clockwise with y down takes the top-left corner to the
        // top-right of the canvas.
        assert_point(quad.corner([-0.5, -0.5]), [1000.0, 0.0], "top left");
        assert_point(quad.corner([0.5, -0.5]), [1000.0, 1000.0], "top right");
        assert_point(quad.corner([0.5, 0.5]), [0.0, 1000.0], "bottom right");
    }

    #[test]
    fn the_uniform_block_carries_the_basis_offset_canvas_and_opacity() {
        let canvas = Resolution::new(1920, 1080).expect("HD is valid");
        let transform = Transform::new(
            Point2::new(Fixed6::from_units(10), Fixed6::from_units(-20)),
            Scale2::UNIFORM,
            Fixed6::ZERO,
        );
        let quad = QuadTransform::new(canvas, 1920, 1080, &transform);
        let bytes = quad.uniform_bytes(Opacity::from_f64(0.5).expect("half is valid").as_f32());
        let values: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("four bytes")))
            .collect();
        assert_eq!(values.len(), 12);
        assert!((values[0] - 1920.0).abs() < EPSILON, "basis m00");
        assert!(values[1].abs() < EPSILON, "basis m10");
        assert!(values[2].abs() < EPSILON, "basis m01");
        assert!((values[3] - 1080.0).abs() < EPSILON, "basis m11");
        assert!((values[4] - 10.0).abs() < EPSILON, "offset x");
        assert!((values[5] + 20.0).abs() < EPSILON, "offset y");
        assert!((values[6] - 1920.0).abs() < EPSILON, "canvas width");
        assert!((values[7] - 1080.0).abs() < EPSILON, "canvas height");
        assert!((values[8] - 0.5).abs() < EPSILON, "opacity");
    }
}
