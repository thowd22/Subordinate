//! The compositor frame graph: stacked video tracks, opacity and a
//! position/scale/rotation transform per clip.
//!
//! docs/PLAN.md §5.3 describes the graph as, per frame and per video track
//! top-down: sample the clip frame, colour convert, apply transform and
//! opacity, blend. This module is that walk. Crossfades extend it rather than
//! replace it: inside a transition a track contributes both of the clips the
//! blend joins, the incoming one weighted by how far through the blend the
//! playhead is. Shader effects (TASK-87) extend it the same way.
//!
//! Three pieces are deliberately separated so most of the behaviour is
//! testable without a GPU:
//!
//! - [`resolve_layers_at`] is pure model arithmetic. It answers "which clips
//!   are under the playhead, on which tracks, and what source time does each
//!   want" entirely in `RationalTime`; no float and no wgpu call is
//!   involved. [`resolve_clip_at`] is its topmost layer.
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

use std::num::NonZeroU64;

use sub_model::{
    Clip, ClipId, Fixed6, Opacity, Resolution, Sequence, Track, TrackId, TrackItem, TrackKind,
    Transform, Transition,
};
use sub_time::{Rational, RationalTime, TimeRange};

use crate::context::RenderContext;
use crate::effect::{EffectCache, EffectFailure, EffectInstance};
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
    /// Inside a crossfade this reaches outside the clip's own source range,
    /// into the handle the transition blends across: past the out point for
    /// the outgoing clip, before the in point for the incoming one.
    pub source_time: RationalTime,
    /// What a crossfade does to this layer's opacity.
    ///
    /// [`Opacity::OPAQUE`] everywhere but inside a transition, where the
    /// incoming clip ramps linearly from nothing to whole across the blend
    /// while the outgoing clip stays as it was underneath it. The compositor
    /// multiplies it into the clip's own opacity, so a half-transparent clip
    /// fading in reaches half, never more.
    pub blend: Opacity,
}

impl ResolvedClip<'_> {
    /// The clip's identity, for logs and for [`FrameSummary`].
    pub fn clip_id(&self) -> ClipId {
        self.clip.id
    }

    /// The opacity the shader receives: the clip's own, scaled by the
    /// crossfade weight.
    pub fn effective_opacity(&self) -> f32 {
        self.clip.opacity.as_f32() * self.blend.as_f32()
    }
}

/// Every clip under the playhead, in composite order: the bottom track first
/// and the top track last.
///
/// This is the §5.3 walk. A track contributes a layer only if it is a video
/// track, is not muted — a muted video track contributes nothing to the
/// composite — and has a clip rather than a gap under the playhead. A gap
/// therefore yields no layer at all, which is what makes it transparent:
/// whatever the tracks below it drew stays visible.
///
/// A track contributes *two* layers where the playhead is inside a crossfade
/// (TASK-38): the outgoing clip, still playing into the handle past its out
/// point, and then the incoming clip over it, ramping up from nothing through
/// [`ResolvedClip::blend`]. Drawn in that order over premultiplied alpha,
/// that is exactly the linear dissolve `(1 - w) * outgoing + w * incoming`.
///
/// The iterator yields in draw order rather than top-down so a caller can
/// blend as it goes; [`resolve_clip_at`] takes the topmost element for the
/// callers that only want the front layer.
///
/// `time` is in sequence time. It is rescaled to the sequence timebase for
/// the comparison, so a caller may hand in a playhead counted in nanoseconds.
pub fn resolve_layers_at(
    sequence: &Sequence,
    time: RationalTime,
) -> impl Iterator<Item = ResolvedClip<'_>> {
    let rate = sequence.settings.frame_rate;
    // A playhead that will not rescale to the sequence timebase resolves to
    // nothing at all rather than to a rounded frame.
    let playhead = time.checked_rescaled_to(rate);
    sequence
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video && !track.muted)
        .flat_map(move |track| {
            playhead
                .map(|playhead| track_layers(track, rate, playhead))
                .unwrap_or_default()
        })
}

/// The layers one track contributes under the playhead: a crossfade's pair,
/// one clip, or nothing at all.
fn track_layers(track: &Track, rate: Rational, playhead: RationalTime) -> Vec<ResolvedClip<'_>> {
    if let Some(pair) = crossfade_layers(track, rate, playhead) {
        return pair;
    }
    track
        .clip_placements(rate)
        .find(|(_, range)| range.contains(playhead))
        .and_then(|(clip, range)| resolve(track.id, clip, range, playhead, Opacity::OPAQUE))
        .into_iter()
        .collect()
}

