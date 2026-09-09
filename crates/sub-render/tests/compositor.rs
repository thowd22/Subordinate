//! End-to-end tests for the v0 frame graph on a real (usually software) wgpu
//! device.
//!
//! A four-quadrant source picture is composited into a sequence canvas and
//! read back, so orientation, letterboxing, opacity and the transform show up
//! as pixel colours rather than as maths. Each quadrant is a different pure
//! colour, which makes a flipped v axis, a rotation the wrong way round or a
//! swapped basis column an obvious failure.
//!
//! Where the machine has no wgpu adapter at all — a container with no ICD —
//! the tests report that and pass rather than failing the build on an
//! environment problem. As in the other GPU tests here, one context is shared
//! and only one test touches the driver at a time: Mesa's lavapipe has
//! segfaulted when several threads drive devices at once.

use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use sub_model::params::{Fixed6, Point2, Scale2};
use sub_model::{
    Clip, Gap, MediaId, Opacity, Resolution, Sequence, SequenceSettings, Track, TrackKind,
    Transform,
};
use sub_render::{Compositor, RenderContext, RenderError, ResolvedClip, SourceFrame};
use sub_time::{Rational, RationalTime, TimeRange};

/// How far a read-back channel may sit from its reference value. Two codes
/// absorb 8-bit rounding through the sRGB target; a wrong colour is off by
/// tens or hundreds.
const TOLERANCE: i32 = 3;

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

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

/// A 2x2 source picture: red top-left, green top-right, blue bottom-left,
/// white bottom-right.
///
/// Two texels per axis and a linear sampler means everything but the middle
/// band of the drawn quad reads back as one of the four pure colours, so a
/// probe near a corner needs no tolerance for interpolation.
fn quadrant_source(context: &RenderContext) -> wgpu::TextureView {
    let texture = context.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("quadrant source"),
        size: wgpu::Extent3d {
            width: 2,
            height: 2,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut pixels = Vec::with_capacity(16);
    for colour in [RED, GREEN, BLUE, WHITE] {
        pixels.extend_from_slice(&colour);
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
            bytes_per_row: Some(8),
            rows_per_image: Some(2),
        },
        wgpu::Extent3d {
            width: 2,
            height: 2,
            depth_or_array_layers: 1,
        },
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// A sequence of `width` x `height` holding one 10-frame clip on one video
/// track, with the given clip parameters.
fn one_clip_sequence(width: u32, height: u32, opacity: Opacity, transform: Transform) -> Sequence {
    let settings = SequenceSettings::new(
        Resolution::new(width, height).expect("the test canvas is non-zero"),
        Rational::FPS_24,
        48_000,
        sub_model::ColorTags::REC709,
    )
    .expect("48 kHz is a valid sample rate");
    let source = TimeRange::new(
        RationalTime::new(0, Rational::FPS_24),
        RationalTime::new(10, Rational::FPS_24),
    )
    .expect("ten frames is a valid range");
    let mut clip = Clip::new("shot", MediaId::new(), source);
    clip.opacity = opacity;
    clip.transform = transform;
    let mut track = Track::new("V1", TrackKind::Video);
    track.items.push(clip.into());
    let mut sequence = Sequence::new("Main", settings);
    sequence.tracks.push(track);
    sequence
}

/// The pixel at `(x, y)` of a tightly packed RGBA readback.
fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let offset = (y as usize * width as usize + x as usize) * 4;
    pixels[offset..offset + 4]
        .try_into()
        .expect("four bytes per pixel")
}

/// Assert the pixel at `(x, y)` is `expected`.
fn assert_pixel(pixels: &[u8], width: u32, x: u32, y: u32, expected: [u8; 4], what: &str) {
    let actual = pixel(pixels, width, x, y);
    let close = actual
        .iter()
        .zip(expected)
        .all(|(&a, e)| (i32::from(a) - i32::from(e)).abs() <= TOLERANCE);
    assert!(
        close,
        "{what} at ({x}, {y}): got {actual:?}, expected {expected:?} (+-{TOLERANCE})"
    );
}

/// Render one frame of `sequence` with the quadrant picture as the only
/// source, and read the canvas back.
fn composite(
    context: &RenderContext,
    sequence: &Sequence,
    frame: i64,
    source_size: (u32, u32),
) -> (Vec<u8>, sub_render::FrameSummary) {
    let device = context.device();
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let view = quadrant_source(context);
    let mut compositor = Compositor::for_sequence(context.clone(), sequence);
    let mut source =
        |_: &ResolvedClip<'_>| Some(SourceFrame::new(view.clone(), source_size.0, source_size.1));
    let summary = compositor.render(
        sequence,
        RationalTime::new(frame, Rational::FPS_24),
        &mut source,
    );
    let pixels = compositor.read_rgba();

    let error = pollster::block_on(error_scope.pop());
    assert!(error.is_none(), "compositing raised {error:?}");
    assert_eq!(compositor.resolution(), sequence.settings.resolution);
    (pixels, summary)
}

#[test]
fn an_identity_clip_fills_the_canvas_in_source_orientation() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    let sequence = one_clip_sequence(64, 64, Opacity::OPAQUE, Transform::IDENTITY);
    let (pixels, summary) = composite(&context, &sequence, 3, (64, 64));

    assert_eq!(pixels.len(), 64 * 64 * 4);
    assert!(!summary.is_blank());
    assert_eq!(
        summary.source_time,
        Some(RationalTime::new(3, Rational::FPS_24))
    );

    assert_pixel(&pixels, 64, 4, 4, RED, "top-left quadrant");
    assert_pixel(&pixels, 64, 59, 4, GREEN, "top-right quadrant");
    assert_pixel(&pixels, 64, 4, 59, BLUE, "bottom-left quadrant");
    assert_pixel(&pixels, 64, 59, 59, WHITE, "bottom-right quadrant");
}

