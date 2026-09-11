//! The pop-out viewer, on the shared `egui_kittest` harness.
//!
//! Two halves, as TASK-67 asks for them. The snapshot paints what the pop-out
//! window shows — the compositor texture the docked viewer paints, letterboxed
//! on black — through the harness's real wgpu render, so the "shares the
//! texture" claim is checked against pixels rather than against a comment. The
//! interaction tests click the Viewer menu item, close the pop-out's window
//! and press a transport key in it, asserting on the model each time.
//!
//! The deferred viewport itself — a second OS window, dragged to a second
//! monitor — is TASK-118's job on real hardware; nothing here creates a
//! window.

mod support;

use eframe::egui;
use egui_kittest::kittest::Queryable;
use sub_time::{Rational, RationalTime};
use sub_ui::popout::{
    CLOSE_LABEL, OPEN_LABEL, PopoutViewer, popout_content_ui, popout_menu_ui,
    popout_viewport_builder, popout_viewport_ui,
};
use sub_ui::shortcuts::{Action, ShortcutMap};
use sub_ui::viewer::{TransportAction, ViewerAction, ViewerFrame, ViewerPanel};

/// The test picture's size in pixels: 16:9, so the fit letterboxes visibly in
/// the harness's 4:3 frame.
const CANVAS: [usize; 2] = [64, 36];

/// A test pattern standing in for the compositor's output.
///
/// Four flat colour bars: enough for the snapshot to show the picture is
/// sampled the right way up and the right way round, and cheap enough that
/// the committed PNG stays tiny.
fn colour_bars() -> egui::ColorImage {
    let bars = [
        egui::Color32::from_rgb(200, 40, 40),
        egui::Color32::from_rgb(40, 200, 40),
        egui::Color32::from_rgb(40, 40, 200),
        egui::Color32::from_rgb(220, 220, 220),
    ];
    let mut pixels = Vec::with_capacity(CANVAS[0] * CANVAS[1]);
    for row in 0..CANVAS[1] {
        for column in 0..CANVAS[0] {
            // The bottom row is dimmed so "upside down" would be obvious.
            let bar = bars[column * bars.len() / CANVAS[0]];
            let colour = if row * 4 >= CANVAS[1] * 3 {
                egui::Color32::from_rgb(bar.r() / 3, bar.g() / 3, bar.b() / 3)
            } else {
                bar
            };
            pixels.push(colour);
        }
    }
    egui::ColorImage::new(CANVAS, pixels)
}

/// What a pop-out test paints with: the pop-out itself, the docked panel it
/// came out of, and the texture standing in for the compositor's output.
struct Scene {
    /// The pop-out under test.
    popout: PopoutViewer,
    /// The docked viewer panel, which gives the picture up while it is out.
    panel: ViewerPanel,
    /// The test picture, uploaded once and then reused, exactly as the
    /// compositor's output is registered once and then reused.
    texture: Option<egui::TextureHandle>,
}

impl Scene {
    /// A scene with the viewer docked and nothing uploaded yet.
    fn new() -> Self {
        let mut panel = ViewerPanel::new(Rational::FPS_24);
        panel
            .state
            .set_duration(RationalTime::new(48, Rational::FPS_24));
        Self {
            popout: PopoutViewer::new(),
            panel,
            texture: None,
        }
    }

    /// The picture, uploading it on the first frame.
    ///
    /// One [`egui::TextureId`] for the whole test, as in the editor: the
    /// docked panel and the pop-out both sample it.
    fn frame(&mut self, ui: &egui::Ui) -> ViewerFrame {
        let texture = self.texture.get_or_insert_with(|| {
            ui.ctx().load_texture(
                "popout-preview",
                colour_bars(),
                egui::TextureOptions::NEAREST,
            )
        });
        ViewerFrame::new(texture.id(), 1920, 1080)
    }
}

#[test]
fn the_pop_out_window_paints_the_compositor_texture() {
    if !support::can_render() {
        return;
    }
    let mut harness = support::panel_harness_state(Scene::new(), |ui, scene| {
        let frame = scene.frame(ui);
        // What the app does once a frame: publish the handle, never a copy.
        scene.popout.publish(frame);
        popout_content_ui(ui, scene.popout.shared().frame());
    });
    // Two passes: the first uploads the texture, the second paints with it.
    harness.run();
    harness.run();
    support::snapshot(&mut harness, "viewer_popout_window");
}