/// The two layers of the crossfade the playhead is inside, if it is inside
/// one.
///
/// A transition sits between the two items it blends and occupies no track
/// time, so its placement is the empty range at the cut; the blend runs from
/// `cut - in_offset` to `cut + out_offset`. A transition with a clip missing
/// on either side blends nothing and is ignored, exactly as an editor would
/// expect of a cut a later edit took a clip away from.
fn crossfade_layers(
    track: &Track,
    rate: Rational,
    playhead: RationalTime,
) -> Option<Vec<ResolvedClip<'_>>> {
    let mut outgoing: Option<(&Clip, TimeRange)> = None;
    let mut pending: Option<(Transition, RationalTime, (&Clip, TimeRange))> = None;
    for (item, range) in track.placements(rate) {
        match item {
            TrackItem::Transition(transition) => {
                pending = outgoing.map(|before| (*transition, range.start(), before));
            }
            TrackItem::Gap(_) => {
                outgoing = None;
                pending = None;
            }
            TrackItem::Clip(clip) => {
                if let Some((transition, cut, before)) = pending.take()
                    && let Some(pair) = blend_at(
                        track.id,
                        &transition,
                        cut,
                        before,
                        (clip, range),
                        rate,
                        playhead,
                    )
                {
                    return Some(pair);
                }
                outgoing = Some((clip, range));
            }
        }
    }
    None
}

/// The pair of layers `transition` produces at `playhead`, when the playhead
/// is inside its blend.
fn blend_at<'a>(
    track: TrackId,
    transition: &Transition,
    cut: RationalTime,
    outgoing: (&'a Clip, TimeRange),
    incoming: (&'a Clip, TimeRange),
    rate: Rational,
    playhead: RationalTime,
) -> Option<Vec<ResolvedClip<'a>>> {
    let start = cut
        .checked_sub(transition.in_offset())?
        .checked_rescaled_to(rate)?;
    let end = cut
        .checked_add(transition.out_offset())?
        .checked_rescaled_to(rate)?;
    if playhead < start || playhead >= end {
        return None;
    }
    let weight = crossfade_weight(start, end, playhead)?;
    let under = resolve(track, outgoing.0, outgoing.1, playhead, Opacity::OPAQUE)?;
    let over = resolve(track, incoming.0, incoming.1, playhead, weight)?;
    Some(vec![under, over])
}

/// How far through the blend `playhead` is, as the incoming clip's weight.
///
/// The ratio is taken in exact integer arithmetic at the sequence timebase
/// and only then rounded to the six decimal places [`Opacity`] carries; no
/// float takes part in deciding which frame is how far through a dissolve.
fn crossfade_weight(
    start: RationalTime,
    end: RationalTime,
    playhead: RationalTime,
) -> Option<Opacity> {
    let span = i128::from(end.value().checked_sub(start.value())?);
    if span <= 0 {
        return None;
    }
    let elapsed = i128::from(playhead.value().checked_sub(start.value())?).clamp(0, span);
    let micros = elapsed * i128::from(Fixed6::ONE.micros()) / span;
    Opacity::new(Fixed6::from_micros(i64::try_from(micros).ok()?)).ok()
}

/// One layer: the clip, where it sits, the source time the playhead maps to
/// through it, and the crossfade weight it carries.
///
/// The source time is offset from the clip's *placement*, so a clip reached
/// from outside its own span — which is what a crossfade does — reads into
/// its handle rather than being clamped to its edge.
fn resolve(
    track: TrackId,
    clip: &Clip,
    timeline_range: TimeRange,
    playhead: RationalTime,
    blend: Opacity,
) -> Option<ResolvedClip<'_>> {
    let offset = playhead.checked_sub(timeline_range.start())?;
    let source_time = clip.source_range.start().checked_add(offset)?;
    Some(ResolvedClip {
        track,
        clip,
        timeline_range,
        source_time,
        blend,
    })
}

/// The frontmost clip under the playhead, or `None` when every video track
/// shows a gap there, the playhead is past the end of them all, or the
/// sequence has no video at all.
///
/// The topmost layer of [`resolve_layers_at`]: what the composite shows where
/// that layer is opaque and covers the canvas.
pub fn resolve_clip_at(sequence: &Sequence, time: RationalTime) -> Option<ResolvedClip<'_>> {
    resolve_layers_at(sequence, time).last()
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

