//! Full-resolution readback on a real (usually software) wgpu device.
//!
//! The export path may not drop a frame, so these tests care about three
//! things the preview never has to: that the pixels handed back are the ones
//! that were drawn, that a pipelined ring returns frames in submission order
//! rather than whichever buffer mapped first, and that the whole thing runs
//! on a worker thread.
//!
//! Where the machine has no wgpu adapter at all — a container with no ICD —
//! the tests report that and pass rather than failing the build on an
//! environment problem. As in the other GPU tests here, one context is shared
//! and only one thread touches the driver at a time: Mesa's lavapipe has
//! segfaulted when several threads drive devices at once.

use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Instant;

use sub_model::{
    Clip, ColorTags, MediaId, Opacity, Resolution, Sequence, SequenceSettings, Track, TrackKind,
    Transform,
};
use sub_render::{
    Compositor, FrameReadback, RenderContext, RenderError, ResolvedClip, SourceFrame,
    padded_row_bytes,
};
use sub_time::{Rational, RationalTime, TimeRange};

/// How far a read-back channel may sit from its reference value: two codes
/// absorb 8-bit rounding through the sRGB target.
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

/// A 1x1 picture of one colour, which composites to a flat canvas.
fn solid_source(context: &RenderContext, colour: [u8; 4]) -> wgpu::TextureView {
    let texture = context.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("solid source"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    context.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &colour,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// A `width` x `height` sequence holding one 100-frame clip on one video
/// track, filling the canvas.
fn one_clip_sequence(width: u32, height: u32) -> Sequence {
    let settings = SequenceSettings::new(
        Resolution::new(width, height).expect("the test canvas is non-zero"),
        Rational::FPS_24,
        48_000,
        ColorTags::REC709,
    )
    .expect("48 kHz is a valid sample rate");
    let source = TimeRange::new(
        RationalTime::new(0, Rational::FPS_24),
        RationalTime::new(100, Rational::FPS_24),
    )
    .expect("a hundred frames is a valid range");
    let mut clip = Clip::new("shot", MediaId::new(), source);
    clip.opacity = Opacity::OPAQUE;
    clip.transform = Transform::IDENTITY;
    let mut track = Track::new("V1", TrackKind::Video);
    track.items.push(clip.into());
    let mut sequence = Sequence::new("Main", settings);
    sequence.tracks.push(track);
    sequence
}

/// Frame `frame` of the sequence rate.
fn at(frame: i64) -> RationalTime {
    RationalTime::new(frame, Rational::FPS_24)
}

/// The pixel at `(x, y)` of a tightly packed RGBA readback.
fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let offset = (y as usize * width as usize + x as usize) * 4;
    pixels[offset..offset + 4]
        .try_into()
        .expect("four bytes per pixel")
}

/// Assert every channel of `actual` is within [`TOLERANCE`] of `expected`.
fn assert_close(actual: [u8; 4], expected: [u8; 4], what: &str) {
    let close = actual
        .iter()
        .zip(expected)
        .all(|(&a, e)| (i32::from(a) - i32::from(e)).abs() <= TOLERANCE);
    assert!(close, "{what}: got {actual:?}, expected {expected:?}");
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

#[test]
fn a_read_back_frame_is_the_frame_that_was_drawn() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    let sequence = one_clip_sequence(64, 48);
    let view = solid_source(&context, RED);
    let mut source = |_: &ResolvedClip<'_>| Some(SourceFrame::new(view.clone(), 64, 48));

    let mut readback = FrameReadback::new(Compositor::for_sequence(context.clone(), &sequence));
    let frame = readback
        .render_frame_to_buffer(&sequence, at(3), &mut source)
        .expect("the frame reads back");

    assert_eq!(frame.width(), 64);
    assert_eq!(frame.height(), 48);
    assert_eq!(frame.time(), at(3));
    assert_eq!(frame.bytes_per_row(), 64 * 4);
    assert_eq!(frame.pixels().len(), 64 * 48 * 4);
    assert_eq!(frame.summary().drawn(), 1);
    assert_close(pixel(frame.pixels(), 64, 32, 24), RED, "the canvas centre");
    assert_eq!(readback.pending(), 0);

    // The pipelined path must agree with the compositor's own blocking read
    // to the byte, or export and preview would disagree about colour.
    let direct = readback.compositor().read_rgba();
    assert_eq!(frame.pixels(), direct.as_slice());
}

#[test]
fn a_canvas_whose_rows_need_padding_comes_back_packed() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    // 50 pixels is 200 bytes a row, which a texture copy pads to 256: the
    // readback has to strip 56 bytes from every row.
    let sequence = one_clip_sequence(50, 30);
    assert_eq!(padded_row_bytes(50), 256);
    let view = solid_source(&context, GREEN);
    let mut source = |_: &ResolvedClip<'_>| Some(SourceFrame::new(view.clone(), 50, 30));

    let mut readback = FrameReadback::new(Compositor::for_sequence(context.clone(), &sequence));
    let frame = readback
        .render_frame_to_buffer(&sequence, at(0), &mut source)
        .expect("the frame reads back");

    assert_eq!(frame.pixels().len(), 50 * 30 * 4);
    for y in 0..30 {
        assert_close(
            pixel(frame.pixels(), 50, 49, y),
            GREEN,
            "the last pixel of a padded row",
        );
    }
}

