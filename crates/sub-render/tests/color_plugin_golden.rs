//! Golden render test for the first-party `effect` plugin, `plugins/color`.
//!
//! The plugin declares a colour grade — a tint colour and its amount, exposure
//! in stops and saturation — and ships one WGSL fragment entry point. This
//! test runs *that* declaration through the compositor: the shader is the
//! plugin's own `src/effect.wgsl`, read here with `include_str!`, and the
//! parameters come from the plugin's own `src/grade.rs`, included as a module
//! with `#[path]`. Neither pulls the plugin's crate into this workspace — it
//! targets `wasm32-wasip2` and keeps a workspace of its own — but both mean a
//! parameter renamed, re-ranged or dropped in the plugin fails here rather
//! than at install time.
//!
//! What is compared is a readback of the composited canvas against
//! [`grade::Grade::apply`], computed in *linear* light: the compositor samples
//! an sRGB texture, so the shader works on decoded values, and the sRGB target
//! encodes again on store. A grade that never ran, a uniform whose bytes
//! landed in the wrong slot (the tint is a `vec4` between two floats, so a
//! packing mistake is visible), or a shader that applied its steps in another
//! order all land far outside the tolerance.
//!
//! Where the machine has no wgpu adapter at all the test reports that and
//! passes rather than failing the build on an environment problem, and one
//! context is shared under a mutex, as in `effect_golden.rs`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use sub_model::{
    Clip, ColorTags, MediaId, Resolution, Sequence, SequenceSettings, Track, TrackKind, Transform,
};
use sub_render::{
    Compositor, EffectDesc, EffectInstance, EffectParam, ParamKind, ParamValue, RenderContext,
    RenderError, ResolvedClip, SourceFrame,
};
use sub_time::{Rational, RationalTime, TimeRange};

/// The plugin's parameter table and its CPU reference, compiled straight out
/// of the plugin's source tree so there is only one of each.
#[path = "../../../plugins/color/src/grade.rs"]
mod grade;

/// The plugin's shader, likewise: this is the file the component ships.
const SHADER: &str = include_str!("../../../plugins/color/src/effect.wgsl");

/// The fragment entry point declared in that file.
const ENTRY: &str = "fs_main";

/// How far a channel may sit from its reference code. One more than the
/// effect suite's, because the grade is three operations deep and the readback
/// is 8-bit.
const TOLERANCE: i32 = 4;

/// Canvas and source size.
const SIZE: u32 = 16;

/// The source picture's sRGB codes: a colour with three different channels, so
/// a tint or a desaturation that acts on the wrong one cannot hide.
const SOURCE: [u8; 3] = [64, 128, 192];

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

/// The plugin's declaration as the host builds it: the same lift `describe`
/// does in `plugins/color/src/lib.rs`, into the compositor's vocabulary rather
/// than the WIT one.
fn color_effect() -> Arc<EffectDesc> {
    let params = grade::PARAMS
        .iter()
        .map(|spec| {
            let kind = match spec.kind {
                grade::SpecKind::Float { min, max, default } => ParamKind::Float {
                    min,
                    max,
                    default,
                    step: None,
                },
                grade::SpecKind::Color { default } => ParamKind::Color { default },
            };
            EffectParam::new(spec.id, spec.label, kind).with_doc(spec.doc)
        })
        .collect();
    Arc::new(
        EffectDesc::new(params, SHADER, ENTRY).expect("the plugin's declaration is a valid one"),
    )
}