/// Where the compositor gets a clip's effect chain.
///
/// Effects hang off a clip, in the order the inspector lists them, and each
/// one is a plugin declaration plus the values this clip binds. The graph
/// asks for them per resolved clip rather than reading them off the model, so
/// the compositor needs no opinion on where a project stores an effect and
/// TASK-88 can hand it whatever the inspector has just edited.
///
/// Any `FnMut(&ResolvedClip) -> Vec<EffectInstance>` is a source, which is
/// what tests and a project with no effects at all use.
pub trait EffectSource {
    /// The effects to run on `clip`, first applied first.
    fn effects(&mut self, clip: &ResolvedClip<'_>) -> Vec<EffectInstance>;
}

impl<F> EffectSource for F
where
    F: FnMut(&ResolvedClip<'_>) -> Vec<EffectInstance>,
{
    fn effects(&mut self, clip: &ResolvedClip<'_>) -> Vec<EffectInstance> {
        self(clip)
    }
}

/// An [`EffectSource`] that gives every clip an empty chain.
///
/// What [`Compositor::render`] passes: the plain render is
/// [`Compositor::render_with_effects`] with nothing to run.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoEffects;

impl EffectSource for NoEffects {
    fn effects(&mut self, _clip: &ResolvedClip<'_>) -> Vec<EffectInstance> {
        Vec::new()
    }
}

/// An effect that was declared on a clip but did not run.
///
/// The clip is still drawn, without that effect: a plugin whose WGSL does not
/// compile costs the user one error in the inspector, not a black frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerEffectFailure {
    /// The track the clip sits on.
    pub track: TrackId,
    /// The clip the effect was declared on.
    pub clip: ClipId,
    /// Which effect of that clip's chain, and why it was disabled.
    pub failure: EffectFailure,
}

/// One layer of a composite: the clip that resolved on one video track, and
/// what became of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerSummary {
    /// The track the layer came from.
    pub track: TrackId,
    /// The clip the playhead resolved to on that track.
    pub clip: ClipId,
    /// The source time asked for.
    pub source_time: RationalTime,
    /// The clip's opacity, as the shader received it: its own, scaled by the
    /// crossfade weight where the playhead is inside a transition.
    pub opacity: f32,
    /// Where the picture landed, or `None` when the source had no picture
    /// ready and nothing was drawn for this layer.
    pub placement: Option<QuadTransform>,
    /// How many of the clip's effects actually ran on this layer. Effects
    /// that failed to compile are excluded, and listed in
    /// [`FrameSummary::effect_failures`].
    pub effects_applied: usize,
}

impl LayerSummary {
    /// True when the layer resolved but no picture was drawn for it.
    pub fn is_missing(&self) -> bool {
        self.placement.is_none()
    }
}

/// What one [`Compositor::render`] call did.
///
/// Enough for the viewer to show "no picture here" and for a test to assert
/// on the graph's decisions without reading pixels back. Layers are listed in
/// composite order: the bottom track first, the top track last.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameSummary {
    /// Every video track that had a clip under the playhead, bottom-up.
    pub layers: Vec<LayerSummary>,
    /// Every effect that was declared on a drawn clip but did not run, in
    /// the order the layers were prepared.
    pub effect_failures: Vec<LayerEffectFailure>,
}

impl FrameSummary {
    /// The frontmost resolved layer, whether or not it was drawn.
    pub fn top(&self) -> Option<&LayerSummary> {
        self.layers.last()
    }

    /// True when some clip's effect could not run this frame.
    pub fn has_effect_failures(&self) -> bool {
        !self.effect_failures.is_empty()
    }

    /// How many effects ran across every layer.
    pub fn effects_applied(&self) -> usize {
        self.layers.iter().map(|layer| layer.effects_applied).sum()
    }

    /// How many layers were actually drawn.
    pub fn drawn(&self) -> usize {
        self.layers
            .iter()
            .filter(|layer| layer.placement.is_some())
            .count()
    }