#[test]
fn pipelined_frames_come_back_in_submission_order() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    let sequence = one_clip_sequence(64, 64);
    let views = [
        solid_source(&context, RED),
        solid_source(&context, GREEN),
        solid_source(&context, BLUE),
    ];
    let colours = [RED, GREEN, BLUE];
    // The picture depends on the frame, so a frame served out of order shows
    // up as the wrong colour rather than as a subtle timing difference.
    let mut source = |clip: &ResolvedClip<'_>| {
        let index = usize::try_from(clip.source_time.value()).expect("a non-negative source time");
        Some(SourceFrame::new(views[index % 3].clone(), 64, 64))
    };

    let mut readback =
        FrameReadback::with_depth(Compositor::for_sequence(context.clone(), &sequence), 3);
    for frame in 0..3 {
        readback
            .submit(&sequence, at(frame), &mut source)
            .expect("the ring has room for three frames");
    }
    assert_eq!(readback.pending(), 3);
    assert!(readback.is_full());

    let frames = readback.drain().expect("the frames read back");
    assert_eq!(frames.len(), 3);
    assert_eq!(readback.pending(), 0);
    for (index, frame) in frames.iter().enumerate() {
        let expected = i64::try_from(index).expect("three frames fit in an i64");
        assert_eq!(frame.time(), at(expected));
        assert_close(
            pixel(frame.pixels(), 64, 32, 32),
            colours[index],
            "a pipelined frame",
        );
    }
}

#[test]
fn a_full_ring_refuses_a_further_frame_rather_than_dropping_one() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    let sequence = one_clip_sequence(32, 32);
    let view = solid_source(&context, BLUE);
    let mut source = |_: &ResolvedClip<'_>| Some(SourceFrame::new(view.clone(), 32, 32));

    let mut readback =
        FrameReadback::with_depth(Compositor::for_sequence(context.clone(), &sequence), 2);
    assert_eq!(readback.depth(), 2);
    for frame in 0..2 {
        readback
            .submit(&sequence, at(frame), &mut source)
            .expect("the ring holds two frames");
    }
    let error = readback
        .submit(&sequence, at(2), &mut source)
        .expect_err("a third frame does not fit");
    assert_eq!(error.code(), "render.readback_ring_full");
    assert!(matches!(error, RenderError::ReadbackRingFull { depth: 2 }));

    // One received frame makes exactly one slot free again.
    let first = readback
        .receive()
        .expect("the oldest frame reads back")
        .expect("a frame was in flight");
    assert_eq!(first.time(), at(0));
    readback
        .submit(&sequence, at(2), &mut source)
        .expect("the freed slot takes the next frame");
    assert_eq!(readback.drain().expect("the rest read back").len(), 2);
    assert!(
        readback
            .receive()
            .expect("an empty ring is not an error")
            .is_none()
    );
}

#[test]
fn a_one_shot_readback_refuses_while_frames_are_in_flight() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    let sequence = one_clip_sequence(32, 32);
    let view = solid_source(&context, RED);
    let mut source = |_: &ResolvedClip<'_>| Some(SourceFrame::new(view.clone(), 32, 32));

    let mut readback = FrameReadback::new(Compositor::for_sequence(context.clone(), &sequence));
    readback
        .submit(&sequence, at(0), &mut source)
        .expect("the first frame is submitted");
    let error = readback
        .render_frame_to_buffer(&sequence, at(1), &mut source)
        .expect_err("a one-shot read cannot jump the queue");
    assert_eq!(error.code(), "render.readback_pending");
    assert!(matches!(error, RenderError::ReadbackPending { pending: 1 }));

    // Draining puts the one-shot path back in business.
    readback.drain().expect("the pending frame reads back");
    readback
        .render_frame_to_buffer(&sequence, at(1), &mut source)
        .expect("an empty ring takes a one-shot read");
}

