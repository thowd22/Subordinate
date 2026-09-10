//! Rendering one frame through an effect declaration, headlessly.
//!
//! An `effect` plugin is checked by rendering a frame (docs/PLAN.md §6.4): a
//! shader that does not compile, or an entry point that is not there, must
//! fail the plugin's test run rather than the editor's next composite. That
//! needs a device, a picture and a composite — everything the compositor
//! already does — so the whole of it lives here, behind one call the plugin
//! host makes with nothing but an [`EffectDesc`].
//!
//! The picture is a flat mid grey the size of the canvas, so one probe pixel
//! describes the whole frame, and the clip is drawn full-canvas at full
//! opacity with the identity transform: whatever separates the output from the
//! input is the effect. The declaration is bound at its declared defaults,
//! which is the state the inspector opens a freshly added effect in.
//!
//! ```no_run
//! use sub_render::{EffectDesc, RenderContext, probe_effect};
//!
//! let context = RenderContext::headless().unwrap();
//! let desc = EffectDesc::new(Vec::new(), "@fragment\nfn fs(in: VsOut) -> @location(0) vec4<f32> { return vec4<f32>(1.0); }", "fs").unwrap();
//! let probe = probe_effect(&context, &desc);
//! assert!(probe.applied);
//! ```

use std::sync::Arc;

use sub_model::{
    Clip, ColorTags, MediaId, Resolution, Sequence, SequenceSettings, Track, TrackKind, Transform,
};
use sub_time::{Rational, RationalTime, TimeRange};

use crate::context::RenderContext;
use crate::effect::{EffectDesc, EffectInstance};
use crate::graph::{Compositor, ResolvedClip, SourceFrame};

/// The canvas and source size one probe renders at.
///
/// The picture is flat, so the frame says everything it has to say at a size
/// that costs nothing: a probe is a check that the shader runs, not a
/// benchmark.
pub const PROBE_SIZE: u32 = 16;

/// The sRGB code of the probe picture: mid grey, off both ends of the
/// transfer curve so a shader that scales it moves it visibly.
pub const PROBE_INPUT: [u8; 4] = [128, 128, 128, 255];

/// What one probe frame came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectProbe {
    /// Whether the effect compiled and ran on the clip.
    pub applied: bool,
    /// The stable code of the failure that disabled it, if it did not run.
    pub code: Option<&'static str>,
    /// The compiler's own message, if it did not run.
    pub message: Option<String>,
    /// The centre pixel of the rendered canvas, RGBA in sRGB codes.
    pub output: [u8; 4],
    /// The centre pixel the same composite produces with no effect at all,
    /// which is what `output` differing from it proves the effect did.
    pub reference: [u8; 4],
}

impl EffectProbe {
    /// Whether the effect changed the picture it was given.
    ///
    /// An effect may legitimately be a no-op at its defaults, so this is
    /// reported rather than required.
    #[must_use]
    pub fn changed_the_picture(&self) -> bool {
        self.output != self.reference
    }
}

/// Renders one frame with `desc` bound at its defaults and reports what
/// happened.
///
/// Compilation failures are not errors here: an effect whose WGSL the driver
/// refuses is *reported*, exactly as the compositor reports it while the clip
/// still draws, so the caller can turn it into a failed check with the
/// compiler's own message attached.
#[must_use]
pub fn probe_effect(context: &RenderContext, desc: &EffectDesc) -> EffectProbe {
    let sequence = probe_sequence();
    let picture = probe_picture(context);
    let mut compositor = Compositor::for_sequence(context.clone(), &sequence);

    let reference = {
        let mut source =
            |_: &ResolvedClip<'_>| Some(SourceFrame::new(picture.clone(), PROBE_SIZE, PROBE_SIZE));
        let mut effects = |_: &ResolvedClip<'_>| Vec::new();
        compositor.render_with_effects(
            &sequence,
            RationalTime::new(0, Rational::FPS_24),
            &mut source,
            &mut effects,
        );
        centre(&compositor.read_rgba())
    };

    let instance = EffectInstance::new(Arc::new(desc.clone()));
    let mut source =
        |_: &ResolvedClip<'_>| Some(SourceFrame::new(picture.clone(), PROBE_SIZE, PROBE_SIZE));
    let mut effects = |_: &ResolvedClip<'_>| vec![instance.clone()];
    let summary = compositor.render_with_effects(
        &sequence,
        RationalTime::new(0, Rational::FPS_24),
        &mut source,
        &mut effects,
    );
    let output = centre(&compositor.read_rgba());

    let failure = summary.effect_failures.first();
    EffectProbe {
        applied: failure.is_none() && summary.effects_applied() == 1,
        code: failure.map(|failed| failed.failure.code),
        message: failure.map(|failed| failed.failure.message.clone()),
        output,
        reference,
    }
}