    /// True when the composite is the bare black canvas: no clip resolved
    /// anywhere, or no source had a picture ready.
    pub fn is_blank(&self) -> bool {
        self.drawn() == 0
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
    let alpha = texel.a * params.opacity.x;
    // Premultiplied alpha: the layer carries its own coverage, so the blend
    // state adds it straight onto what the tracks below already drew.
    return vec4<f32>(texel.rgb * alpha, alpha);
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
    /// Bytes between one layer's uniform block and the next, which is
    /// [`UNIFORM_BYTES`] rounded up to the device's uniform binding
    /// alignment.
    uniform_stride: u64,
    sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    effects: EffectCache,
    /// One ping-pong pair per layer that runs effects, indexed by the order
    /// the layers were prepared. A layer's chain must survive until the
    /// composite pass samples it, so layers may not share one pair.
    chains: Vec<EffectChain>,
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
        // Every layer of a frame gets its own slice of one buffer: a uniform
        // binding may only start on a device-defined boundary, so the blocks
        // are spaced out to it rather than packed.
        let alignment = u64::from(device.limits().min_uniform_buffer_offset_alignment).max(1);
        let uniform_stride = (UNIFORM_BYTES as u64).div_ceil(alignment) * alignment;
        let uniforms = layer_uniform_buffer(device, uniform_stride, 1);
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
        let effects = EffectCache::new(context.clone());
        Self {
            context,
            resolution,
            output,
            output_view,
            uniforms,
            uniform_stride,
            sampler,
            bind_group_layout,
            pipeline,
            effects,
            chains: Vec::new(),
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
    /// Use [`Self::display_view`] to register it with egui once. [`Compositor::render`] only replaces it
    /// when the sequence resolution changes, so a consumer that caches the
    /// handle re-reads it when [`Compositor::resolution`] moves.
    pub fn output(&self) -> &wgpu::Texture {
        &self.output
    }

    /// A gamma-encoded view for UI renderers such as egui.
    ///
    /// The compositor stores sRGB codes, but its normal sRGB view decodes
    /// them to linear light when sampled. egui expects gamma-encoded samples,
    /// so it must use this non-sRGB view of the same bytes.
    pub fn display_view(&self) -> wgpu::TextureView {
        self.output.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8Unorm),
            ..wgpu::TextureViewDescriptor::default()
        })
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
    /// opaque black — that black is both the letterbox and the bottom of the
    /// stack — and then every video track that has a clip under the playhead
    /// is drawn over it, bottom track first, each with its own opacity and
    /// transform and blended with premultiplied alpha. Muted tracks and gaps
    /// contribute nothing, so the layers below them show through.
    pub fn render(
        &mut self,
        sequence: &Sequence,
        time: RationalTime,
        source: &mut dyn FrameSource,
    ) -> FrameSummary {
        self.render_with_effects(sequence, time, source, &mut NoEffects)
    }

    /// The compiled effect pipelines, kept between frames.
    pub fn effect_cache(&self) -> &EffectCache {
        &self.effects
    }

