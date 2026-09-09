//! Golden test for multi-track compositing: two overlapping colour-bar clips
//! at 50 % opacity.
//!
//! A Rec.709-style colour-bar picture is laid on a bottom video track and a
//! rotated copy of the same bars on the track above it, both clips covering
//! the playhead and both at half opacity. The composite is read back and
//! every bar is compared against the reference the blend is *defined* by:
//! straight `over` arithmetic in linear light, computed here on the CPU from
//! the bars' own sRGB codes.
//!
//! That reference is what makes this a golden rather than a smoke test. A
//! stack drawn in the wrong order, a layer whose opacity was dropped, a blend
//! done on encoded rather than linear values, or a second layer that
//! overwrote the first instead of blending with it all land far outside the
//! tolerance, on a different bar each time.
//!
//! Where the machine has no wgpu adapter at all — a container with no ICD —
//! the test reports that and passes rather than failing the build on an
//! environment problem. Like `nv12_golden.rs`, one context is shared and only
//! one test touches the driver at a time: Mesa's lavapipe has segfaulted when
//! several threads drive devices at once.

use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use sub_model::{
    Clip, ClipId, ColorTags, MediaId, Opacity, Resolution, Sequence, SequenceSettings, Track,
    TrackKind, Transform,
};
use sub_render::{Compositor, RenderContext, RenderError, ResolvedClip, SourceFrame};
use sub_time::{Rational, RationalTime, TimeRange};

/// How far a composited channel may sit from its reference value.
///
/// Three codes of slack absorb the 8-bit quantisation of the sRGB target and
/// the small differences between GPUs' `pow` implementations; a mis-ordered
/// or unblended layer is off by tens of codes.
const TOLERANCE: i32 = 3;

/// Width of one bar, in both source texels and canvas pixels.
const BAR: u32 = 8;

/// Canvas and source height. The bars run the full height.
const HEIGHT: u32 = 32;

/// How far the upper track's bars are rotated from the lower track's, so
/// every column blends a different pair of colours.
const ROTATION: usize = 4;

/// The bars, as the sRGB codes a viewer would see: the 100 % Rec.709 bars
/// plus a mid grey, which exercises the middle of the transfer curve rather
/// than only its clipped ends.
const BARS: &[(&str, [u8; 3])] = &[
    ("white", [255, 255, 255]),
    ("yellow", [255, 255, 0]),
    ("cyan", [0, 255, 255]),
    ("green", [0, 255, 0]),
    ("magenta", [255, 0, 255]),
    ("red", [255, 0, 0]),
    ("blue", [0, 0, 255]),
    ("grey", [128, 128, 128]),
    ("black", [0, 0, 0]),
];

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

/// Canvas width: one bar per colour.
fn width() -> u32 {
    BAR * u32::try_from(BARS.len()).expect("nine bars fit in a u32")
}

/// The bar the upper track shows above column `index` of the lower track's.
fn rotated(index: usize) -> usize {
    (index + ROTATION) % BARS.len()
}