/// The values of one [`grade::Grade`] as the host would bind them.
fn instance(desc: &Arc<EffectDesc>, grade: grade::Grade) -> EffectInstance {
    EffectInstance::new(Arc::clone(desc))
        .with_value(grade::TINT, ParamValue::Color(grade.tint))
        .with_value(grade::TINT_AMOUNT, ParamValue::Float(grade.tint_amount))
        .with_value(grade::EXPOSURE, ParamValue::Float(grade.exposure))
        .with_value(grade::SATURATION, ParamValue::Float(grade.saturation))
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

/// What [`SOURCE`] must come back as once `grade` has been applied: decode,
/// grade in linear light, encode.
fn expected(grade: grade::Grade) -> [u8; 4] {
    let linear = SOURCE.map(to_linear);
    let graded = grade.apply(linear);
    [
        to_srgb(graded[0]),
        to_srgb(graded[1]),
        to_srgb(graded[2]),
        255,
    ]
}

/// A flat [`SOURCE`] picture the size of the canvas, as an sRGB texture.
fn source_picture(context: &RenderContext) -> wgpu::TextureView {
    let texture = context.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("flat source"),
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
    let texel = [SOURCE[0], SOURCE[1], SOURCE[2], 255];
    let pixels: Vec<u8> = std::iter::repeat_n(texel, (SIZE * SIZE) as usize)
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
fn flat_sequence() -> Sequence {
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
    let mut clip = Clip::new("flat", MediaId::new(), source_range);
    clip.transform = Transform::IDENTITY;
    let mut track = Track::new("V1", TrackKind::Video);
    track.items.push(clip.into());
    let mut sequence = Sequence::new("Colour", settings);
    sequence.tracks.push(track);
    sequence
}

/// Render the clip with `chain` applied and read the canvas back.
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
fn the_declaration_the_plugin_ships_is_one_the_host_accepts() {
    // No GPU needed: this is the validation half of the contract, and it is
    // the check that catches a parameter id that is not a WGSL identifier or a
    // default that has drifted outside its range.
    let effect = color_effect();
    assert_eq!(effect.params().len(), grade::PARAMS.len());
    assert_eq!(effect.entry(), ENTRY);
    // One 16-byte slot per parameter, in declaration order.
    assert_eq!(effect.layout().size(), 16 * grade::PARAMS.len() as u64);
    for (index, field) in effect.layout().fields().iter().enumerate() {
        assert_eq!(field.id, grade::PARAMS[index].id);
        assert_eq!(field.offset, 16 * index as u64);
    }
    // The declared defaults are the identity grade, so dropping the effect on
    // a clip changes nothing until a control is moved.
    let defaults: BTreeMap<String, ParamValue> = effect.defaults();
    assert_eq!(
        defaults.get(grade::SATURATION),
        Some(&ParamValue::Float(1.0))
    );
    assert_eq!(
        defaults.get(grade::TINT_AMOUNT),
        Some(&ParamValue::Float(0.0))
    );
    assert_eq!(defaults.get(grade::EXPOSURE), Some(&ParamValue::Float(0.0)));
    assert_eq!(
        defaults.get(grade::TINT),
        Some(&ParamValue::Color([1.0, 1.0, 1.0, 1.0]))
    );
}

#[test]
fn the_plugins_shader_grades_exactly_as_its_own_reference_does() {
    let Some((_guard, context)) = context_or_skip() else {
        return;
    };
    let sequence = flat_sequence();
    let picture = source_picture(&context);
    let mut compositor = Compositor::for_sequence(context.clone(), &sequence);
    let effect = color_effect();

    // Every case is the plugin's WGSL against the plugin's own CPU reference,
    // so a change to one without the other fails here.
    let cases: [(&str, grade::Grade); 6] = [
        ("defaults", grade::Grade::IDENTITY),
        (
            "one stop down",
            grade::Grade {
                exposure: -1.0,
                ..grade::Grade::IDENTITY
            },
        ),
        (
            "greyscale",
            grade::Grade {
                saturation: 0.0,
                ..grade::Grade::IDENTITY
            },
        ),
        (
            "twice the saturation",
            grade::Grade {
                saturation: 2.0,
                ..grade::Grade::IDENTITY
            },
        ),
        (
            "a warm tint, half strength",
            grade::Grade {
                tint: [1.0, 0.6, 0.2, 1.0],
                tint_amount: 0.5,
                ..grade::Grade::IDENTITY
            },
        ),
        (
            "all three at once",
            grade::Grade {
                tint: [0.2, 0.4, 1.0, 1.0],
                tint_amount: 0.8,
                exposure: 1.0,
                saturation: 1.5,
            },
        ),
    ];

    for (what, grade) in cases {
        let (pixels, summary) = render_chain(
            &mut compositor,
            &sequence,
            &picture,
            &[instance(&effect, grade)],
        );
        assert!(!summary.has_effect_failures(), "{what}: {summary:?}");
        assert_eq!(summary.effects_applied(), 1, "{what}");
        assert_close(what, centre(&pixels), expected(grade));
    }

    // The identity grade is the interesting one twice over: it must leave the
    // source exactly where it was, so a chain that quietly damages the picture
    // is not mistaken for a grade.
    let (pixels, _) = render_chain(
        &mut compositor,
        &sequence,
        &picture,
        &[instance(&effect, grade::Grade::IDENTITY)],
    );
    assert_close(
        "defaults are a no-op",
        centre(&pixels),
        [SOURCE[0], SOURCE[1], SOURCE[2], 255],
    );

    // One declaration, one pipeline, however many times it is bound.
    assert_eq!(compositor.effect_cache().len(), 1);
    assert!(compositor.effect_cache().contains(effect.key()));
}

#[test]
fn a_value_outside_a_declared_range_is_clamped_rather_than_rendered() {
    let Some((_guard, context)) = context_or_skip() else {
        return;
    };
    let sequence = flat_sequence();
    let picture = source_picture(&context);
    let mut compositor = Compositor::for_sequence(context.clone(), &sequence);
    let effect = color_effect();

    // A project written by an older build, or a command that never checked:
    // the host clamps into the declared range, so the picture is the one the
    // maximum saturation gives rather than an undefined one.
    let wild = EffectInstance::new(Arc::clone(&effect))
        .with_value(grade::SATURATION, ParamValue::Float(99.0));
    let (pixels, summary) = render_chain(&mut compositor, &sequence, &picture, &[wild]);
    assert!(!summary.has_effect_failures(), "{summary:?}");
    let clamped = grade::Grade {
        saturation: 4.0,
        ..grade::Grade::IDENTITY
    };
    assert_close("clamped saturation", centre(&pixels), expected(clamped));
}