    /// [`Compositor::render`] with each clip's plugin effects run first.
    ///
    /// A clip's chain runs in declaration order on the clip's own picture,
    /// before the letterbox fit, the transform and the opacity: an effect
    /// sees the source picture at source resolution, which is what makes the
    /// same chain look the same whatever canvas it is composited onto. Each
    /// pass reads the previous pass's output through a ping-pong pair of
    /// offscreen textures, and the last output is what the composite draws.
    ///
    /// An effect whose shader will not compile is *disabled*: it is skipped,
    /// the rest of the chain still runs, and the failure is reported in
    /// [`FrameSummary::effect_failures`] with the compiler's own message for
    /// the inspector to show. A clip therefore never disappears because a
    /// plugin shipped bad WGSL.
    pub fn render_with_effects(
        &mut self,
        sequence: &Sequence,
        time: RationalTime,
        source: &mut dyn FrameSource,
        effects: &mut dyn EffectSource,
    ) -> FrameSummary {
        self.resize(sequence.settings.resolution);

        // Ask every layer for its picture first: the source may hand back the
        // same view for several clips, and the uniform buffer can only be
        // sized once the number of drawn layers is known.
        let mut summaries = Vec::with_capacity(sequence.tracks.len());
        let mut drawn = Vec::with_capacity(sequence.tracks.len());
        let mut effect_failures = Vec::new();
        let mut chain = 0;
        for resolved in resolve_layers_at(sequence, time) {
            let opacity = resolved.effective_opacity();
            let mut layer = source
                .frame(&resolved)
                .filter(|frame| frame.width > 0 && frame.height > 0);
            let mut applied = 0;
            if let Some(frame) = layer.clone() {
                let instances = effects.effects(&resolved);
                if !instances.is_empty() {
                    let slot = chain;
                    chain += 1;
                    let outcome = self.apply_effects(slot, &frame, &instances);
                    applied = outcome.applied;
                    effect_failures.extend(outcome.failures.into_iter().map(|failure| {
                        LayerEffectFailure {
                            track: resolved.track,
                            clip: resolved.clip_id(),
                            failure,
                        }
                    }));
                    layer = Some(outcome.frame);
                }
            }
            let layer = layer.map(|frame| {
                let placement = QuadTransform::new(
                    self.resolution,
                    frame.width,
                    frame.height,
                    &resolved.clip.transform,
                );
                (frame, placement)
            });
            summaries.push(LayerSummary {
                track: resolved.track,
                clip: resolved.clip_id(),
                source_time: resolved.source_time,
                opacity,
                placement: layer.as_ref().map(|(_, placement)| *placement),
                effects_applied: applied,
            });
            if let Some((frame, placement)) = layer {
                drawn.push((frame, placement, opacity));
            }
        }

        // The uniform writes and the bind groups are prepared before the
        // pass: a pass borrows every resource it binds for its whole
        // lifetime, and growing the buffer would invalidate a bind group.
        self.reserve_uniforms(drawn.len());
        let mut bound = Vec::with_capacity(drawn.len());
        let mut offset = 0;
        for (frame, placement, opacity) in &drawn {
            self.context.queue().write_buffer(
                &self.uniforms,
                offset,
                &placement.uniform_bytes(*opacity),
            );
            bound.push(self.layer_bind_group(&frame.view, offset));
            offset += self.uniform_stride;
        }

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
            if !bound.is_empty() {
                pass.set_pipeline(&self.pipeline);
            }
            // Bottom track first: each draw blends over what is already there.
            for bind_group in &bound {
                pass.set_bind_group(0, bind_group, &[]);
                pass.draw(0..4, 0..1);
            }
        }
        self.context.queue().submit([encoder.finish()]);