/// Decode one sRGB code to linear light, the way the sampler does when it
/// reads an `Rgba8UnormSrgb` texel.
fn to_linear(code: u8) -> f32 {
    let value = f32::from(code) / 255.0;
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// Encode linear light back to an sRGB code, the way the target does on
/// store.
///
/// The cast is bounded by the clamp either side of it: the value is a
/// rounded 0..=255.
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

/// The reference composite of `layers` — each an sRGB colour and its alpha,
/// listed bottom-up — over opaque black.
///
/// Plain `over` in linear light, which is what premultiplied blending on a
/// linear target computes.
fn blend_over_black(layers: &[([u8; 3], f32)]) -> [u8; 4] {
    let mut linear = [0.0f32; 3];
    for (colour, alpha) in layers {
        for (channel, code) in linear.iter_mut().zip(*colour) {
            *channel = alpha.mul_add(to_linear(code), (1.0 - alpha) * *channel);
        }
    }
    let [r, g, b] = linear;
    [to_srgb(r), to_srgb(g), to_srgb(b), 255]
}

/// A colour-bar picture whose bars start at `offset` in [`BARS`], as an sRGB
/// texture the size of the canvas.
///
/// Source and canvas are the same size and the clip carries no transform, so
/// each canvas pixel samples exactly one texel centre: no filtering slack
/// enters the comparison except at the bar boundaries, which are not probed.
fn colour_bars(context: &RenderContext, offset: usize) -> wgpu::TextureView {
    let width = width();
    let texture = context.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("colour bars"),
        size: wgpu::Extent3d {
            width,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut pixels = Vec::with_capacity((width * HEIGHT * 4) as usize);
    for _ in 0..HEIGHT {
        for x in 0..width {
            let bar = (x / BAR) as usize;
            let (_, colour) = BARS[(bar + offset) % BARS.len()];
            pixels.extend_from_slice(&colour);
            pixels.push(255);
        }
    }
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
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(HEIGHT),
        },
        wgpu::Extent3d {
            width,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// A sequence of two video tracks, each holding one ten-frame clip at
/// `opacity` covering the whole canvas. Returns the clip ids bottom-up.
fn two_bar_tracks(opacity: Opacity) -> (Sequence, [ClipId; 2]) {
    let settings = SequenceSettings::new(
        Resolution::new(width(), HEIGHT).expect("the bar canvas is non-zero"),
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

    let mut sequence = Sequence::new("Bars", settings);
    let mut ids = Vec::with_capacity(2);
    for name in ["V1", "V2"] {
        let mut clip = Clip::new("bars", MediaId::new(), source_range);
        clip.opacity = opacity;
        clip.transform = Transform::IDENTITY;
        ids.push(clip.id);
        let mut track = Track::new(name, TrackKind::Video);
        track.items.push(clip.into());
        sequence.tracks.push(track);
    }
    (sequence, [ids[0], ids[1]])
}

/// Render `sequence` at frame 0, giving the lower clip the plain bars and the
/// upper clip the rotated ones, and read the canvas back.
fn composite_bars(
    context: &RenderContext,
    sequence: &Sequence,
    clips: [ClipId; 2],
) -> (Vec<u8>, sub_render::FrameSummary) {
    let device = context.device();
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let lower = colour_bars(context, 0);
    let upper = colour_bars(context, ROTATION);
    let mut compositor = Compositor::for_sequence(context.clone(), sequence);
    let mut source = |resolved: &ResolvedClip<'_>| {
        let view = if resolved.clip_id() == clips[0] {
            lower.clone()
        } else if resolved.clip_id() == clips[1] {
            upper.clone()
        } else {
            panic!("the source was asked for an unknown clip");
        };
        Some(SourceFrame::new(view, width(), HEIGHT))
    };
    let summary = compositor.render(
        sequence,
        RationalTime::new(0, Rational::FPS_24),
        &mut source,
    );
    let pixels = compositor.read_rgba();

    let error = pollster::block_on(error_scope.pop());
    assert!(error.is_none(), "compositing the bars raised {error:?}");
    (pixels, summary)
}

/// The pixel at `(x, y)` of a tightly packed RGBA readback.
fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * width() + x) * 4) as usize;
    pixels[offset..offset + 4]
        .try_into()
        .expect("four bytes per pixel")
}

/// Compare the interior of every bar against `expected`, which is given the
/// bar's index.
fn assert_bars(pixels: &[u8], what: &str, expected: impl Fn(usize) -> [u8; 4]) {
    for (index, (name, _)) in BARS.iter().enumerate() {
        let bar = u32::try_from(index).expect("nine bars fit in a u32");
        let reference = expected(index);
        // Two columns in from each edge of the bar, so a boundary column
        // shared with the neighbour is never probed.
        for x in [bar * BAR + 2, bar * BAR + BAR - 3] {
            for y in [1, HEIGHT / 2, HEIGHT - 2] {
                let actual = pixel(pixels, x, y);
                let close = actual
                    .iter()
                    .zip(reference)
                    .all(|(&a, e)| (i32::from(a) - i32::from(e)).abs() <= TOLERANCE);
                assert!(
                    close,
                    "{what}: {name} bar at ({x}, {y}) is {actual:?}, expected {reference:?} \
                     (+-{TOLERANCE})"
                );
            }
        }
    }
}

#[test]
fn two_half_opacity_bar_clips_blend_to_the_reference_composite() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    let half = Opacity::from_f64(0.5).expect("half opacity is valid");
    let (sequence, clips) = two_bar_tracks(half);
    let (pixels, summary) = composite_bars(&context, &sequence, clips);

    assert_eq!(pixels.len(), (width() * HEIGHT * 4) as usize);
    assert_eq!(summary.drawn(), 2, "both tracks drew a layer");
    assert_eq!(
        summary
            .layers
            .iter()
            .map(|layer| layer.clip)
            .collect::<Vec<_>>(),
        clips.to_vec(),
        "layers are reported bottom-up"
    );

    // Half the lower bars over black, then half the upper bars over that.
    assert_bars(&pixels, "two half-opacity bar clips", |index| {
        blend_over_black(&[(BARS[index].1, 0.5), (BARS[rotated(index)].1, 0.5)])
    });
}

#[test]
fn the_upper_bars_alone_survive_at_full_opacity_and_none_when_muted() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };

    // Opaque: the top track covers the bottom one completely, so the
    // composite is exactly the rotated bars.
    let (sequence, clips) = two_bar_tracks(Opacity::OPAQUE);
    let (pixels, _) = composite_bars(&context, &sequence, clips);
    assert_bars(&pixels, "an opaque top track", |index| {
        blend_over_black(&[(BARS[rotated(index)].1, 1.0)])
    });

    // Mute it and the lower bars come back untouched: a muted video track
    // contributes nothing at all.
    let mut muted = sequence.clone();
    muted.tracks[1].muted = true;
    let (pixels, summary) = composite_bars(&context, &muted, clips);
    assert_eq!(summary.drawn(), 1);
    assert_bars(&pixels, "a muted top track", |index| {
        blend_over_black(&[(BARS[index].1, 1.0)])
    });
}
