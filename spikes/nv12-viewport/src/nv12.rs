//! NV12 plane geometry and the wgpu upload / BT.709 conversion path.
//!
//! NV12 is what every hardware decoder hands back, so it is what the preview
//! path has to accept: one full-resolution 8-bit luma plane followed by one
//! half-resolution plane of interleaved Cb/Cr pairs. Both planes arrive with
//! a *stride* that the decoder chose, which is almost never the picture width
//! (VA-API likes multiples of 64, NVDEC likes 256), so nothing here may assume
//! `stride == width`.
//!
//! The upload is two [`wgpu::Queue::write_texture`] calls — `R8Unorm` for
//! luma, `Rg8Unorm` for chroma — followed by one fullscreen render pass that
//! samples both and writes Rec.709 RGB into an `Rgba8UnormSrgb` texture the UI
//! can hand straight to egui. `write_texture` places no alignment requirement
//! on `bytes_per_row`, so decoder strides are copied verbatim with no repack
//! on the CPU.

use sub_core::{SubError, SubResult};

use crate::codes;

/// Texture format the converted picture lands in.
///
/// sRGB, because the frame carries gamma-encoded Rec.709 values and the
/// hardware's sRGB decode on sample is a close enough inverse for a preview
/// (decision-3 assumes Rec.709 throughout).
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Where the two planes of one NV12 picture live in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Nv12Geometry {
    width: u32,
    height: u32,
    y_stride: u32,
    uv_stride: u32,
}

impl Nv12Geometry {
    /// Describe an NV12 picture of `width` x `height` whose planes are laid
    /// out with the given strides in bytes.
    ///
    /// # Errors
    ///
    /// [`codes::BAD_GEOMETRY`] when a dimension is zero or a stride is too
    /// short to hold one row of samples.
    pub fn new(width: u32, height: u32, y_stride: u32, uv_stride: u32) -> SubResult<Self> {
        if width == 0 || height == 0 {
            return Err(SubError::new(
                codes::BAD_GEOMETRY,
                format!("NV12 picture must have a non-zero size, got {width}x{height}"),
            ));
        }
        let chroma_width = width.div_ceil(2);
        if y_stride < width {
            return Err(SubError::new(
                codes::BAD_GEOMETRY,
                format!("luma stride {y_stride} is shorter than the {width}-pixel row"),
            ));
        }
        // One chroma "pixel" is a Cb/Cr pair: two bytes.
        let chroma_row_bytes = chroma_width * 2;
        if uv_stride < chroma_row_bytes {
            return Err(SubError::new(
                codes::BAD_GEOMETRY,
                format!(
                    "chroma stride {uv_stride} is shorter than the {chroma_row_bytes}-byte row"
                ),
            ));
        }
        Ok(Self {
            width,
            height,
            y_stride,
            uv_stride,
        })
    }

    /// The tightly packed layout: strides equal to the row lengths. This is
    /// what a synthesised frame uses and what `videoconvert` produces for
    /// widths that are already a multiple of four.
    ///
    /// # Errors
    ///
    /// As [`Nv12Geometry::new`].
    pub fn packed(width: u32, height: u32) -> SubResult<Self> {
        Self::new(width, height, width, width.div_ceil(2) * 2)
    }

    /// Picture width in pixels.
    pub fn width(self) -> u32 {
        self.width
    }

    /// Picture height in pixels.
    pub fn height(self) -> u32 {
        self.height
    }

    /// Bytes between the starts of two luma rows.
    pub fn y_stride(self) -> u32 {
        self.y_stride
    }

    /// Bytes between the starts of two chroma rows.
    pub fn uv_stride(self) -> u32 {
        self.uv_stride
    }

    /// Chroma plane width in Cb/Cr pairs.
    pub fn chroma_width(self) -> u32 {
        self.width.div_ceil(2)
    }

    /// Chroma plane height in rows.
    pub fn chroma_height(self) -> u32 {
        self.height.div_ceil(2)
    }

    /// Bytes the luma plane occupies, strides included.
    pub fn y_plane_len(self) -> usize {
        self.y_stride as usize * self.height as usize
    }

