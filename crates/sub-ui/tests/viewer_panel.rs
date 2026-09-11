//! The viewer panel over the committed sample project, on the shared
//! `egui_kittest` harness (TASK-122).
//!
//! Two halves, as the task asks for them.
//!
//! The snapshot is the whole picture path end to end: the sample project's
//! first sequence is composited at frame 10 by the software compositor in
//! `sub-render`, the canvas is read back, uploaded as an egui texture and
//! painted by [`ViewerPanel`] with the playhead parked on that frame, so the
//! committed PNG shows the picture letterboxed on black under the transport
//! row with the timecode readout on `00:00:00:10`. Two devices are in play —
//! the harness renders the UI on its own wgpu device — so the canvas travels
//! between them the only way it can, through a readback and an upload; the
//! editor itself shares one device and samples [`sub_render::Compositor`]'s
//! texture directly.
//!
//! The fixture's media paths are documentation, not files, so the layers are
//! given a generated test picture rather than decoded frames: what is under
//! test here is the viewer, and the decode path has its own tests in
//! `sample_project_render.rs`. The canvas is dropped to 640x360 — the same
//! 16:9 the fixture's 3840x2160 is — to keep the readback and the committed
//! snapshot small.
//!
//! The interaction half drags the scrub bar with a real pointer and asserts
//! on the playhead afterwards, which is view state and so is asserted on the
//! panel rather than on a command.

mod support;

use eframe::egui;
use eframe::wgpu;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use sub_model::sequence::{Resolution, Sequence};
use sub_render::nv12::OUTPUT_FORMAT;
use sub_render::{Compositor, RenderContext, ResolvedClip, SourceFrame};
use sub_time::RationalTime;
use sub_ui::viewer::{ViewerFrame, ViewerPanel};

/// The frame the snapshot shows, as the acceptance criterion names it.
const SNAPSHOT_FRAME: i64 = 10;

/// What the timecode readout must say at that frame, at the fixture's
/// 24000/1001 timebase.
const SNAPSHOT_TIMECODE: &str = "00:00:00:10";

/// The canvas the snapshot composites at: the fixture's aspect, small enough
/// to read back and commit cheaply.
const CANVAS: [u32; 2] = [640, 360];

/// The size of the generated picture the layers are drawn from.
const PICTURE: [u32; 2] = [64, 36];

/// How deep into the scrub bar a pointer aims, in points.
///
/// The bar is the bottom-most thing the panel allocates, so a point just
/// above the bottom of the used area is inside it whatever height the bar is
/// given.
const BAR_DEPTH: f32 = 3.0;

/// The sample project's first sequence, composited at [`CANVAS`].
fn preview_sequence() -> Sequence {
    let project = support::fixture_project();
    let mut sequence = support::fixture_sequence(&project).clone();
    sequence.settings.resolution =
        Resolution::new(CANVAS[0], CANVAS[1]).expect("a valid preview canvas");
    sequence
}

/// A generated picture standing in for a decoded frame: a warm horizontal
/// ramp with a darker lower band, so a flipped or mis-sampled composite is
/// obvious in the snapshot.
fn test_picture(context: &RenderContext) -> wgpu::Texture {
    let width = PICTURE[0] as usize;
    let height = PICTURE[1] as usize;
    let mut pixels = Vec::with_capacity(width * height * 4);
    for row in 0..height {
        for column in 0..width {
            let ramp = u8::try_from(column * 255 / (width - 1)).unwrap_or(u8::MAX);
            // The bottom quarter is dimmed, so an upside-down sample shows.
            let scale = if row * 4 >= height * 3 { 3 } else { 1 };
            pixels.extend_from_slice(&[ramp / scale, (255 - ramp) / scale, 90 / scale, 255]);
        }
    }
    let extent = wgpu::Extent3d {
        width: PICTURE[0],
        height: PICTURE[1],
        depth_or_array_layers: 1,
    };
    let texture = context.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("viewer test picture"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OUTPUT_FORMAT,
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
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(PICTURE[0] * 4),
            rows_per_image: Some(PICTURE[1]),
        },
        extent,
    );
    texture
}

/// Composites `sequence` at `time` and hands the canvas back as an egui
/// image, or `None` when this machine has no wgpu adapter.
fn composite(sequence: &Sequence, time: RationalTime) -> Option<egui::ColorImage> {
    let context = RenderContext::headless().ok()?;
    let picture = test_picture(&context);
    let view = picture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut compositor = Compositor::for_sequence(context, sequence);
    let mut source =
        |_clip: &ResolvedClip<'_>| Some(SourceFrame::new(view.clone(), PICTURE[0], PICTURE[1]));
    let summary = compositor.render(sequence, time, &mut source);
    assert!(
        !summary.is_blank(),
        "the sample project draws a layer at frame {SNAPSHOT_FRAME}"
    );
    let rgba = compositor.read_rgba();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [CANVAS[0] as usize, CANVAS[1] as usize],
        &rgba,
    ))
}