        FrameSummary {
            layers: summaries,
            effect_failures,
        }
    }

    /// Run `instances` over `frame`, in order, into the ping-pong pair at
    /// `slot`, and hand back what the composite should draw.
    ///
    /// Every effect is compiled first, so a chain whose effects all fail
    /// costs no render pass at all and the input picture passes through
    /// untouched.
    fn apply_effects(
        &mut self,
        slot: usize,
        frame: &SourceFrame,
        instances: &[EffectInstance],
    ) -> EffectOutcome {
        let mut failures = Vec::new();
        let mut passes = Vec::with_capacity(instances.len());
        for (index, instance) in instances.iter().enumerate() {
            match self.effects.compile(instance.desc()) {
                Ok(pipeline) => passes.push((pipeline, instance.uniform_bytes())),
                Err(error) => failures.push(EffectFailure {
                    index,
                    effect: instance.desc().key(),
                    code: error.code(),
                    message: error.to_string(),
                }),
            }
        }
        if passes.is_empty() {
            return EffectOutcome {
                frame: frame.clone(),
                applied: 0,
                failures,
            };
        }
        self.reserve_chain(slot, frame.width, frame.height);

        let device = self.context.device();
        // One uniform buffer for the whole chain: a uniform binding may only
        // start on a device boundary, so the blocks are spaced out to it.
        let alignment = u64::from(device.limits().min_uniform_buffer_offset_alignment).max(1);
        let widest = passes
            .iter()
            .map(|(pipeline, _)| pipeline.uniform_size())
            .max()
            .unwrap_or(1);
        let stride = widest.div_ceil(alignment) * alignment;
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("effect params"),
            size: stride * passes.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let chain = &self.chains[slot];
        let mut bound = Vec::with_capacity(passes.len());
        for (index, (pipeline, bytes)) in passes.iter().enumerate() {
            let offset = stride * index as u64;
            self.context.queue().write_buffer(&uniforms, offset, bytes);
            let input = if index == 0 {
                &frame.view
            } else {
                &chain.views[(index - 1) % 2]
            };
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("effect pass"),
                layout: self.effects.bind_group_layout(),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &uniforms,
                            offset,
                            size: NonZeroU64::new(pipeline.uniform_size()),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(self.effects.sampler()),
                    },
                ],
            });
            bound.push(bind_group);
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("effect chain"),
        });
        for (index, (pipeline, _)) in passes.iter().enumerate() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("effect pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &chain.views[index % 2],
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // The quad covers the target, so nothing of the
                        // previous frame survives the clear either way.
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline.pipeline());
            pass.set_bind_group(0, &bound[index], &[]);
            pass.draw(0..4, 0..1);
        }
        self.context.queue().submit([encoder.finish()]);

        let last = (passes.len() - 1) % 2;
        EffectOutcome {
            frame: SourceFrame::new(chain.views[last].clone(), frame.width, frame.height),
            applied: passes.len(),
            failures,
        }
    }

    /// Make sure the chain at `slot` holds a `width` x `height` ping-pong
    /// pair, building or rebuilding it only when the picture size changes.
    fn reserve_chain(&mut self, slot: usize, width: u32, height: u32) {
        while self.chains.len() <= slot {
            self.chains
                .push(EffectChain::new(self.context.device(), width, height));
        }
        let chain = &self.chains[slot];
        if chain.width != width || chain.height != height {
            self.chains[slot] = EffectChain::new(self.context.device(), width, height);
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

    /// Make sure the uniform buffer holds `layers` blocks.
    ///
    /// It only ever grows, so a steady stack of tracks stops reallocating
    /// after the first frame that needs the room.
    fn reserve_uniforms(&mut self, layers: usize) {
        // A layer count that does not fit in a u64 cannot exist: it would
        // need more tracks than the machine has addresses.
        let layers = u64::try_from(layers).unwrap_or(u64::MAX);
        if self.uniform_stride.saturating_mul(layers) <= self.uniforms.size() {
            return;
        }
        self.uniforms = layer_uniform_buffer(self.context.device(), self.uniform_stride, layers);
    }

    /// The bind group for one layer: the uniform block at `offset` and the
    /// layer's own picture. Source views change frame to frame, so this is
    /// built per draw; it is a handful of refcount bumps, not a GPU
    /// allocation.
    fn layer_bind_group(&self, view: &wgpu::TextureView, offset: u64) -> wgpu::BindGroup {
        self.context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("compositor layer"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.uniforms,
                            offset,
                            size: NonZeroU64::new(UNIFORM_BYTES as u64),
                        }),
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

/// What [`Compositor::apply_effects`] made of one clip's chain.
#[derive(Debug)]
struct EffectOutcome {
    /// The picture to composite: the chain's last output, or the input
    /// itself when nothing ran.
    frame: SourceFrame,
    /// How many effects ran.
    applied: usize,
    /// Effects that were skipped, with the reason.
    failures: Vec<EffectFailure>,
}

/// The two offscreen textures one clip's effect chain ping-pongs between.
///
/// Kept between frames: a steady chain allocates once and then only when the
/// source picture changes size.
#[derive(Debug)]
struct EffectChain {
    width: u32,
    height: u32,
    /// The views bound and rendered into. The textures behind them are held
    /// by `_textures`; a view does not keep its texture alive.
    views: [wgpu::TextureView; 2],
    _textures: [wgpu::Texture; 2],
}

impl EffectChain {
    /// Build a pair of `width` x `height` targets in the composite format.
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = |label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: width.max(1),
                    height: height.max(1),
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
        };
        let textures = [texture("effect chain a"), texture("effect chain b")];
        let views = [
            textures[0].create_view(&wgpu::TextureViewDescriptor::default()),
            textures[1].create_view(&wgpu::TextureViewDescriptor::default()),
        ];
        Self {
            width,
            height,
            views,
            _textures: textures,
        }
    }
}

/// A dimension as a float, for the geometry. Canvas sizes in `u32` are far
/// inside `f32`'s exactly representable integers.
#[allow(clippy::cast_precision_loss)]
fn pixels(value: u32) -> f32 {
    value as f32
}

