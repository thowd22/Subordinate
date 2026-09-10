//! Golden test for plugin shader effects: a reference tint effect run through
//! the compositor on a flat grey clip.
//!
//! The effect is declared exactly as an `effect` plugin declares one — a
//! parameter list plus WGSL with a fragment entry point — and the host
//! compiles it, builds the uniform block from the parameters and runs it on
//! the clip's picture before the transform and the opacity. The readback is
//! compared against the mix computed on the CPU in *linear* light, which is
//! what the shader sees when it samples an sRGB texture and what the sRGB
//! target encodes on store. A tint that never ran, a uniform whose bytes
//! landed in the wrong slot, or a chain run in the wrong order all land far
//! outside the tolerance.
//!
//! The same fixtures cover the other two halves of the contract: a chain runs
//! in declaration order, and an effect whose WGSL does not compile is skipped
//! with a reported error while the clip still draws.
//!
//! Where the machine has no wgpu adapter at all the test reports that and
//! passes rather than failing the build on an environment problem, and one
//! context is shared under a mutex, as in `composite_golden.rs`.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use sub_model::{
    Clip, ColorTags, MediaId, Resolution, Sequence, SequenceSettings, Track, TrackKind, Transform,
};
use sub_render::{
    Compositor, EffectDesc, EffectInstance, EffectParam, ParamKind, ParamValue, RenderContext,
    RenderError, ResolvedClip, SourceFrame,
};
use sub_time::{Rational, RationalTime, TimeRange};

/// How far a channel may sit from its reference code.
const TOLERANCE: i32 = 3;

/// Canvas and source size. Flat colour, so one probe is every pixel.
const SIZE: u32 = 16;

/// The sRGB code of the source picture: mid grey, off both ends of the
/// transfer curve.
const GREY: u8 = 128;

/// Held for the length of a test: only one test may talk to the driver.
static DRIVER: Mutex<()> = Mutex::new(());
/// The one context, built under `DRIVER`. `None` means no usable adapter.
static CONTEXT: OnceLock<Option<RenderContext>> = OnceLock::new();

/// Exclusive use of the shared context, or `None` when this machine has no
/// usable adapter. The guard must outlive every use of the context.
fn context_or_skip() -> Option<(MutexGuard<'static, ()>, RenderContext)> {
    let guard = DRIVER.lock().unwrap_or_else(PoisonError::into_inner);
    let context = CONTEXT.get_or_init(|| match RenderContext::headless() {
        Ok(context) => Some(context),
        Err(RenderError::NoAdapter { backends }) => {
            eprintln!("skipping: no wgpu adapter for backends [{backends}]");
            None
        }
        Err(error) => panic!("[{}] {error}", error.code()),
    });
    context.clone().map(|context| (guard, context))
}

/// The reference tint: `mix(picture, tint, amount)`, alpha untouched.
const TINT_WGSL: &str = "
@fragment
fn fs_tint(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(source, source_sampler, in.uv);
    return vec4<f32>(mix(texel.rgb, params.tint.rgb, params.amount), texel.a);
}
";

/// Adds `params.lift` to every channel: half of the order-sensitive pair.
const LIFT_WGSL: &str = "
@fragment
fn fs_lift(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(source, source_sampler, in.uv);
    return vec4<f32>(texel.rgb + vec3<f32>(params.lift), texel.a);
}
";

/// Multiplies every channel by `params.gain`: the other half.
const GAIN_WGSL: &str = "
@fragment
fn fs_gain(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(source, source_sampler, in.uv);
    return vec4<f32>(texel.rgb * params.gain, texel.a);
}
";

/// WGSL that names something that does not exist: it must fail to compile.
const BROKEN_WGSL: &str = "
@fragment
fn fs_broken(in: VsOut) -> @location(0) vec4<f32> {
    return nonesuch(in.uv);
}
";

/// The tint declaration: a mix amount and the colour mixed towards.
fn tint_effect() -> Arc<EffectDesc> {
    let params = vec![
        EffectParam::new(
            "amount",
            "Amount",
            ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.0,
                step: None,
            },
        )
        .with_doc("How far towards the tint colour the picture moves"),
        EffectParam::new(
            "tint",
            "Tint",
            ParamKind::Color {
                default: [1.0, 0.0, 0.0, 1.0],
            },
        ),
    ];
    Arc::new(EffectDesc::new(params, TINT_WGSL, "fs_tint").expect("the tint declaration is valid"))
}

/// A one-float declaration, for the order pair.
fn scalar_effect(id: &str, shader: &str, entry: &str, max: f32, default: f32) -> Arc<EffectDesc> {
    let params = vec![EffectParam::new(
        id,
        id,
        ParamKind::Float {
            min: 0.0,
            max,
            default,
            step: None,
        },
    )];
    Arc::new(EffectDesc::new(params, shader, entry).expect("a scalar declaration is valid"))
}

/// A declaration whose WGSL is broken; validation passes, compilation cannot.
fn broken_effect() -> Arc<EffectDesc> {
    Arc::new(
        EffectDesc::new(Vec::new(), BROKEN_WGSL, "fs_broken")
            .expect("a broken shader is still a well-formed declaration"),
    )
}