#[test]
fn the_ring_follows_a_change_of_sequence_resolution() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    let small = one_clip_sequence(32, 32);
    let large = one_clip_sequence(96, 72);
    let view = solid_source(&context, GREEN);
    let mut source = |_: &ResolvedClip<'_>| Some(SourceFrame::new(view.clone(), 32, 32));

    let mut readback = FrameReadback::new(Compositor::for_sequence(context.clone(), &small));
    let first = readback
        .render_frame_to_buffer(&small, at(0), &mut source)
        .expect("the small frame reads back");
    assert_eq!((first.width(), first.height()), (32, 32));

    let grown = readback
        .render_frame_to_buffer(&large, at(1), &mut source)
        .expect("the larger frame reads back");
    assert_eq!((grown.width(), grown.height()), (96, 72));
    assert_eq!(grown.pixels().len(), 96 * 72 * 4);
    assert_eq!(readback.resolution(), large.settings.resolution);
    assert_close(pixel(grown.pixels(), 96, 48, 36), GREEN, "the grown canvas");

    // Shrinking reuses the bigger buffer rather than reallocating, and must
    // still hand back only the pixels the smaller canvas has.
    let shrunk = readback
        .render_frame_to_buffer(&small, at(2), &mut source)
        .expect("the small frame reads back again");
    assert_eq!(shrunk.pixels().len(), 32 * 32 * 4);
    assert_close(
        pixel(shrunk.pixels(), 32, 16, 16),
        GREEN,
        "the small canvas",
    );
}

#[test]
fn readback_runs_on_a_worker_thread() {
    /// Compile-time half of the claim: what the export job owns can move to
    /// its own thread, so the UI thread is never the one that blocks.
    fn assert_send<T: Send>() {}
    assert_send::<FrameReadback>();
    assert_send::<sub_render::FrameBuffer>();

    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    let sequence = one_clip_sequence(64, 64);
    let view = solid_source(&context, BLUE);

    // The whole readback — compositor, ring and map wait — lives on the
    // spawned thread; this thread only joins it.
    let worker = std::thread::spawn(move || {
        let mut source = |_: &ResolvedClip<'_>| Some(SourceFrame::new(view.clone(), 64, 64));
        let mut readback =
            FrameReadback::with_depth(Compositor::for_sequence(context, &sequence), 2);
        for frame in 0..4 {
            if readback.is_full() {
                readback
                    .receive()
                    .expect("the oldest frame reads back")
                    .expect("a frame was in flight");
            }
            readback
                .submit(&sequence, at(frame), &mut source)
                .expect("a slot is free");
        }
        readback.drain().expect("the tail of the export drains")
    });
    let tail = worker.join().expect("the worker thread finishes");

    assert_eq!(tail.len(), 2);
    assert_eq!(tail[0].time(), at(2));
    assert_eq!(tail[1].time(), at(3));
    assert_close(
        pixel(tail[1].pixels(), 64, 32, 32),
        BLUE,
        "the last frame off the worker",
    );
}

/// AC #2's measurement: composite and read back 1080p frames as fast as the
/// machine will, and report frames per second.
///
/// Ignored by default because the number only means anything on real
/// hardware — a software adapter (what CI and this development box have) is
/// orders of magnitude slower, and the 60 fps bar is a discrete-GPU claim.
/// Run it with `cargo test -p sub-render --test readback -- --ignored
/// --nocapture` on a machine with a GPU.
#[test]
#[ignore = "throughput is only meaningful on a discrete GPU"]
fn throughput_on_a_1080p_canvas() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    let sequence = one_clip_sequence(1920, 1080);
    let view = solid_source(&context, RED);
    let mut source = |_: &ResolvedClip<'_>| Some(SourceFrame::new(view.clone(), 1920, 1080));

    let mut readback = FrameReadback::new(Compositor::for_sequence(context.clone(), &sequence));
    // A warm-up frame pays for pipeline compilation and the first allocation.
    readback
        .render_frame_to_buffer(&sequence, at(0), &mut source)
        .expect("the warm-up frame reads back");

    let frames = 60_i64;
    let started = Instant::now();
    let mut received = 0_usize;
    for frame in 0..frames {
        if readback.is_full() {
            readback
                .receive()
                .expect("the oldest frame reads back")
                .expect("a frame was in flight");
            received += 1;
        }
        readback
            .submit(&sequence, at(frame), &mut source)
            .expect("a slot is free");
    }
    received += readback.drain().expect("the tail drains").len();
    let elapsed = started.elapsed();

    assert_eq!(
        received,
        usize::try_from(frames).expect("60 fits in a usize")
    );
    let fps = f64::from(u32::try_from(frames).expect("60 fits in a u32")) / elapsed.as_secs_f64();
    println!(
        "1080p readback: {received} frames in {elapsed:?} = {fps:.1} fps on {}",
        readback.context().describe()
    );
    assert!(fps > 0.0, "the clock moved");
}