    /// Bytes the chroma plane occupies, strides included.
    pub fn uv_plane_len(self) -> usize {
        self.uv_stride as usize * self.chroma_height() as usize
    }

    /// Bytes one whole picture occupies when both planes are contiguous.
    pub fn frame_len(self) -> usize {
        self.y_plane_len() + self.uv_plane_len()
    }

    /// Bytes of *picture* (strides excluded) that cross the `PCIe` bus per
    /// frame. This is the number the upload cost should be judged against.
    pub fn payload_len(self) -> usize {
        let luma = self.width as usize * self.height as usize;
        let chroma = self.chroma_width() as usize * 2 * self.chroma_height() as usize;
        luma + chroma
    }

    /// Check that `y` and `uv` are long enough for this geometry.
    ///
    /// # Errors
    ///
    /// [`codes::SHORT_PLANE`] naming the plane that is too small.
    pub fn check_planes(self, y: &[u8], uv: &[u8]) -> SubResult<()> {
        if y.len() < self.y_plane_len() {
            return Err(SubError::new(
                codes::SHORT_PLANE,
                format!(
                    "luma plane holds {} bytes, {} needed",
                    y.len(),
                    self.y_plane_len()
                ),
            ));
        }
        if uv.len() < self.uv_plane_len() {
            return Err(SubError::new(
                codes::SHORT_PLANE,
                format!(
                    "chroma plane holds {} bytes, {} needed",
                    uv.len(),
                    self.uv_plane_len()
                ),
            ));
        }
        Ok(())
    }
}