/// Decode one sRGB code to linear light, the way the sampler does.
fn to_linear(code: u8) -> f32 {
    let value = f32::from(code) / 255.0;
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// Encode linear light back to an sRGB code, the way the target does.
///
/// The cast is bounded by the clamp either side of it.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn to_srgb(value: f32) -> u8 {
    let clamped = value.clamp(0.0, 1.0);
    let encoded = if clamped <= 0.003_130_8 {
        clamped * 12.92
    } else {
        1.055f32.mul_add(clamped.powf(1.0 / 2.4), -0.055)
    };
    (encoded * 255.0).round().clamp(0.0, 255.0) as u8
}

/// A flat `GREY` picture the size of the canvas, as an sRGB texture.
fn grey_picture(context: &RenderContext) -> wgpu::TextureView {
    let texture = context.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("flat grey"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let pixels: Vec<u8> = std::iter::repeat_n([GREY, GREY, GREY, 255], (SIZE * SIZE) as usize)
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
            bytes_per_row: Some(SIZE * 4),
            rows_per_image: Some(SIZE),
        },
        wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// A one-track sequence holding one full-canvas clip at full opacity.
fn grey_sequence() -> Sequence {
    let settings = SequenceSettings::new(
        Resolution::new(SIZE, SIZE).expect("the canvas is non-zero"),
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
    let mut clip = Clip::new("grey", MediaId::new(), source_range);
    clip.transform = Transform::IDENTITY;
    let mut track = Track::new("V1", TrackKind::Video);
    track.items.push(clip.into());
    let mut sequence = Sequence::new("Effects", settings);
    sequence.tracks.push(track);
    sequence
}

/// Render the grey clip with `chain` applied and read the canvas back.
fn render_chain(
    compositor: &mut Compositor,
    sequence: &Sequence,
    picture: &wgpu::TextureView,
    chain: &[EffectInstance],
) -> (Vec<u8>, sub_render::FrameSummary) {
    let mut source = |_: &ResolvedClip<'_>| Some(SourceFrame::new(picture.clone(), SIZE, SIZE));
    let mut effects = |_: &ResolvedClip<'_>| chain.to_vec();
    let summary = compositor.render_with_effects(
        sequence,
        RationalTime::new(0, Rational::FPS_24),
        &mut source,
        &mut effects,
    );
    (compositor.read_rgba(), summary)
}

/// The pixel at the centre of a tightly packed RGBA readback.
fn centre(pixels: &[u8]) -> [u8; 4] {
    let offset = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
    pixels[offset..offset + 4]
        .try_into()
        .expect("four bytes per pixel")
}

/// Assert `actual` is `expected` within [`TOLERANCE`] on every channel.
fn assert_close(what: &str, actual: [u8; 4], expected: [u8; 4]) {
    for (channel, (got, want)) in ["r", "g", "b", "a"].iter().zip(actual.iter().zip(expected)) {
        let difference = i32::from(*got) - i32::from(want);
        assert!(
            difference.abs() <= TOLERANCE,
            "{what}: {channel} was {got}, expected {want} (whole pixel {actual:?} vs {expected:?})"
        );
    }
}

#[test]
fn a_tint_effect_moves_the_picture_towards_its_colour() {
    let Some((_guard, context)) = context_or_skip() else {
        return;
    };
    let sequence = grey_sequence();
    let picture = grey_picture(&context);
    let mut compositor = Compositor::for_sequence(context.clone(), &sequence);
    let tint = tint_effect();

    // Untinted first: the effect runs but at amount 0, so the clip must come
    // back as the grey it went in as. That separates "the tint worked" from
    // "the chain broke the picture".
    let (pixels, summary) = render_chain(
        &mut compositor,
        &sequence,
        &picture,
        &[EffectInstance::new(Arc::clone(&tint))],
    );
    assert!(!summary.has_effect_failures(), "{summary:?}");
    assert_eq!(summary.effects_applied(), 1);
    assert_close("amount 0", centre(&pixels), [GREY, GREY, GREY, 255]);

    // Half way to red, computed in linear light: green and blue fall towards
    // zero and red climbs towards one.
    let half = EffectInstance::new(Arc::clone(&tint))
        .with_value("amount", ParamValue::Float(0.5))
        .with_value("tint", ParamValue::Color([1.0, 0.0, 0.0, 1.0]));
    let (pixels, summary) = render_chain(&mut compositor, &sequence, &picture, &[half]);
    assert!(!summary.has_effect_failures(), "{summary:?}");
    let grey = to_linear(GREY);
    let expected = [
        to_srgb(0.5f32.mul_add(1.0 - grey, grey)),
        to_srgb(grey * 0.5),
        to_srgb(grey * 0.5),
        255,
    ];
    assert_close("amount 0.5", centre(&pixels), expected);

    // All the way: the picture is the tint colour, whatever it went in as.
    let full = EffectInstance::new(Arc::clone(&tint))
        .with_value("amount", ParamValue::Float(1.0))
        .with_value("tint", ParamValue::Color([0.0, 1.0, 0.0, 1.0]));
    let (pixels, _) = render_chain(&mut compositor, &sequence, &picture, &[full]);
    assert_close(
        "amount 1",
        centre(&pixels),
        [to_srgb(0.0), to_srgb(1.0), to_srgb(0.0), 255],
    );

    // One declaration, one pipeline, however many frames it is bound in.
    assert_eq!(compositor.effect_cache().len(), 1);
    assert!(compositor.effect_cache().contains(tint.key()));
}

#[test]
fn a_chain_runs_in_declaration_order() {
    let Some((_guard, context)) = context_or_skip() else {
        return;
    };
    let sequence = grey_sequence();
    let picture = grey_picture(&context);
    let mut compositor = Compositor::for_sequence(context.clone(), &sequence);
    let lift = scalar_effect("lift", LIFT_WGSL, "fs_lift", 1.0, 0.25);
    let gain = scalar_effect("gain", GAIN_WGSL, "fs_gain", 4.0, 0.5);
    let grey = to_linear(GREY);

    let (pixels, summary) = render_chain(
        &mut compositor,
        &sequence,
        &picture,
        &[
            EffectInstance::new(Arc::clone(&lift)),
            EffectInstance::new(Arc::clone(&gain)),
        ],
    );
    assert_eq!(summary.effects_applied(), 2);
    let lifted_then_gained = to_srgb((grey + 0.25) * 0.5);
    assert_close(
        "lift then gain",
        centre(&pixels),
        [
            lifted_then_gained,
            lifted_then_gained,
            lifted_then_gained,
            255,
        ],
    );

    let (pixels, _) = render_chain(
        &mut compositor,
        &sequence,
        &picture,
        &[EffectInstance::new(gain), EffectInstance::new(lift)],
    );
    let gained_then_lifted = to_srgb(0.25f32.mul_add(1.0, grey * 0.5));
    assert_ne!(
        gained_then_lifted, lifted_then_gained,
        "the pair must be order-sensitive for this to prove anything"
    );
    assert_close(
        "gain then lift",
        centre(&pixels),
        [
            gained_then_lifted,
            gained_then_lifted,
            gained_then_lifted,
            255,
        ],
    );
}

#[test]
fn a_shader_that_will_not_compile_disables_only_that_effect() {
    let Some((_guard, context)) = context_or_skip() else {
        return;
    };
    let sequence = grey_sequence();
    let picture = grey_picture(&context);
    let mut compositor = Compositor::for_sequence(context.clone(), &sequence);
    let broken = broken_effect();
    let tint = tint_effect();
    let chain = [
        EffectInstance::new(Arc::clone(&broken)),
        EffectInstance::new(Arc::clone(&tint))
            .with_value("amount", ParamValue::Float(1.0))
            .with_value("tint", ParamValue::Color([0.0, 0.0, 1.0, 1.0])),
    ];

    let (pixels, summary) = render_chain(&mut compositor, &sequence, &picture, &chain);
    assert!(summary.has_effect_failures());
    let failure = &summary.effect_failures[0];
    assert_eq!(failure.failure.index, 0);
    assert_eq!(failure.failure.effect, broken.key());
    assert_eq!(failure.failure.code, "render.effect_compile_failed");
    assert!(
        failure.failure.message.contains("fs_broken"),
        "the message must name the entry point: {}",
        failure.failure.message
    );
    assert!(!failure.failure.message.is_empty());

    // The clip still drew, and the rest of the chain still ran.
    assert_eq!(summary.effects_applied(), 1);
    assert!(!summary.is_blank());
    assert_close(
        "the surviving tint",
        centre(&pixels),
        [to_srgb(0.0), to_srgb(0.0), to_srgb(1.0), 255],
    );

    // The failure is cached: a second frame reports the same message without
    // asking the driver again.
    let (_, second) = render_chain(&mut compositor, &sequence, &picture, &chain);
    assert_eq!(second.effect_failures, summary.effect_failures);
    assert_eq!(compositor.effect_cache().len(), 2);
}

#[test]
fn a_clip_with_no_effects_renders_exactly_as_before() {
    let Some((_guard, context)) = context_or_skip() else {
        return;
    };
    let sequence = grey_sequence();
    let picture = grey_picture(&context);
    let mut compositor = Compositor::for_sequence(context.clone(), &sequence);

    let (with_none, summary) = render_chain(&mut compositor, &sequence, &picture, &[]);
    assert_eq!(summary.effects_applied(), 0);
    assert!(!summary.has_effect_failures());

    let mut source = |_: &ResolvedClip<'_>| Some(SourceFrame::new(picture.clone(), SIZE, SIZE));
    let plain = compositor.render(
        &sequence,
        RationalTime::new(0, Rational::FPS_24),
        &mut source,
    );
    assert_eq!(plain.layers.len(), summary.layers.len());
    assert_eq!(compositor.read_rgba(), with_none);
    assert_close("no effects", centre(&with_none), [GREY, GREY, GREY, 255]);
    assert!(compositor.effect_cache().is_empty());
}