#[test]
fn a_narrower_source_gets_pillarbox_bars_and_a_wider_one_letterbox_bars() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };

    // A square source on a 128x64 canvas: the picture is 64 wide, centred,
    // with 32 pixels of black either side.
    let sequence = one_clip_sequence(128, 64, Opacity::OPAQUE, Transform::IDENTITY);
    let (pixels, _) = composite(&context, &sequence, 0, (64, 64));
    assert_pixel(&pixels, 128, 4, 32, BLACK, "left pillarbox bar");
    assert_pixel(&pixels, 128, 123, 32, BLACK, "right pillarbox bar");
    assert_pixel(&pixels, 128, 31, 32, BLACK, "the bar reaches the picture");
    assert_pixel(&pixels, 128, 36, 4, RED, "picture, top-left quadrant");
    assert_pixel(&pixels, 128, 91, 4, GREEN, "picture, top-right quadrant");
    assert_pixel(&pixels, 128, 36, 59, BLUE, "picture, bottom-left quadrant");
    // Full height: no letterbox bar where the aspect already matches.
    assert_pixel(&pixels, 128, 91, 0, GREEN, "picture reaches the top edge");
    assert_pixel(
        &pixels,
        128,
        91,
        63,
        WHITE,
        "picture reaches the bottom edge",
    );

    // The same source on a 64x128 canvas letterboxes instead.
    let sequence = one_clip_sequence(64, 128, Opacity::OPAQUE, Transform::IDENTITY);
    let (pixels, _) = composite(&context, &sequence, 0, (64, 64));
    assert_pixel(&pixels, 64, 32, 4, BLACK, "top letterbox bar");
    assert_pixel(&pixels, 64, 32, 123, BLACK, "bottom letterbox bar");
    assert_pixel(&pixels, 64, 4, 36, RED, "picture, top-left quadrant");
    assert_pixel(&pixels, 64, 4, 91, BLUE, "picture, bottom-left quadrant");
    assert_pixel(&pixels, 64, 0, 91, BLUE, "picture reaches the left edge");
}

#[test]
fn opacity_blends_the_clip_towards_the_black_canvas() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    let sequence = one_clip_sequence(
        64,
        64,
        Opacity::from_f64(0.5).expect("half opacity is valid"),
        Transform::IDENTITY,
    );
    let (pixels, _) = composite(&context, &sequence, 0, (64, 64));

    // Blending happens in linear light, because the target is sRGB and the
    // hardware encodes on store: half of linear 1.0 is 0.5, which sRGB
    // encodes as about 188 — not the 128 a naive blend on encoded values
    // would give.
    let half = 188;
    assert_pixel(&pixels, 64, 4, 4, [half, 0, 0, 255], "half-opacity red");
    assert_pixel(&pixels, 64, 59, 4, [0, half, 0, 255], "half-opacity green");
    assert_pixel(
        &pixels,
        64,
        59,
        59,
        [half, half, half, 255],
        "half-opacity white",
    );

    // A fully transparent clip leaves the canvas untouched.
    let sequence = one_clip_sequence(64, 64, Opacity::TRANSPARENT, Transform::IDENTITY);
    let (pixels, summary) = composite(&context, &sequence, 0, (64, 64));
    assert_pixel(&pixels, 64, 32, 32, BLACK, "a transparent clip draws black");
    // It still resolved and was still drawn; only its alpha was zero.
    assert!(!summary.is_blank());
}