/// The WGSL that turns the two sampled planes into Rec.709 RGB.
const CONVERT_WGSL: &str = r"
struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// One oversized triangle covers the target with no vertex buffer at all.
@vertex
fn vs(@builtin(vertex_index) index: u32) -> VsOut {
    var out: VsOut;
    let x = f32((index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(index & 2u) * 2.0 - 1.0;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

@group(0) @binding(0) var luma: texture_2d<f32>;
@group(0) @binding(1) var chroma: texture_2d<f32>;
@group(0) @binding(2) var plane_sampler: sampler;

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    // Studio-swing Rec.709: luma 16..235, chroma 16..240, both over 255.
    let y = (textureSample(luma, plane_sampler, in.uv).r * 255.0 - 16.0) / 219.0;
    let cbcr = textureSample(chroma, plane_sampler, in.uv).rg * 255.0 - 128.0;
    let cb = cbcr.r / 224.0;
    let cr = cbcr.g / 224.0;
    let rgb = vec3<f32>(
        y + 1.5748 * cr,
        y - 0.1873 * cb - 0.4681 * cr,
        y + 1.8556 * cb,
    );
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
";

/// Owns the two plane textures, the conversion pipeline and the RGB target
/// for one picture size.
///
/// One converter is built per source resolution and reused for every frame:
/// allocating textures per frame is what makes naive preview paths stutter.
#[derive(Debug)]
pub struct Nv12Converter {
    geometry: Nv12Geometry,
    luma: wgpu::Texture,
    chroma: wgpu::Texture,
    output: wgpu::Texture,
    output_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
}

impl Nv12Converter {
    /// Build the textures and pipeline for `geometry` on `device`.
    pub fn new(device: &wgpu::Device, geometry: Nv12Geometry) -> Self {
        let luma = plane_texture(
            device,
            "spike nv12 luma",
            wgpu::TextureFormat::R8Unorm,
            geometry.width(),
            geometry.height(),
        );
        let chroma = plane_texture(
            device,
            "spike nv12 chroma",
            wgpu::TextureFormat::Rg8Unorm,
            geometry.chroma_width(),
            geometry.chroma_height(),
        );
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("spike nv12 rgb"),
            size: wgpu::Extent3d {
                width: geometry.width(),
                height: geometry.height(),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("spike nv12 sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("spike nv12 planes"),
            entries: &[
                sampled_texture_entry(0),
                sampled_texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spike nv12 planes"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &luma.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &chroma.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let pipeline = build_pipeline(device, &layout);

        Self {
            geometry,
            luma,
            chroma,
            output,
            output_view,
            bind_group,
            pipeline,
        }
    }

    /// The geometry this converter was built for.
    pub fn geometry(&self) -> Nv12Geometry {
        self.geometry
    }

    /// The RGB texture the conversion pass writes into.
    ///
    /// Registered with egui once and then reused, so the pop-out viewport and
    /// the main window sample the very same GPU memory.
    pub fn output(&self) -> &wgpu::Texture {
        &self.output
    }

    /// Copy both planes into their textures.
    ///
    /// # Errors
    ///
    /// [`codes::SHORT_PLANE`] when a slice is too small for the geometry;
    /// nothing is uploaded in that case.
    pub fn upload(&self, queue: &wgpu::Queue, y: &[u8], uv: &[u8]) -> SubResult<()> {
        self.geometry.check_planes(y, uv)?;
        write_plane(
            queue,
            &self.luma,
            &y[..self.geometry.y_plane_len()],
            self.geometry.y_stride(),
            self.geometry.width(),
            self.geometry.height(),
        );
        write_plane(
            queue,
            &self.chroma,
            &uv[..self.geometry.uv_plane_len()],
            self.geometry.uv_stride(),
            self.geometry.chroma_width(),
            self.geometry.chroma_height(),
        );
        Ok(())
    }

    /// Record the YUV-to-RGB pass into `encoder`.
    pub fn encode_conversion(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("spike nv12 convert"),
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Upload one frame and submit its conversion in one go, returning the
    /// submission index so a caller can wait on exactly this frame.
    ///
    /// # Errors
    ///
    /// As [`Nv12Converter::upload`].
    pub fn submit_frame(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        y: &[u8],
        uv: &[u8],
    ) -> SubResult<wgpu::SubmissionIndex> {
        self.upload(queue, y, uv)?;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("spike nv12 frame"),
        });
        self.encode_conversion(&mut encoder);
        Ok(queue.submit([encoder.finish()]))
    }
}

/// Compile the conversion shader into a pipeline for `layout`.
fn build_pipeline(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("spike nv12 convert"),
        source: wgpu::ShaderSource::Wgsl(CONVERT_WGSL.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("spike nv12 convert"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("spike nv12 convert"),
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
            targets: &[Some(OUTPUT_FORMAT.into())],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// A sampled-texture binding entry, which both planes need.
fn sampled_texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// Create one plane texture.
fn plane_texture(
    device: &wgpu::Device,
    label: &str,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// Copy one strided plane into its texture.
fn write_plane(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    data: &[u8],
    stride: u32,
    width: u32,
    height: u32,
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(stride),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

/// A deterministic NV12 test picture: a vertical luma ramp with a colour
/// gradient in chroma, so a wrong stride or a swapped plane is visible.
///
/// Only used by the headless benchmark, which needs 4K frames without a
/// decoder.
pub fn synthetic_frame(geometry: Nv12Geometry, phase: u8) -> (Vec<u8>, Vec<u8>) {
    let mut y = vec![0u8; geometry.y_plane_len()];
    for row in 0..geometry.height() as usize {
        let base = row * geometry.y_stride() as usize;
        let level = 16u8
            .wrapping_add(u8::try_from(row % 220).unwrap_or(0))
            .wrapping_add(phase);
        for column in 0..geometry.width() as usize {
            y[base + column] = level.wrapping_add(u8::try_from(column % 16).unwrap_or(0));
        }
    }
    let mut uv = vec![128u8; geometry.uv_plane_len()];
    for row in 0..geometry.chroma_height() as usize {
        let base = row * geometry.uv_stride() as usize;
        for column in 0..geometry.chroma_width() as usize {
            uv[base + column * 2] = 16u8.wrapping_add(u8::try_from(column % 224).unwrap_or(0));
            uv[base + column * 2 + 1] =
                16u8.wrapping_add(u8::try_from((row + column) % 224).unwrap_or(0));
        }
    }
    (y, uv)
}

#[cfg(test)]
mod tests {
    use super::{Nv12Geometry, synthetic_frame};

    #[test]
    fn packed_geometry_matches_the_nv12_layout() {
        let geometry = Nv12Geometry::packed(1920, 1080).expect("1080p is a valid picture");
        assert_eq!(geometry.y_stride(), 1920);
        assert_eq!(geometry.uv_stride(), 1920);
        assert_eq!(geometry.chroma_height(), 540);
        // NV12 is 12 bits per pixel: 1.5 bytes.
        assert_eq!(geometry.frame_len(), 1920 * 1080 * 3 / 2);
        assert_eq!(geometry.payload_len(), geometry.frame_len());
    }

    #[test]
    fn odd_sizes_round_the_chroma_plane_up() {
        let geometry = Nv12Geometry::packed(3, 3).expect("odd sizes are legal NV12");
        assert_eq!(geometry.chroma_width(), 2);
        assert_eq!(geometry.chroma_height(), 2);
        assert_eq!(geometry.uv_stride(), 4);
        assert_eq!(geometry.uv_plane_len(), 8);
    }

    #[test]
    fn decoder_padding_is_carried_in_the_stride_not_the_width() {
        // What a VA-API decoder hands back for 4K: rows padded to 4096.
        let geometry = Nv12Geometry::new(3840, 2160, 4096, 4096).expect("padded 4K is valid");
        assert_eq!(geometry.width(), 3840);
        assert_eq!(geometry.y_plane_len(), 4096 * 2160);
        assert!(
            geometry.frame_len() > geometry.payload_len(),
            "padding should cost memory but not bus traffic"
        );
        assert_eq!(geometry.payload_len(), 3840 * 2160 * 3 / 2);
    }

    #[test]
    fn a_zero_sized_picture_is_rejected() {
        let error = Nv12Geometry::packed(0, 1080).expect_err("zero width is not a picture");
        assert_eq!(error.code.as_str(), "spike.bad_geometry");
    }

    #[test]
    fn a_stride_shorter_than_the_row_is_rejected() {
        let luma = Nv12Geometry::new(1920, 1080, 1919, 1920).expect_err("luma stride is too short");
        assert_eq!(luma.code.as_str(), "spike.bad_geometry");
        let chroma =
            Nv12Geometry::new(1920, 1080, 1920, 1919).expect_err("chroma stride is too short");
        assert_eq!(chroma.code.as_str(), "spike.bad_geometry");
    }

    #[test]
    fn short_planes_are_caught_before_any_upload() {
        let geometry = Nv12Geometry::packed(64, 64).expect("64x64 is valid");
        let (y, uv) = synthetic_frame(geometry, 0);
        geometry
            .check_planes(&y, &uv)
            .expect("a synthesised frame fits its own geometry");

        let error = geometry
            .check_planes(&y[..y.len() - 1], &uv)
            .expect_err("a truncated luma plane must be refused");
        assert_eq!(error.code.as_str(), "spike.short_plane");
        assert!(error.message.contains("luma"), "{}", error.message);

        let error = geometry
            .check_planes(&y, &uv[..uv.len() - 1])
            .expect_err("a truncated chroma plane must be refused");
        assert!(error.message.contains("chroma"), "{}", error.message);
    }

    #[test]
    fn synthetic_frames_fill_every_strided_row() {
        let geometry = Nv12Geometry::new(16, 8, 32, 32).expect("padded 16x8 is valid");
        let (y, uv) = synthetic_frame(geometry, 3);
        assert_eq!(y.len(), 32 * 8);
        assert_eq!(uv.len(), 32 * 4);
        // The last visible pixel of the last row must have been written.
        assert_ne!(y[7 * 32 + 15], 0, "the final luma row should be filled");
        assert_ne!(
            uv[3 * 32 + 15],
            128,
            "the final chroma row should be filled"
        );
    }

    #[test]
    fn different_phases_give_different_pictures() {
        let geometry = Nv12Geometry::packed(64, 64).expect("64x64 is valid");
        let (first, _) = synthetic_frame(geometry, 0);
        let (second, _) = synthetic_frame(geometry, 7);
        assert_ne!(
            first, second,
            "the benchmark must not upload one cached frame"
        );
    }
}