/// Create a uniform buffer holding `layers` blocks spaced `stride` apart.
fn layer_uniform_buffer(device: &wgpu::Device, stride: u64, layers: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("compositor params"),
        size: stride.saturating_mul(layers.max(1)),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
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
        view_formats: &[wgpu::TextureFormat::Rgba8Unorm],
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
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
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
    use super::{LetterboxFit, QuadTransform, resolve_clip_at, resolve_layers_at};
    use sub_model::params::{Fixed6, Point2, Scale2};
    use sub_model::{
        Clip, Gap, MediaId, Opacity, Resolution, Sequence, SequenceSettings, Track, TrackKind,
        Transform, Transition,
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

    /// A sequence whose two video tracks both cover frame 2, with an audio
    /// track between them to prove it is ignored. Returns the sequence and
    /// the clip ids, bottom first.
    fn two_video_tracks() -> (Sequence, [sub_model::ClipId; 2]) {
        let media = MediaId::new();
        let mut sequence = Sequence::new("Main", SequenceSettings::default());

        let mut bottom = Track::new("V1", TrackKind::Video);
        bottom
            .items
            .push(Clip::new("bottom", media, range(0, 10)).into());
        let bottom_clip = bottom.items[0].as_clip().expect("a clip").id;

        let mut audio = Track::new("A1", TrackKind::Audio);
        audio
            .items
            .push(Clip::new("sound", media, range(0, 10)).into());

        let mut top = Track::new("V2", TrackKind::Video);
        top.items
            .push(Clip::new("top", media, range(50, 10)).into());
        let top_clip = top.items[0].as_clip().expect("a clip").id;

        sequence.tracks.push(bottom);
        sequence.tracks.push(audio);
        sequence.tracks.push(top);
        (sequence, [bottom_clip, top_clip])
    }

    #[test]
    fn every_video_track_contributes_a_layer_bottom_first() {
        let (sequence, [bottom_clip, top_clip]) = two_video_tracks();
        let layers: Vec<_> = resolve_layers_at(&sequence, frames(2)).collect();

        assert_eq!(layers.len(), 2, "the audio track is not a layer");
        assert_eq!(layers[0].clip_id(), bottom_clip);
        assert_eq!(layers[0].track, sequence.tracks[0].id);
        assert_eq!(layers[0].source_time, frames(2));
        assert_eq!(layers[1].clip_id(), top_clip);
        assert_eq!(layers[1].track, sequence.tracks[2].id);
        // The top clip's source starts at 50, so the same playhead asks it
        // for a different source time.
        assert_eq!(layers[1].source_time, frames(52));

        // The frontmost layer is what the single-clip resolver returns.
        assert_eq!(
            resolve_clip_at(&sequence, frames(2)).expect("frame 2 is covered"),
            layers[1]
        );
    }

    #[test]
    fn muted_tracks_and_gaps_drop_out_of_the_layer_walk() {
        let (mut sequence, [bottom_clip, top_clip]) = two_video_tracks();

        sequence.tracks[0].muted = true;
        let layers: Vec<_> = resolve_layers_at(&sequence, frames(2)).collect();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].clip_id(), top_clip);

        // A gap under the playhead on the top track leaves only the layer
        // below it, which is what makes the gap transparent.
        sequence.tracks[0].muted = false;
        sequence.tracks[2].items.insert(
            0,
            Gap {
                duration: frames(5),
            }
            .into(),
        );
        let layers: Vec<_> = resolve_layers_at(&sequence, frames(2)).collect();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].clip_id(), bottom_clip);

        // With both tracks muted nothing composites at all.
        sequence.tracks[0].muted = true;
        sequence.tracks[2].muted = true;
        assert_eq!(resolve_layers_at(&sequence, frames(2)).count(), 0);
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

    /// A sequence whose V1 holds two butt-joined 24-frame clips with a
    /// 12-frame crossfade at the cut: `a` from source 48, `b` from source 240,
    /// so both have handle to blend across.
    fn sequence_with_a_crossfade() -> Sequence {
        let media = MediaId::new();
        let mut track = Track::new("V1", TrackKind::Video);
        track
            .items
            .push(Clip::new("a", media, range(48, 24)).into());
        track
            .items
            .push(Transition::crossfade(frames(6), frames(6)).into());
        track
            .items
            .push(Clip::new("b", media, range(240, 24)).into());
        let mut sequence = Sequence::new("Main", SequenceSettings::default());
        sequence.tracks.push(track);
        sequence
    }

    #[test]
    fn a_crossfade_draws_both_clips_with_the_incoming_one_ramping_up() {
        let sequence = sequence_with_a_crossfade();
        let outgoing = sequence.tracks[0].items[0].as_clip().expect("a clip").id;
        let incoming = sequence.tracks[0].items[2].as_clip().expect("a clip").id;

        // The blend runs from frame 18 to frame 30, the cut being frame 24.
        let layers: Vec<_> = resolve_layers_at(&sequence, frames(18)).collect();
        assert_eq!(layers.len(), 2, "both clips are drawn inside the blend");
        assert_eq!(layers[0].clip_id(), outgoing);
        assert_eq!(layers[1].clip_id(), incoming);
        assert_eq!(layers[0].blend, Opacity::OPAQUE);
        assert_eq!(layers[1].blend, Opacity::TRANSPARENT, "nothing yet");

        let middle: Vec<_> = resolve_layers_at(&sequence, frames(24)).collect();
        assert_eq!(middle[1].blend, Opacity::from_f64(0.5).expect("half"));

        let late: Vec<_> = resolve_layers_at(&sequence, frames(27)).collect();
        assert_eq!(late[1].blend, Opacity::from_f64(0.75).expect("three parts"));
    }

    #[test]
    fn the_blend_is_linear_in_the_frames_it_covers() {
        let sequence = sequence_with_a_crossfade();
        let weight = |frame| {
            resolve_layers_at(&sequence, frames(frame))
                .last()
                .expect("a layer")
                .blend
                .factor()
                .micros()
        };
        let steps: Vec<i64> = (18..30).map(weight).collect();
        assert_eq!(steps[0], 0);
        // A twelfth is not a whole number of micro-units, so the ramp is the
        // same step every frame to within the one micro-unit `Opacity` can
        // hold.
        for pair in steps.windows(2) {
            assert!(
                (pair[1] - pair[0] - 83_333).abs() <= 1,
                "every frame of the dissolve is the same step: {steps:?}"
            );
        }
    }

    #[test]
    fn each_side_of_a_crossfade_reads_into_its_own_handle() {
        let sequence = sequence_with_a_crossfade();

        // Three frames past the cut: the outgoing clip is three frames past
        // its out point (48 + 24 + 3) and the incoming three frames into
        // itself (240 + 3).
        let layers: Vec<_> = resolve_layers_at(&sequence, frames(27)).collect();
        assert_eq!(layers[0].source_time, frames(75));
        assert_eq!(layers[1].source_time, frames(243));

        // Three frames before it: the incoming clip reads three frames before
        // its in point.
        let layers: Vec<_> = resolve_layers_at(&sequence, frames(21)).collect();
        assert_eq!(layers[0].source_time, frames(69));
        assert_eq!(layers[1].source_time, frames(237));
    }

    #[test]
    fn outside_the_blend_a_crossfade_changes_nothing() {
        let sequence = sequence_with_a_crossfade();
        let before: Vec<_> = resolve_layers_at(&sequence, frames(17)).collect();
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].blend, Opacity::OPAQUE);
        assert_eq!(before[0].source_time, frames(65));

        let after: Vec<_> = resolve_layers_at(&sequence, frames(30)).collect();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].source_time, frames(246));
        assert_eq!(
            resolve_clip_at(&sequence, frames(24))
                .expect("a clip")
                .clip_id(),
            sequence.tracks[0].items[2].as_clip().expect("a clip").id,
            "the frontmost layer inside a blend is the incoming clip"
        );
    }

    #[test]
    fn a_clip_opacity_still_bounds_a_fading_layer() {
        let mut sequence = sequence_with_a_crossfade();
        if let Some(clip) = sequence.tracks[0].items[2].as_clip().cloned() {
            let mut clip = clip;
            clip.opacity = Opacity::from_f64(0.5).expect("half");
            sequence.tracks[0].items[2] = clip.into();
        }
        let layers: Vec<_> = resolve_layers_at(&sequence, frames(30 - 1)).collect();
        let incoming = layers.last().expect("a layer");
        assert!(
            (incoming.effective_opacity() - 0.5 * (11.0 / 12.0)).abs() < EPSILON,
            "the crossfade weight scales the clip's own opacity"
        );
    }

    #[test]
    fn a_transition_with_a_gap_beside_it_blends_nothing() {
        let media = MediaId::new();
        let mut track = Track::new("V1", TrackKind::Video);
        track.items.push(
            Gap {
                duration: frames(12),
            }
            .into(),
        );
        track
            .items
            .push(Transition::crossfade(frames(6), frames(6)).into());
        track
            .items
            .push(Clip::new("b", media, range(240, 24)).into());
        let mut sequence = Sequence::new("Main", SequenceSettings::default());
        sequence.tracks.push(track);

        let layers: Vec<_> = resolve_layers_at(&sequence, frames(13)).collect();
        assert_eq!(layers.len(), 1, "only the clip that is really there");
        assert_eq!(layers[0].blend, Opacity::OPAQUE);
    }
}