/// The one-clip sequence a probe composites.
fn probe_sequence() -> Sequence {
    let settings = SequenceSettings::new(
        Resolution::new(PROBE_SIZE, PROBE_SIZE).expect("the probe canvas is non-zero"),
        Rational::FPS_24,
        48_000,
        ColorTags::REC709,
    )
    .expect("48 kHz is a valid sample rate");
    let source_range = TimeRange::new(
        RationalTime::new(0, Rational::FPS_24),
        RationalTime::new(10, Rational::FPS_24),
    )
    .expect("ten frames is a valid range");
    let mut clip = Clip::new("probe", MediaId::new(), source_range);
    clip.transform = Transform::IDENTITY;
    let mut track = Track::new("V1", TrackKind::Video);
    track.items.push(clip.into());
    let mut sequence = Sequence::new("Probe", settings);
    sequence.tracks.push(track);
    sequence
}

/// The flat picture the probe clip plays, as an sRGB texture.
fn probe_picture(context: &RenderContext) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width: PROBE_SIZE,
        height: PROBE_SIZE,
        depth_or_array_layers: 1,
    };
    let texture = context.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("effect probe picture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let pixels: Vec<u8> = std::iter::repeat_n(PROBE_INPUT, (PROBE_SIZE * PROBE_SIZE) as usize)
        .flatten()
        .collect();
    context.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(PROBE_SIZE * 4),
            rows_per_image: Some(PROBE_SIZE),
        },
        size,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// The centre pixel of a tightly packed RGBA readback.
fn centre(pixels: &[u8]) -> [u8; 4] {
    let offset = ((PROBE_SIZE / 2 * PROBE_SIZE + PROBE_SIZE / 2) * 4) as usize;
    pixels
        .get(offset..offset + 4)
        .and_then(|pixel| pixel.try_into().ok())
        .unwrap_or([0, 0, 0, 0])
}

#[cfg(test)]
mod tests {
    use super::{PROBE_INPUT, probe_effect};
    use crate::context::RenderContext;
    use crate::effect::{EffectDesc, EffectParam, ParamKind};
    use crate::error::RenderError;

    /// A context for one test, or `None` where this machine has no usable
    /// adapter — an environment problem, not a failure.
    fn context_or_skip() -> Option<RenderContext> {
        match RenderContext::headless() {
            Ok(context) => Some(context),
            Err(RenderError::NoAdapter { backends }) => {
                eprintln!("skipping: no wgpu adapter for backends [{backends}]");
                None
            }
            Err(error) => panic!("[{}] {error}", error.code()),
        }
    }

    /// A declaration that scales the picture down, as an effect plugin's is.
    fn scaling() -> EffectDesc {
        EffectDesc::new(
            vec![EffectParam::new(
                "amount",
                "Amount",
                ParamKind::Float {
                    min: 0.0,
                    max: 1.0,
                    default: 0.5,
                    step: None,
                },
            )],
            "@fragment\nfn fs(in: VsOut) -> @location(0) vec4<f32> {\n    let texel = \
             textureSample(source, source_sampler, in.uv);\n    return vec4<f32>(texel.rgb * \
             params.amount, texel.a);\n}\n",
            "fs",
        )
        .expect("a valid declaration")
    }

    #[test]
    fn a_shader_that_compiles_runs_over_the_probe_picture() {
        let Some(context) = context_or_skip() else {
            return;
        };
        let probe = probe_effect(&context, &scaling());
        assert!(probe.applied, "{probe:?}");
        assert_eq!(probe.code, None);
        // With no effect the composite is the picture itself.
        assert_eq!(probe.reference, PROBE_INPUT);
        assert!(probe.changed_the_picture(), "{probe:?}");
        assert!(probe.output[0] < PROBE_INPUT[0], "{probe:?}");
        // This shader leaves alpha alone, and the probe picture is opaque.
        assert_eq!(probe.output[3], 255);
    }

    #[test]
    fn a_shader_that_does_not_compile_is_reported_rather_than_panicking() {
        let Some(context) = context_or_skip() else {
            return;
        };
        let broken = EffectDesc::new(
            Vec::new(),
            "@fragment\nfn fs(in: VsOut) -> @location(0) vec4<f32> { return nonesuch(in.uv); }\n",
            "fs",
        )
        .expect("a well-formed declaration with broken WGSL");
        let probe = probe_effect(&context, &broken);
        assert!(!probe.applied);
        assert_eq!(probe.code, Some("render.effect_compile_failed"));
        assert!(probe.message.is_some(), "{probe:?}");
        // The clip still drew: the frame is the picture the effect never
        // touched.
        assert_eq!(probe.output, probe.reference);
    }
}