/// What a painted frame needs and what the drag is asserted on.
struct Scene {
    /// The panel under test.
    panel: ViewerPanel,
    /// The composited canvas, or `None` when the test paints no picture.
    canvas: Option<egui::ColorImage>,
    /// The uploaded canvas, uploaded once and then reused as the editor does.
    texture: Option<egui::TextureHandle>,
    /// The area the panel last took, which the scrub bar sits at the foot of.
    used: egui::Rect,
    /// Whether any frame reported the playhead moving.
    moved: bool,
}

impl Scene {
    /// A scene over the sample sequence, with `canvas` as its picture.
    fn new(canvas: Option<egui::ColorImage>) -> Self {
        Self {
            panel: ViewerPanel::for_sequence(&preview_sequence()),
            canvas,
            texture: None,
            used: egui::Rect::NOTHING,
            moved: false,
        }
    }

    /// The picture for this frame, uploading it on the first pass.
    fn frame(&mut self, ui: &egui::Ui) -> Option<ViewerFrame> {
        let canvas = self.canvas.clone()?;
        let texture = self.texture.get_or_insert_with(|| {
            ui.ctx()
                .load_texture("viewer-canvas", canvas, egui::TextureOptions::LINEAR)
        });
        Some(ViewerFrame::new(texture.id(), CANVAS[0], CANVAS[1]))
    }

    /// A point inside the scrub bar, `fraction` of the way along it.
    fn bar_point(&self, fraction: f32) -> egui::Pos2 {
        egui::pos2(
            self.used.left() + self.used.width() * fraction,
            self.used.bottom() - BAR_DEPTH,
        )
    }
}

/// A harness painting the viewer over `scene`.
fn harness(scene: Scene) -> Harness<'static, Scene> {
    support::panel_harness_state(scene, |ui, scene| {
        let frame = scene.frame(ui);
        scene.moved |= scene.panel.ui(ui, frame);
        scene.used = ui.min_rect();
    })
}

#[test]
fn the_viewer_shows_frame_ten_of_the_sample_project() {
    if !support::can_render() {
        return;
    }
    let sequence = preview_sequence();
    let time = RationalTime::new(SNAPSHOT_FRAME, sequence.settings.frame_rate);
    let Some(canvas) = composite(&sequence, time) else {
        eprintln!("skipping: the compositor has no adapter to render on");
        return;
    };

    let mut scene = Scene::new(Some(canvas));
    assert!(
        scene.panel.state.seek_to(time),
        "the playhead moves to frame {SNAPSHOT_FRAME}"
    );
    assert_eq!(
        scene.panel.state.timecode_label(),
        SNAPSHOT_TIMECODE,
        "the readout names the frame the picture was composited at"
    );
    // Nothing may move the playhead off the frame being photographed.
    scene.panel.keyboard = false;

    let mut harness = harness(scene);
    // Two passes: the first uploads the canvas, the second paints with it.
    harness.run();
    harness.run();
    assert!(
        harness.query_by_label(SNAPSHOT_TIMECODE).is_some(),
        "the timecode overlay is in the painted tree"
    );
    assert_eq!(
        harness.state().panel.state.playhead_frame(),
        SNAPSHOT_FRAME,
        "and painting moved nothing"
    );
    support::snapshot(&mut harness, "viewer_frame_ten");
}

#[test]
fn dragging_the_scrub_bar_moves_the_playhead() {
    let mut harness = harness(Scene::new(None));
    harness.run();
    assert_eq!(
        harness.state().panel.state.playhead_frame(),
        0,
        "the viewer opens on the first frame"
    );
    let last = harness.state().panel.state.last_frame_number();
    assert!(last > 0, "the sample sequence is longer than one frame");

    // Press near the head of the bar, then move: egui only reports a drag
    // once the pointer has gone somewhere while the button is down.
    let start = harness.state().bar_point(0.1);
    harness.hover_at(start);
    harness.run();
    harness.drag_at(start);
    harness.run();

    let middle = harness.state().bar_point(0.5);
    harness.hover_at(middle);
    harness.run();

    let halfway = harness.state().panel.state.playhead_frame();
    assert!(
        (halfway - last / 2).abs() <= 1,
        "dragging to the middle of the bar put the playhead at the middle of \
         the sequence: frame {halfway} of {last}"
    );
    assert!(
        harness.state().moved,
        "and the panel told its caller to composite again"
    );

    // Still holding the button, further along the bar: the playhead follows
    // the pointer rather than waiting for the release.
    let later = harness.state().bar_point(0.8);
    harness.hover_at(later);
    harness.run();
    let dragged = harness.state().panel.state.playhead_frame();
    assert!(
        dragged > halfway,
        "the playhead followed the drag: {dragged} after {halfway}"
    );

    harness.drop_at(later);
    harness.run();
    assert_eq!(
        harness.state().panel.state.playhead_frame(),
        dragged,
        "releasing leaves the playhead where the drag left it"
    );
}

#[test]
fn the_timecode_readout_follows_the_playhead() {
    let mut harness = harness(Scene::new(None));
    harness.run();
    assert!(
        harness.query_by_label("00:00:00:00").is_some(),
        "the readout starts at the head of the sequence"
    );

    harness.state_mut().panel.state.go_to_end();
    harness.run();
    let end = harness.state().panel.state.timecode_label();
    assert!(
        harness.query_by_label(&end).is_some(),
        "and reads out the last frame once the playhead is there: {end}"
    );
}