#[test]
fn the_menu_item_pops_the_viewer_out_and_puts_it_back() {
    let mut harness = support::panel_harness_state(PopoutViewer::new(), |ui, popout| {
        popout_menu_ui(ui, popout);
    });
    harness.run();
    assert!(!harness.state().is_open(), "the viewer starts docked");

    harness.get_by_label(OPEN_LABEL).click();
    harness.run();
    assert!(
        harness.state().is_open(),
        "the menu item pops the viewer into its own window"
    );

    harness.get_by_label(CLOSE_LABEL).click();
    harness.run();
    assert!(
        !harness.state().is_open(),
        "and the same item puts it back in the dock"
    );
}

#[test]
fn a_placed_pop_out_opens_on_the_second_monitor() {
    // What the Xvfb window smoke run does with nobody there to drag the
    // window: the second head's origin is set before the pop-out opens, so
    // the first window eframe creates is already on the second monitor
    // (TASK-123). The window itself is TASK-118's job on real hardware.
    let mut placed = PopoutViewer::new();
    placed.set_position([1280.0, 0.0]);
    let mut harness = support::panel_harness_state(placed, |ui, popout| {
        popout_menu_ui(ui, popout);
    });
    harness.run();

    harness.get_by_label(OPEN_LABEL).click();
    harness.run();
    let popout = harness.state();
    assert!(popout.is_open(), "the pop-out opened");
    assert_eq!(
        popout_viewport_builder(popout.placement()).position,
        Some(egui::pos2(1280.0, 0.0)),
        "and it asks for the second monitor rather than wherever the window \
         manager felt like putting it"
    );
}

#[test]
fn closing_the_pop_out_window_returns_the_picture_to_the_dock() {
    let mut harness = support::panel_harness_state(Scene::new(), |ui, scene| {
        let frame = scene.frame(ui);
        scene.popout.publish(frame);
        // The app's frame order: the pop-out's close is picked up first, so
        // the docked panel knows whose picture this frame is.
        scene.panel.popped_out = scene.popout.poll_close();
        scene.panel.ui(ui, Some(frame));
        popout_menu_ui(ui, &mut scene.popout);
    });
    harness.run();
    assert!(
        !harness.state().panel.popped_out,
        "the docked panel paints the picture to start with"
    );

    harness.get_by_label(OPEN_LABEL).click();
    harness.run();
    let scene = harness.state();
    assert!(scene.panel.popped_out, "the picture moved to the pop-out");
    let published = scene.popout.shared().frame();
    assert_eq!(
        published.map(|frame| frame.texture),
        scene.texture.as_ref().map(egui::TextureHandle::id),
        "and it is the same texture, not a second render"
    );

    // What the window manager's close button does.
    harness.state().popout.shared().request_close();
    harness.run();
    let scene = harness.state();
    assert!(
        !scene.popout.is_open(),
        "closing the pop-out window closes the pop-out"
    );
    assert!(
        !scene.panel.popped_out,
        "and the viewer paints in the dock again"
    );
}

#[test]
fn playback_keys_pressed_in_the_pop_out_reach_the_transport() {
    let popout = PopoutViewer::new();
    popout.publish(ViewerFrame::new(egui::TextureId::User(1), 1920, 1080));
    popout
        .shared()
        .set_shortcuts(&sub_ui::popout::playback_shortcuts(
            &ShortcutMap::default_map(),
        ));

    // One pass of the pop-out window, with the keys a user pressed in it.
    let ctx = egui::Context::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(960.0, 540.0),
        )),
        events: vec![
            key(egui::Key::ArrowRight),
            key(egui::Key::ArrowRight),
            key(egui::Key::Space),
            key(egui::Key::Z),
        ],
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| popout_viewport_ui(ui, popout.shared()));
    let _ = ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
    output.textures_delta.clear();

    // And the main pass applying them, as the app does.
    let mut panel = ViewerPanel::new(Rational::FPS_24);
    panel
        .state
        .set_duration(RationalTime::new(48, Rational::FPS_24));
    let mut scheduler = sub_edit::playback::PlaybackScheduler::new(Rational::FPS_24);
    let actions = popout.take_actions();
    assert_eq!(
        actions,
        vec![
            Action::StepForward,
            Action::StepForward,
            Action::TogglePlayback
        ],
        "the pop-out claims the playback keys and leaves the rest alone"
    );
    for action in actions {
        if let Some(viewer) = ViewerAction::for_action(action) {
            panel.state.apply(viewer);
        } else if let Some(transport) = TransportAction::for_action(action) {
            transport.apply(&mut scheduler);
        }
    }
    assert_eq!(
        panel.state.playhead(),
        RationalTime::new(2, Rational::FPS_24),
        "two frame steps in the pop-out moved the playhead twice"
    );
    assert!(scheduler.is_playing(), "and the space bar started playback");
}

/// A key press, as egui reports one.
fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}