#[test]
fn position_scale_and_rotation_place_the_picture() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };

    // Half scale: the picture occupies the middle 32x32 of a 64x64 canvas.
    let halved = Transform::new(
        Point2::ORIGIN,
        Scale2::uniform(Fixed6::from_micros(500_000)).expect("half is a valid scale"),
        Fixed6::ZERO,
    );
    let sequence = one_clip_sequence(64, 64, Opacity::OPAQUE, halved);
    let (pixels, _) = composite(&context, &sequence, 0, (64, 64));
    assert_pixel(&pixels, 64, 4, 4, BLACK, "outside the shrunken picture");
    assert_pixel(&pixels, 64, 20, 20, RED, "shrunken top-left quadrant");
    assert_pixel(&pixels, 64, 43, 43, WHITE, "shrunken bottom-right quadrant");

    // Then move it 16 pixels right and 16 up: y is positive downwards, so up
    // is negative.
    let moved = Transform::new(
        Point2::new(Fixed6::from_units(16), Fixed6::from_units(-16)),
        Scale2::uniform(Fixed6::from_micros(500_000)).expect("half is a valid scale"),
        Fixed6::ZERO,
    );
    let sequence = one_clip_sequence(64, 64, Opacity::OPAQUE, moved);
    let (pixels, _) = composite(&context, &sequence, 0, (64, 64));
    assert_pixel(&pixels, 64, 36, 4, RED, "moved top-left quadrant");
    assert_pixel(&pixels, 64, 59, 27, WHITE, "moved bottom-right quadrant");
    assert_pixel(&pixels, 64, 20, 43, BLACK, "where the picture used to be");

    // A quarter turn clockwise takes the top-left quadrant to the top right.
    let turned = Transform::new(Point2::ORIGIN, Scale2::UNIFORM, Fixed6::from_units(90));
    let sequence = one_clip_sequence(64, 64, Opacity::OPAQUE, turned);
    let (pixels, _) = composite(&context, &sequence, 0, (64, 64));
    assert_pixel(&pixels, 64, 59, 4, RED, "red turned to the top right");
    assert_pixel(
        &pixels,
        64,
        59,
        59,
        GREEN,
        "green turned to the bottom right",
    );
    assert_pixel(&pixels, 64, 4, 59, WHITE, "white turned to the bottom left");
    assert_pixel(&pixels, 64, 4, 4, BLUE, "blue turned to the top left");
}

#[test]
fn a_gap_under_the_playhead_composites_to_black() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    let mut sequence = one_clip_sequence(64, 64, Opacity::OPAQUE, Transform::IDENTITY);
    sequence.tracks[0]
        .items
        .push(Gap::new(RationalTime::new(5, Rational::FPS_24)).into());

    let (pixels, summary) = composite(&context, &sequence, 12, (64, 64));
    assert!(summary.is_blank());
    assert_eq!(summary.clip, None);
    assert_eq!(summary.source_time, None);
    for (x, y) in [(0, 0), (32, 32), (63, 63)] {
        assert_pixel(&pixels, 64, x, y, BLACK, "a gap is black");
    }
}

#[test]
fn the_canvas_follows_the_sequence_resolution() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    let sequence = one_clip_sequence(64, 64, Opacity::OPAQUE, Transform::IDENTITY);
    let view = quadrant_source(&context);
    let mut compositor = Compositor::for_sequence(context.clone(), &sequence);
    let mut source = |_: &ResolvedClip<'_>| Some(SourceFrame::new(view.clone(), 64, 64));

    compositor.render(
        &sequence,
        RationalTime::new(0, Rational::FPS_24),
        &mut source,
    );
    assert_eq!(compositor.output().width(), 64);
    assert_eq!(compositor.read_rgba().len(), 64 * 64 * 4);

    // Change the sequence settings and render again: the target follows with
    // no separate call, which is what the export path relies on.
    let mut wider = sequence.clone();
    wider.settings = SequenceSettings::new(
        Resolution::new(96, 48).expect("the wider canvas is non-zero"),
        wider.settings.frame_rate,
        wider.settings.sample_rate,
        wider.settings.color,
    )
    .expect("the sample rate is unchanged");
    compositor.render(&wider, RationalTime::new(0, Rational::FPS_24), &mut source);

    assert_eq!(compositor.resolution().width(), 96);
    assert_eq!(compositor.resolution().height(), 48);
    assert_eq!(compositor.output().width(), 96);
    assert_eq!(compositor.output().height(), 48);
    let pixels = compositor.read_rgba();
    assert_eq!(pixels.len(), 96 * 48 * 4);
    // 96x48 is twice as wide as it is tall, so the square picture pillarboxes.
    assert_pixel(&pixels, 96, 4, 24, BLACK, "left pillarbox bar");
    assert_pixel(&pixels, 96, 66, 4, GREEN, "the picture is still centred");
}

#[test]
fn the_target_is_both_sampleable_and_readable() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    let device = context.device();
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let sequence = one_clip_sequence(64, 64, Opacity::OPAQUE, Transform::IDENTITY);
    let view = quadrant_source(&context);
    let mut compositor = Compositor::for_sequence(context.clone(), &sequence);
    let mut source = |_: &ResolvedClip<'_>| Some(SourceFrame::new(view.clone(), 64, 64));
    compositor.render(
        &sequence,
        RationalTime::new(0, Rational::FPS_24),
        &mut source,
    );

    // The egui half of §5.3: bind the target as a sampled texture. wgpu
    // rejects this if TEXTURE_BINDING is missing from its usages.
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("preview sampler"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let target_view = compositor
        .output()
        .create_view(&wgpu::TextureViewDescriptor::default());
    drop(device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("preview sampler"),
        layout: &layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&target_view),
        }],
    }));

    // The export half: the same texture copies out to the CPU.
    let pixels = compositor.read_rgba();
    assert_eq!(pixels.len(), 64 * 64 * 4);
    assert_pixel(&pixels, 64, 4, 4, RED, "the read-back picture");

    let error = pollster::block_on(error_scope.pop());
    assert!(error.is_none(), "sharing the target raised {error:?}");
}
